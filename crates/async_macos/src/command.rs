#![allow(clippy::disallowed_methods, reason = "this is the wrapper the lint points to")]

use std::ffi::OsStr;
use std::future::poll_fn;
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::pin::Pin;
use std::process::ExitStatus;
use std::process::Output;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;

use futures::AsyncRead;
use futures::AsyncReadExt as _;
use futures::AsyncWrite;

use crate::gcd::AsyncFd;
use crate::gcd::ProcessExit;

pub fn new_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

pub use std::process::Stdio;

#[derive(Debug)]
pub struct Command {
    inner: std::process::Command,
    stdin: bool,
    stdout: bool,
    stderr: bool,
    kill_on_drop: bool,
}

impl From<std::process::Command> for Command {
    fn from(inner: std::process::Command) -> Self {
        Self { inner, stdin: false, stdout: false, stderr: false, kill_on_drop: false }
    }
}

impl Command {
    #[inline]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        std::process::Command::new(program).into()
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.inner.get_args()
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.inner.env(key, val);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.inner.env_remove(key);
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    pub fn stdin(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.stdin = true;
        self.inner.stdin(cfg.into());
        self
    }

    pub fn stdout(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.stdout = true;
        self.inner.stdout(cfg.into());
        self
    }

    pub fn stderr(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.stderr = true;
        self.inner.stderr(cfg.into());
        self
    }

    pub const fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.kill_on_drop = kill_on_drop;
        self
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        let mut child = self.inner.spawn()?;
        Ok(Child {
            stdin: child.stdin.take().map(|pipe| pipe_fd(pipe.into()).map(ChildStdin)).transpose()?,
            stdout: child.stdout.take().map(|pipe| pipe_fd(pipe.into()).map(ChildStdout)).transpose()?,
            stderr: child.stderr.take().map(|pipe| pipe_fd(pipe.into()).map(ChildStderr)).transpose()?,
            pid: child.id(),
            exit: ProcessExit::watch(child.id()),
            kill_on_drop: self.kill_on_drop,
        })
    }

    /// Runs the command and collects its output. Unless configured otherwise,
    /// stdin is null and stdout/stderr are captured.
    pub async fn output(&mut self) -> io::Result<Output> {
        if !self.stdin {
            self.inner.stdin(Stdio::null());
        }
        if !self.stdout {
            self.inner.stdout(Stdio::piped());
        }
        if !self.stderr {
            self.inner.stderr(Stdio::piped());
        }
        self.spawn()?.output().await
    }

    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.status().await
    }
}

fn pipe_fd(fd: OwnedFd) -> io::Result<Option<AsyncFd>> {
    AsyncFd::new(fd).map(Some)
}

pub struct Child {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    pid: u32,
    exit: Arc<ProcessExit>,
    kill_on_drop: bool,
}

impl Child {
    pub const fn id(&self) -> u32 {
        self.pid
    }

    pub fn kill(&mut self) -> io::Result<()> {
        self.exit.signal(libc::SIGKILL, false);
        Ok(())
    }

    /// Kills the child's whole process group; only meaningful when the child
    /// leads its own group.
    pub fn kill_group(&mut self) {
        self.exit.signal(libc::SIGKILL, true);
    }

    pub fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(self.exit.try_status())
    }

    /// Closes stdin and waits for the child to exit.
    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        self.stdin.take();
        Ok(poll_fn(|cx| self.exit.poll_status(cx)).await)
    }

    pub async fn output(mut self) -> io::Result<Output> {
        self.stdin.take();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut stdout_pipe = self.stdout.take();
        let mut stderr_pipe = self.stderr.take();
        futures::try_join!(
            async {
                match &mut stdout_pipe {
                    Some(pipe) => pipe.read_to_end(&mut stdout).await.map(drop),
                    None => Ok(()),
                }
            },
            async {
                match &mut stderr_pipe {
                    Some(pipe) => pipe.read_to_end(&mut stderr).await.map(drop),
                    None => Ok(()),
                }
            },
        )?;
        let status = self.status().await?;
        Ok(Output { status, stdout, stderr })
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop {
            self.exit.signal(libc::SIGKILL, false);
        }
    }
}

/// The write end of a child's stdin; closing or dropping it sends EOF.
pub struct ChildStdin(Option<AsyncFd>);
pub struct ChildStdout(Option<AsyncFd>);
pub struct ChildStderr(Option<AsyncFd>);

impl AsyncWrite for ChildStdin {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &self.0 {
            Some(fd) => fd.poll_write(cx, buf),
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.take();
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for ChildStdout {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        match &self.0 {
            Some(fd) => fd.poll_read(cx, buf),
            None => Poll::Ready(Ok(0)),
        }
    }
}

impl AsyncRead for ChildStderr {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        match &self.0 {
            Some(fd) => fd.poll_read(cx, buf),
            None => Poll::Ready(Ok(0)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::zombie_processes, reason = "children are reaped by ProcessExit")]
mod tests {
    use std::os::unix::process::CommandExt as _;

    use futures::AsyncWriteExt as _;
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn output_captures_stdout_and_stderr() {
        let output = block_on(new_command("/bin/sh").args(["-c", "echo out; echo err >&2; exit 2"]).output()).unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[test]
    fn status_reports_exit_code() {
        let mut command = Command::from(std::process::Command::new("/usr/bin/false"));
        assert_eq!(block_on(command.status()).unwrap().code(), Some(1));
        assert_eq!(command.get_args().count(), 0);
    }

    #[test]
    fn spawn_pipes_stdin_to_stdout() {
        block_on(async {
            let mut child = new_command("/bin/cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let payload = vec![7_u8; 300_000];
            let write = async {
                stdin.write_all(&payload).await.unwrap();
                stdin.close().await.unwrap();
                stdin.write(b"x").await.unwrap_err();
            };
            let ((), output) = futures::join!(write, child.output());
            let output = output.unwrap();
            assert!(output.status.success());
            assert!(output.stdout == payload);
        });
    }

    #[test]
    fn kill_and_try_status() {
        block_on(async {
            let mut child = new_command("/bin/sleep").arg("30").spawn().unwrap();
            assert!(child.id() > 0);
            assert_eq!(child.try_status().unwrap(), None);
            child.kill().unwrap();
            let status = child.status().await.unwrap();
            assert!(!status.success());
            assert_eq!(child.try_status().unwrap(), Some(status));
        });
    }

    #[test]
    fn kill_on_drop_terminates_the_child() {
        let child = new_command("/bin/sleep").arg("30").kill_on_drop(true).spawn().unwrap();
        let pid = child.id().cast_signed();
        drop(child);
        // The watcher reaps the child, after which the pid no longer exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn kill_group_takes_down_descendants() {
        block_on(async {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", "sleep 30 & wait"]);
            // SAFETY: setsid is async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            let mut child = Command::from(command).spawn().unwrap();
            child.kill_group();
            assert!(!child.status().await.unwrap().success());
        });
    }
}
