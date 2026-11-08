//! Shared motion math and global timing (`motion`): friction-style `Decay` for
//! surfaces, and a closed-form spring for motion that must keep velocity across
//! retargets (the cursor trail). Callers own their state and frame loop.

const CRITICAL_DAMPING_TOLERANCE: f32 = 1e-4;
/// ln(20): a decay is ~95% there after this many time constants.
const DECAY_TIME_CONSTANTS_TO_95: f32 = 2.995_732_3;
/// ω·t at which a critically damped spring released at rest is ~95% there.
const CRITICAL_OMEGA_T_TO_95: f32 = 4.743_864_4;

/// The physical parameters of a damped harmonic oscillator.
///
/// `stiffness` and `mass` must be finite and positive. `damping` must be finite
/// and non-negative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfig {
    /// The spring stiffness, conventionally written as $k$.
    pub stiffness: f32,
    /// The viscous damping coefficient, conventionally written as $c$.
    pub damping: f32,
    /// The moving mass, conventionally written as $m$.
    pub mass: f32,
}

impl SpringConfig {
    /// A unit-mass spring from its natural angular frequency and damping ratio.
    pub const fn from_canonical(natural_frequency: f32, damping_ratio: f32) -> Self {
        Self {
            stiffness: natural_frequency * natural_frequency,
            damping: 2.0 * damping_ratio * natural_frequency,
            mass: 1.0,
        }
    }

    /// A critically damped spring released at rest that covers ~95% of its distance in `settle_time` seconds.
    pub const fn critically_damped(settle_time: f32) -> Self {
        Self::from_canonical(CRITICAL_OMEGA_T_TO_95 / settle_time, 1.0)
    }

    /// Returns the natural angular frequency and damping ratio $(\omega_0, \zeta)$.
    pub fn canonical(&self) -> (f32, f32) {
        let natural_frequency = (self.stiffness / self.mass).sqrt();
        let damping_ratio = self.damping / (2.0 * (self.stiffness * self.mass).sqrt());
        (natural_frequency, damping_ratio)
    }

    /// Advances a spring toward a target that remains fixed for `delta_time`.
    ///
    /// This analytic step is independent of frame rate and preserves velocity,
    /// allowing an interrupted spring to be retargeted without restarting it.
    pub fn step(&self, state: SpringState, target: f32, delta_time: f32) -> SpringState {
        let propagator = self.propagator(delta_time);
        let displacement = state.position - target;
        SpringState {
            position: target + propagator[0][0] * displacement + propagator[0][1] * state.velocity,
            velocity: propagator[1][0] * displacement + propagator[1][1] * state.velocity,
        }
    }

    /// The exact state-transition matrix for a constant target over `delta_time`.
    fn propagator(&self, delta_time: f32) -> [[f32; 2]; 2] {
        let (natural_frequency, damping_ratio) = self.canonical();

        if damping_ratio < 1.0 - CRITICAL_DAMPING_TOLERANCE {
            let decay = damping_ratio * natural_frequency;
            let damped_frequency = natural_frequency * (1.0 - damping_ratio * damping_ratio).sqrt();
            let exponential = (-decay * delta_time).exp();
            let (sine, cosine) = (damped_frequency * delta_time).sin_cos();
            let sine_over_frequency = sine / damped_frequency;

            [
                [exponential * (cosine + decay * sine_over_frequency), exponential * sine_over_frequency],
                [
                    -exponential * natural_frequency * natural_frequency * sine_over_frequency,
                    exponential * (cosine - decay * sine_over_frequency),
                ],
            ]
        } else if damping_ratio > 1.0 + CRITICAL_DAMPING_TOLERANCE {
            let root = (damping_ratio * damping_ratio - 1.0).sqrt();
            let root_sum = damping_ratio + root;
            let slow_root = -natural_frequency / root_sum;
            let fast_root = -natural_frequency * root_sum;
            let denominator = slow_root - fast_root;
            let slow_exponential = (slow_root * delta_time).exp();
            let fast_exponential = (fast_root * delta_time).exp();

            [
                [
                    (-fast_root * slow_exponential + slow_root * fast_exponential) / denominator,
                    (slow_exponential - fast_exponential) / denominator,
                ],
                [
                    slow_root * fast_root * (fast_exponential - slow_exponential) / denominator,
                    (slow_root * slow_exponential - fast_root * fast_exponential) / denominator,
                ],
            ]
        } else {
            let exponential = (-natural_frequency * delta_time).exp();

            [
                [exponential * (1.0 + natural_frequency * delta_time), exponential * delta_time],
                [
                    -exponential * natural_frequency * natural_frequency * delta_time,
                    exponential * (1.0 - natural_frequency * delta_time),
                ],
            ]
        }
    }
}

/// Friction-style deceleration: exponential decay toward the target, fastest at
/// the start and slowing like a flung scroll, never overshooting. Frame-rate
/// independent; a retarget just continues from the current position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decay {
    /// Seconds for the remaining distance to shrink by a factor of e (~63% covered).
    pub time_constant: f32,
}

