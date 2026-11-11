use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::iter;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use futures::StreamExt as _;
use gpui::BackgroundExecutor;
use gpui::Task;
use notify::Event;
use notify::EventKind;
use parking_lot::Mutex;
use smol::channel::Sender;
use util::ResultExt;
use util::paths::SanitizedPath;

use crate::Fs;
use crate::PathEvent;
use crate::PathEventKind;
use crate::Watcher;

/// How often a watch on a not-yet-existing path is retried.
const PENDING_PATH_RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) async fn watch(
    fs: Arc<dyn Fs>,
    native_watcher: Arc<OsWatcher>,
    executor: BackgroundExecutor,
    path: &Path,
    latency: Duration,
) -> (Pin<Box<dyn Send + Stream<Item = Vec<PathEvent>>>>, Arc<dyn Watcher>) {
    let (tx, rx) = async_channel::unbounded();
    let pending_paths: Arc<Mutex<Vec<PathEvent>>> = Arc::default();

    let watcher: Arc<dyn Watcher> =
        Arc::new(FsWatcher::new(native_watcher, fs.clone(), executor.clone(), tx, pending_paths.clone()));

    if let Err(e) = watcher.add(path) {
        log::warn!("Failed to watch {}:\n{e}", path.display());
    }

    // Check if path is a symlink and follow the target parent
    if let Ok(mut target) = fs.read_link(path).await {
        log::trace!("watch symlink {path:?} -> {target:?}");
        // Check if symlink target is relative path, if so make it absolute
        if target.is_relative()
            && let Some(parent) = path.parent()
        {
            target = parent.join(target);
            if let Ok(canonical) = fs.canonicalize(&target).await {
                target = SanitizedPath::new(&canonical).as_path().to_path_buf();
            }
        }
        watcher.add(&target).ok();
        if let Some(parent) = target.parent() {
            watcher.add(parent).log_err();
        }
    }

    (
        Box::pin(rx.filter_map({
            let watcher = watcher.clone();
            move |_| {
                let _ = watcher.clone();
                let pending_paths = pending_paths.clone();
                let executor = executor.clone();
                async move {
                    if !latency.is_zero() {
                        executor.timer(latency).await;
                    }
                    let paths = mem::take(&mut *pending_paths.lock());
                    log::debug!("pending path events: {:?}", paths);
                    (!paths.is_empty()).then_some(paths)
                }
            }
        })),
        watcher,
    )
}

pub struct FsWatcher {
    native_watcher: Arc<OsWatcher>,
    fs: Arc<dyn Fs>,
    executor: BackgroundExecutor,
    tx: async_channel::Sender<()>,
    pending_path_events: Arc<Mutex<Vec<PathEvent>>>,
    registrations: Arc<Mutex<HashMap<WatchKey, FsWatcherRegistration>>>,
    pending_registrations: Arc<Mutex<HashMap<Arc<Path>, Task<()>>>>,
}

#[derive(Clone)]
struct FsWatcherRegistration {
    id: WatcherRegistrationId,
    os_watcher: Arc<OsWatcher>,
}

impl FsWatcher {
    pub(crate) fn new(
        native_watcher: Arc<OsWatcher>,
        fs: Arc<dyn Fs>,
        executor: BackgroundExecutor,
        tx: async_channel::Sender<()>,
        pending_path_events: Arc<Mutex<Vec<PathEvent>>>,
    ) -> Self {
        Self {
            native_watcher,
            fs,
            executor,
            tx,
            pending_path_events,
            registrations: Arc::default(),
            pending_registrations: Arc::default(),
        }
    }

    fn add_existing_path(&self, path: Arc<Path>) -> anyhow::Result<()> {
        let case_insensitive = !self.fs.is_path_case_sensitive(&path);
        let key = WatchKey::for_registration(SanitizedPath::new(&path), case_insensitive);
        if self.registrations.lock().contains_key(&key) {
            log::trace!("path to watch is already registered: {path:?}");
            return Ok(());
        }
        let registration = register_existing_path(
            &self.native_watcher,
            path,
            case_insensitive,
            self.tx.clone(),
            self.pending_path_events.clone(),
        )?;
        self.registrations.lock().insert(key, registration);
        Ok(())
    }

