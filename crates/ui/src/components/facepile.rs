use gpui::AnyElement;
use gpui::StyleRefinement;
use smallvec::SmallVec;

use crate::prelude::App;
use crate::prelude::Div;
use crate::prelude::FluentBuilder;
use crate::prelude::IntoElement;
use crate::prelude::ParentElement;
use crate::prelude::RenderOnce;
use crate::prelude::Styled;
use crate::prelude::Window;
use crate::prelude::div;

/// An element that displays a collection of (usually) faces stacked
/// horizontally, with the left-most face on top, visually descending
/// from left to right.
///
/// Facepiles are used to display a group of people or things,
/// such as a list of participants in a collaboration session.
#[derive(IntoElement)]
pub struct Facepile {
    base: Div,
    faces: SmallVec<[AnyElement; 2]>,
}

impl Facepile {
    /// Creates a new empty facepile.
    pub fn empty() -> Self {
        Self::new(SmallVec::new())
    }

    /// Creates a new facepile with the given faces.
    pub fn new(faces: SmallVec<[AnyElement; 2]>) -> Self {
        Self { base: div(), faces }
    }
}

impl ParentElement for Facepile {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.faces.extend(elements);
    }
}

// Style methods.
impl Facepile {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }

    gpui::padding_style_methods!({
        visibility: pub
    });
}

impl RenderOnce for Facepile {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // Lay the faces out in reverse so they overlap in the desired order (left to right, front to back)
        self.base.flex().flex_row_reverse().items_center().justify_start().children(
            self.faces
                .into_iter()
                .enumerate()
                .rev()
                .map(|(ix, player)| div().when(ix > 0, |div| div.ml_neg_1()).child(player)),
        )
    }
}
