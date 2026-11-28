use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::collections::btree_map::Entry;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_channel::Sender;
use futures::Stream;
use futures::StreamExt as _;
use gpui::BackgroundExecutor;
use gpui::Task;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use util::ResultExt as _;
use util::paths::SanitizedPath;

use crate::Fs;
use crate::PathEvent;
use crate::PathEventKind;
use crate::Watcher;
use crate::fsevents;

/// How often a watch on a not-yet-existing path is retried.
const PENDING_PATH_RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) async fn watch(
    fs: Arc<Fs>,
    native_watcher: Arc<OsWatcher>,
    executor: BackgroundExecutor,
    path: &Path,
    latency: Duration,
) -> (Pin<Box<dyn Send + Stream<Item = Vec<PathEvent>>>>, Arc<dyn Watcher>) {
    let (tx, rx) = async_channel::unbounded();
    let pending_paths: Arc<Mutex<Vec<PathEvent>>> = Arc::default();

    let watcher: Arc<dyn Watcher> =
        Arc::new(FsWatcher::new(native_watcher, Arc::clone(&fs), executor.clone(), tx, Arc::clone(&pending_paths)));

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
            let watcher = Arc::clone(&watcher);
            move |()| {
                _ = Arc::clone(&watcher);
                let pending_paths = Arc::clone(&pending_paths);
                let executor = executor.clone();
                async move {
                    if !latency.is_zero() {
                        executor.timer(latency).await;
                    }
                    let paths = mem::take(&mut *pending_paths.lock());
                    log::debug!("pending path events: {paths:?}");
                    (!paths.is_empty()).then_some(paths)
                }
            }
        })),
        watcher,
    )
}

struct FsWatcher {
    native_watcher: Arc<OsWatcher>,
    fs: Arc<Fs>,
    executor: BackgroundExecutor,
    tx: Sender<()>,
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
    fn new(
        native_watcher: Arc<OsWatcher>,
        fs: Arc<Fs>,
        executor: BackgroundExecutor,
        tx: Sender<()>,
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
            Arc::clone(&self.pending_path_events),
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
            Arc::clone(&self.native_watcher),
            Arc::clone(&self.fs),
            self.executor.clone(),
            Arc::clone(&path),
            self.tx.clone(),
            Arc::clone(&self.pending_path_events),
            Arc::clone(&self.registrations),
            Arc::clone(&self.pending_registrations),
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
    tx: Sender<()>,
    pending_path_events: Arc<Mutex<Vec<PathEvent>>>,
) -> anyhow::Result<FsWatcherRegistration> {
    let root_path = SanitizedPath::new_arc(path.as_ref());
    let path_for_callback = Arc::clone(&path);
    let registration_id = native_watcher.add(path, case_insensitive, move |event: &fsevents::Event| {
        log::trace!("watcher received event: {event:?}");
        push_fs_event(&tx, &pending_path_events, &root_path, case_insensitive, path_for_callback.as_ref(), event);
    })?;
    Ok(FsWatcherRegistration { id: registration_id, os_watcher: Arc::clone(native_watcher) })
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
        Self::Folded(Self::folded_path(path).into())
    }

    fn folded_path(path: &SanitizedPath) -> String {
        // Event ancestors omit trailing separators, even when registration paths include them.
        let normalized_path = path.as_path().components().collect::<PathBuf>();
        let lossy = normalized_path.to_string_lossy();
        // APFS/HFS+ compare names normalization-insensitively (NFC vs NFD), and
        // FSEvents can report NFD while a config/LSP supplies NFC, so normalize
        // before folding case.
        use unicode_normalization::UnicodeNormalization as _;
        if lossy.is_ascii() {
            return lossy.to_ascii_lowercase();
        }
        lossy.chars().nfc().collect::<String>().to_lowercase()
    }

    fn for_registration(path: &SanitizedPath, case_insensitive: bool) -> Self {
        if case_insensitive { Self::folded(path) } else { Self::exact(path) }
    }
}

