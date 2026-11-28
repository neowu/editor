use std::ffi::OsStr;
use std::io;
use std::io::ErrorKind;
use std::ops::Deref;
use std::path::Component;

use git::repository;
mod buffer_git_state;
mod commit_graph;
mod conflict_set;
mod diff_base_load;
pub mod diff_buffer_list;
mod diffs;
pub mod git_traversal;
pub mod pending_op;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::rc::Weak;
use std::sync::Arc;
use std::sync::atomic;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use async_lock::Semaphore;
use buffer_diff::BufferDiff;
use buffer_git_state::BufferGitState;
use buffer_git_state::DiffBasesChange;
use commit_graph::CommitDataHandlerState;
pub use commit_graph::CommitDataState;
pub use commit_graph::GitGraphEvent;
pub use commit_graph::GraphDataResponse;
pub use commit_graph::InitialGitGraphData;
pub use conflict_set::ConflictRegion;
pub use conflict_set::ConflictSet;
pub use conflict_set::ConflictSetSnapshot;
pub use conflict_set::ConflictSetUpdate;
use diff_base_load::DiffBaseLoad;
use diff_base_load::DiffBaseLoadKind;
use diff_base_load::load_revision_groups;
use file_content::decode_text;
use fs::Fs;
use fs::RemoveOptions;
use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::channel::mpsc;
use futures::channel::oneshot;
use futures::channel::oneshot::Canceled;
use futures::future;
use futures::future::Shared;
use futures::stream::FuturesOrdered;
use git::Oid;
use git::blame::Blame;
use git::repository::Branch;
use git::repository::BranchesScanResult;
use git::repository::CommitDetails;
use git::repository::CommitFileStatus;
use git::repository::CommitOptions;
use git::repository::CreateWorktreeTarget;
use git::repository::DiffStatType;
use git::repository::DiffType;
use git::repository::FetchOptions;
use git::repository::GitCommitTemplate;
use git::repository::GitRepository;
use git::repository::LogOrder;
use git::repository::LogSource;
use git::repository::PushOptions;
use git::repository::RebaseAction;
use git::repository::Remote;
use git::repository::RemoteCommandOutput;
use git::repository::RepoPath;
use git::repository::ResetMode;
use git::repository::Worktree as GitWorktree;
use git::repository::delete_branch_flag;
use git::repository::is_binary_content;
use git::stash::GitStash;
use git::status;
use git::status::DiffStat;
use git::status::DiffTreeType;
use git::status::FileStatus;
use git::status::GitSummary;
use git::status::TreeDiff;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::SharedString;
use gpui::Subscription;
use gpui::Task;
use gpui::WeakEntity;
use language::Buffer;
use language::BufferEvent;
use language::LanguageRegistry;
use language::WorktreeId;
use pending_op::PendingOp;
use pending_op::PendingOpId;
use pending_op::PendingOps;
use pending_op::PendingOpsSummary;
use rustc_hash::FxHashMap;
use sum_tree::Edit;
use sum_tree::SumTree;
use sum_tree::TreeMap;
use task::Shell;
use text::Bias;
use text::BufferId;
use util::ResultExt as _;
use util::paths::SanitizedPath;
use util::post_inc;
use util::rel_path::RelPath;
use worktree::File;
use worktree::PathChange;
use worktree::PathKey;
use worktree::PathProgress;
use worktree::PathSummary;
use worktree::PathTarget;
use worktree::ProjectEntryId;
use worktree::UpdatedGitRepositoriesSet;
use worktree::UpdatedGitRepository;
use worktree::Worktree;
use worktree::WorktreeSettings;

use crate::Project;
use crate::ProjectEnvironment;
use crate::ProjectItem as _;
use crate::ProjectPath;
use crate::buffer_store::BufferStore;
use crate::buffer_store::BufferStoreEvent;
use crate::worktree_store::WorktreeStore;
use crate::worktree_store::WorktreeStoreEvent;

pub struct GitStore {
    state: GitStoreState,
    project: Option<WeakEntity<Project>>,
    buffer_store: Entity<BufferStore>,
    worktree_store: Entity<WorktreeStore>,
    repositories: FxHashMap<RepositoryId, Entity<Repository>>,
    parked_repositories: Vec<ParkedRepository>,
    worktree_ids: FxHashMap<RepositoryId, HashSet<WorktreeId>>,
    active_repo_id: Option<RepositoryId>,
    /// The folder the user is working in. `None` derives it from the active
    /// repository (see `active_worktree`).
    active_worktree_id: Option<WorktreeId>,
    #[allow(clippy::type_complexity)]
    loading_diffs: FxHashMap<(BufferId, DiffKind), Shared<Task<Result<Entity<BufferDiff>, Arc<anyhow::Error>>>>>,
    diffs: FxHashMap<BufferId, Entity<BufferGitState>>,
    buffer_ids_by_index_text_buffer_id: FxHashMap<BufferId, BufferId>,
    object_read_limiter: Arc<Semaphore>,
    _subscriptions: Vec<Subscription>,
}

const MIN_PARKED_REPOSITORY_DEPTH: usize = 2;

const MAX_CONCURRENT_OBJECT_READS: usize = 16;

/// Git matches every pathspec against every index entry, so the cost of a
/// scoped `git status`/`git diff` grows with the number of paths. On large
/// repositories a scan of the whole repository is cheaper than this many paths.
const MAX_SCOPED_STATUS_PATHS: usize = 64;

#[derive(Clone, Debug)]
struct ParkedRepository {
    worktree_id: WorktreeId,
    work_directory_abs_path: Arc<Path>,
    dot_git_abs_path: Arc<Path>,
    repository_dir_abs_path: Arc<Path>,
    common_dir_abs_path: Arc<Path>,
}

fn decode_git_text(bytes: Vec<u8>) -> Result<String> {
    let text = decode_text(bytes)?;
    anyhow::ensure!(!is_binary_content(text.as_bytes()), "Binary files are not supported");
    Ok(text)
}

#[derive(Debug)]
pub struct CommitDiff {
    pub files: Vec<CommitFile>,
    pub is_shallow_boundary: bool,
}

#[derive(Debug)]
pub struct CommitFile {
    pub path: RepoPath,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    pub is_binary: bool,
}

impl CommitFile {
    pub const fn status(&self) -> CommitFileStatus {
        match (&self.old_text, &self.new_text) {
            (None, Some(_)) => CommitFileStatus::Added,
            (Some(_), None) => CommitFileStatus::Deleted,
            _ => CommitFileStatus::Modified,
        }
    }
}

