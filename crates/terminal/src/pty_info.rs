use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::Context;
use gpui::Task;
use parking_lot::MappedRwLockReadGuard;
use parking_lot::Mutex;
use parking_lot::RwLock;
use parking_lot::RwLockReadGuard;
use sysinfo::Pid;
use sysinfo::Process;
use sysinfo::ProcessRefreshKind;
use sysinfo::ProcessesToUpdate;
use sysinfo::System;
use sysinfo::UpdateKind;

use crate::Event;
use crate::Terminal;

#[derive(Clone, Copy)]
pub struct ProcessIdGetter {
    handle: i32,
    fallback_pid: u32,
}

impl ProcessIdGetter {
    pub(crate) const fn new(handle: i32, fallback_pid: u32) -> ProcessIdGetter {
        ProcessIdGetter { handle, fallback_pid }
    }

    pub fn fallback_pid(self) -> Pid {
        Pid::from_u32(self.fallback_pid)
    }

    fn pid(self) -> Option<Pid> {
        // Negative pid means error.
        // Zero pid means no foreground process group is set on the PTY yet.
        // Avoid killing the current process by returning a zero pid.
        let pid = unsafe { libc::tcgetpgrp(self.handle) };
        if pid > 0 {
            return Some(Pid::from_u32(pid as u32));
        }

        if self.fallback_pid > 0 {
            return Some(Pid::from_u32(self.fallback_pid));
        }

        None
    }
}

/// Whether the process group leader `foreground` is something other than the
/// shell started as (or, on the default macOS path, under `login` by) the PTY
/// child.
fn is_foreground_job(foreground: Pid, foreground_parent: Option<Pid>, child: Pid, child_name: &OsStr) -> bool {
    foreground != child && !(child_name == "login" && foreground_parent == Some(child))
}

/// How long output must be quiet before the foreground process is rechecked.
const RECHECK_DELAY: Duration = Duration::from_millis(100);

