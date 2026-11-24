use std::collections::BTreeSet;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::result;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use gpui::Context;
use gpui::CursorStyle;
use gpui::Entity;
use gpui::Modifiers;
use gpui::PromptLevel;
use gpui::Task;
use gpui::Window;
use project::Entry;
use project::ProjectEntryId;
use project::ProjectPath;
use project::Worktree;
use rustc_hash::FxHashMap;
use util::ResultExt as _;
use util::TryFutureExt as _;
use util::markdown::MarkdownInlineCode;
use util::rel_path::RelPath;
use util::rel_path::RelPathBuf;
use workspace::DraggedSelection;
use workspace::SelectedEntry;
use workspace::notifications::NotifyResultExt as _;
use worktree::CreatedEntry;

use super::FoldedDirectoryDragTarget;
use super::ProjectPanel;
use super::explain_move_error;
use crate::undo::Change;
use crate::undo::project_path_display;

impl ProjectPanel {
    fn move_worktree_root(&self, entry_to_move: ProjectEntryId, destination: ProjectEntryId, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            let Some(worktree_to_move) = project.worktree_for_entry(entry_to_move, cx) else {
                return;
            };
            let Some(destination_worktree) = project.worktree_for_entry(destination, cx) else {
                return;
            };

            let worktree_id = worktree_to_move.read(cx).id();
            let destination_id = destination_worktree.read(cx).id();

            project.move_worktree(worktree_id, destination_id, cx).log_err();
        });
    }

    fn move_worktree_entry(
        &mut self,
        entry_to_move: ProjectEntryId,
        destination_entry: ProjectEntryId,
        destination_is_file: bool,
        cx: &mut Context<Self>,
    ) -> Option<(ProjectPath, Task<Result<CreatedEntry>>)> {
        if entry_to_move == destination_entry {
            return None;
        }

        let (destination_worktree, rename) = self.project.update(cx, |project, cx| {
            let Some(source_path) = project.path_for_entry(entry_to_move, cx) else {
                return (None, None);
            };
            let Some(destination_path) = project.path_for_entry(destination_entry, cx) else {
                return (None, None);
            };
            let destination_worktree_id = destination_path.worktree_id;

            let destination_dir = if destination_is_file {
                destination_path.path.parent().unwrap_or(RelPath::empty())
            } else {
                destination_path.path.as_ref()
            };

            let Some(source_name) = source_path.path.file_name() else {
                return (None, None);
            };
            let Ok(source_name) = RelPath::from_unix_str(source_name) else {
                return (None, None);
            };

            let mut new_path = destination_dir.to_rel_path_buf();
            new_path.push(source_name);

            let new_project_path: ProjectPath = (destination_worktree_id, new_path).into();
            let rename = (new_project_path.path.as_ref() != source_path.path.as_ref()).then(|| {
                let task = project.rename_entry(entry_to_move, new_project_path.clone(), cx);
                (new_project_path, task)
            });

            (project.worktree_id_for_entry(destination_entry, cx), rename)
        });

        if let Some(destination_worktree) = destination_worktree {
            self.expand_entry(destination_worktree, destination_entry, cx);
        }
        rename
    }

    pub(super) fn drop_external_files(
        &self,
        paths: &[PathBuf],
        entry_id: ProjectEntryId,
        window: &Window,
        cx: &Context<Self>,
    ) {
        let mut paths: Vec<Arc<Path>> = paths.iter().map(|path| Arc::from(path.clone())).collect();

        let open_file_after_drop = paths.len() == 1 && paths[0].is_file();

        let Some((target_directory, worktree)) = (|| {
            let project = self.project.read(cx);
            let worktree = project.worktree_for_entry(entry_id, cx)?;
            let entry = worktree.read(cx).entry_for_id(entry_id)?;
            let path = Arc::clone(&entry.path);
            let target_directory = if entry.is_dir() { path } else { path.parent()?.into() };
            Some((target_directory, worktree))
        })() else {
            return;
        };

        let mut paths_to_replace = Vec::new();
        for path in &paths {
            if let Some(name) = path.file_name()
                && let Some(name) = name.to_str()
            {
                let target_path =
                    target_directory.join(RelPath::from_unix_str(name).expect("file name is a valid relative path"));
                if worktree.read(cx).entry_for_path(&target_path).is_some() {
                    paths_to_replace.push((name.to_owned(), Arc::clone(path)));
                }
            }
        }

        cx.spawn_in(window, async move |this, cx| {
            async move {
                for (filename, original_path) in &paths_to_replace {
                    let prompt_message = format!(
                        concat!(
                            "A file or folder with name {} ",
                            "already exists in the destination folder. ",
                            "Do you want to replace it?"
                        ),
                        MarkdownInlineCode(filename)
                    );
                    let answer = cx
                        .update(|window, _| {
                            window.prompt(PromptLevel::Info, &prompt_message, None, &["Replace", "Cancel"])
                        })?
                        .await?;

                    if answer == 1
                        && let Some(item_idx) = paths.iter().position(|p| p == original_path)
                    {
                        paths.remove(item_idx);
                    }
                }

                if paths.is_empty() {
                    return Ok(());
                }

                let (worktree_id, task) = worktree.update(cx, |worktree, cx| {
                    (worktree.id(), worktree.copy_external_entries(target_directory, paths, cx))
                });

                let opened_entries: Vec<_> = task.await.with_context(|| "failed to copy external paths")?;
                this.update_in(cx, |this, window, cx| {
                    let did_open = open_file_after_drop && !opened_entries.is_empty();
                    if did_open {
                        this.open_entry(opened_entries[0], true, false, cx);
                    } else {
                        let new_selection = opened_entries.last().map(|&entry_id| (worktree_id, entry_id));
                        for &entry_id in &opened_entries {
                            this.expand_entry(worktree_id, entry_id, cx);
                        }
                        this.marked_entries.clear();
                        this.update_visible_entries(new_selection, false, false, window, cx);
                    }

                    let changes: Vec<Change> = opened_entries
                        .iter()
                        .filter_map(|entry_id| {
                            worktree.read(cx).entry_for_id(*entry_id).map(|entry| {
                                Change::Created(ProjectPath { worktree_id, path: Arc::clone(&entry.path) })
                            })
                        })
                        .collect();

                    this.undo_manager.record(changes).log_err();
                })
            }
            .log_err()
            .await
        })
        .detach();
    }

    pub(super) fn refresh_drag_cursor_style(&self, modifiers: Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(existing_cursor) = cx.active_drag_cursor_style() {
            let new_cursor =
                if Self::is_copy_modifier_set(modifiers) { CursorStyle::DragCopy } else { CursorStyle::PointingHand };
            if existing_cursor != new_cursor {
                cx.set_active_drag_cursor_style(new_cursor, window);
            }
        }
    }

    pub(super) fn update_folded_directory_drag_target(&mut self, is_hovered: bool, target: FoldedDirectoryDragTarget) {
        if is_hovered {
            self.folded_directory_drag_target = Some(target);
        } else if self.folded_directory_drag_target == Some(target) {
            self.folded_directory_drag_target = None;
        }
    }

    pub(super) fn clear_drag_state(&mut self, cx: &mut Context<Self>) {
        let had_drag_state = self.drag_target_entry.take().is_some()
            | self.folded_directory_drag_target.take().is_some()
            | self.hover_scroll_task.take().is_some()
            | self.hover_expand_task.take().is_some()
            | self.previous_drag_position.take().is_some();
        if had_drag_state {
            cx.notify();
        }
    }

    pub(super) fn drag_onto(
        &mut self,
        selections: &DraggedSelection,
        target_entry_id: ProjectEntryId,
        is_file: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let resolved_selections = selections
            .items()
            .map(|entry| SelectedEntry { entry_id: self.resolve_entry(entry.entry_id), worktree_id: entry.worktree_id })
            .collect::<BTreeSet<SelectedEntry>>();
        let entries = self.disjoint_entries(resolved_selections, cx);

        let root_entries: Vec<ProjectEntryId> = {
            let project = self.project.read(cx);
            entries
                .iter()
                .filter(|entry| project.entry_is_worktree_root(entry.entry_id, cx))
                .map(|entry| entry.entry_id)
                .collect()
        };
        if !root_entries.is_empty() {
            for entry_id in root_entries {
                self.move_worktree_root(entry_id, target_entry_id, cx);
            }
            return;
        }

        if Self::is_copy_modifier_set(window.modifiers()) {
            let _: Option<_> = (|| {
                let project = self.project.read(cx);
                let target_worktree = project.worktree_for_entry(target_entry_id, cx)?;
                let worktree_id = target_worktree.read(cx).id();
                let target_entry = target_worktree.read(cx).entry_for_id(target_entry_id)?.clone();

                let mut copy_tasks = Vec::new();
                let mut disambiguation_range = None;
                for selection in &entries {
                    let (new_path, new_disambiguation_range) =
                        self.create_paste_path(selection, (target_worktree.clone(), &target_entry), cx)?;

                    let task = self.project.update(cx, |project, cx| {
                        project.copy_entry(selection.entry_id, (worktree_id, new_path).into(), cx)
                    });
                    copy_tasks.push(task);
                    disambiguation_range = new_disambiguation_range.or(disambiguation_range);
                }

                let item_count = copy_tasks.len();

                cx.spawn_in(window, async move |project_panel, cx| {
                    let mut last_succeed = None;
                    let mut changes = Vec::new();
                    for task in copy_tasks {
                        if let Some(Some(entry)) = task.await.log_err() {
                            last_succeed = Some(entry.id);
                            changes.push(Change::Created((worktree_id, entry.path).into()));
                        }
                    }
                    // update selection
                    if let Some(entry_id) = last_succeed {
                        project_panel
                            .update_in(cx, |project_panel, window, cx| {
                                project_panel.selection = Some(SelectedEntry { worktree_id, entry_id });
                                // if only one entry was dragged and it was disambiguated, open the rename editor
                                if item_count == 1 && disambiguation_range.is_some() {
                                    project_panel.rename_impl(disambiguation_range, window, cx);
                                }

                                project_panel.undo_manager.record(changes)
                            })?
                            .log_err();
                    }

                    result::Result::Ok::<(), anyhow::Error>(())
                })
                .detach();
                Some(())
            })();
        } else {
            let update_marks = !self.marked_entries.is_empty();
            let active_selection = selections.active_selection;

            // For folded selections, track the leaf suffix relative to the resolved
            // entry so we can refresh it after the move completes.
            let (folded_selection_info, folded_selection_entries): (
                Vec<(ProjectEntryId, RelPathBuf)>,
                HashSet<SelectedEntry>,
            ) = {
                let project = self.project.read(cx);
                let mut info = Vec::new();
                let mut folded_entries = HashSet::default();

                for selection in selections.items() {
                    let resolved_id = self.resolve_entry(selection.entry_id);
                    if resolved_id == selection.entry_id {
                        continue;
                    }
                    folded_entries.insert(*selection);
                    let Some(source_path) = project.path_for_entry(resolved_id, cx) else {
                        continue;
                    };
                    let Some(leaf_path) = project.path_for_entry(selection.entry_id, cx) else {
                        continue;
                    };
                    let Ok(suffix) = leaf_path.path.strip_prefix(source_path.path.as_ref()) else {
                        continue;
                    };
                    if suffix.as_unix_str().is_empty() {
                        continue;
                    }

                    info.push((resolved_id, suffix.to_rel_path_buf()));
                }
                (info, folded_entries)
            };

            // Capture old paths before moving so we can record undo operations.
            let old_paths: FxHashMap<ProjectEntryId, ProjectPath> = {
                let project = self.project.read(cx);
                entries
                    .iter()
                    .filter_map(|entry| {
                        let path = project.path_for_entry(entry.entry_id, cx)?;
                        Some((entry.entry_id, path))
                    })
                    .collect()
            };
            let destination_worktree_id =
                self.project.read(cx).worktree_for_entry(target_entry_id, cx).map(|wt| wt.read(cx).id());

            // Collect move tasks paired with their source entry ID so we can correlate
            // results with folded selections that need refreshing, plus the display
            // names used to explain a failed move.
            let mut move_tasks = Vec::new();
            for entry in entries {
                if let Some((destination, task)) =
                    self.move_worktree_entry(entry.entry_id, target_entry_id, is_file, cx)
                {
                    let names = old_paths.get(&entry.entry_id).map(|from| {
                        let project = self.project.read(cx);
                        (project_path_display(project, from, cx), project_path_display(project, &destination, cx))
                    });
                    move_tasks.push((entry.entry_id, names, task));
                }
            }

            if move_tasks.is_empty() {
                return;
            }

            let workspace = self.workspace.clone();
            if folded_selection_info.is_empty() {
                cx.spawn_in(window, async move |project_panel, cx| {
                    let mut changes = Vec::new();
                    for (entry_id, names, task) in move_tasks {
                        if let Some(CreatedEntry::Included(new_entry)) = task
                            .await
                            .map_err(|err| explain_move_error(err, names.as_ref()))
                            .notify_workspace_async_err(workspace.clone(), cx)
                            && let (Some(old_path), Some(worktree_id)) =
                                (old_paths.get(&entry_id), destination_worktree_id)
                        {
                            changes.push(Change::Renamed(old_path.clone(), (worktree_id, new_entry.path).into()));
                        }
                    }
                    project_panel
                        .update(cx, |this, _| {
                            this.undo_manager.record(changes).log_err();
                        })
                        .ok();
                })
                .detach();
            } else {
                cx.spawn_in(window, async move |project_panel, cx| {
                    // Await all move tasks and collect successful results
                    let mut move_results: Vec<(ProjectEntryId, Entry)> = Vec::new();
                    let mut operations = Vec::new();
                    for (entry_id, names, task) in move_tasks {
                        if let Some(CreatedEntry::Included(new_entry)) = task
                            .await
                            .map_err(|err| explain_move_error(err, names.as_ref()))
                            .notify_workspace_async_err(workspace.clone(), cx)
                        {
                            if let (Some(old_path), Some(worktree_id)) =
                                (old_paths.get(&entry_id), destination_worktree_id)
                            {
                                operations.push(Change::Renamed(
                                    old_path.clone(),
                                    (worktree_id, Arc::clone(&new_entry.path)).into(),
                                ));
                            }
                            move_results.push((entry_id, new_entry));
                        }
                    }

                    if move_results.is_empty() {
                        return;
                    }

                    project_panel
                        .update(cx, |this, _| {
                            this.undo_manager.record(operations).log_err();
                        })
                        .ok();

                    // For folded selections, we need to refresh the leaf paths (with suffixes)
                    // because they may not be indexed yet after the parent directory was moved.
                    // First collect the paths to refresh, then refresh them.
                    let paths_to_refresh: Vec<(Entity<Worktree>, Arc<RelPath>)> = project_panel
                        .update(cx, |project_panel, cx| {
                            let project = project_panel.project.read(cx);
                            folded_selection_info
                                .iter()
                                .filter_map(|(resolved_id, suffix)| {
                                    let (_, new_entry) = move_results.iter().find(|(id, _)| id == resolved_id)?;
                                    let worktree = project.worktree_for_entry(new_entry.id, cx)?;
                                    let leaf_path = new_entry.path.join(suffix);
                                    Some((worktree, leaf_path.into()))
                                })
                                .collect()
                        })
                        .ok()
                        .unwrap_or_default();

                    let refresh_tasks: Vec<_> = paths_to_refresh
                        .into_iter()
                        .map(|(worktree, leaf_path)| {
                            worktree.update(cx, |worktree, cx| worktree.refresh_entry(leaf_path, None, cx))
                        })
                        .collect();

                    for task in refresh_tasks {
                        task.await.log_err();
                    }

                    if update_marks && !folded_selection_entries.is_empty() {
                        project_panel
                            .update(cx, |project_panel, cx| {
                                project_panel.marked_entries.retain(|entry| {
                                    !folded_selection_entries.contains(entry) || *entry == active_selection
                                });
                                cx.notify();
                            })
                            .ok();
                    }
                })
                .detach();
            }
        }
    }
}