fn decode_commit_diff(diff: repository::CommitDiff) -> CommitDiff {
    let files = diff
        .files
        .into_iter()
        .map(|file| {
            let repository::CommitFile { path, old_content, new_content, mut is_binary } = file;

            if is_binary {
                return CommitFile {
                    path,
                    old_text: old_content.map(|_| String::new()),
                    new_text: new_content.map(|_| String::new()),
                    is_binary,
                };
            }

            let mut decode_content = |content: Option<Vec<u8>>| {
                content.map(|content| match decode_git_text(content) {
                    Ok(text) => text,
                    Err(error) => {
                        log::debug!(
                            "treating commit content for {} as binary after decoding failed: {error:#}",
                            path.as_unix_str()
                        );
                        is_binary = true;
                        String::new()
                    }
                })
            };
            let mut old_text = decode_content(old_content);
            let mut new_text = decode_content(new_content);
            if is_binary {
                old_text = old_text.map(|_| String::new());
                new_text = new_text.map(|_| String::new());
            }

            CommitFile { path, old_text, new_text, is_binary }
        })
        .collect();
    CommitDiff { files, is_shallow_boundary: diff.is_shallow_boundary }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DiffKind {
    Unstaged,
    Staged,
    Uncommitted,
    SinceOid(Option<Oid>),
}

#[derive(Debug, Clone, Copy)]
pub enum GitAccess {
    /// Either:
    /// - the user owns `.git`
    /// - the user doesn't own `.git`, but has both of:
    ///   - OS-level read permissions
    ///   - the directory is marked as safe (git config safe.directory)
    Yes,

    /// The user is not the owner of `.git`, and one of the following is true:
    /// - the directory is not marked as safe (git config safe.directory)
    /// - the user does not have OS-level read permissions to `.git`
    No,
}

struct GitStoreState {
    next_repository_id: Arc<AtomicU64>,
    project_environment: Entity<ProjectEnvironment>,
    fs: Arc<Fs>,
    _fs_watches: Box<[Task<()>]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub repo_path: RepoPath,
    pub status: FileStatus,
    pub diff_stat: Option<DiffStat>,
    pub staged_diff_stat: Option<DiffStat>,
    pub unstaged_diff_stat: Option<DiffStat>,
}

impl sum_tree::Item for StatusEntry {
    type Summary = PathSummary<GitSummary>;

    fn summary(&self, (): <Self::Summary as sum_tree::Summary>::Context<'_>) -> Self::Summary {
        PathSummary { max_path: Arc::clone(self.repo_path.as_ref()), item_summary: self.status.summary() }
    }
}

impl sum_tree::KeyedItem for StatusEntry {
    type Key = PathKey;

    fn key(&self) -> Self::Key {
        PathKey(Arc::clone(self.repo_path.as_ref()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepositoryId(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeDetails {
    merge_heads_by_conflicted_path: TreeMap<RepoPath, Vec<Option<SharedString>>>,
    pub message: Option<SharedString>,
    /// A rebase is stopped on conflicts, waiting to be continued or aborted.
    pub rebase_in_progress: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositorySnapshot {
    pub id: RepositoryId,
    pub statuses_by_path: SumTree<StatusEntry>,
    pub work_directory_abs_path: Arc<Path>,
    dot_git_abs_path: Arc<Path>,
    /// Absolute path to the directory holding this worktree's Git state.
    ///
    /// For a linked worktree this is the worktree-specific directory under the
    /// common Git directory, such as `<main>/.git/worktrees/<name>`.
    repository_dir_abs_path: Arc<Path>,
    /// Absolute path to the repository's common Git directory.
    ///
    /// For a normal checkout this is `<work_directory>/.git`. For a linked
    /// worktree this is the common Git directory shared by all worktrees. If
    /// that common directory is a bare repository, there may be no main
    /// worktree path to derive from it.
    pub common_dir_abs_path: Arc<Path>,
    pub branch: Option<Branch>,
    pub branch_list: Arc<[Branch]>,
    pub branch_list_error: Option<SharedString>,
    pub head_commit: Option<CommitDetails>,
    pub scan_id: u64,
    pub merge: MergeDetails,
    remote_origin_url: Option<String>,
    remote_upstream_url: Option<String>,
    pub stash_entries: GitStash,
    linked_worktrees: Arc<[GitWorktree]>,
}

type JobId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobInfo {
    pub start: Instant,
    pub message: SharedString,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum UnshallowState {
    #[default]
    Idle,
    InProgress,
    Unshallowed,
}

pub struct Repository {
    this: WeakEntity<Self>,
    snapshot: RepositorySnapshot,
    unshallow_state: UnshallowState,
    commit_message_buffer: Option<Entity<Buffer>>,
    git_store: WeakEntity<GitStore>,
    object_read_limiter: Arc<Semaphore>,
    // For a local repository, holds paths that have had worktree events since the last status scan completed,
    // and that should be examined during the next status scan.
    paths_needing_status_update: Vec<Vec<RepoPath>>,
    job_sender: mpsc::UnboundedSender<GitJob>,
    _worker_task: Task<()>,
    active_jobs: FxHashMap<JobId, JobInfo>,
    pending_ops: SumTree<PendingOps>,
    job_id: JobId,
    // The most recent diff base load job's loads, and `job_id` just after that job was queued.
    pending_diff_base_loads: (JobId, Weak<RefCell<Vec<DiffBaseLoad>>>),
    repository_state: Shared<Task<Result<RepositoryState, String>>>,
    initial_graph_data: FxHashMap<(LogSource, LogOrder), InitialGitGraphData>,
    commit_data_handler: CommitDataHandlerState,
    commit_data: FxHashMap<Oid, CommitDataState>,
}

impl Deref for Repository {
    type Target = RepositorySnapshot;

    fn deref(&self) -> &Self::Target {
        &self.snapshot
    }
}

#[derive(Clone)]
struct RepositoryState {
    fs: Arc<Fs>,
    backend: Arc<dyn GitRepository>,
    environment: Arc<FxHashMap<String, String>>,
}

impl RepositoryState {
    async fn new(
        work_directory_abs_path: Arc<Path>,
        dot_git_abs_path: Arc<Path>,
        project_environment: WeakEntity<ProjectEnvironment>,
        fs: Arc<Fs>,
        cx: &mut AsyncApp,
    ) -> Result<Self> {
        let environment = project_environment
            .update(cx, |project_environment, cx| {
                project_environment.local_directory_environment(
                    &Shell::System,
                    Arc::clone(&work_directory_abs_path),
                    cx,
                )
            })?
            .await
            .unwrap_or_else(|| {
                log::error!("failed to get working directory environment for repository {work_directory_abs_path:?}");
                FxHashMap::default()
            });
        let search_paths = environment.get("PATH").map(ToOwned::to_owned);
        let backend = cx
            .background_spawn({
                let fs = Arc::clone(&fs);
                async move {
                    let git_binary_path = search_paths
                        .and_then(|search_paths| {
                            which::which_in("git", Some(search_paths), &work_directory_abs_path).ok()
                        })
                        .or_else(|| which::which("git").ok());
                    fs.open_repo(&dot_git_abs_path, git_binary_path.as_deref())
                        .with_context(|| format!("opening repository at {dot_git_abs_path:?}"))
                }
            })
            .await?;
        Ok(RepositoryState { backend, environment: Arc::new(environment), fs })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositoryEvent {
    StatusesChanged,
    HeadChanged,
    BranchListChanged,
    TagsChanged,
    StashEntriesChanged,
    GitWorktreeListChanged,
    PendingOpsChanged { pending_ops: SumTree<PendingOps> },
    GraphEvent((LogSource, LogOrder), GitGraphEvent),
}

#[derive(Clone, Debug)]
struct JobsUpdated;

#[derive(Debug)]
pub enum GitStoreEvent {
    ActiveRepositoryChanged(Option<RepositoryId>),
    /// The active folder changed without necessarily changing the active
    /// repository (two folders of one repository).
    ActiveWorktreeChanged,
    /// Bool is true when the repository that's updated is the active repository.
    RepositoryUpdated(RepositoryId, RepositoryEvent, bool),
    RepositoryAdded,
    RepositoryRemoved(RepositoryId),
    IndexWriteError(anyhow::Error),
    JobsUpdated,
    ConflictsUpdated,
    GlobalConfigurationUpdated,
}

impl EventEmitter<RepositoryEvent> for Repository {}
impl EventEmitter<JobsUpdated> for Repository {}

impl EventEmitter<GitStoreEvent> for GitStore {}

struct GitJob {
    #[allow(clippy::type_complexity)]
    job: Box<dyn FnOnce(RepositoryState, &mut AsyncApp) -> Task<()>>,
    key: Option<GitJobKey>,
}

#[derive(PartialEq, Eq)]
enum GitJobKey {
    WriteIndex(Vec<RepoPath>),
    ReloadBufferDiffBases,
    RefreshStatuses,
    ReloadGitState,
}

impl GitStore {
    pub(crate) fn local(
        worktree_store: &Entity<WorktreeStore>,
        buffer_store: Entity<BufferStore>,
        environment: Entity<ProjectEnvironment>,
        fs: Arc<Fs>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _fs_watches = app_paths::git_global_config_files()
            .into_iter()
            .map(|path| {
                let fs = Arc::clone(&fs);

                cx.spawn(async move |this, cx| {
                    let watcher = fs.watch(&path, Duration::from_millis(100));
                    let (mut watcher, _) = watcher.await;
                    while watcher.next().await.is_some() {
                        let Ok(()) = this.update(cx, |this, cx| {
                            let GitStoreState { project_environment, fs, .. } = &this.state;
                            let project_environment = project_environment.downgrade();
                            let fs = Arc::clone(fs);
                            let repositories_to_respawn = this
                                .repositories
                                .iter()
                                .filter_map(|(repository_id, repo)| {
                                    repo.read(cx).job_sender.is_closed().then_some((*repository_id, repo.clone()))
                                })
                                .collect::<Vec<_>>();
                            for (_, repo) in repositories_to_respawn {
                                repo.update(cx, |repo, cx| {
                                    repo.respawn_local_worker(project_environment.clone(), Arc::clone(&fs), cx);
                                    repo.schedule_scan(cx);
                                });
                            }
                            cx.emit(GitStoreEvent::GlobalConfigurationUpdated);
                        }) else {
                            return;
                        };
                    }
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self::new(
            worktree_store.clone(),
            buffer_store,
            GitStoreState {
                next_repository_id: Arc::new(AtomicU64::new(1)),
                project_environment: environment,
                _fs_watches,
                fs,
            },
            cx,
        )
    }

    fn new(
        worktree_store: Entity<WorktreeStore>,
        buffer_store: Entity<BufferStore>,
        state: GitStoreState,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut _subscriptions = vec![
            cx.subscribe(&worktree_store, Self::on_worktree_store_event),
            cx.subscribe(&buffer_store, Self::on_buffer_store_event),
        ];

        GitStore {
            state,
            project: None,
            buffer_store,
            worktree_store,
            repositories: FxHashMap::default(),
            parked_repositories: Vec::new(),
            worktree_ids: FxHashMap::default(),
            active_repo_id: None,
            active_worktree_id: None,
            _subscriptions,
            loading_diffs: FxHashMap::default(),
            object_read_limiter: Arc::new(Semaphore::new(MAX_CONCURRENT_OBJECT_READS)),
            diffs: FxHashMap::default(),
            buffer_ids_by_index_text_buffer_id: FxHashMap::default(),
        }
    }

    pub(crate) fn set_project(&mut self, project: WeakEntity<Project>) {
        self.project = Some(project);
    }

    /// Sets the active folder and repository together, emitting an event for
    /// each that changed.
    fn set_active(&mut self, worktree_id: Option<WorktreeId>, repo_id: Option<RepositoryId>, cx: &mut Context<Self>) {
        if self.active_worktree_id != worktree_id {
            self.active_worktree_id = worktree_id;
            cx.emit(GitStoreEvent::ActiveWorktreeChanged);
        }
        if self.active_repo_id != repo_id {
            self.active_repo_id = repo_id;
            cx.emit(GitStoreEvent::ActiveRepositoryChanged(repo_id));
        }
    }

    /// Follows the focused file: its folder becomes active (when visible) along
    /// with the innermost repository containing it, or none.
    pub fn set_active_repo_for_path(&mut self, project_path: &ProjectPath, cx: &mut Context<Self>) {
        let repo_id = self.repository_and_path_for_project_path(project_path, cx).map(|(repo, _)| repo.read(cx).id);
        let is_visible = self
            .worktree_store
            .read(cx)
            .worktree_for_id(project_path.worktree_id, cx)
            .is_some_and(|worktree| worktree.read(cx).is_visible());
        if is_visible {
            self.set_active(Some(project_path.worktree_id), repo_id, cx);
        } else if let Some(repo_id) = repo_id {
            self.set_active(self.active_worktree_id, Some(repo_id), cx);
        }
    }

    /// Makes a folder active, with the innermost repository containing it (or
    /// none, for a folder outside any repository).
    pub fn set_active_worktree(&mut self, worktree_id: WorktreeId, cx: &mut Context<Self>) {
        let repo_id = self.repository_for_worktree(worktree_id, cx).map(|repo| repo.read(cx).id);
        self.set_active(Some(worktree_id), repo_id, cx);
    }

    /// Makes a repository active. The active folder is kept when it belongs to
    /// the repository (or contains it), else the first such folder is chosen.
    pub fn set_active_repository(&mut self, repo_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(repo) = self.repositories.get(&repo_id) else {
            return;
        };
        let repo_path = Arc::clone(&repo.read(cx).work_directory_abs_path);
        let related = |worktree: &Entity<Worktree>| paths_overlap(&worktree.read(cx).abs_path(), &repo_path);
        let worktree_store = self.worktree_store.read(cx);
        let keeps_folder = self
            .active_worktree_id
            .and_then(|id| worktree_store.worktree_for_id(id, cx))
            .is_some_and(|worktree| related(&worktree));
        let worktree_id = if keeps_folder {
            self.active_worktree_id
        } else {
            worktree_store
                .visible_worktrees(cx)
                .find(|worktree| related(worktree))
                .map(|worktree| worktree.read(cx).id())
        };
        self.set_active(worktree_id, Some(repo_id), cx);
    }

    /// The innermost repository containing the folder's root.
    pub fn repository_for_worktree(&self, worktree_id: WorktreeId, cx: &App) -> Option<Entity<Repository>> {
        let worktree = self.worktree_store.read(cx).worktree_for_id(worktree_id, cx)?;
        let worktree_path = worktree.read(cx).abs_path();
        let id = innermost_containing(
            &worktree_path,
            self.repositories.iter().map(|(id, repo)| (*id, Arc::clone(&repo.read(cx).work_directory_abs_path))),
        )?;
        self.repositories.get(&id).cloned()
    }

    /// Repositories inside the folder other than the one containing its root
    /// (submodules, vendored repositories), sorted by path.
    pub fn nested_repositories(&self, worktree_id: WorktreeId, cx: &App) -> Vec<Entity<Repository>> {
        let Some(worktree) = self.worktree_store.read(cx).worktree_for_id(worktree_id, cx) else {
            return Vec::new();
        };
        let worktree_path = worktree.read(cx).abs_path();
        let mut nested: Vec<_> = self
            .repositories
            .values()
            .filter(|repo| is_strictly_inside(&repo.read(cx).work_directory_abs_path, &worktree_path))
            .cloned()
            .collect();
        nested.sort_by_key(|repo| Arc::clone(&repo.read(cx).work_directory_abs_path));
        nested
    }

    /// The folder the window is working in: the one picked, else the first
    /// visible folder belonging to the active repository, else the first
    /// visible folder.
    pub fn active_worktree(&self, cx: &App) -> Option<Entity<Worktree>> {
        let worktree_store = self.worktree_store.read(cx);
        if let Some(worktree) = self
            .active_worktree_id
            .and_then(|id| worktree_store.worktree_for_id(id, cx))
            .filter(|worktree| worktree.read(cx).is_visible())
        {
            return Some(worktree);
        }
        if let Some(repo) = self.active_repository() {
            let repo_path = Arc::clone(&repo.read(cx).work_directory_abs_path);
            if let Some(worktree) = worktree_store
                .visible_worktrees(cx)
                .find(|worktree| paths_overlap(&worktree.read(cx).abs_path(), &repo_path))
            {
                return Some(worktree);
            }
        }
        worktree_store.visible_worktrees(cx).next()
    }

    /// Activates a newly added repository when nothing is active and it
    /// belongs to the active folder (or no folder was picked yet).
    fn activate_added_repository(&mut self, repo_id: RepositoryId, cx: &mut Context<Self>) {
        if self.active_repo_id.is_some() {
            return;
        }
        let Some(repo) = self.repositories.get(&repo_id) else {
            return;
        };
        let repo_path = Arc::clone(&repo.read(cx).work_directory_abs_path);
        let fits_folder =
            match self.active_worktree_id.and_then(|id| self.worktree_store.read(cx).worktree_for_id(id, cx)) {
                Some(worktree) => paths_overlap(&worktree.read(cx).abs_path(), &repo_path),
                None => true,
            };
        if fits_folder {
            self.active_repo_id = Some(repo_id);
            cx.emit(GitStoreEvent::ActiveRepositoryChanged(Some(repo_id)));
        }
    }

    pub fn active_repository(&self) -> Option<Entity<Repository>> {
        self.active_repo_id.as_ref().map(|id| self.repositories[id].clone())
    }

    fn file_is_symlink(file: &File, cx: &App) -> bool {
        file.worktree.read(cx).entry_for_path(&file.path).is_some_and(|entry| entry.canonical_path.is_some())
    }

    fn buffer_is_symlink(buffer: &Entity<Buffer>, cx: &App) -> bool {
        File::from_dyn(buffer.read(cx).file()).is_some_and(|file| Self::file_is_symlink(file, cx))
    }

    pub fn project_path_git_status(&self, project_path: &ProjectPath, cx: &App) -> Option<FileStatus> {
        let (repo, repo_path) = self.repository_and_path_for_project_path(project_path, cx)?;
        Some(repo.read(cx).status_for_path(&repo_path)?.status)
    }

    /// Blames a buffer.
    pub(crate) fn blame_buffer(
        &self,
        buffer: &Entity<Buffer>,
        version: Option<clock::Global>,
        cx: &Context<Self>,
    ) -> Task<Result<Option<Blame>>> {
        let buffer = buffer.read(cx);
        let Some((repo, repo_path)) = self.repository_and_path_for_buffer_id(buffer.buffer_id(), cx) else {
            return Task::ready(Err(anyhow!("failed to find a git repository for buffer")));
        };
        let content = match &version {
            Some(version) => buffer.rope_for_version(version),
            None => buffer.as_rope().clone(),
        };
        let line_ending = buffer.line_ending();

        let repo = repo.downgrade();
        cx.spawn(async move |_, cx| {
            let repository_state =
                repo.update(cx, |repo, _| repo.repository_state.clone())?.await.map_err(|err| anyhow::anyhow!(err))?;
            {
                let RepositoryState { backend, .. } = repository_state;
                backend
                    .blame(repo_path.clone(), content, line_ending)
                    .await
                    .with_context(|| format!("Failed to blame {:?}", repo_path.as_ref()))
                    .map(Some)
            }
        })
    }

    fn on_worktree_store_event(
        &mut self,
        worktree_store: Entity<WorktreeStore>,
        event: &WorktreeStoreEvent,
        cx: &mut Context<Self>,
    ) {
        let GitStoreState { project_environment, fs, .. } = &self.state;

        match event {
            WorktreeStoreEvent::UpdatedEntries(worktree_id, updated_entries) => {
                if let Some(worktree) = self.worktree_store.read(cx).worktree_for_id(*worktree_id, cx) {
                    let paths_by_git_repo = self.process_updated_entries(&worktree, updated_entries, cx);
                    cx.spawn(async move |_, cx| {
                        let paths_by_git_repo = paths_by_git_repo.await;
                        for (repo, paths) in paths_by_git_repo {
                            repo.update(cx, |repo, cx| {
                                repo.paths_changed(paths, cx);
                            });
                        }
                    })
                    .detach();
                }
            }
            WorktreeStoreEvent::UpdatedGitRepositories(worktree_id, changed_repos) => {
                if worktree_store.read(cx).worktree_for_id(*worktree_id, cx).is_none() {
                    return;
                }
                log::debug!("received worktree update for repositories: {changed_repos:?}");
                self.update_repositories_from_worktree(
                    *worktree_id,
                    project_environment.clone(),
                    Arc::clone(changed_repos),
                    Arc::clone(fs),
                    cx,
                );
                self.local_worktree_git_repos_changed(changed_repos, cx);
            }
            WorktreeStoreEvent::Removed(_entity_id, worktree_id) => {
                self.parked_repositories.retain(|parked| parked.worktree_id != *worktree_id);
                let repos_without_worktree: Vec<RepositoryId> = self
                    .worktree_ids
                    .iter_mut()
                    .filter_map(|(repo_id, worktree_ids)| {
                        worktree_ids.remove(worktree_id);
                        worktree_ids.is_empty().then_some(*repo_id)
                    })
                    .collect();
                let is_active_repo_removed =
                    repos_without_worktree.iter().any(|repo_id| self.active_repo_id == Some(*repo_id));
                if self.active_worktree_id == Some(*worktree_id) {
                    self.active_worktree_id = None;
                    cx.emit(GitStoreEvent::ActiveWorktreeChanged);
                }

                for repo_id in repos_without_worktree {
                    self.repositories.remove(&repo_id);
                    self.worktree_ids.remove(&repo_id);
                }

                if is_active_repo_removed {
                    self.select_fallback_active_repository(cx);
                }
            }
            // lint: expect(wildcard_match) only some variants matter here
            _ => {}
        }
    }
    /// After the active repository is removed: the active folder's repository
    /// when that folder is still open, else any remaining repository.
    fn select_fallback_active_repository(&mut self, cx: &mut Context<Self>) {
        let worktree_id =
            self.active_worktree_id.filter(|id| self.worktree_store.read(cx).worktree_for_id(*id, cx).is_some());
        let repo_id = match worktree_id {
            Some(worktree_id) => self.repository_for_worktree(worktree_id, cx).map(|repo| repo.read(cx).id),
            None => self.repositories.keys().next().copied(),
        };
        self.set_active(worktree_id, repo_id, cx);
    }

    fn on_repository_event(&mut self, repo: Entity<Repository>, event: &RepositoryEvent, cx: &mut Context<Self>) {
        let id = repo.read(cx).id;
        let repo_snapshot = repo.read(cx).snapshot.clone();
        for (buffer_id, diff) in &self.diffs {
            if let Some((buffer_repo, repo_path)) = self.repository_and_path_for_buffer_id(*buffer_id, cx)
                && buffer_repo == repo
            {
                diff.update(cx, |diff, cx| {
                    if let Some(conflict_set) = &diff.conflict_set {
                        let conflict_status_changed = conflict_set.update(cx, |conflict_set, cx| {
                            let has_conflict = repo_snapshot.has_conflict(&repo_path);
                            conflict_set.set_has_conflict(has_conflict, cx)
                        })?;
                        if conflict_status_changed {
                            let buffer_store = self.buffer_store.read(cx);
                            if let Some(buffer) = buffer_store.get(*buffer_id) {
                                drop(diff.reparse_conflict_markers(buffer.read(cx).text_snapshot(), cx));
                            }
                        }
                    }
                    anyhow::Ok(())
                })
                .ok();
            }
        }

        cx.emit(GitStoreEvent::RepositoryUpdated(id, event.clone(), self.active_repo_id == Some(id)));
    }

    fn on_jobs_updated(&mut self, _: Entity<Repository>, _: &JobsUpdated, cx: &mut Context<Self>) {
        cx.emit(GitStoreEvent::JobsUpdated);
    }

    /// Update our list of repositories and schedule git scans in response to a notification from a worktree,.
    fn update_repositories_from_worktree(
        &mut self,
        worktree_id: WorktreeId,
        project_environment: Entity<ProjectEnvironment>,
        updated_git_repositories: UpdatedGitRepositoriesSet,
        fs: Arc<Fs>,
        cx: &mut Context<Self>,
    ) {
        let mut removed_ids = Vec::new();

        let mut sorted_updates = updated_git_repositories.iter().collect::<Vec<_>>();
        sorted_updates.sort_by_key(|update| {
            update
                .new_work_directory_abs_path
                .as_ref()
                .map(|work_directory_abs_path| work_directory_abs_path.components().count())
        });
        for update in sorted_updates {
            self.retarget_parked_repositories(update);

            if let Some((repo_id, existing)) = self
                .repositories
                .iter()
                .find(|(_, repo)| {
                    let existing_work_directory_abs_path = &repo.read(cx).work_directory_abs_path;
                    Some(existing_work_directory_abs_path) == update.old_work_directory_abs_path.as_ref()
                        || Some(existing_work_directory_abs_path) == update.new_work_directory_abs_path.as_ref()
                })
                .map(|(id, repo)| (*id, repo.clone()))
            {
                if let Some(new_work_directory_abs_path) = update.new_work_directory_abs_path.clone() {
                    let mut merged_worktree_ids = self.take_parked_at(&new_work_directory_abs_path);
                    merged_worktree_ids.insert(worktree_id);
                    self.worktree_ids.entry(repo_id).or_default().extend(merged_worktree_ids);
                    let path_changed =
                        update.old_work_directory_abs_path.as_ref() != update.new_work_directory_abs_path.as_ref();
                    if path_changed
                        && let Some(dot_git_abs_path) = update.dot_git_abs_path.clone()
                        && let Some(repository_dir_abs_path) = update.repository_dir_abs_path.clone()
                        && let Some(common_dir_abs_path) = update.common_dir_abs_path.clone()
                    {
                        existing.update(cx, |existing, cx| {
                            existing.reinitialize_local_backend(
                                new_work_directory_abs_path,
                                dot_git_abs_path,
                                repository_dir_abs_path,
                                common_dir_abs_path,
                                project_environment.downgrade(),
                                Arc::clone(&fs),
                                cx,
                            );
                            existing.schedule_scan(cx);
                        });
                    } else {
                        existing.update(cx, |existing, cx| {
                            existing.snapshot.work_directory_abs_path = new_work_directory_abs_path;
                            existing.schedule_scan(cx);
                        });
                    }
                } else if let Some(worktree_ids) = self.worktree_ids.get_mut(&repo_id) {
                    worktree_ids.remove(&worktree_id);
                    if worktree_ids.is_empty() {
                        removed_ids.push(repo_id);
                    }
                }
            } else if let UpdatedGitRepository {
                new_work_directory_abs_path: Some(work_directory_abs_path),
                dot_git_abs_path: Some(dot_git_abs_path),
                repository_dir_abs_path: Some(repository_dir_abs_path),
                common_dir_abs_path: Some(common_dir_abs_path),
                ..
            } = update
            {
                let Some(worktree) = self.worktree_store.read(cx).worktree_for_id(worktree_id, cx) else {
                    continue;
                };
                let parked_repository = ParkedRepository {
                    worktree_id,
                    work_directory_abs_path: Arc::clone(work_directory_abs_path),
                    dot_git_abs_path: Arc::clone(dot_git_abs_path),
                    repository_dir_abs_path: Arc::clone(repository_dir_abs_path),
                    common_dir_abs_path: Arc::clone(common_dir_abs_path),
                };
                if self.should_park_repository(&parked_repository, &worktree, cx) {
                    self.parked_repositories.retain(|parked| {
                        parked.worktree_id != parked_repository.worktree_id
                            || parked.work_directory_abs_path != parked_repository.work_directory_abs_path
                    });
                    self.parked_repositories.push(parked_repository);
                } else {
                    self.register_local_repository(parked_repository, cx);
                }
            }
        }

        for id in removed_ids {
            if self.active_repo_id == Some(id) {
                self.active_repo_id = None;
                cx.emit(GitStoreEvent::ActiveRepositoryChanged(None));
            }
            self.repositories.remove(&id);
        }
    }

    fn retarget_parked_repositories(&mut self, update: &UpdatedGitRepository) {
        match (&update.old_work_directory_abs_path, &update.new_work_directory_abs_path) {
            (Some(old_work_directory_abs_path), Some(new_work_directory_abs_path))
                if old_work_directory_abs_path != new_work_directory_abs_path =>
            {
                if let (Some(dot_git_abs_path), Some(repository_dir_abs_path), Some(common_dir_abs_path)) =
                    (&update.dot_git_abs_path, &update.repository_dir_abs_path, &update.common_dir_abs_path)
                {
                    for worktree_id in self.take_parked_at(old_work_directory_abs_path) {
                        let already_parked = self.parked_repositories.iter().any(|parked| {
                            parked.worktree_id == worktree_id
                                && parked.work_directory_abs_path == *new_work_directory_abs_path
                        });
                        if !already_parked {
                            self.parked_repositories.push(ParkedRepository {
                                worktree_id,
                                work_directory_abs_path: Arc::clone(new_work_directory_abs_path),
                                dot_git_abs_path: Arc::clone(dot_git_abs_path),
                                repository_dir_abs_path: Arc::clone(repository_dir_abs_path),
                                common_dir_abs_path: Arc::clone(common_dir_abs_path),
                            });
                        }
                    }
                } else {
                    self.parked_repositories
                        .retain(|parked| &parked.work_directory_abs_path != old_work_directory_abs_path);
                }
            }
            (Some(old_work_directory_abs_path), None) => {
                self.parked_repositories
                    .retain(|parked| &parked.work_directory_abs_path != old_work_directory_abs_path);
            }
            _ => {}
        }
    }

    fn should_park_repository(&self, repository: &ParkedRepository, worktree: &Entity<Worktree>, cx: &App) -> bool {
        if !Self::parking_enabled() {
            return false;
        }
        let worktree = worktree.read(cx);
        if worktree.root_repo_common_dir().is_some() {
            return false;
        }
        let deep_enough = match repository.work_directory_abs_path.strip_prefix(worktree.abs_path()) {
            Ok(relative_path) => relative_path.components().count() >= MIN_PARKED_REPOSITORY_DEPTH,
            Err(_) => false,
        };
        deep_enough
            && !self.repositories.values().any(|active_repository| {
                repository.work_directory_abs_path.starts_with(&active_repository.read(cx).work_directory_abs_path)
            })
            && !self.buffer_store.read(cx).buffers().any(|buffer| {
                buffer
                    .read(cx)
                    .project_path(cx)
                    .and_then(|project_path| self.worktree_store.read(cx).absolutize(&project_path, cx))
                    .is_some_and(|abs_path| abs_path.starts_with(&repository.work_directory_abs_path))
            })
    }

    fn parking_enabled() -> bool {
        WorktreeSettings::get().file_scan_depth.is_some()
    }

    fn take_parked_at(&mut self, work_directory_abs_path: &Path) -> HashSet<WorktreeId> {
        let mut worktree_ids = HashSet::default();
        self.parked_repositories.retain(|parked| {
            if parked.work_directory_abs_path.as_ref() == work_directory_abs_path {
                worktree_ids.insert(parked.worktree_id);
                false
            } else {
                true
            }
        });
        worktree_ids
    }

    fn activate_parked_repositories(
        &mut self,
        mut should_activate: impl FnMut(&ParkedRepository) -> bool,
        cx: &mut Context<Self>,
    ) {
        if self.parked_repositories.is_empty() {
            return;
        }
        let (to_activate, to_keep) = mem::take(&mut self.parked_repositories)
            .into_iter()
            .partition::<Vec<_>, _>(|parked| should_activate(parked));
        self.parked_repositories = to_keep;
        for parked in to_activate {
            self.register_local_repository(parked, cx);
        }
    }

    fn register_local_repository(&mut self, repository: ParkedRepository, cx: &mut Context<Self>) {
        let mut worktree_ids = self.take_parked_at(&repository.work_directory_abs_path);
        worktree_ids.insert(repository.worktree_id);
        if let Some((&existing_id, _)) = self
            .repositories
            .iter()
            .find(|(_, existing)| existing.read(cx).work_directory_abs_path == repository.work_directory_abs_path)
        {
            self.worktree_ids.entry(existing_id).or_default().extend(worktree_ids);
            self.activate_parked_repositories_under(&repository.work_directory_abs_path, cx);
            return;
        }
        let GitStoreState { next_repository_id, project_environment, fs, .. } = &self.state;
        let project_environment = project_environment.downgrade();
        let fs = Arc::clone(fs);

        let id = RepositoryId(next_repository_id.fetch_add(1, atomic::Ordering::Release));
        let git_store = cx.weak_entity();
        let object_read_limiter = Arc::clone(&self.object_read_limiter);
        let repo = cx.new(|cx| {
            let mut repo = Repository::local(
                id,
                Arc::clone(&repository.work_directory_abs_path),
                Arc::clone(&repository.repository_dir_abs_path),
                Arc::clone(&repository.common_dir_abs_path),
                Arc::clone(&repository.dot_git_abs_path),
                project_environment,
                fs,
                git_store,
                object_read_limiter,
                cx,
            );
            repo.schedule_scan(cx);
            repo
        });
        self._subscriptions.push(cx.subscribe(&repo, Self::on_repository_event));
        self._subscriptions.push(cx.subscribe(&repo, Self::on_jobs_updated));
        self.repositories.insert(id, repo);
        self.worktree_ids.insert(id, worktree_ids);
        cx.emit(GitStoreEvent::RepositoryAdded);
        self.activate_added_repository(id, cx);
        self.activate_parked_repositories_under(&repository.work_directory_abs_path, cx);
    }

    fn activate_parked_repositories_under(&mut self, work_directory_abs_path: &Path, cx: &mut Context<Self>) {
        self.activate_parked_repositories(
            |parked| parked.work_directory_abs_path.starts_with(work_directory_abs_path),
            cx,
        );
    }

    fn activate_parked_repositories_for_buffer(&mut self, buffer: &Entity<Buffer>, cx: &mut Context<Self>) {
        if self.parked_repositories.is_empty() {
            return;
        }
        if let Some(abs_path) = buffer
            .read(cx)
            .project_path(cx)
            .and_then(|project_path| self.worktree_store.read(cx).absolutize(&project_path, cx))
        {
            self.activate_parked_repositories(|parked| abs_path.starts_with(&parked.work_directory_abs_path), cx);
        }
    }

    fn on_buffer_store_event(&mut self, _: Entity<BufferStore>, event: &BufferStoreEvent, cx: &mut Context<Self>) {
        match event {
            BufferStoreEvent::Added(buffer) => {
                self.activate_parked_repositories_for_buffer(buffer, cx);
                cx.subscribe(buffer, |this, buffer, event, cx| {
                    if let BufferEvent::LanguageChanged(_) = event {
                        let buffer_id = buffer.read(cx).buffer_id();
                        if let Some(diff_state) = this.diffs.get(&buffer_id) {
                            diff_state.update(cx, |diff_state, cx| {
                                diff_state.buffer_language_changed(buffer, cx);
                            });
                        }
                    }
                })
                .detach();
            }
            BufferStoreEvent::Dropped(buffer_id) => {
                self.diffs.remove(buffer_id);
                self.buffer_ids_by_index_text_buffer_id.retain(|_, main_buffer_id| main_buffer_id != buffer_id);
            }
            BufferStoreEvent::ChangedFilePath { buffer, .. } => {
                self.activate_parked_repositories_for_buffer(buffer, cx);
                // Whenever a buffer's file path changes, it's possible that the
                // new path is actually a path that is being tracked by a git
                // repository. In that case, we'll want to update the buffer's
                // `BufferDiffState`, in case it already has one.
                let buffer_id = buffer.read(cx).buffer_id();
                let diff_state = self.diffs.get(&buffer_id);
                let repo = self.repository_and_path_for_buffer_id(buffer_id, cx);

                if let Some(diff_state) = diff_state
                    && let Some((repo, repo_path)) = repo
                {
                    let buffer = buffer.clone();
                    let diff_state = diff_state.clone();
                    let is_symlink = Self::buffer_is_symlink(&buffer, cx);

                    cx.spawn(async move |_git_store, cx| {
                        async {
                            let diff_bases_change = if is_symlink {
                                DiffBasesChange::Both(None)
                            } else {
                                repo.update(cx, |repo, cx| repo.load_committed_text(repo_path, cx)).await?
                            };

                            diff_state.update(cx, |diff_state, cx| {
                                let buffer_snapshot = buffer.read(cx).text_snapshot();
                                diff_state.diff_bases_changed(buffer_snapshot, Some(diff_bases_change), cx);
                            });
                            anyhow::Ok(())
                        }
                        .await
                        .log_err();
                    })
                    .detach();
                }
            }
        }
    }

    fn local_worktree_git_repos_changed(&self, changed_repos: &UpdatedGitRepositoriesSet, cx: &mut Context<Self>) {
        log::debug!("local worktree repos changed");

        for repository in self.repositories.values() {
            repository.update(cx, |repository, cx| {
                let repo_abs_path = &repository.work_directory_abs_path;
                if changed_repos.iter().any(|update| {
                    update.old_work_directory_abs_path.as_ref() == Some(repo_abs_path)
                        || update.new_work_directory_abs_path.as_ref() == Some(repo_abs_path)
                }) {
                    repository.reload_buffer_diff_bases(cx);
                }
            });
        }
    }

    pub const fn repositories(&self) -> &FxHashMap<RepositoryId, Entity<Repository>> {
        &self.repositories
    }

    pub fn status_for_buffer_id(&self, buffer_id: BufferId, cx: &App) -> Option<FileStatus> {
        let (repo, path) = self.repository_and_path_for_buffer_id(buffer_id, cx)?;
        let status = repo.read(cx).snapshot.status_for_path(&path)?;
        Some(status.status)
    }

    pub fn repository_and_path_for_buffer_id(
        &self,
        buffer_id: BufferId,
        cx: &App,
    ) -> Option<(Entity<Repository>, RepoPath)> {
        let buffer = self.buffer_store.read(cx).get(buffer_id)?;
        let project_path = buffer.read(cx).project_path(cx)?;
        self.repository_and_path_for_project_path(&project_path, cx)
    }

    pub fn repository_and_path_for_project_path(
        &self,
        path: &ProjectPath,
        cx: &App,
    ) -> Option<(Entity<Repository>, RepoPath)> {
        let abs_path = self.worktree_store.read(cx).absolutize(path, cx)?;
        self.repositories
            .values()
            .filter_map(|repo| {
                let repo_path = repo.read(cx).abs_path_to_repo_path(&abs_path)?;
                Some((repo.clone(), repo_path))
            })
            .max_by_key(|(repo, _)| Arc::clone(&repo.read(cx).work_directory_abs_path))
    }

    pub(crate) fn git_init(&self, path: Arc<Path>, fallback_branch_name: String, cx: &App) -> Task<Result<()>> {
        let fs = Arc::clone(&self.state.fs);
        cx.background_executor().spawn(async move { fs.git_init(&path, fallback_branch_name).await })
    }

    pub fn git_clone(&self, repo: String, path: impl Into<Arc<Path>>, cx: &App) -> Task<Result<()>> {
        let path = path.into();
        let fs = Arc::clone(&self.state.fs);
        cx.background_executor().spawn(async move { fs.git_clone(&path, &repo).await })
    }

    pub(crate) fn git_config(&self, path: Arc<Path>, args: Vec<String>, cx: &App) -> Task<Result<String>> {
        let fs = Arc::clone(&self.state.fs);
        cx.background_executor().spawn(async move { fs.git_config(&path, args).await })
    }

    pub fn repo_snapshots(&self, cx: &App) -> FxHashMap<RepositoryId, RepositorySnapshot> {
        self.repositories.iter().map(|(id, repo)| (*id, repo.read(cx).snapshot.clone())).collect()
    }

    pub fn status_for_project_path(&self, project_path: &ProjectPath, cx: &App) -> Option<FileStatus> {
        let (repo, repo_path) = self.repository_and_path_for_project_path(project_path, cx)?;
        repo.read(cx).status_for_path(&repo_path).map(|entry| entry.status)
    }

    fn coalesce_repo_paths(mut paths: Vec<RepoPath>) -> Vec<RepoPath> {
        paths.sort();

        let mut coalesced = Vec::with_capacity(paths.len());
        for path in paths {
            if coalesced.last().is_some_and(|ancestor: &RepoPath| path.starts_with(ancestor)) {
                continue;
            }
            coalesced.push(path);
        }

        coalesced
    }

    fn limit_repo_paths(paths: Vec<RepoPath>) -> Vec<RepoPath> {
        if paths.len() > MAX_SCOPED_STATUS_PATHS { vec![RepoPath::from_rel_path(RelPath::empty())] } else { paths }
    }

    fn process_updated_entries(
        &self,
        worktree: &Entity<Worktree>,
        updated_entries: &[(Arc<RelPath>, ProjectEntryId, PathChange)],
        cx: &App,
    ) -> Task<FxHashMap<Entity<Repository>, Vec<RepoPath>>> {
        let mut repo_paths = self
            .repositories
            .values()
            .map(|repo| (Arc::clone(&repo.read(cx).work_directory_abs_path), repo.clone()))
            .collect::<Vec<_>>();
        let mut entries: Vec<_> = updated_entries.iter().map(|(path, _, _)| Arc::clone(path)).collect();
        entries.sort();
        let worktree = worktree.read(cx);

        let entries = entries.into_iter().map(|path| worktree.absolutize(&path)).collect::<Arc<[_]>>();

        let executor = cx.background_executor().clone();
        cx.background_executor().spawn(async move {
            repo_paths.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
            let mut paths_by_git_repo = FxHashMap::<_, Vec<_>>::default();
            let mut tasks = FuturesOrdered::new();
            for (repo_path, repo) in repo_paths.into_iter().rev() {
                let entries = Arc::clone(&entries);
                let task = executor.spawn(async move {
                    // Find all repository paths that belong to this repo
                    let mut ix = entries.partition_point(|path| path < &*repo_path);
                    if ix == entries.len() {
                        return None;
                    }

                    let mut paths = Vec::new();
                    // All paths prefixed by a given repo will constitute a continuous range.
                    while let Some(path) = entries.get(ix)
                        && let Some(repo_path) = RepositorySnapshot::abs_path_to_repo_path_inner(&repo_path, path)
                    {
                        paths.push((repo_path, ix));
                        ix += 1;
                    }
                    if paths.is_empty() { None } else { Some((repo, paths)) }
                });
                tasks.push_back(task);
            }

            // Now, let's filter out the "duplicate" entries that were processed by multiple distinct repos.
            let mut path_was_used = vec![false; entries.len()];
            let tasks = tasks.collect::<Vec<_>>().await;
            // Process tasks from the back: iterating backwards allows us to see more-specific paths first.
            // We always want to assign a path to it's innermost repository.
            for t in tasks {
                let Some((repo, paths)) = t else {
                    continue;
                };
                let entry = paths_by_git_repo.entry(repo).or_default();
                for (repo_path, ix) in paths {
                    if path_was_used[ix] {
                        continue;
                    }
                    path_was_used[ix] = true;
                    entry.push(repo_path);
                }
            }

            for paths in paths_by_git_repo.values_mut() {
                *paths = Self::coalesce_repo_paths(mem::take(paths));
            }

            paths_by_git_repo
        })
    }
}

impl RepositorySnapshot {
    fn empty(
        id: RepositoryId,
        work_directory_abs_path: Arc<Path>,
        repository_dir_abs_path: Option<Arc<Path>>,
        dot_git_abs_path: Option<Arc<Path>>,
        common_dir_abs_path: Option<Arc<Path>>,
    ) -> Self {
        let repository_dir_abs_path =
            repository_dir_abs_path.unwrap_or_else(|| work_directory_abs_path.join(".git").into());
        let dot_git_abs_path = dot_git_abs_path.unwrap_or_else(|| work_directory_abs_path.join(".git").into());
        let common_dir_abs_path = common_dir_abs_path.unwrap_or_else(|| Arc::clone(&repository_dir_abs_path));

        Self {
            id,
            statuses_by_path: SumTree::default(),
            repository_dir_abs_path,
            dot_git_abs_path,
            common_dir_abs_path,
            work_directory_abs_path,
            branch: None,
            branch_list: Arc::from([]),
            branch_list_error: None,
            head_commit: None,
            scan_id: 0,
            merge: MergeDetails::default(),
            remote_origin_url: None,
            remote_upstream_url: None,
            stash_entries: GitStash::default(),
            linked_worktrees: Arc::from([]),
        }
    }

    /// Returns the main worktree path for this repository, if one exists.
    ///
    /// Linked worktrees attached to bare repositories do not have a main
    /// worktree. For linked worktrees attached to a non-bare repository, the
    /// common Git directory is the main worktree's `.git` directory.
    pub fn main_worktree_abs_path(&self) -> Option<&Path> {
        if self.is_linked_worktree() {
            if self.common_dir_abs_path.file_name() == Some(OsStr::new(".git")) {
                self.common_dir_abs_path.parent()
            } else {
                None
            }
        } else {
            Some(self.work_directory_abs_path.as_ref())
        }
    }

    /// The path that anchors a linked worktree's location: the main worktree
    /// when one exists, otherwise the repository's identity path (relevant for
    /// bare repositories).
    fn linked_worktree_anchor_path(&self) -> &Path {
        self.main_worktree_abs_path().unwrap_or_else(|| repo_identity_path(&self.common_dir_abs_path))
    }

    /// Computes the path where a new linked worktree for this repository
    /// would be created, under [`worktrees_directory_for_repo`].
    pub fn path_for_new_linked_worktree(&self, worktree_name: &str) -> Result<PathBuf> {
        let repository_anchor = self.linked_worktree_anchor_path();
        let project_name =
            repository_anchor.file_name().ok_or_else(|| anyhow!("git repo must have a directory name"))?;
        let directory = worktrees_directory_for_repo(repository_anchor)?;
        let directory = path::join_path(&directory, worktree_name)?;
        path::join_path(&directory, project_name)
    }

    /// Returns true if this repository is a linked worktree, that is, one that
    /// was created from another worktree.
    ///
    /// Returns `false` for both the main worktree and submodules.
    pub fn is_linked_worktree(&self) -> bool {
        self.repository_dir_abs_path != self.common_dir_abs_path
    }

    fn status(&self) -> impl Iterator<Item = StatusEntry> + '_ {
        self.statuses_by_path.iter().cloned()
    }

    pub fn status_summary(&self) -> GitSummary {
        self.statuses_by_path.summary().item_summary
    }

    pub fn status_for_path(&self, path: &RepoPath) -> Option<StatusEntry> {
        self.statuses_by_path.get(&PathKey(Arc::clone(path.as_ref())), ()).cloned()
    }

    fn diff_stat_for_path(&self, path: &RepoPath) -> Option<DiffStat> {
        self.statuses_by_path.get(&PathKey(Arc::clone(path.as_ref())), ()).and_then(|entry| entry.diff_stat)
    }

    pub fn abs_path_to_repo_path(&self, abs_path: &Path) -> Option<RepoPath> {
        Self::abs_path_to_repo_path_inner(&self.work_directory_abs_path, abs_path)
    }

    pub fn repo_path_to_abs_path(&self, repo_path: &RepoPath) -> PathBuf {
        PathBuf::from(
            path::join(&self.work_directory_abs_path, repo_path.display())
                .expect("repo path joins onto the work directory"),
        )
    }

    #[inline]
    fn abs_path_to_repo_path_inner(work_directory_abs_path: &Path, abs_path: &Path) -> Option<RepoPath> {
        let rel_path = abs_path.strip_prefix(work_directory_abs_path).ok().and_then(|path| RelPath::new(path).ok())?;
        Some(RepoPath::from_rel_path(&rel_path))
    }

    pub fn had_conflict_on_last_merge_head_change(&self, repo_path: &RepoPath) -> bool {
        self.merge.merge_heads_by_conflicted_path.contains_key(repo_path)
    }

    fn has_conflict(&self, repo_path: &RepoPath) -> bool {
        let had_conflict_on_last_merge_head_change = self.merge.merge_heads_by_conflicted_path.contains_key(repo_path);
        let has_conflict_currently = self.status_for_path(repo_path).is_some_and(|entry| entry.status.is_conflicted());
        had_conflict_on_last_merge_head_change || has_conflict_currently
    }

    /// This is the name that will be displayed in the repository selector for this repository.
    pub fn display_name(&self) -> SharedString {
        self.work_directory_abs_path.file_name().unwrap_or_default().to_string_lossy().to_string().into()
    }
}

/// Git keeps its rebase state directory exactly as long as the rebase lasts;
/// `REBASE_HEAD` can outlive it.
fn rebase_in_progress(repository_dir: &Path) -> bool {
    repository_dir.join(git::REBASE_MERGE_DIR).exists() || repository_dir.join(git::REBASE_APPLY_DIR).exists()
}

impl MergeDetails {
    async fn update(
        &mut self,
        backend: &Arc<dyn GitRepository>,
        repository_dir: &Path,
        current_conflicted_paths: Vec<RepoPath>,
    ) -> bool {
        log::debug!("load merge details");
        self.message = backend.merge_message().await.map(SharedString::from);
        let heads = backend
            .revparse_batch(vec![
                "MERGE_HEAD".into(),
                "CHERRY_PICK_HEAD".into(),
                "REBASE_HEAD".into(),
                "REVERT_HEAD".into(),
                "APPLY_HEAD".into(),
            ])
            .await
            .log_err()
            .unwrap_or_default()
            .into_iter()
            .map(|opt| opt.map(SharedString::from))
            .collect::<Vec<_>>();

        let rebase_in_progress = rebase_in_progress(repository_dir);
        let mut conflicts_changed = self.rebase_in_progress != rebase_in_progress;
        self.rebase_in_progress = rebase_in_progress;

        // Record the merge state for newly conflicted paths
        for path in &current_conflicted_paths {
            if self.merge_heads_by_conflicted_path.get(path).is_none() {
                conflicts_changed = true;
                self.merge_heads_by_conflicted_path.insert(path.clone(), heads.clone());
            }
        }

        // Clear state for paths that are no longer conflicted and for which the merge heads have changed
        self.merge_heads_by_conflicted_path.retain(|path, old_merge_heads| {
            let keep = current_conflicted_paths.contains(path)
                || (old_merge_heads == &heads && old_merge_heads.iter().any(Option::is_some));
            if !keep {
                conflicts_changed = true;
            }
            keep
        });

        conflicts_changed
    }
}

impl Repository {
    pub fn snapshot(&self) -> RepositorySnapshot {
        self.snapshot.clone()
    }

    pub fn pending_ops(&self) -> impl Iterator<Item = PendingOps> + '_ {
        self.pending_ops.iter().cloned()
    }

    pub fn pending_ops_summary(&self) -> PathSummary<PendingOpsSummary> {
        self.pending_ops.summary().clone()
    }

    pub fn pending_ops_for_path(&self, path: &RepoPath) -> Option<PendingOps> {
        self.pending_ops.get(&PathKey(Arc::clone(path.as_ref())), ()).cloned()
    }

    fn respawn_local_worker(
        &mut self,
        project_environment: WeakEntity<ProjectEnvironment>,
        fs: Arc<Fs>,
        cx: &mut Context<Self>,
    ) {
        let work_directory_abs_path = Arc::clone(&self.snapshot.work_directory_abs_path);
        let dot_git_abs_path = Arc::clone(&self.snapshot.dot_git_abs_path);

        let state = cx
            .spawn(async move |_, cx| {
                RepositoryState::new(work_directory_abs_path, dot_git_abs_path, project_environment, fs, cx)
                    .await
                    .map_err(|err| err.to_string())
            })
            .shared();
        self.job_sender.close_channel();
        self._worker_task = Task::ready(());
        // Loads queued on the old worker are canceled with it, so later loads need a new job.
        self.pending_diff_base_loads = Default::default();
        self.active_jobs.clear();
        cx.notify();

        let (job_sender, worker_task) = Repository::spawn_local_git_worker(state.clone(), cx);
        self.job_sender = job_sender;
        self._worker_task = worker_task;
        self.repository_state = cx
            .spawn(async move |_, _| {
                let state = state.await?;
                Ok(state)
            })
            .shared();
    }

    fn reinitialize_local_backend(
        &mut self,
        work_directory_abs_path: Arc<Path>,
        dot_git_abs_path: Arc<Path>,
        repository_dir_abs_path: Arc<Path>,
        common_dir_abs_path: Arc<Path>,
        project_environment: WeakEntity<ProjectEnvironment>,
        fs: Arc<Fs>,
        cx: &mut Context<Self>,
    ) {
        self.snapshot.work_directory_abs_path = work_directory_abs_path;
        self.snapshot.dot_git_abs_path = dot_git_abs_path;
        self.snapshot.repository_dir_abs_path = repository_dir_abs_path;
        self.snapshot.common_dir_abs_path = common_dir_abs_path;
        self.respawn_local_worker(project_environment, fs, cx);
    }

    fn local(
        id: RepositoryId,
        work_directory_abs_path: Arc<Path>,
        repository_dir_abs_path: Arc<Path>,
        common_dir_abs_path: Arc<Path>,
        dot_git_abs_path: Arc<Path>,
        project_environment: WeakEntity<ProjectEnvironment>,
        fs: Arc<Fs>,
        git_store: WeakEntity<GitStore>,
        object_read_limiter: Arc<Semaphore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshot = RepositorySnapshot::empty(
            id,
            work_directory_abs_path,
            Some(repository_dir_abs_path),
            Some(dot_git_abs_path),
            Some(common_dir_abs_path),
        );

        let mut repo = Repository {
            this: cx.weak_entity(),
            git_store,
            object_read_limiter,
            snapshot,
            unshallow_state: UnshallowState::default(),
            pending_ops: SumTree::default(),
            repository_state: Task::ready(Err("not yet initialized".into())).shared(),
            _worker_task: Task::ready(()),
            commit_message_buffer: None,
            paths_needing_status_update: Vec::default(),
            job_sender: mpsc::unbounded().0,
            job_id: 0,
            pending_diff_base_loads: Default::default(),
            active_jobs: FxHashMap::default(),
            initial_graph_data: FxHashMap::default(),
            commit_data: FxHashMap::default(),
            commit_data_handler: CommitDataHandlerState::Closed,
        };
        repo.respawn_local_worker(project_environment, fs, cx);
        cx.subscribe_self(Self::handle_subscribe_self).detach();
        repo
    }

    fn handle_subscribe_self(&mut self, event: &RepositoryEvent, _: &mut Context<Self>) {
        // scan id greater than 2 means the initial snapshot was calculated,
        // otherwise we don't need to refresh the graph state
        match event {
            RepositoryEvent::HeadChanged | RepositoryEvent::BranchListChanged => {
                if self.scan_id > 2 {
                    self.initial_graph_data.clear();
                }
            }
            // Tags are only changed by explicit user actions, never during the initial scan.
            RepositoryEvent::TagsChanged => {
                self.initial_graph_data.clear();
            }
            RepositoryEvent::StashEntriesChanged if self.scan_id > 2 => {
                self.initial_graph_data.retain(|(log_source, _), _| *log_source != LogSource::All);
            }
            // lint: expect(wildcard_match) only some variants matter here
            _ => {}
        }
    }

    pub fn git_store(&self) -> Option<Entity<GitStore>> {
        self.git_store.upgrade()
    }

    fn reload_buffer_diff_bases(&mut self, cx: &Context<Self>) {
        let this = cx.weak_entity();
        let git_store = self.git_store.clone();
        drop(self.send_keyed_job(Some(GitJobKey::ReloadBufferDiffBases), None, |state, mut cx| async move {
            let RepositoryState { backend, .. } = state;

            let Some(this) = this.upgrade() else {
                return Ok(());
            };

            let repo_diff_state_updates = this.update(&mut cx, |this, cx| {
                git_store.update(cx, |git_store, cx| {
                    git_store
                        .diffs
                        .iter()
                        .filter_map(|(buffer_id, diff_state)| {
                            let buffer_store = git_store.buffer_store.read(cx);
                            let buffer = buffer_store.get(*buffer_id)?;
                            let file = File::from_dyn(buffer.read(cx).file())?;
                            let abs_path = file.worktree.read(cx).absolutize(&file.path);
                            let repo_path = this.abs_path_to_repo_path(&abs_path)?;
                            let is_symlink = GitStore::file_is_symlink(file, cx);
                            log::debug!("start reload diff bases for repo path {}", repo_path.as_unix_str());
                            diff_state.update(cx, |diff_state, _| {
                                let has_unstaged_diff =
                                    diff_state.unstaged_diff.as_ref().is_some_and(|diff| diff.is_upgradable());
                                let has_staged_diff =
                                    diff_state.staged_diff.as_ref().is_some_and(|(diff, _)| diff.is_upgradable());
                                let has_uncommitted_diff =
                                    diff_state.uncommitted_diff.as_ref().is_some_and(|set| set.is_upgradable());

                                Some((
                                    buffer,
                                    repo_path,
                                    is_symlink,
                                    (has_unstaged_diff || has_staged_diff).then(|| diff_state.index_text.clone()),
                                    (has_staged_diff || has_uncommitted_diff).then(|| diff_state.head_text.clone()),
                                ))
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })?;

            let buffer_diff_base_changes = cx
                .background_spawn(async move {
                    let mut revision_groups = Vec::new();
                    for (_, repo_path, is_symlink, current_index_text, current_head_text) in &repo_diff_state_updates {
                        let mut revisions = Vec::new();
                        if current_index_text.is_some() && !*is_symlink {
                            revisions.push(format!(":{}", repo_path.as_unix_str()));
                        }
                        if current_head_text.is_some() && !*is_symlink {
                            revisions.push(format!("HEAD:{}", repo_path.as_unix_str()));
                        }
                        revision_groups.push(revisions);
                    }
                    let loaded_groups =
                        load_revision_groups(|revisions| backend.load_revisions(revisions), revision_groups).await;

                    let mut changes = Vec::new();
                    for ((buffer, _, is_symlink, current_index_text, current_head_text), loaded_revisions) in
                        repo_diff_state_updates.iter().zip(loaded_groups)
                    {
                        // A buffer whose texts fail to load keeps its current diff bases.
                        let Some(loaded_revisions) = loaded_revisions.log_err() else {
                            changes.push((buffer.clone(), None));
                            continue;
                        };
                        let mut loaded_revisions = loaded_revisions.into_iter();
                        let index_text = (current_index_text.is_some() && !*is_symlink)
                            .then(|| loaded_revisions.next().flatten())
                            .flatten()
                            .and_then(|bytes| decode_git_text(bytes).log_err());

                        let head_text = (current_head_text.is_some() && !*is_symlink)
                            .then(|| loaded_revisions.next().flatten())
                            .flatten()
                            .and_then(|bytes| decode_git_text(bytes).log_err());

                        let change = match (current_index_text.as_ref(), current_head_text.as_ref()) {
                            (Some(current_index), Some(current_head)) => {
                                let index_changed = index_text.as_deref() != current_index.as_deref();
                                let head_changed = head_text.as_deref() != current_head.as_deref();
                                if index_changed && head_changed {
                                    if index_text == head_text {
                                        Some(DiffBasesChange::Both(head_text))
                                    } else {
                                        Some(DiffBasesChange::Each { index: index_text, head: head_text })
                                    }
                                } else if index_changed {
                                    Some(DiffBasesChange::Index(index_text))
                                } else if head_changed {
                                    Some(DiffBasesChange::Head(head_text))
                                } else {
                                    None
                                }
                            }
                            (Some(current_index), None) => {
                                let index_changed = index_text.as_deref() != current_index.as_deref();
                                index_changed.then_some(DiffBasesChange::Index(index_text))
                            }
                            (None, Some(current_head)) => {
                                let head_changed = head_text.as_deref() != current_head.as_deref();
                                head_changed.then_some(DiffBasesChange::Head(head_text))
                            }
                            (None, None) => None,
                        };

                        changes.push((buffer.clone(), change));
                    }
                    changes
                })
                .await;

            git_store.update(&mut cx, |git_store, cx| {
                for (buffer, diff_bases_change) in buffer_diff_base_changes {
                    let buffer_snapshot = buffer.read(cx).text_snapshot();
                    let buffer_id = buffer_snapshot.buffer_id();
                    let Some(diff_state) = git_store.diffs.get(&buffer_id) else {
                        continue;
                    };

                    diff_state.update(cx, |diff_state, cx| {
                        diff_state.diff_bases_changed(buffer_snapshot, diff_bases_change, cx);
                    });
                }
            })
        }));
    }

    fn send_job<F, Fut, R>(&mut self, status: Option<SharedString>, job: F) -> oneshot::Receiver<R>
    where
        F: FnOnce(RepositoryState, AsyncApp) -> Fut + 'static,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.send_keyed_job(None, status, job)
    }

    fn send_keyed_job<F, Fut, R>(
        &mut self,
        key: Option<GitJobKey>,
        status: Option<SharedString>,
        job: F,
    ) -> oneshot::Receiver<R>
    where
        F: FnOnce(RepositoryState, AsyncApp) -> Fut + 'static,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let (result_tx, result_rx) = oneshot::channel();
        let job_id = post_inc(&mut self.job_id);
        let this = self.this.clone();

        self.job_sender
            .unbounded_send(GitJob {
                key,
                job: Box::new(move |state, cx: &mut AsyncApp| {
                    let job = job(state, cx.clone());
                    cx.spawn(async move |cx| {
                        this.update(cx, |this, cx| {
                            if let Some(s) = status {
                                this.active_jobs.insert(job_id, JobInfo { start: Instant::now(), message: s });
                            }
                            cx.notify();
                        })
                        .ok();

                        let result = job.await;

                        this.update(cx, |this, cx| {
                            this.active_jobs.remove(&job_id);
                            cx.notify();
                        })
                        .ok();

                        result_tx.send(result).ok();
                    })
                }),
            })
            .ok();
        result_rx
    }

    pub fn cached_status(&self) -> impl '_ + Iterator<Item = StatusEntry> {
        self.snapshot.status()
    }

    pub fn diff_stat_for_path(&self, path: &RepoPath) -> Option<DiffStat> {
        self.snapshot.diff_stat_for_path(path)
    }

    pub fn cached_stash(&self) -> GitStash {
        self.snapshot.stash_entries.clone()
    }

    pub fn repo_path_to_project_path(&self, path: &RepoPath, cx: &App) -> Option<ProjectPath> {
        let git_store = self.git_store.upgrade()?;
        let worktree_store = git_store.read(cx).worktree_store.read(cx);
        let abs_path = self.snapshot.repo_path_to_abs_path(path);
        let abs_path = SanitizedPath::new(&abs_path);
        let (worktree, relative_path) = worktree_store.find_worktree(abs_path, cx)?;
        Some(ProjectPath { worktree_id: worktree.read(cx).id(), path: relative_path })
    }

    pub fn project_path_to_repo_path(&self, path: &ProjectPath, cx: &App) -> Option<RepoPath> {
        let git_store = self.git_store.upgrade()?;
        let worktree_store = git_store.read(cx).worktree_store.read(cx);
        let abs_path = worktree_store.absolutize(path, cx)?;
        self.snapshot.abs_path_to_repo_path(&abs_path)
    }

    pub const fn commit_message_buffer(&self) -> Option<&Entity<Buffer>> {
        self.commit_message_buffer.as_ref()
    }

    pub fn open_commit_buffer(
        &mut self,
        languages: Option<Arc<LanguageRegistry>>,
        buffer_store: Entity<BufferStore>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Buffer>>> {
        if let Some(buffer) = self.commit_message_buffer.clone() {
            return Task::ready(Ok(buffer));
        }
        let this = cx.weak_entity();

        let rx = self.send_job(None, move |_, mut cx| async move {
            let Some(this) = this.upgrade() else {
                bail!("git store was dropped");
            };
            this.update(&mut cx, |_, cx| Self::open_local_commit_buffer(languages, buffer_store, cx)).await
        });

        cx.spawn(|_, _: &mut AsyncApp| async move { rx.await? })
    }

    fn open_local_commit_buffer(
        language_registry: Option<Arc<LanguageRegistry>>,
        buffer_store: Entity<BufferStore>,
        cx: &Context<Self>,
    ) -> Task<Result<Entity<Buffer>>> {
        cx.spawn(async move |repository, cx| {
            let git_commit_language = match language_registry {
                Some(language_registry) => Some(language_registry.language_for_name("Git Commit").await?),
                None => None,
            };
            let buffer = buffer_store
                .update(cx, |buffer_store, cx| buffer_store.create_buffer(git_commit_language, false, cx))
                .await?;

            repository.update(cx, |repository, _| {
                repository.commit_message_buffer = Some(buffer.clone());
            })?;
            Ok(buffer)
        })
    }

    pub fn checkout_files(&mut self, commit: &str, paths: Vec<RepoPath>, cx: &mut Context<Self>) -> Task<Result<()>> {
        let commit = commit.to_owned();

        self.spawn_job_with_tracking(paths.clone(), pending_op::GitStatus::Reverted, cx, async move |this, cx| {
            this.update(cx, |this, _cx| {
                this.send_job(Some(format!("git checkout {commit}").into()), move |git_repo, _| async move {
                    {
                        let RepositoryState { backend, environment, .. } = git_repo;
                        backend.checkout_files(commit, paths, Arc::clone(&environment)).await
                    }
                })
            })?
            .await?
        })
    }

    pub fn reset(
        &mut self,
        commit: String,
        reset_mode: ResetMode,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<()>> {
        let receiver = self.send_job(None, move |git_repo, _| async move {
            {
                let RepositoryState { backend, environment, .. } = git_repo;
                backend.reset(commit, reset_mode, environment).await
            }
        });

        self.schedule_scan(cx);

        receiver
    }

    pub fn show(&mut self, commit: String) -> oneshot::Receiver<Result<CommitDetails>> {
        self.send_job(None, move |state, _cx| Self::show_internal(state, commit))
    }

    pub fn show_commit(&self, sha: String, cx: &App) -> Task<Result<CommitDetails>> {
        let repository_state = self.repository_state.clone();
        let object_read_limiter = Arc::clone(&self.object_read_limiter);
        cx.background_spawn(async move {
            let _permit = object_read_limiter.acquire_arc().await;
            let state = repository_state.await.map_err(|err| anyhow::anyhow!(err))?;
            Self::show_internal(state, sha).await
        })
    }

    async fn show_internal(state: RepositoryState, commit: String) -> Result<CommitDetails> {
        {
            let RepositoryState { backend, .. } = state;
            backend.show(commit).await
        }
    }

    pub fn load_commit_diff(
        &self,
        commit: String,
        ignore_shallow_boundary: bool,
        cx: &App,
    ) -> Task<Result<CommitDiff>> {
        let repository_state = self.repository_state.clone();
        let object_read_limiter = Arc::clone(&self.object_read_limiter);
        cx.spawn(async move |cx| {
            let _permit = object_read_limiter.acquire_arc().await;
            let RepositoryState { backend, .. } = repository_state.await.map_err(|err| anyhow::anyhow!(err))?;
            backend.load_commit(commit, ignore_shallow_boundary, cx.clone()).await.map(decode_commit_diff)
        })
    }

    fn buffer_store(&self, cx: &App) -> Option<Entity<BufferStore>> {
        Some(self.git_store.upgrade()?.read(cx).buffer_store.clone())
    }

    fn save_buffers<'a>(
        &self,
        entries: impl IntoIterator<Item = &'a RepoPath>,
        cx: &mut Context<Self>,
    ) -> Vec<Task<Result<()>>> {
        let mut save_futures = Vec::new();
        if let Some(buffer_store) = self.buffer_store(cx) {
            buffer_store.update(cx, |buffer_store, cx| {
                for path in entries {
                    let Some(project_path) = self.repo_path_to_project_path(path, cx) else {
                        continue;
                    };
                    if let Some(buffer) = buffer_store.get_by_path(&project_path)
                        && buffer.read(cx).file().is_some_and(|file| file.disk_state().exists())
                        && buffer.read(cx).has_unsaved_edits()
                    {
                        save_futures.push(buffer_store.save_buffer(buffer, cx));
                    }
                }
            });
        }
        save_futures
    }

    pub fn stage_entries(&mut self, entries: Vec<RepoPath>, cx: &mut Context<Self>) -> Task<Result<()>> {
        self.stage_or_unstage_entries(true, entries, cx)
    }

    pub fn unstage_entries(&mut self, entries: Vec<RepoPath>, cx: &mut Context<Self>) -> Task<Result<()>> {
        self.stage_or_unstage_entries(false, entries, cx)
    }

    fn stage_or_unstage_entries(
        &mut self,
        stage: bool,
        entries: Vec<RepoPath>,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if entries.is_empty() {
            return Task::ready(Ok(()));
        }
        let Some(git_store) = self.git_store.upgrade() else {
            return Task::ready(Ok(()));
        };
        let save_tasks = self.save_buffers(&entries, cx);
        let paths = entries.iter().map(|p| p.as_unix_str()).collect::<Vec<_>>().join(" ");
        let status = if stage { format!("git add {paths}") } else { format!("git reset {paths}") };
        let job_key = GitJobKey::WriteIndex(entries.clone());

        self.spawn_job_with_tracking(
            entries.clone(),
            if stage { pending_op::GitStatus::Staged } else { pending_op::GitStatus::Unstaged },
            cx,
            async move |this, cx| {
                for save_task in save_tasks {
                    save_task.await?;
                }

                this.update(cx, |this, cx| {
                    let weak_this = cx.weak_entity();
                    this.send_keyed_job(Some(job_key), Some(status.into()), move |git_repo, mut cx| async move {
                        let hunk_staging_operation_counts = weak_this
                            .update(&mut cx, |this, cx| {
                                let mut hunk_staging_operation_counts = FxHashMap::default();
                                for path in &entries {
                                    let Some(project_path) = this.repo_path_to_project_path(path, cx) else {
                                        continue;
                                    };
                                    let Some(buffer) =
                                        git_store.read(cx).buffer_store.read(cx).get_by_path(&project_path)
                                    else {
                                        continue;
                                    };
                                    let Some(diff_state) =
                                        git_store.read(cx).diffs.get(&buffer.read(cx).buffer_id()).cloned()
                                    else {
                                        continue;
                                    };
                                    let buffer_snapshot = buffer.read(cx).text_snapshot();
                                    let hunk_staging_operation_count = diff_state.update(cx, |diff_state, cx| {
                                        diff_state.mark_whole_file_stage_or_unstage_pending(
                                            stage,
                                            &buffer_snapshot,
                                            cx,
                                        );
                                        diff_state.hunk_staging_operation_count += 1;
                                        diff_state.hunk_staging_operation_count
                                    });
                                    hunk_staging_operation_counts
                                        .insert(diff_state.downgrade(), hunk_staging_operation_count);
                                }
                                hunk_staging_operation_counts
                            })
                            .unwrap_or_default();

                        let result = {
                            let RepositoryState { backend, environment, .. } = git_repo;
                            if stage {
                                backend.stage_paths(entries, Arc::clone(&environment)).await
                            } else {
                                backend.unstage_paths(entries, Arc::clone(&environment)).await
                            }
                        };

                        for (diff_state, hunk_staging_operation_count) in hunk_staging_operation_counts {
                            diff_state
                                .update(&mut cx, |diff_state, cx| {
                                    if result.is_ok() {
                                        diff_state.hunk_staging_operation_count_as_of_write =
                                            hunk_staging_operation_count;
                                    } else {
                                        diff_state.clear_pending_hunks(cx);
                                    }
                                })
                                .ok();
                        }

                        result
                    })
                })?
                .await?
            },
        )
    }

    pub fn stage_all(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let snapshot = self.snapshot.clone();
        let pending_ops = self.pending_ops.clone();
        let to_stage = cx.background_spawn(async move {
            snapshot
                .status()
                .filter_map(|entry| {
                    if let Some(ops) = pending_ops
                        .get(&PathKey(Arc::clone(entry.repo_path.as_ref())), ())
                        .filter(|ops| !ops.last_op_errored())
                    {
                        if ops.staging() || ops.staged() { None } else { Some(entry.repo_path) }
                    } else if entry.status.staging().is_fully_staged() {
                        None
                    } else {
                        Some(entry.repo_path)
                    }
                })
                .collect()
        });

        cx.spawn(async move |this, cx| {
            let to_stage = to_stage.await;
            this.update(cx, |this, cx| this.stage_or_unstage_entries(true, to_stage, cx))?.await
        })
    }

    pub fn unstage_all(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let snapshot = self.snapshot.clone();
        let pending_ops = self.pending_ops.clone();
        let to_unstage = cx.background_spawn(async move {
            snapshot
                .status()
                .filter_map(|entry| {
                    if let Some(ops) = pending_ops
                        .get(&PathKey(Arc::clone(entry.repo_path.as_ref())), ())
                        .filter(|ops| !ops.last_op_errored())
                    {
                        if !ops.staging() && !ops.staged() { None } else { Some(entry.repo_path) }
                    } else if entry.status.staging().is_fully_unstaged() {
                        None
                    } else {
                        Some(entry.repo_path)
                    }
                })
                .collect()
        });

        cx.spawn(async move |this, cx| {
            let to_unstage = to_unstage.await;
            this.update(cx, |this, cx| this.stage_or_unstage_entries(false, to_unstage, cx))?.await
        })
    }

    pub fn stash_all(&mut self, message: Option<String>, cx: &mut Context<Self>) -> Task<Result<()>> {
        let to_stash = self.cached_status().map(|entry| entry.repo_path).collect();

        self.stash_entries(to_stash, message, cx)
    }

    pub fn stash_tracked(&mut self, message: Option<String>, cx: &mut Context<Self>) -> Task<Result<()>> {
        // `is_created` rather than `is_untracked`, so that staging a new file does not
        // move it out of the untracked set: `git add` makes it `Tracked { Added }`, but
        // the panel still lists it under "Untracked" and the user expects it left alone.
        let to_stash: Vec<_> =
            self.cached_status().filter(|entry| !entry.status.is_created()).map(|entry| entry.repo_path).collect();

        // An empty pathspec would make `git stash push` stash everything.
        if to_stash.is_empty() {
            return Task::ready(Ok(()));
        }
        self.stash_entries(to_stash, message, cx)
    }

    fn stash_entries(&self, entries: Vec<RepoPath>, message: Option<String>, cx: &Context<Self>) -> Task<Result<()>> {
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, _| {
                this.send_job(None, move |git_repo, _cx| async move {
                    {
                        let RepositoryState { backend, environment, .. } = git_repo;
                        backend.stash_paths(entries, message, environment).await
                    }
                })
            })?
            .await??;
            Ok(())
        })
    }

    pub fn stash_staged(&mut self, message: Option<String>, cx: &mut Context<Self>) -> Task<Result<()>> {
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, _| {
                this.send_job(None, move |git_repo, _cx| async move {
                    {
                        let RepositoryState { backend, environment, .. } = git_repo;
                        backend.stash_staged(message, environment).await
                    }
                })
            })?
            .await??;
            Ok(())
        })
    }

    pub fn stash_pop(&mut self, index: Option<usize>, cx: &mut Context<Self>) -> Task<Result<()>> {
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, _| {
                this.send_job(None, move |git_repo, _cx| async move {
                    {
                        let RepositoryState { backend, environment, .. } = git_repo;
                        backend.stash_pop(index, environment).await
                    }
                })
            })?
            .await??;
            Ok(())
        })
    }

    pub fn stash_apply(&mut self, index: Option<usize>, cx: &mut Context<Self>) -> Task<Result<()>> {
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, _| {
                this.send_job(None, move |git_repo, _cx| async move {
                    {
                        let RepositoryState { backend, environment, .. } = git_repo;
                        backend.stash_apply(index, environment).await
                    }
                })
            })?
            .await??;
            Ok(())
        })
    }

    pub fn add_path_to_gitignore(&mut self, repo_path: &RepoPath, is_dir: bool) -> oneshot::Receiver<Result<()>> {
        let work_dir = Arc::clone(&self.snapshot.work_directory_abs_path);
        let path_display = repo_path.as_ref().display();
        let file_path_str = if is_dir { format!("{path_display}/") } else { path_display.to_owned() };

        self.send_job(None, move |git_repo, _cx| async move {
            {
                let RepositoryState { fs, .. } = git_repo;
                append_pattern_to_ignore_file(fs, work_dir.join(".gitignore"), file_path_str).await
            }
        })
    }

    pub fn add_path_to_git_info_exclude(
        &mut self,
        repo_path: &RepoPath,
        is_dir: bool,
    ) -> oneshot::Receiver<Result<()>> {
        let repository_dir = Arc::clone(&self.snapshot.common_dir_abs_path);
        let path_display = repo_path.as_ref().display();
        let file_path_str = if is_dir { format!("{path_display}/") } else { path_display.to_owned() };

        self.send_job(None, move |git_repo, _cx| async move {
            {
                let RepositoryState { fs, .. } = git_repo;
                append_pattern_to_ignore_file(fs, repository_dir.join(git::REPO_EXCLUDE), file_path_str).await
            }
        })
    }

    pub fn stash_drop(&mut self, index: Option<usize>, cx: &mut Context<Self>) -> oneshot::Receiver<Result<()>> {
        let this = cx.weak_entity();
        self.send_job(None, move |git_repo, mut cx| async move {
            {
                let RepositoryState { backend, environment, .. } = git_repo;
                // TODO would be nice to not have to do this manually
                let result = backend.stash_drop(index, environment).await;
                if result.is_ok()
                    && let Ok(stash_entries) = backend.stash_entries().await
                {
                    this.update(&mut cx, |this, cx| {
                        this.snapshot.stash_entries = stash_entries;
                        cx.emit(RepositoryEvent::StashEntriesChanged);
                    })?;
                }

                result
            }
        })
    }

    pub fn commit(
        &mut self,
        message: SharedString,
        name_and_email: Option<(SharedString, SharedString)>,
        options: CommitOptions,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some("git commit".into()), move |git_repo, _cx| async move {
            {
                let RepositoryState { backend, environment, .. } = git_repo;
                backend.commit(message, name_and_email, options, environment).await
            }
        })
    }

    async fn refresh_branch_list(
        this: &WeakEntity<Self>,
        backend: Arc<dyn GitRepository>,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let branches_scan = backend.branches().await?;
        let branch_list_error = branches_scan.error;
        let branch_list: Arc<[Branch]> = branches_scan.branches.into();
        let branch = branch_list.iter().find(|branch| branch.is_head).cloned();
        log::info!("head branch after scan is {branch:?}");
        this.update(cx, |this, cx| {
            let head_changed = branch != this.snapshot.branch;
            let branch_list_changed = *branch_list != *this.snapshot.branch_list;
            let branch_list_error_changed = this.snapshot.branch_list_error != branch_list_error;
            this.snapshot.branch = branch;
            this.snapshot.branch_list = branch_list;
            this.snapshot.branch_list_error = branch_list_error;
            if head_changed {
                cx.emit(RepositoryEvent::HeadChanged);
            }
            if branch_list_changed || branch_list_error_changed {
                cx.emit(RepositoryEvent::BranchListChanged);
            }
        })?;
        Ok(())
    }

    pub const fn unshallow_state(&self) -> UnshallowState {
        self.unshallow_state
    }

    pub fn fetch_unshallow(&mut self, cx: &mut Context<Self>) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        let receiver = self.fetch(FetchOptions::Unshallow, cx);
        self.unshallow_state = UnshallowState::InProgress;
        cx.notify();
        let (tx, rx) = oneshot::channel();
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let succeeded = matches!(&result, Ok(Ok(_)));
            this.update(cx, |this, cx| {
                this.unshallow_state = if succeeded { UnshallowState::Unshallowed } else { UnshallowState::Idle };
                cx.notify();
            })
            .ok();
            if let Ok(result) = result {
                tx.send(result).ok();
            }
        })
        .detach();
        rx
    }

    pub fn fetch(
        &mut self,
        fetch_options: FetchOptions,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        let this = cx.weak_entity();
        self.send_job(Some("git fetch".into()), move |git_repo, mut cx| async move {
            {
                let RepositoryState { backend, environment, .. } = git_repo;
                let result = backend.fetch(fetch_options, environment, cx.clone()).await;
                if result.is_ok() {
                    Self::refresh_branch_list(&this, backend, &mut cx).await?;
                }
                result
            }
        })
    }

    pub fn push(
        &mut self,
        branch: SharedString,
        remote_branch: SharedString,
        remote: SharedString,
        options: Option<PushOptions>,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        let args = options.map_or("", |option| match option {
            PushOptions::SetUpstream => " --set-upstream",
            PushOptions::Force => " --force-with-lease",
        });

        let this = cx.weak_entity();
        self.send_job(
            Some(format!("git push {args} {remote} {branch}:{remote_branch}").into()),
            move |git_repo, mut cx| async move {
                {
                    let RepositoryState { backend, environment, .. } = git_repo;
                    let result = backend
                        .push(
                            branch.to_string(),
                            remote_branch.to_string(),
                            remote.to_string(),
                            options,
                            Arc::clone(&environment),
                            cx.clone(),
                        )
                        .await;
                    // TODO would be nice to not have to do this manually
                    if result.is_ok() {
                        Self::refresh_branch_list(&this, backend, &mut cx).await?;
                    }
                    result
                }
            },
        )
    }

    pub fn pull(
        &mut self,
        branch: Option<SharedString>,
        remote: SharedString,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        let mut status = "git pull --rebase --autostash".to_owned();
        util::write_str!(status, " {}", remote);
        if let Some(b) = &branch {
            util::write_str!(status, " {}", b);
        }

        self.send_job(Some(status.into()), move |git_repo, cx| async move {
            {
                let RepositoryState { backend, environment, .. } = git_repo;
                backend
                    .pull(branch.as_ref().map(ToString::to_string), remote.to_string(), Arc::clone(&environment), cx)
                    .await
            }
        })
    }

    pub fn rebase(&mut self, action: RebaseAction) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        self.send_job(Some(format!("git rebase {}", action.flag()).into()), move |git_repo, _| async move {
            {
                let RepositoryState { backend, environment, .. } = git_repo;
                backend.rebase(action, Arc::clone(&environment)).await
            }
        })
    }

    fn spawn_set_index_text_job(
        &mut self,
        path: RepoPath,
        content: Option<String>,
        hunk_staging_operation_count: Option<usize>,
        cx: &Context<Self>,
    ) -> oneshot::Receiver<Result<()>> {
        let this = cx.weak_entity();
        let git_store = self.git_store.clone();
        let abs_path = self.snapshot.repo_path_to_abs_path(&path);
        self.send_keyed_job(Some(GitJobKey::WriteIndex(vec![path.clone()])), None, move |git_repo, mut cx| async move {
            log::debug!("start updating index text for buffer {}", path.as_unix_str());

            {
                let RepositoryState { fs, backend, environment } = git_repo;
                let executable = match fs.metadata(&abs_path).await {
                    Ok(Some(meta)) => meta.is_executable,
                    Ok(None) => false,
                    Err(_err) => false,
                };
                let content = content.map(String::into_bytes);
                backend.set_index_text(path.clone(), content, Arc::clone(&environment), executable).await?;
            }
            log::debug!("finish updating index text for buffer {}", path.as_unix_str());

            if let Some(hunk_staging_operation_count) = hunk_staging_operation_count {
                let project_path =
                    this.read_with(&cx, |this, cx| this.repo_path_to_project_path(&path, cx)).ok().flatten();
                git_store
                    .update(&mut cx, |git_store, cx| {
                        let buffer_id =
                            git_store.buffer_store.read(cx).get_by_path(&project_path?)?.read(cx).buffer_id();
                        let diff_state = git_store.diffs.get(&buffer_id)?;
                        diff_state.update(cx, |diff_state, _| {
                            diff_state.hunk_staging_operation_count_as_of_write = hunk_staging_operation_count;
                        });
                        Some(())
                    })
                    .context("Git store dropped")?;
            }
            Ok(())
        })
    }

    pub fn create_remote(&mut self, remote_name: String, remote_url: String) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some(format!("git remote add {remote_name} {remote_url}").into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.create_remote(remote_name, remote_url).await
            }
        })
    }

    pub fn remove_remote(&mut self, remote_name: String) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some(format!("git remove remote {remote_name}").into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.remove_remote(remote_name).await
            }
        })
    }

    pub fn get_remotes(
        &mut self,
        branch_name: Option<String>,
        is_push: bool,
    ) -> oneshot::Receiver<Result<Vec<Remote>>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                let remote = if let Some(branch_name) = branch_name {
                    if is_push {
                        backend.get_push_remote(branch_name).await?
                    } else {
                        backend.get_branch_remote(branch_name).await?
                    }
                } else {
                    None
                };

                match remote {
                    Some(remote) => Ok(vec![remote]),
                    None => backend.get_all_remotes().await,
                }
            }
        })
    }

    pub fn remote_urls(&mut self) -> oneshot::Receiver<Result<FxHashMap<String, String>>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                Ok(backend.remote_urls().await)
            }
        })
    }

    pub fn branches(&mut self) -> oneshot::Receiver<Result<BranchesScanResult>> {
        self.send_job(None, move |repo, _| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.branches().await
            }
        })
    }

    /// If this is a linked worktree (*NOT* the main checkout of a repository),
    /// returns the path for the linked worktree.
    ///
    /// Returns None if this is the main checkout.
    pub fn linked_worktree_path(&self) -> Option<&Arc<Path>> {
        self.snapshot.is_linked_worktree().then_some(&self.work_directory_abs_path)
    }

    pub fn worktrees(&mut self) -> oneshot::Receiver<Result<Vec<GitWorktree>>> {
        self.send_job(None, move |repo, _| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.worktrees().await
            }
        })
    }

    pub fn create_worktree(&mut self, target: CreateWorktreeTarget, path: PathBuf) -> oneshot::Receiver<Result<()>> {
        let job_description = match target.branch_name() {
            Some(branch_name) => format!("git worktree add: {branch_name}"),
            None => "git worktree add (detached)".to_owned(),
        };
        self.send_job(Some(job_description.into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.create_worktree(target, path).await
            }
        })
    }

    /// Returns the creation time of a linked worktree's git metadata
    /// directory. See [`GitRepository::worktree_created_at`]. For remote
    /// projects the stat runs on the remote host, where the worktree's
    /// filesystem lives.
    pub fn worktree_created_at(&mut self, worktree_path: PathBuf) -> oneshot::Receiver<Result<Option<SystemTime>>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.worktree_created_at(worktree_path).await
            }
        })
    }

    pub fn checkout_branch_in_worktree(
        &mut self,
        branch_name: String,
        worktree_path: PathBuf,
        create: bool,
    ) -> oneshot::Receiver<Result<()>> {
        let description =
            if create { format!("git checkout -b {branch_name}") } else { format!("git checkout {branch_name}") };
        self.send_job(Some(description.into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.checkout_branch_in_worktree(branch_name, worktree_path, create).await
            }
        })
    }

    pub fn head_sha(&mut self) -> oneshot::Receiver<Result<Option<String>>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                Ok(backend.head_sha().await)
            }
        })
    }

    fn create_ref(&mut self, ref_name: String, commit: String) -> oneshot::Receiver<Result<()>> {
        let this = self.this.clone();
        self.send_job(Some(format!("git update-ref {ref_name} {commit}").into()), move |repo, mut cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.create_ref(ref_name, commit).await?;
            }

            this.update(&mut cx, |_, cx| {
                cx.emit(RepositoryEvent::TagsChanged);
            })?;
            Ok(())
        })
    }

    pub fn create_tag(&mut self, tag_name: String, commit: String) -> oneshot::Receiver<Result<()>> {
        self.create_ref(format!("refs/tags/{tag_name}"), commit)
    }

    fn edit_ref(&mut self, ref_name: String, commit: Option<String>) -> oneshot::Receiver<Result<()>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                match commit {
                    Some(commit) => backend.update_ref(ref_name, commit).await,
                    None => backend.delete_ref(ref_name).await,
                }
            }
        })
    }

    pub fn update_ref(&mut self, ref_name: String, commit: String) -> oneshot::Receiver<Result<()>> {
        self.edit_ref(ref_name, Some(commit))
    }

    pub fn delete_ref(&mut self, ref_name: String) -> oneshot::Receiver<Result<()>> {
        self.edit_ref(ref_name, None)
    }

    pub fn repair_worktrees(&mut self) -> oneshot::Receiver<Result<()>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.repair_worktrees().await
            }
        })
    }

    pub fn remove_worktree(&mut self, path: PathBuf, force: bool) -> oneshot::Receiver<Result<()>> {
        let repository_anchor_path: Arc<Path> = self.linked_worktree_anchor_path().into();
        self.send_job(Some(format!("git worktree remove: {}", path.display()).into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, fs, .. } = repo;
                // When forcing, delete the worktree directory ourselves before
                // invoking git. `git worktree remove` can remove the admin
                // metadata in `.git/worktrees/<name>` but fail to delete the
                // working directory (it continues past directory-removal errors),
                // leaving an orphaned folder on disk. Deleting first guarantees
                // the directory is gone, and `git worktree remove --force`
                // tolerates a missing working tree while cleaning up the admin
                // entry. We keep this inside the `Local` arm so that for remote
                // projects the deletion runs on the remote machine (where the
                // `GitRemoveWorktree` RPC is handled against the local repo on
                // the headless server) using its own filesystem.
                //
                // After a successful removal, also delete any empty ancestor
                // directories between the worktree path and the configured
                // base directory used when creating linked worktrees.
                //
                // Non-force removals are left untouched before git runs:
                // `git worktree remove` must see the dirty working tree to
                // refuse the operation.
                if force {
                    fs.remove_dir(&path, RemoveOptions { recursive: true, ignore_if_not_exists: true })
                        .await
                        .with_context(|| format!("failed to delete worktree directory '{}'", path.display()))?;
                }

                backend.remove_worktree(path.clone(), force).await?;

                let managed_worktree_base = worktrees_directory_for_repo(&repository_anchor_path).log_err();

                if let Some(managed_worktree_base) = managed_worktree_base {
                    remove_empty_managed_worktree_ancestors(fs.as_ref(), &path, &managed_worktree_base).await;
                }
            }
            Ok(())
        })
    }

    pub fn rename_worktree(&mut self, old_path: PathBuf, new_path: PathBuf) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some(format!("git worktree move: {}", old_path.display()).into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.rename_worktree(old_path, new_path).await
            }
        })
    }

    pub fn default_branch(&mut self, include_remote_name: bool) -> oneshot::Receiver<Result<Option<SharedString>>> {
        self.send_job(None, move |repo, _| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.default_branch(include_remote_name).await
            }
        })
    }

    fn diff_tree(&mut self, diff_type: DiffTreeType, _cx: &App) -> oneshot::Receiver<Result<TreeDiff>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.diff_tree(diff_type).await
            }
        })
    }

    pub fn diff(&mut self, diff_type: DiffType, _cx: &App) -> oneshot::Receiver<Result<String>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.diff(diff_type).await
            }
        })
    }

    pub fn create_branch(&mut self, branch_name: String, base_branch: Option<String>) -> oneshot::Receiver<Result<()>> {
        let status_msg = if let Some(ref base) = base_branch {
            format!("git switch -c {branch_name} {base}").into()
        } else {
            format!("git switch -c {branch_name}").into()
        };
        self.send_job(Some(status_msg), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.create_branch(branch_name, base_branch).await
            }
        })
    }

    pub fn change_branch(&mut self, branch_name: String) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some(format!("git switch {branch_name}").into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.change_branch(branch_name).await
            }
        })
    }

    pub fn delete_branch(
        &mut self,
        is_remote: bool,
        branch_name: String,
        force: bool,
    ) -> oneshot::Receiver<Result<()>> {
        let flag = delete_branch_flag(is_remote, force);
        self.send_job(Some(format!("git branch {flag} {branch_name}").into()), move |repo, _cx| async move {
            repo.backend.delete_branch(is_remote, branch_name, force).await
        })
    }

    pub fn rename_branch(&mut self, branch: String, new_name: String) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some(format!("git branch -m {branch} {new_name}").into()), move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.rename_branch(branch, new_name).await
            }
        })
    }

    pub fn check_for_pushed_commits(&mut self) -> oneshot::Receiver<Result<Vec<SharedString>>> {
        self.send_job(None, move |repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = repo;
                backend.check_for_pushed_commit().await
            }
        })
    }

    fn clear_pending_ops(&mut self, cx: &mut Context<Self>) {
        let updated = SumTree::from_iter(
            self.pending_ops.iter().filter_map(|ops| {
                let inner_ops: Vec<PendingOp> = ops.ops.iter().filter(|op| op.running()).copied().collect();
                if inner_ops.is_empty() {
                    None
                } else {
                    Some(PendingOps { repo_path: ops.repo_path.clone(), ops: inner_ops })
                }
            }),
            (),
        );

        if updated != self.pending_ops {
            cx.emit(RepositoryEvent::PendingOpsChanged { pending_ops: self.pending_ops.clone() });
        }

        self.pending_ops = updated;
    }

    fn schedule_scan(&mut self, cx: &Context<Self>) {
        let this = cx.weak_entity();
        drop(self.send_keyed_job(Some(GitJobKey::ReloadGitState), None, |state, mut cx| async move {
            log::debug!("run scheduled git status scan");

            let Some(this) = this.upgrade() else {
                return anyhow::Ok(());
            };
            let RepositoryState { backend, .. } = state;
            compute_snapshot(this.clone(), Arc::clone(&backend), &mut cx).await;
            this.update(&mut cx, |this, cx| {
                this.clear_pending_ops(cx);
            });
            Ok(())
        }));
    }

    fn spawn_local_git_worker(
        state: Shared<Task<Result<RepositoryState, String>>>,
        cx: &Context<Self>,
    ) -> (mpsc::UnboundedSender<GitJob>, Task<()>) {
        let (job_tx, mut job_rx) = mpsc::unbounded::<GitJob>();

        let worker_task = cx.spawn(async move |_, cx| {
            let Some(state) = state.await.log_err() else {
                return;
            };
            let state = state;
            let mut jobs = VecDeque::new();
            loop {
                while let Ok(next_job) = job_rx.try_recv() {
                    jobs.push_back(next_job);
                }

                if let Some(job) = jobs.pop_front() {
                    if let Some(current_key) = &job.key
                        && jobs.iter().any(|other_job| other_job.key.as_ref() == Some(current_key))
                    {
                        continue;
                    }
                    (job.job)(state.clone(), cx).await;
                } else if let Some(job) = job_rx.next().await {
                    jobs.push_back(job);
                } else {
                    break;
                }
            }
        });

        (job_tx, worker_task)
    }

    fn load_staged_text(&mut self, repo_path: RepoPath, cx: &App) -> Task<Result<Option<String>>> {
        let (result_tx, result_rx) = oneshot::channel();
        self.queue_diff_base_load(DiffBaseLoad { repo_path, kind: DiffBaseLoadKind::Staged(result_tx) });
        cx.spawn(|_: &mut AsyncApp| async move { result_rx.await? })
    }

    fn load_committed_text(&mut self, repo_path: RepoPath, cx: &App) -> Task<Result<DiffBasesChange>> {
        let (result_tx, result_rx) = oneshot::channel();
        self.queue_diff_base_load(DiffBaseLoad { repo_path, kind: DiffBaseLoadKind::Committed(result_tx) });
        cx.spawn(|_: &mut AsyncApp| async move { result_rx.await? })
    }

    fn queue_diff_base_load(&mut self, load: DiffBaseLoad) {
        // Joining only the most recently queued job keeps loads ordered with other git jobs.
        let (next_job_id, loads) = &self.pending_diff_base_loads;
        if *next_job_id == self.job_id
            && let Some(loads) = loads.upgrade()
        {
            loads.borrow_mut().push(load);
            return;
        }
        // The job owns the only `Rc`, so its loads are canceled if it's dropped without running,
        // and no load can join once it starts.
        let loads = Rc::new(RefCell::new(vec![load]));
        let pending_loads = Rc::downgrade(&loads);
        drop(self.send_job(None, move |state, cx| async move {
            let loads = Rc::into_inner(loads).map_or_default(RefCell::into_inner);
            let RepositoryState { backend, .. } = state;
            // A batch can hold many texts, so decode them off the main thread.
            cx.background_spawn(async move {
                let groups = loads.iter().map(DiffBaseLoad::revisions).collect();
                let results = load_revision_groups(|revisions| backend.load_revisions(revisions), groups).await;
                for (load, texts) in loads.into_iter().zip(results) {
                    load.finish(texts);
                }
            })
            .await;
        }));
        self.pending_diff_base_loads = (self.job_id, pending_loads);
    }

    pub fn load_commit_template_text(&mut self) -> oneshot::Receiver<Result<Option<GitCommitTemplate>>> {
        self.send_job(None, move |git_repo, _cx| async move {
            {
                let RepositoryState { backend, .. } = git_repo;
                backend.load_commit_template().await
            }
        })
    }

    pub fn blame_buffer_at_revision(
        &mut self,
        path: RepoPath,
        revision: Oid,
        cx: &App,
    ) -> Task<Result<(String, Blame)>> {
        let rx = self.send_job(None, {
            let path = path.clone();
            move |state, _| async move {
                {
                    let RepositoryState { backend, .. } = state;
                    let content_task = backend.load_revisions(vec![format!("{revision}:{}", path.as_unix_str())]);
                    let blame_task = backend.blame_at_revision(path.clone(), revision);
                    let (mut contents, blame) = futures::try_join!(content_task, blame_task)?;
                    let content = contents.pop().flatten().with_context(|| {
                        format!("cannot load {:?} at revision {revision}: the file is missing or binary", path.as_ref())
                    })?;
                    let content = decode_git_text(content)?;
                    anyhow::Ok((content, blame))
                }
            }
        });
        cx.background_spawn(async move {
            let (content, blame) = rx.await??;
            anyhow::ensure!(
                !is_binary_content(content.as_bytes()),
                "cannot blame binary file {:?} at revision {revision}",
                path.as_ref()
            );
            Ok((content, blame))
        })
    }

    /// Bypasses the serial git job queue: blobs are content-addressed, so this read
    /// doesn't depend on the status snapshot, and a diff issues one per changed file.
    fn load_blob_content(&self, oid: Oid, cx: &App) -> Task<Result<String>> {
        let repository_state = self.repository_state.clone();
        let object_read_limiter = Arc::clone(&self.object_read_limiter);
        cx.background_spawn(async move {
            let _permit = object_read_limiter.acquire_arc().await;
            let RepositoryState { backend, .. } = repository_state.await.map_err(|err| anyhow::anyhow!(err))?;
            decode_git_text(backend.load_blob_content(oid).await?)
        })
    }

    fn paths_changed(&mut self, paths: Vec<RepoPath>, cx: &Context<Self>) {
        if !paths.is_empty() {
            self.paths_needing_status_update.push(paths);
        }

        let this = cx.weak_entity();
        drop(self.send_keyed_job(Some(GitJobKey::RefreshStatuses), None, |state, mut cx| async move {
            let (prev_snapshot, changed_paths) = this
                .update(&mut cx, |this, _| (this.snapshot.clone(), mem::take(&mut this.paths_needing_status_update)))?;
            let RepositoryState { backend, .. } = state;

            if changed_paths.is_empty() {
                return Ok(());
            }

            let has_head = prev_snapshot.head_commit.is_some();

            let changed_path_statuses = cx
                .background_spawn(async move {
                    let changed_paths = GitStore::limit_repo_paths(GitStore::coalesce_repo_paths(
                        changed_paths.into_iter().flatten().collect::<BTreeSet<_>>().into_iter().collect(),
                    ));
                    let changed_paths_vec = changed_paths.clone();

                    let status_task = backend.status(&changed_paths_vec);
                    let diff_stat_future = |diff| {
                        if has_head {
                            backend.diff_stat(diff, &changed_paths_vec)
                        } else {
                            future::ready(Ok(status::GitDiffStat::default())).boxed()
                        }
                    };

                    let (statuses, diff_stats, staged_diff_stats, unstaged_diff_stats) = future::try_join4(
                        status_task,
                        diff_stat_future(DiffStatType::HeadToWorktree),
                        diff_stat_future(DiffStatType::HeadToIndex),
                        diff_stat_future(DiffStatType::IndexToWorktree),
                    )
                    .await?;

                    let diff_stats: FxHashMap<RepoPath, DiffStat> =
                        FxHashMap::from_iter(diff_stats.entries.iter().cloned());
                    let staged_diff_stats: FxHashMap<RepoPath, DiffStat> =
                        FxHashMap::from_iter(staged_diff_stats.entries.iter().cloned());
                    let unstaged_diff_stats: FxHashMap<RepoPath, DiffStat> =
                        FxHashMap::from_iter(unstaged_diff_stats.entries.iter().cloned());

                    let mut changed_path_statuses = Vec::new();
                    let prev_statuses = prev_snapshot.statuses_by_path.clone();
                    let current_status_paths =
                        statuses.entries.iter().map(|(repo_path, _)| repo_path.clone()).collect::<BTreeSet<_>>();

                    for path in &changed_paths {
                        let mut cursor = prev_statuses.cursor::<PathProgress>(());
                        cursor.seek_forward(&PathTarget::Path(path), Bias::Left);
                        while let Some(entry) = cursor.item() {
                            if !entry.repo_path.starts_with(path) {
                                break;
                            }

                            if !current_status_paths.contains(&entry.repo_path) {
                                changed_path_statuses.push(Edit::Remove(PathKey(Arc::clone(entry.repo_path.as_ref()))));
                            }
                            cursor.next();
                        }
                    }

                    let mut cursor = prev_statuses.cursor::<PathProgress>(());

                    for (repo_path, status) in &*statuses.entries {
                        let current_diff_stat = diff_stats.get(repo_path).copied();
                        let current_staged_diff_stat = staged_diff_stats.get(repo_path).copied();
                        let current_unstaged_diff_stat = unstaged_diff_stats.get(repo_path).copied();

                        if cursor.seek_forward(&PathTarget::Path(repo_path), Bias::Left)
                            && cursor.item().is_some_and(|entry| {
                                entry.status == *status
                                    && entry.diff_stat == current_diff_stat
                                    && entry.staged_diff_stat == current_staged_diff_stat
                                    && entry.unstaged_diff_stat == current_unstaged_diff_stat
                            })
                        {
                            continue;
                        }

                        changed_path_statuses.push(Edit::Insert(StatusEntry {
                            repo_path: repo_path.clone(),
                            status: *status,
                            diff_stat: current_diff_stat,
                            staged_diff_stat: current_staged_diff_stat,
                            unstaged_diff_stat: current_unstaged_diff_stat,
                        }));
                    }
                    anyhow::Ok(changed_path_statuses)
                })
                .await?;

            this.update(&mut cx, |this, cx| {
                if !changed_path_statuses.is_empty() {
                    cx.emit(RepositoryEvent::StatusesChanged);
                    this.snapshot.statuses_by_path.edit(changed_path_statuses, ());
                    this.snapshot.scan_id += 1;
                }
            })
        }));
    }

    /// currently running git command and when it started.
    pub fn current_job(&self) -> Option<JobInfo> {
        self.active_jobs.values().next().cloned()
    }

    pub fn barrier(&mut self) -> oneshot::Receiver<()> {
        self.send_job(None, |_, _| async {})
    }

    fn spawn_job_with_tracking<AsyncFn>(
        &mut self,
        paths: Vec<RepoPath>,
        git_status: pending_op::GitStatus,
        cx: &Context<Self>,
        f: AsyncFn,
    ) -> Task<Result<()>>
    where
        AsyncFn: AsyncFnOnce(WeakEntity<Repository>, &mut AsyncApp) -> Result<()> + 'static,
    {
        let ids = self.new_pending_ops_for_paths(paths, git_status);

        cx.spawn(async move |this, cx| {
            let (job_status, result) = match f(this.clone(), cx).await {
                Ok(()) => (pending_op::JobStatus::Finished, Ok(())),
                Err(err) if err.is::<Canceled>() => (pending_op::JobStatus::Skipped, Ok(())),
                Err(err) => (pending_op::JobStatus::Error, Err(err)),
            };

            this.update(cx, |this, _| {
                let mut edits = Vec::with_capacity(ids.len());
                for (id, entry) in ids {
                    if let Some(mut ops) = this.pending_ops.get(&PathKey(Arc::clone(entry.as_ref())), ()).cloned() {
                        if let Some(op) = ops.op_by_id_mut(id) {
                            op.job_status = job_status;
                        }
                        edits.push(Edit::Insert(ops));
                    }
                }
                this.pending_ops.edit(edits, ());
            })?;

            result
        })
    }

    fn new_pending_ops_for_paths(
        &mut self,
        paths: Vec<RepoPath>,
        git_status: pending_op::GitStatus,
    ) -> Vec<(PendingOpId, RepoPath)> {
        let mut edits = Vec::with_capacity(paths.len());
        let mut ids = Vec::with_capacity(paths.len());
        for path in paths {
            let mut ops = self
                .pending_ops
                .get(&PathKey(Arc::clone(path.as_ref())), ())
                .cloned()
                .unwrap_or_else(|| PendingOps::new(&path));
            let id = ops.max_id() + 1;
            ops.ops.push(PendingOp { id, git_status, job_status: pending_op::JobStatus::Running });
            edits.push(Edit::Insert(ops));
            ids.push((id, path));
        }
        self.pending_ops.edit(edits, ());
        ids
    }

    pub fn access(&mut self, _cx: &App) -> oneshot::Receiver<GitAccess> {
        self.send_job(None, move |git_repo, _cx| async move {
            match git_repo.backend.check_access().await {
                Ok(()) => GitAccess::Yes,
                Err(_) => GitAccess::No,
            }
        })
    }
}