async fn poll_path_until_created(
    native_watcher: Arc<OsWatcher>,
    fs: Arc<Fs>,
    executor: BackgroundExecutor,
    path: Arc<Path>,
    tx: Sender<()>,
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
        let case_insensitive = async_macos::unblock({
            let path = Arc::clone(&path);
            let fs = Arc::clone(&fs);
            move || !fs.is_path_case_sensitive(&path)
        })
        .await;
        let key = WatchKey::for_registration(SanitizedPath::new(&path), case_insensitive);

        if registrations.lock().contains_key(&key) {
            pending_registrations.lock().remove(path.as_ref());
            return;
        }

        let register = {
            let path = Arc::clone(&path);
            let tx = tx.clone();
            let pending_path_events = Arc::clone(&pending_path_events);
            let native_watcher = Arc::clone(&native_watcher);
            move || register_existing_path(&native_watcher, path, case_insensitive, tx, pending_path_events)
        };
        let registration = async_macos::unblock(register).await;

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

fn push_fs_event(
    tx: &Sender<()>,
    pending_path_events: &Arc<Mutex<Vec<PathEvent>>>,
    root_path: &SanitizedPath,
    case_insensitive: bool,
    watched_root: &Path,
    event: &fsevents::Event,
) {
    let event_path = SanitizedPath::new(&event.path);
    let under_root = path_is_under(event_path, root_path, case_insensitive);
    let mut path_events = Vec::new();
    if event.must_rescan() {
        // A rescan names the subtree that lost sync; one outside this root
        // (rescans go to every watcher) makes the whole watched root suspect.
        let path = if under_root { event_path.as_path() } else { watched_root };
        path_events.push(PathEvent { path: path.to_path_buf(), kind: Some(PathEventKind::Rescan) });
    } else if under_root {
        path_events
            .extend(event.kinds().map(|kind| PathEvent { path: event_path.as_path().to_path_buf(), kind: Some(kind) }));
    }
    log::trace!("path_events: {path_events:?}");
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
struct WatcherRegistrationId(u32);

struct WatcherRegistrationState {
    callback: Arc<dyn Fn(&fsevents::Event) + Send + Sync>,
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
struct WatchPaths {
    entries: FxHashMap<WatchKey, PathRegistrationState>,
    /// Registration count per component depth, so dispatch probes only ancestors that can match.
    exact_depths: BTreeMap<usize, usize>,
    folded_depths: BTreeMap<usize, usize>,
}

impl WatchPaths {
    fn depths_mut(&mut self, key: &WatchKey) -> (usize, &mut BTreeMap<usize, usize>) {
        match key {
            WatchKey::Exact(path) => (path.components().count(), &mut self.exact_depths),
            WatchKey::Folded(path) => (Path::new(path.as_ref()).components().count(), &mut self.folded_depths),
        }
    }

    fn insert(&mut self, key: WatchKey, registration: PathRegistrationState) {
        let (depth, depths) = self.depths_mut(&key);
        *depths.entry(depth).or_default() += 1;
        if self.entries.insert(key.clone(), registration).is_some() {
            self.forget_depth(&key);
        }
    }

    fn remove(&mut self, key: &WatchKey) -> Option<PathRegistrationState> {
        let registration = self.entries.remove(key)?;
        self.forget_depth(key);
        Some(registration)
    }

    fn forget_depth(&mut self, key: &WatchKey) {
        let (depth, depths) = self.depths_mut(key);
        if let Entry::Occupied(mut entry) = depths.entry(depth) {
            *entry.get_mut() -= 1;
            if *entry.get() == 0 {
                entry.remove();
            }
        }
    }

    fn contains(&self, key: &WatchKey) -> bool {
        self.entries.contains_key(key)
    }

    fn get_mut(&mut self, key: &WatchKey) -> Option<&mut PathRegistrationState> {
        self.entries.get_mut(key)
    }

    /// The nearest strict ancestor whose recursive backend watch covers `path`.
    fn covering_ancestor(&self, path: &SanitizedPath) -> Option<WatchKey> {
        path.as_path().ancestors().skip(1).find_map(|ancestor| {
            let ancestor = SanitizedPath::unchecked_new(ancestor);
            [WatchKey::exact(ancestor), WatchKey::folded(ancestor)].into_iter().find(|key| {
                self.entries
                    .get(key)
                    .is_some_and(|registration| matches!(registration.coverage, Coverage::Watched { .. }))
            })
        })
    }

    fn covered_by_mut(&mut self, key: &WatchKey) -> Option<&mut BTreeSet<(usize, WatchKey)>> {
        match &mut self.entries.get_mut(key)?.coverage {
            Coverage::Watched { covered } => Some(covered),
            Coverage::CoveredBy(_) => None,
        }
    }

    /// Collects the watcher ids of every registration whose directory is an
    /// ancestor of (or equal to) `path`. Both exact and folded keys are probed,
    /// so a real-cased event path matches a folded registration and vice versa.
    fn watcher_ids_covering(&self, path: &SanitizedPath, ids: &mut Vec<WatcherRegistrationId>) {
        for ancestor in ancestors_at_depths(path.as_path(), &self.exact_depths) {
            if let Some(registration) = self.entries.get(&WatchKey::exact(SanitizedPath::unchecked_new(ancestor))) {
                ids.extend_from_slice(&registration.watcher_ids);
            }
        }
        // Fold once, from the deepest ancestor a folded registration can match.
        if let Some(ancestor) = ancestors_at_depths(path.as_path(), &self.folded_depths).next() {
            let folded_path = WatchKey::folded_path(SanitizedPath::unchecked_new(ancestor));
            for ancestor in ancestors_at_depths(Path::new(&folded_path), &self.folded_depths) {
                let key = WatchKey::Folded(Arc::from(ancestor.to_string_lossy().as_ref()));
                if let Some(registration) = self.entries.get(&key) {
                    ids.extend_from_slice(&registration.watcher_ids);
                }
            }
        }
    }
}

/// The ancestors of `path` (itself included) whose component count is a key of `depths`, deepest first.
fn ancestors_at_depths<'a>(path: &'a Path, depths: &'a BTreeMap<usize, usize>) -> impl Iterator<Item = &'a Path> {
    let path_depth = path.components().count();
    let mut ancestor_depth = Some(path_depth);
    let mut ancestors = path.ancestors();
    depths.range(..=path_depth).rev().filter_map(move |(&depth, _)| {
        let ancestor = ancestors.nth(ancestor_depth? - depth)?;
        ancestor_depth = depth.checked_sub(1);
        Some(ancestor)
    })
}

const MAX_DISPATCH_BATCH_SIZE: usize = 256;

/// Returns `Pending` once, after waking itself, so the executor runs other tasks first.
async fn yield_now() {
    let mut yielded = false;
    futures::future::poll_fn(|cx| {
        if yielded {
            return std::task::Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await;
}

struct WatcherState {
    watchers: HashMap<WatcherRegistrationId, WatcherRegistrationState>,
    paths: WatchPaths,
    last_registration: WatcherRegistrationId,
}

pub(crate) struct OsWatcher {
    state: Arc<Mutex<WatcherState>>,

    // One FSEvents stream per backend watch. Locked while holding `state`;
    // stream callbacks take neither lock.
    streams: Mutex<HashMap<PathBuf, fsevents::Stream>>,
    event_tx: Sender<fsevents::Event>,
    _dispatch_task: Task<()>,
    // Closing the event channel can wake the dispatch task. Keep its scheduler
    // alive through channel and task teardown, even when our owner drops first.
    _executor: BackgroundExecutor,
}

impl OsWatcher {
    pub(crate) fn new(executor: BackgroundExecutor) -> Arc<Self> {
        let (event_tx, event_rx) = async_channel::unbounded();
        let state = Arc::new(Mutex::new(WatcherState {
            watchers: HashMap::default(),
            paths: WatchPaths::default(),
            last_registration: WatcherRegistrationId::default(),
        }));
        // Dispatch (and its logging, which can block) runs here rather than on
        // the FSEvents queues.
        let dispatch_task = executor.spawn({
            let state = Arc::clone(&state);
            async move {
                let mut rescan_history = RescanHistory::default();
                let mut dispatched = 0_usize;
                while let Ok(event) = event_rx.recv().await {
                    if let Some(report) = rescan_history.record(&event) {
                        log::error!("filesystem watcher requested rescan: {report}");
                    }
                    dispatch(&state, &event);
                    // `recv` doesn't yield while events are queued, and FSEvents can produce
                    // them faster than we dispatch, so give other tasks a turn now and then.
                    dispatched += 1;
                    if dispatched.is_multiple_of(MAX_DISPATCH_BATCH_SIZE) {
                        yield_now().await;
                    }
                }
            }
        });
        Arc::new(Self {
            state,
            streams: Mutex::default(),
            event_tx,
            _dispatch_task: dispatch_task,
            _executor: executor,
        })
    }

    fn add(
        &self,
        path: Arc<Path>,
        case_insensitive: bool,
        cb: impl Fn(&fsevents::Event) + Send + Sync + 'static,
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
                state.paths.insert(key, PathRegistrationState { path, watcher_ids: vec![id], coverage });
            }
        }

        Ok(id)
    }

    fn remove(&self, id: WatcherRegistrationId) {
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
                state.paths.remove(&covered_entry.1);
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
        let Some(mut released) = state.paths.remove(&key) else {
            return;
        };
        let mut pending = match &mut released.coverage {
            Coverage::Watched { covered } => mem::take(covered),
            Coverage::CoveredBy(_) => {
                state.paths.insert(key, released);
                return;
            }
        };
        while let Some((depth, covered_key)) = pending.pop_first() {
            let Some(covered_path) =
                state.paths.entries.get(&covered_key).map(|registration| Arc::clone(&registration.path))
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
                    state.paths.insert(key, released);
                    return;
                }
            };
            if let Some(registration) = state.paths.get_mut(&covered_key) {
                registration.coverage = coverage;
            }
        }

        self.streams.lock().remove(released.path.as_path());
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
        let stream = fsevents::Stream::new(path, self.event_tx.clone())?;
        self.streams.lock().insert(path.to_path_buf(), stream);
        Ok(())
    }
}

fn dispatch(state: &Mutex<WatcherState>, event: &fsevents::Event) {
    log::trace!("handle file watcher event: {event:?}");

    let callbacks = {
        let state = state.lock();
        if event.must_rescan() {
            state.watchers.values().map(|registration| Arc::clone(&registration.callback)).collect::<Vec<_>>()
        } else {
            let mut ids = Vec::new();
            state.paths.watcher_ids_covering(SanitizedPath::new(&event.path), &mut ids);
            ids.into_iter()
                .filter_map(|id| state.watchers.get(&id))
                .map(|registration| Arc::clone(&registration.callback))
                .collect::<Vec<_>>()
        }
    };

    for callback in callbacks {
        callback(event);
    }
}

const RESCAN_PATH_HISTORY_CAPACITY: usize = 10;

#[derive(Default)]
struct RescanHistory {
    paths: VecDeque<PathBuf>,
}

impl RescanHistory {
    fn record(&mut self, event: &fsevents::Event) -> Option<String> {
        // Capture before inserting the rescan so the report includes all ten
        // preceding paths.
        let report = event.must_rescan().then(|| {
            format!("reason={:?}, path={:?}, recent_paths={:?}", event.rescan_reason(), event.path, self.paths)
        });
        if self.paths.len() == RESCAN_PATH_HISTORY_CAPACITY {
            self.paths.pop_front();
        }
        self.paths.push_back(event.path.clone());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsevents::ITEM_CREATED;
    use crate::fsevents::ITEM_MODIFIED;
    use crate::fsevents::ITEM_REMOVED;
    use crate::fsevents::MUST_SCAN_SUBDIRS;

    fn event(path: &str, flags: u32) -> fsevents::Event {
        fsevents::Event { path: PathBuf::from(path), flags }
    }

    fn path_event(path: &str, kind: PathEventKind) -> PathEvent {
        PathEvent { path: PathBuf::from(path), kind: Some(kind) }
    }

    /// Runs `push_fs_event` for a watch on `/root`, returning the pending events
    /// and whether the stream was signalled.
    fn push(event: &fsevents::Event) -> (Vec<PathEvent>, bool) {
        let (tx, rx) = async_channel::unbounded();
        let pending = Arc::default();
        push_fs_event(&tx, &pending, SanitizedPath::new("/root"), false, Path::new("/root"), event);
        let events = mem::take(&mut *pending.lock());
        (events, rx.try_recv().is_ok())
    }

    #[test]
    fn push_fs_event_maps_kinds_under_the_root() {
        assert_eq!(push(&event("/root/a", ITEM_MODIFIED)), (vec![path_event("/root/a", PathEventKind::Changed)], true));
        // Coalesced flags collapse to one pending event per path; removal wins.
        assert_eq!(
            push(&event("/root/a", ITEM_CREATED | ITEM_MODIFIED | ITEM_REMOVED)),
            (vec![path_event("/root/a", PathEventKind::Removed)], true)
        );
        assert_eq!(push(&event("/other/a", ITEM_MODIFIED)), (vec![], false));
    }

    #[test]
    fn push_fs_event_rescans_the_subtree_or_the_whole_root() {
        assert_eq!(
            push(&event("/root/a", MUST_SCAN_SUBDIRS | ITEM_MODIFIED)),
            (vec![path_event("/root/a", PathEventKind::Rescan)], true)
        );
        assert_eq!(
            push(&event("/other", MUST_SCAN_SUBDIRS | ITEM_MODIFIED)),
            (vec![path_event("/root", PathEventKind::Rescan)], true)
        );
    }

    #[test]
    fn yield_now_is_pending_once() {
        let mut future = std::pin::pin!(yield_now());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn folded_path_lowercases_ascii_and_normalizes_unicode() {
        let folded = |path: &str| WatchKey::folded_path(SanitizedPath::new(path));
        assert_eq!(folded("/Root/Dir/File.TXT"), "/root/dir/file.txt");
        // Trailing separators and `.` components are dropped on the ASCII path too.
        assert_eq!(folded("/Root/./Dir/"), "/root/dir");
        assert_eq!(folded("/Root/CAFE\u{301}"), "/root/caf\u{e9}");
        assert_eq!(folded("/Root/\u{c9}COLE"), "/root/\u{e9}cole");
    }

    #[test]
    fn watcher_ids_covering_matches_exact_and_folded_ancestors() {
        let mut paths = WatchPaths::default();
        let mut register = |id: u32, key: WatchKey, path: &str| {
            paths.insert(
                key,
                PathRegistrationState {
                    path: SanitizedPath::new_arc(path),
                    watcher_ids: vec![WatcherRegistrationId(id)],
                    coverage: Coverage::Watched { covered: BTreeSet::new() },
                },
            );
        };
        register(0, WatchKey::exact(SanitizedPath::new("/Root")), "/Root");
        // NFC "é", registered case-insensitively.
        register(1, WatchKey::folded(SanitizedPath::new("/Root/Caf\u{e9}/")), "/Root/Caf\u{e9}/");
        register(2, WatchKey::folded(SanitizedPath::new("/Other")), "/Other");

        let covering = |paths: &WatchPaths, path: &str| {
            let mut ids = Vec::new();
            paths.watcher_ids_covering(SanitizedPath::new(path), &mut ids);
            let mut ids = ids.into_iter().map(|id| id.0).collect::<Vec<_>>();
            ids.sort_unstable();
            ids
        };
        // FSEvents reports NFD and real casing.
        assert_eq!(covering(&paths, "/Root/CAFE\u{301}/file.txt"), [0, 1]);
        assert_eq!(covering(&paths, "/Root/file.txt"), [0]);
        assert_eq!(covering(&paths, "/root/file.txt"), [] as [u32; 0]);
        assert_eq!(covering(&paths, "/OTHER/x"), [2]);
        assert_eq!(covering(&paths, "/Root/CAFE\u{301}/a/b/c/d/file.txt"), [0, 1]);
        assert_eq!(covering(&paths, "/"), [] as [u32; 0]);

        assert_eq!(paths.exact_depths, BTreeMap::from([(2, 1)]));
        assert_eq!(paths.folded_depths, BTreeMap::from([(2, 1), (3, 1)]));
        paths.remove(&WatchKey::folded(SanitizedPath::new("/Other")));
        paths.remove(&WatchKey::folded(SanitizedPath::new("/Other")));
        assert_eq!(paths.folded_depths, BTreeMap::from([(3, 1)]));
        assert_eq!(covering(&paths, "/OTHER/x"), [] as [u32; 0]);
    }

    #[test]
    fn ancestors_at_depths_visits_only_registered_depths() {
        let depths = BTreeMap::from([(1, 1), (3, 2), (9, 1)]);
        let ancestors = |path: &'static str| ancestors_at_depths(Path::new(path), &depths).collect::<Vec<_>>();
        assert_eq!(ancestors("/a/b/c/d"), [Path::new("/a/b"), Path::new("/")]);
        assert_eq!(ancestors("/a/b"), [Path::new("/a/b"), Path::new("/")]);
        assert_eq!(ancestors("/a"), [Path::new("/")]);
        assert!(ancestors_at_depths(Path::new("/a/b"), &BTreeMap::new()).next().is_none());
    }

    #[test]
    fn dispatch_routes_by_ancestor_and_broadcasts_rescans() {
        let called: Arc<Mutex<Vec<u32>>> = Arc::default();
        let mut state = WatcherState {
            watchers: HashMap::default(),
            paths: WatchPaths::default(),
            last_registration: WatcherRegistrationId::default(),
        };
        for (id, path) in ["/a", "/a/b", "/c"].into_iter().enumerate() {
            let id = WatcherRegistrationId(id as u32);
            let path = SanitizedPath::new_arc(path);
            let key = WatchKey::exact(&path);
            let called = Arc::clone(&called);
            state.watchers.insert(
                id,
                WatcherRegistrationState { callback: Arc::new(move |_| called.lock().push(id.0)), key: key.clone() },
            );
            state.paths.insert(
                key,
                PathRegistrationState {
                    path,
                    watcher_ids: vec![id],
                    coverage: Coverage::Watched { covered: BTreeSet::new() },
                },
            );
        }
        let state = Mutex::new(state);
        let dispatched = |event: fsevents::Event| {
            dispatch(&state, &event);
            let mut ids = mem::take(&mut *called.lock());
            ids.sort_unstable();
            ids
        };

        assert_eq!(dispatched(event("/a/b/file", ITEM_MODIFIED)), [0, 1]);
        assert_eq!(dispatched(event("/a/file", ITEM_MODIFIED)), [0]);
        assert_eq!(dispatched(event("/elsewhere", ITEM_MODIFIED)), [] as [u32; 0]);
        assert_eq!(dispatched(event("/c", MUST_SCAN_SUBDIRS)), [0, 1, 2]);
    }

    #[test]
    fn rescan_history_reports_recent_paths() {
        let mut history = RescanHistory::default();
        for i in 0..RESCAN_PATH_HISTORY_CAPACITY + 2 {
            assert_eq!(history.record(&event(&format!("/root/{i}"), ITEM_MODIFIED)), None);
        }
        assert_eq!(history.paths.len(), RESCAN_PATH_HISTORY_CAPACITY);
        assert_eq!(history.paths.front(), Some(&PathBuf::from("/root/2")));

        let report = history.record(&event("/root", MUST_SCAN_SUBDIRS)).unwrap();
        assert!(report.contains("reason=\"unspecified\""), "{report}");
        assert!(report.contains("path=\"/root\""), "{report}");
        assert!(report.contains("/root/11") && !report.contains("/root/1\""), "{report}");
    }
}
