use std::mem;

use git::RepoPath;
use git::StageAll;
use git::UnstageAll;
use git::status::StageStatus;
use itertools::Itertools as _;
use project::git_store::Repository;
use project::git_store::RepositoryId;
use ui::prelude::App;
use ui::prelude::Context;
use ui::prelude::ToggleState;
use ui::prelude::Window;

use super::GitHeaderEntry;
use super::GitListEntry;
use super::GitPanel;
use super::GitPanelViewMode;
use super::GitStatusEntry;
use super::GitTreeDirEntry;
use super::GitTreeStatusEntry;
use super::Section;
use super::SelectionTargetKind;

/// What clicking a staging control should do.
///
/// In the "staged & unstaged" grouping, a partially staged file appears in both the
/// "Staged" and "Unstaged" sections at once, so a row's meaning comes from
/// the section it is rendered in rather than from the file's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StageIntent {
    Stage,
    Unstage,
    Toggle,
}

impl StageIntent {
    pub(super) const fn for_section(section: Section) -> Self {
        match section {
            Section::Staged => StageIntent::Unstage,
            Section::Unstaged => StageIntent::Stage,
            Section::Conflict | Section::Tracked | Section::New => StageIntent::Toggle,
        }
    }

    /// Resolves to a concrete direction (`true` = stage), consulting the
    /// current stage status only when no section dictates one.
    fn resolve_with(self, stage_status: impl FnOnce() -> StageStatus) -> bool {
        match self {
            StageIntent::Stage => true,
            StageIntent::Unstage => false,
            StageIntent::Toggle => match stage_status() {
                StageStatus::Staged => false,
                StageStatus::Unstaged | StageStatus::PartiallyStaged => true,
            },
        }
    }

    pub(super) fn checkbox_state(self, entry_state: impl FnOnce() -> ToggleState) -> ToggleState {
        match self {
            StageIntent::Stage => ToggleState::Unselected,
            StageIntent::Unstage => ToggleState::Selected,
            StageIntent::Toggle => entry_state(),
        }
    }

    pub(super) fn label(self, stage_status: impl FnOnce() -> StageStatus) -> &'static str {
        if self.resolve_with(stage_status) { "Stage" } else { "Unstage" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BulkStaging {
    pub(super) repo_id: RepositoryId,
    pub(super) anchor: RepoPath,
}

impl GitPanel {
    /// Returns whether activating the staging function for the current target
    /// should unstage its affected entries.
    pub(super) fn should_unstage(
        &self,
        target_kind: SelectionTargetKind,
        entries: &[GitStatusEntry],
        cx: &App,
    ) -> bool {
        if entries.is_empty() {
            return false;
        }

        let Some(repo) = self.active_repository.as_ref().map(|repo| repo.read(cx)) else {
            return false;
        };

        let Some(ix) = self.selected_entry else {
            return false;
        };

        let Some(entry) = self.entries.get(ix) else {
            return false;
        };

        let stage_status = match target_kind {
            SelectionTargetKind::Multiple => {
                let all_staged =
                    entries.iter().all(|entry| Self::stage_status_for_entry(entry, repo).is_fully_staged());

                if all_staged { StageStatus::Staged } else { StageStatus::Unstaged }
            }
            SelectionTargetKind::File => {
                let Some(entry) = entry.status_entry() else {
                    return false;
                };

                Self::stage_status_for_entry(entry, repo)
            }
            SelectionTargetKind::Directory => {
                let Some(entry) = entry.directory_entry() else {
                    return false;
                };

                self.stage_status_for_directory(entry, repo)
            }
        };

        let stage_intent = self.stage_intent_for_entry_index(ix);
        !stage_intent.resolve_with(|| stage_status)
    }

    const fn is_staged_tracked(entry: &GitStatusEntry) -> bool {
        !entry.status.is_created() && entry.staging.has_staged()
    }

    pub(super) fn contains_staged_tracked_entry<'a>(entries: impl IntoIterator<Item = &'a GitStatusEntry>) -> bool {
        entries.into_iter().any(Self::is_staged_tracked)
    }

    pub(super) fn staged_tracked_entries<'a>(
        entries: impl IntoIterator<Item = &'a GitStatusEntry>,
    ) -> Vec<GitStatusEntry> {
        entries.into_iter().filter(|entry| Self::is_staged_tracked(entry)).cloned().collect()
    }

