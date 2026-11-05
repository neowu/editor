//! Terminal mode: terminal panes filling the window, split like editor panes.
//! The terminal crate depends on this one, so it plugs in through
//! [`TerminalModeDelegate`]; the workspace owns the panes, as it does Git
//! History's, so the window header, zoom and focus work the same in every mode.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui::Action;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Div;
use gpui::Entity;
use gpui::Focusable;
use gpui::Global;
use gpui::Task;
use gpui::WeakEntity;
use project::Project;
use ui::Window;
use ui::prelude::*;
use util::ResultExt;

use crate::ActivateNextPane;
use crate::ActivatePaneDown;
use crate::ActivatePaneLeft;
use crate::ActivatePaneRight;
use crate::ActivatePaneUp;
use crate::ActivatePreviousPane;
use crate::Member;
use crate::NewTerminal;
use crate::Pane;
use crate::PaneAxis;
use crate::PaneGroup;
use crate::PaneRenderContext;
use crate::PaneSearchBarCallbacks;
use crate::SplitDirection;
use crate::SplitMode;
use crate::Workspace;
use crate::WorkspaceMode;
use crate::item::ItemHandle;
use crate::pane;
use crate::pane::EmptyPaneHints;
use crate::persistence::model::SerializedTerminalGroup;
use crate::persistence::model::SerializedTerminalPane;
use crate::persistence::model::SerializedTerminalTab;

/// What terminal mode needs from the terminal crate.
pub trait TerminalModeDelegate: 'static {
    /// Sets up a new terminal mode pane: tab style, tab bar buttons, search
    /// bar and which dragged tabs it accepts.
    fn setup_pane(&self, pane: &mut Pane, window: &mut Window, cx: &mut Context<Pane>);

    /// Opens a shell as a new tab of `pane`, in `working_directory` or the
    /// project's default directory.
    fn open_shell(
        &self,
        workspace: &mut Workspace,
        pane: Entity<Pane>,
        working_directory: Option<PathBuf>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>>;

    /// The current working directory of `item`, if it is a terminal.
    fn working_directory(&self, item: &dyn ItemHandle, cx: &App) -> Option<PathBuf>;

    /// Whether `item` is a terminal. Only terminal panes take terminal tabs.
    fn is_terminal(&self, item: &dyn ItemHandle, cx: &App) -> bool;
}

struct GlobalTerminalModeDelegate(Rc<dyn TerminalModeDelegate>);

impl Global for GlobalTerminalModeDelegate {}

/// Enables terminal mode. Set once at startup, before any window opens.
pub fn set_terminal_mode_delegate(delegate: impl TerminalModeDelegate, cx: &mut App) {
    cx.set_global(GlobalTerminalModeDelegate(Rc::new(delegate)));
}

fn terminal_mode_delegate(cx: &App) -> Option<Rc<dyn TerminalModeDelegate>> {
    cx.try_global::<GlobalTerminalModeDelegate>().map(|global| global.0.clone())
}

/// Whether a dragged tab holds a terminal, which editor panes refuse.
pub(crate) fn is_dragged_terminal(dragged: &dyn std::any::Any, cx: &App) -> bool {
    let Some(tab) = dragged.downcast_ref::<crate::DraggedTab>() else {
        return false;
    };
    terminal_mode_delegate(cx).is_some_and(|delegate| delegate.is_terminal(tab.item.as_ref(), cx))
}

/// Terminal mode's layout before and after it is first shown in the window.
pub(crate) enum TerminalLayout {
    /// Not shown yet: the layout saved last time, restored when first shown,
    /// so projects that never use terminal mode start no shells.
    Pending(Option<SerializedTerminalGroup>),
    Started,
}

/// What showing terminal mode does to its terminals.
#[derive(Debug, PartialEq, Eq)]
enum TerminalModeEntry {
    Restore,
    StartShell,
    Nothing,
}

/// A saved layout is restored the first time terminal mode is shown. A
/// shell starts when the mode is shown without terminals (unless the caller
/// opens its own), but not on a repeated switch while already shown, so an
/// emptied mode keeps its "New Terminal" page.
const fn terminal_mode_entry(
    pending: bool,
    has_saved_layout: bool,
    entering: bool,
    is_empty: bool,
    start_shell: bool,
) -> TerminalModeEntry {
    if pending && has_saved_layout {
        TerminalModeEntry::Restore
    } else if start_shell && is_empty && (pending || entering) {
        TerminalModeEntry::StartShell
    } else {
        TerminalModeEntry::Nothing
    }
}

