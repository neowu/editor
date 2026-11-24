use std::rc::Rc;

use gpui::AnyView;
use gpui::ClickEvent;
use gpui::ElementId;
use gpui::Hsla;
use gpui::IntoElement;
use gpui::Styled as _;
use gpui::Window;
use gpui::div;
use gpui::prelude::FluentBuilder as _;
use gpui::prelude::InteractiveElement as _;
use gpui::prelude::ParentElement as _;
use gpui::prelude::RenderOnce;
use gpui::prelude::StatefulInteractiveElement as _;

use crate::Color;
use crate::ElevationIndex;
use crate::Icon;
use crate::IconName;
use crate::KeyBinding;
use crate::ToggleState;
use crate::prelude::App;
use crate::prelude::DynamicSpacing;
use crate::prelude::IconSize;
use crate::prelude::Label;
use crate::prelude::LabelCommon as _;
use crate::prelude::LabelSize;
use crate::prelude::Pixels;
use crate::prelude::SharedString;
use crate::prelude::h_flex;
use crate::prelude::px;
use crate::utils::is_light;

// TODO: Checkbox, CheckboxWithLabel, and Switch could all be
// restructured to use a ToggleLike, similar to Button/Buttonlike, Label/Labellike

/// Creates a new checkbox.
pub fn checkbox(id: impl Into<ElementId>, toggle_state: ToggleState) -> Checkbox {
    Checkbox::new(id, toggle_state)
}

/// Creates a new switch.
pub fn switch(id: impl Into<ElementId>, toggle_state: ToggleState) -> Switch {
    Switch::new(id, toggle_state)
}

/// The visual style of a toggle.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum ToggleStyle {
    /// Toggle has a transparent background.
    #[default]
    Ghost,
    /// Toggle has a filled background based on the
    /// elevation index of the parent container.
    ElevationBased(ElevationIndex),
    /// A custom style using a color to tint the toggle.
    Custom(Hsla),
}

