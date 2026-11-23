use gpui::App;
use workspace::preview;

mod preview_provider;
pub mod svg_preview_view;

pub fn init(cx: &mut App) {
    preview::register_preview_provider(preview_provider::PROVIDER, cx);
}
