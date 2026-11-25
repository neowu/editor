use std::cell::OnceCell;
use std::collections::HashSet;
use std::collections::hash_map;
use std::mem;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use git::status::GitSummary;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Focusable as _;
use gpui::Pixels;
use gpui::Window;
use itertools::Itertools as _;
use project::Entry;
use project::EntryKind;
use project::GitEntry;
use project::GitEntryRef;
use project::GitTraversal;
use project::ProjectEntryId;
use project::WorktreeId;
use ui::font;
use ui::prelude::Icon;
use util::ResultExt as _;
use util::rel_path::RelPath;
use workspace::SelectedEntry;
use workspace::Workspace;

use super::EntryDetails;
use super::FoldedAncestors;
use super::LIBRARIES_ENTRY_ID;
use super::LIBRARIES_WORKTREE_ID;
use super::NEW_ENTRY_ID;
use super::ProjectPanel;
use super::State;
use super::UpdateVisibleEntriesTask;
use super::VisibleEntriesForWorktree;
use super::item_width_estimate;
use super::libraries_header_entry;
use super::sort_worktree_entries;
use crate::utils;

impl ProjectPanel {
    pub(super) fn index_for_selection(&self, selection: SelectedEntry) -> Option<(usize, usize, usize)> {
        self.index_for_entry(selection.entry_id, selection.worktree_id)
    }

    fn create_new_git_entry(parent_entry: &Entry, git_summary: GitSummary, new_entry_kind: EntryKind) -> GitEntry {
        GitEntry {
            entry: Entry {
                id: NEW_ENTRY_ID,
                kind: new_entry_kind,
                path: parent_entry
                    .path
                    .join(RelPath::from_unix_str("\0").expect("placeholder is a valid relative path"))
                    .into(),
                inode: 0,
                mtime: parent_entry.mtime,
                size: parent_entry.size,
                is_ignored: parent_entry.is_ignored,
                is_hidden: parent_entry.is_hidden,
                is_external: false,
                is_always_included: parent_entry.is_always_included,
                canonical_path: parent_entry.canonical_path.clone(),
                char_bag: parent_entry.char_bag,
                is_fifo: parent_entry.is_fifo,
                is_writable: true,
            },
            git_summary,
        }
    }

