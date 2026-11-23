use futures::future;
use git::DeleteUntrackedFiles;
use git::RestoreFile;
use git::RestoreTrackedFiles;
use git::repository::RepoPath;
use git_ui_core::notifications::show_error_toast;
use gpui::PromptLevel;
use gpui::TaskExt as _;
use itertools::Itertools as _;
use ui::prelude::Context;
use ui::prelude::Window;
use util::markdown::MarkdownInlineCode;
use workspace::notifications::DetachAndPromptErr as _;

use super::GitPanel;
use super::GitStatusEntry;
use super::SelectionTargetKind;
use super::prompt;

#[derive(strum::EnumIter, strum::VariantNames)]
#[strum(serialize_all = "title_case")]
enum DeleteCancel {
    Delete,
    Cancel,
}

/// Label of the context-menu entry that reverts the selection. Untracked
/// entries are deleted (moved to the system trash), matching the project panel.
pub(super) fn restore_title(
    target_kind: SelectionTargetKind,
    all_created: bool,
    all_deleted: bool,
    file_count: usize,
) -> String {
    let file_label = if file_count == 1 { "File" } else { "Files" };
    match (target_kind, all_created, all_deleted) {
        (SelectionTargetKind::Directory, true, _) => "Delete Folder".into(),
        (SelectionTargetKind::Directory, false, _) => "Discard Changes".into(),
        (SelectionTargetKind::File, true, _) => "Delete File".into(),
        (SelectionTargetKind::File, _, true) => "Restore File".into(),
        (SelectionTargetKind::File, _, _) => "Discard Changes".into(),
        (SelectionTargetKind::Multiple, true, _) => format!("Delete {file_count} {file_label}"),
        (SelectionTargetKind::Multiple, _, true) => format!("Restore {file_count} {file_label}"),
        (SelectionTargetKind::Multiple, _, _) => format!("Discard Changes to {file_count} {file_label}"),
    }
}

/// Message and confirm label for reverting several entries at once, or `None`
/// when there is nothing to revert.
fn bulk_revert_prompt(tracked_count: usize, untracked_count: usize) -> Option<(String, &'static str)> {
    match (tracked_count, untracked_count) {
        (0, 0) => None,
        (tracked_count, 0) => Some((format!("Discard changes to {tracked_count} files?"), "Discard")),
        (0, untracked_count) => Some((format!("Do you want to delete {untracked_count} files?"), "Delete")),
        (tracked_count, untracked_count) => Some((
            format!("Discard changes to {tracked_count} files and delete {untracked_count} files?"),
            "Discard and Delete",
        )),
    }
}

impl GitPanel {
    pub(super) fn revert_selected(&mut self, action: &RestoreFile, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.effective_status_entries();
        self.revert_entries(entries, action.skip_prompt, window, cx);
    }

    /// Returns the repo path and whether it represents a directory for the
    /// currently selected entry, provided the entry can be added to an ignore
    /// file.
    ///
    /// A file is eligible when its status reports it as newly created. A
    /// directory is eligible only when all of its descendants are newly created.
    fn selected_ignorable_path(&self) -> Option<(RepoPath, bool)> {
        let selected_index = self.selected_entry?;
        let list_entry = self.entries.get(selected_index)?;

        if let Some(directory) = list_entry.directory_entry() {
            self.directory_descendants(selected_index)?
                .iter()
                .all(|entry| entry.status.is_created())
                .then(|| (directory.key.path.clone(), true))
        } else {
            let entry = list_entry.status_entry()?;
            entry.status.is_created().then(|| (entry.repo_path.clone(), false))
        }
    }

    fn selected_ignorable_paths(&self) -> Vec<(RepoPath, bool)> {
        if self.selection_target_kind() == Some(SelectionTargetKind::Multiple) {
            let entries = self.effective_status_entries();
            if entries.iter().any(|entry| !entry.status.is_created()) {
                return Vec::new();
            }
            entries.into_iter().map(|entry| (entry.repo_path, false)).collect()
        } else {
            self.selected_ignorable_path().into_iter().collect()
        }
    }

    pub(super) fn add_to_gitignore(&mut self, _: &git::AddToGitignore, _window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_ignorable_paths();
        if paths.is_empty() {
            return;
        }

        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };

        let receivers = active_repository.update(cx, |repo, _| {
            paths.iter().map(|(repo_path, is_dir)| repo.add_path_to_gitignore(repo_path, *is_dir)).collect::<Vec<_>>()
        });

