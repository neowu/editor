//! Opt-in per-window frame timing, read by external automation (`edctl perf`).

use std::time::Duration;

use crate::Window;

/// Frame timings collected since [`Window::reset_frame_stats`].
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    /// Render, layout and prepaint time of each frame, in order.
    builds: Vec<Duration>,
    /// Paint time of each frame, in order.
    paints: Vec<Duration>,
    /// Whole `Window::draw` time of each frame, in order.
    draws: Vec<Duration>,
    /// Time handing each frame's scene to the platform renderer.
    presents: Vec<Duration>,
    /// Lines the platform shaped while drawing each frame (line layout cache misses).
    shaped_lines: Vec<usize>,
}

/// Aggregates of [`FrameStats`], in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStatsSummary {
    pub frames: usize,
    pub presents: usize,
    pub draw: DurationSummary,
    pub build: DurationSummary,
    pub paint: DurationSummary,
    pub present: DurationSummary,
    pub shaped_lines: CountSummary,
}

/// Total and distribution of a per-frame count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CountSummary {
    pub total: usize,
    pub mean: f64,
    pub p95: usize,
    pub max: usize,
}

/// Total and distribution of a set of durations, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DurationSummary {
    pub total_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl FrameStats {
    pub(crate) fn record_draw(&mut self, build: Duration, paint: Duration, draw: Duration, shaped_lines: usize) {
        self.shaped_lines.push(shaped_lines);
        self.builds.push(build);
        self.paints.push(paint);
        self.draws.push(draw);
    }

    pub(crate) fn record_present(&mut self, present: Duration) {
        self.presents.push(present);
    }

    pub fn summary(&self) -> FrameStatsSummary {
        FrameStatsSummary {
            frames: self.draws.len(),
            presents: self.presents.len(),
            draw: DurationSummary::of(&self.draws),
            build: DurationSummary::of(&self.builds),
            paint: DurationSummary::of(&self.paints),
            present: DurationSummary::of(&self.presents),
            shaped_lines: CountSummary::of(&self.shaped_lines),
        }
    }
}

impl DurationSummary {
    fn of(durations: &[Duration]) -> DurationSummary {
        if durations.is_empty() {
            return DurationSummary::default();
        }
        let mut sorted: Vec<Duration> = durations.to_vec();
        sorted.sort_unstable();
        let total: Duration = sorted.iter().sum();
        let percentile = |p: f64| {
            let index = ((sorted.len() - 1) as f64 * p).round() as usize;
            millis(sorted[index])
        };
        DurationSummary {
            total_ms: millis(total),
            mean_ms: millis(total) / sorted.len() as f64,
            p50_ms: percentile(0.5),
            p95_ms: percentile(0.95),
            max_ms: millis(sorted[sorted.len() - 1]),
        }
    }
}

impl CountSummary {
    fn of(counts: &[usize]) -> CountSummary {
        if counts.is_empty() {
            return CountSummary::default();
        }
        let mut sorted: Vec<usize> = counts.to_vec();
        sorted.sort_unstable();
        let total: usize = sorted.iter().sum();
        CountSummary {
            total,
            mean: total as f64 / sorted.len() as f64,
            p95: sorted[((sorted.len() - 1) as f64 * 0.95).round() as usize],
            max: sorted[sorted.len() - 1],
        }
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}

impl Window {
    /// Start collecting [`FrameStats`] from now on, dropping any collected before.
    pub fn reset_frame_stats(&mut self) {
        self.frame_stats = Some(Box::default());
    }

    /// Frame timings since [`Window::reset_frame_stats`], or `None` if it was never called.
    pub fn frame_stats(&self) -> Option<&FrameStats> {
        self.frame_stats.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::CountSummary;
    use super::DurationSummary;
    use super::FrameStats;
    use super::FrameStatsSummary;

    fn assert_ms(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn empty_stats_summarize_to_zero() {
        assert_eq!(FrameStats::default().summary(), FrameStatsSummary::default());
    }

    #[test]
    fn summary_aggregates_each_phase() {
        let mut stats = FrameStats::default();
        for ms in 1..=20 {
            stats.record_draw(
                Duration::from_millis(ms),
                Duration::from_millis(1),
                Duration::from_millis(ms + 1),
                ms as usize,
            );
        }
        stats.record_present(Duration::from_millis(2));

        let summary = stats.summary();
        assert_eq!(summary.frames, 20);
        assert_eq!(summary.presents, 1);
        assert_ms(summary.build.total_ms, 210.);
        assert_ms(summary.build.mean_ms, 10.5);
        assert_ms(summary.build.max_ms, 20.);
        assert_ms(summary.paint.total_ms, 20.);
        assert_ms(summary.draw.max_ms, 21.);
        assert_ms(summary.present.total_ms, 2.);
        assert_eq!(summary.shaped_lines.total, 210);
        assert_eq!(summary.shaped_lines.max, 20);
    }

    #[test]
    fn count_summary_uses_nearest_rank() {
        let summary = CountSummary::of(&[0, 10, 0, 0, 5]);
        assert_eq!(summary, CountSummary { total: 15, mean: 3., p95: 10, max: 10 });
    }

    #[test]
    fn percentiles_use_nearest_rank_of_sorted_durations() {
        let durations: Vec<Duration> = [5, 1, 4, 2, 3].into_iter().map(Duration::from_millis).collect();
        let summary = DurationSummary::of(&durations);
        assert_eq!(summary.p50_ms, 3.);
        assert_eq!(summary.p95_ms, 5.);
        assert_eq!(summary.max_ms, 5.);
        assert_eq!(summary.mean_ms, 3.);
    }
}
