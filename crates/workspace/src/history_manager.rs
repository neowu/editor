use std::path::PathBuf;
use std::sync::Arc;

use fs::Fs;
use gpui::AppContext;
use gpui::Entity;
use gpui::Global;
use gpui::MenuItem;
use smallvec::SmallVec;
use ui::App;
use ui::Context;
use util::paths::PathExt;

use crate::NewWindow;
use crate::ProjectLocation;
use crate::ProjectsStore;
use crate::SerializedWorkspaceLocation;
use crate::path_list::PathList;
use crate::persistence::recent_project_workspaces;

pub fn init(fs: Arc<dyn Fs>, cx: &mut App) {
    let manager = cx.new(|_| HistoryManager::new());
    HistoryManager::set_global(manager.clone(), cx);
    HistoryManager::init(manager, fs, cx);
}

pub struct HistoryManager {
    /// The history of workspaces that have been opened in the past, in reverse order.
    /// The most recent workspace is at the end of the vector.
    history: Vec<HistoryManagerEntry>,
}

#[derive(Debug)]
pub struct HistoryManagerEntry {
    pub paths: PathList,
    pub path: SmallVec<[PathBuf; 2]>,
}

struct GlobalHistoryManager(Entity<HistoryManager>);

impl Global for GlobalHistoryManager {}

impl HistoryManager {
    const fn new() -> HistoryManager {
        HistoryManager { history: Vec::new() }
    }

    fn init(this: Entity<HistoryManager>, fs: Arc<dyn Fs>, cx: &App) {
        let recent = ProjectsStore::global(cx).recent();
        cx.spawn(async move |cx| {
            let recent_folders = recent_project_workspaces(recent, fs.as_ref())
                .await
                .into_iter()
                .rev()
                .filter(|workspace| matches!(workspace.location, SerializedWorkspaceLocation::Local))
                .map(|workspace| HistoryManagerEntry::new(&workspace.paths))
                .collect::<Vec<_>>();
            this.update(cx, |this, cx| {
                this.history = recent_folders;
                this.update_jump_list(cx);
            })
        })
        .detach();
    }

    pub fn global(cx: &App) -> Option<Entity<HistoryManager>> {
        cx.try_global::<GlobalHistoryManager>().map(|model| model.0.clone())
    }

    fn set_global(history_manager: Entity<HistoryManager>, cx: &mut App) {
        cx.set_global(GlobalHistoryManager(history_manager));
    }

    pub fn update_history(&mut self, entry: HistoryManagerEntry, cx: &mut Context<'_, HistoryManager>) {
        if self.history.last().is_some_and(|last| last.paths == entry.paths) {
            return;
        }
        self.history.retain(|existing| existing.paths != entry.paths);
        self.history.push(entry);
        self.update_jump_list(cx);
    }

    pub fn delete_history(&mut self, paths: &PathList, cx: &mut Context<'_, HistoryManager>) {
        let len = self.history.len();
        self.history.retain(|entry| &entry.paths != paths);
        if self.history.len() != len {
            self.update_jump_list(cx);
        }
    }

    fn update_jump_list(&self, cx: &Context<'_, HistoryManager>) {
        let menus = vec![MenuItem::action("New Window", NewWindow)];
        let entries = self.history.iter().rev().map(|entry| entry.path.clone()).collect::<Vec<_>>();
        let user_removed = cx.update_jump_list(menus, entries);
        cx.spawn(async move |this, cx| {
            let user_removed = user_removed.await;
            if user_removed.is_empty() {
                return;
            }
            let mut removed_locations = Vec::new();
            if let Ok(()) = this.update(cx, |this, _| {
                this.history.retain(|entry| {
                    let removed = user_removed.contains(&entry.path);
                    if removed {
                        removed_locations.push(ProjectLocation {
                            paths: entry.paths.ordered_paths().cloned().collect(),
                            remote: None,
                        });
                    }
                    !removed
                });
            }) {
                cx.update(|cx| {
                    ProjectsStore::global(cx).remove_recent(|location| removed_locations.contains(location), cx)
                })
                .await;
            }
        })
        .detach();
    }
}

impl HistoryManagerEntry {
    pub fn new(paths: &PathList) -> HistoryManagerEntry {
        let path = paths.ordered_paths().map(|path| path.compact()).collect::<SmallVec<[PathBuf; 2]>>();
        HistoryManagerEntry { paths: paths.clone(), path }
    }
}
