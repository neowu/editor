use gpui::AnyElement;
use gpui::EventEmitter;
use gpui::WeakEntity;
use multi_buffer::MultiBuffer;
use ui::prelude::*;
use workspace::ItemHandle;
use workspace::ToolbarItemEvent;
use workspace::ToolbarItemLocation;
use workspace::ToolbarItemView;
use workspace::Workspace;

use crate::svg_preview_view::SvgPreviewView;

/// Adds the tab bar's preview button for SVG files. Detection is by file name,
/// so it is resolved on each tab bar render.
pub struct SvgPreviewButton {
    workspace: WeakEntity<Workspace>,
    active_item: Option<Box<dyn ItemHandle>>,
}

impl SvgPreviewButton {
    pub fn new(workspace: WeakEntity<Workspace>) -> SvgPreviewButton {
        SvgPreviewButton { workspace, active_item: None }
    }
}

impl Render for SvgPreviewButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<SvgPreviewButton>) -> impl IntoElement {
        gpui::Empty
    }
}

impl EventEmitter<ToolbarItemEvent> for SvgPreviewButton {}

impl ToolbarItemView for SvgPreviewButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<SvgPreviewButton>,
    ) -> ToolbarItemLocation {
        self.active_item = active_pane_item.map(|item| item.boxed_clone());
        ToolbarItemLocation::Hidden
    }

    fn tab_bar_button(&self, cx: &App) -> Option<AnyElement> {
        let item = self.active_item.as_ref()?;
        let buffer = item.act_as::<MultiBuffer>(cx).filter(|buffer| SvgPreviewView::is_svg_file(buffer, cx))?;
        Some(workspace::pane::render_tab_bar_preview_button(
            "Preview SVG",
            &crate::OpenPreview,
            self.workspace.clone(),
            item.as_ref(),
            move |workspace, pane, to_the_side, window, cx| {
                if to_the_side {
                    SvgPreviewView::open_preview_to_the_side_of_pane(workspace, buffer.clone(), pane, window, cx);
                } else {
                    SvgPreviewView::open_preview_in_pane(workspace, buffer.clone(), pane, window, cx);
                }
            },
        ))
    }
}