/// If `path` is a git linked worktree checkout, resolves it to the main
/// repository's identity path. For regular linked worktrees this is the main
/// repository's working directory; for linked worktrees backed by a bare repo
/// such as `.bare`, this is the parent project directory users think of as the
/// repository root. Returns `None` if `path` is a normal repository, not a git
/// repo, or if resolution fails.
///
/// Resolution works by:
/// 1. Reading the `.git` file to get the `gitdir:` pointer
/// 2. Following that to the worktree-specific git directory
/// 3. Reading the `commondir` file to find the shared `.git` directory
/// 4. Deriving the main repo's identity path from the common dir
pub async fn resolve_git_worktree_to_main_repo(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let dot_git = path.join(".git");
    let metadata = fs.metadata(&dot_git).await.ok()??;
    if metadata.is_dir {
        return None; // Normal repo, not a linked worktree
    }
    // It's a .git file — parse the gitdir: pointer
    let content = fs.load_regular_file(&dot_git).await.ok()?;
    let gitdir_rel = content.strip_prefix("gitdir:")?.trim();
    let gitdir_abs = fs.canonicalize(&path.join(gitdir_rel)).await.ok()?;
    // Submodules also use a `.git` file, but they are independent projects whose
    // identity is their own working directory (`path`), not the superproject's
    // `.git/modules/<name>` git dir. Leave them unresolved.
    if is_submodule_git_dir(&gitdir_abs) {
        return None;
    }
    // Read commondir to find the main .git directory
    let commondir_content = fs.load_regular_file(&gitdir_abs.join("commondir")).await.ok()?;
    let common_dir = fs.canonicalize(&gitdir_abs.join(commondir_content.trim())).await.ok()?;
    Some(repo_identity_path(&common_dir).to_path_buf())
}