impl Decay {
    pub const fn new(time_constant: f32) -> Self {
        Self { time_constant }
    }

    /// A decay that covers ~95% of its distance in `settle_time` seconds.
    pub const fn settling_in(settle_time: f32) -> Self {
        Self::new(settle_time / DECAY_TIME_CONSTANTS_TO_95)
    }

    /// Fraction of the remaining distance still left after `delta_time` seconds.
    /// Compute once per frame and apply to every value sharing this decay.
    pub fn remaining(&self, delta_time: f32) -> f32 {
        (-delta_time / self.time_constant).exp()
    }

    pub fn step(&self, position: f32, target: f32, delta_time: f32) -> f32 {
        target + (position - target) * self.remaining(delta_time)
    }
}

/// Global motion tuning, like a theme for timing. Durations are seconds for a
/// motion to cover ~95% of its distance; every animation derives from `BASE`
/// through a ratio, so retuning `BASE` rescales all of them together.
pub mod motion {
    use super::Decay;

    /// Surfaces moving or arriving: panes and docks.
    pub const BASE: f32 = 0.12;
    /// Navigation that follows each keystroke (cursor trail, smooth scroll) is
    /// faster than surfaces, so it never feels like waiting on input.
    pub const NAVIGATION: f32 = 0.75 * BASE;

    /// Surfaces decelerate like friction, no overshoot.
    pub const GLIDE: Decay = Decay::settling_in(BASE);
}

/// The instantaneous position and velocity of a spring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpringState {
    /// The current value in the animated unit.
    pub position: f32,
    /// The current value's change per second.
    pub velocity: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_canonical_round_trips() {
        let (frequency, damping) = SpringConfig::from_canonical(12.0, 0.8).canonical();
        assert!((frequency - 12.0).abs() < 1e-4);
        assert!((damping - 0.8).abs() < 1e-4);
    }

    #[test]
    fn settle_time_constructors_cover_95_percent_in_settle_time() {
        let settle_time = 0.1;
        let decay = Decay::settling_in(settle_time);
        assert!((decay.step(1.0, 0.0, settle_time) - 0.05).abs() < 1e-4);

        let spring = SpringConfig::critically_damped(settle_time);
        let state = spring.step(SpringState { position: 1.0, velocity: 0.0 }, 0.0, settle_time);
        assert!((state.position - 0.05).abs() < 1e-4, "{state:?}");
        assert!((spring.canonical().1 - 1.0).abs() < 1e-4);

        const { assert!(motion::NAVIGATION < motion::BASE) };
        assert!((motion::GLIDE.step(1.0, 0.0, motion::BASE) - 0.05).abs() < 1e-4);
    }

    #[test]
    fn critical_step_matches_closed_form() {
        let omega = 32.0;
        let config = SpringConfig::from_canonical(omega, 1.0);
        let (x0, v0, t) = (10.0_f32, -40.0_f32, 0.01_f32);
        let next = config.step(SpringState { position: x0, velocity: v0 }, 0.0, t);
        let decay = (-omega * t).exp();
        assert!((next.position - (x0 + (x0 * omega + v0) * t) * decay).abs() < 1e-4);
        assert!((next.velocity - decay * (v0 - omega * t * (x0 * omega + v0))).abs() < 1e-3);
    }

    #[test]
    fn decay_decelerates_without_overshoot_and_is_frame_rate_independent() {
        let decay = motion::GLIDE;
        let (mut position, mut previous_step) = (100.0_f32, f32::INFINITY);
        for _ in 0..60 {
            let next = decay.step(position, 0.0, 1.0 / 120.0);
            assert!(next >= 0.0 && next < position);
            assert!(position - next < previous_step);
            previous_step = position - next;
            position = next;
        }
        let once = decay.step(100.0, 0.0, 0.02);
        let twice = decay.step(decay.step(100.0, 0.0, 0.01), 0.0, 0.01);
        assert!((once - twice).abs() < 1e-4);
        assert!((decay.remaining(decay.time_constant) - (-1.0_f32).exp()).abs() < 1e-6);
    }

    #[test]
    fn spring_retarget_keeps_velocity_and_every_damping_settles() {
        for damping_ratio in [0.5, 1.0, 2.0] {
            let config = SpringConfig::from_canonical(30.0, damping_ratio);
            let moving = config.step(SpringState { position: 0.0, velocity: 0.0 }, 1.0, 0.02);
            assert!(moving.velocity > 0.0);
            // Retargeting continues from the current position and velocity.
            let retargeted = config.step(moving, -1.0, 0.0);
            assert!((retargeted.position - moving.position).abs() < 1e-5);
            assert!((retargeted.velocity - moving.velocity).abs() < 1e-4);
            let mut state = moving;
            for _ in 0..240 {
                state = config.step(state, 1.0, 1.0 / 120.0);
            }
            assert!((state.position - 1.0).abs() < 1e-3, "ζ={damping_ratio} ended at {state:?}");
        }
    }
}
