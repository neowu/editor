use gpui::Div;
use gpui::div;
use gpui::prelude::Styled;

/// Creates a horizontal group with tight, consistent spacing.
///
/// xs: ~2px @16px/rem
pub fn h_group_sm() -> Div {
    div().flex().gap_0p5()
}
