use std::fmt::Debug;
use std::fs;
use std::fs::File;
use std::path;
use std::result;
use std::sync::Weak;
use std::thread::Builder;

use futures::channel::mpsc;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
pub mod fs_watcher;
mod fsevents;
mod git_clone_progress;

use std::ffi::CString;
use std::io;
use std::io::Write as _;
use std::mem::MaybeUninit;
use std::os::fd::AsFd as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::FileTypeExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use anyhow::Context as _;
use anyhow::Result;
use async_macos::command::Stdio;
use async_macos::command::new_command;
use async_macos::unblock;
use futures::Stream;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use futures::stream::iter;
use git::repository::GitRepository;
use git::repository::RealGitRepository;
use gpui::App;
use gpui::BackgroundExecutor;
use gpui::Global;
use gpui::ReadGlobal as _;
use gpui::SharedString;
use parking_lot::Mutex;
use rope::Rope;
use serde::Deserialize;
use serde::Serialize;
use slotmap::SlotMap;
use tempfile::TempDir;
use text::LineEnding;

pub trait Watcher: Send + Sync {
    fn add(&self, path: &Path) -> Result<()>;
    fn remove(&self, path: &Path) -> Result<()>;
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PathEventKind {
    Removed,
    Created,
    Changed,
    Rescan,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PathEvent {
    pub path: PathBuf,
    pub kind: Option<PathEventKind>,
}

impl From<PathEvent> for PathBuf {
    fn from(event: PathEvent) -> Self {
        event.path
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TrashRestoreError {
    #[error("The specified `path` ({}) was not found in the system's trash.", path.display())]
    NotFound { path: PathBuf },
    #[error("File or directory ({}) already exists at the restore destination.", path.display())]
    Collision { path: PathBuf },
    // This should never occur, the only way to get a TrashId is to undo
    // consumes the Change::Trashed. We worry about remoting duplicate messages
    // we do not want to crash the app then which is why this error is there.
    #[error("The item was already restored")]
    AlreadyRestored,
    #[error("Unknown error ({description})")]
    Unknown { description: String },
}

impl From<trash::TrashError> for TrashRestoreError {
    fn from(err: trash::TrashError) -> Self {
        match err {
            trash::TrashError::RestoreCollision { path } => TrashRestoreError::Collision { path },
            trash::TrashError::NoLongerInTrash { path } => TrashRestoreError::NotFound { path },
            trash::TrashError::Unknown { description } => TrashRestoreError::Unknown { description },
            other => TrashRestoreError::Unknown { description: other.to_string() },
        }
    }
}

struct GlobalFs(Arc<Fs>);

impl Global for GlobalFs {}

#[derive(Copy, Clone, Default)]
pub struct CreateOptions {
    pub overwrite: bool,
    pub ignore_if_exists: bool,
}

#[derive(Copy, Clone, Default)]
pub struct CopyOptions {
    pub overwrite: bool,
    pub ignore_if_exists: bool,
}

#[derive(Copy, Clone, Default)]
pub struct RenameOptions {
    pub overwrite: bool,
    pub ignore_if_exists: bool,
    /// Whether to create parent directories if they do not exist.
    pub create_parents: bool,
}

#[derive(Copy, Clone, Default)]
pub struct RemoveOptions {
    pub recursive: bool,
    pub ignore_if_not_exists: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct Metadata {
    pub inode: u64,
    pub mtime: MTime,
    pub is_symlink: bool,
    pub is_dir: bool,
    pub len: u64,
    pub is_fifo: bool,
    pub is_executable: bool,
    pub is_writable: bool,
}

/// Filesystem modification time. The purpose of this newtype is to discourage use of operations
/// that do not make sense for mtimes. In particular, it is not always valid to compare mtimes using
/// `<` or `>`, as there are many things that can cause the mtime of a file to be earlier than it
/// was. See ["mtime comparison considered harmful" - apenwarr](https://apenwarr.ca/log/20181113).
///
/// Do not derive Ord, PartialOrd, or arithmetic operation traits.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct MTime(SystemTime);

pub type JobId = usize;

#[derive(Clone, Debug)]
pub struct JobInfo {
    pub start: Instant,
    pub message: SharedString,
    pub id: JobId,
}

#[derive(Debug, Clone)]
pub enum JobEvent {
    Started { info: JobInfo },
    Updated { id: JobId, message: SharedString },
    Completed { id: JobId },
}

pub type JobEventSender = UnboundedSender<JobEvent>;
pub type JobEventReceiver = UnboundedReceiver<JobEvent>;

struct JobTracker {
    id: JobId,
    subscribers: Arc<Mutex<Vec<JobEventSender>>>,
}

impl JobTracker {
    fn new(info: JobInfo, subscribers: Arc<Mutex<Vec<JobEventSender>>>) -> Self {
        let id = info.id;
        {
            let mut subs = subscribers.lock();
            subs.retain(|sender| sender.unbounded_send(JobEvent::Started { info: info.clone() }).is_ok());
        }
        Self { id, subscribers }
    }

    fn update(&self, message: SharedString) {
        let mut subscribers = self.subscribers.lock();
        subscribers.retain(|sender| {
            sender.unbounded_send(JobEvent::Updated { id: self.id, message: message.clone() }).is_ok()
        });
    }
}

impl Drop for JobTracker {
    fn drop(&mut self) {
        let mut subs = self.subscribers.lock();
        subs.retain(|sender| sender.unbounded_send(JobEvent::Completed { id: self.id }).is_ok());
    }
}

impl MTime {
    /// Returns the value wrapped by this `MTime`, for presentation to the user. The name including
    /// "_for_user" is to discourage misuse - this method should not be used when making decisions
    /// about file dirtiness.
    pub const fn timestamp_for_user(self) -> SystemTime {
        self.0
    }

    /// Temporary method to split out the behavior changes from introduction of this newtype.
    pub fn bad_is_greater_than(self, other: MTime) -> bool {
        self.0 > other.0
    }
}

slotmap::new_key_type! { pub struct TrashId; }

pub struct Fs {
    this: Weak<Self>,
    executor: BackgroundExecutor,
    native_watcher: Arc<fs_watcher::OsWatcher>,
    next_job_id: Arc<AtomicUsize>,
    job_event_subscribers: Arc<Mutex<Vec<JobEventSender>>>,
    trash: Arc<Mutex<SlotMap<TrashId, trash::TrashItem>>>,
    is_case_sensitive: AtomicU8,
}

/// Current path of an open file, following renames and moves since it was opened.
pub fn file_current_path(file: &File) -> Result<PathBuf> {
    use std::ffi::CStr;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let fd = file.as_fd();
    let mut path_buf = MaybeUninit::<[u8; libc::PATH_MAX as usize]>::uninit();

    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETPATH, path_buf.as_mut_ptr()) };
    anyhow::ensure!(result != -1, "fcntl returned -1");

    // SAFETY: `fcntl` will initialize the path buffer.
    let c_str = unsafe { CStr::from_ptr(path_buf.as_ptr().cast()) };
    anyhow::ensure!(!c_str.is_empty(), "Could find a path for the file handle");
    let path = PathBuf::from(OsStr::from_bytes(c_str.to_bytes()));
    Ok(path)
}

fn rename_without_replace(source: &Path, target: &Path) -> io::Result<()> {
    let source = path_to_c_string(source)?;
    let target = path_to_c_string(target)?;

    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };

    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn path_to_c_string(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("{err}: {}", path.display())))
}

// On Unix targets, std::fs::ReadDir panics in its Drop implementation
// when an unexpected error is returned from closedir(2). We hit this
// condition in production; one cause seems to be macOS's FSEventStream
// incorrectly closing fds it doesn't own, resulting in closedir returning
// EBADF, see https://github.com/zed-industries/zed/issues/59952#issuecomment-5080178879.
//
// We also see occasional errors like ENXIO and ETIMEDOUT that seem to
// come from network or other exotic filesystems.
//
// To avoid crashing the app in this situation, we use the rustix analogue of
// ReadDir, which doesn't have this panic in drop.

fn read_dir_entries(path: PathBuf) -> Result<impl Send + Iterator<Item = Result<PathBuf>>> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    use rustix::fs::Dir;
    use rustix::fs::Mode;
    use rustix::fs::OFlags;