/// The next (or previous) of `len` panes after `index`, wrapping around.
const fn cycled_index(index: usize, len: usize, forward: bool) -> usize {
    if forward { (index + 1) % len } else { (index + len - 1) % len }
}

/// Index of the active tab among the terminal tabs kept when saving.
fn saved_active_tab(is_terminal: &[bool], active_item: usize) -> usize {
    is_terminal.iter().take(active_item).filter(|terminal| **terminal).count()
}

impl Workspace {
    pub(super) fn render_terminal_mode(&self, window: &mut Window, cx: &mut Context<Workspace>) -> Div {
        let pane = self.active_terminal_pane.clone();
        let panes = match self.zoomed_pane(WorkspaceMode::Terminal, cx) {
            Some(zoomed_pane) => zoomed_pane.into_any_element(),
            None => {
                let weak_handle = self.weak_handle();
                let render_cx = PaneRenderContext { active_pane: &pane, workspace: &weak_handle };
                self.terminal_center.render(None, None, &render_cx, window, cx).into_any_element()
            }
        };
        let content = div()
            .id("terminal-panes")
            .flex()
            .flex_1()
            .min_h_0()
            .size_full()
            .child(panes)
            .on_action(cx.listener(|workspace: &mut Workspace, _: &ActivatePaneLeft, window, cx| {
                workspace.activate_terminal_pane_in_direction(SplitDirection::Left, window, cx);
            }))
            .on_action(cx.listener(|workspace: &mut Workspace, _: &ActivatePaneRight, window, cx| {
                workspace.activate_terminal_pane_in_direction(SplitDirection::Right, window, cx);
            }))
            .on_action(cx.listener(|workspace: &mut Workspace, _: &ActivatePaneUp, window, cx| {
                workspace.activate_terminal_pane_in_direction(SplitDirection::Up, window, cx);
            }))
            .on_action(cx.listener(|workspace: &mut Workspace, _: &ActivatePaneDown, window, cx| {
                workspace.activate_terminal_pane_in_direction(SplitDirection::Down, window, cx);
            }))
            .on_action(cx.listener(|workspace: &mut Workspace, _: &ActivateNextPane, window, cx| {
                workspace.activate_terminal_pane_by_offset(true, window, cx);
            }))
            .on_action(cx.listener(|workspace: &mut Workspace, _: &ActivatePreviousPane, window, cx| {
                workspace.activate_terminal_pane_by_offset(false, window, cx);
            }));
        let content = match cx.try_global::<PaneSearchBarCallbacks>() {
            Some(callbacks) => (callbacks.wrap_div_with_search_actions)(div().size_full().flex().child(content), pane),
            None => div().size_full().flex().child(content),
        };
        // A header row of its own, where the other modes' tab bars sit, so the
        // window header and the card tops stay put when switching modes.
        self.mode_container(cx).child(self.render_sidebar_header(cx)).child(content)
    }

    /// Whether this window can show terminal mode: the terminal crate is
    /// loaded and the project can run terminals.
    pub fn supports_terminal_mode(&self, cx: &App) -> bool {
        terminal_mode_delegate(cx).is_some() && self.project.read(cx).supports_terminal(cx)
    }

    /// Switches to terminal mode, restoring the saved layout the first time.
    /// With `start_shell`, a shell starts when the mode has no terminals.
    pub fn show_terminal_mode(&mut self, start_shell: bool, window: &mut Window, cx: &mut Context<Workspace>) {
        if !self.supports_terminal_mode(cx) {
            return;
        }
        let entering = self.mode != WorkspaceMode::Terminal;
        self.set_mode(WorkspaceMode::Terminal, window, cx);
        let (pending, saved_layout) = match std::mem::replace(&mut self.terminal_layout, TerminalLayout::Started) {
            TerminalLayout::Pending(saved_layout) => (true, saved_layout),
            TerminalLayout::Started => (false, None),
        };
        match terminal_mode_entry(pending, saved_layout.is_some(), entering, self.terminal_is_empty(cx), start_shell) {
            TerminalModeEntry::Restore => {
                if let Some(saved_layout) = saved_layout {
                    self.restore_terminal_layout(saved_layout, window, cx);
                }
            }
            TerminalModeEntry::StartShell => self.open_terminal_mode_shell(None, window, cx),
            TerminalModeEntry::Nothing => {}
        }
    }

    /// Opens a shell in the active terminal mode pane, switching to terminal
    /// mode, in `working_directory` or the project's default directory.
    pub fn open_terminal_mode_shell(
        &mut self,
        working_directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let Some(delegate) = terminal_mode_delegate(cx) else {
            return;
        };
        if self.mode != WorkspaceMode::Terminal {
            self.show_terminal_mode(false, window, cx);
        }
        let pane = self.active_terminal_pane.clone();
        delegate.open_shell(self, pane, working_directory, true, window, cx).detach_and_log_err(cx);
    }

