use editor::Editor;
use editor::EditorElement;
use editor::EditorStyle;
use editor::MultiBufferOffset;
use editor::ToOffset as _;
use gpui::Action;
use gpui::App;
use gpui::ClickEvent;
use gpui::Context;
use gpui::Entity;
use gpui::FocusHandle;
use gpui::Hsla;
use gpui::IntoElement;
use gpui::TextStyle;
use gpui::Window;
use ui::IconButton;
use ui::IconButtonShape;
use ui::Tooltip;
use ui::font;
use ui::prelude::ButtonCommon as _;
use ui::prelude::Clickable as _;
use ui::prelude::Color;
use ui::prelude::Disableable as _;
use ui::prelude::Div;
use ui::prelude::FluentBuilder as _;
use ui::prelude::SharedString;
use ui::prelude::Styled as _;
use ui::prelude::Toggleable as _;
use ui::prelude::h_flex;
use ui::prelude::relative;
use ui::prelude::rems;

pub(super) enum HistoryNavigationDirection {
    Previous,
    Next,
}

pub(super) fn should_navigate_history(
    editor: &Entity<Editor>,
    direction: HistoryNavigationDirection,
    cx: &App,
) -> bool {
    let editor_ref = editor.read(cx);
    let snapshot = editor_ref.buffer().read(cx).snapshot(cx);
    if snapshot.max_point().row == 0 {
        return true;
    }
    let selections = editor_ref.selections.disjoint_anchors();
    if let [selection] = selections {
        let offset = selection.end.to_offset(&snapshot);
        match direction {
            HistoryNavigationDirection::Previous => offset == MultiBufferOffset(0),
            HistoryNavigationDirection::Next => offset == snapshot.len(),
        }
    } else {
        true
    }
}

pub(super) enum ActionButtonState {
    Disabled,
    Toggled,
}

/// A button that runs `handler` on the search bar that rendered it. Dispatching
/// the action instead would reach workspace-level handlers, which resolve the
/// active pane before a click in another pane has activated it.
pub(super) fn render_local_action_button<T: 'static, A: Action>(
    id_prefix: &'static str,
    icon: ui::IconName,
    button_state: Option<ActionButtonState>,
    tooltip: &'static str,
    action: &'static A,
    focus_handle: FocusHandle,
    cx: &Context<T>,
    handler: fn(&mut T, &A, &mut Window, &mut Context<T>),
) -> impl IntoElement {
    IconButton::new(SharedString::from(format!("{id_prefix}-{}", action.name())), icon)
        .shape(IconButtonShape::Square)
        .on_click(cx.listener({
            let focus_handle = focus_handle.clone();
            move |this, _: &ClickEvent, window, cx| {
                window.focus(&focus_handle, cx);
                handler(this, action, window, cx);
            }
        }))
        .tooltip(move |_window, cx| Tooltip::for_action_in(tooltip, action, &focus_handle, cx))
        .when_some(button_state, |this, state| match state {
            ActionButtonState::Toggled => this.toggle_state(true),
            ActionButtonState::Disabled => this.disabled(true),
        })
}

pub(crate) fn input_base_styles(border_color: Hsla, map: impl FnOnce(Div) -> Div) -> Div {
    h_flex().map(map).min_w_32().min_h_8().pl_2().pr_1().border_1().border_color(border_color).rounded_md()
}

pub(crate) fn render_text_input(editor: &Entity<Editor>, color_override: Option<Color>, app: &App) -> impl IntoElement {
    let (color, use_syntax) = if editor.read(app).read_only(app) {
        (theme::theme().colors().text_disabled, false)
    } else {
        match color_override {
            Some(color_override) => (color_override.color(), false),
            None => (theme::theme().colors().text, true),
        }
    };

    let text_style = TextStyle {
        color,
        font_family: font::buffer_font().family.clone(),
        font_features: font::buffer_font().features.clone(),
        font_size: rems(0.875).into(),
        font_weight: font::buffer_font().weight,
        line_height: relative(1.3),
        ..TextStyle::default()
    };

    let mut editor_style = EditorStyle {
        background: theme::theme().colors().toolbar_background,
        cursor_colors: theme::theme().cursor_colors(),
        text: text_style,
        ..EditorStyle::default()
    };
    if use_syntax {
        editor_style.syntax = theme::theme().syntax();
    }

    EditorElement::new(editor, editor_style)
}
