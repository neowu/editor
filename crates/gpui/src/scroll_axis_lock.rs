//! Locks pixel scroll gestures to their initially dominant axis.

use std::time::Duration;

use scheduler::Instant;

use crate::Axis;
use crate::IsZero;
use crate::Pixels;
use crate::Point;
use crate::TouchPhase;
use crate::px;

const SCROLL_EVENT_SEPARATION: Duration = Duration::from_millis(28);

fn dominant_axis(delta: Point<Pixels>) -> Axis {
    if delta.x.abs() <= delta.y.abs() { Axis::Vertical } else { Axis::Horizontal }
}

const fn lock_delta_to_axis(delta: &mut Point<Pixels>, axis: Axis) {
    match axis {
        Axis::Vertical => delta.x = Pixels::ZERO,
        Axis::Horizontal => delta.y = Pixels::ZERO,
    }
}

/// Tracks the dominant axis across the events in a scroll gesture.
#[derive(Clone, Copy, Debug, Default)]
pub struct OngoingScroll {
    last_event: Option<Instant>,
    axis: Option<Axis>,
}

impl OngoingScroll {
    /// Filters the given delta to the dominant axis of the current scroll gesture.
    ///
    /// Gestures are delimited by their touch phase when available, with a timeout
    /// fallback for platforms that only emit [`TouchPhase::Moved`].
    pub fn filter(&mut self, delta: &mut Point<Pixels>, touch_phase: TouchPhase) {
        self.filter_at(delta, touch_phase, Instant::now())
    }

    fn filter_at(&mut self, delta: &mut Point<Pixels>, touch_phase: TouchPhase, now: Instant) {
        const UNLOCK_PERCENT: f32 = 1.9;
        const UNLOCK_LOWER_BOUND: Pixels = px(6.);

        if matches!(touch_phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.last_event = None;
            self.axis = None;
            return;
        }

        let x = delta.x.abs();
        let y = delta.y.abs();
        if x.is_zero() && y.is_zero() {
            if touch_phase == TouchPhase::Started {
                self.last_event = None;
                self.axis = None;
            }
            return;
        }

        let starts_new_gesture = touch_phase == TouchPhase::Started
            || self.last_event.is_none_or(|last_event| now.duration_since(last_event) >= SCROLL_EVENT_SEPARATION);
        let mut axis = self.axis;
        if starts_new_gesture {
            axis = Some(dominant_axis(*delta));
        } else if x.max(y) >= UNLOCK_LOWER_BOUND {
            match axis {
                Some(Axis::Vertical) if x > y && x >= y * UNLOCK_PERCENT => {
                    axis = None;
                }
                Some(Axis::Horizontal) if y > x && y >= x * UNLOCK_PERCENT => {
                    axis = None;
                }
                _ => {}
            }
        }

        self.last_event = Some(now);
        self.axis = axis;
        if let Some(axis) = axis {
            lock_delta_to_axis(delta, axis);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Axis;
    use super::Duration;
    use super::Instant;
    use super::OngoingScroll;
    use super::Pixels;
    use super::Point;
    use super::SCROLL_EVENT_SEPARATION;
    use super::TouchPhase;
    use super::dominant_axis;
    use super::lock_delta_to_axis;
    use super::px;
    use crate::point;

    fn filtered(scroll: &mut OngoingScroll, delta: Point<Pixels>, phase: TouchPhase, now: Instant) -> Point<Pixels> {
        let mut delta = delta;
        scroll.filter_at(&mut delta, phase, now);
        delta
    }

    #[test]
    fn dominant_axis_prefers_vertical_on_tie() {
        assert_eq!(dominant_axis(point(px(3.), px(3.))), Axis::Vertical);
        assert_eq!(dominant_axis(point(px(4.), px(3.))), Axis::Horizontal);
    }

    #[test]
    fn lock_delta_to_axis_zeroes_cross_axis() {
        let mut delta = point(px(2.), px(5.));
        lock_delta_to_axis(&mut delta, Axis::Vertical);
        assert_eq!(delta, point(px(0.), px(5.)));
        let mut delta = point(px(2.), px(5.));
        lock_delta_to_axis(&mut delta, Axis::Horizontal);
        assert_eq!(delta, point(px(2.), px(0.)));
    }

    #[test]
    fn gesture_locks_to_initial_axis() {
        let now = Instant::now();
        let mut scroll = OngoingScroll::default();
        assert_eq!(filtered(&mut scroll, point(px(1.), px(10.)), TouchPhase::Started, now), point(px(0.), px(10.)));
        let later = now + Duration::from_millis(5);
        assert_eq!(filtered(&mut scroll, point(px(3.), px(4.)), TouchPhase::Moved, later), point(px(0.), px(4.)));
    }

    #[test]
    fn strong_cross_axis_motion_unlocks() {
        let now = Instant::now();
        let mut scroll = OngoingScroll::default();
        filtered(&mut scroll, point(px(0.), px(10.)), TouchPhase::Started, now);
        let later = now + Duration::from_millis(5);
        assert_eq!(filtered(&mut scroll, point(px(20.), px(2.)), TouchPhase::Moved, later), point(px(20.), px(2.)));
    }

    #[test]
    fn ended_phase_resets_and_passes_delta_through() {
        let now = Instant::now();
        let mut scroll = OngoingScroll::default();
        filtered(&mut scroll, point(px(0.), px(10.)), TouchPhase::Started, now);
        assert_eq!(filtered(&mut scroll, point(px(5.), px(1.)), TouchPhase::Ended, now), point(px(5.), px(1.)));
        assert!(scroll.axis.is_none());
    }

    #[test]
    fn timeout_starts_new_gesture_without_phase() {
        let now = Instant::now();
        let mut scroll = OngoingScroll::default();
        filtered(&mut scroll, point(px(0.), px(10.)), TouchPhase::Moved, now);
        let later = now + SCROLL_EVENT_SEPARATION;
        assert_eq!(filtered(&mut scroll, point(px(10.), px(1.)), TouchPhase::Moved, later), point(px(10.), px(0.)));
    }
}
