//! Async IO primitives backed by GCD dispatch sources, so no reactor or reaper
//! thread is needed.

use std::ffi::c_void;
use std::io;
use std::os::fd::AsRawFd as _;
use std::os::fd::OwnedFd;
use std::os::fd::RawFd;
use std::os::unix::process::ExitStatusExt as _;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use dispatch2::DispatchObject as _;
use dispatch2::DispatchQueue;
use dispatch2::DispatchQueueGlobalPriority;
use dispatch2::DispatchRetained;
use dispatch2::DispatchSource;
use dispatch2::GlobalQueueIdentifier;
use dispatch2::dispatch_source_type_t;
use futures::AsyncRead;
use futures::AsyncWrite;
use futures::channel::oneshot;
use futures::task::AtomicWaker;

const DISPATCH_PROC_EXIT: usize = 0x8000_0000;

fn queue() -> DispatchRetained<DispatchQueue> {
    DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Default))
}

/// Runs blocking work on a GCD worker thread.
pub fn unblock<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T> + Send {
    let (tx, rx) = oneshot::channel();
    queue().exec_async(move || {
        tx.send(work()).ok();
    });
    async move { rx.await.expect("unblock work panicked") }
}

const IDLE: u8 = 0;
const ARMED: u8 = 1;
const CLOSED: u8 = 2;

/// Readiness of one direction of a file descriptor.
///
/// The source is level triggered, so it stays suspended (`IDLE`) until a poll
/// hits `WouldBlock`, and suspends itself again when it fires.
struct Readiness {
    source: DispatchRetained<DispatchSource>,
    state: AtomicU8,
    waker: AtomicWaker,
    // Keeps the descriptor open until the source is fully cancelled.
    _fd: Arc<OwnedFd>,
}

impl Readiness {
    fn new(fd: &Arc<OwnedFd>, source_type: dispatch_source_type_t) -> Arc<Self> {
        // SAFETY: the descriptor outlives the source, see `_fd`.
        let source = unsafe { DispatchSource::new(source_type, fd.as_raw_fd() as usize, 0, Some(&queue())) };
        let this =
            Arc::new(Self { source, state: AtomicU8::new(IDLE), waker: AtomicWaker::new(), _fd: Arc::clone(fd) });
        // SAFETY: the context is a strong reference released by `on_cancel`.
        unsafe {
            this.source.set_context(Arc::into_raw(Arc::clone(&this)) as *mut c_void);
        }
        this.source.set_event_handler_f(Self::on_event);
        this.source.set_cancel_handler_f(Self::on_cancel);
        this
    }

    fn arm(&self) {
        if self.state.compare_exchange(IDLE, ARMED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.source.resume();
        }
    }

    fn close(&self) {
        self.source.cancel();
        // A suspended source never delivers its cancel handler.
        if self.state.swap(CLOSED, Ordering::AcqRel) == IDLE {
            self.source.resume();
        }
    }

    extern "C" fn on_event(context: *mut c_void) {
        // SAFETY: `on_cancel` owns the reference and runs after the last event.
        let this = unsafe { &*(context as *const Self) };
        this.source.suspend();
        if this.state.compare_exchange(ARMED, IDLE, Ordering::AcqRel, Ordering::Acquire).is_err() {
            this.source.resume();
        }
        this.waker.wake();
    }

    extern "C" fn on_cancel(context: *mut c_void) {
        // SAFETY: balances `Arc::into_raw` in `new`.
        drop(unsafe { Arc::from_raw(context as *const Self) });
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Read,
    Write,
}

/// A non-blocking file descriptor whose readiness is delivered by GCD.
pub struct AsyncFd {
    fd: Arc<OwnedFd>,
    read: OnceLock<Arc<Readiness>>,
    write: OnceLock<Arc<Readiness>>,
}

impl AsyncFd {
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        let raw = fd.as_raw_fd();
        // SAFETY: plain fcntl calls on a descriptor we own.
        unsafe {
            let flags = libc::fcntl(raw, libc::F_GETFL);
            if flags < 0 || libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self { fd: Arc::new(fd), read: OnceLock::new(), write: OnceLock::new() })
    }

