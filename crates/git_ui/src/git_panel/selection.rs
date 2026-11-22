use actions::menu;
use git::repository::RepoPath;
use gpui::Entity;
use gpui::ScrollStrategy;
use itertools::Itertools as _;
use project::ProjectPath;
use project::git_store::Repository;
use rustc_hash::FxHashSet;
use ui::prelude::Context;
use ui::prelude::Window;

use super::CollapseSelectedEntry;
use super::DiffTarget;
use super::ExpandSelectedEntry;
use super::FirstEntry;
use super::GitListEntry;
use super::GitPanel;
use super::GitPanelViewMode;
use super::GitStatusEntry;
use super::LastEntry;
use super::NextEntry;
use super::PreviousEntry;
use super::Section;
use super::SelectionTargetKind;
use super::ToggleFocus;
use super::TreeKey;
use crate::git_panel_view_options::GitPanelGroupBy;
use crate::git_panel_view_options::GitPanelViewOptions;
use crate::project_diff::ProjectDiff;
use crate::staged_diff::StagedDiff;
use crate::unstaged_diff::UnstagedDiff;

/// The identity a row contributes to the multi-selection: files select by
/// repo path, directories select as a unit covering their descendants.
enum RowMark {
    File(RepoPath),
    Directory(TreeKey),
}

/// A live shift-range gesture. Marks are recomputed as the base set plus
/// everything between the anchor and the cursor on every extension, so
/// reversing direction shrinks the range instead of growing it.
pub(super) struct MarkRangeGesture {
    anchor_ix: usize,
    base_files: FxHashSet<RepoPath>,
    base_directories: FxHashSet<TreeKey>,
}

/// Tracks, while walking `entries` in display order, whether the current row
/// sits inside a directory that is part of the multi-selection.
#[derive(Default)]
struct MarkedDirectoryCoverage {
    covering_depth: Option<usize>,
}

impl MarkedDirectoryCoverage {
    /// Feeds the next row in display order; returns whether that row is
    /// inside a marked directory.
    fn observe(&mut self, entry: &GitListEntry, marked_directories: &FxHashSet<TreeKey>) -> bool {
        let depth = match entry {
            GitListEntry::Header(_) => {
                self.covering_depth = None;
                return false;
            }
            GitListEntry::Directory(directory) => {
                if self.covers(directory.depth) {
                    return true;
                }
                self.covering_depth = marked_directories.contains(&directory.key).then_some(directory.depth);
                return false;
            }
            // Flat view has no directory rows.
            GitListEntry::Status(_) => return false,
            GitListEntry::TreeStatus(status) => status.depth,
            GitListEntry::EmptySection(_) => return false,
        };
        if self.covers(depth) {
            true
        } else {
            self.covering_depth = None;
            false
        }
    }

    fn covers(&self, depth: usize) -> bool {
        self.covering_depth.is_some_and(|covering_depth| depth > covering_depth)
    }
}

impl GitPanel {
    pub fn selected_file_history_target(&self) -> Option<(Entity<Repository>, RepoPath)> {
        let entry = self.get_selected_entry()?.status_entry()?;
        let repository = self.active_repository.clone()?;
        if entry.status.is_created() {
            return None;
        }
        Some((repository, entry.repo_path.clone()))
    }

    pub fn entry_by_path(&self, path: &RepoPath) -> Option<usize> {
        self.projected_entries_by_path.get(path)?.first().map(|entry| entry.index)
    }

    pub(super) fn entry_by_path_in_section(&self, path: &RepoPath, section: Section) -> Option<usize> {
        self.projected_entries_by_path.get(path)?.iter().find(|entry| entry.section == section).map(|entry| entry.index)
    }

    fn has_marks(&self) -> bool {
        !self.marked_entries.is_empty() || !self.marked_directories.is_empty()
    }

    pub(super) fn clear_marks(&mut self) {
        self.marked_entries.clear();
        self.marked_directories.clear();
        self.mark_range_gesture = None;
    }