    pub(super) fn change_all_files_stage(&self, stage: bool, cx: &Context<Self>) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };
        cx.spawn({
            async move |this, cx| {
                let result = this
                    .update(cx, |_this, cx| {
                        active_repository.update(
                            cx,
                            |repo, cx| {
                                if stage { repo.stage_all(cx) } else { repo.unstage_all(cx) }
                            },
                        )
                    })?
                    .await;

                this.update(cx, |this, cx| {
                    if let Err(err) = result {
                        this.show_error_toast(if stage { "add" } else { "reset" }, err, cx);
                    }
                    this.update_counts(active_repository.read(cx));
                    cx.notify();
                })
            }
        })
        .detach();
    }

    pub(super) fn stage_status_for_entry(entry: &GitStatusEntry, repo: &Repository) -> StageStatus {
        // Checking for current staged/unstaged file status is a chained operation:
        // 1. first, we check for any pending operation recorded in repository
        // 2. if there are no pending ops either running or finished, we then ask the repository
        //    for the most up-to-date file status read from disk - we do this since `entry` arg to this function `render_entry`
        //    is likely to be staled, and may lead to weird artifacts in the form of subsecond auto-uncheck/check on
        //    the checkbox's state (or flickering) which is undesirable.
        // 3. finally, if there is no info about this `entry` in the repo, we fall back to whatever status is encoded
        //    in `entry` arg.
        repo.pending_ops_for_path(&entry.repo_path)
            .and_then(|ops| {
                // In case the last operation in the list of pending operations
                // failed, we can't assume the stage status for this entry and
                // need to fallback to the actual state in the repo.
                if ops.last_op_errored() {
                    return None;
                }

                if ops.staging() || ops.staged() { Some(StageStatus::Staged) } else { Some(StageStatus::Unstaged) }
            })
            .or_else(|| repo.status_for_path(&entry.repo_path).map(|status| status.status.staging()))
            .unwrap_or(entry.staging)
    }

    pub(super) fn stage_status_for_directory(&self, entry: &GitTreeDirEntry, repo: &Repository) -> StageStatus {
        let GitPanelViewMode::Tree(tree_state) = &self.view_mode else {
            log::error!("We should never render a directory entry while in flat view mode");
            return StageStatus::Unstaged;
        };

        let Some(descendants) = tree_state.directory_descendants.get(&entry.key) else {
            return StageStatus::Unstaged;
        };

        let show_placeholders = self.show_placeholders && !self.has_staged_changes();
        let mut fully_staged_count = 0_usize;
        let mut any_staged_or_partially_staged = false;

        for descendant in descendants {
            if show_placeholders && !descendant.status.is_created() {
                fully_staged_count += 1;
                any_staged_or_partially_staged = true;
            } else {
                match GitPanel::stage_status_for_entry(descendant, repo) {
                    StageStatus::Staged => {
                        fully_staged_count += 1;
                        any_staged_or_partially_staged = true;
                    }
                    StageStatus::PartiallyStaged => {
                        any_staged_or_partially_staged = true;
                    }
                    StageStatus::Unstaged => {}
                }
            }
        }

        if descendants.is_empty() {
            StageStatus::Unstaged
        } else if fully_staged_count == descendants.len() {
            StageStatus::Staged
        } else if any_staged_or_partially_staged {
            StageStatus::PartiallyStaged
        } else {
            StageStatus::Unstaged
        }
    }

    pub(crate) fn stage_all(&mut self, _: &StageAll, _window: &mut Window, cx: &mut Context<Self>) {
        self.change_all_files_stage(true, cx);
    }

    pub(crate) fn unstage_all(&mut self, _: &UnstageAll, _window: &mut Window, cx: &mut Context<Self>) {
        self.change_all_files_stage(false, cx);
    }

    pub(super) fn toggle_staged_for_entry(
        &mut self,
        entry: &GitListEntry,
        intent: StageIntent,
        _window: &mut Window,
        cx: &Context<Self>,
    ) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };
        let mut set_anchor: Option<RepoPath> = None;
        let mut clear_anchor = None;

        let (stage, repo_paths) = {
            let repo = active_repository.read(cx);
            match entry {
                GitListEntry::Status(status_entry)
                | GitListEntry::TreeStatus(GitTreeStatusEntry { entry: status_entry, .. }) => {
                    let repo_paths = vec![status_entry.clone()];
                    let stage = intent.resolve_with(|| GitPanel::stage_status_for_entry(status_entry, repo));

                    if stage {
                        set_anchor = Some(status_entry.repo_path.clone());
                    } else if let Some(op) = self.bulk_staging.clone()
                        && op.anchor == status_entry.repo_path
                    {
                        clear_anchor = Some(op.anchor);
                    }
                    (stage, repo_paths)
                }
                GitListEntry::Header(section) => {
                    let goal_staged_state = match intent {
                        StageIntent::Stage => true,
                        StageIntent::Unstage => false,
                        StageIntent::Toggle => !self.header_state(section.header).selected(),
                    };
                    let entries = self
                        .change_entries_by_path()
                        .filter(|status_entry| {
                            section.contains(status_entry, repo)
                                && GitPanel::stage_status_for_entry(status_entry, repo).as_bool()
                                    != Some(goal_staged_state)
                        })
                        .cloned()
                        .collect::<Vec<_>>();

                    (goal_staged_state, entries)
                }
                GitListEntry::Directory(entry) => {
                    let goal_stage = intent.resolve_with(|| self.stage_status_for_directory(entry, repo));
                    let goal_staged_state = if goal_stage { StageStatus::Staged } else { StageStatus::Unstaged };

                    let entries = self
                        .view_mode
                        .tree_state()
                        .and_then(|state| state.directory_descendants.get(&entry.key))
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|status_entry| {
                            GitPanel::stage_status_for_entry(status_entry, repo) != goal_staged_state
                        })
                        .collect::<Vec<_>>();
                    (goal_stage, entries)
                }
                GitListEntry::EmptySection(_) => return,
            }
        };
        if let Some(anchor) = clear_anchor
            && let Some(op) = self.bulk_staging.clone()
            && op.anchor == anchor
        {
            self.bulk_staging = None;
        }
        if let Some(anchor) = set_anchor {
            self.set_bulk_staging_anchor(anchor, cx);
        }

        self.change_file_stage(stage, repo_paths, cx);
    }

    pub(super) fn change_file_stage(&self, stage: bool, entries: Vec<GitStatusEntry>, cx: &Context<Self>) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };
        cx.spawn({
            async move |this, cx| {
                let result = this
                    .update(cx, |this, cx| {
                        let task = active_repository.update(cx, |repo, cx| {
                            let repo_paths = entries.iter().map(|entry| entry.repo_path.clone()).unique().collect();
                            if stage {
                                repo.stage_entries(repo_paths, cx)
                            } else {
                                repo.unstage_entries(repo_paths, cx)
                            }
                        });
                        this.update_counts(active_repository.read(cx));
                        cx.notify();
                        task
                    })?
                    .await;

                this.update(cx, |this, cx| {
                    if let Err(err) = result {
                        this.show_error_toast(if stage { "add" } else { "reset" }, err, cx);
                        this.update_counts(active_repository.read(cx));
                    }
                    cx.notify();
                })
            }
        })
        .detach();
    }

    pub(crate) const fn total_staged_count(&self) -> usize {
        self.tracked_staged_count + self.new_staged_count + self.conflicted_staged_count
    }

    pub(super) fn toggle_staged_for_selected(
        &mut self,
        _: &git::ToggleStaged,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_kind) = self.selection_target_kind() else {
            return;
        };

        if target_kind == SelectionTargetKind::Multiple {
            let entries = self.effective_status_entries();
            let should_unstage = self.should_unstage(target_kind, &entries, cx);
            return self.change_file_stage(!should_unstage, entries, cx);
        }

        let Some(selected_index) = self.selected_entry else {
            return;
        };

        let Some(selected_entry) = self.entries.get(selected_index).cloned() else {
            return;
        };

        if self.is_resolved_conflict(selected_index, cx) {
            return;
        }

        let intent = self.stage_intent_for_entry_index(selected_index);
        self.toggle_staged_for_entry(&selected_entry, intent, window, cx);
    }

    pub(super) fn stage_range(&mut self, _: &git::StageRange, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_entry else {
            return;
        };
        let stage = self.stage_intent_for_entry_index(index) != StageIntent::Unstage;
        self.stage_bulk(index, stage, cx);
    }

    pub(super) fn stage_selected(&mut self, _: &git::StageFile, _window: &mut Window, cx: &mut Context<Self>) {
        let to_stage = self
            .effective_status_entries()
            .into_iter()
            .filter(|entry| entry.staging != StageStatus::Staged)
            .collect::<Vec<_>>();
        if !to_stage.is_empty() {
            self.change_file_stage(true, to_stage, cx);
        }
    }

    pub(super) fn unstage_selected(&mut self, _: &git::UnstageFile, _window: &mut Window, cx: &mut Context<Self>) {
        let to_unstage = self
            .effective_status_entries()
            .into_iter()
            .filter(|entry| entry.staging != StageStatus::Unstaged)
            .collect::<Vec<_>>();
        if !to_unstage.is_empty() {
            self.change_file_stage(false, to_unstage, cx);
        }
    }

    pub(super) fn stage_section(&mut self, _: &git::StageSection, window: &mut Window, cx: &mut Context<Self>) {
        self.change_selected_section_stage(StageIntent::Stage, window, cx);
    }

    pub(super) fn unstage_section(&mut self, _: &git::UnstageSection, window: &mut Window, cx: &mut Context<Self>) {
        self.change_selected_section_stage(StageIntent::Unstage, window, cx);
    }

    fn change_selected_section_stage(&mut self, intent: StageIntent, window: &mut Window, cx: &Context<Self>) {
        let Some(selected_index) = self.selected_entry else {
            return;
        };
        let Some(section) = self.section_for_entry_index(selected_index) else {
            return;
        };
        self.toggle_staged_for_entry(&GitListEntry::Header(GitHeaderEntry { header: section }), intent, window, cx);
    }

    pub(super) fn primary_changes_action_stages(&self) -> bool {
        self.entry_count == 0 || self.has_unstaged_changes()
    }

    pub(crate) fn can_stage_all(&self) -> bool {
        self.has_unstaged_changes()
    }

    pub(crate) const fn can_unstage_all(&self) -> bool {
        self.has_staged_changes()
    }

    pub(super) fn stage_bulk(&mut self, mut index: usize, stage: bool, cx: &Context<'_, Self>) {
        let Some(op) = self.bulk_staging.as_ref() else {
            return;
        };
        let Some(mut anchor_index) = self.entry_by_path(&op.anchor) else {
            return;
        };
        // Only a staged anchor survives the next entries refresh, so there's no
        // point re-anchoring on the entry we're about to unstage.
        if stage
            && let Some(entry) = self.entries.get(index)
            && let Some(entry) = entry.status_entry()
        {
            self.set_bulk_staging_anchor(entry.repo_path.clone(), cx);
        }
        if index < anchor_index {
            mem::swap(&mut index, &mut anchor_index);
        }
        let Some(repo) = self.active_repository.clone() else {
            return;
        };
        let repo = repo.read(cx);
        // Conflicts only change staging via their own explicit controls; a
        // range sweep must neither mark them resolved nor un-resolve them.
        let entries = self
            .entries
            .get(anchor_index..=index)
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.status_entry().cloned())
            .filter(|entry| !repo.had_conflict_on_last_merge_head_change(&entry.repo_path))
            .collect::<Vec<_>>();
        self.change_file_stage(stage, entries, cx);
    }

    fn set_bulk_staging_anchor(&mut self, path: RepoPath, cx: &Context<'_, GitPanel>) {
        let Some(repo) = self.active_repository.as_ref() else {
            return;
        };
        self.bulk_staging = Some(BulkStaging { repo_id: repo.read(cx).id, anchor: path });
    }
}
