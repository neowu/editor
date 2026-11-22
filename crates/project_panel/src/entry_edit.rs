use std::cmp;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::collections::hash_map;
use std::iter;
use std::ops::Range;
use std::sync::Arc;

use actions::menu::SelectLast;
use anyhow::Result;
use editor::MultiBufferOffset;
use fs::TrashId;
use gpui::Context;
use gpui::PromptLevel;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::Window;
use notifications::status_toast::StatusToast;
use project::ProjectPath;
use project::WorktreeId;
use project::git_store::git_traversal::ChildEntriesGitIter;
use ui::prelude::Color;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use util::ResultExt as _;
use util::markdown::MarkdownInlineCode;
use util::paths::compare_paths;
use util::rel_path::RelPath;
use workspace::OpenOptions;
use workspace::OpenVisible;
use workspace::SelectedEntry;
use worktree::CreatedEntry;

use super::Delete;
use super::EditState;
use super::NEW_ENTRY_ID;
use super::NewDirectory;
use super::NewFile;
use super::ProjectPanel;
use super::RemovalPrompt;
use super::Rename;
use super::TemporaryUnfoldedPendingState;
use super::TrashEntryOutcome;
use super::ValidationState;
use super::sort_worktree_entries;
use crate::undo as crate_undo;
use crate::undo::Change;

impl ProjectPanel {
    pub(super) fn populate_validation_error(&mut self, cx: &mut Context<Self>) {
        let Some(edit_state) = self.state.edit_state.as_mut() else {
            return;
        };
        let filename = self.filename_editor.read(cx).text(cx);
        if !filename.is_empty() {
            let trimmed_filename = filename.trim();
            if trimmed_filename.is_empty() {
                edit_state.validation_state =
                    ValidationState::Error("File or directory name cannot be empty.".to_owned());
                cx.notify();
                return;
            }

            if trimmed_filename != filename {
                edit_state.validation_state = ValidationState::Warning(
                    "File or directory name contains leading or trailing whitespace.".to_owned(),
                );
                cx.notify();
                return;
            }
            let trimmed_filename = trimmed_filename.trim_start_matches('/');

            let Ok(filename) = RelPath::from_unix_str(trimmed_filename) else {
                edit_state.validation_state = ValidationState::Warning(
                    "File or directory name contains leading or trailing whitespace.".to_owned(),
                );
                cx.notify();
                return;
            };

            let project = self.project.read(cx);
            if let Some(worktree) = project.worktree_for_id(edit_state.worktree_id, cx)
                && let Some(entry) = worktree.read(cx).entry_for_id(edit_state.entry_id)
            {
                let mut already_exists = false;
                if edit_state.is_new_entry() {
                    let new_path = entry.path.join(filename);
                    if worktree.read(cx).entry_for_path(&new_path).is_some() {
                        already_exists = true;
                    }
                } else {
                    let new_path = match Arc::clone(&entry.path).parent() {
                        Some(parent) => parent.join(filename),
                        None => filename.to_owned(),
                    };

                    // We skip the collision check for worktree roots as the
                    // lookup resolves paths relative to the root itself,
                    // whereas renaming the root should resolve it against its
                    // parent directory. Otherwise, renaming `bar/` to `foo/`
                    // would report a false collision with `bar/foo/`.
                    if !project.entry_is_worktree_root(entry.id, cx)
                        && let Some(existing) = worktree.read(cx).entry_for_path(&new_path)
                        && existing.id != entry.id
                    {
                        already_exists = true;
                    }
                }
                if already_exists {
                    edit_state.validation_state = ValidationState::Error(format!(
                        "File or directory '{}' already exists at location. Please choose a different name.",
                        filename.as_unix_str()
                    ));
                    cx.notify();
                    return;
                }
            }
        }
        edit_state.validation_state = ValidationState::None;
        cx.notify();
    }

