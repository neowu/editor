use std::future::Future;
use std::io;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use fs::Fs;
use fs::copy_recursive;
use futures::FutureExt;
use futures::future;
use futures::future::Shared;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::EntityId;
use gpui::EventEmitter;
use gpui::Global;
use gpui::Task;
use gpui::WeakEntity;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use util::path_list::PathList;
use util::paths::SanitizedPath;
use util::rel_path::RelPath;
use worktree::CreatedEntry;
use worktree::Entry;
use worktree::ProjectEntryId;
use worktree::UpdatedEntriesSet;
use worktree::UpdatedGitRepositoriesSet;
use worktree::Worktree;
use worktree::WorktreeId;

use crate::ProjectPath;
use crate::git_store;
use crate::library::library_root;

/// The current paths for a project's worktrees. Each folder path has a corresponding
/// main worktree path at the same position. The two lists are always the
/// same length and are modified together via `add_path` / `remove_main_path`.
///
/// For non-linked worktrees, the main path and folder path are identical.
/// For linked worktrees, the main path is the original repo and the folder
/// path is the linked worktree location.
#[derive(Default, Debug, Clone)]
pub struct WorktreePaths {
    paths: PathList,
    main_paths: PathList,
}

impl PartialEq for WorktreePaths {
    fn eq(&self, other: &Self) -> bool {
        self.ordered_pairs().eq(other.ordered_pairs())
    }
}

impl WorktreePaths {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Iterate the (main_worktree_path, folder_path) pairs in insertion order.
    pub fn ordered_pairs(&self) -> impl Iterator<Item = (&PathBuf, &PathBuf)> {
        self.main_paths.ordered_paths().zip(self.paths.ordered_paths())
    }
}

#[derive(Clone)]
pub struct WorktreeIdCounter(Arc<AtomicU64>);

impl Default for WorktreeIdCounter {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }
}

impl WorktreeIdCounter {
    pub fn get(cx: &mut App) -> Self {
        cx.default_global::<Self>().clone()
    }

    fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

impl Global for WorktreeIdCounter {}

/// Summarizes worktree ownership and current snapshot sizes.
#[derive(Debug, Default)]
pub struct WorktreeStoreDiagnostics {
    pub worktree_slots: usize,
    pub live_worktrees: usize,
    pub visible_worktrees: usize,
    pub strong_handles: usize,
    pub dead_weak_handles: usize,
    pub loading_worktrees: usize,
    pub total_entries: usize,
    pub visible_entries: usize,
    pub largest_worktree: Option<LargestWorktreeDiagnostics>,
}

/// Identifies the worktree with the largest current snapshot.
#[derive(Debug)]
pub struct LargestWorktreeDiagnostics {
    pub path: PathBuf,
    pub entries: usize,
    pub visible_entries: usize,
}

pub struct WorktreeStore {
    next_entry_id: Arc<AtomicUsize>,
    next_worktree_id: WorktreeIdCounter,
    retain_worktrees: bool,
    worktrees: Vec<WorktreeHandle>,
    /// Library worktrees the user opened a file from. They are held strongly
    /// and listed in the project panel; other library worktrees only live as
    /// long as something (e.g. an inlay hint's location) references them.
    shown_libraries: FxHashSet<WorktreeId>,
    scanning_enabled: bool,
    #[allow(clippy::type_complexity)]
    loading_worktrees: FxHashMap<Arc<SanitizedPath>, Shared<Task<Result<Entity<Worktree>, Arc<anyhow::Error>>>>>,
    initial_scan_complete: (watch::Sender<bool>, watch::Receiver<bool>),
    fs: Arc<dyn Fs>,
}

#[derive(Debug)]
pub enum WorktreeStoreEvent {
    WorktreeAdded(Entity<Worktree>),
    WorktreeRemoved(EntityId, WorktreeId),
    WorktreeReleased(EntityId, WorktreeId),
    WorktreeOrderChanged,
    WorktreeUpdatedEntries(WorktreeId, UpdatedEntriesSet),
    WorktreeUpdatedGitRepositories(WorktreeId, UpdatedGitRepositoriesSet),
    WorktreeDeletedEntry(WorktreeId, ProjectEntryId),
}

impl EventEmitter<WorktreeStoreEvent> for WorktreeStore {}

impl WorktreeStore {
    pub fn local(retain_worktrees: bool, fs: Arc<dyn Fs>, next_worktree_id: WorktreeIdCounter) -> Self {
        Self {
            next_entry_id: Arc::default(),
            next_worktree_id,
            loading_worktrees: FxHashMap::default(),
            worktrees: Vec::new(),
            shown_libraries: FxHashSet::default(),
            scanning_enabled: true,
            retain_worktrees,
            initial_scan_complete: watch::channel(true),
            fs,
        }
    }

