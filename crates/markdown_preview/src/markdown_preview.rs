use gpui::App;
use gpui::actions;
use workspace::preview;

mod markdown_preview_style;
pub mod markdown_preview_view;
mod mermaid_diagrams;
mod preview_provider;

use crate::markdown_preview_view::MarkdownPreviewView;

actions!(
    markdown,
    [
        /// Scrolls up by one page in the markdown preview.
        ScrollPageUp,
        /// Scrolls down by one page in the markdown preview.
        ScrollPageDown,
        /// Scrolls up by approximately one visual line.
        ScrollUp,
        /// Scrolls down by approximately one visual line.
        ScrollDown,
        /// Scrolls up by one markdown element in the markdown preview
        ScrollUpByItem,
        /// Scrolls down by one markdown element in the markdown preview
        ScrollDownByItem,
        /// Scrolls to the top of the markdown preview.
        ScrollToTop,
        /// Scrolls to the bottom of the markdown preview.
        ScrollToBottom,
        /// Closes the markdown preview and returns focus to the source editor.
        CloseAndReturnToEditor
    ]
);

pub fn init(cx: &mut App) {
    workspace::register_serializable_item::<MarkdownPreviewView>(cx);
    preview::register_preview_provider(preview_provider::PROVIDER, cx);
}
