use git::repository::RebaseAction;
use git::repository::Remote;
use git::repository::RemoteCommandOutput;
use ui::SharedString;

#[derive(Clone)]
pub enum RemoteAction {
    Fetch(Option<Remote>),
    Pull(Remote),
    Push(SharedString, Remote),
    Rebase(RebaseAction),
}

impl RemoteAction {
    pub const fn name(&self) -> &'static str {
        match self {
            RemoteAction::Fetch(_) => "fetch",
            RemoteAction::Pull(_) => "pull",
            RemoteAction::Push(_, _) => "push",
            RemoteAction::Rebase(action) => action.name(),
        }
    }
}

pub enum SuccessStyle {
    Toast,
    ToastWithLog { output: RemoteCommandOutput },
}

pub struct SuccessMessage {
    pub message: String,
    pub style: SuccessStyle,
}

const AUTOSTASH_CONFLICT: &str =
    "your uncommitted changes conflict with it. Resolve the conflicts, then drop the stash that still holds them";

/// Git reapplied the autostash with conflicts and kept the stash entry.
fn autostash_conflicted(output: &RemoteCommandOutput) -> bool {
    output.stderr.contains("Your local changes are stashed")
}

pub fn format_output(action: &RemoteAction, output: RemoteCommandOutput) -> SuccessMessage {
    match action {
        RemoteAction::Fetch(remote) => {
            if output.stderr.is_empty() {
                SuccessMessage { message: "Fetch: Already up to date".into(), style: SuccessStyle::Toast }
            } else {
                let message = match remote {
                    Some(remote) => format!("Synchronized with {}", remote.name),
                    None => "Synchronized with remotes".into(),
                };
                SuccessMessage { message, style: SuccessStyle::ToastWithLog { output } }
            }
        }
        RemoteAction::Pull(remote_ref) => {
            // Output of `git pull --rebase --autostash`; a rebase that stops on
            // conflicts exits non-zero and never gets here.
            let files_changed = output
                .stdout
                .lines()
                .find(|line| line.contains(" changed"))
                .and_then(|line| line.split_whitespace().next()?.parse::<u32>().ok());
            if autostash_conflicted(&output) {
                SuccessMessage {
                    message: format!("Pulled from {}, but {AUTOSTASH_CONFLICT}", remote_ref.name),
                    style: SuccessStyle::ToastWithLog { output },
                }
            } else if output.stdout.contains("up to date.") {
                SuccessMessage { message: "Pull: Already up to date".into(), style: SuccessStyle::Toast }
            } else if let Some(files_changed) = files_changed {
                SuccessMessage {
                    message: format!(
                        "Received {} file change{} from {}",
                        files_changed,
                        if files_changed == 1 { "" } else { "s" },
                        remote_ref.name
                    ),
                    style: SuccessStyle::ToastWithLog { output },
                }
            } else if output.stderr.contains("Successfully rebased") {
                SuccessMessage {
                    message: format!("Rebased onto {}", remote_ref.name),
                    style: SuccessStyle::ToastWithLog { output },
                }
            } else {
                SuccessMessage {
                    message: format!("Successfully pulled from {}", remote_ref.name),
                    style: SuccessStyle::ToastWithLog { output },
                }
            }
        }
        RemoteAction::Rebase(action) => {
            if autostash_conflicted(&output) {
                SuccessMessage {
                    message: format!("Rebase finished, but {AUTOSTASH_CONFLICT}"),
                    style: SuccessStyle::ToastWithLog { output },
                }
            } else {
                let message = match action {
                    RebaseAction::Continue => "Rebase finished",
                    RebaseAction::Abort => "Rebase aborted",
                };
                SuccessMessage { message: message.into(), style: SuccessStyle::Toast }
            }
        }
        RemoteAction::Push(branch_name, remote_ref) => {
            if output.stderr.ends_with("Everything up-to-date\n") {
                SuccessMessage { message: "Push: Everything is up-to-date".to_string(), style: SuccessStyle::Toast }
            } else {
                SuccessMessage {
                    message: format!("Pushed {} to {}", branch_name, remote_ref.name),
                    style: SuccessStyle::ToastWithLog { output },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use git::repository::RebaseAction;
    use git::repository::Remote;
    use git::repository::RemoteCommandOutput;

    use super::RemoteAction;
    use super::SuccessStyle;
    use super::format_output;

    fn pull_message(stdout: &str, stderr: &str) -> (String, bool) {
        let action = RemoteAction::Pull(Remote { name: "origin".into() });
        let message = format_output(&action, RemoteCommandOutput { stdout: stdout.into(), stderr: stderr.into() });
        (message.message, matches!(message.style, SuccessStyle::ToastWithLog { .. }))
    }

    #[test]
    fn test_format_pull_output() {
        assert_eq!(pull_message("Already up to date.\n", ""), ("Pull: Already up to date".into(), false));
        assert_eq!(
            pull_message(
                "Updating 292bce8..50250db\nCreated autostash: dffb31c\nFast-forward\n f | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n",
                "Applied autostash.\n"
            ),
            ("Received 1 file change from origin".into(), true)
        );
        assert_eq!(pull_message(" 3 files changed, 4 insertions(+)\n", "").0, "Received 3 file changes from origin");
        assert_eq!(
            pull_message(
                "Created autostash: a72ceb9\n",
                "Rebasing (1/1)Applied autostash.\nSuccessfully rebased and updated refs/heads/master.\n"
            ),
            ("Rebased onto origin".into(), true)
        );
        let (message, has_log) = pull_message(
            "Created autostash: 0a11f09\n",
            "Your local changes are stashed, however applying them\nresulted in conflicts.\nSuccessfully rebased and updated refs/heads/master.\n",
        );
        assert!(message.contains("drop the stash"));
        assert!(has_log);
        assert_eq!(pull_message("", "").0, "Successfully pulled from origin");
    }

    #[test]
    fn test_format_rebase_output() {
        let message = |action, stderr: &str| {
            let output = RemoteCommandOutput { stdout: String::new(), stderr: stderr.into() };
            let message = format_output(&RemoteAction::Rebase(action), output);
            (message.message, matches!(message.style, SuccessStyle::ToastWithLog { .. }))
        };
        assert_eq!(message(RebaseAction::Continue, "Applied autostash.\n"), ("Rebase finished".into(), false));
        assert_eq!(message(RebaseAction::Abort, ""), ("Rebase aborted".into(), false));
        let (text, has_log) = message(
            RebaseAction::Continue,
            "Your local changes are stashed, however applying them\nresulted in conflicts.\n",
        );
        assert!(text.starts_with("Rebase finished, but"));
        assert!(has_log);
        assert_eq!(RemoteAction::Rebase(RebaseAction::Abort).name(), "rebase abort");
    }
}
