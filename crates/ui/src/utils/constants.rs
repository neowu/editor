use gpui::Pixels;
use gpui::px;

use crate::DynamicSpacing;

/// Gap between the cards and the window's edges, including the gap above the
/// window's top row (the tab bars beside the window header), which moves the
/// window.
pub const CARD_GAP: Pixels = px(CARD_GAP_PX);
const CARD_GAP_PX: f32 = 8.;

/// The tab row's height at the default UI font size (`Tab::container_height`).
const DEFAULT_TAB_ROW_PX: f32 = DynamicSpacing::Base32.base_px() + DynamicSpacing::Base04.base_px();

/// The macOS traffic lights are 14px and don't scale with the UI.
const TRAFFIC_LIGHT_SIZE_PX: f32 = 14.;

// Use pixels here instead of a rem-based size because the macOS traffic
// lights are a static size, and don't scale with the rest of the UI.
//
/// Where the macOS traffic lights sit: as far in from the left edge as the
/// window's rounded corner needs, and on the centre line of the top row's tab
/// row below the card gap (at the default UI font size), with everything else
/// in that row.
pub const TRAFFIC_LIGHT_POSITION: (f32, f32) =
    (16., CARD_GAP_PX + DEFAULT_TAB_ROW_PX / 2. - TRAFFIC_LIGHT_SIZE_PX / 2.);

/// Room after the close button. With the window header's 4px gap and its
/// first button's 8px padding, the header's first text sits as far right of
/// the close button as the button is from the window's left edge (16px), and
/// that button's hover background 8px from it, the header's control spacing.
const TRAFFIC_LIGHT_MARGIN_PX: f32 = 4.;

/// Room from the window's left edge to the end of the traffic lights plus a
/// margin. Only the close button is shown (`gpui_macos` hides minimize and
/// zoom), so this is the same on every macOS version.
pub const TRAFFIC_LIGHT_PADDING: f32 = TRAFFIC_LIGHT_POSITION.0 + TRAFFIC_LIGHT_SIZE_PX + TRAFFIC_LIGHT_MARGIN_PX;

#[cfg(test)]
mod tests {
    use super::CARD_GAP_PX;
    use super::DEFAULT_TAB_ROW_PX;
    use super::TRAFFIC_LIGHT_PADDING;
    use super::TRAFFIC_LIGHT_POSITION;
    use super::TRAFFIC_LIGHT_SIZE_PX;

    #[test]
    fn test_traffic_lights_on_tab_row_centre_line() {
        assert_eq!(DEFAULT_TAB_ROW_PX, 36.);
        assert_eq!(TRAFFIC_LIGHT_POSITION, (16., 19.));
        assert_eq!(TRAFFIC_LIGHT_POSITION.1 + TRAFFIC_LIGHT_SIZE_PX / 2., CARD_GAP_PX + DEFAULT_TAB_ROW_PX / 2.);
    }

    #[test]
    fn test_traffic_light_padding_fits_close_button_only() {
        assert_eq!(TRAFFIC_LIGHT_PADDING, 34.);
    }
}