    let directory_fd = rustix::fs::open(&path, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
        .with_context(|| format!("failed to open directory {path:?}"))?;
    let directory = Dir::new(directory_fd).with_context(|| format!("failed to read directory {path:?}"))?;

    Ok(directory.filter_map(move |entry| {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return Some(Err(
                    anyhow::Error::new(error).context(format!("failed to read directory entry in {path:?}"))
                ));
            }
        };
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            return None;
        }
        Some(Ok(path.join(OsStr::from_bytes(name))))
    }))
}

impl Fs {
    pub fn new(executor: BackgroundExecutor) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: Weak::clone(this),
            native_watcher: fs_watcher::OsWatcher::new(executor.clone()),
            executor,
            next_job_id: Arc::new(AtomicUsize::new(0)),
            job_event_subscribers: Arc::new(Mutex::new(Vec::new())),
            trash: Arc::new(Mutex::new(SlotMap::with_key())),
            is_case_sensitive: AtomicU8::default(),
        })
    }

    /// Returns the global [`Fs`].
    pub fn global(cx: &App) -> Arc<Self> {
        Arc::clone(&GlobalFs::global(cx).0)
    }

    /// Sets the global [`Fs`].
    pub fn set_global(fs: Arc<Self>, cx: &mut App) {
        cx.set_global(GlobalFs(fs));
    }

    pub async fn create_dir(&self, path: &Path) -> Result<()> {
        Ok(async_macos::fs::create_dir_all(path).await?)
    }

    pub async fn create_file(&self, path: &Path, options: CreateOptions) -> Result<()> {
        let mut open_options = fs::OpenOptions::new();
        open_options.write(true).create(true);
        if options.overwrite {
            open_options.truncate(true);
        } else if !options.ignore_if_exists {
            open_options.create_new(true);
        }
        let path = path.to_owned();
        unblock(move || open_options.open(&path).with_context(|| format!("Failed to create file at {path:?}"))).await?;
        Ok(())
    }

    pub async fn copy_file(&self, source: &Path, target: &Path, options: CopyOptions) -> Result<()> {
        if !options.overwrite && async_macos::fs::metadata(target).await.is_ok() {
            if options.ignore_if_exists {
                return Ok(());
            }
            anyhow::bail!("{target:?} already exists");
        }

        async_macos::fs::copy(source, target).await?;
        Ok(())
    }

    pub async fn rename(&self, source: &Path, target: &Path, options: RenameOptions) -> Result<()> {
        if options.create_parents
            && let Some(parent) = target.parent()
        {
            self.create_dir(parent).await?;
        }

        if options.overwrite {
            async_macos::fs::rename(source, target).await?;
            return Ok(());
        }

        let use_metadata_fallback = {
            {
                let source = source.to_path_buf();
                let target = target.to_path_buf();
                match self.executor.spawn(async move { rename_without_replace(&source, &target) }).await {
                    Ok(()) => return Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        if options.ignore_if_exists {
                            return Ok(());
                        }
                        return Err(error.into());
                    }
                    Err(error)
                        if error.raw_os_error().is_some_and(|code| {
                            code == libc::ENOSYS
                                || code == libc::ENOTSUP
                                || code == libc::EOPNOTSUPP
                                || code == libc::EINVAL
                        }) =>
                    {
                        // For case when filesystem or kernel does not support atomic no-overwrite rename.
                        // EINVAL is returned by FUSE-based filesystems (e.g. NTFS via ntfs-3g)
                        // that don't support RENAME_NOREPLACE.
                        true
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        };

        if use_metadata_fallback && async_macos::fs::metadata(target).await.is_ok() {
            if options.ignore_if_exists {
                return Ok(());
            }
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{target:?} already exists")).into());
        }

        async_macos::fs::rename(source, target).await?;
        Ok(())
    }

    pub async fn remove_dir(&self, path: &Path, options: RemoveOptions) -> Result<()> {
        let result = if options.recursive {
            async_macos::fs::remove_dir_all(path).await
        } else {
            async_macos::fs::remove_dir(path).await
        };
        match result {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound && options.ignore_if_not_exists => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    pub async fn remove_file(&self, path: &Path, options: RemoveOptions) -> Result<()> {
        match async_macos::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound && options.ignore_if_not_exists => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    pub async fn trash(&self, path: &Path, _options: RemoveOptions) -> Result<TrashId> {
        // We must make the path absolute or trash will make a weird abomination
        // of the ed working directory (not usually the worktree) and whatever
        // the path variable holds.
        // We deliberately use `std::path::absolute` instead of `canonicalize`
        // to avoid resolving symlinks. Otherwise trashing a symlink would trash
        // its target and leave the link behind.
        let path = path::absolute(path).context("Could not make the path absolute")?;

        let item = unblock(move || trash::delete_with_info(path)).await.context("Could not trash file or dir")?;

        Ok(self.trash.lock().insert(item))
    }

    pub fn open_sync(&self, path: &Path) -> Result<File> {
        Ok(File::open(path)?)
    }

    pub async fn load(&self, path: &Path) -> Result<String> {
        let path = path.to_path_buf();
        self.executor
            .spawn(async move {
                fs::read_to_string(&path).with_context(|| format!("Failed to read file {}", path.display()))
            })
            .await
    }

    /// Like [`Self::load`], but fails on anything other than a regular file. For reads that hold shared
    /// state, where opening a FIFO would block until a writer shows up.
    pub async fn load_regular_file(&self, path: &Path) -> Result<String> {
        let path = path.to_path_buf();
        self.executor.spawn(async move { read_regular_file(&path) }).await
    }

    pub async fn load_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        let path = path.to_path_buf();
        let bytes = self.executor.spawn(async move { fs::read(path) }).await?;
        Ok(bytes)
    }

    pub async fn atomic_write(&self, path: PathBuf, data: String) -> Result<()> {
        unblock(move || {
            // Use the directory of the destination as temp dir to avoid
            // invalid cross-device link error, and XDG_CACHE_DIR for fallback.
            // See https://github.com/zed-industries/zed/pull/8437 for more details.
            let mut tmp_file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(app_paths::temp_dir()))?;
            tmp_file.write_all(data.as_bytes())?;
            tmp_file.persist(path)?;
            anyhow::Ok(())
        })
        .await?;

        Ok(())
    }

    pub async fn save(&self, path: &Path, text: &Rope, line_ending: LineEnding) -> Result<()> {
        let buffer_size = text.summary().len.min(10 * 1024);
        if let Some(path) = path.parent() {
            self.create_dir(path).await.with_context(|| format!("Failed to create directory at {path:?}"))?;
        }
        let path = path.to_owned();
        let text = text.clone();
        unblock(move || {
            let file = File::create(&path).with_context(|| format!("Failed to create file at {path:?}"))?;
            let mut writer = io::BufWriter::with_capacity(buffer_size, file);
            for chunk in text::chunks_with_line_ending(&text, line_ending) {
                writer.write_all(chunk.as_bytes())?;
            }
            writer.flush()?;
            anyhow::Ok(())
        })
        .await?;
        Ok(())
    }

    pub async fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        if let Some(path) = path.parent() {
            self.create_dir(path).await.with_context(|| format!("Failed to create directory at {path:?}"))?;
        }
        let path = path.to_owned();
        let contents = content.to_owned();
        self.executor
            .spawn(async move {
                fs::write(path, contents)?;
                Ok(())
            })
            .await
    }

    pub async fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        let path = path.to_owned();
        self.executor
            .spawn(async move {
                let result = fs::canonicalize(&path);

                result.with_context(|| format!("canonicalizing {path:?}"))
            })
            .await
    }

    pub async fn is_file(&self, path: &Path) -> bool {
        let path = path.to_owned();
        self.executor.spawn(async move { fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) }).await
    }

    pub async fn is_dir(&self, path: &Path) -> bool {
        let path = path.to_owned();
        self.executor.spawn(async move { fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) }).await
    }

    pub async fn metadata(&self, path: &Path) -> Result<Option<Metadata>> {
        let path_buf = path.to_owned();
        let symlink_metadata = match self.executor.spawn(async move { fs::symlink_metadata(&path_buf) }).await {
            Ok(metadata) => metadata,
            Err(err) => {
                return match err.kind() {
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Ok(None),
                    _ => Err(anyhow::Error::new(err)),
                };
            }
        };

        let is_symlink = symlink_metadata.file_type().is_symlink();
        let metadata = if is_symlink {
            let path_buf = path.to_path_buf();
            // Read target metadata, if the target exists
            match self.executor.spawn(async move { fs::metadata(path_buf) }).await {
                Ok(target_metadata) => target_metadata,
                Err(err) => {
                    if err.kind() != io::ErrorKind::NotFound {
                        // TODO: Also FilesystemLoop when that's stable
                        log::warn!("Failed to read symlink target metadata for path {path:?}: {err}");
                    }
                    // For a broken or recursive symlink, return the symlink metadata. (Or
                    // as edge cases, a symlink into a directory we can't read, which is hard
                    // to distinguish from just being broken.)
                    symlink_metadata
                }
            }
        } else {
            symlink_metadata
        };

        let inode = metadata.ino();

        let is_fifo = metadata.file_type().is_fifo();

        let is_executable = metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;

        Ok(Some(Metadata {
            inode,
            mtime: MTime(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
            len: metadata.len(),
            is_symlink,
            is_dir: metadata.file_type().is_dir(),
            is_fifo,
            is_executable,
            is_writable: !metadata.permissions().readonly(),
        }))
    }

    pub async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        let path = path.to_owned();
        let path = self.executor.spawn(async move { fs::read_link(&path) }).await?;
        Ok(path)
    }

    pub async fn read_dir(&self, path: &Path) -> Result<Pin<Box<dyn Send + Stream<Item = Result<PathBuf>>>>> {
        let path = path.to_owned();
        let entries = self.executor.spawn(async move { read_dir_entries(path) }).await?;
        Ok(Box::pin(iter(entries)))
    }

    pub fn path_exists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    pub fn is_path_case_sensitive(&self, path: &Path) -> bool {
        !fs_watcher::case_insensitive_path(path)
    }

    pub async fn watch(
        &self,
        path: &Path,
        latency: Duration,
    ) -> (Pin<Box<dyn Send + Stream<Item = Vec<PathEvent>>>>, Arc<dyn Watcher>) {
        let this = self.this.upgrade().expect("Fs is only constructed inside an Arc");
        fs_watcher::watch(this, Arc::clone(&self.native_watcher), self.executor.clone(), path, latency).await
    }

    pub fn open_repo(&self, dotgit_path: &Path, git_binary_path: Option<&Path>) -> Result<Arc<dyn GitRepository>> {
        Ok(Arc::new(RealGitRepository::new(
            dotgit_path,
            git_binary_path.map(|x| x.to_path_buf()),
            self.executor.clone(),
        )?))
    }

    pub async fn git_init(&self, abs_work_directory_path: &Path, fallback_branch_name: String) -> Result<()> {
        let result = new_command("git")
            .current_dir(abs_work_directory_path)
            .args(["config", "--global", "--get", "init.defaultBranch"])
            .output()
            .await;

        // In case the `git config` command fails, which would be the case if
        // the user doesn't have an `init.defaultBranch` value set, we'll just
        // default to the provided `fallback_branch_name`.
        let branch_name = match result {
            Ok(output) if !output.stdout.is_empty() => String::from_utf8(output.stdout)?,
            _ => fallback_branch_name,
        };

        new_command("git")
            .current_dir(abs_work_directory_path)
            .args(["init", "-b"])
            .arg(branch_name.trim())
            .output()
            .await?;

        Ok(())
    }

    pub async fn git_clone(&self, abs_work_directory: &Path, repo_url: &str) -> Result<()> {
        let job_id = self.next_job_id.fetch_add(1, Ordering::SeqCst);
        let job_info =
            JobInfo { id: job_id, start: Instant::now(), message: SharedString::from(format!("Cloning {repo_url}")) };

        let job_tracker = JobTracker::new(job_info, Arc::clone(&self.job_event_subscribers));
        let mut child = new_command("git")
            .current_dir(abs_work_directory)
            .args(["clone", "--progress", repo_url])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS", "/usr/bin/false")
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("GCM_INTERACTIVE", "never")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = child.stderr.take().context("failed to read git clone progress")?;
        let stderr_output = git_clone_progress::read(stderr, |message| {
            job_tracker.update(message.into());
        })
        .await?;
        let status = child.status().await?;

        if !status.success() {
            anyhow::bail!("git clone failed: {}", git_clone_progress::failure_message(&stderr_output));
        }

        Ok(())
    }

    /// Runs `git config` with the given arguments.
    /// Will return `Ok` if the commands exit status is `0`, with the stdout
    /// contents. Otherwise returns `Err` with the stderr contents.
    pub async fn git_config(&self, abs_work_directory: &Path, args: Vec<String>) -> Result<String> {
        let output = new_command("git")
            .current_dir(abs_work_directory)
            .args(std::iter::once(String::from("config")).chain(args))
            .output()
            .await?;

        if !output.status.success() {
            let err = String::from_utf8(output.stderr)?;
            anyhow::bail!(err);
        }

        String::from_utf8(output.stdout).map_err(Into::into)
    }

    pub fn subscribe_to_jobs(&self) -> JobEventReceiver {
        let (sender, receiver) = mpsc::unbounded();
        self.job_event_subscribers.lock().push(sender);
        receiver
    }

    /// Checks whether the file system is case sensitive by attempting to create two files
    /// that have the same name except for the casing.
    ///
    /// It creates both files in a temporary directory it removes at the end.
    pub async fn is_case_sensitive(&self) -> bool {
        const UNINITIALIZED: u8 = 0;
        const CASE_SENSITIVE: u8 = 1;
        const NOT_CASE_SENSITIVE: u8 = 2;

        // Note we could CAS here, but really, if we race we do this work twice at worst which isn't a big deal.
        let load = self.is_case_sensitive.load(Ordering::Acquire);
        if load != UNINITIALIZED {
            return load == CASE_SENSITIVE;
        }
        let temp_dir = self.executor.spawn(async { TempDir::new() });
        let res = async {
            let temp_dir = temp_dir.await?;
            let test_file_1 = temp_dir.path().join("case_sensitivity_test.tmp");
            let test_file_2 = temp_dir.path().join("CASE_SENSITIVITY_TEST.TMP");

            let create_opts = CreateOptions { overwrite: false, ignore_if_exists: false };

            // Create file1
            self.create_file(&test_file_1, create_opts).await?;

            // Now check whether it's possible to create file2
            let case_sensitive = match self.create_file(&test_file_2, create_opts).await {
                Ok(()) => Ok(true),
                Err(e) => {
                    if let Some(io_error) = e.downcast_ref::<io::Error>() {
                        if io_error.kind() == io::ErrorKind::AlreadyExists { Ok(false) } else { Err(e) }
                    } else {
                        Err(e)
                    }
                }
            };

            temp_dir.close()?;
            case_sensitive
        }
        .await
        .unwrap_or_else(|e| {
            log::error!(
                "Failed to determine whether filesystem is case sensitive (falling back to true) due to error: {e:#}"
            );
            true
        });
        self.is_case_sensitive.store(if res { CASE_SENSITIVE } else { NOT_CASE_SENSITIVE }, Ordering::Release);
        res
    }

    pub fn original_path_for_trash_id(&self, trash_id: TrashId) -> Option<PathBuf> {
        self.trash.lock().get(trash_id).map(trash::TrashItem::original_path)
    }

    pub async fn restore(&self, trash_id: TrashId) -> result::Result<PathBuf, TrashRestoreError> {
        let item = self.trash.lock().get(trash_id).cloned().ok_or(TrashRestoreError::AlreadyRestored)?;
        let restored_item_path = item.original_path();

        let (tx, rx) = oneshot::channel();
        Builder::new()
            .name("restore trashed item".to_owned())
            .spawn(move || {
                let res = trash::restore(item);
                tx.send(res)
            })
            .expect("The OS can spawn a threads");

        rx.await.expect("restore never panics")?;
        self.trash.lock().remove(trash_id);
        Ok(restored_item_path)
    }
}

