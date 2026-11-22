use gpui::AnyElement;
use smallvec::SmallVec;

use crate::Label;
use crate::prelude::App;
use crate::prelude::Color;
use crate::prelude::DynamicSpacing;
use crate::prelude::FluentBuilder as _;
use crate::prelude::IntoElement;
use crate::prelude::LabelCommon as _;
use crate::prelude::ParentElement;
use crate::prelude::RenderOnce;
use crate::prelude::Styled as _;
use crate::prelude::Window;
use crate::prelude::v_flex;

/// Vertical list that shows "No items" when it has no children.
#[derive(IntoElement, Default)]
pub struct List {
    children: SmallVec<[AnyElement; 2]>,
}

impl List {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ParentElement for List {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for List {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        v_flex().w_full().py(DynamicSpacing::Base04.rems()).map(|this| {
            if self.children.is_empty() {
                this.px_2().child(Label::new("No items").color(Color::Muted))
            } else {
                this.children(self.children)
            }
        })
    }
}