/// Directory holding new linked worktrees of a repository: `<parent>/worktrees/<repo name>`,
/// a sibling of the repository, scoped by its name so sibling repos don't collide.
fn worktrees_directory_for_repo(repository_anchor_path: &Path) -> Result<PathBuf> {
    let parent = repository_anchor_path.parent().ok_or_else(|| anyhow!("git repo must have a parent directory"))?;
    let repo_name = repository_anchor_path.file_name().ok_or_else(|| anyhow!("git repo must have a directory name"))?;
    Ok(parent.join("worktrees").join(repo_name))
}

async fn remove_empty_managed_worktree_ancestors(fs: &Fs, child_path: &Path, base_path: &Path) {
    let mut current = child_path;
    while let Some(parent) = current.parent() {
        if parent == base_path {
            break;
        }
        if !parent.starts_with(base_path) {
            break;
        }

        let result = fs.remove_dir(parent, RemoveOptions { recursive: false, ignore_if_not_exists: true }).await;

        match result {
            Ok(()) => {
                log::info!("Removed empty managed worktree directory: {}", parent.display());
            }
            Err(error) => {
                log::debug!("Stopped removing managed worktree parent directories at {}: {error}", parent.display());
                break;
            }
        }

        current = parent;
    }
}

/// Returns the repository's identity path given its common Git directory.
///
/// This is the canonical, on-disk path used for project grouping and as the
/// basis for display names. The goal is to return the directory the user
/// thinks of as "the project":
///
/// - If `common_dir`'s last component starts with `.` (e.g. `.git` for a
///   normal checkout, or `.bare` for a bare clone), the parent directory is
///   returned. Both of these are internal Git directories; the parent is the
///   meaningful project root.
/// - Otherwise (e.g. `zed.git` for a bare clone), `common_dir` itself is
///   returned — it is already a meaningful on-disk path.
pub fn repo_identity_path(common_dir: &Path) -> &Path {
    let is_dot_entry = common_dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
    if is_dot_entry { common_dir.parent().unwrap_or(common_dir) } else { common_dir }
}

