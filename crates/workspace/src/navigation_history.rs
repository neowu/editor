use std::collections::hash_map;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use gpui::App;
use gpui::Context;
use gpui::Entity;
use gpui::EntityId;
use gpui::Task;
use itertools::Itertools as _;
use project::PathChange;
use project::ProjectPath;
use project::UpdatedEntriesSet;
use project::Worktree;
use project::WorktreeId;
use project::WorktreeSettings;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use ui::Window;
use util::rel_path::RelPath;

use super::MAX_RECENT_SELECTIONS;
use super::NavHistory;
use super::NavigationEntry;
use super::NavigationMode;
use super::OpenVisible;
use super::Pane;
use super::TagNavigationMode;
use super::Workspace;
use super::WorkspaceMode;
use crate::item::ItemHandle;
use crate::open::OpenOptions;

impl Workspace {
    pub fn recently_activated_items(&self, cx: &App) -> FxHashMap<EntityId, usize> {
        let mut history: FxHashMap<EntityId, usize> = FxHashMap::default();

        for pane_handle in &self.panes {
            let pane = pane_handle.read(cx);

            for entry in pane.activation_history() {
                history
                    .insert(entry.entity_id, history.get(&entry.entity_id).copied().unwrap_or(0).max(entry.timestamp));
            }
        }

        history
    }

