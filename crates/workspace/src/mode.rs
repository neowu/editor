//! What fills the window above the status bar: one view per
//! [`WorkspaceMode`], with each mode's rendering in its own module.

mod editor;
mod git_history;
mod terminal;

pub(crate) use editor::DockMotion;
pub(crate) use git_history::GIT_HISTORY_SIDEBAR_WIDTH;
use gpui::AnyElement;
use gpui::App;
use gpui::Context;
use gpui::Entity;
use gpui::IntoElement;
use gpui::Render;
use gpui::Subscription;
use gpui::WeakEntity;
use gpui::Window;
use gpui::actions;
use gpui::div;
pub(crate) use terminal::TerminalLayout;
pub use terminal::TerminalModeDelegate;
pub(crate) use terminal::is_dragged_terminal;
pub use terminal::set_terminal_mode_delegate;
use ui::prelude::Div;
use ui::prelude::FluentBuilder as _;
use ui::prelude::ParentElement as _;
use ui::prelude::Styled as _;

use crate::CARD_GAP;
use crate::Pane;
use crate::Workspace;
use crate::WorkspaceMode;
use crate::dock::Dock;

actions!(
    mode,
    [
        /// Switches the window to Editor mode.
        pub Editor,
        /// Switches the window to Git History mode for the active repository:
        /// keeps the active history tab if it belongs to that repository,
        /// otherwise shows the repository's graph.
        pub GitHistory,
        /// Switches the window to Terminal mode, starting a shell (or the
        /// saved layout) when it has no terminals yet.
        pub Terminal
    ]
);

/// Renders one mode's content, everything above the status bar. The workspace draws only the shown mode's view, cached, so a
/// mode is laid out only while shown and redraws of the shared chrome reuse it.
pub(crate) struct ModeView {
    workspace: WeakEntity<Workspace>,
    mode: WorkspaceMode,
    _subscriptions: Vec<Subscription>,
}

impl ModeView {
    /// A cached view re-renders only when it or a view inside it notifies, so
    /// it also follows the workspace state it reads, and `followed` entities
    /// that aren't always drawn inside it.
    pub(crate) fn new(
        workspace: &Entity<Workspace>,
        mode: WorkspaceMode,
        followed: &[Entity<Dock>],
        cx: &mut Context<ModeView>,
    ) -> ModeView {
        let mut subscriptions = vec![cx.observe(workspace, |mode_view: &mut ModeView, _, cx| {
            mode_view.notify_if_shown(cx);
        })];
        subscriptions.extend(
            followed
                .iter()
                .map(|entity| cx.observe(entity, |mode_view: &mut ModeView, _, cx| mode_view.notify_if_shown(cx))),
        );
        ModeView { workspace: workspace.downgrade(), mode, _subscriptions: subscriptions }
    }

    fn notify_if_shown(&self, cx: &mut Context<ModeView>) {
        if self.workspace.upgrade().is_some_and(|workspace| workspace.read(cx).mode() == self.mode) {
            cx.notify();
        }
    }
}

impl Render for ModeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<ModeView>) -> impl IntoElement {
        let mode = self.mode;
        self.workspace
            .update(cx, |workspace: &mut Workspace, cx| workspace.render_mode(mode, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

impl Workspace {
    /// What fills the window above the status bar in `mode`, rendered by that
    /// mode's [`ModeView`].
    fn render_mode(&self, mode: WorkspaceMode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // lint: expect(module_cycle)
        self.assign_tab_bar_chrome(mode, window, cx);
        // The header over the sidebar is drawn last, over the panes beside it.
        // lint: expect(module_cycle)
        let header_overlay = (!self.window_header_in_tab_bar(cx)).then(|| self.render_sidebar_header_overlay(cx));
        match mode {
            WorkspaceMode::Editor => self.render_editor_mode(window, cx),
            WorkspaceMode::GitHistory => self.mode_container(cx).child(self.render_git_history(window, cx)),
            WorkspaceMode::Terminal => self.render_terminal_mode(window, cx),
        }
        .children(header_overlay)
        .into_any_element()
    }

    fn mode_container(&self, cx: &App) -> Div {
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .px(CARD_GAP)
            // The status bar pads itself to center between the card and window edge.
            .when(!self.status_bar_visible(cx), |this| this.pb(CARD_GAP))
    }

    /// The pane zoomed in `mode`. Editor mode can zoom a dock panel instead,
    /// which [`Workspace::has_zoomed`] also counts.
    pub(crate) fn zoomed_pane(&self, mode: WorkspaceMode, cx: &App) -> Option<Entity<Pane>> {
        match mode {
            WorkspaceMode::Editor => {
                let view = self.zoomed.as_ref()?;
                view.upgrade()?.downcast::<Pane>().ok()
            }
            WorkspaceMode::GitHistory => {
                self.git_history_center.panes().into_iter().find(|pane| pane.read(cx).is_zoomed()).cloned()
            }
            WorkspaceMode::Terminal => {
                self.terminal_center.panes().into_iter().find(|pane| pane.read(cx).is_zoomed()).cloned()
            }
        }
    }

    /// Whether `mode` has a pane or dock panel zoomed.
    pub(crate) fn has_zoomed(&self, mode: WorkspaceMode, cx: &App) -> bool {
        match mode {
            WorkspaceMode::Editor => self.zoomed.is_some(),
            WorkspaceMode::GitHistory | WorkspaceMode::Terminal => self.zoomed_pane(mode, cx).is_some(),
        }
    }

    pub(crate) const fn mode_view(&self, mode: WorkspaceMode) -> &Entity<ModeView> {
        match mode {
            WorkspaceMode::Editor => &self.editor_view,
            WorkspaceMode::GitHistory => &self.git_history_view,
            WorkspaceMode::Terminal => &self.terminal_mode_view,
        }
    }

    /// Redraws after a change to `mode`'s content. A hidden mode isn't drawn,
    /// and re-renders when it is shown again, so its changes redraw nothing.
    pub(crate) fn notify_mode(&self, mode: WorkspaceMode, cx: &mut Context<Self>) {
        if self.mode == mode {
            cx.notify();
        }
    }
}
