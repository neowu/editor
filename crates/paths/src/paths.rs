//! Paths to locations used by ed.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use ignore::gitignore;
use util::paths::SanitizedPath;
pub use util::paths::home_dir;

/// The application name, used for the XDG data, cache, and state
/// directories and the log file name. Must match the editor binary name.
pub const APP_NAME: &str = "ed";

/// A custom root override, set only by `set_custom_root_dir`. When set, the
/// data, state and cache directories all live under it instead of the XDG
/// bases, so an instance with its own root shares nothing with the default one.
static CUSTOM_ROOT_DIR: OnceLock<PathBuf> = OnceLock::new();

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();
static TEMP_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Resolves one of ed's base directories: `<custom_root>/<name>` when a custom
/// root is set, else the XDG base for [`APP_NAME`] on every platform, including
/// macOS: the absolute value of `xdg_env` if set, else `~/<xdg_fallback>`.
fn base_dir(custom_root: Option<&PathBuf>, name: &str, xdg_env: &str, xdg_fallback: &str) -> PathBuf {
    match custom_root {
        Some(root) => root.join(name),
        None => env::var_os(xdg_env)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home_dir().join(xdg_fallback))
            .join(APP_NAME),
    }
}

/// Sets a custom root for the data, state and cache directories
/// (`<dir>/data`, `<dir>/state`, `<dir>/cache`). The path is created and
/// canonicalized by blocking FS operations.
///
/// # Panics
///
/// Panics if called after any of those directories was resolved, or if the
/// directory cannot be created or canonicalized.
pub fn set_custom_root_dir(dir: &str) -> &'static PathBuf {
    if DATA_DIR.get().is_some() || STATE_DIR.get().is_some() || TEMP_DIR.get().is_some() {
        panic!("set_custom_root_dir called after data_dir, state_dir or temp_dir was resolved");
    }
    CUSTOM_ROOT_DIR.get_or_init(|| {
        let path = PathBuf::from(dir);
        fs::create_dir_all(&path).expect("failed to create custom root directory");
        let canonicalized =
            path.canonicalize().expect("failed to canonicalize custom root directory's path to an absolute path");
        // On Windows, `canonicalize` produces extended-length paths prefixed
        // with `\\?\`. Strip that prefix so downstream consumers (e.g.
        // Node.js language servers) that receive derived paths as arguments
        // don't choke on the verbatim syntax.
        SanitizedPath::new(&canonicalized).as_path().to_path_buf()
    })
}

/// Returns the path to the data directory (`projects.json`, remote server state).
pub fn data_dir() -> &'static PathBuf {
    DATA_DIR.get_or_init(|| base_dir(CUSTOM_ROOT_DIR.get(), "data", "XDG_DATA_HOME", ".local/share"))
}

/// Returns the path to the state directory (single-instance lock, logs).
pub fn state_dir() -> &'static PathBuf {
    STATE_DIR.get_or_init(|| base_dir(CUSTOM_ROOT_DIR.get(), "state", "XDG_STATE_HOME", ".local/state"))
}

/// Returns the path to the cache/temp directory.
pub fn temp_dir() -> &'static PathBuf {
    TEMP_DIR.get_or_init(|| base_dir(CUSTOM_ROOT_DIR.get(), "cache", "XDG_CACHE_HOME", ".cache"))
}

/// Returns the path to the logs directory.
pub fn logs_dir() -> &'static PathBuf {
    static LOGS_DIR: OnceLock<PathBuf> = OnceLock::new();
    LOGS_DIR.get_or_init(|| state_dir().join("logs"))
}

/// Returns the path to the remote server state directory on this SSH host.
pub fn remote_server_state_dir() -> &'static PathBuf {
    static REMOTE_SERVER_STATE: OnceLock<PathBuf> = OnceLock::new();
    REMOTE_SERVER_STATE.get_or_init(|| data_dir().join("server_state"))
}

/// Returns the path to the `ed.log` file.
pub fn log_file() -> &'static PathBuf {
    static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    LOG_FILE.get_or_init(|| logs_dir().join(format!("{}.log", APP_NAME)))
}

/// Returns the path to the `ed.log.old` file.
pub fn old_log_file() -> &'static PathBuf {
    static OLD_LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    OLD_LOG_FILE.get_or_init(|| logs_dir().join(format!("{}.log.old", APP_NAME)))
}

/// Git's global config files: `~/.gitconfig` and `$XDG_CONFIG_HOME/git/config`
/// (`~/.config/git/config` when unset or empty), as git itself reads them.
pub fn git_global_config_files() -> [PathBuf; 2] {
    git_global_config_files_for(home_dir(), env::var_os("XDG_CONFIG_HOME"))
}

fn git_global_config_files_for(home: &Path, xdg_config_home: Option<OsString>) -> [PathBuf; 2] {
    let xdg_config_home =
        xdg_config_home.filter(|dir| !dir.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    [home.join(".gitconfig"), xdg_config_home.join("git").join("config")]
}

pub fn global_gitignore_path() -> Option<PathBuf> {
    static GLOBAL_GITIGNORE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    GLOBAL_GITIGNORE_PATH.get_or_init(gitignore::gitconfig_excludes_path).clone()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;
    use std::path::PathBuf;

    use super::APP_NAME;
    use super::base_dir;
    use super::git_global_config_files_for;

    #[test]
    fn git_global_config_files_follow_xdg_config_home() {
        let home = Path::new("/home/me");
        assert_eq!(
            git_global_config_files_for(home, Some(OsString::from("/xdg"))),
            [PathBuf::from("/home/me/.gitconfig"), PathBuf::from("/xdg/git/config")]
        );
    }

    #[test]
    fn git_global_config_files_default_to_dot_config() {
        let home = Path::new("/home/me");
        let expected = [PathBuf::from("/home/me/.gitconfig"), PathBuf::from("/home/me/.config/git/config")];
        assert_eq!(git_global_config_files_for(home, None), expected);
        assert_eq!(git_global_config_files_for(home, Some(OsString::new())), expected);
    }

    #[test]
    fn base_dir_lives_under_custom_root() {
        let root = PathBuf::from("/tmp/ed-root");
        assert_eq!(
            base_dir(Some(&root), "state", "XDG_STATE_HOME", ".local/state"),
            PathBuf::from("/tmp/ed-root/state")
        );
        assert_eq!(base_dir(Some(&root), "data", "XDG_DATA_HOME", ".local/share"), PathBuf::from("/tmp/ed-root/data"));
    }

    #[test]
    fn base_dir_defaults_to_xdg_base() {
        let dir = base_dir(None, "state", "ED_TEST_UNSET_XDG_VAR", ".local/state");
        assert!(dir.is_absolute());
        assert!(dir.ends_with(PathBuf::from(".local/state").join(APP_NAME)));
    }
}
