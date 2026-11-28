use std::sync::LazyLock;

use gpui::Action;
use gpui::ActionTable;
use gpui::Context;
use gpui::Div;
use gpui::Entity;
use gpui::InteractiveElement as _;
use gpui::Window;
use workspace::Pane;
use workspace::WorkspaceActions;

use crate::BufferSearchBar;

/// Registrar inverts the dependency between search and its downstream user, allowing said downstream user to register search action without knowing exactly what those actions are.
pub(crate) trait SearchActionsRegistrar {
    fn register_handler<A: Action>(&mut self, callback: impl ActionExecutor<A>);
}

type SearchBarActionCallback<A> = fn(&mut BufferSearchBar, &A, &mut Window, &mut Context<BufferSearchBar>);

/// The search actions for a pane outside the workspace's active-pane handling (terminal panel,
/// terminal and git history modes), acting on the search bar in that pane's toolbar.
static PANE_ACTIONS: LazyLock<ActionTable<Pane>> = LazyLock::new(|| {
    let mut table = ActionTable::default();
    BufferSearchBar::register(&mut table);
    table
});

impl SearchActionsRegistrar for ActionTable<Pane> {
    fn register_handler<A: Action>(&mut self, callback: impl ActionExecutor<A>) {
        self.add(move |pane, action: &A, window, cx| {
            let search_bar = pane.toolbar().read(cx).item_of_type::<BufferSearchBar>();
            let handled = search_bar.is_some_and(|search_bar| {
                search_bar.update(cx, |search_bar, cx| callback.execute(search_bar, action, window, cx))
            });
            if handled {
                cx.notify();
            } else {
                cx.propagate();
            }
        });
    }
}

pub fn register_pane_search_actions(div: Div, pane: Entity<Pane>) -> Div {
    div.action_table(&pane, &PANE_ACTIONS)
}

/// Registers the actions on every workspace, acting on its active pane.
impl SearchActionsRegistrar for WorkspaceActions {
    fn register_handler<A: Action>(&mut self, callback: impl ActionExecutor<A>) {
        self.register_action(move |workspace, action: &A, window, cx| {
            if workspace.has_active_modal(window, cx) && !workspace.hide_modal(window, cx) {
                cx.propagate();
                return;
            }

            let pane = workspace.active_pane();
            let callback = callback.clone();
            pane.update(cx, |this, cx| {
                this.toolbar().update(cx, move |this, cx| {
                    if let Some(search_bar) = this.item_of_type::<BufferSearchBar>() {
                        let should_notify = search_bar
                            .update(cx, move |search_bar, cx| callback.execute(search_bar, action, window, cx));
                        if should_notify {
                            cx.notify();
                        } else {
                            cx.propagate();
                        }
                    }
                });
            });
        });
    }
}

type DidHandleAction = bool;
/// Potentially executes the underlying action if some preconditions are met (e.g. buffer search bar is visible).
pub(crate) trait ActionExecutor<A: Action>: 'static + Clone + Send + Sync {
    fn execute(
        &self,
        search_bar: &mut BufferSearchBar,
        action: &A,
        window: &mut Window,
        cx: &mut Context<BufferSearchBar>,
    ) -> DidHandleAction;
}

/// Run an action when the search bar has been dismissed from the panel.
pub(super) struct ForDismissed<A>(pub(super) SearchBarActionCallback<A>);
impl<A> Clone for ForDismissed<A> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<A: Action> ActionExecutor<A> for ForDismissed<A> {
    fn execute(
        &self,
        search_bar: &mut BufferSearchBar,
        action: &A,
        window: &mut Window,
        cx: &mut Context<BufferSearchBar>,
    ) -> DidHandleAction {
        if search_bar.is_dismissed() {
            self.0(search_bar, action, window, cx);
            true
        } else {
            false
        }
    }
}

/// Run an action when the search bar is deployed.
pub(super) struct ForDeployed<A>(pub(super) SearchBarActionCallback<A>);
impl<A> Clone for ForDeployed<A> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<A: Action> ActionExecutor<A> for ForDeployed<A> {
    fn execute(
        &self,
        search_bar: &mut BufferSearchBar,
        action: &A,
        window: &mut Window,
        cx: &mut Context<BufferSearchBar>,
    ) -> DidHandleAction {
        if search_bar.is_dismissed() || search_bar.active_searchable_item.is_none() {
            false
        } else {
            self.0(search_bar, action, window, cx);
            true
        }
    }
}

/// Run an action when the search bar has any matches or a pending external query,
/// regardless of whether it is visible or not.
pub(super) struct WithResultsOrExternalQuery<A>(pub(super) SearchBarActionCallback<A>);
impl<A> Clone for WithResultsOrExternalQuery<A> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<A: Action> ActionExecutor<A> for WithResultsOrExternalQuery<A> {
    fn execute(
        &self,
        search_bar: &mut BufferSearchBar,
        action: &A,
        window: &mut Window,
        cx: &mut Context<BufferSearchBar>,
    ) -> DidHandleAction {
        let has_external_query = search_bar.pending_external_query.is_some();

        if has_external_query || search_bar.active_match_index.is_some() {
            self.0(search_bar, action, window, cx);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;

    #[test]
    fn pane_actions_cover_every_registered_search_action() {
        struct Count(usize);
        impl SearchActionsRegistrar for Count {
            fn register_handler<A: Action>(&mut self, _: impl ActionExecutor<A>) {
                self.0 += 1;
            }
        }
        let mut count = Count(0);
        BufferSearchBar::register(&mut count);

        assert_eq!(PANE_ACTIONS.action_types().len(), count.0);
        assert!(PANE_ACTIONS.action_types().contains(&TypeId::of::<crate::buffer_search::Deploy>()));
        assert!(PANE_ACTIONS.conditional().all(|conditional| !conditional));
    }
}
