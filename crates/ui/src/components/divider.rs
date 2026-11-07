use gpui::Hsla;
use gpui::IntoElement;
use gpui::Refineable as _;
use gpui::StyleRefinement;

use crate::prelude::ActiveTheme;
use crate::prelude::App;
use crate::prelude::Div;
use crate::prelude::FluentBuilder;
use crate::prelude::RenderOnce;
use crate::prelude::Styled;
use crate::prelude::Window;
use crate::prelude::div;

#[derive(Clone, Copy, PartialEq)]
enum DividerDirection {
    Horizontal,
    Vertical,
}

/// The color of a [`Divider`].
#[derive(Default)]
pub enum DividerColor {
    Border,
    BorderFaded,
    #[default]
    BorderVariant,
}

impl DividerColor {
    pub fn hsla(self, cx: &mut App) -> Hsla {
        match self {
            DividerColor::Border => cx.theme().colors().border,
            DividerColor::BorderFaded => cx.theme().colors().border.opacity(0.6),
            DividerColor::BorderVariant => cx.theme().colors().border_variant,
        }
    }
}

#[derive(IntoElement)]
pub struct Divider {
    direction: DividerDirection,
    color: DividerColor,
    style: StyleRefinement,
    inset: bool,
}

impl Divider {
    pub fn horizontal() -> Self {
        Self {
            direction: DividerDirection::Horizontal,
            color: DividerColor::default(),
            style: StyleRefinement::default(),
            inset: false,
        }
    }

    pub fn vertical() -> Self {
        Self {
            direction: DividerDirection::Vertical,
            color: DividerColor::default(),
            style: StyleRefinement::default(),
            inset: false,
        }
    }

    pub const fn inset(mut self) -> Self {
        self.inset = true;
        self
    }

    pub const fn color(mut self, color: DividerColor) -> Self {
        self.color = color;
        self
    }

    pub fn render_solid(self, base: Div, cx: &mut App) -> impl IntoElement {
        base.bg(self.color.hsla(cx))
    }
}

impl Styled for Divider {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Divider {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut base = match self.direction {
            DividerDirection::Horizontal => {
                div().min_w_0().h_px().max_h_px().w_full().when(self.inset, |this| this.mx_1p5())
            }
            DividerDirection::Vertical => div().min_w_0().w_px().h_4().when(self.inset, |this| this.my_1p5()),
        };

        base.style().refine(&self.style);

        self.render_solid(base, cx)
    }
}
