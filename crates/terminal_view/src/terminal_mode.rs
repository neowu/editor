//! Terminal mode's terminals: what the workspace's terminal mode needs from
//! this crate (pane setup, opening shells, reading working directories).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::Window;
use workspace::DraggedTab;
use workspace::ItemHandle;
use workspace::Pane;
use workspace::TerminalModeDelegate;
use workspace::Workspace;

use crate::TerminalView;
use crate::default_working_directory;
use crate::terminal_panel::FailedToSpawnTerminal;
use crate::terminal_panel::set_terminal_tab_bar_buttons;

pub(crate) fn init(cx: &mut App) {
    workspace::set_terminal_mode_delegate(TerminalModeTerminals, cx);
}

struct TerminalModeTerminals;

/// A saved working directory that no longer exists falls back to the default.
/// Remote directories can't be checked from here, so they are kept.
fn existing_directory(directory: Option<PathBuf>, is_local: bool) -> Option<PathBuf> {
    directory.filter(|directory| !is_local || directory.is_dir())
}

impl TerminalModeDelegate for TerminalModeTerminals {
    fn setup_pane(&self, pane: &mut Pane, window: &mut Window, cx: &mut Context<Pane>) {
        pane.set_tab_style(ui::TabStyle::Pill);
        set_terminal_tab_bar_buttons(pane, true, cx);
        // Only terminal tabs split off into terminal panes.
        pane.set_can_split(Some(Arc::new(|pane, dragged_item, _window, cx| {
            let Some(tab) = dragged_item.downcast_ref::<DraggedTab>() else {
                return false;
            };
            let item = if tab.pane == cx.entity() {
                pane.item_for_index(tab.ix)
            } else {
                tab.pane.read(cx).item_for_index(tab.ix)
            };
            item.is_some_and(|item| item.downcast::<TerminalView>().is_some())
        })));
        let toolbar = pane.toolbar().clone();
        if let Some(callbacks) = cx.try_global::<workspace::PaneSearchBarCallbacks>() {
            (callbacks.setup_search_bar)(&toolbar, window, cx);
        }
    }

    fn open_shell(
        &self,
        workspace: &mut Workspace,
        pane: Entity<Pane>,
        working_directory: Option<PathBuf>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<()>> {
        let project = workspace.project().clone();
        let is_local = project.read(cx).is_local();
        let working_directory =
            existing_directory(working_directory, is_local).or_else(|| default_working_directory(workspace, cx));
        let create_terminal = project.update(cx, |project, cx| project.create_terminal_shell(working_directory, cx));
        cx.spawn_in(window, async move |workspace, cx| {
            let terminal = create_terminal.await;
            workspace.update_in(cx, |workspace, window, cx| {
                // Don't steal focus from an open modal (e.g. the command palette).
                let focus = focus && !workspace.has_active_modal(window, cx);
                let item: Box<dyn ItemHandle> =
                    match &terminal {
                        Ok(terminal) => Box::new(cx.new(|cx| {
                            TerminalView::new(
                                terminal.clone(),
                                workspace.weak_handle(),
                                workspace.project().downgrade(),
                                window,
                                cx,
                            )
                        })),
                        Err(error) => Box::new(cx.new(|cx| FailedToSpawnTerminal {
                            error: error.to_string(),
                            focus_handle: cx.focus_handle(),
                        })),
                    };
                pane.update(cx, |pane, cx| pane.add_item(item, true, focus, None, window, cx));
            })?;
            terminal.map(|_| ())
        })
    }

    fn working_directory(&self, item: &dyn ItemHandle, cx: &App) -> Option<PathBuf> {
        item.downcast::<TerminalView>()?.read(cx).terminal().read(cx).working_directory()
    }

    fn is_terminal(&self, item: &dyn ItemHandle, _cx: &App) -> bool {
        item.downcast::<TerminalView>().is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::existing_directory;

    #[test]
    fn test_existing_directory() {
        let temp_dir = std::env::temp_dir();
        assert_eq!(existing_directory(Some(temp_dir.clone()), true), Some(temp_dir));
        let missing = PathBuf::from("/definitely/not/a/real/dir");
        assert_eq!(existing_directory(Some(missing.clone()), true), None);
        assert_eq!(existing_directory(Some(missing.clone()), false), Some(missing));
        assert_eq!(existing_directory(None, true), None);
    }
}