/// # Checkbox
///
/// Checkboxes are used for multiple choices, not for mutually exclusive choices.
/// Each checkbox works independently from other checkboxes in the list,
/// therefore checking an additional box does not affect any other selections.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    toggle_state: ToggleState,
    style: ToggleStyle,
    disabled: bool,
    placeholder: bool,
    filled: bool,
    visualization: bool,
    label: Option<SharedString>,
    label_size: LabelSize,
    label_color: Color,
    #[allow(clippy::type_complexity)]
    tooltip: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView>>,
    #[allow(clippy::type_complexity)]
    on_click: Option<Box<dyn Fn(&ToggleState, &ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Checkbox {
    /// Creates a new [`Checkbox`].
    pub fn new(id: impl Into<ElementId>, checked: ToggleState) -> Self {
        Self {
            id: id.into(),
            toggle_state: checked,
            style: ToggleStyle::default(),
            disabled: false,
            placeholder: false,
            filled: false,
            visualization: false,
            label: None,
            label_size: LabelSize::Default,
            label_color: Color::Muted,
            tooltip: None,
            on_click: None,
        }
    }

    /// Sets the disabled state of the [`Checkbox`].
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the disabled state of the [`Checkbox`].
    pub const fn placeholder(mut self, placeholder: bool) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// Binds a handler to the [`Checkbox`] that will be called when clicked.
    pub fn on_click(mut self, handler: impl Fn(&ToggleState, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(move |state, _, window, cx| handler(state, window, cx)));
        self
    }

    pub fn on_click_ext(
        mut self,
        handler: impl Fn(&ToggleState, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Sets the `fill` setting of the checkbox, indicating whether it should be filled.
    pub const fn fill(mut self) -> Self {
        self.filled = true;
        self
    }

    /// Makes the checkbox look enabled but without pointer cursor and hover styles.
    /// Primarily used for uninteractive markdown previews.
    pub const fn visualization_only(mut self, visualization: bool) -> Self {
        self.visualization = visualization;
        self
    }

    /// Sets the style of the checkbox using the specified [`ToggleStyle`].
    pub const fn style(mut self, style: ToggleStyle) -> Self {
        self.style = style;
        self
    }

    /// Match the style of the checkbox to the current elevation using [`ToggleStyle::ElevationBased`].
    pub const fn elevation(mut self, elevation: ElevationIndex) -> Self {
        self.style = ToggleStyle::ElevationBased(elevation);
        self
    }

    /// Sets the tooltip for the checkbox.
    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }

    /// Set the label for the checkbox.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub const fn label_size(mut self, size: LabelSize) -> Self {
        self.label_size = size;
        self
    }

    pub const fn label_color(mut self, color: Color) -> Self {
        self.label_color = color;
        self
    }

    fn bg_color(&self) -> Hsla {
        let style = self.style.clone();
        match (style, self.filled) {
            (ToggleStyle::Ghost, false) => theme::theme().colors().ghost_element_background,
            (ToggleStyle::Ghost, true) => theme::theme().colors().element_background,
            (ToggleStyle::ElevationBased(_), false) => gpui::transparent_black(),
            (ToggleStyle::ElevationBased(elevation), true) => elevation.darker_bg(),
            (ToggleStyle::Custom(_), false) => gpui::transparent_black(),
            (ToggleStyle::Custom(color), true) => color.opacity(0.2),
        }
    }

    fn border_color(&self) -> Hsla {
        if self.disabled {
            return theme::theme().colors().border_variant;
        }

        match self.style.clone() {
            ToggleStyle::Ghost => theme::theme().colors().border,
            ToggleStyle::ElevationBased(_) => theme::theme().colors().border,
            ToggleStyle::Custom(color) => color.opacity(0.3),
        }
    }

    pub const fn container_size() -> Pixels {
        px(20.0)
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let group_id = format!("checkbox_group_{:?}", self.id);
        let color = if self.disabled { Color::Disabled } else { Color::Selected };

        let icon = match self.toggle_state {
            ToggleState::Selected => {
                if self.placeholder {
                    None
                } else {
                    Some(Icon::new(IconName::Check).size(IconSize::Small).color(color))
                }
            }
            ToggleState::Indeterminate => Some(Icon::new(IconName::Dash).size(IconSize::Small).color(color)),
            ToggleState::Unselected => None,
        };

        let bg_color = self.bg_color();
        let border_color = self.border_color();
        let hover_border_color = border_color.alpha(0.7);

        let size = Self::container_size();

        let checkbox = h_flex().group(group_id.clone()).id(self.id.clone()).size(size).justify_center().child(
            div()
                .flex()
                .flex_none()
                .justify_center()
                .items_center()
                .m_1()
                .size_4()
                .rounded_xs()
                .bg(bg_color)
                .border_1()
                .border_color(border_color)
                .when(self.disabled, |this| this.cursor_not_allowed())
                .when(self.disabled, |this| this.bg(theme::theme().colors().element_disabled.opacity(0.6)))
                .when(!self.disabled && !self.visualization, |this| {
                    this.group_hover(group_id.clone(), |el| el.border_color(hover_border_color))
                })
                .when(self.placeholder, |this| {
                    this.child(div().flex_none().rounded_full().bg(color.color().alpha(0.5)).size(px(4.)))
                })
                .children(icon),
        );

        h_flex()
            .id(self.id)
            .map(|this| {
                if self.disabled {
                    this.cursor_not_allowed()
                } else if self.visualization {
                    this.cursor_default()
                } else {
                    this.cursor_pointer()
                }
            })
            .gap(DynamicSpacing::Base06.rems())
            .child(checkbox)
            .when_some(self.label, |this, label| {
                this.child(Label::new(label).color(self.label_color).size(self.label_size))
            })
            .when_some(self.tooltip, |this, tooltip| this.tooltip(move |window, cx| tooltip(window, cx)))
            .when_some(self.on_click.filter(|_| !self.disabled), |this, on_click| {
                this.on_click(move |click, window, cx| on_click(&self.toggle_state.inverse(), click, window, cx))
            })
    }
}

/// Defines the color for a switch component.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Default)]
pub enum SwitchColor {
    #[default]
    Accent,
    Custom(Hsla),
}

impl SwitchColor {
    fn get_colors(&self, is_on: bool) -> (Hsla, Hsla) {
        if !is_on {
            return (theme::theme().colors().element_disabled, theme::theme().colors().border);
        }

        match self {
            SwitchColor::Accent => {
                let status = theme::theme().status();
                let colors = theme::theme().colors();
                (status.info.opacity(0.4), colors.text_accent.opacity(0.2))
            }
            SwitchColor::Custom(color) => (*color, color.opacity(0.6)),
        }
    }
}

impl From<SwitchColor> for Color {
    fn from(color: SwitchColor) -> Self {
        match color {
            SwitchColor::Accent => Color::Accent,
            SwitchColor::Custom(_) => Color::Default,
        }
    }
}

/// Defines the color for a switch component.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Default)]
pub enum SwitchLabelPosition {
    Start,
    #[default]
    End,
}

/// # Switch
///
/// Switches are used to represent opposite states, such as enabled or disabled.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    toggle_state: ToggleState,
    disabled: bool,
    #[allow(clippy::type_complexity)]
    on_click: Option<Rc<dyn Fn(&ToggleState, &mut Window, &mut App) + 'static>>,
    label: Option<SharedString>,
    label_position: Option<SwitchLabelPosition>,
    label_size: LabelSize,
    label_color: Color,
    full_width: bool,
    key_binding: Option<KeyBinding>,
    color: SwitchColor,
    tab_index: Option<isize>,
}

