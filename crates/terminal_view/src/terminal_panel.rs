use std::cmp;
use std::iter;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::Result;
use futures::channel::oneshot;
use futures::future::join_all;
use gpui::Action;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::AsyncWindowContext;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Pixels;
use gpui::Render;
use gpui::Styled;
use gpui::Task;
use gpui::TaskExt;
use gpui::WeakEntity;
use gpui::Window;
use gpui::actions;
use itertools::Itertools;
use project::Project;
use rustc_hash::FxHashMap;
use task::RevealStrategy;
use task::SpawnInTerminal;
use task::TaskId;
use terminal::Terminal;
use terminal::terminal_settings::TerminalSettings;
use ui::CommonAnimationExt;
use ui::Tooltip;
use ui::prelude::ActiveTheme;
use ui::prelude::ButtonCommon;
use ui::prelude::Clickable;
use ui::prelude::Color;
use ui::prelude::DynamicSpacing;
use ui::prelude::FluentBuilder;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::InteractiveElement;
use ui::prelude::Label;
use ui::prelude::LabelCommon;
use ui::prelude::LabelSize;
use ui::prelude::SharedString;
use ui::prelude::div;
use ui::prelude::h_flex;
use ui::prelude::v_flex;
use util::ResultExt;
use util::defer;
use workspace::ActivateNextPane;
use workspace::ActivatePane;
use workspace::ActivatePaneDown;
use workspace::ActivatePaneLeft;
use workspace::ActivatePaneRight;
use workspace::ActivatePaneUp;
use workspace::ActivatePreviousPane;
use workspace::DraggedTab;
use workspace::MoveItemToPane;
use workspace::MoveItemToPaneInDirection;
use workspace::MovePaneDown;
use workspace::MovePaneLeft;
use workspace::MovePaneRight;
use workspace::MovePaneUp;
use workspace::Pane;
use workspace::PaneGroup;
use workspace::SplitDirection;
use workspace::SplitMode;
use workspace::SwapPaneDown;
use workspace::SwapPaneLeft;
use workspace::SwapPaneRight;
use workspace::SwapPaneUp;
use workspace::Workspace;
use workspace::WorkspaceMode;
use workspace::dock::DockPosition;
use workspace::dock::Panel;
use workspace::dock::PanelEvent;
use workspace::move_active_item;
use workspace::pane;

use crate::TerminalView;
use crate::default_working_directory;

const TERMINAL_PANEL_KEY: &str = "TerminalPanel";

actions!(
    terminal_panel,
    [
        /// Toggles the terminal panel.
        Toggle,
        /// Toggles focus on the terminal panel.
        ToggleFocus,
        /// Shows a shell terminal (not task output) in the terminal panel,
        /// opening one if there is none, or hides the panel when a shell is
        /// already shown.
        ToggleShell
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
        workspace.register_action(TerminalPanel::new_terminal);
        workspace.register_action(TerminalPanel::open_terminal);
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            if is_enabled_in_workspace(workspace, cx) {
                workspace.toggle_panel_focus::<TerminalPanel>(window, cx);
            }
        });
        workspace.register_action(TerminalPanel::toggle_shell);
        workspace.register_action(|workspace, _: &Toggle, window, cx| {
            if is_enabled_in_workspace(workspace, cx) && !workspace.toggle_panel_focus::<TerminalPanel>(window, cx) {
                workspace.close_panel::<TerminalPanel>(window, cx);
            }
        });
    })
    .detach();
}

pub struct TerminalPanel {
    pub(crate) active_pane: Entity<Pane>,
    pub(crate) center: PaneGroup,
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    pending_terminals_to_add: usize,
    deferred_tasks: FxHashMap<TaskId, Task<()>>,
    active: bool,
}

impl TerminalPanel {
    pub fn new(workspace: &Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = workspace.project();
        let pane = new_terminal_pane(workspace.weak_handle(), project.clone(), false, window, cx);
        let mut center = PaneGroup::new(pane.clone());
        center.set_backdrop(workspace::pane_group::PaneBackdrop::Panel);
        Self {
            center,
            active_pane: pane,
            focus_handle: cx.focus_handle(),
            workspace: workspace.weak_handle(),
            pending_terminals_to_add: 0,
            deferred_tasks: FxHashMap::default(),
            active: false,
        }
    }

    pub async fn load(workspace: WeakEntity<Workspace>, mut cx: AsyncWindowContext) -> Result<Entity<Self>> {
        let terminal_panel = workspace
            .update_in(&mut cx, |workspace, window, cx| cx.new(|cx| TerminalPanel::new(workspace, window, cx)))?;

        workspace
            .update(&mut cx, |workspace, _| workspace.set_terminal_provider(TerminalProvider(terminal_panel.clone())))
            .ok();

        Ok(terminal_panel)
    }

