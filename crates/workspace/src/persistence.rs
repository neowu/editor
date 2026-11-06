use std::fs as std_fs;
use std::time::Duration;

use futures::future;
use futures::lock;
use project::git_store;
pub mod model;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use fs::Fs;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::Global;
use gpui::Task;
use parking_lot::Mutex;
use project::Project;
use remote::RemoteConnectionIdentity;
use remote::RemoteConnectionOptions;
use remote::SshConnectionOptions;
use remote::remote_connection_identity;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;
use util::ResultExt;
use util::rel_path::RelPath;

use self::model::SerializedWorkspace;
use self::model::SerializedWorkspaceLocation;
use self::model::SessionWorkspace;
use crate::Workspace;
use crate::path_list::PathList;

/// Name of the global file, under the data dir, that lists recent projects,
/// marking the ones open when ed last quit.
const PROJECTS_FILE_NAME: &str = "projects.json";
/// Project-level folder holding per-project state.
pub const WORKSPACE_STATE_DIR: &str = ".ed";
const WORKSPACE_STATE_FILE: &str = "workspace.json";
const MAX_RECENT_PROJECTS: usize = 100;

/// Where a project lives: its root folders, in the order they were opened, and
/// the server holding them for a remote project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLocation {
    pub paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteServer {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl RemoteServer {
    fn from_options(options: &RemoteConnectionOptions) -> RemoteServer {
        let RemoteConnectionIdentity::Ssh { host, username, port } = remote_connection_identity(options);
        RemoteServer { host, username, port }
    }

    fn to_options(&self) -> RemoteConnectionOptions {
        RemoteConnectionOptions::Ssh(SshConnectionOptions {
            host: self.host.clone().into(),
            username: self.username.clone(),
            port: self.port,
            ..Default::default()
        })
    }
}

impl ProjectLocation {
    pub fn new(location: &SerializedWorkspaceLocation, paths: &PathList) -> ProjectLocation {
        ProjectLocation {
            paths: paths.ordered_paths().cloned().collect(),
            remote: match location {
                SerializedWorkspaceLocation::Local => None,
                SerializedWorkspaceLocation::Remote(options) => Some(RemoteServer::from_options(options)),
            },
        }
    }

    pub fn workspace_location(&self) -> SerializedWorkspaceLocation {
        match &self.remote {
            None => SerializedWorkspaceLocation::Local,
            Some(server) => SerializedWorkspaceLocation::Remote(server.to_options()),
        }
    }

    pub fn path_list(&self) -> PathList {
        PathList::new(&self.paths)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProject {
    #[serde(flatten)]
    pub location: ProjectLocation,
    /// Unix time, in seconds, the project was last opened or focused.
    #[serde(default)]
    pub opened_at: i64,
    /// Whether the project is open in a window. Kept across quitting so the
    /// next launch can reopen it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
}

/// A server of the recent remote projects, with those projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentServer {
    pub server: RemoteServer,
    /// Most recent first.
    pub projects: Vec<ProjectLocation>,
}

/// Contents of `projects.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectsFile {
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<RecentProject>,
}

impl ProjectsFile {
    /// Parses `projects.json`, treating unreadable contents as an empty file.
    pub fn parse(text: &str) -> ProjectsFile {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Moves `location` to the front of the recent list, keeping its open
    /// flag. Returns whether the list order changed, which is when the file
    /// needs rewriting.
    pub fn record_recent(&mut self, location: ProjectLocation, now: i64) -> bool {
        if location.paths.is_empty() {
            return false;
        }
        let existing = self.recent.iter().position(|project| project.location == location);
        if existing == Some(0) {
            self.recent[0].opened_at = now;
            return false;
        }
        let mut project = match existing {
            Some(index) => self.recent.remove(index),
            None => RecentProject { location, opened_at: 0, open: false },
        };
        project.opened_at = now;
        self.recent.insert(0, project);
        self.recent.truncate(MAX_RECENT_PROJECTS);
        true
    }

    /// Drops every recent entry `remove` matches. Returns whether any was dropped.
    pub fn remove_recent(&mut self, mut remove: impl FnMut(&ProjectLocation) -> bool) -> bool {
        let len = self.recent.len();
        self.recent.retain(|project| !remove(&project.location));
        self.recent.len() != len
    }

    /// Marks exactly the recent entries in `open` as open. Returns whether
    /// any flag changed.
    pub fn set_open(&mut self, open: &[ProjectLocation]) -> bool {
        let mut changed = false;
        for project in &mut self.recent {
            let is_open = open.contains(&project.location);
            changed |= project.open != is_open;
            project.open = is_open;
        }
        changed
    }

    /// Servers of the recent remote projects, most recently used first.
    pub fn recent_servers(&self) -> Vec<RecentServer> {
        let mut servers: Vec<RecentServer> = Vec::new();
        for project in &self.recent {
            let Some(server) = &project.location.remote else {
                continue;
            };
            match servers.iter_mut().find(|recent| &recent.server == server) {
                Some(recent) => recent.projects.push(project.location.clone()),
                None => servers.push(RecentServer { server: server.clone(), projects: vec![project.location.clone()] }),
            }
        }
        servers
    }

    /// Locations of the projects marked open, most recent first.
    pub fn open_projects(&self) -> Vec<ProjectLocation> {
        self.recent.iter().filter(|project| project.open).map(|project| project.location.clone()).collect()
    }
}

/// In-memory copy of `projects.json`, rewritten in full on every change.
pub struct ProjectsStore {
    fs: Arc<dyn Fs>,
    path: PathBuf,
    state: Arc<Mutex<ProjectsFile>>,
    /// Projects marked open at launch, before restored windows update the flags.
    last_session: Vec<ProjectLocation>,
    write_lock: Arc<lock::Mutex<()>>,
}

struct GlobalProjectsStore(Arc<ProjectsStore>);

impl Global for GlobalProjectsStore {}

impl ProjectsStore {
    fn load(fs: Arc<dyn Fs>, path: PathBuf) -> ProjectsStore {
        let file = std_fs::read_to_string(&path).map(|text| ProjectsFile::parse(&text)).unwrap_or_default();
        ProjectsStore {
            fs,
            path,
            last_session: file.open_projects(),
            state: Arc::new(Mutex::new(file)),
            write_lock: Arc::default(),
        }
    }

    pub fn global(cx: &App) -> Arc<ProjectsStore> {
        cx.global::<GlobalProjectsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Arc<ProjectsStore>> {
        cx.try_global::<GlobalProjectsStore>().map(|store| store.0.clone())
    }

    pub fn recent(&self) -> Vec<RecentProject> {
        self.state.lock().recent.clone()
    }

    pub fn recent_servers(&self) -> Vec<RecentServer> {
        self.state.lock().recent_servers()
    }

    pub fn last_session(&self) -> Vec<ProjectLocation> {
        self.last_session.clone()
    }

    pub fn record_recent(&self, location: ProjectLocation, cx: &App) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if self.state.lock().record_recent(location, now) {
            self.save(cx).detach();
        }
    }

    pub fn remove_recent(&self, remove: impl FnMut(&ProjectLocation) -> bool, cx: &App) -> Task<()> {
        if self.state.lock().remove_recent(remove) { self.save(cx) } else { Task::ready(()) }
    }

    fn set_open(&self, open: &[ProjectLocation], cx: &App) -> Task<()> {
        if self.state.lock().set_open(open) { self.save(cx) } else { Task::ready(()) }
    }

    fn save(&self, cx: &App) -> Task<()> {
        let fs = self.fs.clone();
        let path = self.path.clone();
        let state = self.state.clone();
        let write_lock = self.write_lock.clone();
        cx.background_spawn(async move {
            // Writes run one at a time, each taking the latest state, so the
            // file always ends up with the newest contents.
            let _guard = write_lock.lock().await;
            let result = async {
                let json = serde_json::to_string_pretty(&*state.lock())?;
                if let Some(parent) = path.parent() {
                    fs.create_dir(parent).await?;
                }
                fs.atomic_write(path.clone(), json).await
            }
            .await;
            result.with_context(|| format!("writing {path:?}")).log_err();
        })
    }
}

pub fn init(fs: Arc<dyn Fs>, cx: &mut App) {
    let store = ProjectsStore::load(fs, paths::data_dir().join(PROJECTS_FILE_NAME));
    cx.set_global(GlobalProjectsStore(Arc::new(store)));
    track_session(cx);
}

/// Keeps the open flags in `projects.json` matching the open windows. Closing a
/// window clears its project's flag; quitting keeps everything that was open.
fn track_session(cx: &App) {
    cx.spawn(async move |cx| {
        loop {
            cx.update(|cx| {
                if let Some(open) = open_project_locations(cx) {
                    ProjectsStore::global(cx).set_open(&open, cx).detach();
                }
            });
            cx.background_executor().timer(Duration::from_millis(500)).await;
        }
    })
    .detach();

    cx.on_app_quit(|cx| match open_project_locations(cx) {
        Some(open) => ProjectsStore::global(cx).set_open(&open, cx),
        None => Task::ready(()),
    })
    .detach();
}

/// Project locations of the open workspace windows. `None` while no workspace
/// window is open, so the last window closing while quitting doesn't clear the
/// open flags.
fn open_project_locations(cx: &App) -> Option<Vec<ProjectLocation>> {
    let workspaces = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Workspace>()?.entity(cx).ok())
        .collect::<Vec<_>>();
    if workspaces.is_empty() {
        return None;
    }
    Some(workspaces.iter().filter_map(|workspace| workspace.read(cx).project_location(cx)).collect())
}

/// Projects that were open when ed last quit, in the order to reopen them
/// (most recent last, so it ends up on top).
pub fn last_session_workspaces(cx: &App) -> Vec<SessionWorkspace> {
    let mut session = ProjectsStore::global(cx)
        .last_session()
        .into_iter()
        .map(|location| SessionWorkspace { paths: location.path_list(), location: location.workspace_location() })
        .collect::<Vec<_>>();
    session.reverse();
    session
}

pub fn workspace_state_rel_path() -> Arc<RelPath> {
    RelPath::from_unix_str(&format!("{WORKSPACE_STATE_DIR}/{WORKSPACE_STATE_FILE}")).unwrap().into_arc()
}

/// Whether `path`, relative to a root folder, is inside the project-level
/// `.ed` folder. Tabs on such files aren't persisted: `.ed` is config, and
/// recording a tab on `workspace.json` would rewrite the file it shows.
pub fn is_in_workspace_state_dir(path: &RelPath) -> bool {
    path.starts_with(RelPath::from_unix_str(WORKSPACE_STATE_DIR).unwrap())
}

/// Path of the state file for a project whose first root folder is `root`.
pub fn workspace_state_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_STATE_DIR).join(WORKSPACE_STATE_FILE)
}