    fn add_pending_path(&self, path: Arc<Path>) {
        let mut pending_registrations = self.pending_registrations.lock();
        if pending_registrations.contains_key(path.as_ref()) {
            return;
        }

        let task = self.executor.spawn(poll_path_until_created(
            self.native_watcher.clone(),
            self.fs.clone(),
            self.executor.clone(),
            path.clone(),
            self.tx.clone(),
            self.pending_path_events.clone(),
            self.registrations.clone(),
            self.pending_registrations.clone(),
        ));
        pending_registrations.insert(path, task);
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        self.pending_registrations.lock().clear();

        let mut registrations = HashMap::new();
        {
            let old = &mut self.registrations.lock();
            mem::swap(&mut **old, &mut registrations);
        }

        for (_, registration) in registrations {
            registration.os_watcher.remove(registration.id);
        }
    }
}

impl Watcher for FsWatcher {
    fn add(&self, path: &Path) -> anyhow::Result<()> {
        log::trace!("watcher add: {path:?}");

        let path: Arc<Path> = path.into();
        if path_covered_by_registration(&self.registrations.lock(), SanitizedPath::new(&path)) {
            log::trace!("path to watch is covered by an existing registration: {path:?}");
            return Ok(());
        }

        if self.pending_registrations.lock().contains_key(path.as_ref()) {
            log::trace!("path to watch is already pending: {path:?}");
            return Ok(());
        }

        if !self.fs.path_exists(&path) {
            self.add_pending_path(path);
            return Ok(());
        }

        self.add_existing_path(path)
    }

    fn remove(&self, path: &Path) -> anyhow::Result<()> {
        log::trace!("remove watched path: {path:?}");
        self.pending_registrations.lock().remove(path);

        let sanitized = SanitizedPath::new(path);
        let registration = {
            let mut registrations = self.registrations.lock();
            registrations
                .remove(&WatchKey::exact(sanitized))
                .or_else(|| registrations.remove(&WatchKey::folded(sanitized)))
        };
        if let Some(registration) = registration {
            registration.os_watcher.remove(registration.id);
        }
        Ok(())
    }
}

/// Whether a (recursive) registration on a strict ancestor of `path` already
/// covers it. Both key spellings are probed so a folded registration still matches.
fn path_covered_by_registration(
    registrations: &HashMap<WatchKey, FsWatcherRegistration>,
    path: &SanitizedPath,
) -> bool {
    path.as_path().ancestors().skip(1).any(|ancestor| {
        let ancestor = SanitizedPath::unchecked_new(ancestor);
        [WatchKey::exact(ancestor), WatchKey::folded(ancestor)].iter().any(|key| registrations.contains_key(key))
    })
}

fn register_existing_path(
    native_watcher: &Arc<OsWatcher>,
    path: Arc<Path>,
    case_insensitive: bool,
    tx: async_channel::Sender<()>,
    pending_path_events: Arc<Mutex<Vec<PathEvent>>>,
) -> anyhow::Result<FsWatcherRegistration> {
    let root_path = SanitizedPath::new_arc(path.as_ref());
    let path_for_callback = path.clone();
    let registration_id = native_watcher.add(path, case_insensitive, move |event: &notify::Event| {
        log::trace!("watcher received event: {event:?}");
        push_notify_event(&tx, &pending_path_events, &root_path, case_insensitive, path_for_callback.as_ref(), event);
    })?;
    Ok(FsWatcherRegistration { id: registration_id, os_watcher: native_watcher.clone() })
}

/// Whether the volume backing `path` does case-insensitive name lookups, used to
/// pick exact vs. folded matching.
pub(crate) fn case_insensitive_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;

    // `pathconf(_PC_CASE_SENSITIVE)` returns 1 (sensitive), 0 (insensitive), or -1
    // on error; default errors to insensitive (the APFS/HFS+ default).
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: We just initialized c_path, so it's a valid pointer
    unsafe { libc::pathconf(c_path.as_ptr(), libc::_PC_CASE_SENSITIVE) == 0 }
}

