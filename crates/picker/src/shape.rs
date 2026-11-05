use gpui::Pixels;
use gpui::Rems;
use gpui::Window;

/// Room kept free around a modal: it sits 5 rems from the top and stays clear
/// of the bottom bar.
const VERTICAL_CHROME: Rems = Rems(10.0);
/// Floor (320px) for the picker's max height.
const MIN_HEIGHT: Rems = Rems(20.0);

/// The picker's size: a fixed width and a max height it shrinks below to fit
/// its content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shape {
    pub(crate) width: Rems,
    pub(crate) max_height: Rems,
}

impl Default for Shape {
    fn default() -> Self {
        Self { width: crate::DEFAULT_MODAL_WIDTH, max_height: crate::DEFAULT_MODAL_MAX_HEIGHT }
    }
}

impl Shape {
    /// The picker's width, kept within 95% of the viewport but never narrower
    /// than it was asked to be.
    pub(crate) fn width(&self, window: &Window) -> Pixels {
        let width = self.width * window.rem_size();
        clamp_size(width, width, window.viewport_size().width * 0.95)
    }

    /// The height the picker's results may grow to, kept clear of the window
    /// chrome.
    pub(crate) fn max_height(&self, window: &Window) -> Pixels {
        let rem = window.rem_size();
        let max = (window.viewport_size().height - VERTICAL_CHROME * rem) * 0.95;
        clamp_size(self.max_height * rem, MIN_HEIGHT * rem, max)
    }
}

/// Clamps `value` to `max`, then raises it to `min`, so `min` wins when the
/// bounds cross (e.g. a tiny window).
fn clamp_size(value: Pixels, min: Pixels, max: Pixels) -> Pixels {
    value.min(max).max(min)
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn clamp_size_limits_to_max() {
        assert_eq!(clamp_size(px(500.), px(100.), px(300.)), px(300.));
    }

    #[test]
    fn clamp_size_raises_to_min() {
        assert_eq!(clamp_size(px(50.), px(100.), px(300.)), px(100.));
    }

    #[test]
    fn clamp_size_prefers_min_when_bounds_cross() {
        assert_eq!(clamp_size(px(200.), px(400.), px(300.)), px(400.));
    }

    #[test]
    fn default_shape_uses_modal_defaults() {
        let shape = Shape::default();
        assert_eq!(shape.width, crate::DEFAULT_MODAL_WIDTH);
        assert_eq!(shape.max_height, crate::DEFAULT_MODAL_MAX_HEIGHT);
    }
}