    pub(super) fn update_visible_entries(
        &mut self,
        new_selected_entry: Option<(WorktreeId, ProjectEntryId)>,
        focus_filename_editor: bool,
        autoscroll: bool,
        window: &Window,
        cx: &Context<Self>,
    ) {
        let hide_gitignore = self.hide_gitignore;
        let hide_hidden = self.hide_hidden;
        let project = self.project.read(cx);
        let repo_snapshots = project.git_store().read(cx).repo_snapshots(cx);

        let old_ancestors = self.state.ancestors.clone();
        let temporary_unfolded_pending_state = self.state.temporarily_unfolded_pending_state.take();
        let mut new_state = State::derive(&self.state);
        new_state.last_worktree_root_id = project
            .visible_worktrees(cx)
            .next_back()
            .and_then(|worktree| worktree.read(cx).root_entry())
            .map(|entry| entry.id);
        let mut max_width_item = None;

        // External libraries follow the project roots, each under its own root row.
        let worktrees = project
            .visible_worktrees(cx)
            .map(|worktree| (worktree.read(cx).snapshot(), false))
            .chain(project.library_worktrees(cx).into_iter().map(|worktree| (worktree.read(cx).snapshot(), true)))
            .collect::<Vec<_>>();
        let libraries_expanded = self.libraries_expanded;

        let visible_entries_task = cx.spawn_in(window, async move |this, cx| {
            let new_state = cx
                .background_spawn(async move {
                    let mut libraries_header_pushed = false;
                    for (worktree_snapshot, is_library) in worktrees {
                        if is_library && !libraries_header_pushed {
                            libraries_header_pushed = true;
                            new_state.visible_entries.push(VisibleEntriesForWorktree {
                                worktree_id: LIBRARIES_WORKTREE_ID,
                                depth_offset: 0,
                                entries: vec![libraries_header_entry()],
                                index: OnceCell::new(),
                            });
                        }
                        if is_library && !libraries_expanded {
                            continue;
                        }
                        let worktree_id = worktree_snapshot.id();

                        let (new_entry_parent_id, new_entry_kind) = if let Some(edit_state) = &new_state.edit_state
                            && edit_state.worktree_id == worktree_id
                            && edit_state.is_new_entry()
                        {
                            (
                                Some(edit_state.entry_id),
                                if edit_state.is_dir { EntryKind::Dir } else { EntryKind::File },
                            )
                        } else {
                            (None, EntryKind::Dir)
                        };

                        let mut visible_worktree_entries = Vec::new();
                        let mut entry_iter = GitTraversal::new(&repo_snapshots, worktree_snapshot.entries(true, 0));
                        let mut auto_folded_ancestors = vec![];
                        let worktree_abs_path = worktree_snapshot.abs_path();
                        while let Some(entry) = entry_iter.entry() {
                            if entry.kind.is_dir() {
                                auto_folded_ancestors.push(entry.id);
                                if !new_state.is_unfolded(entry.id)
                                    && let Some(root_path) = worktree_snapshot.root_entry()
                                {
                                    let mut child_entries = worktree_snapshot.child_entries(&entry.path);
                                    if let Some(child) = child_entries.next()
                                        && entry.path != root_path.path
                                        && child_entries.next().is_none()
                                        && child.kind.is_dir()
                                    {
                                        entry_iter.advance();

                                        continue;
                                    }
                                }
                                let depth = temporary_unfolded_pending_state
                                    .as_ref()
                                    .and_then(|state| {
                                        if state.previously_focused_leaf_entry.worktree_id == worktree_id
                                            && state.previously_focused_leaf_entry.entry_id == entry.id
                                        {
                                            auto_folded_ancestors
                                                .iter()
                                                .rev()
                                                .position(|id| *id == state.temporarily_unfolded_active_entry_id)
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or_else(|| {
                                        old_ancestors
                                            .get(&entry.id)
                                            .map_or_default(|ancestor| ancestor.current_ancestor_depth)
                                    })
                                    .min(auto_folded_ancestors.len());
                                if let Some(edit_state) = &mut new_state.edit_state
                                    && edit_state.entry_id == entry.id
                                {
                                    edit_state.depth = depth;
                                }
                                let mut ancestors = mem::take(&mut auto_folded_ancestors);
                                if ancestors.len() > 1 {
                                    ancestors.reverse();
                                    new_state
                                        .ancestors
                                        .insert(entry.id, FoldedAncestors { current_ancestor_depth: depth, ancestors });
                                }
                            }
                            auto_folded_ancestors.clear();
                            if (!hide_gitignore || !entry.is_ignored) && (!hide_hidden || !entry.is_hidden) {
                                visible_worktree_entries.push(entry.to_owned());
                            }
                            let precedes_new_entry = if let Some(new_entry_id) = new_entry_parent_id {
                                entry.id == new_entry_id || {
                                    new_state
                                        .ancestors
                                        .get(&entry.id)
                                        .is_some_and(|entries| entries.ancestors.contains(&new_entry_id))
                                }
                            } else {
                                false
                            };
                            if precedes_new_entry
                                && (!hide_gitignore || !entry.is_ignored)
                                && (!hide_hidden || !entry.is_hidden)
                            {
                                visible_worktree_entries.push(Self::create_new_git_entry(
                                    entry.entry,
                                    entry.git_summary,
                                    new_entry_kind,
                                ));
                            }

                            let (depth, chars) = if Some(entry.entry) == worktree_snapshot.root_entry() {
                                let Some(path_name) = worktree_abs_path.file_name() else {
                                    entry_iter.advance();
                                    continue;
                                };
                                let depth = 0;
                                (depth, path_name.to_string_lossy().chars().count())
                            } else if entry.is_file() {
                                let Some(path_name) = entry
                                    .path
                                    .file_name()
                                    .with_context(|| format!("Non-root entry has no file name: {entry:?}"))
                                    .log_err()
                                else {
                                    continue;
                                };
                                let depth = entry.path.ancestors().count() - 1;
                                (depth, path_name.chars().count())
                            } else {
                                let path = new_state
                                    .ancestors
                                    .get(&entry.id)
                                    .and_then(|ancestors| {
                                        let outermost_ancestor = ancestors.ancestors.last()?;
                                        let root_folded_entry =
                                            worktree_snapshot.entry_for_id(*outermost_ancestor)?.path.as_ref();
                                        let suffix = entry.path.strip_prefix(root_folded_entry).ok()?;
                                        Some(
                                            RelPath::from_unix_str(root_folded_entry.file_name()?)
                                                .expect("file name is a valid relative path")
                                                .join(suffix),
                                        )
                                    })
                                    .or_else(|| {
                                        entry.path.file_name().map(|file_name| {
                                            RelPath::from_unix_str(file_name)
                                                .expect("file name is a valid relative path")
                                                .to_owned()
                                        })
                                    })
                                    .unwrap_or_else(|| entry.path.to_rel_path_buf());
                                let depth = path.components().count();
                                (depth, path.as_unix_str().chars().count())
                            };
                            let width_estimate = item_width_estimate(depth, chars, entry.canonical_path.is_some());

                            match max_width_item.as_mut() {
                                Some((id, worktree_id, width)) => {
                                    if *width < width_estimate {
                                        *id = entry.id;
                                        *worktree_id = worktree_snapshot.id();
                                        *width = width_estimate;
                                    }
                                }
                                None => max_width_item = Some((entry.id, worktree_snapshot.id(), width_estimate)),
                            }

                            let expanded_dir_ids = match new_state.expanded_dir_ids.entry(worktree_id) {
                                hash_map::Entry::Occupied(e) => e.into_mut(),
                                hash_map::Entry::Vacant(e) => {
                                    // The first time a worktree's root entry becomes available,
                                    // mark that root entry as expanded.
                                    if let Some(entry) = worktree_snapshot.root_entry() {
                                        e.insert(vec![entry.id]).as_slice()
                                    } else {
                                        &[]
                                    }
                                }
                            };

                            if expanded_dir_ids.binary_search(&entry.id).is_err() && entry_iter.advance_to_sibling() {
                                continue;
                            }
                            entry_iter.advance();
                        }

                        sort_worktree_entries(&mut visible_worktree_entries);
                        new_state.visible_entries.push(VisibleEntriesForWorktree {
                            worktree_id,
                            depth_offset: usize::from(is_library),
                            entries: visible_worktree_entries,
                            index: OnceCell::new(),
                        });
                    }
                    if let Some((project_entry_id, worktree_id, _)) = max_width_item {
                        let mut visited_worktrees_length = 0;
                        let index = new_state.visible_entries.iter().find_map(|visible_entries| {
                            if worktree_id == visible_entries.worktree_id {
                                visible_entries.entries.iter().position(|entry| entry.id == project_entry_id)
                            } else {
                                visited_worktrees_length += visible_entries.entries.len();
                                None
                            }
                        });
                        if let Some(index) = index {
                            new_state.max_width_item_index = Some(visited_worktrees_length + index);
                        }
                    }
                    new_state
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.state = new_state;
                if let Some((worktree_id, entry_id)) = this.update_visible_entries_task.selected_entry.take() {
                    this.selection = Some(SelectedEntry { worktree_id, entry_id });
                }

                if this.update_visible_entries_task.focus_filename_editor {
                    this.update_visible_entries_task.focus_filename_editor = false;
                    this.filename_editor.update(cx, |editor, cx| {
                        window.focus(&editor.focus_handle(cx), cx);
                    });
                }
                if this.update_visible_entries_task.autoscroll {
                    this.update_visible_entries_task.autoscroll = false;
                    this.autoscroll(cx);
                }
                this.preload_initial_entry_icons(window, cx);
                cx.notify();
            })
            .ok();
        });

        self.update_visible_entries_task = UpdateVisibleEntriesTask {
            _visible_entries_task: visible_entries_task,
            focus_filename_editor: focus_filename_editor || self.update_visible_entries_task.focus_filename_editor,
            autoscroll: autoscroll || self.update_visible_entries_task.autoscroll,
            selected_entry: new_selected_entry.or(self.update_visible_entries_task.selected_entry),
        };
    }

    pub(super) fn preload_initial_entry_icons(&self, window: &Window, cx: &mut Context<Self>) {
        if self.state.edit_state.is_some() {
            return;
        }
        {
            let scroll = self.scroll_handle.0.borrow();
            if scroll.last_item_size.is_some_and(|size| size.contents.height > Pixels::ZERO)
                || scroll.base_handle.offset().y != Pixels::ZERO
                || scroll.deferred_scroll_to_item.is_some()
            {
                return;
            }
        }

        let Some(workspace) =
            Workspace::for_window(window, cx).filter(|workspace| workspace.entity_id() == self.workspace.entity_id())
        else {
            return;
        };
        let workspace = workspace.read(cx);
        let panel_id = cx.entity_id();
        if workspace.zoomed_item().and_then(gpui::AnyWeakView::upgrade).is_some_and(|view| view.entity_id() != panel_id)
            || !workspace
                .all_docks()
                .into_iter()
                .any(|dock| dock.read(cx).visible_panel().is_some_and(|panel| panel.panel_id() == panel_id))
        {
            return;
        }

        let scale_factor = window.scale_factor();
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return;
        }
        let font_size = font::ui_font_size(cx).as_f32();
        let minimum_row_height = (1.5 * font_size * scale_factor).floor() / scale_factor;
        let row_count = (window.viewport_size().height.as_f32() / minimum_row_height).ceil();
        if minimum_row_height <= 0.0 || !row_count.is_finite() || row_count <= 0.0 {
            return;
        }
        let row_count = (row_count as usize).saturating_add(1);
        for (worktree_id, entry) in self
            .state
            .visible_entries
            .iter()
            .flat_map(|worktree| worktree.entries.iter().map(move |entry| (worktree.worktree_id, entry)))
            .take(row_count)
        {
            let is_expanded = self
                .state
                .expanded_dir_ids
                .get(&worktree_id)
                .is_some_and(|entries| entries.binary_search(&entry.id).is_ok());
            if let Some(icon) = Self::entry_icon(entry, is_expanded, cx) {
                Icon::from_path(icon).preload(cx);
            }
        }
    }

    pub(super) fn index_for_entry(
        &self,
        entry_id: ProjectEntryId,
        worktree_id: WorktreeId,
    ) -> Option<(usize, usize, usize)> {
        let mut total_ix = 0;
        for (worktree_ix, visible) in self.state.visible_entries.iter().enumerate() {
            if worktree_id != visible.worktree_id {
                total_ix += visible.entries.len();
                continue;
            }

            return visible
                .entries
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.id == entry_id)
                .map(|(ix, _)| (worktree_ix, ix, total_ix + ix));
        }
        None
    }

    pub(super) fn entry_at_index(&self, index: usize) -> Option<(WorktreeId, GitEntryRef<'_>)> {
        let mut offset = 0;
        for worktree in &self.state.visible_entries {
            let current_len = worktree.entries.len();
            if index < offset + current_len {
                return worktree.entries.get(index - offset).map(|entry| (worktree.worktree_id, entry.to_ref()));
            }
            offset += current_len;
        }
        None
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn iter_visible_entries(
        &self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<ProjectPanel>,
        callback: &mut dyn FnMut(&Entry, usize, &HashSet<Arc<RelPath>>, usize, &mut Window, &mut Context<ProjectPanel>),
    ) {
        let mut ix = 0;
        for visible in &self.state.visible_entries {
            if ix >= range.end {
                return;
            }

            if ix + visible.entries.len() <= range.start {
                ix += visible.entries.len();
                continue;
            }

            let end_ix = range.end.min(ix + visible.entries.len());
            let entry_range = range.start.saturating_sub(ix)..end_ix - ix;
            let entries = visible.index.get_or_init(|| visible.entries.iter().map(|e| Arc::clone(&e.path)).collect());
            let base_index = ix + entry_range.start;
            for (i, entry) in visible.entries[entry_range].iter().enumerate() {
                let global_index = base_index + i;
                callback(entry, global_index, entries, visible.depth_offset, window, cx);
            }
            ix = end_ix;
        }
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn for_each_visible_entry(
        &self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<ProjectPanel>,
        callback: &mut dyn FnMut(ProjectEntryId, EntryDetails, &mut Window, &mut Context<ProjectPanel>),
    ) {
        let mut ix = 0;
        for visible in &self.state.visible_entries {
            if ix >= range.end {
                return;
            }

            if ix + visible.entries.len() <= range.start {
                ix += visible.entries.len();
                continue;
            }

            let end_ix = range.end.min(ix + visible.entries.len());
            if visible.worktree_id == LIBRARIES_WORKTREE_ID {
                callback(LIBRARIES_ENTRY_ID, self.libraries_header_details(), window, cx);
                ix = end_ix;
                continue;
            }
            if let Some(worktree) = self.project.read(cx).worktree_for_id(visible.worktree_id, cx) {
                let snapshot = worktree.read(cx).snapshot();
                let root_name = snapshot.root_name();

                let entry_range = range.start.saturating_sub(ix)..end_ix - ix;
                let entries =
                    visible.index.get_or_init(|| visible.entries.iter().map(|e| Arc::clone(&e.path)).collect());
                for entry in &visible.entries[entry_range] {
                    let status = entry.git_summary;

                    let mut details = self.details_for_entry(
                        entry,
                        visible.worktree_id,
                        root_name,
                        entries,
                        status,
                        None,
                        window,
                        cx,
                    );

                    if let Some(edit_state) = &self.state.edit_state {
                        let is_edited_entry = if edit_state.is_new_entry() {
                            entry.id == NEW_ENTRY_ID
                        } else {
                            entry.id == edit_state.entry_id
                                || self.state.ancestors.get(&entry.id).is_some_and(|auto_folded_dirs| {
                                    auto_folded_dirs.ancestors.contains(&edit_state.entry_id)
                                })
                        };

                        if is_edited_entry {
                            if let Some(processing_filename) = &edit_state.processing_filename {
                                details.is_processing = true;
                                if let Some(ancestors) =
                                    edit_state.leaf_entry_id.and_then(|entry| self.state.ancestors.get(&entry))
                                {
                                    let position = ancestors
                                        .ancestors
                                        .iter()
                                        .position(|entry_id| *entry_id == edit_state.entry_id)
                                        .expect("Edited sub-entry should be an ancestor of selected leaf entry")
                                        + 1;
                                    let all_components = ancestors.ancestors.len();

                                    let prefix_components = all_components - position;
                                    let suffix_components = position.checked_sub(1);
                                    let mut previous_components = Path::new(&details.filename).components();
                                    let mut new_path =
                                        previous_components.by_ref().take(prefix_components).collect::<PathBuf>();
                                    if let Some(last_component) = processing_filename.components().next_back() {
                                        new_path.push(last_component);
                                        previous_components.next();
                                    }

                                    if suffix_components.is_some() {
                                        new_path.push(previous_components);
                                    }
                                    if let Some(str) = new_path.to_str() {
                                        details.filename.clear();
                                        details.filename.push_str(str);
                                    }
                                } else {
                                    details.filename.clear();
                                    details.filename.push_str(processing_filename.as_unix_str());
                                }
                            } else {
                                if edit_state.is_new_entry() {
                                    details.filename.clear();
                                }
                                details.is_editing = true;
                            }
                        }
                    }

                    callback(entry.id, details, window, cx);
                }
            }
            ix = end_ix;
        }
    }

    fn find_entry_in_worktree(
        &self,
        worktree_id: WorktreeId,
        reverse_search: bool,
        only_visible_entries: bool,
        predicate: &dyn Fn(GitEntryRef, WorktreeId) -> bool,
        cx: &Context<Self>,
    ) -> Option<GitEntry> {
        if only_visible_entries {
            let entries = self
                .state
                .visible_entries
                .iter()
                .find_map(|visible| (worktree_id == visible.worktree_id).then_some(&visible.entries))?
                .clone();

            return utils::ReversibleIterable::new(entries.iter(), reverse_search)
                .find(|ele| predicate(ele.to_ref(), worktree_id))
                .cloned();
        }

        let repo_snapshots = self.project.read(cx).git_store().read(cx).repo_snapshots(cx);
        let worktree = self.project.read(cx).worktree_for_id(worktree_id, cx)?;
        worktree.read_with(cx, |tree, _| {
            utils::ReversibleIterable::new(
                GitTraversal::new(&repo_snapshots, tree.entries(true, 0_usize)),
                reverse_search,
            )
            .find_single_ended(|ele| predicate(*ele, worktree_id))
            .map(GitEntryRef::to_owned)
        })
    }

    pub(super) fn find_entry(
        &self,
        start: Option<&SelectedEntry>,
        reverse_search: bool,
        predicate: &dyn Fn(GitEntryRef, WorktreeId) -> bool,
        cx: &Context<Self>,
    ) -> Option<SelectedEntry> {
        let mut worktree_ids: Vec<_> = self.state.visible_entries.iter().map(|worktree| worktree.worktree_id).collect();
        let repo_snapshots = self.project.read(cx).git_store().read(cx).repo_snapshots(cx);

        let mut last_found: Option<SelectedEntry> = None;

        if let Some(start) = start {
            let worktree = self.project.read(cx).worktree_for_id(start.worktree_id, cx)?.read(cx);

            let search = {
                let entry = worktree.entry_for_id(start.entry_id)?;
                let root_entry = worktree.root_entry()?;
                let tree_id = worktree.id();

                let mut first_iter = GitTraversal::new(
                    &repo_snapshots,
                    worktree.traverse_from_path(true, true, true, entry.path.as_ref()),
                );

                if reverse_search {
                    first_iter.next();
                }

                let first = first_iter
                    .enumerate()
                    .take_while_inclusive(|(count, entry)| entry.entry != root_entry || *count == 0_usize)
                    .map(|(_, entry)| entry)
                    .find(|ele| predicate(*ele, tree_id))
                    .map(GitEntryRef::to_owned);

                let second_iter = GitTraversal::new(&repo_snapshots, worktree.entries(true, 0_usize));

                let second = if reverse_search {
                    second_iter
                        .take_while_inclusive(|ele| ele.id != start.entry_id)
                        .filter(|ele| predicate(*ele, tree_id))
                        .last()
                        .map(GitEntryRef::to_owned)
                } else {
                    second_iter
                        .take_while(|ele| ele.id != start.entry_id)
                        .filter(|ele| predicate(*ele, tree_id))
                        .last()
                        .map(GitEntryRef::to_owned)
                };

                if reverse_search { Some((second, first)) } else { Some((first, second)) }
            };

            if let Some((first, second)) = search {
                let first = first.map(|entry| SelectedEntry { worktree_id: start.worktree_id, entry_id: entry.id });

                let second = second.map(|entry| SelectedEntry { worktree_id: start.worktree_id, entry_id: entry.id });

                if first.is_some() {
                    return first;
                }
                last_found = second;

                let idx =
                    worktree_ids.iter().enumerate().find(|(_, ele)| **ele == start.worktree_id).map(|(idx, _)| idx);

                if let Some(idx) = idx {
                    worktree_ids.rotate_left(idx + 1_usize);
                    worktree_ids.pop();
                }
            }
        }

        for tree_id in worktree_ids {
            if let Some(found) = self.find_entry_in_worktree(tree_id, reverse_search, false, &predicate, cx) {
                return Some(SelectedEntry { worktree_id: tree_id, entry_id: found.id });
            }
        }

        last_found
    }

    pub(super) fn find_visible_entry(
        &self,
        start: Option<&SelectedEntry>,
        reverse_search: bool,
        predicate: &dyn Fn(GitEntryRef, WorktreeId) -> bool,
        cx: &Context<Self>,
    ) -> Option<SelectedEntry> {
        let mut worktree_ids: Vec<_> = self.state.visible_entries.iter().map(|worktree| worktree.worktree_id).collect();

        let mut last_found: Option<SelectedEntry> = None;

        if let Some(start) = start {
            let entries = self
                .state
                .visible_entries
                .iter()
                .find(|worktree| worktree.worktree_id == start.worktree_id)
                .map(|worktree| &worktree.entries)?;

            let mut start_idx =
                entries.iter().enumerate().find(|(_, ele)| ele.id == start.entry_id).map(|(idx, _)| idx)?;

            if reverse_search {
                start_idx = start_idx.saturating_add(1_usize);
            }

            let (left, right) = entries.split_at_checked(start_idx)?;

            let (first_iter, second_iter) = if reverse_search {
                (
                    utils::ReversibleIterable::new(left.iter(), reverse_search),
                    utils::ReversibleIterable::new(right.iter(), reverse_search),
                )
            } else {
                (
                    utils::ReversibleIterable::new(right.iter(), reverse_search),
                    utils::ReversibleIterable::new(left.iter(), reverse_search),
                )
            };

            let first_search = first_iter.find(|ele| predicate(ele.to_ref(), start.worktree_id));
            let second_search = second_iter.find(|ele| predicate(ele.to_ref(), start.worktree_id));

            if first_search.is_some() {
                return first_search.map(|entry| SelectedEntry { worktree_id: start.worktree_id, entry_id: entry.id });
            }

            last_found =
                second_search.map(|entry| SelectedEntry { worktree_id: start.worktree_id, entry_id: entry.id });

            let idx = worktree_ids.iter().enumerate().find(|(_, ele)| **ele == start.worktree_id).map(|(idx, _)| idx);

            if let Some(idx) = idx {
                worktree_ids.rotate_left(idx + 1_usize);
                worktree_ids.pop();
            }
        }

        for tree_id in worktree_ids {
            if let Some(found) = self.find_entry_in_worktree(tree_id, reverse_search, true, &predicate, cx) {
                return Some(SelectedEntry { worktree_id: tree_id, entry_id: found.id });
            }
        }

        last_found
    }
}
