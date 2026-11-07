use gpui::Hsla;

use crate::prelude::ActiveTheme;
use crate::prelude::App;
use crate::prelude::ElementId;
use crate::prelude::FluentBuilder;
use crate::prelude::InteractiveElement;
use crate::prelude::IntoElement;
use crate::prelude::ParentElement;
use crate::prelude::RenderOnce;
use crate::prelude::Styled;
use crate::prelude::Window;
use crate::prelude::div;
use crate::prelude::px;
use crate::prelude::relative;

/// A progress bar is a horizontal bar that communicates the status of a process.
///
/// A progress bar should not be used to represent indeterminate progress.
#[derive(IntoElement)]
pub struct ProgressBar {
    id: ElementId,
    value: f32,
    max_value: f32,
    bg_color: Hsla,
    over_color: Hsla,
    fg_color: Hsla,
}

impl ProgressBar {
    pub fn new(id: impl Into<ElementId>, value: f32, max_value: f32, cx: &App) -> Self {
        Self {
            id: id.into(),
            value,
            max_value,
            bg_color: cx.theme().colors().background,
            over_color: cx.theme().status().error,
            fg_color: cx.theme().status().info,
        }
    }

    /// Sets the current value of the progress bar.
    pub const fn value(mut self, value: f32) -> Self {
        self.value = value;
        self
    }

    /// Sets the background color of the progress bar.
    pub const fn bg_color(mut self, color: Hsla) -> Self {
        self.bg_color = color;
        self
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let fill_width = (self.value / self.max_value).clamp(0.02, 1.0);

        div()
            .id(self.id.clone())
            .w_full()
            .h_2()
            .p_0p5()
            .rounded_full()
            .bg(self.bg_color)
            .shadow(vec![gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.08))])
            .child(
                div()
                    .h_full()
                    .rounded_full()
                    .when(self.value > self.max_value, |div| div.bg(self.over_color))
                    .when(self.value <= self.max_value, |div| div.bg(self.fg_color))
                    .w(relative(fill_width)),
            )
    }
}