/// Whether `path` is `root` or sits beneath it, folding case on case-insensitive
/// volumes so a differently-cased spelling still matches.
fn path_is_under(path: &SanitizedPath, root: &SanitizedPath, case_insensitive: bool) -> bool {
    if case_insensitive {
        let path = path.as_path().to_string_lossy().to_lowercase();
        let root = root.as_path().to_string_lossy().to_lowercase();
        Path::new(&path).starts_with(Path::new(&root))
    } else {
        path.starts_with(root)
    }
}

/// Lookup key for a watch path, shared by add, remove, and dispatch so they all
/// agree on whether two spellings denote the same directory.
///
/// On case-sensitive volumes the exact (sanitized) path is the key, so genuinely
/// distinct directories stay distinct. On case-insensitive volumes the folded
/// (lowercased) spelling is the key, so any casing of a directory collides.
///
/// The two variants are distinct map keys, so a case-sensitive registration can
/// never be hit by a folded lookup (or vice versa); dispatch can therefore probe
/// both forms of an event path without risking a cross-rule false match.
///
/// Folded keys use path-component spelling so separators and `.` components
/// agree with exact path equality. macOS also normalizes Unicode before folding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum WatchKey {
    Exact(Arc<Path>),
    Folded(Arc<str>),
}

impl WatchKey {
    fn exact(path: &SanitizedPath) -> Self {
        Self::Exact(Arc::from(path.as_path()))
    }

    fn folded(path: &SanitizedPath) -> Self {
        // Event ancestors omit trailing separators, even when registration paths include them.
        let normalized_path = path.as_path().components().collect::<PathBuf>();
        let lossy = normalized_path.to_string_lossy();
        // APFS/HFS+ compare names normalization-insensitively (NFC vs NFD), and
        // FSEvents can report NFD while a config/LSP supplies NFC, so normalize
        // before folding case.
        use unicode_normalization::UnicodeNormalization as _;
        Self::Folded(lossy.chars().nfc().collect::<String>().to_lowercase().into())
    }

    fn for_registration(path: &SanitizedPath, case_insensitive: bool) -> Self {
        if case_insensitive { Self::folded(path) } else { Self::exact(path) }
    }
}

async fn poll_path_until_created(
    native_watcher: Arc<OsWatcher>,
    fs: Arc<dyn Fs>,
    executor: BackgroundExecutor,
    path: Arc<Path>,
    tx: async_channel::Sender<()>,
    pending_path_events: Arc<Mutex<Vec<PathEvent>>>,
    registrations: Arc<Mutex<HashMap<WatchKey, FsWatcherRegistration>>>,
    pending_registrations: Arc<Mutex<HashMap<Arc<Path>, Task<()>>>>,
) {
    loop {
        executor.timer(PENDING_PATH_RETRY_INTERVAL).await;

        if !pending_registrations.lock().contains_key(path.as_ref()) {
            return;
        }

        if !fs.path_exists(&path) {
            continue;
        }

        // Probe case sensitivity now that the path exists, rather than at add
        // time when it didn't.
        let case_insensitive = if fs.is_fake() {
            !fs.is_path_case_sensitive(&path)
        } else {
            smol::unblock({
                let path = path.clone();
                let fs = fs.clone();
                move || !fs.is_path_case_sensitive(&path)
            })
            .await
        };
        let key = WatchKey::for_registration(SanitizedPath::new(&path), case_insensitive);

        if registrations.lock().contains_key(&key) {
            pending_registrations.lock().remove(path.as_ref());
            return;
        }

        let register = {
            let path = path.clone();
            let tx = tx.clone();
            let pending_path_events = pending_path_events.clone();
            let native_watcher = native_watcher.clone();
            move || register_existing_path(&native_watcher, path, case_insensitive, tx, pending_path_events)
        };
        let registration = if fs.is_fake() { register() } else { smol::unblock(register).await };

        match registration {
            Ok(registration) => {
                {
                    let mut pending_registrations = pending_registrations.lock();
                    if pending_registrations.remove(path.as_ref()).is_none() {
                        registration.os_watcher.remove(registration.id);
                        return;
                    }
                    registrations.lock().insert(key, registration);
                }
                enqueue_path_events(
                    &tx,
                    &pending_path_events,
                    vec![
                        PathEvent { path: path.to_path_buf(), kind: Some(PathEventKind::Created) },
                        PathEvent { path: path.to_path_buf(), kind: Some(PathEventKind::Rescan) },
                    ],
                );
                return;
            }
            Err(error) => {
                log::warn!("failed to watch newly-created path {path:?}: {error}; retrying");
            }
        }
    }
}

