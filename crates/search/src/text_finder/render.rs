use gpui::KeyContext;
use ui::Context;
use ui::InteractiveElement as _;
use ui::IntoElement;
use ui::ParentElement as _;
use ui::Render;
use ui::Window;
use ui::v_flex;

use super::TextFinder;

static TEXT_FINDER_ACTIONS: std::sync::LazyLock<gpui::ActionTable<TextFinder>> = std::sync::LazyLock::new(|| {
    let mut t = gpui::ActionTable::<TextFinder>::default();
    t.add(TextFinder::switch_to_project_search);
    t.add(TextFinder::split_left);
    t.add(TextFinder::split_right);
    t.add(TextFinder::split_up);
    t.add(TextFinder::split_down);
    t.add(TextFinder::fold);
    t.add(TextFinder::unfold);
    t.add(TextFinder::toggle_fold_all);
    t
});

impl Render for TextFinder {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("TextFinder");

        v_flex().key_context(key_context).action_table(&cx.entity(), &TEXT_FINDER_ACTIONS).child(self.picker.clone())
    }
}