/// Returns true if `git_dir` is a Git submodule's git directory.
///
/// Submodules store their git directory inside the superproject at
/// `<superproject>/.git/modules/<name>`. Unlike a linked worktree, a submodule
/// is an independent project whose identity is its own working directory, so its
/// path must not be resolved to the superproject's `.git/modules/...` directory
/// (from which the working directory cannot be derived).
pub(crate) fn is_submodule_git_dir(git_dir: &Path) -> bool {
    let mut previous_was_git_dir = false;
    for component in git_dir.components() {
        if let Component::Normal(name) = component {
            if previous_was_git_dir && name == OsStr::new("modules") {
                return true;
            }
            previous_was_git_dir = name.to_string_lossy().ends_with(".git");
        } else {
            previous_was_git_dir = false;
        }
    }
    false
}

/// Returns a short name for a linked worktree suitable for UI display.
///
/// Uses the main worktree path to come up with a short name that disambiguates
/// the linked worktree from the main worktree.
pub fn linked_worktree_short_name(main_worktree_path: &Path, linked_worktree_path: &Path) -> Option<SharedString> {
    if main_worktree_path == linked_worktree_path {
        return None;
    }

    let project_name = main_worktree_path.file_name()?.to_str()?;
    let directory_name = linked_worktree_path.file_name()?.to_str()?;
    let name = if directory_name == project_name {
        let parent = linked_worktree_path.parent()?;
        parent.file_name()?.to_str()?.to_owned()
    } else {
        directory_name.to_owned()
    };
    Some(name.into())
}