    pub(super) fn confirm_edit(
        &mut self,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        let edit_state = self.state.edit_state.as_mut()?;
        let worktree_id = edit_state.worktree_id;
        let is_new_entry = edit_state.is_new_entry();
        let filename = self.filename_editor.read(cx).text(cx);
        if filename.trim().is_empty() {
            return None;
        }

        let filename_indicates_dir = filename.ends_with('/');
        let filename = filename.trim_start_matches('/');
        let filename = RelPath::new(filename.as_ref()).ok()?.into_arc();

        edit_state.is_dir = edit_state.is_dir || (edit_state.is_new_entry() && filename_indicates_dir);
        let is_dir = edit_state.is_dir;
        let worktree = self.project.read(cx).worktree_for_id(worktree_id, cx)?;
        let entry = worktree.read(cx).entry_for_id(edit_state.entry_id)?.clone();

        let (edit_task, edited_entry_id, changes) = if is_new_entry {
            self.selection = Some(SelectedEntry { worktree_id, entry_id: NEW_ENTRY_ID });
            let new_path = entry.path.join(&filename);
            if worktree.read(cx).entry_for_path(&new_path).is_some() {
                return None;
            }

            let new_project_path: ProjectPath = (worktree_id, new_path).into();
            let edit_task =
                self.project.update(cx, |project, cx| project.create_entry(new_project_path.clone(), is_dir, cx));
            (edit_task, NEW_ENTRY_ID, vec![Change::Created(new_project_path)])
        } else {
            let new_path = match entry.path.parent() {
                Some(parent) => parent.join(&filename).into(),
                None => Arc::clone(&filename),
            };

            // We skip the collision check for worktree roots as the lookup
            // resolves paths relative to the root itself, whereas renaming the
            // root should resolve it against its parent directory. Otherwise,
            // renaming `bar/` to `foo/` would report a false collision with
            // `bar/foo/`.
            if !self.project.read(cx).entry_is_worktree_root(entry.id, cx)
                && let Some(existing) = worktree.read(cx).entry_for_path(&new_path)
            {
                if existing.id == entry.id && refocus {
                    window.focus(&self.focus_handle, cx);
                }
                return None;
            }
            let edited_entry_id = entry.id;
            let new_project_path: ProjectPath = (worktree_id, new_path).into();

            // Before renaming, keep track of which directories will need to be
            // created, so we can remove these when undoing.
            let created_dirs =
                crate_undo::missing_parent_dirs(worktree.read(cx), worktree_id, new_project_path.path.as_ref());

            let edit_task = self
                .project
                .update(cx, |project, cx| project.rename_entry(edited_entry_id, new_project_path.clone(), cx));

            // Record the directory creations shallowest-first, then the rename,
            // so undoing reverses them in the right order.
            let changes = created_dirs
                .into_iter()
                .rev()
                .map(Change::DirCreated)
                .chain(iter::once(Change::Renamed((worktree_id, entry.path).into(), new_project_path)))
                .collect();
            (edit_task, edited_entry_id, changes)
        };

        if refocus {
            window.focus(&self.focus_handle, cx);
        }
        edit_state.processing_filename = Some(filename);
        cx.notify();

        Some(cx.spawn_in(window, async move |project_panel, cx| {
            let new_entry = edit_task.await;
            project_panel.update(cx, |project_panel, cx| {
                project_panel.state.edit_state = None;
                cx.notify();
            })?;

            match new_entry {
                Err(e) => {
                    project_panel
                        .update_in(cx, |project_panel, window, cx| {
                            project_panel.marked_entries.clear();
                            project_panel.update_visible_entries(None, false, false, window, cx);
                        })
                        .ok();
                    return Err(e);
                }
                Ok(CreatedEntry::Included(new_entry)) => {
                    project_panel.update_in(cx, |project_panel, window, cx| {
                        // Only recording changes in included files as otherwise
                        // undoing some operations would fail, as
                        // `Worktree::entry_for_path` would return `None` for
                        // excluded paths.
                        // In the future we can look into adding support for recording
                        // changes in excluded paths, but that would mean updating
                        // methods that rely on `EntryId` to now rely on the actual
                        // paths.
                        project_panel.undo_manager.record(changes).log_err();

                        if let Some(selection) = &mut project_panel.selection
                            && selection.entry_id == edited_entry_id
                        {
                            selection.worktree_id = worktree_id;
                            selection.entry_id = new_entry.id;
                            project_panel.marked_entries.clear();
                            project_panel.expand_to_selection(cx);
                        }
                        project_panel.update_visible_entries(None, false, false, window, cx);
                        if is_new_entry && !is_dir {
                            project_panel.open_entry(new_entry.id, true, false, cx);
                        }
                        cx.notify();
                    })?;
                }
                Ok(CreatedEntry::Excluded { abs_path }) => {
                    if let Some(open_task) = project_panel
                        .update_in(cx, |project_panel, window, cx| {
                            project_panel.marked_entries.clear();
                            project_panel.update_visible_entries(None, false, false, window, cx);

                            if is_dir {
                                project_panel.project.update(cx, |_, cx| {
                                    cx.emit(project::Event::Toast {
                                        notification_id: "excluded-directory".into(),
                                        message: format!(
                                            concat!(
                                                "Created an excluded directory at {:?}.\n",
                                                "Alter `file_scan_exclusions` in the settings ",
                                                "to show it in the panel"
                                            ),
                                            abs_path
                                        ),
                                        link: None,
                                    });
                                });
                                None
                            } else {
                                project_panel
                                    .workspace
                                    .update(cx, |workspace, cx| {
                                        workspace.open_abs_path(
                                            abs_path,
                                            OpenOptions { visible: Some(OpenVisible::All), ..Default::default() },
                                            window,
                                            cx,
                                        )
                                    })
                                    .ok()
                            }
                        })
                        .ok()
                        .flatten()
                    {
                        open_task.await?;
                    }
                }
            }
            Ok(())
        }))
    }

