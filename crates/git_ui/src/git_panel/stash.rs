use actions::menu;
use editor::Editor;
use git::StashAll;
use git::StashApply;
use git::StashPop;
use git::StashStaged;
use git::StashTracked;
use gpui::AppContext as _;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::WeakEntity;
use ui::prelude::App;
use ui::prelude::Context;
use ui::prelude::Headline;
use ui::prelude::HeadlineSize;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::InteractiveElement as _;
use ui::prelude::IntoElement;
use ui::prelude::ParentElement as _;
use ui::prelude::Render;
use ui::prelude::Styled as _;
use ui::prelude::StyledExt as _;
use ui::prelude::Window;
use ui::prelude::div;
use ui::prelude::h_flex;
use ui::prelude::rems;
use ui::prelude::v_flex;
use workspace::ModalView;

use super::GitPanel;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StashKind {
    All,
    Tracked,
    Staged,
}

impl StashKind {
    const fn title(self) -> &'static str {
        match self {
            StashKind::All => "Stash All",
            StashKind::Tracked => "Stash Tracked",
            StashKind::Staged => "Stash Staged",
        }
    }

    const fn error_action(self) -> &'static str {
        match self {
            StashKind::All => "stash",
            StashKind::Tracked => "stash tracked",
            StashKind::Staged => "stash staged",
        }
    }
}

/// Prompts for an optional stash name. Confirming with an empty editor stashes
/// without `--message`, letting git generate its usual "WIP on ..." description.
struct StashMessageModal {
    editor: Entity<Editor>,
    panel: WeakEntity<GitPanel>,
    kind: StashKind,
}

impl StashMessageModal {
    fn new(panel: WeakEntity<GitPanel>, kind: StashKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Optionally provide a stash message", window, cx);
            editor
        });
        Self { editor, panel, kind }
    }

    fn cancel(&mut self, _: &menu::Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn confirm(&mut self, _: &menu::Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        let message = self.editor.read(cx).text(cx).trim().to_owned();
        let message = (!message.is_empty()).then_some(message);
        let kind = self.kind;
        self.panel.update(cx, |panel, cx| panel.perform_stash(kind, message, cx)).ok();
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for StashMessageModal {}
impl ModalView for StashMessageModal {}
impl Focusable for StashMessageModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

static STASH_MESSAGE_MODAL_ACTIONS: std::sync::LazyLock<gpui::ActionTable<StashMessageModal>> =
    std::sync::LazyLock::new(|| {
        let mut t = gpui::ActionTable::<StashMessageModal>::default();
        t.add(StashMessageModal::cancel);
        t.add(StashMessageModal::confirm);
        t
    });

impl Render for StashMessageModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("StashMessageModal")
            .action_table(&cx.entity(), &STASH_MESSAGE_MODAL_ACTIONS)
            .elevation_2()
            .w(rems(34.))
            .child(
                h_flex()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .w_full()
                    .gap_1p5()
                    .child(Icon::new(IconName::GitBranch).size(IconSize::XSmall))
                    .child(Headline::new(self.kind.title()).size(HeadlineSize::XSmall)),
            )
            .child(div().px_3().pb_3().w_full().child(self.editor.clone()))
    }
}

impl GitPanel {
    pub fn stash_pop(&mut self, _: &StashPop, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };
        if self.rebase_in_progress(cx) {
            return;
        }

        cx.spawn({
            async move |this, cx| {
                let stash_task = active_repository.update(cx, |repo, cx| repo.stash_pop(None, cx)).await;
                this.update(cx, |this, cx| {
                    stash_task
                        .map_err(|e| {
                            this.show_error_toast("stash pop", e, cx);
                        })
                        .ok();
                    cx.notify();
                })
            }
        })
        .detach();
    }

    pub fn stash_apply(&mut self, _: &StashApply, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };
        if self.rebase_in_progress(cx) {
            return;
        }

        cx.spawn({
            async move |this, cx| {
                let stash_task = active_repository.update(cx, |repo, cx| repo.stash_apply(None, cx)).await;
                this.update(cx, |this, cx| {
                    stash_task
                        .map_err(|e| {
                            this.show_error_toast("stash apply", e, cx);
                        })
                        .ok();
                    cx.notify();
                })
            }
        })
        .detach();
    }

    pub fn stash_all(&mut self, _: &StashAll, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_for_stash_message(StashKind::All, window, cx);
    }

    pub fn stash_tracked(&mut self, _: &StashTracked, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_for_stash_message(StashKind::Tracked, window, cx);
    }

    pub fn stash_staged(&mut self, _: &StashStaged, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_for_stash_message(StashKind::Staged, window, cx);
    }

    fn prompt_for_stash_message(&self, kind: StashKind, window: &Window, cx: &mut Context<Self>) {
        if self.active_repository.is_none() || self.rebase_in_progress(cx) {
            return;
        }
        // `git::StashAll` is also registered on the workspace, which dispatches it while
        // `Workspace` is leased, so opening the modal inline would re-enter that update.
        cx.defer_in(window, move |this, window, cx| {
            let panel = cx.entity().downgrade();
            this.workspace
                .update(cx, |workspace, cx| {
                    workspace.toggle_modal(window, cx, |window, cx| StashMessageModal::new(panel, kind, window, cx));
                })
                .ok();
        });
    }

    fn perform_stash(&self, kind: StashKind, message: Option<String>, cx: &Context<Self>) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };

        cx.spawn({
            async move |this, cx| {
                let stash_task = active_repository
                    .update(cx, |repo, cx| match kind {
                        StashKind::All => repo.stash_all(message, cx),
                        StashKind::Tracked => repo.stash_tracked(message, cx),
                        StashKind::Staged => repo.stash_staged(message, cx),
                    })
                    .await;
                this.update(cx, |this, cx| {
                    stash_task
                        .map_err(|e| {
                            this.show_error_toast(kind.error_action(), e, cx);
                        })
                        .ok();
                    cx.notify();
                })
            }
        })
        .detach();
    }
}
