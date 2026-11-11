//! UI-related utilities

use gpui::App;
use theme::ActiveTheme;

use crate::font;

mod apca_contrast;
mod color_contrast;
mod constants;
mod control_characters;
mod corner_solver;
mod highlight_contrast;
mod search_input;
mod window_drag;
mod with_rem_size;

#[allow(clippy::wildcard_imports)]
pub use apca_contrast::*;
#[allow(clippy::wildcard_imports)]
pub use color_contrast::*;
#[allow(clippy::wildcard_imports)]
pub use constants::*;
#[allow(clippy::wildcard_imports)]
pub use control_characters::*;
pub use corner_solver::inner_corner_radius;
#[allow(clippy::wildcard_imports)]
pub use highlight_contrast::*;
#[allow(clippy::wildcard_imports)]
pub use search_input::*;
pub use window_drag::window_drag_area;
#[allow(clippy::wildcard_imports)]
pub use with_rem_size::*;

/// Returns true if the current theme is light or vibrant light.
pub fn is_light(cx: &mut App) -> bool {
    cx.theme().appearance.is_light()
}

pub fn ui_text_style(cx: &App) -> gpui::TextStyle {
    let ui_font = font::ui_font();
    gpui::TextStyle {
        color: cx.theme().colors().text,
        font_family: ui_font.family.clone(),
        font_features: ui_font.features.clone(),
        font_fallbacks: ui_font.fallbacks.clone(),
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

/// Capitalizes the first character of a string.
///
/// This function takes a string slice as input and returns a new `String` with the first character
/// capitalized.
pub fn capitalize(str: &str) -> String {
    let mut chars = str.chars();
    match chars.next() {
        None => String::new(),
        Some(first_char) => first_char.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

pub fn format_number_with_commas(value: u128) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + (digits.len() - 1) / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

#[cfg(test)]
mod tests {

    use super::capitalize;

    #[test]
    fn capitalize_first_letter() {
        assert_eq!(capitalize("hello"), "Hello");
        assert_eq!(capitalize("WORLD"), "WORLD");
        assert_eq!(capitalize(""), "");
    }
}
