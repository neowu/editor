//! What the gutter run icon spawns: a command for a terminal tab.

use std::path::PathBuf;

use gpui::SharedString;
use rustc_hash::FxHashMap;
pub use util::shell::Shell;
pub use util::shell::ShellKind;
pub use util::shell_builder::ShellBuilder;

/// Everything needed to spawn a task in a terminal tab.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SpawnInTerminal {
    /// Full label; also identifies the task's terminal tab, so the same label reuses the tab.
    pub full_label: String,
    /// Single-line label for the tab title.
    pub label: String,
    /// Executable command to spawn.
    pub command: Option<String>,
    /// Arguments, passed through the shell that spawns the command.
    pub args: Vec<String>,
    /// The command and its arguments, joined.
    pub command_label: String,
    /// Current working directory to spawn the command into.
    pub cwd: Option<PathBuf>,
    /// Env overrides, applied on top of the terminal's environment.
    pub env: FxHashMap<String, String>,
    /// Whether to replace a running instance of the task right away, rather than wait for it to finish.
    pub allow_concurrent_runs: bool,
}

impl SpawnInTerminal {
    pub fn new(label: impl Into<String>, command: impl Into<String>, args: Vec<String>, cwd: Option<PathBuf>) -> Self {
        let full_label = label.into();
        let command = command.into();
        let mut command_label = command.clone();
        for arg in &args {
            command_label.push(' ');
            command_label.push_str(arg);
        }
        Self {
            label: full_label.replace('\n', "\\n"),
            full_label,
            command: Some(command),
            args,
            command_label,
            cwd,
            ..Self::default()
        }
    }
}

/// A tag on a runnable symbol (a test or main function), set by `#set! tag` in a `runnables.scm`.
#[derive(Clone, Debug)]
pub struct RunnableTag(pub SharedString);

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SpawnInTerminal;

    #[test]
    fn new_joins_command_label_and_escapes_display_newlines() {
        let task = SpawnInTerminal::new("Build\nRun", "cargo", vec!["test".into(), "-p".into(), "a".into()], None);
        assert_eq!(task.full_label, "Build\nRun");
        assert_eq!(task.label, "Build\\nRun");
        assert_eq!(task.command.as_deref(), Some("cargo"));
        assert_eq!(task.command_label, "cargo test -p a");
        assert_eq!(task.cwd, None);

        let task = SpawnInTerminal::new("go", "go", Vec::new(), Some(PathBuf::from("/a")));
        assert_eq!(task.label, "go");
        assert_eq!(task.command_label, "go");
        assert_eq!(task.cwd, Some(PathBuf::from("/a")));
    }
}
