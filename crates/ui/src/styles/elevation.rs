use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use gpui::BoxShadow;
use gpui::Hsla;
use gpui::hsla;
use gpui::px;
use theme::Appearance;

/// Today, elevation is primarily used to add shadows to elements, and set the correct background for elements like buttons.
///
/// Elevation can be thought of as the physical closeness of an element to the
/// user. Elements with lower elevations are physically further away on the
/// z-axis and appear to be underneath elements with higher elevations.
///
/// In the future, a more complete approach to elevation may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationIndex {
    /// On the layer of the app background. This is under panels, panes, and
    /// other surfaces.
    Background,
    /// The primary surface – Contains panels, panes, containers, etc.
    Surface,
    /// The same elevation as the primary surface, but used for the editable areas, like buffers.
    EditorSurface,
    /// A surface that is elevated above the primary surface. but below washes, models, and dragged elements.
    ElevatedSurface,
    /// A surface above the [ElevationIndex::ElevatedSurface] that is used for dialogs, alerts, modals, etc.
    ModalSurface,
}

impl Display for ElevationIndex {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            ElevationIndex::Background => write!(f, "Background"),
            ElevationIndex::Surface => write!(f, "Surface"),
            ElevationIndex::EditorSurface => write!(f, "Editor Surface"),
            ElevationIndex::ElevatedSurface => write!(f, "Elevated Surface"),
            ElevationIndex::ModalSurface => write!(f, "Modal Surface"),
        }
    }
}

impl ElevationIndex {
    /// Returns an appropriate shadow for the given elevation index.
    pub fn shadow(self) -> Vec<BoxShadow> {
        let is_light = theme::theme().appearance() == Appearance::Light;

        match self {
            ElevationIndex::Surface => vec![],
            ElevationIndex::EditorSurface => vec![],

            ElevationIndex::ElevatedSurface => vec![
                BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.12)).blur_radius(px(3.)),
                BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., if is_light { 0.03 } else { 0.06 })),
            ],

            ElevationIndex::ModalSurface => vec![
                BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., if is_light { 0.06 } else { 0.12 }))
                    .blur_radius(px(3.)),
                BoxShadow::new(px(0.), px(3.), hsla(0., 0., 0., if is_light { 0.06 } else { 0.08 }))
                    .blur_radius(px(6.)),
                BoxShadow::new(px(0.), px(6.), hsla(0., 0., 0., 0.04)).blur_radius(px(12.)),
                BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., if is_light { 0.04 } else { 0.12 })),
            ],

            ElevationIndex::Background => vec![],
        }
    }

    /// Returns the background color for the given elevation index.
    pub fn bg(&self) -> Hsla {
        match self {
            ElevationIndex::Background => theme::theme().colors().background,
            ElevationIndex::Surface => theme::theme().colors().surface_background,
            ElevationIndex::EditorSurface => theme::theme().colors().editor_background,
            ElevationIndex::ElevatedSurface => theme::theme().colors().elevated_surface_background,
            ElevationIndex::ModalSurface => theme::theme().colors().elevated_surface_background,
        }
    }

    /// Returns a color that is appropriate a filled element on this elevation.
    pub fn on_elevation_bg(&self) -> Hsla {
        match self {
            ElevationIndex::Background => theme::theme().colors().surface_background,
            ElevationIndex::Surface => theme::theme().colors().background,
            ElevationIndex::EditorSurface => theme::theme().colors().surface_background,
            ElevationIndex::ElevatedSurface => theme::theme().colors().background,
            ElevationIndex::ModalSurface => theme::theme().colors().background,
        }
    }

    /// Attempts to return a darker background color than the current elevation index's background.
    ///
    /// If the current background color is already dark, it will return a lighter color instead.
    pub fn darker_bg(&self) -> Hsla {
        match self {
            ElevationIndex::Background => theme::theme().colors().surface_background,
            ElevationIndex::Surface => theme::theme().colors().editor_background,
            ElevationIndex::EditorSurface => theme::theme().colors().surface_background,
            ElevationIndex::ElevatedSurface => theme::theme().colors().editor_background,
            ElevationIndex::ModalSurface => theme::theme().colors().editor_background,
        }
    }
}