    pub fn next_entry_id(&self) -> Arc<AtomicUsize> {
        self.next_entry_id.clone()
    }

    pub fn next_worktree_id(&self) -> WorktreeId {
        WorktreeId::from_usize(self.next_worktree_id.next() as usize)
    }

    /// Returns a future that resolves when all visible worktrees have completed
    /// their initial scan (entries populated, git repos detected).
    pub fn wait_for_initial_scan(&self) -> impl Future<Output = ()> + use<> {
        let mut rx = self.initial_scan_complete.1.clone();
        async move {
            while !*watch::Receiver::borrow(&mut rx) {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    }

    /// Returns whether all visible worktrees have completed their initial scan.
    pub fn initial_scan_completed(&self) -> bool {
        *watch::Receiver::borrow(&mut self.initial_scan_complete.1.clone())
    }

    /// Checks whether all visible worktrees have completed their initial scan
    /// and no worktree creations are pending, and updates the watch channel accordingly.
    fn update_initial_scan_state(&mut self, cx: &App) {
        let complete = self.loading_worktrees.is_empty()
            && self.visible_worktrees(cx).all(|wt| wt.read(cx).completed_scan_id() >= 1);
        self.initial_scan_complete.0.send(complete).ok();
    }

    /// Spawns a detached task that waits for a worktree's initial scan to complete,
    /// then rechecks and updates the aggregate initial scan state.
    fn observe_worktree_scan_completion(&self, worktree: &Entity<Worktree>, cx: &mut Context<Self>) {
        let await_scan = worktree.update(cx, |worktree, _cx| worktree.wait_for_snapshot(1));
        cx.spawn(async move |this, cx| {
            await_scan.await.ok();
            this.update(cx, |this, cx| {
                this.update_initial_scan_state(cx);
            })
            .ok();
            anyhow::Ok(())
        })
        .detach();
    }

    /// Returns worktree ownership and current snapshot size diagnostics.
    pub fn diagnostics(&self, cx: &App) -> WorktreeStoreDiagnostics {
        let mut diagnostics = WorktreeStoreDiagnostics {
            worktree_slots: self.worktrees.len(),
            loading_worktrees: self.loading_worktrees.len(),
            ..WorktreeStoreDiagnostics::default()
        };

        for handle in &self.worktrees {
            let worktree = match handle {
                WorktreeHandle::Strong(worktree) => {
                    diagnostics.strong_handles += 1;
                    Some(worktree.clone())
                }
                WorktreeHandle::Weak(worktree) => {
                    let worktree = worktree.upgrade();
                    if worktree.is_none() {
                        diagnostics.dead_weak_handles += 1;
                    }
                    worktree
                }
            };
            let Some(worktree) = worktree else {
                continue;
            };

            diagnostics.live_worktrees += 1;
            let worktree = worktree.read(cx);
            diagnostics.visible_worktrees += usize::from(worktree.is_visible());
            let snapshot = worktree.snapshot();
            let entries = snapshot.entry_count();
            let visible_entries = snapshot.visible_entry_count();
            diagnostics.total_entries += entries;
            diagnostics.visible_entries += visible_entries;

            let is_largest = diagnostics.largest_worktree.as_ref().is_none_or(|largest| entries > largest.entries);
            if is_largest {
                diagnostics.largest_worktree = Some(LargestWorktreeDiagnostics {
                    path: worktree.abs_path().to_path_buf(),
                    entries,
                    visible_entries,
                });
            }
        }

        diagnostics
    }

    /// Iterates through all worktrees, including ones that don't appear in the project panel
    pub fn worktrees(&self) -> impl '_ + DoubleEndedIterator<Item = Entity<Worktree>> {
        self.worktrees.iter().filter_map(move |worktree| worktree.upgrade())
    }

    /// Iterates through all user-visible worktrees, the ones that appear in the project panel.
    pub fn visible_worktrees<'a>(&'a self, cx: &'a App) -> impl 'a + DoubleEndedIterator<Item = Entity<Worktree>> {
        self.worktrees().filter(|worktree| worktree.read(cx).is_visible())
    }

    /// Iterates through all user-visible worktrees (directories and files that appear in the project panel) and other, invisible single files that could appear e.g. due to drag and drop.
    pub fn visible_worktrees_and_single_files<'a>(
        &'a self,
        cx: &'a App,
    ) -> impl 'a + DoubleEndedIterator<Item = Entity<Worktree>> {
        self.worktrees().filter(|worktree| worktree.read(cx).is_visible() || worktree.read(cx).is_single_file())
    }