fn enqueue_path_events(
    tx: &Sender<()>,
    pending_path_events: &Arc<Mutex<Vec<PathEvent>>>,
    mut path_events: Vec<PathEvent>,
) {
    if path_events.is_empty() {
        return;
    }

    path_events.sort();
    let mut pending_paths = pending_path_events.lock();
    if pending_paths.is_empty() {
        tx.try_send(()).ok();
    }
    coalesce_pending_rescans(&mut pending_paths, &mut path_events);
    util::extend_sorted(&mut *pending_paths, path_events, usize::MAX, |a, b| a.path.cmp(&b.path));
}

fn push_notify_event(
    tx: &Sender<()>,
    pending_path_events: &Arc<Mutex<Vec<PathEvent>>>,
    root_path: &SanitizedPath,
    case_insensitive: bool,
    watched_root: &Path,
    event: &notify::Event,
) {
    let kind = match event.kind {
        EventKind::Create(_) => Some(PathEventKind::Created),
        EventKind::Modify(_) => Some(PathEventKind::Changed),
        EventKind::Remove(_) => Some(PathEventKind::Removed),
        _ => None,
    };
    let mut path_events = event
        .paths
        .iter()
        .filter_map(|event_path| {
            let event_path = SanitizedPath::new(event_path);
            path_is_under(event_path, root_path, case_insensitive)
                .then(|| PathEvent { path: event_path.as_path().to_path_buf(), kind })
        })
        .collect::<Vec<_>>();

    if event.need_rescan() {
        // A rescan event names the subtrees that lost sync (FSEvents
        // `MustScanSubDirs`); a pathless one means the whole watched root is
        // suspect.
        if path_events.is_empty() {
            path_events.push(PathEvent { path: watched_root.to_path_buf(), kind: Some(PathEventKind::Rescan) });
        } else {
            for path_event in &mut path_events {
                path_event.kind = Some(PathEventKind::Rescan);
            }
        }
    }
    log::trace!("path_events: {:?}", path_events);
    enqueue_path_events(tx, pending_path_events, path_events);
}

fn coalesce_pending_rescans(pending_paths: &mut Vec<PathEvent>, path_events: &mut Vec<PathEvent>) {
    if !path_events.iter().any(|event| event.kind == Some(PathEventKind::Rescan)) {
        return;
    }

    let mut new_rescan_paths: Vec<PathBuf> =
        path_events.iter().filter(|e| e.kind == Some(PathEventKind::Rescan)).map(|e| e.path.clone()).collect();
    new_rescan_paths.sort_unstable();

    let mut deduped_rescans: Vec<PathBuf> = Vec::with_capacity(new_rescan_paths.len());
    for path in new_rescan_paths {
        if deduped_rescans.iter().any(|ancestor| path != *ancestor && path.starts_with(ancestor)) {
            continue;
        }
        deduped_rescans.push(path);
    }

    deduped_rescans.retain(|new_path| {
        !pending_paths.iter().any(|pending| is_covered_rescan(pending.kind, new_path, &pending.path))
    });

    if !deduped_rescans.is_empty() {
        pending_paths.retain(|pending| {
            !deduped_rescans.iter().any(|rescan_path| {
                pending.path == *rescan_path || is_covered_rescan(pending.kind, &pending.path, rescan_path)
            })
        });
    }

    path_events.retain(|event| event.kind != Some(PathEventKind::Rescan) || deduped_rescans.contains(&event.path));
}

