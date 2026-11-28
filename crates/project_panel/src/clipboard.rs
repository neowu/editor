use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use gpui::App;
use gpui::ClipboardEntry as GpuiClipboardEntry;
use gpui::ClipboardItem;
use gpui::Context;
use gpui::Entity;
use gpui::ExternalPaths;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::Window;
use project::Entry;
use project::ProjectPath;
use project::Worktree;
use util::ResultExt as _;
use util::rel_path::RelPath;
use workspace::SelectedEntry;
use workspace::notifications::NotifyResultExt as _;
use worktree::CreatedEntry;

use super::ClipboardEntry;
use super::Copy;
use super::Cut;
use super::Duplicate;
use super::EntryTransferOperation;
use super::Paste;
use super::ProjectPanel;
use crate::undo::Change;

impl ProjectPanel {
    pub(super) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let entries = self.disjoint_effective_entries_excluding_roots(cx);
        if !entries.is_empty() {
            self.write_entries_to_system_clipboard(&entries, cx);
            self.clipboard = Some(ClipboardEntry::Cut(entries));
            cx.notify();
        }
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let entries = self.disjoint_effective_entries_excluding_roots(cx);
        if !entries.is_empty() {
            self.write_entries_to_system_clipboard(&entries, cx);
            self.clipboard = Some(ClipboardEntry::Copied(entries));
            cx.notify();
        }
    }

    pub(super) fn create_paste_path(
        &self,
        source: &SelectedEntry,
        (worktree, target_entry): (Entity<Worktree>, &Entry),
        cx: &App,
    ) -> Option<(Arc<RelPath>, Option<Range<usize>>)> {
        let mut new_path = target_entry.path.to_rel_path_buf();
        // If we're pasting into a file, or a directory into itself, go up one level.
        if target_entry.is_file() || (target_entry.is_dir() && target_entry.id == source.entry_id) {
            new_path.pop();
        }

        let source_worktree = self.project.read(cx).worktree_for_entry(source.entry_id, cx)?;
        let source_entry = source_worktree.read(cx).entry_for_id(source.entry_id)?;

        let source_file_name = source_entry.path.file_name()?.to_owned();
        new_path.push(RelPath::from_unix_str(&source_file_name).expect("file name is a valid relative path"));

        let (extension, file_name_without_extension) = if source_entry.is_file() {
            (new_path.extension().map(str::to_owned), new_path.file_stem()?.to_owned())
        } else {
            (None, source_file_name)
        };

        let file_name_len = file_name_without_extension.len();
        let mut disambiguation_range = None;
        let mut ix = 0;
        {
            let worktree = worktree.read(cx);
            while worktree.entry_for_path(&new_path).is_some() {
                new_path.pop();

                let mut new_file_name = file_name_without_extension.clone();

                let disambiguation = " copy";
                let mut disambiguation_len = disambiguation.len();

                new_file_name.push_str(disambiguation);

                if ix > 0 {
                    let extra_disambiguation = format!(" {ix}");
                    disambiguation_len += extra_disambiguation.len();
                    new_file_name.push_str(&extra_disambiguation);
                }
                if let Some(extension) = extension.as_ref() {
                    new_file_name.push('.');
                    new_file_name.push_str(extension);
                }

                new_path.push(RelPath::from_unix_str(&new_file_name).expect("file name is a valid relative path"));

                disambiguation_range = Some(0..(file_name_len + disambiguation_len));
                ix += 1;
            }
        }
        Some((new_path.as_rel_path().into(), disambiguation_range))
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        // Files copied in another window (or Finder) arrive as external paths;
        // files this panel copied itself go through the internal clipboard so
        // cut-moves and "copy" renaming keep working.
        if let Some(external_paths) = self.external_paths_from_system_clipboard(cx)
            && self.internal_clipboard_paths(cx).as_deref() != Some(external_paths.paths())
        {
            let target_entry_id = self.selection.map(|s| s.entry_id).or(self.state.last_worktree_root_id);
            if let Some(entry_id) = target_entry_id {
                // lint: expect(module_cycle)
                self.drop_external_files(external_paths.paths(), entry_id, window, cx);
            }
            return;
        }

        self.paste_from_internal_clipboard(window, cx);
    }

    fn paste_from_internal_clipboard(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(clipboard_entry) =
            self.clipboard.clone().filter(|clipboard_entry| !clipboard_entry.items().is_empty())
        else {
            return;
        };

        let (entries, operation) = match clipboard_entry {
            ClipboardEntry::Copied(entries) => (entries, EntryTransferOperation::Copy),
            ClipboardEntry::Cut(entries) => (entries, EntryTransferOperation::Move),
        };
        if self.transfer_entries(entries, operation, window, cx).is_some()
            && matches!(operation, EntryTransferOperation::Move)
        {
            // Convert the clipboard cut entry to a copy entry after the first paste.
            self.clipboard = self.clipboard.take().map(ClipboardEntry::into_copy_entry);
        }
    }

    /// Copies or moves `entries` into the selected entry, without touching either clipboard.
    fn transfer_entries(
        &mut self,
        entries: BTreeSet<SelectedEntry>,
        operation: EntryTransferOperation,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<()> {
        let (worktree, entry) = self.selected_entry_handle(cx)?;
        let entry = entry.clone();
        let worktree_id = worktree.read(cx).id();

        enum EntryTransferTask {
            Rename { task: Task<Result<CreatedEntry>>, from: ProjectPath, to: ProjectPath },
            Copy { task: Task<Result<Option<Entry>>>, destination: ProjectPath },
        }

        let mut transfer_tasks = Vec::new();
        let mut disambiguation_range = None;
        for source_entry in &entries {
            let (new_path, new_disambiguation_range) =
                self.create_paste_path(source_entry, self.selected_sub_entry(cx)?, cx)?;
            let source_entry_id = source_entry.entry_id;
            let destination: ProjectPath = (worktree_id, new_path).into();
            let task = match operation {
                EntryTransferOperation::Copy => {
                    let task = self
                        .project
                        .update(cx, |project, cx| project.copy_entry(source_entry_id, destination.clone(), cx));
                    EntryTransferTask::Copy { task, destination }
                }
                EntryTransferOperation::Move => {
                    let original_path = self.project.read(cx).path_for_entry(source_entry_id, cx)?;
                    let task = self
                        .project
                        .update(cx, |project, cx| project.rename_entry(source_entry_id, destination.clone(), cx));
                    EntryTransferTask::Rename { task, from: original_path, to: destination }
                }
            };
            transfer_tasks.push(task);
            disambiguation_range = new_disambiguation_range.or(disambiguation_range);
        }

        let item_count = transfer_tasks.len();
        let workspace = self.workspace.clone();

        cx.spawn_in(window, async move |project_panel, cx| {
            let mut last_succeed = None;
            let mut changes = Vec::new();

            for task in transfer_tasks {
                match task {
                    EntryTransferTask::Rename { task, from, to } => {
                        if let Some(CreatedEntry::Included(entry)) =
                            task.await.notify_workspace_async_err(workspace.clone(), cx)
                        {
                            changes.push(Change::Renamed(from, to));
                            last_succeed = Some(entry);
                        }
                    }
                    EntryTransferTask::Copy { task, destination } => {
                        if let Some(Some(entry)) = task.await.notify_workspace_async_err(workspace.clone(), cx) {
                            changes.push(Change::Created(destination));
                            last_succeed = Some(entry);
                        }
                    }
                }
            }

            project_panel
                .update(cx, |this, _| {
                    this.undo_manager.record(changes).log_err();
                })
                .ok();

            // update selection
            if let Some(entry) = last_succeed {
                project_panel
                    .update_in(cx, |project_panel, window, cx| {
                        project_panel.selection = Some(SelectedEntry { worktree_id, entry_id: entry.id });

                        if item_count == 1 {
                            // open entry if not dir, setting is enabled, and only focus if rename is not pending
                            if !entry.is_dir() {
                                project_panel.open_entry(entry.id, disambiguation_range.is_none(), false, cx);
                            }

                            // if only one entry was pasted and it was disambiguated, open the rename editor
                            if disambiguation_range.is_some() {
                                cx.defer_in(window, |this, window, cx| {
                                    this.rename_impl(disambiguation_range, window, cx);
                                });
                            }
                        }
                    })
                    .ok();
            }

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);

        self.expand_entry(worktree_id, entry.id, cx);
        Some(())
    }

    pub(super) fn duplicate(&mut self, _: &Duplicate, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.disjoint_effective_entries_excluding_roots(cx);
        if entries.is_empty() {
            return;
        }

        self.transfer_entries(entries, EntryTransferOperation::Copy, window, cx);
    }

    fn write_entries_to_system_clipboard(&self, entries: &BTreeSet<SelectedEntry>, cx: &App) {
        let project = self.project.read(cx);
        let paths: Vec<String> = entries
            .iter()
            .filter_map(|entry| {
                let worktree = project.worktree_for_id(entry.worktree_id, cx)?;
                let worktree = worktree.read(cx);
                let worktree_entry = worktree.entry_for_id(entry.entry_id)?;
                Some(worktree.abs_path().join(worktree_entry.path.as_std_path()).to_string_lossy().to_string())
            })
            .collect();
        if paths.is_empty() {
            return;
        }
        let text = ClipboardItem::new_string(paths.join("\n"));
        // Files also go on the clipboard as file URLs, so they can be pasted
        // as files into another window's project panel or Finder.
        let files = self.clipboard_file_paths(entries.iter().copied(), cx);
        let item = if files.is_empty() {
            text
        } else {
            ClipboardItem {
                entries: std::iter::once(GpuiClipboardEntry::ExternalPaths(ExternalPaths(files.into_iter().collect())))
                    .chain(text.entries)
                    .collect(),
            }
        };
        cx.write_to_clipboard(item);
    }

    /// Paths of `entries` that exist on this machine: every local entry, plus
    /// remote entries whose path is also visible here (e.g. a VM sharing the
    /// host's folders at the same path).
    fn clipboard_file_paths(&self, entries: impl IntoIterator<Item = SelectedEntry>, cx: &App) -> Vec<PathBuf> {
        let project = self.project.read(cx);
        entries
            .into_iter()
            .filter_map(|selection| {
                let worktree = project.worktree_for_id(selection.worktree_id, cx)?.read(cx);
                let entry = worktree.entry_for_id(selection.entry_id)?;
                Some(worktree.absolutize(&entry.path))
            })
            .collect()
    }

    /// The paths this panel last put on the system clipboard, used to tell
    /// whether the system clipboard still holds what this panel copied.
    fn internal_clipboard_paths(&self, cx: &App) -> Option<Vec<PathBuf>> {
        let clipboard = self.clipboard.as_ref()?;
        Some(self.clipboard_file_paths(clipboard.items().iter().copied(), cx))
    }

    fn external_paths_from_system_clipboard(&self, cx: &App) -> Option<ExternalPaths> {
        let clipboard_item = cx.read_from_clipboard()?;
        for entry in clipboard_item.entries() {
            if let GpuiClipboardEntry::ExternalPaths(paths) = entry
                && !paths.paths().is_empty()
            {
                return Some(paths.clone());
            }
        }
        None
    }

    pub(super) fn has_pasteable_content(&self, cx: &App) -> bool {
        if self.clipboard.as_ref().is_some_and(|c| !c.items().is_empty()) {
            return true;
        }
        self.external_paths_from_system_clipboard(cx).is_some()
    }
}

impl ClipboardEntry {
    pub(super) const fn is_cut(&self) -> bool {
        matches!(self, Self::Cut { .. })
    }

    pub(super) const fn items(&self) -> &BTreeSet<SelectedEntry> {
        match self {
            ClipboardEntry::Copied(entries) | ClipboardEntry::Cut(entries) => entries,
        }
    }

    fn into_copy_entry(self) -> Self {
        match self {
            ClipboardEntry::Copied(_) => self,
            ClipboardEntry::Cut(entries) => ClipboardEntry::Copied(entries),
        }
    }
}