    fn poll_io<T>(
        &self,
        direction: Direction,
        cx: &Context<'_>,
        mut io: impl FnMut(RawFd) -> isize,
        map: impl FnOnce(isize) -> T,
    ) -> Poll<io::Result<T>> {
        loop {
            let result = io(self.fd.as_raw_fd());
            if result >= 0 {
                return Poll::Ready(Ok(map(result)));
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock => {
                    let readiness = match direction {
                        Direction::Read => self.read.get_or_init(|| {
                            Readiness::new(&self.fd, (&raw const dispatch2::_dispatch_source_type_read).cast_mut())
                        }),
                        Direction::Write => self.write.get_or_init(|| {
                            Readiness::new(&self.fd, (&raw const dispatch2::_dispatch_source_type_write).cast_mut())
                        }),
                    };
                    // Register before arming so an event that fires in between still wakes us.
                    readiness.waker.register(cx.waker());
                    readiness.arm();
                    return Poll::Pending;
                }
                _ => return Poll::Ready(Err(error)),
            }
        }
    }

    pub fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.poll_io(
            Direction::Read,
            cx,
            // SAFETY: `buf` is valid for `buf.len()` bytes.
            |fd| unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) },
            |read| read as usize,
        )
    }

    pub fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.poll_io(
            Direction::Write,
            cx,
            // SAFETY: `buf` is valid for `buf.len()` bytes.
            |fd| unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) },
            |written| written as usize,
        )
    }

    /// Accepts a connection on a listening socket.
    pub fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<io::Result<OwnedFd>> {
        use std::os::fd::FromRawFd as _;
        self.poll_io(
            Direction::Read,
            cx,
            // SAFETY: null address arguments are allowed.
            |fd| unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) as isize },
            // SAFETY: accept returned a fresh descriptor.
            |fd| unsafe { OwnedFd::from_raw_fd(fd as RawFd) },
        )
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        for readiness in [self.read.get(), self.write.get()].into_iter().flatten() {
            readiness.close();
        }
    }
}

impl AsyncRead for AsyncFd {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        AsyncFd::poll_read(&self, cx, buf)
    }
}

