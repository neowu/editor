//! The window has no title bar row. The window header (room for the macOS
//! traffic lights, the project name, worktree and branch, the Editor / Git
//! History switch, and Go Back / Go Forward) sits above the sidebar when one is shown, otherwise at
//! the start of the top-left tab bar. Above a sidebar narrower than the header,
//! the header overhangs it and the top-left tab bar starts after the header.

use ed_actions::mode::Editor;
use ed_actions::mode::GitHistory;
use ed_actions::mode::Terminal;
use gpui::AnyElement;
use gpui::App;
use gpui::Context;
use gpui::Entity;
use gpui::IntoElement;
use gpui::Pixels;
use gpui::Render;
use gpui::Subscription;
use gpui::TaskExt as _;
use gpui::WeakEntity;
use gpui::Window;
use gpui::canvas;
use gpui::div;
use project::Project;
use project::git_store::GitStoreEvent;
use ui::Tab;
use ui::Tooltip;
use ui::prelude::ActiveTheme;
use ui::prelude::ButtonCommon;
use ui::prelude::ButtonSize;
use ui::prelude::ButtonStyle;
use ui::prelude::Clickable;
use ui::prelude::Color;
use ui::prelude::Disableable;
use ui::prelude::DynamicSpacing;
use ui::prelude::FluentBuilder;
use ui::prelude::IconName;
use ui::prelude::InteractiveElement;
use ui::prelude::ParentElement;
use ui::prelude::Styled;
use ui::prelude::h_flex;
use ui::prelude::px;
use ui::utils::TRAFFIC_LIGHT_PADDING;
use ui::utils::window_drag_area;
use util::ResultExt;

use crate::CARD_GAP;
use crate::GoBack;
use crate::GoForward;
use crate::Pane;
use crate::Workspace;
use crate::WorkspaceMode;
use crate::dock::Dock;
use crate::pane::top_row_icon_button;

/// Where tab bar start content begins, from the window's left edge, when the
/// pane is not after a sidebar: `CARD_GAP`, the folder tab bar's 10px inset,
/// then the start slot's 6px padding.
pub(crate) const TAB_BAR_START_OFFSET: Pixels = px(24.);

/// Width of the empty room left for the traffic lights by an element that
/// starts `offset` from the window's left edge. Fullscreen windows hide them.
fn traffic_light_room(offset: Pixels, fullscreen: bool) -> Pixels {
    if fullscreen { px(0.) } else { (px(TRAFFIC_LIGHT_PADDING) - offset).max(px(0.)) }
}

/// The folder tab bar's inset before its first tab or start content.
const TAB_BAR_FOLDER_INSET: Pixels = px(10.);

/// Room the tab bar after a `sidebar_width` sidebar keeps before its tabs for
/// a `header_width` window header above that sidebar, so the tabs start where
/// they do with the sidebar hidden: right after the header and the tab bar
/// start slot's `start_padding`. Header and sidebar both start `CARD_GAP` in,
/// and the tabs beside a sidebar start the folder inset after it.
fn sidebar_header_overhang(header_width: Pixels, sidebar_width: Pixels, start_padding: Pixels) -> Pixels {
    (header_width + start_padding - TAB_BAR_FOLDER_INSET - sidebar_width).max(px(0.))
}

