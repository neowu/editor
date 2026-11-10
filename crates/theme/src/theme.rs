pub mod built_in;
mod styles;

use gpui::App;
use gpui::Hsla;
use gpui::SharedString;
use gpui::WindowBackgroundAppearance;

#[allow(clippy::wildcard_imports)]
pub use crate::styles::*;

/// The appearance of the theme.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Appearance {
    /// A light appearance.
    Light,
    /// A dark appearance.
    Dark,
}

impl Appearance {
    /// Returns whether the appearance is light.
    pub const fn is_light(&self) -> bool {
        match self {
            Self::Light => true,
            Self::Dark => false,
        }
    }
}

/// Implementing this trait allows accessing the active theme.
pub trait ActiveTheme {
    /// Returns the theme. It is constant, so this never depends on app state.
    fn theme(&self) -> &'static Theme;
}

impl ActiveTheme for App {
    #[inline(always)]
    fn theme(&self) -> &'static Theme {
        built_in::theme()
    }
}

/// A theme is the primary mechanism for defining the appearance of the UI.
#[derive(Debug, PartialEq)]
pub struct Theme {
    /// The unique identifier for the theme.
    pub id: String,
    /// The name of the theme.
    pub name: SharedString,
    /// The appearance of the theme (light or dark).
    pub appearance: Appearance,
    /// The colors and other styles for the theme.
    pub styles: ThemeStyles,
}

impl Theme {
    /// Returns the [`SystemColors`] for the theme.
    #[inline(always)]
    pub const fn system(&self) -> &SystemColors {
        &self.styles.system
    }

    /// Returns the [`AccentColors`] for the theme.
    #[inline(always)]
    pub const fn accents(&self) -> &AccentColors {
        &self.styles.accents
    }

    /// Returns the [`CursorColors`] (cursor and selection) for the theme.
    #[inline(always)]
    pub const fn cursor_colors(&self) -> CursorColors {
        self.styles.cursor_colors
    }

    /// Returns the [`ThemeColors`] for the theme.
    #[inline(always)]
    pub const fn colors(&self) -> &ThemeColors {
        &self.styles.colors
    }

    /// Returns the [`SyntaxTheme`] for the theme.
    #[inline(always)]
    pub const fn syntax(&self) -> &SyntaxTheme {
        &self.styles.syntax
    }

    /// Returns the [`StatusColors`] for the theme.
    #[inline(always)]
    pub const fn status(&self) -> &StatusColors {
        &self.styles.status
    }

    /// Returns the [`Appearance`] for the theme.
    #[inline(always)]
    pub const fn appearance(&self) -> Appearance {
        self.appearance
    }

    /// Returns the [`WindowBackgroundAppearance`] for the theme.
    #[inline(always)]
    pub const fn window_background_appearance(&self) -> WindowBackgroundAppearance {
        self.styles.window_background_appearance
    }

    /// Darkens the color by reducing its lightness.
    /// The resulting lightness is clamped to ensure it doesn't go below 0.0.
    ///
    /// The first value darkens light appearance mode, the second darkens appearance dark mode.
    ///
    /// Note: This is a tentative solution and may be replaced with a more robust color system.
    pub fn darken(&self, color: Hsla, light_amount: f32, dark_amount: f32) -> Hsla {
        let amount = match self.appearance {
            Appearance::Light => light_amount,
            Appearance::Dark => dark_amount,
        };
        let mut hsla = color;
        hsla.l = (hsla.l - amount).max(0.0);
        hsla
    }
}
