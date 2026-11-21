use editor::Editor;
use encoding_rs::Encoding;
use encoding_rs::UTF_8;
use gpui::Context;
use gpui::Entity;
use gpui::IntoElement;
use gpui::ParentElement as _;
use gpui::Render;
use gpui::Styled as _;
use gpui::Subscription;
use gpui::WeakEntity;
use gpui::Window;
use gpui::div;
use ui::Button;
use ui::ButtonCommon as _;
use ui::Clickable as _;
use ui::LabelSize;
use ui::Tooltip;
use workspace::StatusItemView;
use workspace::Workspace;
use workspace::item::ItemHandle;
use workspace::status_item_color;

use crate::EncodingSelector;
use crate::Toggle;

pub struct ActiveBufferEncoding {
    active_encoding: Option<&'static Encoding>,
    workspace: WeakEntity<Workspace>,
    _observe_active_editor: Option<Subscription>,
    has_bom: bool,
    is_dirty: bool,
}

impl ActiveBufferEncoding {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            active_encoding: None,
            workspace: workspace.weak_handle(),
            _observe_active_editor: None,
            has_bom: false,
            is_dirty: false,
        }
    }

    fn update_encoding(&mut self, editor: Entity<Editor>, _: &mut Window, cx: &mut Context<Self>) {
        let state = editor.read(cx).active_buffer(cx).map_or((None, false, false), |buffer| {
            let buffer = buffer.read(cx);
            (Some(buffer.encoding()), buffer.has_bom(), buffer.is_dirty())
        });
        // Runs on every editor notify, so only redraw the status bar on a change.
        if state != (self.active_encoding, self.has_bom, self.is_dirty) {
            (self.active_encoding, self.has_bom, self.is_dirty) = state;
            cx.notify();
        }
    }
}

impl Render for ActiveBufferEncoding {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(active_encoding) = self.active_encoding else {
            return div().hidden();
        };

        if !is_non_standard_encoding(active_encoding, self.has_bom) {
            return div().hidden();
        }

        let mut text = active_encoding.name().to_owned();
        if self.has_bom {
            text.push_str(" (BOM)");
        }

        let (disabled, tooltip_text) =
            if self.is_dirty { (true, "Save file to change encoding") } else { (false, "Reopen with Encoding") };

        let open = self
            .workspace
            .upgrade()
            .is_some_and(|workspace| workspace.read(cx).active_modal::<EncodingSelector>(cx).is_some());

        div().child(
            Button::new("change-encoding", text)
                .label_size(LabelSize::Small)
                .color(status_item_color(open))
                .tab_index(0_isize)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if disabled {
                        return;
                    }
                    if let Some(workspace) = this.workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| EncodingSelector::toggle(workspace, window, cx));
                    }
                }))
                .tooltip(move |_window, cx| {
                    if disabled {
                        Tooltip::text(tooltip_text)(_window, cx)
                    } else {
                        Tooltip::for_action(tooltip_text, &Toggle, cx)
                    }
                }),
        )
    }
}

impl StatusItemView for ActiveBufferEncoding {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = active_pane_item.and_then(<dyn workspace::ItemHandle + 'static>::downcast::<Editor>) {
            self._observe_active_editor = Some(cx.observe_in(&editor, window, Self::update_encoding));
            self.update_encoding(editor, window, cx);
        } else {
            self.active_encoding = None;
            self.has_bom = false;
            self.is_dirty = false;
            self._observe_active_editor = None;
        }

        cx.notify();
    }
}

/// The status bar shows the encoding only when it is not plain UTF-8 (another
/// encoding, or UTF-8 with a BOM).
fn is_non_standard_encoding(encoding: &'static Encoding, has_bom: bool) -> bool {
    encoding != UTF_8 || has_bom
}

#[cfg(test)]
mod tests {
    use encoding_rs::UTF_16LE;
    use encoding_rs::WINDOWS_1252;

    use super::UTF_8;
    use super::is_non_standard_encoding;

    #[test]
    fn is_non_standard_encoding_works() {
        assert!(!is_non_standard_encoding(UTF_8, false));
        assert!(is_non_standard_encoding(UTF_8, true));
        assert!(is_non_standard_encoding(UTF_16LE, false));
        assert!(is_non_standard_encoding(WINDOWS_1252, false));
    }
}