/// The window header, drawn above the sidebar or at the start of the header
/// pane's tab bar. Redraws only when what it shows changes, since redrawing it
/// in a tab bar redraws the pane holding it.
pub(crate) struct WindowHeader {
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    shown: WindowHeaderState,
    /// Width last drawn above a sidebar.
    sidebar_width: Pixels,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct WindowHeaderState {
    in_tab_bar: bool,
    can_go_back: bool,
    can_go_forward: bool,
    mode: WorkspaceMode,
    has_repository: bool,
    has_terminal_mode: bool,
}

impl WindowHeaderState {
    fn of(workspace: &Workspace, project: &Entity<Project>, cx: &App) -> WindowHeaderState {
        WindowHeaderState {
            in_tab_bar: workspace.window_header_in_tab_bar(cx),
            can_go_back: workspace.can_go_back(),
            can_go_forward: workspace.can_go_forward(),
            mode: workspace.mode(),
            has_repository: project.read(cx).active_repository(cx).is_some(),
            has_terminal_mode: workspace.supports_terminal_mode(cx),
        }
    }
}

impl WindowHeader {
    /// Made while the workspace is being built, so it takes the left dock and
    /// project rather than reading them from the workspace.
    pub(crate) fn new(
        workspace: &Entity<Workspace>,
        left_dock: &Entity<Dock>,
        project: &Entity<Project>,
        cx: &mut Context<WindowHeader>,
    ) -> WindowHeader {
        let git_store = project.read(cx).git_store().clone();
        let subscriptions = vec![
            cx.observe(workspace, |header: &mut WindowHeader, _, cx| header.refresh(cx)),
            cx.observe(left_dock, |header: &mut WindowHeader, _, cx| header.refresh(cx)),
            cx.subscribe(&git_store, |header: &mut WindowHeader, _, event: &GitStoreEvent, cx| {
                if matches!(event, GitStoreEvent::ActiveRepositoryChanged(_)) {
                    header.refresh(cx);
                }
            }),
        ];
        WindowHeader {
            workspace: workspace.downgrade(),
            project: project.clone(),
            shown: WindowHeaderState::default(),
            sidebar_width: px(0.),
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&self, cx: &mut Context<WindowHeader>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        if WindowHeaderState::of(workspace.read(cx), &self.project, cx) != self.shown {
            cx.notify();
        }
    }
}

impl Render for WindowHeader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<WindowHeader>) -> impl IntoElement {
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let workspace = workspace.read(cx);
        self.shown = WindowHeaderState::of(workspace, &self.project, cx);
        let fullscreen = window.is_fullscreen() || window.is_simple_fullscreen();
        let offset = if self.shown.in_tab_bar { TAB_BAR_START_OFFSET } else { CARD_GAP };

        h_flex()
            .h_full()
            .gap_1()
            .when(!self.shown.in_tab_bar, |this| this.child(self.measure_sidebar_width(cx)))
            .child(window_drag_area(div().id("traffic-lights").h_full()).w(traffic_light_room(offset, fullscreen)))
            .children(workspace.render_titlebar_region())
            .child(
                h_flex()
                    .ml_1()
                    .gap_2()
                    // Clicks on the controls must not move the window.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .map(|this| {
                        let modes = switchable_modes(self.shown.has_repository, self.shown.has_terminal_mode);
                        this.when(modes.len() > 1, |this| this.child(render_mode_switch(self.shown.mode, &modes, cx)))
                    })
                    .child(render_nav_buttons(&self.workspace, self.shown)),
            )
            .into_any_element()
    }
}

impl WindowHeader {
    /// Records the header's width above a sidebar, redrawing the workspace when
    /// it changes so the top-left tab bar keeps room for any overhang.
    fn measure_sidebar_width(&self, cx: &Context<WindowHeader>) -> impl IntoElement {
        let header = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                header
                    .update(cx, |header, cx| {
                        if header.sidebar_width != bounds.size.width {
                            header.sidebar_width = bounds.size.width;
                            header.workspace.update(cx, |_, cx| cx.notify()).ok();
                        }
                    })
                    .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }
}

/// Go Back / Go Forward over the workspace's one history, which spans every
/// pane in both modes.
fn render_nav_buttons(workspace: &WeakEntity<Workspace>, state: WindowHeaderState) -> impl IntoElement {
    let back_workspace = workspace.clone();
    let go_back = top_row_icon_button("go-back", IconName::ArrowLeft)
        .disabled(!state.can_go_back)
        .tooltip(|_window, cx| Tooltip::for_action("Go Back", &GoBack, cx))
        .on_click(move |_, window, cx| {
            back_workspace.update(cx, |workspace, cx| workspace.go_back(window, cx).detach_and_log_err(cx)).log_err();
        });

    let forward_workspace = workspace.clone();
    let go_forward = top_row_icon_button("go-forward", IconName::ArrowRight)
        .disabled(!state.can_go_forward)
        .tooltip(|_window, cx| Tooltip::for_action("Go Forward", &GoForward, cx))
        .on_click(move |_, window, cx| {
            forward_workspace
                .update(cx, |workspace, cx| workspace.go_forward(window, cx).detach_and_log_err(cx))
                .log_err();
        });

    h_flex().gap_1().child(go_back).child(go_forward)
}

