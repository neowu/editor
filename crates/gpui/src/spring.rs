use std::ops::RangeInclusive;

use crate::Hsla;
use crate::Pixels;
use crate::Rems;
use crate::Rgba;

const CRITICAL_DAMPING_TOLERANCE: f32 = 1e-4;
const DEFAULT_SPRING_EPSILON: f32 = 0.001;

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
    /// Creates a spring from its physical parameters.
    pub const fn new(stiffness: f32, damping: f32, mass: f32) -> Self {
        Self { stiffness, damping, mass }
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

    /// Returns the exact state-transition matrix for a constant target.
    ///
    /// Materializing this matrix is useful when many springs share the same
    /// configuration and frame delta. A matrix must not be reused when the
    /// frame delta changes.
    pub fn propagator(&self, delta_time: f32) -> [[f32; 2]; 2] {
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

    /// Tests both displacement and velocity against a positional tolerance.
    ///
    /// Velocity is compared with `epsilon * natural_frequency`, giving it the
    /// corresponding animated-units-per-second scale.
    pub fn is_settled(&self, state: SpringState, target: f32, epsilon: f32) -> bool {
        let (natural_frequency, _) = self.canonical();
        epsilon.is_finite()
            && epsilon >= 0.0
            && (state.position - target).abs() <= epsilon
            && state.velocity.abs() <= epsilon * natural_frequency
    }
}

/// The instantaneous position and velocity of a spring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpringState {
    /// The current value in the animated unit.
    pub position: f32,
    /// The current value's change per second.
    pub velocity: f32,
}

/// A value that can be targeted by a one-dimensional spring.
///
/// Implementations may project the spring coordinate into a richer output,
/// allowing a discrete state or a path through a multidimensional value to be
/// driven by one spring.
pub trait SpringTarget: 'static {
    /// The value supplied to the spring animator.
    type Output;

    /// Returns the target in the spring's coordinate space.
    fn target(&self) -> f32;

    /// Projects a spring coordinate into the animated output.
    fn resolve(&self, value: f32) -> Self::Output;
}

impl SpringTarget for f32 {
    type Output = f32;

    fn target(&self) -> f32 {
        *self
    }

    fn resolve(&self, value: f32) -> Self::Output {
        value
    }
}

impl SpringTarget for Pixels {
    type Output = Pixels;

    fn target(&self) -> f32 {
        self.as_f32()
    }

    fn resolve(&self, value: f32) -> Self::Output {
        Pixels::from(value)
    }
}

impl SpringTarget for Rems {
    type Output = Rems;

    fn target(&self) -> f32 {
        self.0
    }

    fn resolve(&self, value: f32) -> Self::Output {
        Rems(value)
    }
}

impl SpringTarget for bool {
    type Output = AnimationPhase;

    fn target(&self) -> f32 {
        if *self { 1.0 } else { 0.0 }
    }

    fn resolve(&self, value: f32) -> Self::Output {
        AnimationPhase(value)
    }
}

/// A potentially overshooting coordinate within an animation.
///
/// Phases are not restricted to 0..1. Multi-stage animations can assign each
/// stage its own coordinate and interpolate over ranges such as 1..=2.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct AnimationPhase(
    /// The unbounded phase coordinate.
    pub f32,
);

impl AnimationPhase {
    /// Restricts this phase to the bounds of a range.
    pub fn clamp(self, range: RangeInclusive<f32>) -> Self {
        let (first, second) = range.into_inner();
        Self(self.0.clamp(first.min(second), first.max(second)))
    }

    /// Interpolates between values using 0 and 1 as their phase coordinates.
    pub fn interpolate<T: Interpolate>(self, from: T, to: T) -> T {
        T::interpolate(from, to, self.0)
    }
}

impl From<f32> for AnimationPhase {
    fn from(value: f32) -> Self {
        Self(value)
    }
}

impl From<bool> for AnimationPhase {
    fn from(value: bool) -> Self {
        Self(if value { 1.0 } else { 0.0 })
    }
}

impl SpringTarget for AnimationPhase {
    type Output = AnimationPhase;

    fn target(&self) -> f32 {
        self.0
    }

    fn resolve(&self, value: f32) -> Self::Output {
        Self(value)
    }
}

/// A value that supports linear interpolation and extrapolation.
pub trait Interpolate: Sized {
    /// Resolves the value at `phase`, where 0 is `from` and 1 is `to`.
    fn interpolate(from: Self, to: Self, phase: f32) -> Self;
}

impl Interpolate for f32 {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        from + (to - from) * phase
    }
}

impl Interpolate for Pixels {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        from + (to - from) * phase
    }
}

impl Interpolate for Rems {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        from + (to - from) * phase
    }
}

impl Interpolate for Rgba {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        Self {
            r: f32::interpolate(from.r, to.r, phase),
            g: f32::interpolate(from.g, to.g, phase),
            b: f32::interpolate(from.b, to.b, phase),
            a: f32::interpolate(from.a, to.a, phase),
        }
    }
}

impl Interpolate for Hsla {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        let hue_delta = (to.h - from.h + 0.5).rem_euclid(1.0) - 0.5;
        Self {
            h: (from.h + hue_delta * phase).rem_euclid(1.0),
            s: f32::interpolate(from.s, to.s, phase),
            l: f32::interpolate(from.l, to.l, phase),
            a: f32::interpolate(from.a, to.a, phase),
        }
    }
}

/// Controls how a spring advances and resolves its presentation value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpringPlayback {
    /// Advances toward the latest target, preserving velocity across retargets.
    #[default]
    Running,
    /// Holds the current position and velocity until playback resumes.
    Paused,
    /// Holds the current position and discards velocity.
    Stopped,
    /// Snaps to the latest target and discards velocity.
    Completed,
    /// Returns to the initial value and discards velocity.
    Cancelled,
}

/// A stateful spring animation targeting a value or projected path.
#[derive(Clone, Debug)]
pub struct SpringAnimation<T = ()> {
    pub(crate) config: SpringConfig,
    pub(crate) target: T,
    pub(crate) epsilon: f32,
    pub(crate) initial: Option<f32>,
    pub(crate) playback: SpringPlayback,
}

impl SpringAnimation<()> {
    /// Creates a spring animation builder.
    pub const fn new(config: SpringConfig) -> Self {
        Self { config, target: (), epsilon: DEFAULT_SPRING_EPSILON, initial: None, playback: SpringPlayback::Running }
    }

    /// Sets the value or path targeted by this spring.
    pub const fn to<T: SpringTarget>(self, target: T) -> SpringAnimation<T> {
        let SpringAnimation { config, target: (), epsilon, initial, playback } = self;
        SpringAnimation { config, target, epsilon, initial, playback }
    }
}

impl<T: SpringTarget> SpringAnimation<T> {
    /// Sets the coordinate used when this element has no prior spring state.
    pub fn from(mut self, initial: T) -> Self {
        self.initial = Some(initial.target());
        self
    }
}
