use std::any::Any;
use std::sync::Arc;

use gpui::AnyElement;
use gpui::App;
use gpui::Context;
use gpui::Entity;
use gpui::FocusHandle;
use gpui::Focusable as _;
use gpui::FontStyle;
use gpui::Subscription;
use gpui::TextStyle;
use gpui::Window;
use gpui::prelude::Element as _;
use gpui::relative;
use ui::ErasedEditor;
use ui::font;
use ui::prelude::rems;

use super::Editor;
use super::EditorEvent;
use super::EditorStyle;
use super::SelectionEffects;
use crate::actions::SelectAll;
use crate::element::EditorElement;
use crate::scroll::Autoscroll;

impl Editor {
    pub fn erased(&self, cx: &Context<Self>) -> Arc<dyn ErasedEditor> {
        Arc::new(ErasedEditorImpl(cx.entity()))
    }
}

#[derive(Clone)]
pub(super) struct ErasedEditorImpl(Entity<Editor>);

impl ui::ErasedEditor for ErasedEditorImpl {
    fn text(&self, cx: &App) -> String {
        self.0.read(cx).text(cx)
    }

    fn set_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |this, cx| {
            this.set_text(text, window, cx);
        });
    }

    fn clear(&self, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |this, cx| this.clear(window, cx));
    }

    fn set_placeholder_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |this, cx| {
            this.set_placeholder_text(text, window, cx);
        });
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.read(cx).focus_handle(cx)
    }

    fn render(&self, _: &mut Window, _: &App) -> AnyElement {
        let theme_color = theme::theme().colors();

        let text_style = TextStyle {
            font_family: font::ui_font().family.clone(),
            font_features: font::ui_font().features.clone(),
            font_size: rems(0.875).into(),
            font_weight: font::ui_font().weight,
            font_style: FontStyle::Normal,
            line_height: relative(1.2),
            color: theme_color.text,
            ..Default::default()
        };
        let editor_style = EditorStyle {
            background: theme_color.ghost_element_background,
            cursor_colors: theme::theme().cursor_colors(),
            syntax: theme::theme().syntax(),
            text: text_style,
            ..Default::default()
        };
        EditorElement::new(&self.0, editor_style).into_any()
    }

    fn as_any(&self) -> &dyn Any {
        &self.0
    }

    fn move_selection_to_end(&self, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |editor, cx| {
            let editor_offset = editor.buffer().read(cx).len(cx);
            editor.change_selections(SelectionEffects::scroll(Autoscroll::Next), window, cx, |s| {
                s.select_ranges(Some(editor_offset..editor_offset));
            });
        });
    }

    fn select_all(&self, window: &mut Window, cx: &mut App) {
        self.0.update(cx, |editor, cx| {
            editor.select_all(&SelectAll, window, cx);
        });
    }

    fn subscribe(
        &self,
        mut callback: Box<dyn FnMut(ui::ErasedEditorEvent, &mut Window, &mut App) + 'static>,
        window: &mut Window,
        cx: &mut App,
    ) -> Subscription {
        window.subscribe(&self.0, cx, move |_, event: &EditorEvent, window, cx| {
            let event = match event {
                EditorEvent::BufferEdited => ui::ErasedEditorEvent::BufferEdited,
                EditorEvent::Blurred => ui::ErasedEditorEvent::Blurred,
                _ => return,
            };
            (callback)(event, window, cx);
        })
    }

    fn set_masked(&self, masked: bool, _window: &mut Window, cx: &mut App) {
        self.0.update(cx, |editor, cx| {
            editor.set_masked(masked, cx);
        });
    }

    fn set_read_only(&self, read_only: bool, cx: &mut App) {
        self.0.update(cx, |editor, cx| {
            editor.set_read_only(read_only);
            cx.notify();
        });
    }
}
