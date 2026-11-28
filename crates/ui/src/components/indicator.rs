use gpui::Hsla;
use gpui::prelude::FluentBuilder as _;

use super::AnyIcon;
use crate::Icon;
use crate::prelude::App;
use crate::prelude::Color;
use crate::prelude::IntoElement;
use crate::prelude::ParentElement as _;
use crate::prelude::RenderOnce;
use crate::prelude::Styled as _;
use crate::prelude::Window;
use crate::prelude::div;
use crate::prelude::rems_from_px;

#[allow(clippy::large_enum_variant)]
#[derive(Default)]
enum IndicatorKind {
    #[default]
    Dot,
    Bar,
    Icon(AnyIcon),
}

#[derive(IntoElement)]
pub struct Indicator {
    kind: IndicatorKind,
    border_color: Option<Color>,
    pub color: Color,
}

impl Indicator {
    pub const fn dot() -> Self {
        Self { kind: IndicatorKind::Dot, border_color: None, color: Color::Default }
    }

    pub const fn bar() -> Self {
        Self { kind: IndicatorKind::Bar, border_color: None, color: Color::Default }
    }

    pub fn icon(icon: impl Into<AnyIcon>) -> Self {
        Self { kind: IndicatorKind::Icon(icon.into()), border_color: None, color: Color::Default }
    }

    pub const fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub const fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }
}

impl RenderOnce for Indicator {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let container = div().flex_none();
        let container = if let Some(border_color) = self.border_color {
            if matches!(self.kind, IndicatorKind::Dot | IndicatorKind::Bar) {
                container.border_1().border_color(border_color.color())
            } else {
                container
            }
        } else {
            container
        };

        match self.kind {
            IndicatorKind::Icon(icon) => {
                container.child(icon.map(|icon| icon.custom_size(rems_from_px(8_f32)).color(self.color)))
            }
            IndicatorKind::Dot => container.w_1p5().h_1p5().rounded_full().bg(self.color.color()),
            IndicatorKind::Bar => container.w_full().h_1p5().rounded_t_sm().bg(self.color.color()),
        }
    }
}

#[derive(IntoElement)]
pub(crate) struct IconWithIndicator {
    icon: Icon,
    indicator: Option<Indicator>,
    indicator_border_color: Option<Hsla>,
}

impl IconWithIndicator {
    pub(crate) const fn new(icon: Icon, indicator: Option<Indicator>) -> Self {
        Self { icon, indicator, indicator_border_color: None }
    }

    pub(crate) const fn indicator_border_color(mut self, color: Option<Hsla>) -> Self {
        self.indicator_border_color = color;
        self
    }
}

impl RenderOnce for IconWithIndicator {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let indicator_border_color =
            self.indicator_border_color.unwrap_or_else(|| theme::theme().colors().elevated_surface_background);

        div().relative().child(self.icon).when_some(self.indicator, |this, indicator| {
            this.child(
                div()
                    .absolute()
                    .size_2p5()
                    .border_2()
                    .border_color(indicator_border_color)
                    .rounded_full()
                    .bottom_neg_0p5()
                    .right_neg_0p5()
                    .child(indicator),
            )
        })
    }
}
