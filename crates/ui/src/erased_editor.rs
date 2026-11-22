//! Type-erased editor handle so crates below `editor` (e.g. `picker`) can host a text input.

use std::any::Any;
use std::sync::Arc;
use std::sync::OnceLock;

use gpui::AnyElement;
use gpui::App;
use gpui::FocusHandle;
use gpui::Subscription;
use gpui::Window;

pub trait ErasedEditor: 'static {
    fn text(&self, cx: &App) -> String;
    fn set_text(&self, text: &str, window: &mut Window, cx: &mut App);
    fn clear(&self, window: &mut Window, cx: &mut App);
    fn set_placeholder_text(&self, text: &str, window: &mut Window, _: &mut App);
    fn move_selection_to_end(&self, window: &mut Window, _: &mut App);
    fn select_all(&self, window: &mut Window, cx: &mut App);
    fn set_masked(&self, masked: bool, window: &mut Window, cx: &mut App);
    fn set_read_only(&self, read_only: bool, cx: &mut App);

    fn focus_handle(&self, cx: &App) -> FocusHandle;

    #[allow(clippy::type_complexity)]
    fn subscribe(
        &self,
        callback: Box<dyn FnMut(ErasedEditorEvent, &mut Window, &mut App) + 'static>,
        window: &mut Window,
        cx: &mut App,
    ) -> Subscription;
    fn render(&self, window: &mut Window, cx: &App) -> AnyElement;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErasedEditorEvent {
    BufferEdited,
    Blurred,
}
#[allow(clippy::type_complexity)]
pub static ERASED_EDITOR_FACTORY: OnceLock<fn(&mut Window, &mut App) -> Arc<dyn ErasedEditor>> = OnceLock::new();