    fn handle_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &pane::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            pane::Event::Remove { focus_on_pane } => {
                let pane_count_before_removal = self.center.panes().len();
                let _removal_result = self.center.remove(pane, cx);
                if pane_count_before_removal == 1 {
                    self.center.first_pane().update(cx, |pane, cx| {
                        pane.set_zoomed(false, cx);
                    });
                    cx.emit(PanelEvent::Close);
                } else if let Some(focus_on_pane) = focus_on_pane.as_ref().or_else(|| self.center.panes().pop()) {
                    focus_on_pane.focus_handle(cx).focus(window, cx);
                }
            }
            pane::Event::ZoomIn => {
                for pane in self.center.panes() {
                    pane.update(cx, |pane, cx| {
                        pane.set_zoomed(true, cx);
                    })
                }
                cx.emit(PanelEvent::ZoomIn);
                cx.notify();
            }
            pane::Event::ZoomOut => {
                for pane in self.center.panes() {
                    pane.update(cx, |pane, cx| {
                        pane.set_zoomed(false, cx);
                    })
                }
                cx.emit(PanelEvent::ZoomOut);
                cx.notify();
            }
            pane::Event::AddItem { item } => {
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| item.added_to_pane(workspace, pane.clone(), window, cx))
                }
            }
            &pane::Event::Split { direction, mode } => {
                // Moving needs another tab left behind; otherwise open a new shell beside.
                if mode == SplitMode::MovePane && pane.read(cx).items_len() > 1 {
                    let Some(item) = pane.update(cx, |pane, cx| pane.take_active_item(window, cx)) else {
                        return;
                    };
                    let Ok(project) = self.workspace.update(cx, |workspace, _| workspace.project().clone()) else {
                        return;
                    };
                    let new_pane = new_terminal_pane(self.workspace.clone(), project, false, window, cx);
                    new_pane.update(cx, |pane, cx| {
                        pane.add_item(item, true, true, None, window, cx);
                    });
                    self.center.split(pane, &new_pane, direction, cx);
                    window.focus(&new_pane.focus_handle(cx), cx);
                    return;
                }
                let new_pane = self.new_pane_with_active_terminal(window, cx);
                let pane = pane.clone();
                cx.spawn_in(window, async move |panel, cx| {
                    let Some(new_pane) = new_pane.await else {
                        return;
                    };
                    panel
                        .update_in(cx, |panel, window, cx| {
                            panel.center.split(&pane, &new_pane, direction, cx);
                            window.focus(&new_pane.focus_handle(cx), cx);
                        })
                        .ok();
                })
                .detach();
            }
            pane::Event::Focus => {
                self.active_pane = pane.clone();
            }

            _ => {}
        }
    }

    /// A pane with a new shell cloned from the active terminal (its folder, and
    /// zoomed if it is).
    fn new_pane_with_active_terminal(&self, window: &Window, cx: &Context<Self>) -> Task<Option<Entity<Pane>>> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Task::ready(None);
        };
        let workspace = workspace.read(cx);
        let weak_workspace = self.workspace.clone();
        let project = workspace.project().clone();
        let active_pane = &self.active_pane;
        let terminal_view = active_pane.read(cx).active_item().and_then(|item| item.downcast::<TerminalView>());
        let working_directory = terminal_view
            .as_ref()
            .and_then(|terminal_view| terminal_view.read(cx).terminal().read(cx).working_directory())
            .or_else(|| default_working_directory(workspace, cx));
        let is_zoomed = active_pane.read(cx).is_zoomed();
        cx.spawn_in(window, async move |panel, cx| {
            let terminal = project
                .update(cx, |project, cx| match terminal_view {
                    Some(view) => project.clone_terminal(&view.read(cx).terminal.clone(), cx, working_directory),
                    None => project.create_terminal_shell(working_directory, cx),
                })
                .await
                .log_err()?;

            panel
                .update_in(cx, move |_, window, cx| {
                    let terminal_view = Box::new(cx.new(|cx| {
                        TerminalView::new(terminal.clone(), weak_workspace.clone(), project.downgrade(), window, cx)
                    }));
                    let pane = new_terminal_pane(weak_workspace, project, is_zoomed, window, cx);
                    pane.update(cx, |pane, cx| {
                        pane.add_item(terminal_view, true, true, None, window, cx);
                    });
                    Some(pane)
                })
                .ok()
                .flatten()
        })
    }

    fn toggle_shell(workspace: &mut Workspace, _: &ToggleShell, window: &mut Window, cx: &mut Context<Workspace>) {
        if !workspace.mode().has_docks() || !is_enabled_in_workspace(workspace, cx) {
            return;
        }
        let Some(terminal_panel) = workspace.panel::<Self>(cx) else {
            return;
        };
        let bottom_dock = workspace.dock_at_position(DockPosition::Bottom).read(cx);
        let visible = workspace.is_dock_visible(DockPosition::Bottom, cx)
            && bottom_dock.visible_panel().is_some_and(|panel| panel.panel_id() == terminal_panel.entity_id());

        let panel = terminal_panel.read(cx);
        let panes: Vec<Entity<Pane>> = panel.center.panes().into_iter().cloned().collect();
        let active_pane_index = panes.iter().position(|pane| *pane == panel.active_pane).unwrap_or(0);
        let pane_shells: Vec<PaneShells> = panes
            .iter()
            .map(|pane| {
                let pane = pane.read(cx);
                PaneShells {
                    active_item: (pane.items_len() > 0).then(|| pane.active_item_index()),
                    shells: pane
                        .items()
                        .map(|item| {
                            item.act_as::<TerminalView>(cx)
                                .is_some_and(|view| view.read(cx).terminal().read(cx).task().is_none())
                        })
                        .collect(),
                }
            })
            .collect();
        let has_no_terminals = panel.has_no_terminals(cx);

        match shell_toggle_target(visible, active_pane_index, &pane_shells) {
            ShellToggleTarget::Close => workspace.toggle_dock(DockPosition::Bottom, window, cx),
            ShellToggleTarget::Show { pane, item } => {
                if !visible {
                    workspace.toggle_dock(DockPosition::Bottom, window, cx);
                }
                panes[pane].update(cx, |pane, cx| pane.activate_item(item, true, true, window, cx));
            }
            ShellToggleTarget::New => {
                if !visible {
                    workspace.toggle_dock(DockPosition::Bottom, window, cx);
                }
                // Opening an empty panel already starts a shell.
                if visible || !has_no_terminals {
                    let working_directory = default_working_directory(workspace, cx);
                    terminal_panel
                        .update(cx, |panel, cx| {
                            panel.add_terminal_shell(false, working_directory, RevealStrategy::Always, window, cx)
                        })
                        .detach_and_log_err(cx);
                }
            }
        }
    }

    /// "Open in Terminal" opens a shell in that directory in terminal mode.
    pub fn open_terminal(
        workspace: &mut Workspace,
        action: &workspace::OpenTerminal,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        workspace.open_terminal_mode_shell(Some(action.working_directory.clone()), window, cx);
    }

    pub fn spawn_task(
        &mut self,
        task: &SpawnInTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<WeakEntity<Terminal>>> {
        let task = task.clone();

        let mut terminals_for_task = self.terminals_for_task(&task.full_label, cx);
        let Some(existing) = terminals_for_task.pop() else {
            let reveal = task.reveal;
            return self.add_terminal_task(task, reveal, window, cx);
        };

        let (existing_item_index, task_pane, existing_terminal) = existing;
        if task.allow_concurrent_runs {
            return self.replace_terminal(task, task_pane, existing_item_index, existing_terminal, window, cx);
        }

        let (tx, rx) = oneshot::channel();

        self.deferred_tasks.insert(
            task.id.clone(),
            cx.spawn_in(window, async move |terminal_panel, cx| {
                wait_for_terminals_tasks(terminals_for_task, cx).await;
                let task = terminal_panel.update_in(cx, |terminal_panel, window, cx| {
                    terminal_panel.replace_terminal(task, task_pane, existing_item_index, existing_terminal, window, cx)
                });
                if let Ok(task) = task {
                    tx.send(task.await).ok();
                }
            }),
        );

        cx.spawn(async move |_, _| rx.await?)
    }

    /// Opens a shell in the project's default directory: in terminal mode's
    /// active pane while that mode is shown, else in the terminal panel.
    fn new_terminal(
        workspace: &mut Workspace,
        action: &workspace::NewTerminal,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if workspace.mode() == WorkspaceMode::Terminal {
            let working_directory = default_working_directory(workspace, cx);
            workspace.open_terminal_mode_shell(working_directory, window, cx);
            return;
        }

        let Some(terminal_panel) = workspace.panel::<Self>(cx) else {
            return;
        };

        terminal_panel
            .update(cx, |this, cx| {
                this.add_terminal_shell(
                    action.local,
                    default_working_directory(workspace, cx),
                    RevealStrategy::Always,
                    window,
                    cx,
                )
            })
            .detach_and_log_err(cx);
    }

    fn terminals_for_task(&self, label: &str, cx: &App) -> Vec<(usize, Entity<Pane>, Entity<TerminalView>)> {
        let pane_terminal_views = |pane: Entity<Pane>| {
            pane.read(cx)
                .items()
                .enumerate()
                .filter_map(|(index, item)| Some((index, item.act_as::<TerminalView>(cx)?)))
                .filter_map(|(index, terminal_view)| {
                    let task_state = terminal_view.read(cx).terminal().read(cx).task()?;
                    if task_state.spawned_task.full_label == label { Some((index, terminal_view)) } else { None }
                })
                .map(move |(index, terminal_view)| (index, pane.clone(), terminal_view))
        };

        self.center
            .panes()
            .into_iter()
            .cloned()
            .flat_map(pane_terminal_views)
            .sorted_by_key(|(_, _, terminal_view)| terminal_view.entity_id())
            .collect()
    }

    fn activate_terminal_view(
        &self,
        pane: &Entity<Pane>,
        item_index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        pane.update(cx, |pane, cx| pane.activate_item(item_index, true, focus, window, cx))
    }

    pub fn add_terminal_task(
        &mut self,
        task: SpawnInTerminal,
        reveal_strategy: RevealStrategy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<WeakEntity<Terminal>>> {
        let workspace = self.workspace.clone();
        self.spawn_pending_terminal(window, cx, async move |terminal_panel, cx| {
            if workspace.update(cx, |workspace, cx| !is_enabled_in_workspace(workspace, cx))? {
                anyhow::bail!("terminal not yet supported for remote projects");
            }
            let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;
            let terminal = project.update(cx, |project, cx| project.create_terminal_task(task, cx)).await?;
            let pane = terminal_panel.read_with(cx, |terminal_panel, _| terminal_panel.active_pane.clone())?;
            workspace.update_in(cx, |workspace, window, cx| {
                let terminal_view = Box::new(cx.new(|cx| {
                    TerminalView::new(
                        terminal.clone(),
                        workspace.weak_handle(),
                        workspace.project().downgrade(),
                        window,
                        cx,
                    )
                }));

                let take_focus = reveal_strategy == RevealStrategy::Always && !workspace.has_active_modal(window, cx);
                match reveal_strategy {
                    RevealStrategy::Always if take_focus => {
                        workspace.focus_panel::<Self>(window, cx);
                    }
                    RevealStrategy::Always | RevealStrategy::NoFocus => {
                        workspace.open_panel::<Self>(window, cx);
                    }
                    RevealStrategy::Never => {}
                }

                pane.update(cx, |pane, cx| {
                    pane.add_item(terminal_view, true, take_focus, None, window, cx);
                });

                terminal.downgrade()
            })
        })
    }

    fn add_terminal_shell(
        &mut self,
        force_local: bool,
        cwd: Option<PathBuf>,
        reveal_strategy: RevealStrategy,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<WeakEntity<Terminal>>> {
        let workspace = self.workspace.clone();
        self.spawn_pending_terminal(window, cx, async move |terminal_panel, cx| {
            if workspace.update(cx, |workspace, cx| !is_enabled_in_workspace(workspace, cx))? {
                anyhow::bail!("terminal is unavailable for this project");
            }
            let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;
            let terminal = if force_local {
                project.update(cx, |project, cx| project.create_local_terminal(cx)).await
            } else {
                project.update(cx, |project, cx| project.create_terminal_shell(cwd, cx)).await
            };

            let pane = terminal_panel.read_with(cx, |terminal_panel, _| terminal_panel.active_pane.clone())?;
            match terminal {
                Ok(terminal) => workspace.update_in(cx, |workspace, window, cx| {
                    let terminal_view = Box::new(cx.new(|cx| {
                        TerminalView::new(
                            terminal.clone(),
                            workspace.weak_handle(),
                            workspace.project().downgrade(),
                            window,
                            cx,
                        )
                    }));

                    let take_focus =
                        reveal_strategy == RevealStrategy::Always && !workspace.has_active_modal(window, cx);
                    match reveal_strategy {
                        RevealStrategy::Always if take_focus => {
                            workspace.focus_panel::<Self>(window, cx);
                        }
                        RevealStrategy::Always | RevealStrategy::NoFocus => {
                            workspace.open_panel::<Self>(window, cx);
                        }
                        RevealStrategy::Never => {}
                    }

                    pane.update(cx, |pane, cx| {
                        pane.add_item(terminal_view, true, take_focus, None, window, cx);
                    });

                    terminal.downgrade()
                }),
                Err(error) => {
                    pane.update_in(cx, |pane, window, cx| {
                        let focus = pane.has_focus(window, cx);
                        let failed_to_spawn = cx.new(|cx| FailedToSpawnTerminal {
                            error: error.to_string(),
                            focus_handle: cx.focus_handle(),
                        });
                        pane.add_item(Box::new(failed_to_spawn), true, focus, None, window, cx);
                    })?;
                    Err(error)
                }
            }
        })
    }

    fn spawn_pending_terminal(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        create_terminal: impl AsyncFnOnce(WeakEntity<Self>, &mut AsyncWindowContext) -> Result<WeakEntity<Terminal>>
        + 'static,
    ) -> Task<Result<WeakEntity<Terminal>>> {
        self.pending_terminals_to_add += 1;
        cx.notify();
        let decrement_when_cancelled = defer({
            let terminal_panel = cx.weak_entity();
            let cx = cx.to_async();
            move || {
                cx.spawn(async move |cx| {
                    terminal_panel.update(cx, |terminal_panel, cx| terminal_panel.finish_pending_terminal(cx)).ok();
                })
                .detach();
            }
        });
        cx.spawn_in(window, async move |terminal_panel, cx| {
            let result = create_terminal(terminal_panel.clone(), cx).await;
            decrement_when_cancelled.abort();
            terminal_panel.update(cx, |terminal_panel, cx| terminal_panel.finish_pending_terminal(cx)).ok();
            result
        })
    }

    fn finish_pending_terminal(&mut self, cx: &mut Context<Self>) {
        self.pending_terminals_to_add = self.pending_terminals_to_add.saturating_sub(1);
        cx.notify();
    }

    fn replace_terminal(
        &self,
        spawn_task: SpawnInTerminal,
        task_pane: Entity<Pane>,
        terminal_item_index: usize,
        terminal_to_replace: Entity<TerminalView>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Task<Result<WeakEntity<Terminal>>> {
        let reveal = spawn_task.reveal;
        let task_workspace = self.workspace.clone();
        cx.spawn_in(window, async move |terminal_panel, cx| {
            let project = terminal_panel
                .update(cx, |this, cx| this.workspace.update(cx, |workspace, _| workspace.project().clone()))??;
            let new_terminal = project.update(cx, |project, cx| project.create_terminal_task(spawn_task, cx)).await?;
            terminal_to_replace.update_in(cx, |terminal_to_replace, window, cx| {
                terminal_to_replace.set_terminal(new_terminal.clone(), window, cx);
            })?;

            // Task tabs stay in the terminal panel: editor panes refuse terminals.
            match reveal {
                RevealStrategy::Always => {
                    terminal_panel.update_in(cx, |terminal_panel, window, cx| {
                        terminal_panel.activate_terminal_view(&task_pane, terminal_item_index, true, window, cx)
                    })?;

                    cx.spawn(async move |cx| {
                        task_workspace
                            .update_in(cx, |workspace, window, cx| workspace.focus_panel::<Self>(window, cx))
                            .ok()
                    })
                    .detach();
                }
                RevealStrategy::NoFocus => {
                    terminal_panel.update_in(cx, |terminal_panel, window, cx| {
                        terminal_panel.activate_terminal_view(&task_pane, terminal_item_index, false, window, cx)
                    })?;

                    cx.spawn(async move |cx| {
                        task_workspace
                            .update_in(cx, |workspace, window, cx| workspace.open_panel::<Self>(window, cx))
                            .ok()
                    })
                    .detach();
                }
                RevealStrategy::Never => {}
            }

            Ok(new_terminal.downgrade())
        })
    }

    fn has_no_terminals(&self, cx: &App) -> bool {
        self.active_pane.read(cx).items_len() == 0 && self.pending_terminals_to_add == 0
    }

    /// The pane a zoomed panel shows alone: zoom expands the active terminal
    /// pane, not every split, as zooming an editor pane does.
    fn zoomed_pane(&self, cx: &App) -> Option<&Entity<Pane>> {
        (self.active_pane.read(cx).is_zoomed() && self.center.panes().len() > 1).then_some(&self.active_pane)
    }

    /// Returns all panes in the terminal panel.
    pub fn panes(&self) -> Vec<&Entity<Pane>> {
        self.center.panes()
    }

    fn is_enabled(&self, cx: &App) -> bool {
        self.workspace.upgrade().is_some_and(|workspace| is_enabled_in_workspace(workspace.read(cx), cx))
    }

    fn activate_pane_in_direction(&mut self, direction: SplitDirection, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.center.find_pane_in_direction(&self.active_pane, direction, cx) {
            window.focus(&pane.focus_handle(cx), cx);
        } else {
            self.workspace.update(cx, |workspace, cx| workspace.activate_pane_in_direction(direction, window, cx)).ok();
        }
    }

    fn swap_pane_in_direction(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        if let Some(to) = self.center.find_pane_in_direction(&self.active_pane, direction, cx).cloned() {
            self.center.swap(&self.active_pane, &to, cx);
            cx.notify();
        }
    }

    fn move_pane_to_border(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        if self.center.move_to_border(&self.active_pane, direction, cx).unwrap() {
            cx.notify();
        }
    }
}

fn is_enabled_in_workspace(workspace: &Workspace, cx: &App) -> bool {
    workspace.project().read(cx).supports_terminal(cx)
}

/// The "+" opens a terminal right away: every entry of the generic new-item
/// menu other than "New Terminal" is unrelated to the terminal panel.
/// Terminal mode's panes also get Split Right / Split Down, which open a shell
/// in the active terminal's directory beside the pane.
pub(crate) fn set_terminal_tab_bar_buttons(pane: &mut Pane, with_splits: bool, cx: &mut Context<Pane>) {
    pane.set_render_tab_bar_buttons(cx, move |pane, window, cx| {
        let has_focused_rename_editor = pane
            .active_item()
            .and_then(|item| item.downcast::<TerminalView>())
            .is_some_and(|view| view.read(cx).rename_editor_is_focused(window, cx));
        if !pane.has_focus(window, cx) && !has_focused_rename_editor {
            return (None, None);
        }
        let right_children = h_flex()
            .gap(DynamicSpacing::Base04.rems())
            .child(
                pane::top_row_icon_button("plus", IconName::Plus)
                    .on_click(|_, window, cx| {
                        window.dispatch_action(workspace::NewTerminal::default().boxed_clone(), cx);
                    })
                    .tooltip(Tooltip::for_action_title("New Terminal", &workspace::NewTerminal::default())),
            )
            .when(with_splits, |this| {
                this.child(
                    pane::top_row_icon_button("split-right", IconName::SplitRight)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.split(SplitDirection::Right, SplitMode::EmptyPane, cx);
                        }))
                        .tooltip(Tooltip::for_action_title("Split Right", &workspace::SplitRight::default())),
                )
                .child(
                    pane::top_row_icon_button("split-down", IconName::SplitDown)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.split(SplitDirection::Down, SplitMode::EmptyPane, cx);
                        }))
                        .tooltip(Tooltip::for_action_title("Split Down", &workspace::SplitDown::default())),
                )
            })
            .children(pane::render_tab_bar_pane_buttons(pane, cx))
            .into_any_element()
            .into();
        (None, right_children)
    });
}

