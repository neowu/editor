//! UI-related utilities.

use gpui::App;

use crate::font;

mod constants;
mod control_characters;
mod corner_solver;
mod highlight_contrast;
mod search_input;
mod window_drag;
mod with_rem_size;

pub use constants::CARD_GAP;
pub use constants::TRAFFIC_LIGHT_PADDING;
pub use constants::TRAFFIC_LIGHT_POSITION;
pub(crate) use control_characters::replace_control_characters;
pub(crate) use control_characters::replace_control_characters_remapping_offsets;
pub(crate) use corner_solver::inner_corner_radius;
pub use highlight_contrast::ensure_highlight_contrast;
pub use highlight_contrast::merge_highlight_backgrounds;
pub use search_input::SearchInputWidth;
pub use theme::contrast::apca_contrast;
pub use theme::contrast::ensure_minimum_contrast;
pub use window_drag::window_drag_area;
pub use with_rem_size::WithRemSize;

pub fn ui_text_style(cx: &App) -> gpui::TextStyle {
    let ui_font = font::ui_font();
    gpui::TextStyle {
        color: theme::theme().colors().text,
        font_family: ui_font.family.clone(),
        font_features: ui_font.features.clone(),
        font_size: gpui::AbsoluteLength::from(crate::TextSize::Default.rems(cx)),
        font_weight: ui_font.weight,
        line_height: gpui::relative(1.),
        ..gpui::TextStyle::default()
    }
}

/// Returns the platform-appropriate label for the "reveal in file manager" action.
pub const fn reveal_in_file_manager_label() -> &'static str {
    "Reveal in Finder"
}
