use gpui::App;
use gpui::actions;
use workspace::WorkspaceActions;
use workspace::preview;

use crate::svg_preview_view::SvgPreviewView;

mod preview_provider;
pub mod svg_preview_view;

actions!(
    svg,
    [
        /// Opens a following SVG preview that syncs with the editor.
        OpenFollowingPreview
    ]
);

pub fn init(workspace_actions: &mut WorkspaceActions, cx: &mut App) {
    preview::register_preview_provider(preview_provider::PROVIDER, cx);
    SvgPreviewView::register(workspace_actions);
}
