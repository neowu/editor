//! Process and main-thread CPU counters for `edctl perf`.

use std::time::Instant;

/// Cumulative CPU counters at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuSample {
    pub wall: Option<Instant>,
    pub process_user_ns: u64,
    pub process_system_ns: u64,
    pub instructions: u64,
    pub cycles: u64,
    /// CPU time of the calling thread.
    pub thread_ns: u64,
}

impl CpuSample {
    /// Sample the current process, and the calling thread's CPU time.
    pub fn now() -> CpuSample {
        let mut sample = CpuSample { wall: Some(Instant::now()), thread_ns: thread_cpu_ns(), ..CpuSample::default() };
        if let Some(usage) = process_rusage() {
            let (numer, denom) = timebase();
            sample.process_user_ns = mach_to_ns(usage.ri_user_time, numer, denom);
            sample.process_system_ns = mach_to_ns(usage.ri_system_time, numer, denom);
            sample.instructions = usage.ri_instructions;
            sample.cycles = usage.ri_cycles;
        }
        sample
    }

    /// Counters accumulated from `start` to `self`, as a report.
    pub fn since(&self, start: &CpuSample) -> control::CpuUsage {
        let wall_ms = match (self.wall, start.wall) {
            (Some(end), Some(start)) => end.saturating_duration_since(start).as_secs_f64() * 1000.,
            _ => 0.,
        };
        control::CpuUsage {
            wall_ms,
            process_user_ms: ns_to_ms(self.process_user_ns.saturating_sub(start.process_user_ns)),
            process_system_ms: ns_to_ms(self.process_system_ns.saturating_sub(start.process_system_ns)),
            instructions: self.instructions.saturating_sub(start.instructions),
            cycles: self.cycles.saturating_sub(start.cycles),
            main_thread_ms: ns_to_ms(self.thread_ns.saturating_sub(start.thread_ns)),
        }
    }
}

fn ns_to_ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.
}

fn mach_to_ns(ticks: u64, numer: u32, denom: u32) -> u64 {
    (u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1))) as u64
}

fn thread_cpu_ns() -> u64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `time` is a valid, writable timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut time) } != 0 {
        return 0;
    }
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

fn process_rusage() -> Option<libc::rusage_info_v4> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: the buffer is a `rusage_info_v4`, matching the requested flavor.
    let result = unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, usage.as_mut_ptr().cast()) };
    // SAFETY: the call succeeded and filled the buffer, which was zeroed before.
    (result == 0).then(|| unsafe { usage.assume_init() })
}

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
}

/// The mach absolute time unit: `ri_user_time` and `ri_system_time` are in it, not in nanoseconds.
fn timebase() -> (u32, u32) {
    let mut info = MachTimebaseInfo { numer: 1, denom: 1 };
    // SAFETY: `info` is a valid, writable struct with the C layout the function expects.
    unsafe { mach_timebase_info(&raw mut info) };
    (info.numer, info.denom)
}

#[cfg(test)]
mod tests {
    use super::CpuSample;
    use super::mach_to_ns;

    #[test]
    fn mach_ticks_scale_by_timebase() {
        assert_eq!(mach_to_ns(24, 125, 3), 1000);
        assert_eq!(mach_to_ns(7, 1, 1), 7);
        assert_eq!(mach_to_ns(7, 1, 0), 7);
    }

    #[test]
    fn since_subtracts_counters() {
        let start = CpuSample {
            wall: None,
            process_user_ns: 1_000_000,
            process_system_ns: 0,
            instructions: 10,
            cycles: 20,
            thread_ns: 500_000,
        };
        let end = CpuSample {
            wall: None,
            process_user_ns: 3_000_000,
            process_system_ns: 2_000_000,
            instructions: 110,
            cycles: 70,
            thread_ns: 2_500_000,
        };
        let usage = end.since(&start);
        assert_eq!(usage.wall_ms, 0.);
        assert_eq!(usage.process_user_ms, 2.);
        assert_eq!(usage.process_system_ms, 2.);
        assert_eq!(usage.instructions, 100);
        assert_eq!(usage.cycles, 50);
        assert_eq!(usage.main_thread_ms, 2.);
    }

    #[test]
    fn samples_grow_while_working() {
        let start = CpuSample::now();
        let mut sum = 0u64;
        for i in 0..5_000_000u64 {
            sum = std::hint::black_box(sum.wrapping_add(i * i));
        }
        let usage = CpuSample::now().since(&start);
        assert!(usage.main_thread_ms > 0., "{usage:?}");
        assert!(usage.process_user_ms > 0., "{usage:?}");
        assert!(usage.instructions > 0, "{usage:?}");
        assert!(usage.wall_ms > 0., "{usage:?}");
    }
}
