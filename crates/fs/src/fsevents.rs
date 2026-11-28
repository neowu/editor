//! Minimal FSEvents binding: one recursive, file-level stream per watched root,
//! delivered on its own serial dispatch queue.

use std::ffi::CStr;
use std::ffi::OsStr;
use std::ffi::c_char;
use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;

use anyhow::Context as _;
use async_channel::Sender;
use dispatch2::DispatchQueue;
use dispatch2::DispatchQueueAttr;
use dispatch2::DispatchRetained;
use objc2_core_foundation::CFArray;
use objc2_core_foundation::CFString;

use crate::PathEventKind;

// kFSEventStreamEventFlag*
pub(crate) const MUST_SCAN_SUBDIRS: u32 = 0x1;
const USER_DROPPED: u32 = 0x2;
const KERNEL_DROPPED: u32 = 0x4;
const HISTORY_DONE: u32 = 0x10;
const MOUNT: u32 = 0x40;
const UNMOUNT: u32 = 0x80;
pub(crate) const ITEM_CREATED: u32 = 0x100;
pub(crate) const ITEM_REMOVED: u32 = 0x200;
const ITEM_INODE_META_MOD: u32 = 0x400;
const ITEM_RENAMED: u32 = 0x800;
pub(crate) const ITEM_MODIFIED: u32 = 0x1000;
const ITEM_FINDER_INFO_MOD: u32 = 0x2000;
const ITEM_CHANGE_OWNER: u32 = 0x4000;
const ITEM_XATTR_MOD: u32 = 0x8000;

const CREATED: u32 = ITEM_CREATED | MOUNT;
const REMOVED: u32 = ITEM_REMOVED | UNMOUNT;
// FSEvents cannot pair the two sides of a rename, so each side is just a change.
const CHANGED: u32 =
    ITEM_RENAMED | ITEM_MODIFIED | ITEM_INODE_META_MOD | ITEM_FINDER_INFO_MOD | ITEM_CHANGE_OWNER | ITEM_XATTR_MOD;

// kFSEventStreamCreateFlagNoDefer | kFSEventStreamCreateFlagFileEvents
const CREATE_FLAGS: u32 = 0x2 | 0x10;
// kFSEventStreamEventIdSinceNow
const SINCE_NOW: u64 = u64::MAX;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type FSEventStreamCallback = unsafe extern "C" fn(
    stream: *const c_void,
    info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const u32,
    event_ids: *const u64,
);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths_to_watch: &CFArray<CFString>,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: &DispatchQueue);
    fn FSEventStreamStart(stream: *mut c_void) -> bool;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);
}

/// One changed path with its raw (possibly coalesced) FSEvents flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) path: PathBuf,
    pub(crate) flags: u32,
}

impl Event {
    /// The subtree at `path` lost sync and must be rescanned.
    pub(crate) const fn must_rescan(&self) -> bool {
        self.flags & MUST_SCAN_SUBDIRS != 0
    }

    pub(crate) const fn rescan_reason(&self) -> &'static str {
        if self.flags & USER_DROPPED != 0 {
            "user dropped"
        } else if self.flags & KERNEL_DROPPED != 0 {
            "kernel dropped"
        } else {
            "unspecified"
        }
    }

    /// FSEvents coalesces changes to one path, so a single event can carry
    /// several kinds.
    pub(crate) fn kinds(&self) -> impl Iterator<Item = PathEventKind> + use<> {
        let flags = self.flags;
        [(CREATED, PathEventKind::Created), (CHANGED, PathEventKind::Changed), (REMOVED, PathEventKind::Removed)]
            .into_iter()
            .filter_map(move |(mask, kind)| (flags & mask != 0).then_some(kind))
    }
}

struct StreamContext {
    requested: PathBuf,
    canonical: PathBuf,
    tx: Sender<Event>,
}

/// A running recursive watch on one directory tree; dropping it stops the watch.
pub(crate) struct Stream {
    stream: *mut c_void,
    queue: DispatchRetained<DispatchQueue>,
    context: *mut StreamContext,
}