/// Whether `mode`'s top-row tab bars are part of the window's top row. Terminal
/// mode has a header row above its panes instead, which moves the window.
const fn tab_bars_are_titlebar(mode: WorkspaceMode) -> bool {
    !matches!(mode, WorkspaceMode::Terminal)
}

/// Inset of the mode switch's buttons within its track.
const MODE_SWITCH_PADDING: Pixels = px(2.);

/// Size of the mode switch's buttons: shorter (22px) than a top-row button so
/// the filled track clears the row's edges, and the card below, instead of
/// filling the row. They keep the top-row width and icon.
const MODE_SWITCH_BUTTON_SIZE: ButtonSize = ButtonSize::Default;

/// The modes the switch offers, in key order (`cmd-1`, `cmd-2`, `cmd-3`):
/// Git History needs a repository, Terminal a project that runs terminals.
fn switchable_modes(has_repository: bool, has_terminal_mode: bool) -> Vec<WorkspaceMode> {
    [
        (WorkspaceMode::Editor, true),
        (WorkspaceMode::GitHistory, has_repository),
        (WorkspaceMode::Terminal, has_terminal_mode),
    ]
    .into_iter()
    .filter_map(|(mode, available)| available.then_some(mode))
    .collect()
}

/// Icon-only mode switch (Editor / Git History / Terminal): top-row icon
/// buttons grouped on a faint track, so the modes read as one control, with
/// the shown mode's button filled and its icon in the accent colour.
fn render_mode_switch(shown_mode: WorkspaceMode, modes: &[WorkspaceMode], cx: &App) -> impl IntoElement {
    let segment = |mode: WorkspaceMode| {
        let (id, icon, label, action): (&'static str, IconName, &'static str, Box<dyn gpui::Action>) = match mode {
            WorkspaceMode::Editor => ("workspace-mode-editor", IconName::Code, "Editor", Box::new(Editor)),
            WorkspaceMode::GitHistory => {
                ("workspace-mode-git-history", IconName::GitGraph, "Git History", Box::new(GitHistory))
            }
            WorkspaceMode::Terminal => {
                ("workspace-mode-terminal", IconName::TerminalAlt, "Terminal", Box::new(Terminal))
            }
        };
        let shown = mode == shown_mode;
        let tooltip_action = action.boxed_clone();
        top_row_icon_button(id, icon)
            .size(MODE_SWITCH_BUTTON_SIZE)
            .icon_color(if shown { Color::Accent } else { Color::Muted })
            .when(shown, |this| this.style(ButtonStyle::Filled))
            .tooltip(move |_window, cx| Tooltip::for_action(label, tooltip_action.as_ref(), cx))
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    };

    h_flex()
        .p(MODE_SWITCH_PADDING)
        .gap(MODE_SWITCH_PADDING)
        .rounded_lg()
        .bg(cx.theme().colors().element_background.opacity(0.4))
        .children(modes.iter().map(|mode| segment(*mode)))
}

impl Workspace {
    /// Whether the window header sits in the tab bar because no sidebar is
    /// shown to hold it. Terminal mode never puts it there: its pill tabs sit
    /// inside the cards, so it has a header row of its own.
    pub(crate) fn window_header_in_tab_bar(&self, cx: &App) -> bool {
        match self.mode {
            WorkspaceMode::Editor => {
                self.has_zoomed(WorkspaceMode::Editor, cx) || self.left_dock.read(cx).visible_panel().is_none()
            }
            WorkspaceMode::GitHistory => self.git_history_side_card(cx).is_none(),
            WorkspaceMode::Terminal => false,
        }
    }

    /// The strip above a sidebar (left dock or Git History sidebar), or above
    /// terminal mode's panes, that the window header sits on: it moves the window and is as high as a top-row
    /// tab bar with its gap, so the sidebar content lines up with the cards.
    pub(crate) fn render_sidebar_header(&self, cx: &App) -> impl IntoElement {
        window_drag_area(div().id("sidebar-header")).flex_none().w_full().h(CARD_GAP + Tab::container_height(cx))
    }

    /// The window header over the sidebar's header strip. It's drawn over the
    /// mode's content rather than inside the sidebar, so a narrow sidebar
    /// doesn't clip it and the top-left tab bar beside it doesn't cover it.
    pub(crate) fn render_sidebar_header_overlay(&self, cx: &App) -> gpui::Div {
        div()
            .absolute()
            .top_0()
            .left(CARD_GAP)
            .h(CARD_GAP + Tab::container_height(cx))
            .pt(CARD_GAP)
            .flex()
            .items_center()
            .child(self.window_header.clone())
    }