/// Parses a `workspace.json`, ignoring unreadable contents.
pub(crate) fn parse_workspace_state(text: &str) -> Option<SerializedWorkspace> {
    serde_json::from_str(text).log_err()
}

pub(crate) async fn load_local_workspace_state(fs: &dyn Fs, root: &Path) -> Option<SerializedWorkspace> {
    let text = fs.load(&workspace_state_path(root)).await.ok()?;
    parse_workspace_state(&text)
}

/// Reads `.ed/workspace.json` from the worktree rooted at `root` of a remote
/// project, through the project's buffers.
pub(crate) async fn load_remote_workspace_state(
    project: &Entity<Project>,
    root: &Path,
    cx: &mut AsyncApp,
) -> Option<SerializedWorkspace> {
    let open_buffer = project.update(cx, |project, cx| {
        let worktree = project.visible_worktrees(cx).find(|worktree| worktree.read(cx).abs_path().as_ref() == root)?;
        let worktree_id = worktree.read(cx).id();
        Some(project.open_buffer((worktree_id, workspace_state_rel_path()), cx))
    })?;
    let buffer = open_buffer.await.ok()?;
    let text = buffer.read_with(cx, |buffer, _| buffer.text());
    parse_workspace_state(&text)
}

/// Writes `json` to `.ed/workspace.json` in the project's first root folder.
pub(crate) fn write_workspace_state(
    project: &Entity<Project>,
    root: &Path,
    json: String,
    cx: &mut App,
) -> Task<Result<()>> {
    if project.read(cx).is_local() {
        let fs = project.read(cx).fs().clone();
        let path = workspace_state_path(root);
        return cx.background_spawn(async move {
            if let Some(parent) = path.parent() {
                fs.create_dir(parent).await?;
            }
            fs.atomic_write(path, json).await
        });
    }

    let Some(worktree) =
        project.read(cx).visible_worktrees(cx).find(|worktree| worktree.read(cx).abs_path().as_ref() == root)
    else {
        return Task::ready(Ok(()));
    };
    let write = worktree.update(cx, |worktree, cx| {
        worktree.create_entry(workspace_state_rel_path(), false, Some(json.into_bytes()), cx)
    });
    cx.background_spawn(async move {
        write.await?;
        Ok(())
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentWorkspace {
    pub location: SerializedWorkspaceLocation,
    pub paths: PathList,
    pub identity_paths: PathList,
    pub timestamp: OffsetDateTime,
}

async fn all_paths_exist_with_a_directory(paths: &[PathBuf], fs: &dyn Fs) -> bool {
    let mut any_dir = false;
    for path in paths {
        match fs.metadata(path).await.ok().flatten() {
            None => return false,
            Some(meta) => {
                if meta.is_dir {
                    any_dir = true;
                }
            }
        }
    }
    any_dir
}

/// Recent projects for recent-project UIs, most recent first. Local projects
/// whose folders are gone are skipped, and git worktrees of one repository are
/// grouped under the repository's main worktree.
pub async fn recent_project_workspaces(recent: Vec<RecentProject>, fs: &dyn Fs) -> Vec<RecentWorkspace> {
    let mut result = Vec::new();
    for project in recent {
        let paths = project.location.path_list();
        let timestamp = OffsetDateTime::from_unix_timestamp(project.opened_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
        if project.location.remote.is_some() {
            result.push(RecentWorkspace {
                location: project.location.workspace_location(),
                identity_paths: paths.clone(),
                paths,
                timestamp,
            });
            continue;
        }
        if all_paths_exist_with_a_directory(paths.paths(), fs).await {
            let identity_paths = resolve_local_workspace_identity(fs, &paths).await.unwrap_or_else(|| paths.clone());
            result.push(RecentWorkspace {
                location: SerializedWorkspaceLocation::Local,
                paths,
                identity_paths,
                timestamp,
            });
        }
    }
    dedupe_recent_workspaces(result)
}

/// Removes every recent entry grouped with `target`.
pub async fn delete_recent_workspace_group(target: &RecentWorkspace, fs: &dyn Fs, cx: &mut AsyncApp) {
    let store = cx.update(|cx| ProjectsStore::global(cx));
    let mut doomed = Vec::new();
    for workspace in recent_project_workspaces_ungrouped(store.recent(), fs).await {
        if same_group(&workspace, target) {
            doomed.push(ProjectLocation::new(&workspace.location, &workspace.paths));
        }
    }
    cx.update(|cx| store.remove_recent(|location| doomed.contains(location), cx)).await;
}

async fn recent_project_workspaces_ungrouped(recent: Vec<RecentProject>, fs: &dyn Fs) -> Vec<RecentWorkspace> {
    let mut result = Vec::new();
    for project in recent {
        let paths = project.location.path_list();
        let identity_paths = if project.location.remote.is_none() {
            resolve_local_workspace_identity(fs, &paths).await.unwrap_or_else(|| paths.clone())
        } else {
            paths.clone()
        };
        result.push(RecentWorkspace {
            location: project.location.workspace_location(),
            paths,
            identity_paths,
            timestamp: OffsetDateTime::UNIX_EPOCH,
        });
    }
    result
}

fn same_group(left: &RecentWorkspace, right: &RecentWorkspace) -> bool {
    location_identity(&left.location) == location_identity(&right.location)
        && left.identity_paths == right.identity_paths
}

fn location_identity(location: &SerializedWorkspaceLocation) -> Option<RemoteConnectionIdentity> {
    match location {
        SerializedWorkspaceLocation::Local => None,
        SerializedWorkspaceLocation::Remote(connection) => Some(remote_connection_identity(connection)),
    }
}

async fn resolve_local_workspace_identity(fs: &dyn Fs, paths: &PathList) -> Option<PathList> {
    let raw_paths = paths.paths();
    let resolved_paths =
        future::join_all(raw_paths.iter().map(|path| git_store::resolve_git_worktree_to_main_repo(fs, path))).await;

    if resolved_paths.iter().all(|resolved| resolved.is_none()) {
        return None;
    }

    let resolved_paths: Vec<PathBuf> = raw_paths
        .iter()
        .zip(resolved_paths.iter())
        .map(|(original, resolved)| resolved.as_ref().cloned().unwrap_or_else(|| original.clone()))
        .collect();
    let resolved_path_refs: Vec<&Path> = resolved_paths.iter().map(PathBuf::as_path).collect();
    Some(PathList::new(&resolved_path_refs))
}

/// Keeps the first, most recent, workspace of each group.
fn dedupe_recent_workspaces(workspaces: impl IntoIterator<Item = RecentWorkspace>) -> Vec<RecentWorkspace> {
    let mut seen: FxHashMap<(Option<RemoteConnectionIdentity>, Vec<PathBuf>), ()> = FxHashMap::default();
    workspaces
        .into_iter()
        .filter(|workspace| {
            let key = (location_identity(&workspace.location), workspace.identity_paths.paths().to_vec());
            seen.insert(key, ()).is_none()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use time::OffsetDateTime;
    use util::path_list::PathList;
    use util::rel_path::RelPath;

    use super::ProjectLocation;
    use super::ProjectsFile;
    use super::RecentProject;
    use super::RecentServer;
    use super::RecentWorkspace;
    use super::RemoteServer;
    use super::dedupe_recent_workspaces;
    use super::is_in_workspace_state_dir;
    use super::model::SerializedWorkspaceLocation;
    use super::workspace_state_path;
    use super::workspace_state_rel_path;

    fn local(paths: &[&str]) -> ProjectLocation {
        ProjectLocation { paths: paths.iter().map(PathBuf::from).collect(), remote: None }
    }

    #[test]
    fn test_projects_file_parse_ignores_invalid_contents() {
        assert_eq!(ProjectsFile::parse("not json"), ProjectsFile::default());
        assert_eq!(ProjectsFile::parse(r#"{"recent": 3}"#), ProjectsFile::default());
        assert_eq!(ProjectsFile::parse("{}"), ProjectsFile::default());
    }

    #[test]
    fn test_projects_file_round_trip() {
        let file = ProjectsFile {
            recent: vec![
                RecentProject { location: local(&["/b", "/a"]), opened_at: 7, open: true },
                RecentProject {
                    location: ProjectLocation {
                        paths: vec![PathBuf::from("/srv")],
                        remote: Some(RemoteServer { host: "box".into(), username: Some("me".into()), port: None }),
                    },
                    opened_at: 3,
                    open: false,
                },
            ],
        };
        let json = serde_json::to_string(&file).unwrap();
        assert_eq!(ProjectsFile::parse(&json), file);
    }

    #[test]
    fn test_record_recent_moves_project_to_front() {
        let mut file = ProjectsFile::default();
        assert!(file.record_recent(local(&["/a"]), 1));
        assert!(file.record_recent(local(&["/b"]), 2));
        // Already first: only the timestamp moves, and nothing needs writing.
        assert!(!file.record_recent(local(&["/b"]), 3));
        assert!(file.record_recent(local(&["/a"]), 4));
        assert!(!file.record_recent(local(&[]), 5));
        assert_eq!(
            file.recent,
            [
                RecentProject { location: local(&["/a"]), opened_at: 4, open: false },
                RecentProject { location: local(&["/b"]), opened_at: 3, open: false }
            ]
        );
    }

    #[test]
    fn test_recent_servers_groups_remote_projects_by_server() {
        let remote = |host: &str, paths: &[&str]| ProjectLocation {
            paths: paths.iter().map(PathBuf::from).collect(),
            remote: Some(RemoteServer { host: host.into(), username: None, port: None }),
        };
        let mut file = ProjectsFile::default();
        file.record_recent(remote("a", &["/one"]), 1);
        file.record_recent(local(&["/local"]), 2);
        file.record_recent(remote("b", &["/two"]), 3);
        file.record_recent(remote("a", &["/three"]), 4);
        let server = |host: &str| RemoteServer { host: host.into(), username: None, port: None };
        assert_eq!(
            file.recent_servers(),
            [
                RecentServer { server: server("a"), projects: vec![remote("a", &["/three"]), remote("a", &["/one"])] },
                RecentServer { server: server("b"), projects: vec![remote("b", &["/two"])] },
            ]
        );
        assert_eq!(ProjectsFile::default().recent_servers(), []);
    }

    #[test]
    fn test_record_recent_caps_the_list() {
        let mut file = ProjectsFile::default();
        for index in 0..super::MAX_RECENT_PROJECTS + 5 {
            file.record_recent(local(&[&format!("/{index}")]), i64::try_from(index).unwrap());
        }
        assert_eq!(file.recent.len(), super::MAX_RECENT_PROJECTS);
        assert_eq!(file.recent[0].location, local(&[&format!("/{}", super::MAX_RECENT_PROJECTS + 4)]));
    }

    #[test]
    fn test_remove_recent() {
        let mut file = ProjectsFile::default();
        file.record_recent(local(&["/a"]), 1);
        file.record_recent(local(&["/b"]), 2);
        assert!(file.remove_recent(|location| location == &local(&["/a"])));
        assert!(!file.remove_recent(|location| location == &local(&["/a"])));
        assert_eq!(file.recent.len(), 1);
    }

    #[test]
    fn test_record_recent_keeps_open_flag() {
        let mut file = ProjectsFile::default();
        file.record_recent(local(&["/a"]), 1);
        file.record_recent(local(&["/b"]), 2);
        file.set_open(&[local(&["/a"])]);
        assert!(file.record_recent(local(&["/a"]), 3));
        assert_eq!(file.recent[0], RecentProject { location: local(&["/a"]), opened_at: 3, open: true });
    }

    #[test]
    fn test_set_open_reports_changes() {
        let mut file = ProjectsFile::default();
        file.record_recent(local(&["/a"]), 1);
        file.record_recent(local(&["/b"]), 2);
        assert!(file.set_open(&[local(&["/a"]), local(&["/c"])]));
        assert!(!file.set_open(&[local(&["/a"])]));
        assert_eq!(file.open_projects(), [local(&["/a"])]);
        assert!(file.set_open(&[]));
        assert!(file.open_projects().is_empty());
    }

    #[test]
    fn test_open_projects_most_recent_first() {
        let mut file = ProjectsFile::default();
        file.record_recent(local(&["/a"]), 1);
        file.record_recent(local(&["/b"]), 2);
        file.record_recent(local(&["/c"]), 3);
        file.set_open(&[local(&["/a"]), local(&["/c"])]);
        assert_eq!(file.open_projects(), [local(&["/c"]), local(&["/a"])]);
    }

    #[test]
    fn test_is_in_workspace_state_dir() {
        fn rel(path: &str) -> &RelPath {
            RelPath::from_unix_str(path).unwrap()
        }
        assert!(is_in_workspace_state_dir(rel(".ed")));
        assert!(is_in_workspace_state_dir(rel(".ed/workspace.json")));
        assert!(is_in_workspace_state_dir(rel(".ed/sub/settings.json")));
        assert!(is_in_workspace_state_dir(&workspace_state_rel_path()));
        assert!(!is_in_workspace_state_dir(rel(".editorconfig")));
        assert!(!is_in_workspace_state_dir(rel("src/.ed/a.rs")));
        assert!(!is_in_workspace_state_dir(rel("src/main.rs")));
        assert!(!is_in_workspace_state_dir(rel("")));
    }

    #[test]
    fn test_open_flag_serialized_only_when_set() {
        let mut file = ProjectsFile::default();
        file.record_recent(local(&["/a"]), 1);
        assert!(!serde_json::to_string(&file).unwrap().contains(r#""open":"#));
        file.set_open(&[local(&["/a"])]);
        assert!(serde_json::to_string(&file).unwrap().contains(r#""open":true"#));
    }

    #[test]
    fn test_project_location_keeps_path_order() {
        let paths = PathList::new(&["/b", "/a"]);
        let location = ProjectLocation::new(&SerializedWorkspaceLocation::Local, &paths);
        assert_eq!(location.paths, [PathBuf::from("/b"), PathBuf::from("/a")]);
        assert_eq!(location.path_list(), paths);
        assert_eq!(location.workspace_location(), SerializedWorkspaceLocation::Local);
    }

    #[test]
    fn test_remote_server_round_trip() {
        let server = RemoteServer { host: "box".into(), username: Some("me".into()), port: Some(2222) };
        let location = ProjectLocation { paths: vec![PathBuf::from("/srv")], remote: Some(server.clone()) };
        let SerializedWorkspaceLocation::Remote(options) = location.workspace_location() else {
            panic!("expected a remote location");
        };
        assert_eq!(RemoteServer::from_options(&options), server);
    }

    #[test]
    fn test_workspace_state_paths() {
        assert_eq!(workspace_state_path("/p".as_ref()), PathBuf::from("/p/.ed/workspace.json"));
        assert_eq!(workspace_state_rel_path().as_unix_str(), ".ed/workspace.json");
    }

    #[test]
    fn test_dedupe_recent_workspaces_keeps_first_of_group() {
        let workspace = |paths: &[&str], identity: &[&str]| RecentWorkspace {
            location: SerializedWorkspaceLocation::Local,
            paths: PathList::new(paths),
            identity_paths: PathList::new(identity),
            timestamp: OffsetDateTime::UNIX_EPOCH,
        };
        let deduped = dedupe_recent_workspaces([
            workspace(&["/repo-wt"], &["/repo"]),
            workspace(&["/repo"], &["/repo"]),
            workspace(&["/other"], &["/other"]),
        ]);
        assert_eq!(deduped, [workspace(&["/repo-wt"], &["/repo"]), workspace(&["/other"], &["/other"])]);
    }
}