    async fn resolve_trash_entry(task: Option<Task<Result<TrashId>>>, worktree_id: WorktreeId) -> TrashEntryOutcome {
        let Some(task) = task else {
            return TrashEntryOutcome::Missing;
        };

        match task.await {
            Ok(trash_id) => TrashEntryOutcome::Trashed(Change::Trashed(worktree_id, trash_id)),
            Err(err) => {
                log::error!("Failed to trash an entry: {err:#}");
                TrashEntryOutcome::Failed
            }
        }
    }

    pub(super) fn discard_edit_state(&mut self, window: &Window, cx: &Context<Self>) {
        if let Some(edit_state) = self.state.edit_state.take() {
            self.state.temporarily_unfolded_pending_state =
                edit_state.temporarily_unfolded.and_then(|temporarily_unfolded_entry_id| {
                    let previously_focused_leaf_entry = edit_state.previously_focused?;
                    let folded_ancestors = self.state.ancestors.get(&temporarily_unfolded_entry_id)?;
                    Some(TemporaryUnfoldedPendingState {
                        previously_focused_leaf_entry,
                        temporarily_unfolded_active_entry_id: folded_ancestors
                            .active_ancestor()
                            .unwrap_or(temporarily_unfolded_entry_id),
                    })
                });
            let previously_focused = edit_state.previously_focused.map(|entry| (entry.worktree_id, entry.entry_id));
            self.update_visible_entries(previously_focused, false, previously_focused.is_some(), window, cx);
        }
    }