    pub(super) fn clear_marks_and_select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.clear_marks();
        self.selected_entry = Some(ix);
        cx.notify();
    }

    fn row_mark(&self, ix: usize) -> Option<RowMark> {
        match self.entries.get(ix)? {
            GitListEntry::Status(entry) => Some(RowMark::File(entry.repo_path.clone())),
            GitListEntry::TreeStatus(entry) => Some(RowMark::File(entry.entry.repo_path.clone())),
            GitListEntry::Directory(directory) => Some(RowMark::Directory(directory.key.clone())),
            GitListEntry::Header(_) => None,
            GitListEntry::EmptySection(_) => None,
        }
    }

    fn mark_row(&mut self, ix: usize) {
        match self.row_mark(ix) {
            Some(RowMark::File(path)) => {
                self.marked_entries.insert(path);
            }
            Some(RowMark::Directory(key)) => {
                self.marked_directories.insert(key);
            }
            None => {}
        }
    }

    pub(super) fn toggle_mark(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.mark_range_gesture = None;
        if let Some(mark) = self.row_mark(ix) {
            if !self.has_marks()
                && let Some(previous) = self.selected_entry.filter(|&previous| previous != ix)
            {
                self.mark_row(previous);
            }
            match mark {
                RowMark::File(path) => {
                    if !self.marked_entries.remove(&path) {
                        self.marked_entries.insert(path);
                    }
                }
                RowMark::Directory(key) => {
                    if !self.marked_directories.remove(&key) {
                        self.marked_directories.insert(key);
                    }
                }
            }
        }
        self.selected_entry = Some(ix);
        cx.notify();
    }

    fn mark_range(&mut self, anchor_ix: usize, target_ix: usize) {
        let entry_indices: Vec<usize> = match &self.view_mode {
            GitPanelViewMode::Flat => {
                let (start, end) = if anchor_ix <= target_ix { (anchor_ix, target_ix) } else { (target_ix, anchor_ix) };
                (start..=end).collect()
            }
            GitPanelViewMode::Tree(state) => {
                let anchor_pos = state.logical_indices.iter().position(|&ix| ix == anchor_ix);
                let target_pos = state.logical_indices.iter().position(|&ix| ix == target_ix);
                match anchor_pos.zip(target_pos) {
                    Some((anchor_pos, target_pos)) => {
                        let (start, end) =
                            if anchor_pos <= target_pos { (anchor_pos, target_pos) } else { (target_pos, anchor_pos) };
                        state.logical_indices[start..=end].to_vec()
                    }
                    None => vec![target_ix],
                }
            }
        };
        for ix in entry_indices {
            self.mark_row(ix);
        }
    }

    /// Every file in the multi-selection, in display order: files marked
    /// directly plus the descendants of marked directories.
    fn marked_file_entries(&self) -> Vec<GitStatusEntry> {
        let mut coverage = MarkedDirectoryCoverage::default();
        let mut marked = Vec::new();
        let mut seen_paths = FxHashSet::default();
        for entry in &self.entries {
            let covered = coverage.observe(entry, &self.marked_directories);
            if let Some(status_entry) = entry.status_entry()
                && (covered || self.marked_entries.contains(&status_entry.repo_path))
                && seen_paths.insert(status_entry.repo_path.clone())
            {
                marked.push(status_entry.clone());
            }
        }
        marked
    }

    pub(super) fn selection_target_kind(&self) -> Option<SelectionTargetKind> {
        if self.use_selected_entry() {
            return match self.selected_entry.and_then(|ix| self.entries.get(ix)) {
                Some(GitListEntry::Status(_) | GitListEntry::TreeStatus(_)) => Some(SelectionTargetKind::File),
                Some(GitListEntry::Directory(_)) => Some(SelectionTargetKind::Directory),
                _ => None,
            };
        }

        match (self.marked_entries.len(), self.marked_directories.len()) {
            (0, 0) => None,
            (0, 1) => Some(SelectionTargetKind::Directory),
            (1, 0) => Some(SelectionTargetKind::File),
            _ => Some(SelectionTargetKind::Multiple),
        }
    }

    /// Determines whether file operations should use the currently selected
    /// entry or the marked entries.
    fn use_selected_entry(&self) -> bool {
        let marked_count = self.marked_entries.len() + self.marked_directories.len();
        let selected_is_marked = self.selected_entry.is_some_and(|index| self.row_is_marked(index));

        match marked_count {
            0 => true,
            1 => !selected_is_marked,
            _ => false,
        }
    }

    /// Checks whether the entry at `index` is explicitly marked.
    pub(super) fn row_is_marked(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| match entry {
            GitListEntry::Status(entry) => self.marked_entries.contains(&entry.repo_path),
            GitListEntry::TreeStatus(entry) => self.marked_entries.contains(&entry.entry.repo_path),
            GitListEntry::Directory(entry) => self.marked_directories.contains(&entry.key),
            GitListEntry::Header(_) | GitListEntry::EmptySection(_) => false,
        })
    }

    /// The entries a file operation should act on: the marked set when a
    /// multi-selection exists, the selected entry otherwise. A single mark on
    /// another row does not override the selection, matching the project panel.
    pub(super) fn effective_status_entries(&self) -> Vec<GitStatusEntry> {
        let entry = self.selected_entry.and_then(|ix| self.entries.get(ix));

        if self.use_selected_entry() {
            if let Some(status_entry) = entry.and_then(|entry| entry.status_entry()) {
                return vec![status_entry.clone()];
            }

            // If using the selected entry and it is a directory, we'll want to
            // return the directory's descendants instead.
            if entry.and_then(GitListEntry::directory_entry).is_some() {
                return self
                    .selected_entry
                    .and_then(|ix| self.directory_descendants(ix))
                    .map_or_default(|entries| entries.to_vec());
            }
        }

        // Using `unique_by` allows us to deduplicate entries by `RepoPath`
        // ensuring that if a file is partially staged, meaning it shows up on
        // both "Staged" and "Unstaged" sections, we still report it as a single
        // entry and not two different ones.
        self.marked_file_entries().into_iter().unique_by(|entry| entry.repo_path.clone()).collect()
    }

    pub(super) fn effective_repo_paths(&self) -> Vec<RepoPath> {
        let selected_index = self.selected_entry;
        let selected_path = self.get_selected_entry().and_then(GitListEntry::repo_path);
        if let Some(path) = selected_path {
            let selected_is_marked =
                selected_index.and_then(|index| self.row_mark(index)).is_some_and(|mark| match mark {
                    RowMark::File(path) => self.marked_entries.contains(&path),
                    RowMark::Directory(key) => self.marked_directories.contains(&key),
                });
            let mark_count = self.marked_entries.len() + self.marked_directories.len();
            if mark_count == 0 || mark_count == 1 && !selected_is_marked {
                return vec![path.clone()];
            }
        }

        let mut paths = Vec::new();
        let mut seen_paths = FxHashSet::default();
        for (index, entry) in self.entries.iter().enumerate() {
            let marked = match self.row_mark(index) {
                Some(RowMark::File(path)) => self.marked_entries.contains(&path),
                Some(RowMark::Directory(key)) => self.marked_directories.contains(&key),
                None => false,
            };
            if marked
                && let Some(path) = entry.repo_path()
                && seen_paths.insert(path.clone())
            {
                paths.push(path.clone());
            }
        }
        paths
    }

    pub(super) fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_marks() {
            self.clear_marks();
            cx.notify();
        } else {
            window.dispatch_action(Box::new(ToggleFocus), cx);
        }
    }

    /// Extends or shrinks the live shift-range: marks become the gesture's
    /// base set plus everything between the gesture's anchor and `target_ix`.
    /// `origin_ix` anchors a new gesture when none is active.
    pub(super) fn apply_range_gesture(&mut self, origin_ix: usize, target_ix: usize) {
        if self.mark_range_gesture.is_none() {
            self.mark_range_gesture = Some(MarkRangeGesture {
                anchor_ix: origin_ix,
                base_files: self.marked_entries.clone(),
                base_directories: self.marked_directories.clone(),
            });
        }
        let Some(gesture) = &self.mark_range_gesture else {
            return;
        };
        let anchor_ix = gesture.anchor_ix;
        self.marked_entries = gesture.base_files.clone();
        self.marked_directories = gesture.base_directories.clone();
        self.mark_range(anchor_ix, target_ix);
    }

    pub fn select_entry_by_path(&mut self, path: ProjectPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git_repo) = self.active_repository.as_ref() else {
            return;
        };

        let (repo_path, default_section) = {
            let repo = git_repo.read(cx);
            let Some(repo_path) = repo.project_path_to_repo_path(&path, cx) else {
                return;
            };

            let section = repo.status_for_path(&repo_path).map(|status| status.status).map(|status| {
                if GitPanelViewOptions::get(cx).group_by == GitPanelGroupBy::Staging {
                    if repo.had_conflict_on_last_merge_head_change(&repo_path) {
                        Section::Conflict
                    } else if status.staging().has_staged() {
                        Section::Staged
                    } else {
                        Section::Unstaged
                    }
                } else if repo.had_conflict_on_last_merge_head_change(&repo_path) {
                    Section::Conflict
                } else if status.is_created() {
                    Section::New
                } else {
                    Section::Tracked
                }
            });

            (repo_path, section)
        };
        let selected_section = self.selected_entry.and_then(|index| {
            let selected_entry = self.entries.get(index)?.status_entry()?;
            if selected_entry.repo_path == repo_path { self.section_for_entry_index(index) } else { None }
        });
        let section = selected_section.or(default_section);

        let mut needs_rebuild = false;
        if let (Some(section), Some(tree_state)) = (section, self.view_mode.tree_state_mut()) {
            let mut current_dir = repo_path.parent();
            while let Some(dir) = current_dir {
                let key = TreeKey { section, path: RepoPath::from_rel_path(dir) };

                if tree_state.expanded_dirs.get(&key) == Some(&false) {
                    tree_state.expanded_dirs.insert(key, true);
                    needs_rebuild = true;
                }

                current_dir = dir.parent();
            }
        }

        if needs_rebuild {
            self.update_visible_entries(window, cx);
        }

        let Some(ix) = section
            .and_then(|section| self.entry_by_path_in_section(&repo_path, section))
            .or_else(|| self.entry_by_path(&repo_path))
        else {
            return;
        };

        self.selected_entry = Some(ix);
        self.scroll_to_selected_entry(cx);
    }

    fn scroll_to_selected_entry(&self, cx: &mut Context<Self>) {
        let Some(selected_entry) = self.selected_entry else {
            cx.notify();
            return;
        };

        let visible_index = match &self.view_mode {
            GitPanelViewMode::Flat => Some(selected_entry),
            GitPanelViewMode::Tree(state) => state.logical_indices.iter().position(|&ix| ix == selected_entry),
        };

        if let Some(visible_index) = visible_index {
            self.scroll_handle.scroll_to_item(visible_index, ScrollStrategy::Center);
        }

        cx.notify();
    }

    pub(super) fn expand_selected_entry(
        &mut self,
        _: &ExpandSelectedEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.get_selected_entry().cloned() else {
            return;
        };

        if let GitListEntry::Directory(dir_entry) = entry {
            if dir_entry.expanded {
                self.select_next(&menu::SelectNext, window, cx);
            } else {
                self.toggle_directory(&dir_entry.key, window, cx);
            }
        } else {
            self.select_next(&menu::SelectNext, window, cx);
        }
    }

    /// Finds the nearest expanded directory at or above the given path.
    fn nearest_expanded_directory_key(&self, section: Section, path: &RepoPath) -> Option<TreeKey> {
        let tree_state = self.view_mode.tree_state()?;
        let mut candidate_path = Some(path.clone());

        while let Some(path) = candidate_path {
            candidate_path = path.parent().map(RepoPath::from_rel_path);
            let key = TreeKey { section, path };

            if tree_state.expanded_dirs.get(&key).copied() == Some(true) {
                return Some(key);
            }
        }

        None
    }

    fn directory_entry_index(&self, key: &TreeKey) -> Option<usize> {
        self.entries.iter().position(|entry| entry.directory_entry().is_some_and(|directory| directory.key == *key))
    }

    /// Collapses the selected directory or its nearest expanded ancestor.
    ///
    /// If a file or collapsed directory is selected, walks up the tree to find,
    /// collapse, and select the nearest expanded directory.
    /// For other entry types, selects the previous entry.
    pub(super) fn collapse_selected_entry(
        &mut self,
        _: &CollapseSelectedEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_index) = self.selected_entry else {
            return;
        };

        let directory_key = match self.entries.get(selected_index) {
            Some(GitListEntry::Directory(directory)) => {
                self.nearest_expanded_directory_key(directory.key.section, &directory.key.path)
            }
            Some(GitListEntry::TreeStatus(status)) => self
                .section_for_entry_index(selected_index)
                .and_then(|section| self.nearest_expanded_directory_key(section, &status.entry.repo_path)),
            _ => None,
        };

        let Some(directory_key) = directory_key else {
            self.select_previous(&menu::SelectPrevious, window, cx);
            return;
        };

        self.toggle_directory(&directory_key, window, cx);

        if let Some(index) = self.directory_entry_index(&directory_key) {
            self.selected_entry = Some(index);
            self.scroll_to_selected_entry(cx);
        }
    }

    pub(super) fn select_first(&mut self, _: &menu::SelectFirst, _window: &mut Window, cx: &mut Context<Self>) {
        let is_selectable = |index| self.entries.get(index).is_some_and(GitListEntry::is_selectable);
        let first_entry = match &self.view_mode {
            GitPanelViewMode::Flat => self.visible_flat_entry_indices().into_iter().find(|&index| is_selectable(index)),
            GitPanelViewMode::Tree(state) => state.logical_indices.iter().copied().find(|&index| is_selectable(index)),
        };

        if let Some(first_entry) = first_entry {
            self.mark_range_gesture = None;
            self.selected_entry = Some(first_entry);
            self.scroll_to_selected_entry(cx);
        }
    }

    pub(super) fn select_previous(&mut self, _: &menu::SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        let item_count = self.entries.len();
        if item_count == 0 {
            return;
        }

        let Some(selected_entry) = self.selected_entry else {
            return;
        };

        let new_index = match &self.view_mode {
            GitPanelViewMode::Flat => selected_entry.saturating_sub(1),
            GitPanelViewMode::Tree(state) => {
                let Some(current_logical_index) = state.logical_indices.iter().position(|&i| i == selected_entry)
                else {
                    return;
                };

                state.logical_indices[current_logical_index.saturating_sub(1)]
            }
        };

        if selected_entry == 0 && new_index == 0 {
            return;
        }

        let candidate = match &self.view_mode {
            GitPanelViewMode::Flat => {
                let mut candidate = new_index;
                loop {
                    match self.entries.get(candidate) {
                        Some(entry) if entry.is_selectable() => break Some(candidate),
                        Some(_) => {
                            if candidate == 0 {
                                break None;
                            }
                            candidate -= 1;
                        }
                        None => break None,
                    }
                }
            }
            GitPanelViewMode::Tree(state) => {
                let mut position = state.logical_indices.iter().position(|&i| i == new_index);
                loop {
                    let Some(current) = position else { break None };
                    let Some(&index) = state.logical_indices.get(current) else {
                        break None;
                    };
                    match self.entries.get(index) {
                        Some(entry) if entry.is_selectable() => break Some(index),
                        _ => position = current.checked_sub(1),
                    }
                }
            }
        };

        if window.modifiers().shift {
            let target_ix = candidate.unwrap_or(selected_entry);
            self.apply_range_gesture(selected_entry, target_ix);
        } else {
            self.mark_range_gesture = None;
        }
        let Some(candidate) = candidate else {
            return;
        };

        self.selected_entry = Some(candidate);
        self.scroll_to_selected_entry(cx);
    }

    pub(super) fn select_next(&mut self, _: &menu::SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        let item_count = self.entries.len();
        if item_count == 0 {
            return;
        }

        let Some(selected_entry) = self.selected_entry else {
            return;
        };

        let new_index = match &self.view_mode {
            GitPanelViewMode::Flat => {
                if selected_entry >= item_count.saturating_sub(1) {
                    return;
                }

                selected_entry.saturating_add(1)
            }
            GitPanelViewMode::Tree(state) => {
                let Some(current_logical_index) = state.logical_indices.iter().position(|&i| i == selected_entry)
                else {
                    return;
                };

                let Some(new_index) = state.logical_indices.get(current_logical_index.saturating_add(1)).copied()
                else {
                    return;
                };

                new_index
            }
        };

        let candidate = match &self.view_mode {
            GitPanelViewMode::Flat => {
                let mut candidate = new_index;
                loop {
                    match self.entries.get(candidate) {
                        Some(entry) if entry.is_selectable() => break Some(candidate),
                        Some(_) => candidate += 1,
                        None => break None,
                    }
                }
            }
            GitPanelViewMode::Tree(state) => {
                let mut position = state.logical_indices.iter().position(|&i| i == new_index);
                loop {
                    let Some(current) = position else { break None };
                    let Some(&index) = state.logical_indices.get(current) else {
                        break None;
                    };
                    match self.entries.get(index) {
                        Some(entry) if entry.is_selectable() => break Some(index),
                        _ => position = Some(current + 1),
                    }
                }
            }
        };

        if window.modifiers().shift {
            let target_ix = candidate.unwrap_or(selected_entry);
            self.apply_range_gesture(selected_entry, target_ix);
        } else {
            self.mark_range_gesture = None;
        }
        let Some(candidate) = candidate else {
            return;
        };

        self.selected_entry = Some(candidate);
        self.scroll_to_selected_entry(cx);
    }

    pub(super) fn select_last(&mut self, _: &menu::SelectLast, _window: &mut Window, cx: &mut Context<Self>) {
        if self.entries.last().is_some() {
            self.mark_range_gesture = None;
            self.selected_entry = Some(self.entries.len() - 1);
        }

        let last_entry = match &self.view_mode {
            GitPanelViewMode::Flat => self.entries.iter().rposition(GitListEntry::is_selectable),
            GitPanelViewMode::Tree(state) => state
                .logical_indices
                .iter()
                .rev()
                .copied()
                .find(|&index| self.entries.get(index).is_some_and(GitListEntry::is_selectable)),
        };

        if let Some(last_entry) = last_entry {
            self.selected_entry = Some(last_entry);
            self.scroll_to_selected_entry(cx);
        }
    }

    /// Show diff view at selected entry, only if the diff view is open.
    fn move_diff_to_entry(&self, window: &mut Window, cx: &mut Context<Self>) {
        (|| {
            let workspace = self.workspace.upgrade()?;
            let selected_index = self.selected_entry?;
            let entry = self.entries.get(selected_index)?.status_entry()?.clone();
            let target = Self::diff_target_for_section(self.section_for_entry_index(selected_index));

            match target {
                DiffTarget::Staged => {
                    if let Some(staged_diff) = workspace.read(cx).item_of_type::<StagedDiff>(cx) {
                        staged_diff.update(cx, |staged_diff, cx| {
                            staged_diff.move_to_entry(entry, window, cx);
                        });
                    }
                }
                DiffTarget::Unstaged => {
                    if let Some(unstaged_diff) = workspace.read(cx).item_of_type::<UnstagedDiff>(cx) {
                        unstaged_diff.update(cx, |unstaged_diff, cx| {
                            unstaged_diff.move_to_entry(entry, window, cx);
                        });
                    }
                }
                DiffTarget::Uncommitted => {
                    if let Some(project_diff) = workspace.read(cx).item_of_type::<ProjectDiff>(cx) {
                        project_diff.update(cx, |project_diff, cx| {
                            project_diff.move_to_entry(entry, window, cx);
                        });
                    }
                }
            }

            Some(())
        })();
    }

    pub(super) fn first_entry(&mut self, _: &FirstEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.select_first(&menu::SelectFirst, window, cx);
        self.move_diff_to_entry(window, cx);
    }

    pub(super) fn last_entry(&mut self, _: &LastEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.select_last(&menu::SelectLast, window, cx);
        self.move_diff_to_entry(window, cx);
    }

    pub(super) fn next_entry(&mut self, _: &NextEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.select_next(&menu::SelectNext, window, cx);
        self.move_diff_to_entry(window, cx);
    }

    pub(super) fn previous_entry(&mut self, _: &PreviousEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.select_previous(&menu::SelectPrevious, window, cx);
        self.move_diff_to_entry(window, cx);
    }

    pub(super) fn select_first_entry_if_none(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let have_entries = self
            .active_repository
            .as_ref()
            .is_some_and(|active_repository| active_repository.read(cx).status_summary().count > 0);
        if have_entries && self.selected_entry.is_none() {
            self.select_first(&menu::SelectFirst, window, cx);
        }
    }

    pub(super) fn select_last_entry_if_out_of_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(idx) = self.selected_entry
            && idx >= self.entries.len()
        {
            self.select_last(&menu::SelectLast, window, cx);
        }
    }

    pub(super) fn get_selected_entry(&self) -> Option<&GitListEntry> {
        let i = self.selected_entry?;
        self.entries.get(i)
    }

    pub(super) fn change_entries_by_path(&self) -> impl Iterator<Item = &GitStatusEntry> {
        // A grouping can project one changed file into multiple list rows.
        self.entries.iter().filter_map(GitListEntry::status_entry).unique_by(|entry| entry.repo_path.clone())
    }

    pub(super) fn directory_descendants(&self, entry_index: usize) -> Option<&[GitStatusEntry]> {
        let GitListEntry::Directory(directory) = self.entries.get(entry_index)? else {
            return None;
        };

        self.view_mode.tree_state()?.directory_descendants.get(&directory.key).map(Vec::as_slice)
    }

    /// Returns the list of entries that are children of the directory where the
    /// context menu is deployed, if deployed.
    pub(super) fn directory_context_descendants(&self) -> Option<&[GitStatusEntry]> {
        let entry_index = self.context_menu.as_ref()?.target_entry_index?;
        self.directory_descendants(entry_index)
    }
}