        for receiver in receivers {
            let workspace = self.workspace.clone();
            cx.spawn(async move |_, cx| {
                if let Err(error) = receiver.await?
                    && let Some(workspace) = workspace.upgrade()
                {
                    cx.update(|cx| {
                        show_error_toast(workspace, "add to .gitignore", error, cx);
                    });
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
        }
    }

    pub(super) fn add_to_git_info_exclude(
        &mut self,
        _: &git::AddToGitInfoExclude,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = self.selected_ignorable_paths();
        if paths.is_empty() {
            return;
        }

        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };

        let receivers = active_repository.update(cx, |repo, _| {
            paths
                .iter()
                .map(|(repo_path, is_dir)| repo.add_path_to_git_info_exclude(repo_path, *is_dir))
                .collect::<Vec<_>>()
        });

        for receiver in receivers {
            let workspace = self.workspace.clone();
            cx.spawn(async move |_, cx| {
                if let Err(error) = receiver.await?
                    && let Some(workspace) = workspace.upgrade()
                {
                    cx.update(|cx| {
                        show_error_toast(workspace, "add to .git/info/exclude", error, cx);
                    });
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
        }
    }

    fn revert_entries(
        &self,
        entries: Vec<GitStatusEntry>,
        skip_prompt: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            return;
        }

        (|| {
            let active_repo = self.active_repository.clone()?;
            let workspace = self.workspace.clone();

            let staged =
                entries.iter().filter(|entry| entry.status.staging().has_staged()).cloned().collect::<Vec<_>>();

            let (tracked, untracked): (Vec<_>, Vec<_>) =
                entries.into_iter().partition(|entry| !entry.status.is_created());

            let prompt = if skip_prompt {
                None
            } else if tracked.len() + untracked.len() == 1 {
                let entry = tracked.first().or_else(|| untracked.first())?;

                let (message, confirm_text) = if entry.status.is_deleted() {
                    ("Are you sure you want to restore ", "Restore File")
                } else if entry.status.is_created() {
                    ("Do you want to delete ", "Delete")
                } else {
                    ("Are you sure you want to discard changes to ", "Discard Changes")
                };

                Some(window.prompt(
                    PromptLevel::Warning,
                    &format!(
                        "{}{}?",
                        message,
                        MarkdownInlineCode(entry.repo_path.file_name().unwrap_or_else(|| entry.repo_path.display()))
                    ),
                    None,
                    &[confirm_text, "Cancel"],
                    cx,
                ))
            } else {
                let (message, confirm_text) = bulk_revert_prompt(tracked.len(), untracked.len())?;

                Some(window.prompt(PromptLevel::Warning, &message, None, &[confirm_text, "Cancel"], cx))
            };

            // Resolve paths NOW, before entering async context.
            let paths = untracked
                .iter()
                .filter_map(|entry| active_repo.read(cx).repo_path_to_project_path(&entry.repo_path, cx))
                .collect::<Vec<_>>();

            cx.spawn_in(window, async move |this, cx| {
                if let Some(prompt) = prompt
                    && prompt.await? != 0
                {
                    return Ok(());
                }

                if !staged.is_empty() || !tracked.is_empty() {
                    this.update_in(cx, |this, window, cx| {
                        if !staged.is_empty() {
                            this.change_file_stage(false, staged, cx);
                        }

                        if !tracked.is_empty() {
                            this.perform_checkout(tracked, window, cx);
                        }
                    })?;
                }

                for path in paths {
                    let task = workspace.update(cx, |workspace, cx| {
                        workspace.project().update(cx, |project, cx| project.trash_file(path, cx))
                    })?;

                    if let Some(task) = task {
                        task.await?;
                    }
                }

                Ok(())
            })
            .detach_and_prompt_err("Failed to revert changes", window, cx, |e| Some(format!("{e}")));

            Some(())
        })();
    }

    fn perform_checkout(&self, entries: Vec<GitStatusEntry>, window: &Window, cx: &Context<Self>) {
        let workspace = self.workspace.clone();
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };

        let task = cx.spawn_in(window, async move |this, cx| {
            let tasks: Vec<_> = workspace.update(cx, |workspace, cx| {
                workspace.project().update(cx, |project, cx| {
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let path = active_repository.read(cx).repo_path_to_project_path(&entry.repo_path, cx)?;
                            Some(project.open_buffer(path, cx))
                        })
                        .collect()
                })
            })?;

            let buffers = future::join_all(tasks).await;

            this.update_in(cx, |this, window, cx| {
                let task = active_repository.update(cx, |repo, cx| {
                    repo.checkout_files("HEAD", entries.into_iter().map(|entries| entries.repo_path).collect(), cx)
                });
                this.update_visible_entries(window, cx);
                task
            })?
            .await?;

            let tasks: Vec<_> = cx.update(|_, cx| {
                buffers
                    .iter()
                    .filter_map(|buffer| {
                        buffer.as_ref().ok()?.update(cx, |buffer, cx| buffer.is_dirty().then(|| buffer.reload(cx)))
                    })
                    .collect()
            })?;

            future::join_all(tasks).await;

            Ok(())
        });

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;

            this.update_in(cx, |this, window, cx| {
                if let Err(err) = result {
                    this.update_visible_entries(window, cx);
                    this.show_error_toast("checkout", err, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn restore_tracked_files(
        &mut self,
        _: &RestoreTrackedFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self
            .directory_context_descendants()
            .map_or_else(|| Self::staged_tracked_entries(self.change_entries_by_path()), Self::staged_tracked_entries);

        match entries.len() {
            0 => return,
            1 => {
                return self.revert_entries(entries, false, window, cx);
            }
            _ => {}
        }
        let mut details = entries
            .iter()
            .filter_map(|entry| entry.repo_path.as_ref().file_name())
            .map(|filename| filename.to_owned())
            .take(5)
            .join("\n");
        if entries.len() > 5 {
            util::write_str!(details, "\nand {} more…", entries.len() - 5);
        }

        #[derive(strum::EnumIter, strum::VariantNames)]
        #[strum(serialize_all = "title_case")]
        enum RestoreCancel {
            RestoreTrackedFiles,
            Cancel,
        }
        let prompt = prompt("Discard changes to these files?", Some(&details), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if matches!(prompt.await, Ok(RestoreCancel::RestoreTrackedFiles)) {
                this.update_in(cx, |this, window, cx| {
                    this.perform_checkout(entries, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn clean_all(&mut self, _: &DeleteUntrackedFiles, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let Some(active_repo) = self.active_repository.clone() else {
            return;
        };
        let to_delete = self
            .change_entries_by_path()
            .filter(|status_entry| status_entry.status.is_created())
            .cloned()
            .collect::<Vec<_>>();

        match to_delete.len() {
            0 => return,
            1 => {
                return self.revert_entries(to_delete, false, window, cx);
            }
            _ => {}
        }

        let mut details = to_delete
            .iter()
            .map(|entry| entry.repo_path.as_ref().file_name().map_or_default(|f| f.to_owned()))
            .take(5)
            .join("\n");

        if to_delete.len() > 5 {
            util::write_str!(details, "\nand {} more…", to_delete.len() - 5);
        }

        let prompt = prompt("Do you want to delete these files?", Some(&details), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            match prompt.await? {
                DeleteCancel::Delete => {}
                DeleteCancel::Cancel => return Ok(()),
            }
            let tasks = workspace.update(cx, |workspace, cx| {
                to_delete
                    .iter()
                    .filter_map(|entry| {
                        workspace.project().update(cx, |project, cx| {
                            let project_path = active_repo.read(cx).repo_path_to_project_path(&entry.repo_path, cx)?;
                            project.trash_file(project_path, cx)
                        })
                    })
                    .collect::<Vec<_>>()
            })?;
            let total_count = tasks.len();
            let to_unstage =
                to_delete.into_iter().filter(|entry| !entry.status.staging().is_fully_unstaged()).collect();
            this.update(cx, |this, cx| this.change_file_stage(false, to_unstage, cx))?;

            let results = future::join_all(tasks).await;
            let errors: Vec<anyhow::Error> = results.into_iter().filter_map(|r| r.err()).collect();
            let failed_count = errors.len();
            if let Some(first_error) = errors.into_iter().next() {
                return Err(anyhow::anyhow!("Failed to delete {failed_count} of {total_count} files: {first_error:#}"));
            }
            Ok(())
        })
        .detach_and_prompt_err("Failed to delete files", window, cx, |e| Some(format!("{e}")));
    }
}

#[cfg(test)]
mod tests {

    use super::SelectionTargetKind;
    use super::bulk_revert_prompt;
    use super::restore_title;

    #[test]
    fn restore_title_works() {
        assert_eq!(restore_title(SelectionTargetKind::Directory, true, false, 3), "Delete Folder");
        assert_eq!(restore_title(SelectionTargetKind::Directory, false, false, 3), "Discard Changes");
        assert_eq!(restore_title(SelectionTargetKind::File, true, false, 1), "Delete File");
        assert_eq!(restore_title(SelectionTargetKind::File, false, true, 1), "Restore File");
        assert_eq!(restore_title(SelectionTargetKind::File, false, false, 1), "Discard Changes");
        assert_eq!(restore_title(SelectionTargetKind::Multiple, true, false, 2), "Delete 2 Files");
        assert_eq!(restore_title(SelectionTargetKind::Multiple, true, false, 1), "Delete 1 File");
        assert_eq!(restore_title(SelectionTargetKind::Multiple, false, true, 2), "Restore 2 Files");
        assert_eq!(restore_title(SelectionTargetKind::Multiple, false, false, 2), "Discard Changes to 2 Files");
    }

    #[test]
    fn bulk_revert_prompt_works() {
        assert_eq!(bulk_revert_prompt(0, 0), None);
        assert_eq!(bulk_revert_prompt(2, 0), Some(("Discard changes to 2 files?".into(), "Discard")));
        assert_eq!(bulk_revert_prompt(0, 3), Some(("Do you want to delete 3 files?".into(), "Delete")));
        assert_eq!(
            bulk_revert_prompt(2, 3),
            Some(("Discard changes to 2 files and delete 3 files?".into(), "Discard and Delete"))
        );
    }
}
