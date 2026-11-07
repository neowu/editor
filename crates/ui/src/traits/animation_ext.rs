use std::time::Duration;

use gpui::Animation;
use gpui::AnimationElement;
use gpui::AnimationExt;
use gpui::Transformation;
use gpui::percentage;

use crate::prelude::ElementId;
use crate::traits::transformable::Transformable;

/// An extension trait for adding common animations to animatable components.
pub trait CommonAnimationExt: AnimationExt {
    /// Render this component as rotating over the given duration.
    ///
    /// NOTE: This method uses the location of the caller to generate an ID for this state.
    ///       If this is not sufficient to identify your state (e.g. you're rendering a list item),
    ///       you can provide a custom ElementID using the `use_keyed_rotate_animation` method.
    #[track_caller]
    fn with_rotate_animation(self, duration: u64) -> AnimationElement<Self>
    where
        Self: Transformable + Sized,
    {
        self.with_keyed_rotate_animation(ElementId::CodeLocation(*std::panic::Location::caller()), duration)
    }

    /// Render this component as rotating with the given element ID over the given duration.
    fn with_keyed_rotate_animation(self, id: impl Into<ElementId>, duration: u64) -> AnimationElement<Self>
    where
        Self: Transformable + Sized,
    {
        self.with_animation(id, rotate_animation(duration), |component, delta| {
            component.transform(Transformation::rotate(percentage(delta)))
        })
    }
}

impl<T: AnimationExt> CommonAnimationExt for T {}

/// Re-renders per second for rotating spinners. A spinner shown while a
/// language server indexes would otherwise redraw the window every vsync.
const ROTATE_ANIMATION_MAX_FPS: f32 = 10.;

fn rotate_animation(duration: u64) -> Animation {
    Animation::new(Duration::from_secs(duration)).repeat_synced().with_max_fps(ROTATE_ANIMATION_MAX_FPS)
}

#[cfg(test)]
mod tests {

    use super::Duration;
    use super::ROTATE_ANIMATION_MAX_FPS;
    use super::rotate_animation;

    #[test]
    fn rotate_animation_repeats_synced_and_is_throttled() {
        let animation = rotate_animation(2);
        assert_eq!(animation.duration, Duration::from_secs(2));
        assert!(!animation.oneshot);
        assert!(animation.synced);
        assert_eq!(animation.max_fps, Some(ROTATE_ANIMATION_MAX_FPS));
    }
}