    pub fn worktree_for_id(&self, id: WorktreeId, cx: &App) -> Option<Entity<Worktree>> {
        self.worktrees().find(|worktree| worktree.read(cx).id() == id)
    }

    pub fn worktree_for_entry(&self, entry_id: ProjectEntryId, cx: &App) -> Option<Entity<Worktree>> {
        self.worktrees().find(|worktree| worktree.read(cx).contains_entry(entry_id))
    }

    pub fn find_worktree(&self, abs_path: impl AsRef<Path>, cx: &App) -> Option<(Entity<Worktree>, Arc<RelPath>)> {
        let abs_path = SanitizedPath::new(abs_path.as_ref());
        for tree in self.worktrees() {
            if let Some(relative_path) = abs_path
                .as_ref()
                .strip_prefix(tree.read(cx).abs_path().as_ref())
                .ok()
                .and_then(|path| RelPath::new(path).ok())
            {
                return Some((tree.clone(), relative_path.into_arc()));
            }
        }
        None
    }

    pub fn project_path_for_absolute_path(&self, abs_path: &Path, cx: &App) -> Option<ProjectPath> {
        self.find_worktree(abs_path, cx)
            .map(|(worktree, relative_path)| ProjectPath { worktree_id: worktree.read(cx).id(), path: relative_path })
    }

    pub fn absolutize(&self, project_path: &ProjectPath, cx: &App) -> Option<PathBuf> {
        let worktree = self.worktree_for_id(project_path.worktree_id, cx)?;
        Some(worktree.read(cx).absolutize(&project_path.path))
    }

    pub fn find_or_create_worktree(
        &mut self,
        abs_path: impl AsRef<Path>,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Entity<Worktree>, Arc<RelPath>)>> {
        let abs_path = abs_path.as_ref();
        if let Some((tree, relative_path)) = self.find_worktree(abs_path, cx) {
            Task::ready(Ok((tree, relative_path)))
        } else {
            let abs_path = abs_path.to_path_buf();
            let worktree = self.create_worktree(&abs_path, visible, cx);
            cx.spawn(async move |_, cx| {
                let worktree = worktree.await?;
                // A library worktree is rooted at the library, not at `abs_path`.
                let relative_path = worktree.read_with(cx, |worktree, _| {
                    abs_path
                        .strip_prefix(worktree.abs_path().as_ref())
                        .ok()
                        .and_then(|path| RelPath::new(path).ok())
                        .map(|path| path.into_arc())
                        .context("path is outside of the created worktree")
                })?;
                Ok((worktree, relative_path))
            })
        }
    }

