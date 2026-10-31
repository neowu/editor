//! Starts child processes that are responsible for themselves on macOS.
//!
//! macOS charges a child process to the app that spawned it. When a child
//! outlives the editor (an ssh `ControlPersist` master forked by `git fetch`, a
//! job left running in the terminal), System Settings lists the editor under
//! "Allow in the Background". Disclaiming makes the child its own responsible
//! process, so nothing is attributed to the editor once it quits.
//!
//! A process can only be disclaimed while it is exec'd, so the child re-execs
//! itself with `posix_spawn(POSIX_SPAWN_SETEXEC)` and the disclaim attribute.
//! Commands spawned by the editor do this from a `pre_exec` hook. Spawns the
//! editor does not control (the terminal PTY) go through the editor binary
//! started with [`EXEC_DISCLAIMED_ARG`], which does the same re-exec.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

/// First argument that makes the editor binary exec the rest of its arguments disclaimed.
pub const EXEC_DISCLAIMED_ARG: &str = "--exec-disclaimed";

static TRAMPOLINE: OnceLock<PathBuf> = OnceLock::new();

/// Registers the binary that handles [`EXEC_DISCLAIMED_ARG`]. Until this is
/// called, [`trampoline_command`] returns `None`.
pub fn set_trampoline(path: PathBuf) {
    TRAMPOLINE.set(path).ok();
}

/// Rewrites `program args` to run through the registered trampoline, or
/// `None` when no trampoline is registered.
pub fn trampoline_command(program: String, args: Vec<String>) -> Option<(String, Vec<String>)> {
    TRAMPOLINE.get().map(|trampoline| wrap_command(trampoline, program, args))
}

fn wrap_command(trampoline: &Path, program: String, args: Vec<String>) -> (String, Vec<String>) {
    let mut wrapped_args = Vec::with_capacity(args.len() + 2);
    wrapped_args.push(EXEC_DISCLAIMED_ARG.to_string());
    wrapped_args.push(program);
    wrapped_args.extend(args);
    (trampoline.to_string_lossy().into_owned(), wrapped_args)
}

/// The environment a command will run with: the current process environment
/// (unless cleared) with the command's explicit changes applied.
pub fn child_env<'a>(
    explicit: impl IntoIterator<Item = (&'a OsStr, Option<&'a OsStr>)>,
    cleared: bool,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = if cleared { Vec::new() } else { std::env::vars_os().collect() };
    for (key, value) in explicit {
        env.retain(|(existing, _)| existing != key);
        if let Some(value) = value {
            env.push((key.to_owned(), value.to_owned()));
        }
    }
    env
}

/// Resolves `program` the way `execvp` would with `env`'s `PATH`. Returns
/// `None` when nothing executable is found.
pub fn resolve_program(program: &OsStr, env: &[(OsString, OsString)]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    if program.as_bytes().contains(&b'/') {
        return Some(PathBuf::from(program));
    }
    let path = env
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| OsString::from("/usr/bin:/bin"));
    std::env::split_paths(&path).map(|dir| dir.join(program)).find(|candidate| {
        std::fs::metadata(candidate)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

/// Execs `program args` in place, disclaimed. Only returns on failure.
pub fn exec_disclaimed(program: &OsStr, args: &[OsString]) -> std::io::Error {
    let env = child_env([], false);
    let Some(path) = resolve_program(program, &env) else {
        return std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} not found", program.to_string_lossy()));
    };
    match platform::DisclaimedExec::new(&path, program, args, &env, true) {
        Ok(exec) => exec.exec(),
        Err(error) => error,
    }
}

/// Handles [`EXEC_DISCLAIMED_ARG`] when it is the first argument: execs the
/// remaining arguments and exits if that fails. Returns otherwise.
pub fn handle_exec_disclaimed_args() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|arg| arg != EXEC_DISCLAIMED_ARG) {
        return;
    }
    let Some(program) = args.next() else {
        eprintln!("{EXEC_DISCLAIMED_ARG}: missing program");
        std::process::exit(127);
    };
    let args: Vec<OsString> = args.collect();
    let error = exec_disclaimed(&program, &args);
    eprintln!("{EXEC_DISCLAIMED_ARG}: failed to exec {}: {error}", program.to_string_lossy());
    std::process::exit(127);
}

