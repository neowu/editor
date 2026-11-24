use gpui::WindowBackgroundAppearance;

/// Returns the [WindowBackgroundAppearance].
fn window_appearance() -> WindowBackgroundAppearance {
    theme::theme().styles.window_background_appearance
}

/// Returns if the window and it's surfaces are expected
/// to be transparent.
///
/// Helps determine if you need to take extra steps to prevent
/// transparent backgrounds.
pub fn theme_is_transparent() -> bool {
    matches!(window_appearance(), WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred)
}