    /// The active terminal mode pane.
    pub const fn terminal_pane(&self) -> &Entity<Pane> {
        &self.active_terminal_pane
    }

    pub fn terminal_panes(&self) -> Vec<&Entity<Pane>> {
        self.terminal_center.panes()
    }

    fn terminal_is_empty(&self, cx: &App) -> bool {
        self.terminal_panes().iter().all(|pane| pane.read(cx).items_len() == 0)
    }

    pub(crate) fn new_terminal_pane(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Pane> {
        let delegate = terminal_mode_delegate(cx);
        let pane = cx.new(|cx| {
            let mut pane =
                Pane::new(workspace, project, Arc::default(), None, NewTerminal::default().boxed_clone(), window, cx);
            pane.set_can_navigate(false, cx);
            pane.set_zoom_out_on_close(false);
            pane.set_empty_pane_hints(EmptyPaneHints::Terminal);
            if let Some(delegate) = &delegate {
                delegate.setup_pane(&mut pane, window, cx);
            }
            pane
        });
        cx.subscribe_in(&pane, window, Workspace::handle_terminal_pane_event).detach();
        pane
    }

    pub(crate) fn add_terminal_pane(&self, window: &mut Window, cx: &mut Context<Workspace>) -> Entity<Pane> {
        Workspace::new_terminal_pane(self.weak_handle(), self.project.clone(), window, cx)
    }

    /// Splits `pane`. Moving a tab needs another left behind; otherwise the
    /// new pane gets a new shell in the directory of `pane`'s active terminal.
    pub(crate) fn split_terminal_pane(
        &mut self,
        pane: Entity<Pane>,
        direction: SplitDirection,
        mode: SplitMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        self.zoom_terminal_pane(None, cx);
        if mode == SplitMode::MovePane && pane.read(cx).items_len() > 1 {
            let Some(item) = pane.update(cx, |pane, cx| pane.take_active_item(window, cx)) else {
                return;
            };
            let new_pane = self.add_terminal_pane(window, cx);
            new_pane.update(cx, |pane, cx| pane.add_item(item, true, true, None, window, cx));
            self.terminal_center.split(&pane, &new_pane, direction, cx);
            self.set_active_terminal_pane(&new_pane, cx);
        } else {
            let Some(delegate) = terminal_mode_delegate(cx) else {
                return;
            };
            let working_directory =
                pane.read(cx).active_item().and_then(|item| delegate.working_directory(item.as_ref(), cx));
            let new_pane = self.add_terminal_pane(window, cx);
            self.terminal_center.split(&pane, &new_pane, direction, cx);
            self.set_active_terminal_pane(&new_pane, cx);
            window.focus(&new_pane.focus_handle(cx), cx);
            delegate.open_shell(self, new_pane, working_directory, true, window, cx).detach_and_log_err(cx);
        }
        self.notify_mode(WorkspaceMode::Terminal, cx);
        self.serialize_workspace(window, cx);
    }

    /// Adds an empty pane beside `pane`, for a tab dragged to its edge.
    pub(crate) fn split_terminal_pane_for_drop(
        &mut self,
        pane: &Entity<Pane>,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Pane> {
        self.zoom_terminal_pane(None, cx);
        let new_pane = self.add_terminal_pane(window, cx);
        self.terminal_center.split(pane, &new_pane, direction, cx);
        self.notify_mode(WorkspaceMode::Terminal, cx);
        new_pane
    }

    /// Drops an emptied pane, unless it is the last one: that one stays and
    /// offers "New Terminal".
    fn remove_terminal_pane(
        &mut self,
        pane: &Entity<Pane>,
        focus_on: Option<&Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if !self.terminal_center.remove(pane, cx).log_err().unwrap_or(false) {
            return;
        }
        if self.active_terminal_pane == *pane {
            let next_pane = focus_on
                .filter(|focus_on| self.terminal_center.root.contains_pane(focus_on))
                .cloned()
                .unwrap_or_else(|| self.terminal_center.first_pane());
            self.set_active_terminal_pane(&next_pane, cx);
            if self.mode == WorkspaceMode::Terminal {
                window.focus(&next_pane.focus_handle(cx), cx);
            }
        }
        self.notify_mode(WorkspaceMode::Terminal, cx);
        self.serialize_workspace(window, cx);
    }

    pub(crate) fn set_active_terminal_pane(&mut self, pane: &Entity<Pane>, cx: &mut Context<Workspace>) {
        if self.active_terminal_pane != *pane {
            self.active_terminal_pane = pane.clone();
            self.notify_mode(WorkspaceMode::Terminal, cx);
        }
    }

    /// Zooms `pane` (unzooming any other terminal pane), or unzooms all for `None`.
    fn zoom_terminal_pane(&self, pane: Option<&Entity<Pane>>, cx: &mut Context<Workspace>) {
        for terminal_pane in self.terminal_center.panes() {
            let zoomed = pane == Some(terminal_pane);
            if terminal_pane.read(cx).is_zoomed() != zoomed {
                terminal_pane.update(cx, |terminal_pane, cx| terminal_pane.set_zoomed(zoomed, cx));
            }
        }
        self.notify_mode(WorkspaceMode::Terminal, cx);
    }

    fn activate_terminal_pane_in_direction(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if let Some(pane) =
            self.terminal_center.find_pane_in_direction(&self.active_terminal_pane, direction, cx).cloned()
        {
            window.focus(&pane.focus_handle(cx), cx);
        }
    }

    fn activate_terminal_pane_by_offset(&self, forward: bool, window: &mut Window, cx: &mut Context<Workspace>) {
        let panes = self.terminal_center.panes();
        let Some(index) = panes.iter().position(|pane| **pane == self.active_terminal_pane) else {
            return;
        };
        let pane = panes[cycled_index(index, panes.len(), forward)].clone();
        window.focus(&pane.focus_handle(cx), cx);
    }

    fn handle_terminal_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &pane::Event,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        match event {
            pane::Event::AddItem { item } => {
                item.added_to_pane(self, pane.clone(), window, cx);
                self.serialize_workspace(window, cx);
            }
            pane::Event::Split { direction, mode } => {
                self.split_terminal_pane(pane.clone(), *direction, *mode, window, cx);
            }
            pane::Event::Remove { focus_on_pane } => {
                self.remove_terminal_pane(pane, focus_on_pane.as_ref(), window, cx);
            }
            pane::Event::RemovedItem { .. } => {
                if self.terminal_is_empty(cx) {
                    self.zoom_terminal_pane(None, cx);
                }
                self.notify_mode(WorkspaceMode::Terminal, cx);
                self.serialize_workspace(window, cx);
            }
            pane::Event::ZoomIn => self.zoom_terminal_pane(Some(pane), cx),
            pane::Event::ZoomOut => self.zoom_terminal_pane(None, cx),
            pane::Event::Focus => {
                self.set_active_terminal_pane(pane, cx);
                self.switch_mode(WorkspaceMode::Terminal, cx);
            }
            pane::Event::ActivateItem { .. } => {
                self.notify_mode(WorkspaceMode::Terminal, cx);
                self.serialize_workspace(window, cx);
            }
            pane::Event::ChangeItemTitle => self.notify_mode(WorkspaceMode::Terminal, cx),
            _ => {}
        }
    }

    /// The layout to save: the saved one while terminal mode hasn't been shown
    /// yet, else the current panes with each terminal's working directory.
    pub(crate) fn terminal_layout_to_save(&self, cx: &App) -> Option<SerializedTerminalGroup> {
        match &self.terminal_layout {
            TerminalLayout::Pending(saved_layout) => saved_layout.clone(),
            TerminalLayout::Started => {
                let delegate = terminal_mode_delegate(cx)?;
                self.serialize_terminal_member(&self.terminal_center.root, delegate.as_ref(), cx).prune()
            }
        }
    }

    fn serialize_terminal_member(
        &self,
        member: &Member,
        delegate: &dyn TerminalModeDelegate,
        cx: &App,
    ) -> SerializedTerminalGroup {
        match member {
            Member::Axis(PaneAxis { axis, members, flexes, .. }) => SerializedTerminalGroup::Group {
                axis: *axis,
                flexes: Some(flexes.lock().clone()),
                children: members.iter().map(|member| self.serialize_terminal_member(member, delegate, cx)).collect(),
            },
            Member::Pane(pane_entity) => {
                let pane = pane_entity.read(cx);
                let is_terminal: Vec<bool> = pane.items().map(|item| delegate.is_terminal(item.as_ref(), cx)).collect();
                let tabs = pane
                    .items()
                    .filter(|item| delegate.is_terminal(item.as_ref(), cx))
                    .map(|item| SerializedTerminalTab {
                        working_directory: delegate.working_directory(item.as_ref(), cx),
                    })
                    .collect();
                SerializedTerminalGroup::Pane(SerializedTerminalPane {
                    active: *pane_entity == self.active_terminal_pane,
                    active_tab: saved_active_tab(&is_terminal, pane.active_item_index()),
                    tabs,
                })
            }
        }
    }

    /// Rebuilds the saved splits and starts a shell for each saved tab, in
    /// order, in its directory.
    fn restore_terminal_layout(
        &mut self,
        saved_layout: SerializedTerminalGroup,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let Some(delegate) = terminal_mode_delegate(cx) else {
            return;
        };
        let mut panes = Vec::new();
        let root = self.build_terminal_member(saved_layout, &mut panes, window, cx);
        self.terminal_center = PaneGroup::with_root(root);
        let active_pane = panes
            .iter()
            .find(|(_, saved_pane)| saved_pane.active)
            .map(|(pane, _)| pane.clone())
            .unwrap_or_else(|| self.terminal_center.first_pane());
        self.active_terminal_pane = active_pane.clone();
        window.focus(&active_pane.focus_handle(cx), cx);

        for (pane, saved_pane) in panes {
            let delegate = delegate.clone();
            let is_active_pane = pane == active_pane;
            cx.spawn_in(window, async move |workspace, cx| {
                // One after another, so tabs keep their saved order.
                for tab in saved_pane.tabs {
                    let pane = pane.clone();
                    let task = workspace.update_in(cx, |workspace, window, cx| {
                        delegate.open_shell(workspace, pane, tab.working_directory, false, window, cx)
                    })?;
                    task.await.log_err();
                }
                workspace.update_in(cx, |workspace, window, cx| {
                    let focus = is_active_pane && workspace.mode == WorkspaceMode::Terminal;
                    let active_tab = saved_pane.active_tab.min(pane.read(cx).items_len().saturating_sub(1));
                    pane.update(cx, |pane, cx| pane.activate_item(active_tab, true, focus, window, cx));
                })
            })
            .detach_and_log_err(cx);
        }
        self.notify_mode(WorkspaceMode::Terminal, cx);
    }

    fn build_terminal_member(
        &self,
        saved_group: SerializedTerminalGroup,
        panes: &mut Vec<(Entity<Pane>, SerializedTerminalPane)>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Member {
        match saved_group {
            SerializedTerminalGroup::Group { axis, flexes, children } => {
                let members =
                    children.into_iter().map(|child| self.build_terminal_member(child, panes, window, cx)).collect();
                Member::Axis(PaneAxis::load(axis, members, flexes))
            }
            SerializedTerminalGroup::Pane(saved_pane) => {
                let pane = self.add_terminal_pane(window, cx);
                panes.push((pane.clone(), saved_pane));
                Member::Pane(pane)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalModeEntry;
    use super::cycled_index;
    use super::saved_active_tab;
    use super::terminal_mode_entry;

    #[test]
    fn test_terminal_mode_entry() {
        // The saved layout comes back the first time, shell or not.
        assert_eq!(terminal_mode_entry(true, true, true, true, true), TerminalModeEntry::Restore);
        assert_eq!(terminal_mode_entry(true, true, true, true, false), TerminalModeEntry::Restore);
        // Nothing saved: the first switch starts a shell.
        assert_eq!(terminal_mode_entry(true, false, true, true, true), TerminalModeEntry::StartShell);
        // A caller opening its own shell doesn't get a second one.
        assert_eq!(terminal_mode_entry(true, false, true, true, false), TerminalModeEntry::Nothing);
        // Switching back to an emptied mode starts a shell again.
        assert_eq!(terminal_mode_entry(false, false, true, true, true), TerminalModeEntry::StartShell);
        // Pressing the key while shown keeps the empty page.
        assert_eq!(terminal_mode_entry(false, false, false, true, true), TerminalModeEntry::Nothing);
        // Terminals already open.
        assert_eq!(terminal_mode_entry(false, false, true, false, true), TerminalModeEntry::Nothing);
    }

    #[test]
    fn test_cycled_index() {
        assert_eq!(cycled_index(0, 3, true), 1);
        assert_eq!(cycled_index(2, 3, true), 0);
        assert_eq!(cycled_index(0, 3, false), 2);
        assert_eq!(cycled_index(1, 3, false), 0);
        assert_eq!(cycled_index(0, 1, false), 0);
    }

    #[test]
    fn test_saved_active_tab() {
        assert_eq!(saved_active_tab(&[true, true, true], 2), 2);
        // A tab that isn't saved (a failed spawn) shifts the later ones.
        assert_eq!(saved_active_tab(&[true, false, true], 2), 1);
        assert_eq!(saved_active_tab(&[], 0), 0);
    }
}