    pub fn entry_for_id<'a>(&'a self, entry_id: ProjectEntryId, cx: &'a App) -> Option<&'a Entry> {
        self.worktrees().find_map(|worktree| worktree.read(cx).entry_for_id(entry_id))
    }

    pub fn worktree_and_entry_for_id<'a>(
        &'a self,
        entry_id: ProjectEntryId,
        cx: &'a App,
    ) -> Option<(Entity<Worktree>, &'a Entry)> {
        self.worktrees().find_map(|worktree| worktree.read(cx).entry_for_id(entry_id).map(|e| (worktree.clone(), e)))
    }

    pub fn entry_for_path<'a>(&'a self, path: &ProjectPath, cx: &'a App) -> Option<&'a Entry> {
        self.worktree_for_id(path.worktree_id, cx)?.read(cx).entry_for_path(&path.path)
    }

    pub fn copy_entry(
        &mut self,
        entry_id: ProjectEntryId,
        new_project_path: ProjectPath,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<Entry>>> {
        let Some(old_worktree) = self.worktree_for_entry(entry_id, cx) else {
            return Task::ready(Err(anyhow!("no such worktree")));
        };
        let Some(old_entry) = old_worktree.read(cx).entry_for_id(entry_id) else {
            return Task::ready(Err(anyhow!("no such entry")));
        };
        let Some(new_worktree) = self.worktree_for_id(new_project_path.worktree_id, cx) else {
            return Task::ready(Err(anyhow!("no such worktree")));
        };

        {
            {
                let fs = &self.fs;
                let old_abs_path = old_worktree.read(cx).absolutize(&old_entry.path);
                let new_abs_path = new_worktree.read(cx).absolutize(&new_project_path.path);
                let fs = fs.clone();
                let copy = cx.background_spawn(async move {
                    copy_recursive(fs.as_ref(), &old_abs_path, &new_abs_path, fs::CopyOptions::default()).await
                });

                cx.spawn(async move |_, cx| {
                    copy.await?;
                    new_worktree.update(cx, |this, cx| this.refresh_entry(new_project_path.path, None, cx)).await
                })
            }
        }
    }

    pub fn rename_entry(
        &mut self,
        entry_id: ProjectEntryId,
        new_project_path: ProjectPath,
        cx: &mut Context<Self>,
    ) -> Task<Result<CreatedEntry>> {
        let Some(old_worktree) = self.worktree_for_entry(entry_id, cx) else {
            return Task::ready(Err(anyhow!("no such worktree")));
        };
        let Some(old_entry) = old_worktree.read(cx).entry_for_id(entry_id).cloned() else {
            return Task::ready(Err(anyhow!("no such entry")));
        };
        let Some(new_worktree) = self.worktree_for_id(new_project_path.worktree_id, cx) else {
            return Task::ready(Err(anyhow!("no such worktree")));
        };

        {
            {
                let fs = &self.fs;
                let abs_old_path = old_worktree.read(cx).absolutize(&old_entry.path);
                let new_worktree_ref = new_worktree.read(cx);
                let is_root_entry = new_worktree_ref.root_entry().is_some_and(|e| e.id == entry_id);
                let abs_new_path = if is_root_entry {
                    let abs_path = new_worktree_ref.abs_path();
                    let Some(root_parent_path) = abs_path.parent() else {
                        return Task::ready(Err(anyhow!("no parent for path {:?}", abs_path)));
                    };
                    root_parent_path.join(new_project_path.path.as_std_path())
                } else {
                    new_worktree_ref.absolutize(&new_project_path.path)
                };

                let fs = fs.clone();
                let case_sensitive = new_worktree.read(cx).fs_is_case_sensitive();

                let do_rename = async move |fs: &dyn Fs, old_path: &Path, new_path: &Path, overwrite| {
                    fs.rename(old_path, new_path, fs::RenameOptions { overwrite, ..fs::RenameOptions::default() })
                        .await
                        .with_context(|| format!("renaming {old_path:?} into {new_path:?}"))
                };

                let rename = cx.background_spawn({
                    let abs_new_path = abs_new_path.clone();
                    async move {
                        // If we're on a case-insensitive FS and we're doing a case-only rename (i.e. `foobar` to `FOOBAR`)
                        // we want to overwrite, because otherwise we run into a file-already-exists error.
                        let overwrite = !case_sensitive
                            && abs_old_path != abs_new_path
                            && abs_old_path.to_str().map(|p| p.to_lowercase())
                                == abs_new_path.to_str().map(|p| p.to_lowercase());

                        // The directory we're renaming into might not exist yet
                        if let Err(e) = do_rename(fs.as_ref(), &abs_old_path, &abs_new_path, overwrite).await {
                            if let Some(err) = e.downcast_ref::<io::Error>()
                                && err.kind() == ErrorKind::NotFound
                                && let Some(parent) = abs_new_path.parent()
                            {
                                fs.create_dir(parent)
                                    .await
                                    .with_context(|| format!("creating parent directory {parent:?}"))?;
                                return do_rename(fs.as_ref(), &abs_old_path, &abs_new_path, overwrite).await;
                            }
                            return Err(e);
                        }
                        Ok(())
                    }
                });

                cx.spawn(async move |_, cx| {
                    rename.await?;
                    Ok(new_worktree
                        .update(cx, |this, cx| {
                            if is_root_entry {
                                // We eagerly update `abs_path` and refresh this worktree.
                                // Otherwise, the FS watcher would do it on the `RootUpdated` event,
                                // but with a noticeable delay, so we handle it proactively.
                                this.update_abs_path_and_refresh(SanitizedPath::new_arc(&abs_new_path), cx);
                                Task::ready(Ok(this.root_entry().cloned()))
                            } else {
                                // First refresh the parent directory (in case it was newly created)
                                if let Some(parent) = new_project_path.path.parent() {
                                    drop(this.refresh_entries_for_paths(vec![parent.into()]));
                                }
                                // Then refresh the new path
                                this.refresh_entry(new_project_path.path.clone(), Some(old_entry.path), cx)
                            }
                        })
                        .await?
                        .map(CreatedEntry::Included)
                        .unwrap_or_else(|| CreatedEntry::Excluded { abs_path: abs_new_path }))
                })
            }
        }
    }

    pub fn create_worktree(
        &mut self,
        abs_path: impl AsRef<Path>,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Worktree>>> {
        // Anything inside a library joins its library worktree, even when opened
        // as a visible path (e.g. a file passed on the command line). Opening the
        // library's root folder itself still makes a normal project root.
        let library =
            library_root(abs_path.as_ref(), cx).filter(|library| !visible || library.path != abs_path.as_ref());
        let visible = visible && library.is_none();
        let abs_path: Arc<SanitizedPath> = match &library {
            Some(library) => SanitizedPath::new_arc(&library.path),
            None => SanitizedPath::new_arc(&abs_path),
        };
        if library.is_some()
            && let Some(worktree) =
                self.worktrees().find(|worktree| worktree.read(cx).abs_path().as_ref() == abs_path.as_path())
        {
            return Task::ready(Ok(worktree));
        }
        let library_name: Option<Arc<str>> = library.map(|library| library.name.into());
        if !self.loading_worktrees.contains_key(&abs_path) {
            let task = self.create_local_worktree(self.fs.clone(), abs_path.clone(), visible, library_name, cx);

            let loading_task = cx.spawn({
                let abs_path = abs_path.clone();
                async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.loading_worktrees.remove(&abs_path);
                        if !visible || !this.scanning_enabled || result.is_err() {
                            this.update_initial_scan_state(cx);
                        }
                    })
                    .ok();

                    if let Ok(worktree) = &result {
                        this.update(cx, |this, cx| {
                            if this.scanning_enabled && visible {
                                this.observe_worktree_scan_completion(worktree, cx);
                            }
                        })
                        .ok();
                    }

                    result
                }
            });

            self.loading_worktrees.insert(abs_path.clone(), loading_task.shared());

            if visible && self.scanning_enabled {
                self.initial_scan_complete.0.send(false).ok();
            }
        }
        let task = self.loading_worktrees.get(&abs_path).unwrap().clone();
        cx.background_spawn(async move { task.await.map_err(|error| anyhow!("{error:#}")) })
    }

    fn create_local_worktree(
        &self,
        fs: Arc<dyn Fs>,
        abs_path: Arc<SanitizedPath>,
        visible: bool,
        library_name: Option<Arc<str>>,
        cx: &Context<Self>,
    ) -> Task<Result<Entity<Worktree>, Arc<anyhow::Error>>> {
        let next_entry_id = self.next_entry_id.clone();
        let scanning_enabled = self.scanning_enabled;

        let worktree_id = self.next_worktree_id();

        cx.spawn(async move |this, cx| {
            let worktree = Worktree::local(
                SanitizedPath::cast_arc(abs_path.clone()),
                visible,
                fs,
                next_entry_id,
                scanning_enabled,
                worktree_id,
                cx,
            )
            .await?;
            if library_name.is_some() {
                worktree.update(cx, |worktree, _| worktree.set_library_name(library_name));
                // Files open right after this returns, and manifest lookups (e.g. the
                // crate's `Cargo.toml`) must not see a half-scanned library.
                worktree.read_with(cx, |worktree, _| worktree.scan_complete()).await;
            }

            this.update(cx, |this, cx| this.add(&worktree, cx))?;

            if visible {
                cx.update(|cx| {
                    cx.add_recent_document(abs_path.as_path());
                });
            }

            Ok(worktree)
        })
    }

    pub fn add(&mut self, worktree: &Entity<Worktree>, cx: &mut Context<Self>) {
        let push_strong_handle = self.retain_worktrees || worktree.read(cx).is_visible();
        let handle = if push_strong_handle {
            WorktreeHandle::Strong(worktree.clone())
        } else {
            WorktreeHandle::Weak(worktree.downgrade())
        };
        self.worktrees.push(handle);

        cx.emit(WorktreeStoreEvent::WorktreeAdded(worktree.clone()));

        let handle_id = worktree.entity_id();
        cx.subscribe(worktree, |_, worktree, event, cx| {
            let worktree_id = worktree.read(cx).id();
            match event {
                worktree::Event::UpdatedEntries(changes) => {
                    cx.emit(WorktreeStoreEvent::WorktreeUpdatedEntries(worktree_id, changes.clone()));
                }
                worktree::Event::UpdatedGitRepositories(set) => {
                    cx.emit(WorktreeStoreEvent::WorktreeUpdatedGitRepositories(worktree_id, set.clone()));
                }
                worktree::Event::DeletedEntry(id) => {
                    cx.emit(WorktreeStoreEvent::WorktreeDeletedEntry(worktree_id, *id))
                }
                worktree::Event::Deleted => {
                    // The worktree root itself has been deleted (for single-file worktrees)
                    // The worktree will be removed via the observe_release callback
                }
            }
        })
        .detach();
        cx.observe_release(worktree, move |_, worktree, cx| {
            cx.emit(WorktreeStoreEvent::WorktreeReleased(handle_id, worktree.id()));
            cx.emit(WorktreeStoreEvent::WorktreeRemoved(handle_id, worktree.id()));
        })
        .detach();
    }

    pub fn is_library_shown(&self, worktree_id: WorktreeId) -> bool {
        self.shown_libraries.contains(&worktree_id)
    }

    /// Lists the library worktree in the project panel and keeps it alive.
    pub fn show_library(&mut self, worktree_id: WorktreeId, cx: &mut Context<Self>) {
        if self.shown_libraries.contains(&worktree_id) {
            return;
        }
        let Some(worktree) = self.worktree_for_id(worktree_id, cx) else {
            return;
        };
        if worktree.read(cx).library_name().is_none() {
            return;
        }
        self.shown_libraries.insert(worktree_id);
        for handle in &mut self.worktrees {
            if handle.upgrade().is_some_and(|handle| handle == worktree) {
                *handle = WorktreeHandle::Strong(worktree.clone());
            }
        }
        cx.emit(WorktreeStoreEvent::WorktreeOrderChanged);
    }

    pub fn remove_worktree(&mut self, id_to_remove: WorktreeId, cx: &mut Context<Self>) {
        self.shown_libraries.remove(&id_to_remove);
        self.worktrees.retain(|worktree| {
            if let Some(worktree) = worktree.upgrade() {
                if worktree.read(cx).id() == id_to_remove {
                    cx.emit(WorktreeStoreEvent::WorktreeRemoved(worktree.entity_id(), id_to_remove));
                    false
                } else {
                    true
                }
            } else {
                false
            }
        });
        self.update_initial_scan_state(cx);
    }

    pub fn move_worktree(&mut self, source: WorktreeId, destination: WorktreeId, cx: &mut Context<Self>) -> Result<()> {
        if source == destination {
            return Ok(());
        }

        let mut source_index = None;
        let mut destination_index = None;
        for (i, worktree) in self.worktrees.iter().enumerate() {
            if let Some(worktree) = worktree.upgrade() {
                let worktree_id = worktree.read(cx).id();
                if worktree_id == source {
                    source_index = Some(i);
                    if destination_index.is_some() {
                        break;
                    }
                } else if worktree_id == destination {
                    destination_index = Some(i);
                    if source_index.is_some() {
                        break;
                    }
                }
            }
        }

        let source_index = source_index.with_context(|| format!("Missing worktree for id {source}"))?;
        let destination_index = destination_index.with_context(|| format!("Missing worktree for id {destination}"))?;

        if source_index == destination_index {
            return Ok(());
        }

        let worktree_to_move = self.worktrees.remove(source_index);
        self.worktrees.insert(destination_index, worktree_to_move);
        cx.emit(WorktreeStoreEvent::WorktreeOrderChanged);
        cx.notify();
        Ok(())
    }

    /// Rescans every worktree and reloads its git state, picking up changes the
    /// file watcher never reported.
    pub fn rescan_worktrees(&self, cx: &App) -> Task<Result<()>> {
        let scans = self.worktrees().map(|worktree| worktree.read(cx).rescan()).collect::<Vec<_>>();
        cx.background_spawn(async move {
            future::join_all(scans).await;
            Ok(())
        })
    }

    pub fn fs(&self) -> Arc<dyn Fs> {
        self.fs.clone()
    }

    pub fn paths(&self, cx: &App) -> WorktreePaths {
        let (mains, folders): (Vec<PathBuf>, Vec<PathBuf>) = self
            .visible_worktrees(cx)
            .map(|worktree| {
                let snapshot = worktree.read(cx).snapshot();
                let folder_path = snapshot.abs_path().to_path_buf();
                let main_path = snapshot
                    .root_repo_common_dir()
                    .filter(|dir| !git_store::is_submodule_git_dir(dir))
                    .map(|dir| git_store::repo_identity_path(dir))
                    .filter(|repo_path| {
                        snapshot.root_repo_is_linked_worktree()
                            || *repo_path == folder_path.as_path()
                            || !folder_path.starts_with(*repo_path)
                    })
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| folder_path.clone());
                (main_path, folder_path)
            })
            .unzip();

        WorktreePaths { paths: PathList::new(&folders), main_paths: PathList::new(&mains) }
    }
}

#[derive(Clone, Debug)]
enum WorktreeHandle {
    Strong(Entity<Worktree>),
    Weak(WeakEntity<Worktree>),
}

impl WorktreeHandle {
    fn upgrade(&self) -> Option<Entity<Worktree>> {
        match self {
            WorktreeHandle::Strong(handle) => Some(handle.clone()),
            WorktreeHandle::Weak(handle) => handle.upgrade(),
        }
    }
}
