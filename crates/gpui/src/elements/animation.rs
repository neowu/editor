use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

pub use easing::ease_in_out;
pub use easing::ease_out_quint;
pub(crate) use easing::linear;
pub use easing::pulsating_between;
use smallvec::SmallVec;

use crate::AnyElement;
use crate::App;
use crate::Element;
use crate::ElementId;
use crate::GlobalElementId;
use crate::IntoElement;
use crate::ParentElement;
use crate::Window;

/// An animation that can be applied to an element.
#[derive(Clone)]
pub struct Animation {
    /// The amount of time for which this animation should run.
    duration: Duration,
    /// Whether to repeat this animation when it finishes.
    oneshot: bool,
    /// Whether to derive the phase from a shared clock. See [`Animation::repeat_synced`].
    synced: bool,
    /// A function that maps normalized time to an animated value.
    /// The result may exceed 0..1 for easing functions that overshoot.
    easing: Rc<dyn Fn(f32) -> f32>,
}

impl Animation {
    /// Create a new animation with the given duration.
    /// By default the animation will only run once and will use a linear easing function.
    pub fn new(duration: Duration) -> Self {
        Self { duration, oneshot: true, synced: false, easing: Rc::new(linear) }
    }

    /// Set the animation to loop when it finishes.
    pub const fn repeat(mut self) -> Self {
        self.oneshot = false;
        self
    }

    /// Sets the easing function used to map normalized time to an animated value.
    ///
    /// The output is not clamped, allowing physical easing functions such as
    /// springs to overshoot.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }
}

/// An extension trait for adding the animation wrapper to both Elements and Components.
pub trait AnimationExt {
    /// Render this component or element with an animation.
    fn with_animation(
        self,
        id: impl Into<ElementId>,
        animation: Animation,
        animator: impl Fn(Self, f32) -> Self + 'static,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: Box::new(move |this, _, value| animator(this, value)),
            animations: smallvec::smallvec![animation],
        }
    }

    /// Render this component or element with a chain of animations.
    fn with_animations(
        self,
        id: impl Into<ElementId>,
        animations: Vec<Animation>,
        animator: impl Fn(Self, usize, f32) -> Self + 'static,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: Box::new(animator),
            animations: animations.into(),
        }
    }
}

impl<E: IntoElement + 'static> AnimationExt for E {}

/// A GPUI element that applies an animation to another element.
pub struct AnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    animations: SmallVec<[Animation; 1]>,
    animator: Box<dyn Fn(E, usize, f32) -> E + 'static>,
}

impl<E: ParentElement> ParentElement for AnimationElement<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        let Some(element) = &mut self.element else {
            return;
        };

        element.extend(elements);
    }
}

impl<E> AnimationElement<E> {
    /// Returns a new [`AnimationElement<E>`] after applying the given function
    /// to the element being animated.
    pub fn map_element(mut self, f: impl FnOnce(E) -> E) -> AnimationElement<E> {
        self.element = self.element.map(f);
        self
    }
}

impl<E: IntoElement + 'static> IntoElement for AnimationElement<E> {
    type Element = AnimationElement<E>;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct AnimationState {
    start: Instant,
    animation_ix: usize,
}

impl<E: IntoElement + 'static> Element for AnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        window.with_element_state(global_id.expect("animation elements always have an id"), |state, window| {
            let mut state = state.unwrap_or_else(|| AnimationState { start: Instant::now(), animation_ix: 0 });
            let animation_ix = state.animation_ix;
            let duration = self.animations[animation_ix].duration;

            let elapsed = if self.animations[animation_ix].synced && !duration.is_zero() {
                let elapsed = cx.background_executor().now() - cx.synced_animation_epoch;
                // Reduce modulo the duration before f32 conversion, which loses sub-second precision at scale.
                Duration::from_nanos((elapsed.as_nanos() % duration.as_nanos()) as u64)
            } else {
                state.start.elapsed()
            };
            let mut delta = elapsed.as_secs_f32() / duration.as_secs_f32();

            let mut done = false;
            if delta > 1.0 {
                if self.animations[animation_ix].oneshot {
                    if animation_ix >= self.animations.len() - 1 {
                        done = true;
                    } else {
                        state.start = Instant::now();
                        state.animation_ix += 1;
                    }
                    delta = 1.0;
                } else {
                    delta = delta.fract();
                }
            }
            let delta = (self.animations[animation_ix].easing)(delta);

            let element = self.element.take().expect("should only be called once");
            let mut element = (self.animator)(element, animation_ix, delta).into_any_element();

            if !done {
                window.request_animation_frame();
            }

            ((element.request_layout(window, cx), element), state)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        (): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

mod easing {
    use std::f32::consts::PI;

    /// The linear easing function, or delta itself.
    pub(crate) const fn linear(delta: f32) -> f32 {
        delta
    }

    /// The quadratic ease-in-out function, which starts and ends slowly but speeds up in the middle.
    pub fn ease_in_out(delta: f32) -> f32 {
        if delta < 0.5 {
            2.0 * delta * delta
        } else {
            let x = -2.0 * delta + 2.0;
            1.0 - x * x / 2.0
        }
    }

    /// The Quint ease-out function, which starts quickly and decelerates to a stop.
    pub fn ease_out_quint() -> impl Fn(f32) -> f32 {
        move |delta| 1.0 - (1.0 - delta).powi(5)
    }

    /// A custom easing function for pulsating alpha that slows down as it approaches 0.1.
    pub fn pulsating_between(min: f32, max: f32) -> impl Fn(f32) -> f32 {
        let range = max - min;

        move |delta| {
            // Use a combination of sine and cubic functions for a more natural breathing rhythm
            let t = (delta * 2.0 * PI).sin();
            let breath = f32::midpoint(t * t * t, t);

            // Map the breath to our desired alpha range
            let normalized_alpha = f32::midpoint(breath, 1.0);

            min + (normalized_alpha * range)
        }
    }
}