pub(crate) use platform::DisclaimedExec;

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::CString;
    use std::ffi::OsStr;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;

    unsafe extern "C" {
        fn responsibility_spawnattrs_setdisclaim(
            attrs: *mut libc::posix_spawnattr_t,
            disclaim: libc::c_int,
        ) -> libc::c_int;
        fn _NSGetEnviron() -> *mut *const *const libc::c_char;
    }

    /// Everything the child needs to re-exec, prepared before fork so the
    /// child only makes async-signal-safe calls.
    pub(crate) struct DisclaimedExec {
        path: CString,
        _argv: Vec<CString>,
        argv_ptrs: Vec<*const libc::c_char>,
        _envp: Vec<CString>,
        envp_ptrs: Vec<*const libc::c_char>,
        attrs: libc::posix_spawnattr_t,
    }

    // The pointers only reference the `CString`s owned by the same value.
    unsafe impl Send for DisclaimedExec {}

    fn c_string(value: &OsStr) -> std::io::Result<CString> {
        CString::new(value.as_bytes()).map_err(std::io::Error::other)
    }

    impl DisclaimedExec {
        /// `reset_signals` restores default dispositions for every signal,
        /// for callers whose own runtime ignored some (Rust ignores `SIGPIPE`).
        pub(crate) fn new(
            path: &Path,
            program: &OsStr,
            args: &[OsString],
            env: &[(OsString, OsString)],
            reset_signals: bool,
        ) -> std::io::Result<DisclaimedExec> {
            let path = c_string(path.as_os_str())?;
            let argv: Vec<CString> = std::iter::once(Ok(c_string(program)?))
                .chain(args.iter().map(|arg| c_string(arg)))
                .collect::<std::io::Result<_>>()?;
            let envp: Vec<CString> = env
                .iter()
                .map(|(key, value)| {
                    let mut entry = key.clone();
                    entry.push("=");
                    entry.push(value);
                    c_string(&entry)
                })
                .collect::<std::io::Result<_>>()?;
            let argv_ptrs = argv.iter().map(|arg| arg.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
            let envp_ptrs = envp.iter().map(|entry| entry.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();

            let mut attrs: libc::posix_spawnattr_t = std::ptr::null_mut();
            unsafe {
                let result = libc::posix_spawnattr_init(&raw mut attrs);
                if result != 0 {
                    return Err(std::io::Error::from_raw_os_error(result));
                }
                let mut flags = libc::POSIX_SPAWN_SETEXEC;
                if reset_signals {
                    let mut all_signals: libc::sigset_t = 0;
                    libc::sigfillset(&raw mut all_signals);
                    libc::posix_spawnattr_setsigdefault(&raw mut attrs, &raw const all_signals);
                    flags |= libc::POSIX_SPAWN_SETSIGDEF;
                }
                libc::posix_spawnattr_setflags(&raw mut attrs, flags as libc::c_short);
                responsibility_spawnattrs_setdisclaim(&raw mut attrs, 1);
            }

            Ok(DisclaimedExec { path, _argv: argv, argv_ptrs, _envp: envp, envp_ptrs, attrs })
        }

        /// Replaces the current process image. Only returns on failure. Safe
        /// to call between fork and exec.
        pub(crate) fn exec(&self) -> std::io::Error {
            unsafe {
                // Like `execvp`, the new image and anything it looks up see the command's environment.
                *_NSGetEnviron() = self.envp_ptrs.as_ptr();
                let result = libc::posix_spawn(
                    std::ptr::null_mut(),
                    self.path.as_ptr(),
                    std::ptr::null(),
                    &raw const self.attrs,
                    self.argv_ptrs.as_ptr() as *const *mut libc::c_char,
                    self.envp_ptrs.as_ptr() as *const *mut libc::c_char,
                );
                std::io::Error::from_raw_os_error(result)
            }
        }
    }

    impl Drop for DisclaimedExec {
        fn drop(&mut self) {
            unsafe { libc::posix_spawnattr_destroy(&raw mut self.attrs) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::ffi::OsStr;
    use std::ffi::OsString;
    use std::os::unix::process::CommandExt as _;
    use std::path::Path;

    /// Other platforms have no responsibility tracking, so this is a plain exec.
    pub(crate) struct DisclaimedExec {
        path: std::path::PathBuf,
        program: OsString,
        args: Vec<OsString>,
    }

    impl DisclaimedExec {
        pub(crate) fn new(
            path: &Path,
            program: &OsStr,
            args: &[OsString],
            _env: &[(OsString, OsString)],
            _reset_signals: bool,
        ) -> std::io::Result<DisclaimedExec> {
            Ok(DisclaimedExec { path: path.to_owned(), program: program.to_owned(), args: args.to_vec() })
        }

        pub(crate) fn exec(&self) -> std::io::Error {
            std::process::Command::new(&self.path).arg0(&self.program).args(&self.args).exec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_command() {
        let (program, args) = wrap_command(
            Path::new("/Applications/ed.app/Contents/MacOS/ed"),
            "zsh".to_string(),
            vec!["-l".to_string()],
        );
        assert_eq!(program, "/Applications/ed.app/Contents/MacOS/ed");
        assert_eq!(args, vec![EXEC_DISCLAIMED_ARG, "zsh", "-l"]);
    }

    #[test]
    fn test_child_env_applies_explicit_changes() {
        let env = child_env(
            [
                (OsStr::new("A"), Some(OsStr::new("1"))),
                (OsStr::new("B"), Some(OsStr::new("2"))),
                (OsStr::new("A"), Some(OsStr::new("3"))),
                (OsStr::new("B"), None),
            ],
            true,
        );
        assert_eq!(env, vec![(OsString::from("A"), OsString::from("3"))]);
    }

    #[test]
    fn test_child_env_inherits_unless_cleared() {
        let inherited = child_env([], false);
        assert!(inherited.iter().any(|(key, _)| key == "PATH"));
        let overridden = child_env([(OsStr::new("PATH"), Some(OsStr::new("/nowhere")))], false);
        assert_eq!(
            overridden.iter().filter(|(key, _)| key == "PATH").collect::<Vec<_>>(),
            vec![&(OsString::from("PATH"), OsString::from("/nowhere"))]
        );
    }

    #[test]
    fn test_resolve_program() {
        let env = vec![(OsString::from("PATH"), OsString::from("/nonexistent:/bin"))];
        assert_eq!(resolve_program(OsStr::new("sh"), &env), Some(PathBuf::from("/bin/sh")));
        assert_eq!(resolve_program(OsStr::new("./x"), &env), Some(PathBuf::from("./x")));
        assert_eq!(resolve_program(OsStr::new("no-such-program-ed"), &env), None);
        // Without PATH, execvp's default search path applies.
        assert_eq!(resolve_program(OsStr::new("sh"), &[]), Some(PathBuf::from("/bin/sh")));
    }
}