pub fn new_terminal_pane(
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    zoomed: bool,
    window: &mut Window,
    cx: &mut Context<TerminalPanel>,
) -> Entity<Pane> {
    let terminal_panel = cx.entity();
    let pane = cx.new(|cx| {
        let mut pane = Pane::new(
            workspace.clone(),
            project.clone(),
            Arc::default(),
            None,
            workspace::NewTerminal::default().boxed_clone(),
            window,
            cx,
        );
        pane.set_zoomed(zoomed, cx);
        pane.set_can_navigate(false, cx);
        pane.set_tab_style(ui::TabStyle::Pill);
        pane.set_zoom_out_on_close(false);
        set_terminal_tab_bar_buttons(&mut pane, false, cx);

        let split_closure_terminal_panel = terminal_panel.downgrade();
        pane.set_can_split(Some(Arc::new(move |pane, dragged_item, _window, cx| {
            if let Some(tab) = dragged_item.downcast_ref::<DraggedTab>() {
                let is_current_pane = tab.pane == cx.entity();
                let Some(can_drag_away) = split_closure_terminal_panel
                    .read_with(cx, |terminal_panel, _| {
                        let current_panes = terminal_panel.center.panes();
                        !current_panes.contains(&&tab.pane)
                            || current_panes.len() > 1
                            || (!is_current_pane || pane.items_len() > 1)
                    })
                    .ok()
                else {
                    return false;
                };
                if can_drag_away {
                    let item = if is_current_pane {
                        pane.item_for_index(tab.ix)
                    } else {
                        tab.pane.read(cx).item_for_index(tab.ix)
                    };
                    if let Some(item) = item {
                        return item.downcast::<TerminalView>().is_some();
                    }
                }
            }
            false
        })));

        let toolbar = pane.toolbar().clone();
        if let Some(callbacks) = cx.try_global::<workspace::PaneSearchBarCallbacks>() {
            (callbacks.setup_search_bar)(&toolbar, window, cx);
        }

        pane
    });

    cx.subscribe_in(&pane, window, TerminalPanel::handle_pane_event).detach();
    cx.observe(&pane, |_, _, cx| cx.notify()).detach();

    pane
}