pub async fn copy_recursive<'a>(fs: &'a Fs, source: &'a Path, target: &'a Path, options: CopyOptions) -> Result<()> {
    for (item, is_dir) in read_dir_items(fs, source).await? {
        let Ok(item_relative_path) = item.strip_prefix(source) else {
            continue;
        };
        let target_item =
            if item_relative_path == Path::new("") { target.to_path_buf() } else { target.join(item_relative_path) };
        if is_dir {
            if !options.overwrite && fs.metadata(&target_item).await.is_ok_and(|m| m.is_some()) {
                if options.ignore_if_exists {
                    continue;
                }
                anyhow::bail!("{target_item:?} already exists");
            }
            fs.remove_dir(&target_item, RemoveOptions { recursive: true, ignore_if_not_exists: true }).await.ok();
            fs.create_dir(&target_item).await?;
        } else {
            fs.copy_file(&item, &target_item, options).await?;
        }
    }
    Ok(())
}

/// Recursively reads all of the paths in the given directory.
///
/// Returns a vector of tuples of (path, is_dir).
pub async fn read_dir_items<'a>(fs: &'a Fs, source: &'a Path) -> Result<Vec<(PathBuf, bool)>> {
    let mut items = Vec::new();
    read_recursive(fs, source, &mut items).await?;
    Ok(items)
}