async fn append_pattern_to_ignore_file(fs: Arc<Fs>, file_path: PathBuf, pattern: String) -> Result<()> {
    let existing_content = match fs.load(&file_path).await {
        Ok(content) => content,
        Err(error)
            if error
                .root_cause()
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == ErrorKind::NotFound) =>
        {
            String::new()
        }
        Err(error) => {
            return Err(error).with_context(|| format!("loading {}", file_path.display()));
        }
    };

    if existing_content.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }

    let new_content = if existing_content.is_empty() {
        format!("{pattern}\n")
    } else if existing_content.ends_with('\n') {
        format!("{existing_content}{pattern}\n")
    } else {
        format!("{existing_content}\n{pattern}\n")
    };

    fs.save(&file_path, &text::Rope::from(new_content.as_str()), text::LineEnding::Unix).await
}

/// This snapshot computes the repository state on the foreground thread while
/// running the git commands on the background thread. We update branch, head,
/// remotes, and worktrees first so the UI can react sooner, then compute file
/// state and emit those events immediately after.
async fn compute_snapshot(
    this: Entity<Repository>,
    backend: Arc<dyn GitRepository>,
    cx: &mut AsyncApp,
) -> RepositorySnapshot {
    log::debug!("starting compute snapshot");

    let (id, work_directory_abs_path, prev_snapshot) = this.update(cx, |this, _| {
        this.paths_needing_status_update.clear();
        (this.id, Arc::clone(&this.work_directory_abs_path), this.snapshot.clone())
    });

    let branches_future = {
        let backend = Arc::clone(&backend);
        async move { backend.branches().await.log_err().unwrap_or_default() }
    };
    let head_commit_future = {
        let backend = Arc::clone(&backend);
        async move { backend.show("HEAD".to_owned()).await.ok() }
    };
    let worktrees_future = {
        let backend = Arc::clone(&backend);
        async move { backend.worktrees().await.log_err().unwrap_or_default() }
    };
    let (branches, head_commit, all_worktrees) =
        future::join3(branches_future, head_commit_future, worktrees_future).await;
    log::debug!("fetched branches, head commit, worktrees");

    let BranchesScanResult { branches, error: branch_list_error } = branches;
    let branch = branches.iter().find(|branch| branch.is_head).cloned();
    let branch_list: Arc<[Branch]> = branches.into();

    let linked_worktrees: Arc<[GitWorktree]> =
        all_worktrees.into_iter().filter(|wt| wt.path != *work_directory_abs_path).collect();

    let mut remote_urls = backend.remote_urls().await;
    let remote_origin_url = remote_urls.remove("origin");
    let remote_upstream_url = remote_urls.remove("upstream");

    log::debug!("fetched remotes");

    let snapshot = this.update(cx, |this, cx| {
        let head_changed = branch != this.snapshot.branch || head_commit != this.snapshot.head_commit;
        let branch_list_changed = *branch_list != *this.snapshot.branch_list;
        let branch_list_error_changed = branch_list_error != this.snapshot.branch_list_error;
        let worktrees_changed = *linked_worktrees != *this.snapshot.linked_worktrees;

        this.snapshot = RepositorySnapshot {
            id,
            work_directory_abs_path,
            branch,
            branch_list: Arc::clone(&branch_list),
            branch_list_error,
            head_commit,
            remote_origin_url,
            remote_upstream_url,
            linked_worktrees,
            scan_id: prev_snapshot.scan_id + 1,
            ..prev_snapshot
        };

        if head_changed {
            cx.emit(RepositoryEvent::HeadChanged);
        }

        if branch_list_changed || branch_list_error_changed {
            cx.emit(RepositoryEvent::BranchListChanged);
        }

        if worktrees_changed {
            cx.emit(RepositoryEvent::GitWorktreeListChanged);
        }

        this.snapshot.clone()
    });

    let statuses_future = {
        let backend = Arc::clone(&backend);
        async move {
            backend
                .status(&[RepoPath::from_rel_path(&RelPath::new(".".as_ref()).expect("'.' is a valid relative path"))])
                .await
                .log_err()
                .unwrap_or_default()
        }
    };
    let diff_stats_future = {
        let snapshot = snapshot.clone();
        let backend = Arc::clone(&backend);
        async move {
            if snapshot.head_commit.is_some() {
                future::join3(
                    backend.diff_stat(DiffStatType::HeadToWorktree, &[]),
                    backend.diff_stat(DiffStatType::HeadToIndex, &[]),
                    backend.diff_stat(DiffStatType::IndexToWorktree, &[]),
                )
                .await
            } else {
                (
                    Ok(status::GitDiffStat::default()),
                    Ok(status::GitDiffStat::default()),
                    Ok(status::GitDiffStat::default()),
                )
            }
        }
    };
    let stash_entries_future = {
        let backend = Arc::clone(&backend);
        async move { backend.stash_entries().await.log_err().unwrap_or_default() }
    };

    let (statuses, diff_stats, stash_entries) =
        future::join3(statuses_future, diff_stats_future, stash_entries_future).await;
    let (diff_stats, staged_diff_stats, unstaged_diff_stats) = diff_stats;
    let diff_stats = diff_stats.log_err().unwrap_or_default();
    let staged_diff_stats = staged_diff_stats.log_err().unwrap_or_default();
    let unstaged_diff_stats = unstaged_diff_stats.log_err().unwrap_or_default();
    log::debug!("fetched statuses, diff stats, stash entries");

    let diff_stat_map: FxHashMap<&RepoPath, DiffStat> = diff_stats.entries.iter().map(|(p, s)| (p, *s)).collect();
    let staged_diff_stat_map: FxHashMap<&RepoPath, DiffStat> =
        staged_diff_stats.entries.iter().map(|(p, s)| (p, *s)).collect();
    let unstaged_diff_stat_map: FxHashMap<&RepoPath, DiffStat> =
        unstaged_diff_stats.entries.iter().map(|(p, s)| (p, *s)).collect();
    let mut conflicted_paths = Vec::new();
    let statuses_by_path = SumTree::from_iter(
        statuses.entries.iter().map(|(repo_path, status)| {
            if status.is_conflicted() {
                conflicted_paths.push(repo_path.clone());
            }
            StatusEntry {
                repo_path: repo_path.clone(),
                status: *status,
                diff_stat: diff_stat_map.get(repo_path).copied(),
                staged_diff_stat: staged_diff_stat_map.get(repo_path).copied(),
                unstaged_diff_stat: unstaged_diff_stat_map.get(repo_path).copied(),
            }
        }),
        (),
    );

    let (merge_details, conflicts_changed) = cx
        .background_spawn({
            let backend = Arc::clone(&backend);
            let mut merge_details = snapshot.merge.clone();
            let repository_dir = Arc::clone(&snapshot.repository_dir_abs_path);
            async move {
                let conflicts_changed = merge_details.update(&backend, &repository_dir, conflicted_paths).await;
                (merge_details, conflicts_changed)
            }
        })
        .await;
    log::debug!("new merge details: {merge_details:?}");

    this.update(cx, |this, cx| {
        if conflicts_changed || statuses_by_path != this.snapshot.statuses_by_path {
            cx.emit(RepositoryEvent::StatusesChanged);
        }
        if stash_entries != this.snapshot.stash_entries {
            cx.emit(RepositoryEvent::StashEntriesChanged);
        }

        this.snapshot.scan_id += 1;
        this.snapshot.merge = merge_details;
        this.snapshot.statuses_by_path = statuses_by_path;
        this.snapshot.stash_entries = stash_entries;

        this.snapshot.clone()
    })
}