    pub fn recent_active_item_by_type<T: 'static>(&self, cx: &App) -> Option<Entity<T>> {
        let mut recent_item: Option<Entity<T>> = None;
        let mut recent_timestamp = 0;
        for pane_handle in &self.panes {
            let pane = pane_handle.read(cx);
            let item_map: FxHashMap<EntityId, &Box<dyn ItemHandle>> =
                pane.items().map(|item| (item.item_id(), item)).collect();
            for entry in pane.activation_history() {
                if entry.timestamp > recent_timestamp
                    && let Some(&item) = item_map.get(&entry.entity_id)
                    && let Some(typed_item) = item.act_as::<T>(cx)
                {
                    recent_timestamp = entry.timestamp;
                    recent_item = Some(typed_item);
                }
            }
        }
        recent_item
    }

    pub(crate) fn recent_navigation_history_iter(
        &self,
        cx: &App,
    ) -> impl Iterator<Item = (ProjectPath, Option<PathBuf>)> + use<> {
        let mut abs_paths_opened: FxHashMap<PathBuf, FxHashSet<ProjectPath>> = FxHashMap::default();
        let mut history: FxHashMap<ProjectPath, (Option<PathBuf>, usize)> = FxHashMap::default();

        for pane in &self.panes {
            let pane = pane.read(cx);

            pane.nav_history().for_each_entry(cx, &mut |entry, (project_path, fs_path)| {
                if let Some(fs_path) = &fs_path {
                    abs_paths_opened.entry(fs_path.clone()).or_default().insert(project_path.clone());
                }
                let timestamp = entry.timestamp;
                match history.entry(project_path) {
                    hash_map::Entry::Occupied(mut entry) => {
                        let (_, old_timestamp) = entry.get();
                        if &timestamp > old_timestamp {
                            entry.insert((fs_path, timestamp));
                        }
                    }
                    hash_map::Entry::Vacant(entry) => {
                        entry.insert((fs_path, timestamp));
                    }
                }
            });

            if let Some(item) = pane.active_item()
                && let Some(project_path) = item.project_path(cx)
            {
                let fs_path = self.project.read(cx).absolute_path(&project_path, cx);

                if let Some(fs_path) = &fs_path {
                    abs_paths_opened.entry(fs_path.clone()).or_default().insert(project_path.clone());
                }

                history.insert(project_path, (fs_path, usize::MAX));
            }
        }

        let mut recent_history = history
            .into_iter()
            .sorted_by_key(|(_, (_, order))| *order)
            .map(|(project_path, (fs_path, _))| (project_path, fs_path))
            .rev()
            .filter(move |(history_path, abs_path)| {
                let latest_project_path_opened =
                    abs_path.as_ref().and_then(|abs_path| abs_paths_opened.get(abs_path)).and_then(|project_paths| {
                        project_paths.iter().max_by(|b1, b2| b1.worktree_id.cmp(&b2.worktree_id))
                    });

                latest_project_path_opened.is_none_or(|path| path == history_path)
            })
            .collect::<Vec<_>>();

        let mut seen_paths =
            recent_history.iter().map(|(project_path, _)| project_path.clone()).collect::<FxHashSet<_>>();
        let project = self.project.read(cx);
        for abs_path in &self.persisted_recent_navigation_history {
            let Some(project_path) = project.project_path_for_absolute_path(abs_path, cx) else {
                continue;
            };
            if seen_paths.insert(project_path.clone()) {
                recent_history.push((project_path, Some(abs_path.clone())));
            }
        }

        recent_history.retain(|(path, _)| !self.is_deleted_history_path(path, cx));
        recent_history.into_iter()
    }

    pub fn recent_navigation_history(&self, limit: Option<usize>, cx: &App) -> Vec<(ProjectPath, Option<PathBuf>)> {
        self.recent_navigation_history_iter(cx).take(limit.unwrap_or(usize::MAX)).collect()
    }

    pub(crate) fn clear_navigation_history(&mut self, window: &Window, cx: &mut Context<Workspace>) {
        for pane in &self.panes {
            pane.update(cx, |pane, cx| pane.nav_history_mut().clear(cx));
        }
        self.persisted_recent_navigation_history.clear();
        self.last_active_project_path = None;
        self.serialize_workspace(window, cx);
    }

    pub(super) fn is_deleted_history_path(&self, path: &ProjectPath, cx: &App) -> bool {
        let project = self.project.read(cx);
        project
            .get_open_buffer(path, cx)
            .is_some_and(|buffer| buffer.read(cx).file().is_some_and(|file| file.disk_state().is_deleted()))
            && project
                .worktree_for_id(path.worktree_id, cx)
                .is_some_and(|worktree| Workspace::history_path_is_removed(worktree.read(cx), &path.path))
    }

    /// Whether `path` is gone from the worktree, as opposed to excluded or inside a directory that isn't loaded.
    fn history_path_is_removed(worktree: &Worktree, path: &RelPath) -> bool {
        let settings = WorktreeSettings::get();
        worktree.entry_for_path(path).is_none()
            && !settings.is_path_excluded(path)
            && !path
                .ancestors()
                .skip(1)
                .find_map(|ancestor| worktree.entry_for_path(ancestor))
                .is_some_and(|entry| entry.kind.is_unloaded())
    }

    pub(super) fn remove_deleted_navigation_history(
        &mut self,
        worktree_id: WorktreeId,
        changes: &UpdatedEntriesSet,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.read(cx);
        let Some(worktree) = project.worktree_for_id(worktree_id, cx) else {
            return;
        };
        let worktree = worktree.read(cx);
        let removed_paths = changes
            .iter()
            .filter(|(path, entry_id, change)| {
                *change == PathChange::Removed
                    && worktree.entry_for_id(*entry_id).is_none()
                    && Workspace::history_path_is_removed(worktree, path)
            })
            .map(|(path, _, _)| Arc::clone(path))
            .collect::<FxHashSet<_>>();
        if removed_paths.is_empty() {
            return;
        }

        let (paths, absolute_paths): (FxHashSet<_>, FxHashSet<_>) = self
            .recent_navigation_history_iter(cx)
            .chain(self.persisted_recent_navigation_history.iter().filter_map(|absolute_path| {
                let path = project.project_path_for_absolute_path(absolute_path, cx)?;
                Some((path, Some(absolute_path.clone())))
            }))
            .filter(|(path, _)| {
                path.worktree_id == worktree_id
                    && path.path.ancestors().any(|ancestor| removed_paths.contains(ancestor))
                    && worktree.entry_for_path(&path.path).is_none()
            })
            .filter_map(|(path, _)| {
                let absolute_path = project.absolute_path(&path, cx)?;
                Some((path, absolute_path))
            })
            .unzip();

        if paths.is_empty() {
            return;
        }
        self.persisted_recent_navigation_history.retain(|path| !absolute_paths.contains(path));
        for pane in &self.panes {
            pane.update(cx, |pane, cx| pane.remove_navigation_history_paths(&paths, cx));
        }
        if self.last_active_project_path.as_ref().is_some_and(|path| paths.contains(path)) {
            self.last_active_project_path = None;
        }
        cx.notify();
    }

    pub(super) fn rename_persisted_navigation_history_paths(&mut self, old_path: &Path, new_path: &Path) -> bool {
        let mut changed = false;
        for path in &mut self.persisted_recent_navigation_history {
            let Ok(suffix) = path.strip_prefix(old_path) else {
                continue;
            };
            let renamed_path = new_path.join(suffix);
            if *path != renamed_path {
                *path = renamed_path;
                changed = true;
            }
        }

        if changed {
            let mut seen_paths = FxHashSet::default();
            self.persisted_recent_navigation_history.retain(|path| seen_paths.insert(path.clone()));
        }
        changed
    }

    pub(super) fn remember_navigation_history_path(&mut self, project_path: &ProjectPath, cx: &App) -> bool {
        if self.restoring_workspace {
            return false;
        }
        let Some(absolute_path) = self.project.read(cx).absolute_path(project_path, cx) else {
            return false;
        };

        self.last_active_project_path = Some(project_path.clone());

        if self.persisted_recent_navigation_history.first() == Some(&absolute_path) {
            return false;
        }

        self.persisted_recent_navigation_history.retain(|path| path != &absolute_path);
        self.persisted_recent_navigation_history.insert(0, absolute_path);
        self.persisted_recent_navigation_history.truncate(MAX_RECENT_SELECTIONS);
        true
    }

    fn navigate_history(
        &mut self,
        mode: NavigationMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history_impl(mode, window, &mut |history, cx| history.pop(mode, cx), cx)
    }

    pub(super) fn navigate_tag_history(
        &mut self,
        mode: TagNavigationMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        self.navigate_history_impl(NavigationMode::Normal, window, &mut |history, _cx| history.pop_tag(mode), cx)
    }

    /// Walks the shared history to the next entry that moves the user, revealing
    /// its tab in whichever pane (and mode) holds it, or reopening a closed file in
    /// the last active center pane.
    fn navigate_history_impl(
        &mut self,
        mode: NavigationMode,
        window: &mut Window,
        cb: &mut dyn FnMut(&mut NavHistory, &mut App) -> Option<NavigationEntry>,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        let mut history = self.nav_history.clone();
        // Where the user is now goes on the opposite stack (back <-> forward). Only
        // this is recorded: activating the target must not record what its pane
        // showed before, since the user was never there.
        let current_item = self.last_content_pane.upgrade().and_then(|pane| pane.read(cx).active_item());
        if let Some(current_item) = &current_item {
            history.set_mode(mode);
            current_item.deactivated(window, cx);
            history.set_mode(NavigationMode::Normal);
        }

        let to_load = loop {
            let Some(entry) = cb(&mut history, cx) else {
                return Task::ready(Ok(()));
            };
            if let Some(item) = entry.item.upgrade()
                && let Some(pane) = self.pane_for(item.as_ref())
                && let Some(index) = pane.read(cx).index_for_item(item.as_ref())
            {
                history.disable();
                self.reveal_pane(&pane, window, cx);
                pane.update(cx, |pane, cx| pane.activate_item(index, true, true, window, cx));
                let mut navigated = current_item.as_ref().map(|item| item.item_id()) != Some(item.item_id());
                if let Some(data) = entry.data {
                    navigated |= item.navigate(data, window, cx);
                }
                history.enable();
                if navigated {
                    return Task::ready(Ok(()));
                }
            } else if let Some((project_path, abs_path)) = history.path_for_item(entry.item.id()) {
                break (project_path, abs_path, entry);
            }
        };

        // A closed file reopens in the center, where project files live.
        let (project_path, abs_path, entry) = to_load;
        let pane = self
            .last_active_center_pane
            .clone()
            .and_then(|pane| pane.upgrade())
            .unwrap_or_else(|| self.active_pane.clone());
        self.reveal_pane(&pane, window, cx);
        let pane = pane.downgrade();
        let open_by_project_path = self.load_path(project_path.clone(), window, cx);
        cx.spawn_in(window, async move |workspace, cx| {
            let open_by_project_path = open_by_project_path.await;
            let mut navigated = false;
            match open_by_project_path.with_context(|| format!("Navigating to {project_path:?}")) {
                Ok((project_entry_id, build_item)) => {
                    let prev_active_item_id = pane.update(cx, |pane, _| {
                        pane.nav_history_mut().disable();
                        pane.active_item().map(|p| p.item_id())
                    })?;

                    pane.update_in(cx, |pane, window, cx| {
                        let item = pane.open_item(
                            project_entry_id,
                            project_path,
                            true,
                            entry.is_preview,
                            true,
                            None,
                            window,
                            cx,
                            build_item,
                        );
                        navigated |= Some(item.item_id()) != prev_active_item_id;
                        if let Some(data) = entry.data {
                            navigated |= item.navigate(data, window, cx);
                        }
                        pane.nav_history_mut().enable();
                    })?;
                }
                Err(open_by_project_path_e) => {
                    // Fall back to opening by abs path, in case an external file was opened and closed,
                    // and its worktree is now dropped
                    if let Some(abs_path) = abs_path {
                        let prev_active_item_id = pane.update(cx, |pane, _| {
                            pane.nav_history_mut().disable();
                            pane.active_item().map(|p| p.item_id())
                        })?;
                        let open_by_abs_path = workspace.update_in(cx, |workspace, window, cx| {
                            workspace.open_abs_path(
                                abs_path.clone(),
                                OpenOptions { visible: Some(OpenVisible::None), ..Default::default() },
                                window,
                                cx,
                            )
                        })?;
                        let result = open_by_abs_path.await.with_context(|| format!("Navigating to {abs_path:?}"));
                        pane.update_in(cx, |pane, window, cx| {
                            if let Ok(item) = &result {
                                navigated |= Some(item.item_id()) != prev_active_item_id;
                                if let Some(data) = entry.data {
                                    navigated |= item.navigate(data, window, cx);
                                }
                            }
                            pane.nav_history_mut().enable();
                        })?;
                        if let Err(open_by_abs_path_e) = result {
                            log::error!(
                                "Failed to navigate history: {open_by_project_path_e:#} and {open_by_abs_path_e:#}"
                            );
                        }
                    }
                }
            }

            if !navigated {
                workspace.update_in(cx, |workspace, window, cx| workspace.navigate_history(mode, window, cx))?.await?;
            }

            Ok(())
        })
    }

    /// Shows `pane`'s group (switching mode if needed), makes it the active pane
    /// there and the user's current pane. Focus is the caller's to move.
    fn reveal_pane(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        self.last_content_pane = pane.downgrade();
        if self.git_history_center.root.contains_pane(pane) {
            if !pane.read(cx).is_zoomed() && self.has_zoomed(WorkspaceMode::GitHistory, cx) {
                self.zoom_git_history_pane(None, cx);
            }
            self.set_active_git_history_pane(pane, window, cx);
            self.switch_mode(WorkspaceMode::GitHistory, cx);
        } else if self.terminal_center.root.contains_pane(pane) {
            self.set_active_terminal_pane(pane, cx);
            self.switch_mode(WorkspaceMode::Terminal, cx);
        } else {
            self.switch_mode(WorkspaceMode::Editor, cx);
            if self.active_pane != *pane {
                self.set_active_pane(pane, window, cx);
            }
        }
    }

    /// Focus moved into the content pane `pane`. Coming from another content
    /// pane, records where the user was there, so going back returns to it.
    pub(super) fn content_pane_focused(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let previous = mem::replace(&mut self.last_content_pane, pane.downgrade());
        if previous.entity_id() != pane.entity_id()
            && let Some(item) = previous.upgrade().and_then(|previous| previous.read(cx).active_item())
        {
            item.deactivated(window, cx);
        }
    }

    pub(crate) fn can_go_back(&self) -> bool {
        self.nav_history.can_go_back()
    }

    pub(crate) fn can_go_forward(&self) -> bool {
        self.nav_history.can_go_forward()
    }

    pub(crate) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Workspace>) -> Task<Result<()>> {
        self.navigate_history(NavigationMode::GoingBack, window, cx)
    }

    pub(crate) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Workspace>) -> Task<Result<()>> {
        self.navigate_history(NavigationMode::GoingForward, window, cx)
    }

    pub(crate) fn reopen_closed_item(&mut self, window: &mut Window, cx: &mut Context<Workspace>) -> Task<Result<()>> {
        self.navigate_history(NavigationMode::ReopeningClosedItem, window, cx)
    }
}