/// Whether the foreground process changed in a way the tab shows: its name,
/// its directory, or the process itself, since a job named like the shell
/// (a script run by it) still starts and ends a running process.
fn process_changed(previous: Option<&ProcessInfo>, current: Option<&ProcessInfo>) -> bool {
    match (previous, current) {
        (None, None) => false,
        (Some(prev), Some(now)) => prev.pid != now.pid || prev.cwd != now.cwd || prev.name != now.name,
        _ => true,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessInfo {
    pid: Pid,
    pub(crate) name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) argv: Vec<String>,
}

/// Fetches ed-relevant Pseudo-Terminal (PTY) process information.
pub(crate) struct PtyProcessInfo {
    system: RwLock<System>,
    refresh_kind: ProcessRefreshKind,
    pid_getter: ProcessIdGetter,
    last_foreground_pid: Mutex<Option<Pid>>,
    pub(crate) current: RwLock<Option<ProcessInfo>>,
    task: Mutex<Option<Task<()>>>,
    /// Output arrived while `task` was checking or waiting.
    woken: AtomicBool,
}

impl PtyProcessInfo {
    pub(crate) fn new(pid_getter: ProcessIdGetter) -> PtyProcessInfo {
        // Task enumeration is on by default and would retain a `Process` entry
        // per thread (#58651).
        let process_refresh_kind = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .without_tasks();
        // `System::new_with_specifics` with a process refresh kind would
        // snapshot every process on the machine into this terminal's `System`,
        // retaining one open procfs handle per process for the lifetime of the
        // terminal (#58651). Refresh only the spawned child so that
        // `kill_child_process` works before the first foreground refresh.
        let mut system = System::new().expect("System::new is infallible on macOS");
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid_getter.fallback_pid()]),
            true,
            process_refresh_kind,
        );

        PtyProcessInfo {
            system: RwLock::new(system),
            refresh_kind: process_refresh_kind,
            pid_getter,
            last_foreground_pid: Mutex::new(None),
            current: RwLock::new(None),
            task: Mutex::new(None),
            woken: AtomicBool::new(false),
        }
    }

    pub(crate) const fn pid_getter(&self) -> &ProcessIdGetter {
        &self.pid_getter
    }

    fn refresh(&self) -> Option<MappedRwLockReadGuard<'_, Process>> {
        let pid = self.pid_getter.pid()?;
        let fallback_pid = self.pid_getter.fallback_pid();
        let mut system = self.system.write();
        // sysinfo never evicts processes that are absent from the refreshed pid
        // set, so entries for former foreground processes would otherwise
        // accumulate for as long as this terminal lives (#58651). Rebuild the `System` whenever
        // the foreground process changes to keep the map bounded.
        if self.last_foreground_pid.lock().replace(pid) != Some(pid) {
            *system = System::new().expect("System::new is infallible on macOS");
        }
        let pids = [pid, fallback_pid];
        let pids = if pid == fallback_pid { &pids[..1] } else { &pids[..] };
        system.refresh_processes_specifics(ProcessesToUpdate::Some(pids), true, self.refresh_kind);
        drop(system);
        RwLockReadGuard::try_map(self.system.read(), |system| system.process(pid)).ok()
    }

    fn get_child(&self) -> Option<MappedRwLockReadGuard<'_, Process>> {
        let pid = self.pid_getter.fallback_pid();
        RwLockReadGuard::try_map(self.system.read(), |system| system.process(pid)).ok()
    }

    pub(crate) fn kill_child_process(&self) -> bool {
        self.get_child().is_some_and(|process| process.kill())
    }

    pub(crate) fn terminate_child_process(&self) -> bool {
        let pid = self.pid_getter.fallback_pid();
        unsafe { libc::killpg(pid.as_u32().cast_signed(), libc::SIGTERM) == 0 }
    }

    /// Whether the PTY's foreground process group is a job, not the shell at
    /// its prompt.
    pub(crate) fn has_foreground_job(&self) -> bool {
        let child = self.pid_getter.fallback_pid();
        let Some((foreground, foreground_parent)) = self.refresh().map(|process| (process.pid(), process.parent()))
        else {
            return false;
        };
        self.get_child().is_some_and(|process| is_foreground_job(foreground, foreground_parent, child, process.name()))
    }

    fn load(&self) -> Option<ProcessInfo> {
        let process = self.refresh()?;
        let cwd = process.cwd().map_or(PathBuf::new(), ToOwned::to_owned);

        let info = ProcessInfo {
            pid: process.pid(),
            name: process.name().to_str()?.to_owned(),
            cwd,
            argv: process.cmd().iter().filter_map(|s| s.to_str().map(ToOwned::to_owned)).collect(),
        };
        *self.current.write() = Some(info.clone());
        Some(info)
    }

    /// Reloads the foreground process, returning whether it changed and its
    /// directory if that did.
    fn detect_change(&self) -> (bool, Option<PathBuf>) {
        let previous = self.current.read().clone();
        let current = self.load();
        let has_changed = process_changed(previous.as_ref(), current.as_ref());
        if has_changed {
            (*self.current.write()).clone_from(&current);
        }
        let changed_cwd = match (previous.as_ref(), current.as_ref()) {
            (Some(prev), Some(now)) if prev.cwd != now.cwd => Some(now.cwd.clone()),
            (None, Some(now)) => Some(now.cwd.clone()),
            _ => None,
        };
        (has_changed, changed_cwd)
    }

    /// Updates the cached process info on terminal output, emitting a
    /// [`Event::TitleChanged`] event if the ed-relevant info has changed.
    /// Checks at once, then again once output has been quiet for
    /// [`RECHECK_DELAY`]: the shell echoes a command before it starts the job,
    /// and a quiet job (`sleep`) prints nothing more to trigger a later check.
    pub(crate) fn emit_title_changed_if_changed(self: &Arc<Self>, cx: &Context<'_, Terminal>) {
        if self.task.lock().is_some() {
            self.woken.store(true, Ordering::Relaxed);
            return;
        }
        let weak = Arc::downgrade(self);
        *self.task.lock() = Some(cx.spawn(async move |term, cx| {
            let mut quiet = false;
            loop {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let detect = Arc::clone(&this);
                let (has_changed, new_cwd) =
                    cx.background_executor().spawn(async move { detect.detect_change() }).await;
                if has_changed {
                    term.update(cx, |terminal, cx| {
                        if let Some(cwd) = new_cwd {
                            terminal.record_cwd_change(cwd);
                        }
                        cx.emit(Event::TitleChanged);
                    })
                    .ok();
                }
                if quiet && !this.woken.load(Ordering::Relaxed) {
                    this.task.lock().take();
                    return;
                }
                drop(this);
                cx.background_executor().timer(RECHECK_DELAY).await;
                let Some(this) = weak.upgrade() else {
                    return;
                };
                quiet = !this.woken.swap(false, Ordering::Relaxed);
            }
        }));
    }

    pub(crate) fn pid(&self) -> Option<Pid> {
        self.pid_getter.pid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_foreground_job_works() {
        let pid = Pid::from_u32;
        let login = OsStr::new("login");
        let fish = OsStr::new("fish");
        // The shell is the PTY child.
        assert!(!is_foreground_job(pid(10), Some(pid(1)), pid(10), fish));
        assert!(is_foreground_job(pid(11), Some(pid(10)), pid(10), fish));
        // The shell runs under `login`.
        assert!(!is_foreground_job(pid(10), Some(pid(1)), pid(10), login));
        assert!(!is_foreground_job(pid(11), Some(pid(10)), pid(10), login));
        assert!(is_foreground_job(pid(12), Some(pid(11)), pid(10), login));
        assert!(is_foreground_job(pid(12), None, pid(10), login));
    }

    #[test]
    fn process_changed_works() {
        let info = |pid: u32, name: &str, cwd: &str| ProcessInfo {
            pid: Pid::from_u32(pid),
            name: name.to_owned(),
            cwd: PathBuf::from(cwd),
            argv: Vec::new(),
        };
        let shell = info(10, "fish", "/a");
        assert!(!process_changed(None, None));
        assert!(process_changed(None, Some(&shell)));
        assert!(process_changed(Some(&shell), None));
        assert!(!process_changed(Some(&shell), Some(&shell.clone())));
        assert!(process_changed(Some(&shell), Some(&info(10, "fish", "/b"))));
        assert!(process_changed(Some(&shell), Some(&info(11, "sleep", "/a"))));
        // A script run by the shell has the shell's name and directory.
        assert!(process_changed(Some(&shell), Some(&info(11, "fish", "/a"))));
    }
}