/// Whether one path contains the other: a folder inside a repository, or a
/// repository nested in a folder.
fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Whether `path` is below `ancestor` and not `ancestor` itself.
fn is_strictly_inside(path: &Path, ancestor: &Path) -> bool {
    path != ancestor && path.starts_with(ancestor)
}

/// The candidate with the longest path that contains `path`.
fn innermost_containing<Id, P: AsRef<Path>>(path: &Path, candidates: impl Iterator<Item = (Id, P)>) -> Option<Id> {
    candidates
        .filter(|(_, candidate)| path.starts_with(candidate.as_ref()))
        .max_by_key(|(_, candidate)| candidate.as_ref().as_os_str().len())
        .map(|(id, _)| id)
}

#[cfg(test)]
mod rebase_in_progress_tests {
    use super::rebase_in_progress;

    #[test]
    fn rebase_in_progress_works() {
        let dir = std::env::temp_dir().join(format!("rebase-in-progress-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!rebase_in_progress(&dir));
        // A stale REBASE_HEAD alone doesn't count.
        std::fs::write(dir.join("REBASE_HEAD"), "abc").unwrap();
        assert!(!rebase_in_progress(&dir));
        std::fs::create_dir_all(dir.join("rebase-merge")).unwrap();
        assert!(rebase_in_progress(&dir));
        std::fs::remove_dir(dir.join("rebase-merge")).unwrap();
        std::fs::create_dir_all(dir.join("rebase-apply")).unwrap();
        assert!(rebase_in_progress(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod active_folder_tests {
    use std::path::Path;

    use super::innermost_containing;
    use super::is_strictly_inside;
    use super::paths_overlap;
    use super::worktrees_directory_for_repo;

    #[test]
    fn paths_overlap_works() {
        assert!(paths_overlap(Path::new("/r"), Path::new("/r")));
        assert!(paths_overlap(Path::new("/r/sub"), Path::new("/r")));
        assert!(paths_overlap(Path::new("/r"), Path::new("/r/vendor/lib")));
        assert!(!paths_overlap(Path::new("/r"), Path::new("/other")));
        assert!(!paths_overlap(Path::new("/r"), Path::new("/rr")));
    }

    #[test]
    fn is_strictly_inside_works() {
        assert!(is_strictly_inside(Path::new("/r/vendor/lib"), Path::new("/r")));
        assert!(!is_strictly_inside(Path::new("/r"), Path::new("/r")));
        assert!(!is_strictly_inside(Path::new("/r"), Path::new("/r/sub")));
        assert!(!is_strictly_inside(Path::new("/rr"), Path::new("/r")));
    }

    #[test]
    fn worktrees_directory_for_repo_works() {
        assert_eq!(worktrees_directory_for_repo(Path::new("/code/ed")).unwrap(), Path::new("/code/worktrees/ed"));
        worktrees_directory_for_repo(Path::new("/")).unwrap_err();
    }

    #[test]
    fn innermost_containing_works() {
        let repos = [(1, "/r"), (2, "/r/vendor/lib"), (3, "/other")];
        let find = |path: &str| innermost_containing(Path::new(path), repos.iter().map(|(id, p)| (*id, Path::new(p))));
        assert_eq!(find("/r"), Some(1));
        assert_eq!(find("/r/src"), Some(1));
        assert_eq!(find("/r/vendor/lib/x"), Some(2));
        assert_eq!(find("/scratch"), None);
    }
}

#[cfg(test)]
mod tests {
    use git::repository::RepoPath;

    use super::GitStore;
    use super::MAX_SCOPED_STATUS_PATHS;

    fn repo_paths(count: usize) -> Vec<RepoPath> {
        (0..count).map(|index| RepoPath::new(&format!("dir{index}/file.rs")).unwrap()).collect()
    }

    #[test]
    fn limit_repo_paths_falls_back_to_root_above_threshold() {
        let at_limit = repo_paths(MAX_SCOPED_STATUS_PATHS);
        assert_eq!(GitStore::limit_repo_paths(at_limit.clone()), at_limit);
        assert_eq!(GitStore::limit_repo_paths(repo_paths(MAX_SCOPED_STATUS_PATHS + 1)), [RepoPath::new("").unwrap()]);
    }
}