impl Switch {
    /// Creates a new [`Switch`].
    pub fn new(id: impl Into<ElementId>, state: ToggleState) -> Self {
        Self {
            id: id.into(),
            toggle_state: state,
            disabled: false,
            on_click: None,
            label: None,
            label_position: None,
            label_size: LabelSize::Small,
            label_color: Color::Default,
            full_width: false,
            key_binding: None,
            color: SwitchColor::default(),
            tab_index: None,
        }
    }

    /// Sets the color of the switch using the specified [`SwitchColor`].
    pub const fn color(mut self, color: SwitchColor) -> Self {
        self.color = color;
        self
    }

    /// Sets the disabled state of the [`Switch`].
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Binds a handler to the [`Switch`] that will be called when clicked.
    pub fn on_click(mut self, handler: impl Fn(&ToggleState, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Sets the label of the [`Switch`].
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn label_position(mut self, label_position: impl Into<Option<SwitchLabelPosition>>) -> Self {
        self.label_position = label_position.into();
        self
    }

    pub const fn label_size(mut self, size: LabelSize) -> Self {
        self.label_size = size;
        self
    }

    pub const fn label_color(mut self, color: Color) -> Self {
        self.label_color = color;
        self
    }

    pub const fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// Display the keybinding that triggers the switch action.
    pub fn key_binding(mut self, key_binding: impl Into<Option<KeyBinding>>) -> Self {
        self.key_binding = key_binding.into();
        self
    }

    pub fn tab_index(mut self, tab_index: impl Into<isize>) -> Self {
        self.tab_index = Some(tab_index.into());
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_on = self.toggle_state == ToggleState::Selected;
        let adjust_ratio = if is_light() { 1.5 } else { 1.0 };

        let base_color = theme::theme().colors().text;
        let thumb_color = base_color;
        let (bg_color, border_color) = self.color.get_colors(is_on);

        let bg_hover_color = if is_on {
            bg_color.blend(base_color.opacity(0.16 * adjust_ratio))
        } else {
            bg_color.blend(base_color.opacity(0.05 * adjust_ratio))
        };

        let thumb_opacity = match (is_on, self.disabled) {
            (_, true) => 0.2,
            (true, false) => 1.0,
            (false, false) => 0.5,
        };

        let group_id = format!("switch_group_{:?}", self.id);
        let label = self.label;
        let switch = div()
            .id((self.id.clone(), "switch"))
            .p(px(1.0))
            .border_2()
            .border_color(theme::theme().colors().border_transparent)
            .rounded_full()
            .when_some(self.tab_index.filter(|_| !self.disabled), |this, tab_index| {
                this.tab_index(tab_index)
                    .focus_visible(|mut style| {
                        style.border_color = Some(theme::theme().colors().border_focused);
                        style
                    })
                    .when_some(self.on_click.clone(), |this, on_click| {
                        this.on_click(move |_, window, cx| on_click(&self.toggle_state.inverse(), window, cx))
                    })
            })
            .child(
                h_flex()
                    .w(DynamicSpacing::Base32.rems())
                    .h(DynamicSpacing::Base20.rems())
                    .group(group_id.clone())
                    .child(
                        h_flex()
                            .when(is_on, |on| on.justify_end())
                            .when(!is_on, |off| off.justify_start())
                            .size_full()
                            .rounded_full()
                            .px(DynamicSpacing::Base02.px(cx))
                            .bg(bg_color)
                            .when(!self.disabled, |this| this.group_hover(group_id.clone(), |el| el.bg(bg_hover_color)))
                            .border_1()
                            .border_color(border_color)
                            .child(
                                div()
                                    .size(DynamicSpacing::Base12.rems())
                                    .rounded_full()
                                    .bg(thumb_color)
                                    .opacity(thumb_opacity),
                            ),
                    ),
            );

        h_flex()
            .id(self.id)
            .cursor_pointer()
            .gap(DynamicSpacing::Base06.rems())
            .when(self.full_width, |this| this.w_full().justify_between())
            .when(self.label_position == Some(SwitchLabelPosition::Start), |this| {
                this.when_some(label.clone(), |this, label| {
                    this.child(Label::new(label).size(self.label_size).color(self.label_color))
                })
            })
            .child(switch)
            .when(self.label_position == Some(SwitchLabelPosition::End), |this| {
                this.when_some(label, |this, label| {
                    this.child(Label::new(label).size(self.label_size).color(self.label_color))
                })
            })
            .children(self.key_binding)
            .when_some(self.on_click.filter(|_| !self.disabled), |this, on_click| {
                this.on_click(move |_, window, cx| on_click(&self.toggle_state.inverse(), window, cx))
            })
    }
}
