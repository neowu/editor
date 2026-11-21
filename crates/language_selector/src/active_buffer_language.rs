use editor::Editor;
use gpui::Context;
use gpui::Entity;
use gpui::IntoElement;
use gpui::ParentElement as _;
use gpui::Render;
use gpui::SharedString;
use gpui::Subscription;
use gpui::WeakEntity;
use gpui::Window;
use gpui::div;
use language::LanguageName;
use language::PLAIN_TEXT;
use ui::Button;
use ui::ButtonCommon as _;
use ui::Clickable as _;
use ui::FluentBuilder as _;
use ui::LabelSize;
use ui::Tooltip;
use workspace::StatusItemView;
use workspace::Workspace;
use workspace::item::ItemHandle;
use workspace::status_item_color;

use crate::LanguageSelector;
use crate::Toggle;

pub struct ActiveBufferLanguage {
    // `None` hides the button; `Some` holds the label to show.
    active_language: Option<SharedString>,
    workspace: WeakEntity<Workspace>,
    _observe_active_editor: Option<Subscription>,
}

impl ActiveBufferLanguage {
    pub fn new(workspace: &Workspace) -> Self {
        Self { active_language: None, workspace: workspace.weak_handle(), _observe_active_editor: None }
    }

    fn update_language(&mut self, editor: Entity<Editor>, _: &mut Window, cx: &mut Context<Self>) {
        let mut active_language: Option<SharedString> = Some("Unknown".into());

        let editor = editor.read(cx);
        if let Some(buffer) = editor.active_buffer(cx) {
            let buffer = buffer.read(cx);
            let language =
                buffer.language().filter(|language| **language != *PLAIN_TEXT).map(|language| language.name());
            // Files with no registered language (e.g. TOML) still show
            // the name of the syntax they are highlighted with.
            if let Some(name) = language.or_else(|| {
                buffer.fallback_syntax_name().map(LanguageName::new).or_else(|| Some(buffer.language()?.name()))
            }) {
                active_language = Some(name.0);
            }
        }

        // Runs on every editor notify, so only redraw the status bar on a change.
        if active_language != self.active_language {
            self.active_language = active_language;
            cx.notify();
        }
    }
}

impl Render for ActiveBufferLanguage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self
            .workspace
            .upgrade()
            .is_some_and(|workspace| workspace.read(cx).active_modal::<LanguageSelector>(cx).is_some());
        div().when_some(self.active_language.clone(), |el, active_language_text| {
            el.child(
                Button::new("change-language", active_language_text)
                    .label_size(LabelSize::Small)
                    .color(status_item_color(open))
                    .tab_index(0_isize)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(workspace) = this.workspace.upgrade() {
                            workspace.update(cx, |workspace, cx| LanguageSelector::toggle(workspace, window, cx));
                        }
                    }))
                    .tooltip(|_window, cx| Tooltip::for_action("Select Language", &Toggle, cx)),
            )
        })
    }
}

impl StatusItemView for ActiveBufferLanguage {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = active_pane_item.and_then(|item| item.act_as::<Editor>(cx)) {
            self._observe_active_editor = Some(cx.observe_in(&editor, window, Self::update_language));
            self.update_language(editor, window, cx);
        } else {
            self.active_language = None;
            self._observe_active_editor = None;
        }

        cx.notify();
    }
}
