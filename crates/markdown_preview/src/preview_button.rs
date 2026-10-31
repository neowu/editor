use editor::Editor;
use gpui::AnyElement;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::Subscription;
use gpui::WeakEntity;
use ui::prelude::*;
use workspace::ItemHandle;
use workspace::ToolbarItemEvent;
use workspace::ToolbarItemLocation;
use workspace::ToolbarItemView;
use workspace::Workspace;

use crate::markdown_preview_view::MarkdownPreviewView;

/// Adds the tab bar's preview button for Markdown and HTML files.
pub struct MarkdownPreviewButton {
    workspace: WeakEntity<Workspace>,
    active_item: Option<Box<dyn ItemHandle>>,
    /// `Some(is_html)` while the active item can be previewed.
    previewable: Option<bool>,
    _editor_subscription: Option<Subscription>,
}

impl MarkdownPreviewButton {
    pub fn new(workspace: WeakEntity<Workspace>) -> MarkdownPreviewButton {
        MarkdownPreviewButton { workspace, active_item: None, previewable: None, _editor_subscription: None }
    }

    /// The preview acts as its source editor, so it is excluded explicitly.
    fn previewable_editor(item: &dyn ItemHandle, cx: &App) -> Option<Entity<Editor>> {
        if item.act_as::<MarkdownPreviewView>(cx).is_some() {
            return None;
        }
        item.act_as::<Editor>(cx).filter(|editor| MarkdownPreviewView::is_previewable_file(editor, cx))
    }

    /// Redraws the tab bar when the button appears, disappears or changes
    /// tooltip, e.g. once a newly opened buffer gets its language.
    fn refresh(&mut self, cx: &mut Context<MarkdownPreviewButton>) {
        let previewable = self
            .active_item
            .as_ref()
            .and_then(|item| MarkdownPreviewButton::previewable_editor(item.as_ref(), cx))
            .map(|editor| MarkdownPreviewView::is_html_file(&editor, cx));
        if previewable != self.previewable {
            self.previewable = previewable;
            cx.emit(ToolbarItemEvent::ChangeLocation(ToolbarItemLocation::Hidden));
        }
    }
}

const fn tooltip(is_html: bool) -> &'static str {
    if is_html { "Preview HTML" } else { "Preview Markdown" }
}

impl Render for MarkdownPreviewButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<MarkdownPreviewButton>) -> impl IntoElement {
        gpui::Empty
    }
}

impl EventEmitter<ToolbarItemEvent> for MarkdownPreviewButton {}

impl ToolbarItemView for MarkdownPreviewButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<MarkdownPreviewButton>,
    ) -> ToolbarItemLocation {
        self.active_item = active_pane_item.map(|item| item.boxed_clone());
        self._editor_subscription = active_pane_item
            .and_then(|item| item.act_as::<Editor>(cx))
            .map(|editor| cx.observe(&editor, |this, _, cx| this.refresh(cx)));
        self.refresh(cx);
        ToolbarItemLocation::Hidden
    }

    fn tab_bar_button(&self, cx: &App) -> Option<AnyElement> {
        let item = self.active_item.as_ref()?;
        let editor = MarkdownPreviewButton::previewable_editor(item.as_ref(), cx)?;
        let is_html = MarkdownPreviewView::is_html_file(&editor, cx);
        Some(workspace::pane::render_tab_bar_preview_button(
            tooltip(is_html),
            &crate::OpenPreview,
            self.workspace.clone(),
            item.as_ref(),
            move |workspace, pane, to_the_side, window, cx| {
                if to_the_side {
                    MarkdownPreviewView::open_preview_to_the_side_of_pane(workspace, editor.clone(), pane, window, cx);
                } else {
                    MarkdownPreviewView::open_preview_in_pane(workspace, editor.clone(), pane, window, cx);
                }
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::tooltip;

    #[test]
    fn test_tooltip() {
        assert_eq!(tooltip(false), "Preview Markdown");
        assert_eq!(tooltip(true), "Preview HTML");
    }
}