    pub(super) fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        self.add_entry(false, window, cx);
    }

    pub(super) fn new_directory(&mut self, _: &NewDirectory, window: &mut Window, cx: &mut Context<Self>) {
        self.add_entry(true, window, cx);
    }

    fn add_entry(&mut self, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((worktree_id, entry_id)) =
            self.selection.map(|entry| (entry.worktree_id, entry.entry_id)).or_else(|| {
                let entry_id = self.state.last_worktree_root_id?;
                let worktree_id = self.project.read(cx).worktree_for_entry(entry_id, cx)?.read(cx).id();

                self.selection = Some(SelectedEntry { worktree_id, entry_id });

                Some((worktree_id, entry_id))
            })
        else {
            return;
        };
        if self.project.read(cx).is_library_worktree(worktree_id, cx) {
            return;
        }

        let directory_id;
        let new_entry_id = self.resolve_entry(entry_id);
        if let Some(worktree) = self.project.read(cx).worktree_for_id(worktree_id, cx) {
            let worktree = worktree.read(cx);
            let expanded_dir_ids = match self.state.expanded_dir_ids.entry(worktree_id) {
                hash_map::Entry::Occupied(entry) => entry.into_mut(),
                hash_map::Entry::Vacant(entry) => {
                    let Some(root_entry_id) = worktree.root_entry().map(|entry| entry.id) else {
                        return;
                    };
                    entry.insert(vec![root_entry_id])
                }
            };

            if let Some(mut entry) = worktree.entry_for_id(new_entry_id) {
                loop {
                    if entry.is_dir() {
                        if let Err(ix) = expanded_dir_ids.binary_search(&entry.id) {
                            expanded_dir_ids.insert(ix, entry.id);
                        }
                        directory_id = entry.id;
                        break;
                    }
                    if let Some(parent_path) = entry.path.parent()
                        && let Some(parent_entry) = worktree.entry_for_path(parent_path)
                    {
                        entry = parent_entry;
                        continue;
                    }
                    return;
                }
            } else {
                return;
            }
        } else {
            return;
        }

        self.marked_entries.clear();
        self.state.edit_state = Some(EditState {
            worktree_id,
            entry_id: directory_id,
            leaf_entry_id: None,
            is_dir,
            processing_filename: None,
            previously_focused: self.selection,
            depth: 0,
            validation_state: ValidationState::None,
            temporarily_unfolded: (new_entry_id != entry_id).then_some(new_entry_id),
        });
        self.filename_editor.update(cx, |editor, cx| {
            editor.clear(window, cx);
        });
        self.update_visible_entries(Some((worktree_id, NEW_ENTRY_ID)), true, true, window, cx);
        cx.notify();
    }

    pub(super) fn rename_impl(&mut self, selection: Option<Range<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(SelectedEntry { worktree_id, entry_id }) = self.selection
            && let Some(worktree) = self.project.read(cx).worktree_for_id(worktree_id, cx)
        {
            if worktree.read(cx).library_name().is_some() {
                return;
            }
            let sub_entry_id = self.unflatten_entry_id(entry_id);
            if let Some(entry) = worktree.read(cx).entry_for_id(sub_entry_id) {
                self.state.edit_state = Some(EditState {
                    worktree_id,
                    entry_id: sub_entry_id,
                    leaf_entry_id: Some(entry_id),
                    is_dir: entry.is_dir(),
                    processing_filename: None,
                    previously_focused: None,
                    depth: 0,
                    validation_state: ValidationState::None,
                    temporarily_unfolded: None,
                });
                let file_name = entry.path.file_name().unwrap_or_default().to_owned();
                let selection = selection.unwrap_or_else(|| {
                    // Folders have no extension, so select the whole name. Only
                    // files keep their extension unselected for quick renames.
                    let selection_end = if entry.is_dir() {
                        file_name.len()
                    } else {
                        let file_stem = entry.path.file_stem();
                        file_stem.map_or(file_name.len(), str::len)
                    };
                    0..selection_end
                });
                self.filename_editor.update(cx, |editor, cx| {
                    editor.set_text(file_name, window, cx);
                    editor.change_selections(editor::SelectionEffects::default(), window, cx, |s| {
                        s.select_ranges([MultiBufferOffset(selection.start)..MultiBufferOffset(selection.end)]);
                    });
                });
                self.update_visible_entries(None, true, true, window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_impl(None, window, cx);
    }

    /// Builds the confirmation prompt shared by direct removal and undo/redo.
    ///
    /// The `names` slice must contain every path being removed, and this
    /// function will apply markdown formatting and display truncation. The
    /// `dirty_buffers` parameter should be the number of those paths that have
    /// unsaved changes.
    pub(super) fn build_removal_prompt<S>(names: &[S], dirty_buffers: usize) -> RemovalPrompt
    where
        S: AsRef<str>,
    {
        let message_start = "Do you want to delete";

        let mut message = if let [name] = names {
            format!("{message_start} {}?", MarkdownInlineCode(name.as_ref()))
        } else {
            const CUTOFF_POINT: usize = 10;
            let mut listed_names = names
                .iter()
                .take(CUTOFF_POINT)
                .map(|name| MarkdownInlineCode(name.as_ref()).to_string())
                .collect::<Vec<_>>();
            let omitted_count = names.len().saturating_sub(CUTOFF_POINT);
            if omitted_count == 1 {
                listed_names.push(".. 1 file not shown".into());
            } else if omitted_count > 1 {
                listed_names.push(format!(".. {omitted_count} files not shown"));
            }

            format!("{message_start} the following {} files?\n{}", names.len(), listed_names.join("\n"))
        };
        match dirty_buffers {
            0 => {}
            1 if names.len() == 1 => {
                message.push_str("\n\nIt has unsaved changes, which will be lost.");
            }
            1 => {
                message.push_str("\n\n1 of these has unsaved changes, which will be lost.");
            }
            dirty_buffers => {
                util::write_str!(message, "\n\n{dirty_buffers} of these have unsaved changes, which will be lost.");
            }
        }

        RemovalPrompt { message, confirmation_label: "Delete" }
    }

    /// Moves the selection to the system trash and records it for undo.
    pub(super) fn delete(&mut self, action: &Delete, window: &mut Window, cx: &mut Context<ProjectPanel>) {
        (|| {
            let items_to_delete = self.disjoint_effective_entries_excluding_roots(cx);
            let project = self.project.read(cx);
            if items_to_delete.is_empty()
                || items_to_delete.iter().any(|selection| project.is_library_worktree(selection.worktree_id, cx))
            {
                return None;
            }

            let mut dirty_buffers = 0;
            let file_paths = items_to_delete
                .iter()
                .filter_map(|selection| {
                    let project_path = project.path_for_entry(selection.entry_id, cx)?;
                    dirty_buffers += project.dirty_buffers(cx).any(|path| path == project_path) as usize;

                    Some((selection.entry_id, selection.worktree_id, project_path.path.file_name()?.to_owned()))
                })
                .collect::<Vec<_>>();
            if file_paths.is_empty() {
                return None;
            }
            let answer = if action.skip_prompt {
                None
            } else {
                let names = file_paths.iter().map(|(_, _, name)| name.as_str()).collect::<Vec<_>>();
                let prompt = ProjectPanel::build_removal_prompt(&names, dirty_buffers);

                Some(window.prompt(
                    PromptLevel::Info,
                    &prompt.message,
                    None,
                    &[prompt.confirmation_label, "Cancel"],
                    cx,
                ))
            };
            let next_selection = self.find_next_selection_after_deletion(items_to_delete, cx);
            cx.spawn_in(window, async move |panel, cx| {
                if let Some(answer) = answer
                    && answer.await != Ok(0)
                {
                    return anyhow::Ok(());
                }

                let mut changes = Vec::new();
                let total_count = file_paths.len();
                let mut failed_count = 0;

                let tasks = panel.update(cx, |panel, cx| {
                    panel.project.update(cx, |project, cx| {
                        file_paths
                            .into_iter()
                            .map(|(entry_id, worktree_id, _)| (worktree_id, project.trash_entry(entry_id, cx)))
                            .collect::<Vec<_>>()
                    })
                })?;

                for (worktree_id, task) in tasks {
                    match ProjectPanel::resolve_trash_entry(task, worktree_id).await {
                        TrashEntryOutcome::Missing => {}
                        TrashEntryOutcome::Trashed(change) => changes.push(change),
                        TrashEntryOutcome::Failed => failed_count += 1,
                    }
                }

                panel.update_in(cx, |panel, window, cx| {
                    panel.undo_manager.record(changes).log_err();

                    if failed_count > 0 {
                        panel.show_remove_failure_toast(total_count, failed_count, cx);
                    }

                    if let Some(next_selection) = next_selection {
                        panel.update_visible_entries(
                            Some((next_selection.worktree_id, next_selection.entry_id)),
                            false,
                            true,
                            window,
                            cx,
                        );
                    } else {
                        panel.select_last(&SelectLast {}, window, cx);
                    }
                })?;
                Ok(())
            })
            .detach_and_log_err(cx);
            Some(())
        })();
    }

    /// Displays a toast notification, informing that some of the selected
    /// files could not be moved to the trash.
    fn show_remove_failure_toast(&self, total_count: usize, failed_count: usize, cx: &mut Context<Self>) {
        let noun = if total_count == 1 { "file" } else { "files" };
        let message = format!("Failed to move {failed_count} of {total_count} {noun} to the trash.");

        let toast = StatusToast::new(message, cx, |this, _| {
            this.icon(Icon::new(IconName::XCircle).size(IconSize::Small).color(Color::Error)).dismiss_button(true)
        });

        self.workspace
            .update(cx, |workspace, cx| {
                workspace.toggle_status_toast(toast, cx);
            })
            .ok();
    }

    fn find_next_selection_after_deletion(
        &self,
        sanitized_entries: BTreeSet<SelectedEntry>,
        cx: &Context<Self>,
    ) -> Option<SelectedEntry> {
        if sanitized_entries.is_empty() {
            return None;
        }
        let project = self.project.read(cx);
        let (worktree_id, worktree) = sanitized_entries
            .iter()
            .map(|entry| entry.worktree_id)
            .filter_map(|id| project.worktree_for_id(id, cx).map(|w| (id, w.read(cx))))
            .max_by(|(_, a), (_, b)| a.root_name().cmp(b.root_name()))?;
        let git_store = project.git_store().read(cx);

        let marked_entries_in_worktree =
            sanitized_entries.iter().filter(|e| e.worktree_id == worktree_id).collect::<HashSet<_>>();
        let latest_entry = marked_entries_in_worktree
            .iter()
            .max_by(|a, b| match (worktree.entry_for_id(a.entry_id), worktree.entry_for_id(b.entry_id)) {
                (Some(a), Some(b)) => {
                    compare_paths((a.path.as_std_path(), a.is_file()), (b.path.as_std_path(), b.is_file()))
                }
                _ => cmp::Ordering::Equal,
            })
            .and_then(|e| worktree.entry_for_id(e.entry_id))?;

        let parent_path = latest_entry.path.parent()?;
        let parent_entry = worktree.entry_for_path(parent_path)?;

        // Remove all siblings that are being deleted except the last marked entry
        let repo_snapshots = git_store.repo_snapshots(cx);
        let worktree_snapshot = worktree.snapshot();
        let hide_gitignore = self.hide_gitignore;
        let mut siblings: Vec<_> = ChildEntriesGitIter::new(&repo_snapshots, &worktree_snapshot, parent_path)
            .filter(|sibling| {
                (sibling.id == latest_entry.id)
                    || (!marked_entries_in_worktree.contains(&&SelectedEntry { worktree_id, entry_id: sibling.id })
                        && (!hide_gitignore || !sibling.is_ignored))
            })
            .map(project::GitEntryRef::to_owned)
            .collect();

        sort_worktree_entries(&mut siblings);
        let sibling_entry_index = siblings.iter().position(|sibling| sibling.id == latest_entry.id)?;

        if let Some(next_sibling) = sibling_entry_index.checked_add(1).and_then(|i| siblings.get(i)) {
            return Some(SelectedEntry { worktree_id, entry_id: next_sibling.id });
        }
        if let Some(prev_sibling) = sibling_entry_index.checked_sub(1).and_then(|i| siblings.get(i)) {
            return Some(SelectedEntry { worktree_id, entry_id: prev_sibling.id });
        }
        // No neighbour sibling found, fall back to parent
        Some(SelectedEntry { worktree_id, entry_id: parent_entry.id })
    }
}