fn path_depth(path: &SanitizedPath) -> usize {
    path.as_path().components().count()
}

fn is_covered_rescan(kind: Option<PathEventKind>, path: &Path, ancestor: &Path) -> bool {
    kind == Some(PathEventKind::Rescan) && path != ancestor && path.starts_with(ancestor)
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct WatcherRegistrationId(u32);

struct WatcherRegistrationState {
    callback: Arc<dyn Fn(&notify::Event) + Send + Sync>,
    key: WatchKey,
}

struct PathRegistrationState {
    path: Arc<SanitizedPath>,
    /// Empty while a backend watch is retained for covered descendants that
    /// could not get their own backend watch.
    watcher_ids: Vec<WatcherRegistrationId>,
    coverage: Coverage,
}

enum Coverage {
    /// Has its own backend watch. That watch also serves the descendant registrations listed here by [`path_depth`] and
    /// key, so promotion proceeds shallowest first without re-sorting.
    Watched { covered: BTreeSet<(usize, WatchKey)> },
    /// Served by the recursive backend watch of this ancestor registration.
    CoveredBy(WatchKey),
}

/// The registered watch paths for one backend, keyed by [`WatchKey`] so that
/// add (dedup), remove, and dispatch share a single notion of path identity.
#[derive(Default)]
struct WatchPaths(HashMap<WatchKey, PathRegistrationState>);

impl WatchPaths {
    fn contains(&self, key: &WatchKey) -> bool {
        self.0.contains_key(key)
    }

    fn get_mut(&mut self, key: &WatchKey) -> Option<&mut PathRegistrationState> {
        self.0.get_mut(key)
    }

    /// The nearest strict ancestor whose recursive backend watch covers `path`.
    fn covering_ancestor(&self, path: &SanitizedPath) -> Option<WatchKey> {
        path.as_path().ancestors().skip(1).find_map(|ancestor| {
            let ancestor = SanitizedPath::unchecked_new(ancestor);
            [WatchKey::exact(ancestor), WatchKey::folded(ancestor)].into_iter().find(|key| {
                self.0.get(key).is_some_and(|registration| matches!(registration.coverage, Coverage::Watched { .. }))
            })
        })
    }

    fn covered_by_mut(&mut self, key: &WatchKey) -> Option<&mut BTreeSet<(usize, WatchKey)>> {
        match &mut self.0.get_mut(key)?.coverage {
            Coverage::Watched { covered } => Some(covered),
            Coverage::CoveredBy(_) => None,
        }
    }

    /// Collects the watcher ids of every registration whose directory is an
    /// ancestor of (or equal to) `path`. Both exact and folded keys are probed,
    /// so a real-cased event path matches a folded registration and vice versa.
    fn watcher_ids_covering(&self, path: &SanitizedPath, ids: &mut Vec<WatcherRegistrationId>) {
        for ancestor in path.as_path().ancestors() {
            let ancestor = SanitizedPath::unchecked_new(ancestor);
            if let Some(registration) = self.0.get(&WatchKey::exact(ancestor)) {
                ids.extend_from_slice(&registration.watcher_ids);
            }
            if let Some(registration) = self.0.get(&WatchKey::folded(ancestor)) {
                ids.extend_from_slice(&registration.watcher_ids);
            }
        }
    }
}

struct WatcherState {
    watchers: HashMap<WatcherRegistrationId, WatcherRegistrationState>,
    paths: WatchPaths,
    last_registration: WatcherRegistrationId,
}

pub(crate) trait WatchBackend: Send {
    fn watch(&mut self, path: &Path, mode: notify::RecursiveMode) -> notify::Result<()>;
    fn unwatch(&mut self, path: &Path) -> notify::Result<()>;
}

impl<T: notify::Watcher + Send> WatchBackend for T {
    fn watch(&mut self, path: &Path, mode: notify::RecursiveMode) -> notify::Result<()> {
        notify::Watcher::watch(self, path, mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        notify::Watcher::unwatch(self, path)
    }
}

pub struct OsWatcher {
    state: Arc<Mutex<WatcherState>>,

    // Hold state across backend mutations. Callbacks don't acquire state or
    // invert the lock order.
    backend: Mutex<Option<Box<dyn WatchBackend>>>,
    event_tx: async_channel::Sender<notify::Result<notify::Event>>,
    _dispatch_task: Task<()>,
    // Closing the event channel can wake the dispatch task. Keep its scheduler
    // alive through channel and task teardown, even when our owner drops first.
    _executor: BackgroundExecutor,
}

impl OsWatcher {
    /// Watches with notify's native backend, created lazily on first use so
    /// that a failure to start it can be reported by `ensure_backend`.
    pub fn new(executor: BackgroundExecutor) -> Arc<Self> {
        Self::with_backend(executor, None)
    }

    /// Watches with the given backend in place of notify's; `event_sink` is how
    /// that backend reports events, standing in for notify's `EventHandler`.
    pub(crate) fn with_backend(executor: BackgroundExecutor, backend: Option<Box<dyn WatchBackend>>) -> Arc<Self> {
        let (event_tx, event_rx) = async_channel::unbounded();
        let state = Arc::new(Mutex::new(WatcherState {
            watchers: HashMap::default(),
            paths: WatchPaths::default(),
            last_registration: WatcherRegistrationId::default(),
        }));
        let dispatch_task = executor.spawn({
            let state = state.clone();
            async move {
                let mut rescan_history = RescanHistory::default();
                while let Ok(first) = event_rx.recv().await {
                    dispatch_batch(&state, &mut rescan_history, first, &event_rx);
                }
            }
        });
        Arc::new(Self {
            state,
            backend: Mutex::new(backend),
            event_tx,
            _dispatch_task: dispatch_task,
            _executor: executor,
        })
    }

    /// The callback a substitute backend uses to report events, standing
    /// in for the `EventHandler` notify would have been constructed with.
    pub(crate) fn event_sink(&self) -> impl Fn(notify::Result<notify::Event>) + Send + Sync + 'static {
        let event_tx = self.event_tx.clone();
        move |event| {
            enqueue(&event_tx, event);
        }
    }

    fn add(
        &self,
        path: Arc<Path>,
        case_insensitive: bool,
        cb: impl Fn(&notify::Event) + Send + Sync + 'static,
    ) -> anyhow::Result<WatcherRegistrationId> {
        let path = SanitizedPath::from_arc(path);
        let key = WatchKey::for_registration(&path, case_insensitive);
        let mut state = self.state.lock();
        let new_coverage = if state.paths.contains(&key) {
            None
        } else if let Some(ancestor) = state.paths.covering_ancestor(&path) {
            Some(Coverage::CoveredBy(ancestor))
        } else {
            self.watch(path.as_path())?;
            Some(Coverage::Watched { covered: BTreeSet::new() })
        };

        let id = state.last_registration;
        state.last_registration = WatcherRegistrationId(id.0 + 1);

        let registration_state = WatcherRegistrationState { callback: Arc::new(cb), key: key.clone() };
        state.watchers.insert(id, registration_state);
        match new_coverage {
            None => {
                if let Some(registration) = state.paths.get_mut(&key) {
                    registration.watcher_ids.push(id);
                }
            }
            Some(coverage) => {
                if let Coverage::CoveredBy(ancestor) = &coverage
                    && let Some(covered) = state.paths.covered_by_mut(ancestor)
                {
                    covered.insert((path_depth(&path), key.clone()));
                }
                state.paths.0.insert(key, PathRegistrationState { path, watcher_ids: vec![id], coverage });
            }
        }

        Ok(id)
    }

    pub fn remove(&self, id: WatcherRegistrationId) {
        let mut state = self.state.lock();
        let Some(registration) = state.watchers.remove(&id) else {
            return;
        };
        let key = registration.key;
        let Some(path_state) = state.paths.get_mut(&key) else {
            return;
        };
        path_state.watcher_ids.retain(|&existing| existing != id);
        if !path_state.watcher_ids.is_empty() {
            return;
        }
        match &path_state.coverage {
            Coverage::Watched { .. } => self.release_watch(&mut state, key),
            Coverage::CoveredBy(ancestor) => {
                let ancestor = ancestor.clone();
                let covered_entry = (path_depth(&path_state.path), key);
                state.paths.0.remove(&covered_entry.1);
                let Some(ancestor_state) = state.paths.get_mut(&ancestor) else {
                    return;
                };
                let retained = ancestor_state.watcher_ids.is_empty();
                let Coverage::Watched { covered } = &mut ancestor_state.coverage else {
                    return;
                };
                // A retained watch stopped at its shallowest covered registration,
                // so only that registration leaving can unblock its release.
                let blocked_release = covered.first() == Some(&covered_entry);
                covered.remove(&covered_entry);
                if retained && blocked_release {
                    self.release_watch(&mut state, ancestor);
                }
            }
        }
    }

    /// Unwatches the unused backend watch at `key` once every registration it
    /// covers is served by another watch.
    ///
    /// Replacements are installed shallowest first, so one can cover deeper
    /// registrations. If one cannot be installed, the watch is retained for
    /// the registrations still covered by it, and is reconsidered when the one
    /// that could not be promoted is removed.
    fn release_watch(&self, state: &mut WatcherState, key: WatchKey) {
        // Exclude this watch from coverage checks while it stays installed.
        let Some(mut released) = state.paths.0.remove(&key) else {
            return;
        };
        let mut pending = match &mut released.coverage {
            Coverage::Watched { covered } => mem::take(covered),
            Coverage::CoveredBy(_) => {
                state.paths.0.insert(key, released);
                return;
            }
        };
        while let Some((depth, covered_key)) = pending.pop_first() {
            let Some(covered_path) = state.paths.0.get(&covered_key).map(|registration| registration.path.clone())
            else {
                continue;
            };
            let coverage = match state.paths.covering_ancestor(&covered_path) {
                Some(ancestor) => {
                    if let Some(covered) = state.paths.covered_by_mut(&ancestor) {
                        covered.insert((depth, covered_key.clone()));
                    }
                    Coverage::CoveredBy(ancestor)
                }
                None if self.watch_replacement(&covered_path) => Coverage::Watched { covered: BTreeSet::new() },
                None => {
                    pending.insert((depth, covered_key));
                    released.coverage = Coverage::Watched { covered: pending };
                    state.paths.0.insert(key, released);
                    return;
                }
            };
            if let Some(registration) = state.paths.get_mut(&covered_key) {
                registration.coverage = coverage;
            }
        }

        // Forget the watch even if unwatching fails: notify can report failure
        // after dropping the watch (FSEvents restarts its stream afterwards), so
        // a later registration of this path must install a fresh watch.
        self.unwatch(released.path.as_path()).log_err();
    }

    fn watch_replacement(&self, path: &SanitizedPath) -> bool {
        match self.watch(path.as_path()) {
            Ok(()) => true,
            Err(error) => {
                log::error!("failed to restore watch for {path:?}; retaining its covering watch: {error:#}");
                false
            }
        }
    }

    fn watch(&self, path: &Path) -> anyhow::Result<()> {
        self.ensure_backend()?;
        self.backend
            .lock()
            .as_mut()
            .expect("watcher backend initialized")
            .watch(path, notify::RecursiveMode::Recursive)?;
        Ok(())
    }

    fn unwatch(&self, path: &Path) -> anyhow::Result<()> {
        let watcher = self.backend.lock().as_mut().map(|watcher| watcher.unwatch(path));

        match watcher {
            // A watch may already be gone (e.g. its directory was deleted); either
            // way the path is no longer watched, which is all we wanted.
            Some(Err(error)) if !matches!(error.kind, notify::ErrorKind::WatchNotFound) => Err(error.into()),
            _ => Ok(()),
        }
    }

    pub(crate) fn ensure_backend(&self) -> anyhow::Result<()> {
        // The lock is held across creation: with a check-then-insert under two
        // separate lock acquisitions, concurrent callers could each create a
        // watcher and the loser's insert would silently drop the winner's
        // watcher along with every path registered on it.
        let mut backend = self.backend.lock();
        if backend.is_none() {
            // CORE excludes Access events, which ed discards anyway.
            let config = notify::Config::default().with_event_kinds(notify::EventKindMask::CORE);
            *backend = Some(Box::new(<notify::RecommendedWatcher as notify::Watcher>::new(self.event_sink(), config)?));
        }
        Ok(())
    }
}

fn dispatch(state: &Mutex<WatcherState>, event: notify::Result<notify::Event>) {
    let event = match event {
        Ok(event) => event,
        Err(error) => {
            log::warn!("file watcher error: {error}");
            return;
        }
    };

    log::trace!("handle file watcher event: {event:?}");

    let callbacks = {
        let state = state.lock();
        if event.need_rescan() {
            state.watchers.values().map(|registration| registration.callback.clone()).collect::<Vec<_>>()
        } else {
            let mut ids = Vec::new();
            for path in &event.paths {
                let sanitized = SanitizedPath::new(path);
                state.paths.watcher_ids_covering(sanitized, &mut ids);
            }
            ids.sort_unstable_by_key(|id| id.0);
            ids.dedup();
            ids.into_iter()
                .filter_map(|id| state.watchers.get(&id))
                .map(|registration| registration.callback.clone())
                .collect::<Vec<_>>()
        }
    };

    for callback in callbacks {
        callback(&event);
    }
}

const RESCAN_PATH_HISTORY_CAPACITY: usize = 10;

#[derive(Default)]
struct RescanHistory {
    paths: VecDeque<PathBuf>,
}

impl RescanHistory {
    fn record(&mut self, event: &notify::Event) -> Option<String> {
        let report = event.need_rescan().then(|| {
            // Capture before inserting the rescan so the report includes all ten
            // preceding paths, even when this is the start of an overflow burst.
            format!(
                "reason={:?}, paths={:?}, recent_paths={:?}",
                event.info().unwrap_or("unspecified"),
                event.paths,
                self.paths,
            )
        });
        let first_path = event.paths.len().saturating_sub(RESCAN_PATH_HISTORY_CAPACITY);
        for path in event.paths.iter().skip(first_path) {
            if self.paths.len() == RESCAN_PATH_HISTORY_CAPACITY {
                self.paths.pop_front();
            }
            self.paths.push_back(path.clone());
        }
        report
    }
}

fn dispatch_batch(
    state: &Mutex<WatcherState>,
    rescan_history: &mut RescanHistory,
    first: notify::Result<notify::Event>,
    event_rx: &async_channel::Receiver<notify::Result<notify::Event>>,
) {
    // A single backend overflow can enqueue many pathless rescan markers, each
    // of which rescans every watched root, so one covers the entire
    // drained batch. Rescans that name paths only cover those subtrees and are
    // all dispatched; ordinary events still run.
    let mut rescan_dispatched = false;

    for event in iter::once(first).chain(iter::from_fn(|| event_rx.try_recv().ok())) {
        if event.as_ref().is_ok_and(|event| event.need_rescan() && event.paths.is_empty()) {
            if rescan_dispatched {
                continue;
            }
            rescan_dispatched = true;
        }

        // Log writes can block, so keep them off the native watcher reader.
        if let Ok(event) = &event
            && let Some(report) = rescan_history.record(event)
        {
            log::error!("filesystem watcher requested rescan: {report}");
        }
        dispatch(state, event);
    }
}

fn enqueue(event_tx: &async_channel::Sender<notify::Result<notify::Event>>, event: notify::Result<notify::Event>) {
    if matches!(event, Ok(Event { kind: EventKind::Access(_), .. })) {
        return;
    }

    // A failed send only happens once the dispatch thread has shut down, at
    // which point there's nothing left to dispatch to.
    event_tx.try_send(event).ok();
}
