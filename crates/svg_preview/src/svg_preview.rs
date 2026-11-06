use gpui::App;
use gpui::actions;
use workspace::Workspace;
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

pub fn init(cx: &mut App) {
    preview::register_preview_provider(preview_provider::PROVIDER, cx);
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        SvgPreviewView::register(workspace, window, cx);
    })
    .detach();
}
