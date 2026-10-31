use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

pub use gpui_util::new_std_command;

use crate::disclaim::DisclaimedExec;

pub fn new_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

/// Like [`new_command`], but the process does not count as the editor's on
/// macOS (see `crate::disclaim`). Use it for processes that may outlive the
/// editor, such as anything that can start an ssh connection.
pub fn new_disclaimed_command(program: impl AsRef<OsStr>) -> Command {
    Command::new_disclaimed(program)
}

pub type Child = smol::process::Child;

pub use std::process::Stdio;

pub struct Command {
    inner: smol::process::Command,
    env_cleared: bool,
    /// Filled right before spawning and read by the child's `pre_exec` hook.
    disclaimed_exec: Option<Arc<Mutex<Option<DisclaimedExec>>>>,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl Command {
    #[inline]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self { inner: smol::process::Command::new(program), env_cleared: false, disclaimed_exec: None }
    }

    pub fn new_disclaimed(program: impl AsRef<OsStr>) -> Self {
        use std::os::unix::process::CommandExt as _;

        let disclaimed_exec: Arc<Mutex<Option<DisclaimedExec>>> = Arc::new(Mutex::new(None));
        let mut command = std::process::Command::new(program);
        let child_exec = disclaimed_exec.clone();
        // safety: the hook only reads state prepared before fork and makes async-signal-safe calls.
        unsafe {
            command.pre_exec(move || {
                if let Ok(exec) = child_exec.try_lock()
                    && let Some(exec) = exec.as_ref()
                {
                    // On failure, fall back to the plain exec that follows.
                    exec.exec();
                }
                Ok(())
            });
        }
        Self {
            inner: smol::process::Command::from(command),
            env_cleared: false,
            disclaimed_exec: Some(disclaimed_exec),
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.inner.get_args()
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.inner.env(key, val);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.inner.env_remove(key);
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.inner.env_clear();
        self.env_cleared = true;
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    pub fn stdin(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.inner.stdin(cfg.into());
        self
    }

    pub fn stdout(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.inner.stdout(cfg.into());
        self
    }

    pub fn stderr(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.inner.stderr(cfg.into());
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.inner.kill_on_drop(kill_on_drop);
        self
    }

    pub fn spawn(&mut self) -> std::io::Result<Child> {
        self.prepare_disclaimed_exec();
        let child = self.inner.spawn();
        self.clear_disclaimed_exec();
        child
    }

    pub async fn output(&mut self) -> std::io::Result<std::process::Output> {
        // smol spawns when `output`/`status` is called, before the future is polled.
        self.prepare_disclaimed_exec();
        let output = self.inner.output();
        self.clear_disclaimed_exec();
        output.await
    }

    pub async fn status(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.prepare_disclaimed_exec();
        let status = self.inner.status();
        self.clear_disclaimed_exec();
        status.await
    }

    /// Builds the child's re-exec from the command's final program, arguments
    /// and environment. If it can't be built, the child execs normally.
    fn prepare_disclaimed_exec(&self) {
        let Some(disclaimed_exec) = &self.disclaimed_exec else {
            return;
        };
        let env = crate::disclaim::child_env(self.inner.get_envs(), self.env_cleared);
        let program = self.inner.get_program();
        let exec = crate::disclaim::resolve_program(program, &env).and_then(|path| {
            let args: Vec<std::ffi::OsString> = self.inner.get_args().map(OsStr::to_owned).collect();
            DisclaimedExec::new(&path, program, &args, &env, false).ok()
        });
        if let Ok(mut slot) = disclaimed_exec.lock() {
            *slot = exec;
        }
    }

    fn clear_disclaimed_exec(&self) {
        if let Some(disclaimed_exec) = &self.disclaimed_exec
            && let Ok(mut slot) = disclaimed_exec.lock()
        {
            *slot = None;
        }
    }

    pub fn get_program(&self) -> &OsStr {
        self.inner.get_program()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disclaimed_command_keeps_args_env_and_cwd() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let output = smol::block_on(
            new_disclaimed_command("sh")
                .args([
                    "-c",
                    "printf '%s|%s|%s|%s' \"$1\" \"$ED_TEST_VAR\" \"${HOME-unset}\" \"$(pwd -P)\"",
                    "sh",
                    "arg one",
                ])
                .env("ED_TEST_VAR", "value")
                .env_remove("HOME")
                .current_dir(&dir)
                .output(),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("arg one|value|unset|{}", dir.display()));
    }

    #[test]
    fn test_disclaimed_command_with_cleared_env() {
        let output =
            smol::block_on(new_disclaimed_command("/usr/bin/env").env_clear().env("ONLY", "1").output()).unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "ONLY=1\n");
    }

    #[test]
    fn test_disclaimed_command_missing_program() {
        assert!(new_disclaimed_command("no-such-program-ed").spawn().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_disclaimed_command_is_responsible_for_itself() {
        unsafe extern "C" {
            fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
        }
        let responsible = |command: &mut Command, disclaimed: bool| {
            let mut child = command.arg("5").kill_on_drop(true).spawn().unwrap();
            let pid = child.id().cast_signed();
            // Wait for the re-exec to replace the image.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut responsible = unsafe { responsibility_get_pid_responsible_for_pid(pid) };
            while disclaimed && responsible != pid && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
                responsible = unsafe { responsibility_get_pid_responsible_for_pid(pid) };
            }
            child.kill().ok();
            (pid, responsible)
        };
        let (pid, responsible_pid) = responsible(&mut new_disclaimed_command("sleep"), true);
        assert_eq!(responsible_pid, pid);
        let (pid, responsible_pid) = responsible(&mut new_command("sleep"), false);
        assert_ne!(responsible_pid, pid);
    }
}