async fn wait_for_terminals_tasks(
    terminals_for_task: Vec<(usize, Entity<Pane>, Entity<TerminalView>)>,
    cx: &mut AsyncApp,
) {
    let pending_tasks = terminals_for_task.iter().map(|(_, _, terminal)| {
        terminal.update(cx, |terminal_view, cx| {
            terminal_view.terminal().update(cx, |terminal, cx| terminal.wait_for_completed_task(cx))
        })
    });
    join_all(pending_tasks).await;
}

pub(crate) struct FailedToSpawnTerminal {
    pub(crate) error: String,
    pub(crate) focus_handle: FocusHandle,
}

impl Focusable for FailedToSpawnTerminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FailedToSpawnTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .p_4()
            .items_center()
            .justify_center()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .max_w_112()
                    .items_center()
                    .justify_center()
                    .text_center()
                    .child(Label::new("Failed to spawn terminal"))
                    .child(Label::new(self.error.to_string()).size(LabelSize::Small).color(Color::Muted).mb_4())
                    .child(
                        Label::new("Check the built-in terminal shell settings.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
    }
}

impl EventEmitter<()> for FailedToSpawnTerminal {}

impl workspace::Item for FailedToSpawnTerminal {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        SharedString::new_static("Failed to spawn terminal")
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let registrar = cx
            .try_global::<workspace::PaneSearchBarCallbacks>()
            .map(|callbacks| (callbacks.wrap_div_with_search_actions)(div(), self.active_pane.clone()))
            .unwrap_or_else(div);
        let no_items_in_panes = self.center.panes().into_iter().all(|pane| pane.read(cx).items_len() == 0);
        let starting_placeholder = (self.pending_terminals_to_add > 0 && no_items_in_panes).then(|| {
            h_flex()
                .absolute()
                .inset_0()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::ArrowCircle).color(Color::Muted).size(IconSize::Small).with_rotate_animation(2),
                )
                .child(Label::new("Starting terminal…").color(Color::Muted))
        });
        self.workspace
            .update(cx, |workspace, cx| {
                registrar
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .relative()
                    .map(|this| match self.zoomed_pane(cx) {
                        Some(pane) => this.child(pane.clone()),
                        None => this.child(self.center.render(
                            workspace.zoomed_item(),
                            None,
                            &workspace::PaneRenderContext {
                                active_pane: &self.active_pane,
                                workspace: &workspace.weak_handle(),
                            },
                            window,
                            cx,
                        )),
                    })
                    .children(starting_placeholder)
            })
            .ok()
            .map(|div| {
                div.on_action({
                    cx.listener(|terminal_panel, _: &ActivatePaneLeft, window, cx| {
                        terminal_panel.activate_pane_in_direction(SplitDirection::Left, window, cx);
                    })
                })
                .on_action({
                    cx.listener(|terminal_panel, _: &ActivatePaneRight, window, cx| {
                        terminal_panel.activate_pane_in_direction(SplitDirection::Right, window, cx);
                    })
                })
                .on_action({
                    cx.listener(|terminal_panel, _: &ActivatePaneUp, window, cx| {
                        terminal_panel.activate_pane_in_direction(SplitDirection::Up, window, cx);
                    })
                })
                .on_action({
                    cx.listener(|terminal_panel, _: &ActivatePaneDown, window, cx| {
                        terminal_panel.activate_pane_in_direction(SplitDirection::Down, window, cx);
                    })
                })
                .on_action(cx.listener(|terminal_panel, _action: &ActivateNextPane, window, cx| {
                    let panes = terminal_panel.center.panes();
                    if let Some(ix) = panes.iter().position(|pane| **pane == terminal_panel.active_pane) {
                        let next_ix = (ix + 1) % panes.len();
                        window.focus(&panes[next_ix].focus_handle(cx), cx);
                    }
                }))
                .on_action(cx.listener(|terminal_panel, _action: &ActivatePreviousPane, window, cx| {
                    let panes = terminal_panel.center.panes();
                    if let Some(ix) = panes.iter().position(|pane| **pane == terminal_panel.active_pane) {
                        let prev_ix = cmp::min(ix.wrapping_sub(1), panes.len() - 1);
                        window.focus(&panes[prev_ix].focus_handle(cx), cx);
                    }
                }))
                .on_action(cx.listener(|terminal_panel, action: &ActivatePane, window, cx| {
                    let panes = terminal_panel.center.panes();
                    if let Some(&pane) = panes.get(action.0) {
                        window.focus(&pane.read(cx).focus_handle(cx), cx);
                    } else {
                        let future = terminal_panel.new_pane_with_active_terminal(window, cx);
                        cx.spawn_in(window, async move |terminal_panel, cx| {
                            if let Some(new_pane) = future.await {
                                _ = terminal_panel.update_in(cx, |terminal_panel, window, cx| {
                                    terminal_panel.center.split(
                                        &terminal_panel.active_pane,
                                        &new_pane,
                                        SplitDirection::Right,
                                        cx,
                                    );
                                    let new_pane = new_pane.read(cx);
                                    window.focus(&new_pane.focus_handle(cx), cx);
                                });
                            }
                        })
                        .detach();
                    }
                }))
                .on_action(cx.listener(|terminal_panel, _: &SwapPaneLeft, _, cx| {
                    terminal_panel.swap_pane_in_direction(SplitDirection::Left, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &SwapPaneRight, _, cx| {
                    terminal_panel.swap_pane_in_direction(SplitDirection::Right, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &SwapPaneUp, _, cx| {
                    terminal_panel.swap_pane_in_direction(SplitDirection::Up, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &SwapPaneDown, _, cx| {
                    terminal_panel.swap_pane_in_direction(SplitDirection::Down, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &MovePaneLeft, _, cx| {
                    terminal_panel.move_pane_to_border(SplitDirection::Left, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &MovePaneRight, _, cx| {
                    terminal_panel.move_pane_to_border(SplitDirection::Right, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &MovePaneUp, _, cx| {
                    terminal_panel.move_pane_to_border(SplitDirection::Up, cx);
                }))
                .on_action(cx.listener(|terminal_panel, _: &MovePaneDown, _, cx| {
                    terminal_panel.move_pane_to_border(SplitDirection::Down, cx);
                }))
                .on_action(cx.listener(|terminal_panel, action: &MoveItemToPane, window, cx| {
                    let Some(&target_pane) = terminal_panel.center.panes().get(action.destination) else {
                        return;
                    };
                    move_active_item(&terminal_panel.active_pane, target_pane, action.focus, true, window, cx);
                }))
                .on_action(cx.listener(
                    |terminal_panel, action: &MoveItemToPaneInDirection, window, cx| {
                        let source_pane = &terminal_panel.active_pane;
                        if let Some(destination_pane) =
                            terminal_panel.center.find_pane_in_direction(source_pane, action.direction, cx)
                        {
                            move_active_item(source_pane, destination_pane, action.focus, true, window, cx);
                        };
                    },
                ))
            })
            .unwrap_or_else(div)
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TerminalPanel {
    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.active_pane.focus_handle(cx)
    }

    fn position(&self, _: &Window, _: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        TerminalSettings::get().default_height
    }

    fn is_zoomed(&self, _window: &Window, cx: &App) -> bool {
        self.active_pane.read(cx).is_zoomed()
    }

    fn set_zoomed(&mut self, zoomed: bool, _: &mut Window, cx: &mut Context<Self>) {
        for pane in self.center.panes() {
            pane.update(cx, |pane, cx| {
                pane.set_zoomed(zoomed, cx);
            })
        }
        cx.notify();
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        let old_active = self.active;
        self.active = active;
        if !active || old_active == active || !self.has_no_terminals(cx) {
            return;
        }
        cx.defer_in(window, |this, window, cx| {
            let Ok(kind) = this.workspace.update(cx, |workspace, cx| default_working_directory(workspace, cx)) else {
                return;
            };

            this.add_terminal_shell(false, kind, RevealStrategy::Always, window, cx).detach_and_log_err(cx)
        })
    }

    fn persistent_name() -> &'static str {
        "TerminalPanel"
    }

    fn panel_key() -> &'static str {
        TERMINAL_PANEL_KEY
    }

    fn icon(&self, _window: &Window, cx: &App) -> Option<IconName> {
        if (self.is_enabled(cx) || !self.has_no_terminals(cx)) && TerminalSettings::get().button {
            Some(IconName::TerminalAlt)
        } else {
            None
        }
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Terminal Panel")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(Toggle)
    }

    fn pane(&self) -> Option<Entity<Pane>> {
        Some(self.active_pane.clone())
    }

    fn activation_priority(&self) -> u32 {
        2
    }
}

struct TerminalProvider(Entity<TerminalPanel>);

impl workspace::TerminalProvider for TerminalProvider {
    fn spawn(&self, task: SpawnInTerminal, window: &mut Window, cx: &mut App) -> Task<Option<Result<ExitStatus>>> {
        let terminal_panel = self.0.clone();
        window.spawn(cx, async move |cx| {
            let terminal = terminal_panel
                .update_in(cx, |terminal_panel, window, cx| terminal_panel.spawn_task(&task, window, cx))
                .ok()?
                .await;
            match terminal {
                Ok(terminal) => {
                    let exit_status =
                        terminal.read_with(cx, |terminal, cx| terminal.wait_for_completed_task(cx)).ok()?.await?;
                    Some(Ok(exit_status))
                }
                Err(e) => Some(Err(e)),
            }
        })
    }
}

/// The shell terminals among a terminal pane's items, for `ToggleShell`.
struct PaneShells {
    active_item: Option<usize>,
    /// Per item: whether it is a shell terminal, not task output or another item.
    shells: Vec<bool>,
}

#[derive(Debug, PartialEq, Eq)]
enum ShellToggleTarget {
    Close,
    Show { pane: usize, item: usize },
    New,
}

/// What `ToggleShell` does: close the panel when a shell is in front,
/// otherwise bring a shell to the front, preferring the active pane's active
/// item, then the active pane, then the other panes, else open a new shell.
fn shell_toggle_target(visible: bool, active_pane: usize, panes: &[PaneShells]) -> ShellToggleTarget {
    let is_shell = |pane: usize, item: usize| panes.get(pane).is_some_and(|p| p.shells.get(item) == Some(&true));
    if let Some(item) = panes.get(active_pane).and_then(|pane| pane.active_item)
        && is_shell(active_pane, item)
    {
        return if visible { ShellToggleTarget::Close } else { ShellToggleTarget::Show { pane: active_pane, item } };
    }
    iter::once(active_pane)
        .chain((0..panes.len()).filter(|pane| *pane != active_pane))
        .find_map(|pane| {
            let item = panes.get(pane)?.shells.iter().position(|shell| *shell)?;
            Some(ShellToggleTarget::Show { pane, item })
        })
        .unwrap_or(ShellToggleTarget::New)
}

#[cfg(test)]
mod tests {
    use super::PaneShells;
    use super::ShellToggleTarget;
    use super::shell_toggle_target;

    fn pane(active_item: Option<usize>, shells: &[bool]) -> PaneShells {
        PaneShells { active_item, shells: shells.to_vec() }
    }

    #[test]
    fn test_shell_toggle_target() {
        // A shell in front: close when visible, just show when hidden.
        assert_eq!(shell_toggle_target(true, 0, &[pane(Some(0), &[true])]), ShellToggleTarget::Close);
        assert_eq!(
            shell_toggle_target(false, 0, &[pane(Some(0), &[true])]),
            ShellToggleTarget::Show { pane: 0, item: 0 }
        );
        // Task output in front: switch to a shell in the same pane, visible or not.
        assert_eq!(
            shell_toggle_target(true, 0, &[pane(Some(1), &[true, false])]),
            ShellToggleTarget::Show { pane: 0, item: 0 }
        );
        assert_eq!(
            shell_toggle_target(false, 0, &[pane(Some(1), &[true, false])]),
            ShellToggleTarget::Show { pane: 0, item: 0 }
        );
        // The active pane has no shell: look in the other panes.
        assert_eq!(
            shell_toggle_target(true, 1, &[pane(Some(0), &[false, true]), pane(Some(0), &[false])]),
            ShellToggleTarget::Show { pane: 0, item: 1 }
        );
        // No shell anywhere, or no items at all: open a new one.
        assert_eq!(shell_toggle_target(true, 0, &[pane(Some(0), &[false])]), ShellToggleTarget::New);
        assert_eq!(shell_toggle_target(false, 0, &[pane(None, &[])]), ShellToggleTarget::New);
        assert_eq!(shell_toggle_target(false, 0, &[]), ShellToggleTarget::New);
    }
}
