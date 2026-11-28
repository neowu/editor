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
        pub ScrollPageUp,
        /// Scrolls down by one page in the markdown preview.
        pub ScrollPageDown,
        /// Scrolls up by approximately one visual line.
        pub ScrollUp,
        /// Scrolls down by approximately one visual line.
        pub ScrollDown,
        /// Scrolls up by one markdown element in the markdown preview
        pub ScrollUpByItem,
        /// Scrolls down by one markdown element in the markdown preview
        pub ScrollDownByItem,
        /// Scrolls to the top of the markdown preview.
        pub ScrollToTop,
        /// Scrolls to the bottom of the markdown preview.
        pub ScrollToBottom,
        /// Closes the markdown preview and returns focus to the source editor.
        pub CloseAndReturnToEditor
    ]
);

pub fn init(cx: &mut App) {
    workspace::register_serializable_item::<MarkdownPreviewView>(cx);
    preview::register_preview_provider(preview_provider::PROVIDER, cx);
}