// SAFETY: the stream is only touched on creation and on drop, and the context
// is only read by the callback.
unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

impl Stream {
    /// Watches `path` recursively, sending events to `tx`. Event paths are
    /// reported under `path` as given, even where it traverses symlinks.
    pub(crate) fn new(path: &Path, tx: Sender<Event>) -> anyhow::Result<Self> {
        let canonical = std::fs::canonicalize(path)?;
        let cf_path = CFString::from_str(canonical.to_str().context("watch path is not valid UTF-8")?);
        let paths = CFArray::from_retained_objects(&[cf_path]);
        let context = Box::into_raw(Box::new(StreamContext { requested: path.to_path_buf(), canonical, tx }));
        let stream_context = FSEventStreamContext {
            version: 0,
            info: context.cast(),
            retain: ptr::null(),
            release: ptr::null(),
            copy_description: ptr::null(),
        };
        let queue = DispatchQueue::new("ed.fsevents", DispatchQueueAttr::SERIAL);
        // SAFETY: `context` outlives the stream (freed in `drop` after the stream
        // is released), and the stream is started only after it has a queue.
        let started = unsafe {
            let stream = FSEventStreamCreate(
                ptr::null(),
                callback,
                &raw const stream_context,
                &paths,
                SINCE_NOW,
                0.0,
                CREATE_FLAGS,
            );
            if stream.is_null() {
                None
            } else {
                FSEventStreamSetDispatchQueue(stream, &queue);
                if FSEventStreamStart(stream) {
                    Some(stream)
                } else {
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                    None
                }
            }
        };
        let Some(stream) = started else {
            // SAFETY: no stream refers to `context` any more.
            drop(unsafe { Box::from_raw(context) });
            anyhow::bail!("failed to start FSEvents stream for {path:?}");
        };
        Ok(Self { stream, queue, context })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let this = &*self;
        // Tear down on the stream's own serial queue so no callback is running
        // or can run afterwards, which makes freeing the context safe.
        this.queue.exec_sync(move || unsafe {
            FSEventStreamStop(this.stream);
            FSEventStreamInvalidate(this.stream);
            FSEventStreamRelease(this.stream);
        });
        // SAFETY: allocated by `Box::into_raw` in `new`; the stream is gone.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

unsafe extern "C" fn callback(
    _stream: *const c_void,
    info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const u32,
    _event_ids: *const u64,
) {
    // SAFETY: `info` is the `StreamContext` the stream was created with, and
    // FSEvents passes `num_events` C-string paths and flags.
    let context = unsafe { &*info.cast::<StreamContext>() };
    let event_paths = event_paths.cast::<*const c_char>();
    for i in 0..num_events {
        let flags = unsafe { *event_flags.add(i) };
        // `HISTORY_DONE` is a sentinel whose path is meaningless; anything
        // else without these bits (e.g. only an item-type flag) is not a change.
        if flags & HISTORY_DONE != 0 || flags & (MUST_SCAN_SUBDIRS | CREATED | CHANGED | REMOVED) == 0 {
            continue;
        }
        // Paths are not guaranteed to be UTF-8 (e.g. NFS), so keep the raw bytes.
        let path = Path::new(OsStr::from_bytes(unsafe { CStr::from_ptr(*event_paths.add(i)) }.to_bytes()));
        let path = reported_path(&context.canonical, &context.requested, path);
        // Fails only once the dispatch task is gone, when nobody is listening.
        context.tx.try_send(Event { path, flags }).ok();
    }
}

/// FSEvents reports canonical paths; map `path` back under the root as the
/// caller spelled it, so its prefix checks hold for symlinked roots.
fn reported_path(canonical: &Path, requested: &Path, path: &Path) -> PathBuf {
    match path.strip_prefix(canonical) {
        Ok(relative) if relative.as_os_str().is_empty() => requested.to_path_buf(),
        Ok(relative) => requested.join(relative),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;

    use async_channel::Receiver;

    use super::*;

    fn event(flags: u32) -> Event {
        Event { path: PathBuf::from("/a"), flags }
    }

    /// Waits for an event matching `predicate`, skipping unrelated ones.
    fn wait_for(rx: &Receiver<Event>, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => {}
                Err(_) => {
                    assert!(Instant::now() < deadline, "timed out waiting for an FSEvents event");
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    #[test]
    fn kinds_follow_flags() {
        const IS_FILE: u32 = 0x10000;
        assert_eq!(event(IS_FILE).kinds().count(), 0);
        assert_eq!(event(ITEM_CREATED | IS_FILE).kinds().collect::<Vec<_>>(), [PathEventKind::Created]);
        assert_eq!(event(MOUNT).kinds().collect::<Vec<_>>(), [PathEventKind::Created]);
        assert_eq!(event(UNMOUNT).kinds().collect::<Vec<_>>(), [PathEventKind::Removed]);
        for flag in
            [ITEM_RENAMED, ITEM_MODIFIED, ITEM_INODE_META_MOD, ITEM_FINDER_INFO_MOD, ITEM_CHANGE_OWNER, ITEM_XATTR_MOD]
        {
            assert_eq!(event(flag).kinds().collect::<Vec<_>>(), [PathEventKind::Changed]);
        }
        assert_eq!(
            event(ITEM_CREATED | ITEM_MODIFIED | ITEM_RENAMED | ITEM_REMOVED).kinds().collect::<Vec<_>>(),
            [PathEventKind::Created, PathEventKind::Changed, PathEventKind::Removed]
        );
    }

    #[test]
    fn rescan_flags() {
        assert!(!event(ITEM_MODIFIED).must_rescan());
        assert!(event(MUST_SCAN_SUBDIRS).must_rescan());
        assert_eq!(event(MUST_SCAN_SUBDIRS).rescan_reason(), "unspecified");
        assert_eq!(event(MUST_SCAN_SUBDIRS | USER_DROPPED).rescan_reason(), "user dropped");
        assert_eq!(event(MUST_SCAN_SUBDIRS | KERNEL_DROPPED).rescan_reason(), "kernel dropped");
    }

    #[test]
    fn reported_path_maps_canonical_prefix_to_requested() {
        let canonical = Path::new("/private/tmp/root");
        let requested = Path::new("/tmp/root");
        assert_eq!(reported_path(canonical, requested, canonical), requested);
        assert_eq!(reported_path(canonical, requested, Path::new("/private/tmp/root/a/b")), Path::new("/tmp/root/a/b"));
        assert_eq!(reported_path(canonical, requested, Path::new("/elsewhere/a")), Path::new("/elsewhere/a"));
    }

    #[test]
    fn stream_reports_changes_under_the_requested_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        unix::fs::symlink(&real, &link).unwrap();

        let (tx, rx) = async_channel::unbounded();
        let stream = Stream::new(&link, tx).unwrap();

        // Named through the symlink, although FSEvents reports the real path.
        let file = link.join("sub").join("file.txt");
        fs::create_dir_all(link.join("sub")).unwrap();
        fs::write(&file, "one").unwrap();
        let created = wait_for(&rx, |event| event.path == file);
        assert!(created.kinds().any(|kind| kind == PathEventKind::Created));

        fs::remove_file(&file).unwrap();
        wait_for(&rx, |event| event.path == file && event.kinds().any(|kind| kind == PathEventKind::Removed));

        // Dropping the stream stops delivery and releases its sender.
        drop(stream);
        while rx.try_recv().is_ok() {}
        fs::write(&file, "two").unwrap();
        thread::sleep(Duration::from_millis(200));
        rx.try_recv().unwrap_err();
        assert!(rx.is_closed());
    }

    #[test]
    fn stream_fails_for_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = async_channel::unbounded();
        assert!(Stream::new(&dir.path().join("missing"), tx).is_err());
    }
}