impl AsyncWrite for AsyncFd {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        AsyncFd::poll_write(&self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Reaps a child process when it exits and hands out its status.
///
/// The watcher keeps itself alive until the process is reaped, so dropping the
/// handle never leaves a zombie.
pub struct ProcessExit {
    pid: libc::pid_t,
    status: Mutex<Option<ExitStatus>>,
    waker: AtomicWaker,
    source: DispatchRetained<DispatchSource>,
}

impl ProcessExit {
    pub fn watch(pid: u32) -> Arc<Self> {
        let pid = pid.cast_signed();
        // SAFETY: a proc source only needs a pid; one that already exited fires immediately.
        let source = unsafe {
            DispatchSource::new(
                (&raw const dispatch2::_dispatch_source_type_proc).cast_mut(),
                pid as usize,
                DISPATCH_PROC_EXIT,
                Some(&queue()),
            )
        };
        let this = Arc::new(Self { pid, status: Mutex::new(None), waker: AtomicWaker::new(), source });
        // SAFETY: the context is a strong reference released by `on_cancel`.
        unsafe {
            this.source.set_context(Arc::into_raw(Arc::clone(&this)) as *mut c_void);
        }
        this.source.set_event_handler_f(Self::on_exit);
        this.source.set_cancel_handler_f(Self::on_cancel);
        this.source.resume();
        this
    }

    pub fn try_status(&self) -> Option<ExitStatus> {
        *self.status.lock().unwrap()
    }

    pub fn poll_status(&self, cx: &mut Context<'_>) -> Poll<ExitStatus> {
        self.waker.register(cx.waker());
        match self.try_status() {
            Some(status) => Poll::Ready(status),
            None => Poll::Pending,
        }
    }

    /// Sends `signal` unless the process was already reaped, in which case the
    /// pid may belong to someone else.
    pub fn signal(&self, signal: libc::c_int, group: bool) {
        let status = self.status.lock().unwrap();
        if status.is_none() {
            // SAFETY: the pid is still ours while the lock is held and no status is stored.
            unsafe {
                if group {
                    libc::killpg(self.pid, signal);
                } else {
                    libc::kill(self.pid, signal);
                }
            }
        }
    }

    extern "C" fn on_exit(context: *mut c_void) {
        // SAFETY: `on_cancel` owns the reference and runs after the last event.
        let this = unsafe { &*(context as *const Self) };
        let mut status = this.status.lock().unwrap();
        if status.is_none() {
            let mut raw = 0;
            // SAFETY: the process has exited, so this returns without blocking.
            while unsafe { libc::waitpid(this.pid, &raw mut raw, 0) } < 0
                && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
            {}
            *status = Some(ExitStatus::from_raw(raw));
        }
        drop(status);
        this.source.cancel();
        this.waker.wake();
    }

    extern "C" fn on_cancel(context: *mut c_void) {
        // SAFETY: balances `Arc::into_raw` in `watch`.
        drop(unsafe { Arc::from_raw(context as *const Self) });
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use futures::AsyncReadExt as _;
    use futures::AsyncWriteExt as _;
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn unblock_returns_the_result() {
        assert_eq!(block_on(unblock(|| 6 * 7)), 42);
    }

    #[test]
    fn pipe_blocks_in_both_directions() {
        let (reader, writer) = io::pipe().unwrap();
        let mut reader = AsyncFd::new(reader.into()).unwrap();
        let mut writer = AsyncFd::new(writer.into()).unwrap();
        // Larger than the pipe buffer, so the writer must wait for the reader.
        let payload = (0..1_000_000_u32).map(|i| i as u8).collect::<Vec<_>>();
        let mut received = Vec::new();
        block_on(async {
            let write = async {
                writer.write_all(&payload).await.unwrap();
                drop(writer);
            };
            let read = async { reader.read_to_end(&mut received).await.unwrap() };
            futures::join!(write, read);
        });
        assert!(received == payload);
    }

    #[test]
    fn dropping_an_armed_fd_closes_it() {
        let (reader, writer) = io::pipe().unwrap();
        let reader = AsyncFd::new(reader.into()).unwrap();
        let mut writer = AsyncFd::new(writer.into()).unwrap();
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(reader.poll_read(&mut cx, &mut [0; 8]).is_pending());
        drop(reader);
        // The read end closes asynchronously, after which writes fail.
        let error = loop {
            match block_on(writer.write(b"x")) {
                Ok(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
                Err(error) => break error,
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    #[allow(clippy::zombie_processes, reason = "children are reaped by ProcessExit")]
    fn process_exit_reports_status() {
        // Repeated to cover children that exit before the watcher is installed.
        for code in [0, 3].into_iter().cycle().take(40) {
            let child = std::process::Command::new("/bin/sh").args(["-c", &format!("exit {code}")]).spawn().unwrap();
            let exit = ProcessExit::watch(child.id());
            let status = block_on(poll_fn(|cx| exit.poll_status(cx)));
            assert_eq!(status.code(), Some(code));
            assert_eq!(exit.try_status(), Some(status));
        }
    }

    #[test]
    #[allow(clippy::zombie_processes, reason = "children are reaped by ProcessExit")]
    fn signal_kills_a_running_process() {
        let child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let exit = ProcessExit::watch(child.id());
        assert_eq!(exit.try_status(), None);
        exit.signal(libc::SIGKILL, false);
        let status = block_on(poll_fn(|cx| exit.poll_status(cx)));
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        // A no-op once reaped.
        exit.signal(libc::SIGKILL, false);
    }
}