    /// Width of the sidebar holding the window header in `mode`, up to the
    /// panes, or `None` when the header is in a tab bar.
    fn header_sidebar_width(&self, mode: WorkspaceMode, window: &Window, cx: &App) -> Option<Pixels> {
        if self.window_header_in_tab_bar(cx) {
            return None;
        }
        match mode {
            WorkspaceMode::Editor => {
                let dock = self.left_dock.read(cx);
                let size = dock.stored_active_panel_size(window, cx)?;
                let min_size = dock.visible_panel()?.min_size(window, cx).unwrap_or(px(0.));
                Some(size.max(min_size))
            }
            WorkspaceMode::GitHistory => Some(self.git_history_sidebar_width),
            WorkspaceMode::Terminal => None,
        }
    }

    /// The title bar item (project, worktree and branch) inside its focus region.
    /// The region is a toolbar so region navigation (FocusNextPart) can reach
    /// its controls: it lands on the first control, Tab steps through them,
    /// and arrow keys move between them once focus is inside.
    pub(crate) fn render_titlebar_region(&self) -> Option<AnyElement> {
        let item = self.titlebar_item.clone()?;
        let workspace = self.weak_self.clone();
        Some(
            div()
                .id("titlebar-region")
                .track_focus(&self.titlebar_focus_handle)
                .tab_group()
                .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    let forward = match event.keystroke.key.as_str() {
                        "right" => true,
                        "left" => false,
                        _ => return,
                    };
                    workspace
                        .update(cx, |workspace, cx| workspace.move_titlebar_item_focus(forward, window, cx))
                        .log_err();
                    cx.stop_propagation();
                })
                // Clicks on the project controls must not move the window.
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(item)
                .into_any_element(),
        )
    }

    /// The pane whose tab bar starts the window's top row in `mode`, which
    /// holds the [`WindowHeader`] when no sidebar does, or `None` while a
    /// zoomed dock covers the panes.
    fn tab_bar_header_pane(&self, mode: WorkspaceMode, cx: &App) -> Option<Entity<Pane>> {
        match mode {
            WorkspaceMode::Editor => {
                if self.zoomed_position.is_some() {
                    return None;
                }
                let zoomed_pane = self.zoomed_pane(mode, cx);
                let maximized_pane = self.maximized_pane.as_ref().and_then(WeakEntity::upgrade);
                Some(zoomed_pane.or(maximized_pane).unwrap_or_else(|| self.center.first_pane()))
            }
            WorkspaceMode::GitHistory => {
                Some(self.zoomed_pane(mode, cx).unwrap_or_else(|| self.git_history_center.first_pane()))
            }
            WorkspaceMode::Terminal => {
                Some(self.zoomed_pane(mode, cx).unwrap_or_else(|| self.terminal_center.first_pane()))
            }
        }
    }

    /// Puts the [`WindowHeader`] on `mode`'s header pane when no sidebar holds it, and marks the panes
    /// whose tab bars form the window's top row, so their empty space moves
    /// the window. Panes only redraw when this changes for them.
    /// The pane after the sidebar keeps room for a header overhanging it.
    pub(crate) fn assign_tab_bar_chrome(&self, mode: WorkspaceMode, window: &Window, cx: &mut Context<Self>) {
        let group = match mode {
            WorkspaceMode::Editor => &self.center,
            WorkspaceMode::GitHistory => &self.git_history_center,
            WorkspaceMode::Terminal => &self.terminal_center,
        };
        let header_pane = self.window_header_in_tab_bar(cx).then(|| self.tab_bar_header_pane(mode, cx)).flatten();
        let header_width = self.window_header.read(cx).sidebar_width;
        let overhang = self
            .header_sidebar_width(mode, window, cx)
            .map(|sidebar_width| sidebar_header_overhang(header_width, sidebar_width, DynamicSpacing::Base06.px(cx)))
            .filter(|overhang| *overhang > px(0.))
            .and_then(|overhang| Some((self.tab_bar_header_pane(mode, cx)?, overhang)));
        let top_row: Vec<Entity<Pane>> =
            if tab_bars_are_titlebar(mode) { group.top_row_panes().into_iter().cloned().collect() } else { Vec::new() };
        let panes: Vec<Entity<Pane>> = group.panes().into_iter().cloned().collect();
        for pane in panes {
            let has_header = header_pane.as_ref() == Some(&pane);
            let header = has_header.then(|| self.window_header.clone().into());
            let is_titlebar = has_header || top_row.contains(&pane);
            let header_room = match &overhang {
                Some((overhang_pane, overhang)) if *overhang_pane == pane => *overhang,
                _ => px(0.),
            };
            pane.update(cx, |pane, cx| pane.set_tab_bar_chrome(header, is_titlebar, header_room, cx));
        }
    }

    /// The header row above a zoomed dock, which covers the pane that would
    /// hold the [`WindowHeader`]. Starts where a tab bar's start content would.
    pub(crate) fn render_zoomed_dock_header(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .flex_none()
            .w_full()
            .h(CARD_GAP + Tab::container_height(cx))
            .pt(CARD_GAP)
            .pl(TAB_BAR_START_OFFSET - CARD_GAP)
            .child(self.window_header.clone())
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;
    use ui::utils::TRAFFIC_LIGHT_PADDING;

    use super::MODE_SWITCH_BUTTON_SIZE;
    use super::MODE_SWITCH_PADDING;
    use super::TAB_BAR_START_OFFSET;
    use super::sidebar_header_overhang;
    use super::switchable_modes;
    use super::tab_bars_are_titlebar;
    use super::traffic_light_room;

    #[test]
    fn test_tab_bars_are_titlebar() {
        use crate::WorkspaceMode;
        assert!(tab_bars_are_titlebar(WorkspaceMode::Editor));
        assert!(tab_bars_are_titlebar(WorkspaceMode::GitHistory));
        assert!(!tab_bars_are_titlebar(WorkspaceMode::Terminal));
    }

    #[test]
    fn test_switchable_modes() {
        use crate::WorkspaceMode;
        assert_eq!(switchable_modes(false, false), [WorkspaceMode::Editor]);
        assert_eq!(switchable_modes(true, false), [WorkspaceMode::Editor, WorkspaceMode::GitHistory]);
        assert_eq!(switchable_modes(false, true), [WorkspaceMode::Editor, WorkspaceMode::Terminal]);
        assert_eq!(
            switchable_modes(true, true),
            [WorkspaceMode::Editor, WorkspaceMode::GitHistory, WorkspaceMode::Terminal]
        );
    }

    #[test]
    fn test_mode_switch_track_clears_top_row() {
        use crate::pane::TOP_ROW_BUTTON_SIZE;
        let track = MODE_SWITCH_BUTTON_SIZE.rems().to_pixels(px(16.)) + MODE_SWITCH_PADDING * 2.;
        // Shorter than a 32px track of top-row buttons, leaving at least 4px
        // clear above and below in the 36px top row.
        assert_eq!(track, px(26.));
        assert!(track < TOP_ROW_BUTTON_SIZE + MODE_SWITCH_PADDING * 2.);
        assert!((px(36.) - track) / 2. >= px(4.));
    }

    #[test]
    fn test_traffic_light_room() {
        assert_eq!(traffic_light_room(px(0.), false), px(TRAFFIC_LIGHT_PADDING));
        assert_eq!(traffic_light_room(TAB_BAR_START_OFFSET, false), px(TRAFFIC_LIGHT_PADDING) - TAB_BAR_START_OFFSET);
        assert_eq!(traffic_light_room(px(500.), false), px(0.));
        assert_eq!(traffic_light_room(TAB_BAR_START_OFFSET, true), px(0.));
    }

    #[test]
    fn test_sidebar_header_overhang() {
        // A sidebar wide enough that its tabs already clear the header leaves no room.
        assert_eq!(sidebar_header_overhang(px(300.), px(400.), px(6.)), px(0.));
        assert_eq!(sidebar_header_overhang(px(300.), px(296.), px(6.)), px(0.));
        // Narrower, the room puts the first tab right after the header and the start padding.
        assert_eq!(sidebar_header_overhang(px(300.), px(200.), px(6.)), px(96.));
        assert_eq!(sidebar_header_overhang(px(300.), px(0.), px(6.)), px(296.));
        assert_eq!(sidebar_header_overhang(px(300.), px(200.), px(8.)), px(98.));
    }
}