fn read_recursive<'a>(fs: &'a Fs, source: &'a Path, output: &'a mut Vec<(PathBuf, bool)>) -> BoxFuture<'a, Result<()>> {
    use futures::future::FutureExt as _;

    async move {
        let metadata = fs.metadata(source).await?.with_context(|| format!("path does not exist: {source:?}"))?;

        if metadata.is_dir {
            output.push((source.to_path_buf(), true));
            let mut children = fs.read_dir(source).await?;
            while let Some(child_path) = children.next().await {
                if let Ok(child_path) = child_path {
                    read_recursive(fs, &child_path, output).await?;
                }
            }
        } else {
            output.push((source.to_path_buf(), false));
        }
        Ok(())
    }
    .boxed()
}

// todo(windows)
// can we get file id not open the file twice?
// https://github.com/rust-lang/rust/issues/63010

fn read_regular_file(path: &Path) -> Result<String> {
    anyhow::ensure!(fs::metadata(path).is_ok_and(|metadata| metadata.is_file()), "{path:?} is not a file");
    fs::read_to_string(path).with_context(|| format!("Failed to read file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_regular_file_rejects_fifos() {
        use std::os::unix::ffi::OsStrExt as _;

        let dir = TempDir::new().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "x").unwrap();
        assert_eq!(read_regular_file(&file).unwrap(), "x");
        assert!(read_regular_file(&dir.path().join("missing")).is_err());

        // Opening a FIFO for reading blocks until a writer opens it.
        let fifo = dir.path().join("fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        assert!(read_regular_file(&fifo).is_err());
    }

    #[test]
    fn file_current_path_follows_renames() {
        let dir = TempDir::new().unwrap();
        let dir_path = fs::canonicalize(dir.path()).unwrap();
        let old = dir_path.join("old.txt");
        let new = dir_path.join("new.txt");
        fs::write(&old, "x").unwrap();

        let file = File::open(&old).unwrap();
        assert_eq!(file_current_path(&file).unwrap(), old);
        fs::rename(&old, &new).unwrap();
        assert_eq!(file_current_path(&file).unwrap(), new);
    }
}
