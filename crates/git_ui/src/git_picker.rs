use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use actions::menu;
use git::Branch;
use git::CheckoutBranch;
use git::Switch;
use git::ViewStash;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::InteractiveElement as _;
use gpui::KeyContext;
use gpui::ModifiersChangedEvent;
use gpui::MouseButton;
use gpui::ParentElement as _;
use gpui::Rems;
use gpui::Render;
use gpui::Styled as _;
use gpui::Subscription;
use gpui::WeakEntity;
use gpui::Window;
use gpui::actions;
use gpui::rems;
use project::git_store::Repository;
use ui::FluentBuilder as _;
use ui::ToggleButtonGroup;
use ui::ToggleButtonGroupStyle;
use ui::ToggleButtonSimple;
use ui::Tooltip;
use ui::prelude::IntoElement;
use ui::prelude::LabelSize;
use ui::prelude::StyledExt as _;
use ui::prelude::h_flex;
use ui::prelude::v_flex;
use workspace::ModalView;
use workspace::Workspace;
use workspace::WorkspaceActions;
use workspace::pane;

use crate::branch_picker;
use crate::branch_picker::BranchList;
use crate::branch_picker::CycleBranchFilter;
use crate::branch_picker::DeleteBranch;
use crate::branch_picker::ForceDeleteBranch;
use crate::branch_picker::ShowAllBranches;
use crate::branch_picker::ShowLocalBranches;
use crate::branch_picker::ShowRemoteBranches;
use crate::stash_picker;
use crate::stash_picker::DropStashItem;
use crate::stash_picker::ShowStashItem;
use crate::stash_picker::StashList;

actions!(git_picker, [ActivateBranchesTab, ActivateStashTab]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GitPickerTab {
    Branches,
    Stashes,
}

impl Display for GitPickerTab {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            GitPickerTab::Branches => "Branches",
            GitPickerTab::Stashes => "Stashes",
        };
        write!(f, "{label}")
    }
}

pub(crate) struct GitPicker {
    tab: GitPickerTab,
    workspace: WeakEntity<Workspace>,
    repository: Option<Entity<Repository>>,
    width: Rems,
    branch_list: Option<Entity<BranchList>>,
    stash_list: Option<Entity<StashList>>,
    _subscriptions: Vec<Subscription>,
    popover_style: bool,
}

impl GitPicker {
    fn new(
        workspace: WeakEntity<Workspace>,
        repository: Option<Entity<Repository>>,
        initial_tab: GitPickerTab,
        width: Rems,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_internal(workspace, repository, initial_tab, width, false, window, cx)
    }

    fn new_internal(
        workspace: WeakEntity<Workspace>,
        repository: Option<Entity<Repository>>,
        initial_tab: GitPickerTab,
        width: Rems,
        popover_style: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            tab: initial_tab,
            workspace,
            repository,
            width,
            branch_list: None,
            stash_list: None,
            _subscriptions: Vec::new(),
            popover_style,
        };

        this.ensure_active_picker(window, cx);
        this
    }

    fn ensure_active_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            GitPickerTab::Branches => {
                self.ensure_branch_list(window, cx);
            }
            GitPickerTab::Stashes => {
                self.ensure_stash_list(window, cx);
            }
        }
    }

    fn ensure_branch_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<BranchList> {
        if self.branch_list.is_none() {
            let show_footer = !self.popover_style;
            let branch_list = cx.new(|cx| {
                branch_picker::create_embedded(
                    self.workspace.clone(),
                    self.repository.clone(),
                    self.width,
                    show_footer,
                    window,
                    cx,
                )
            });

            let subscription = cx.subscribe(&branch_list, |this, _, _: &DismissEvent, cx| {
                if this.tab == GitPickerTab::Branches {
                    cx.emit(DismissEvent);
                }
            });

            self._subscriptions.push(subscription);
            self.branch_list = Some(branch_list);
        }
        self.branch_list.clone().expect("branch list was just set")
    }

    fn ensure_stash_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<StashList> {
        if self.stash_list.is_none() {
            let show_footer = !self.popover_style;
            let stash_list = cx.new(|cx| {
                stash_picker::create_embedded(
                    self.repository.clone(),
                    self.workspace.clone(),
                    self.width,
                    show_footer,
                    window,
                    cx,
                )
            });

            let subscription = cx.subscribe(&stash_list, |this, _, _: &DismissEvent, cx| {
                if this.tab == GitPickerTab::Stashes {
                    cx.emit(DismissEvent);
                }
            });

            self._subscriptions.push(subscription);
            self.stash_list = Some(stash_list);
        }
        self.stash_list.clone().expect("stash list was just set")
    }

    fn activate_next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = match self.tab {
            GitPickerTab::Branches => GitPickerTab::Stashes,
            GitPickerTab::Stashes => GitPickerTab::Branches,
        };
        self.ensure_active_picker(window, cx);
        self.focus_active_picker(window, cx);
        cx.notify();
    }

    fn activate_previous_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = match self.tab {
            GitPickerTab::Branches => GitPickerTab::Stashes,
            GitPickerTab::Stashes => GitPickerTab::Branches,
        };
        self.ensure_active_picker(window, cx);
        self.focus_active_picker(window, cx);
        cx.notify();
    }

    fn focus_active_picker(&self, window: &mut Window, cx: &mut App) {
        match self.tab {
            GitPickerTab::Branches => {
                if let Some(branch_list) = &self.branch_list {
                    branch_list.focus_handle(cx).focus(window, cx);
                }
            }
            GitPickerTab::Stashes => {
                if let Some(stash_list) = &self.stash_list {
                    stash_list.focus_handle(cx).focus(window, cx);
                }
            }
        }
    }

    fn render_tab_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle(cx);
        let branches_focus_handle = focus_handle.clone();
        let stash_focus_handle = focus_handle;

        h_flex().p_2().pb_0p5().w_full().child(
            ToggleButtonGroup::single_row(
                "git-picker-tabs",
                [
                    ToggleButtonSimple::new(
                        GitPickerTab::Branches.to_string(),
                        cx.listener(|this, _, window, cx| {
                            this.tab = GitPickerTab::Branches;
                            this.ensure_active_picker(window, cx);
                            this.focus_active_picker(window, cx);
                            cx.notify();
                        }),
                    )
                    .tooltip(move |_, cx| {
                        Tooltip::for_action_in("Toggle Branch Picker", &ActivateBranchesTab, &branches_focus_handle, cx)
                    }),
                    ToggleButtonSimple::new(
                        GitPickerTab::Stashes.to_string(),
                        cx.listener(|this, _, window, cx| {
                            this.tab = GitPickerTab::Stashes;
                            this.ensure_active_picker(window, cx);
                            this.focus_active_picker(window, cx);
                            cx.notify();
                        }),
                    )
                    .tooltip(move |_, cx| {
                        Tooltip::for_action_in("Toggle Stash Picker", &ActivateStashTab, &stash_focus_handle, cx)
                    }),
                ],
            )
            .label_size(LabelSize::Default)
            .style(ToggleButtonGroupStyle::Outlined)
            .auto_width()
            .selected_index(match self.tab {
                GitPickerTab::Branches => 0,
                GitPickerTab::Stashes => 1,
            }),
        )
    }

    fn render_active_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.tab {
            GitPickerTab::Branches => {
                let branch_list = self.ensure_branch_list(window, cx);
                branch_list.into_any_element()
            }
            GitPickerTab::Stashes => {
                let stash_list = self.ensure_stash_list(window, cx);
                stash_list.into_any_element()
            }
        }
    }

    fn handle_modifiers_changed(&mut self, ev: &ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            GitPickerTab::Branches => {
                if let Some(branch_list) = &self.branch_list {
                    branch_list.update(cx, |list, cx| {
                        list.handle_modifiers_changed(ev, window, cx);
                    });
                }
            }
            GitPickerTab::Stashes => {
                if let Some(stash_list) = &self.stash_list {
                    stash_list.update(cx, |list, cx| {
                        list.handle_modifiers_changed(ev, window, cx);
                    });
                }
            }
        }
    }

    fn handle_delete_branch(&mut self, _: &DeleteBranch, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch_list) = &self.branch_list {
            branch_list.update(cx, |list, cx| {
                list.handle_delete(&DeleteBranch, window, cx);
            });
        }
    }

    fn handle_force_delete_branch(&mut self, _: &ForceDeleteBranch, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch_list) = &self.branch_list {
            branch_list.update(cx, |list, cx| {
                list.handle_force_delete(&ForceDeleteBranch, window, cx);
            });
        }
    }

    fn set_active_branch_filter(
        &self,
        branch_filter: branch_picker::BranchFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(branch_list) = &self.branch_list {
            branch_list.update(cx, |list, cx| {
                list.set_branch_filter(branch_filter, window, cx);
            });
        }
    }

    fn handle_show_all_branches(&mut self, _: &ShowAllBranches, window: &mut Window, cx: &mut Context<Self>) {
        self.set_active_branch_filter(branch_picker::BranchFilter::All, window, cx);
    }

    fn handle_show_local_branches(&mut self, _: &ShowLocalBranches, window: &mut Window, cx: &mut Context<Self>) {
        self.set_active_branch_filter(branch_picker::BranchFilter::Local, window, cx);
    }

    fn handle_show_remote_branches(&mut self, _: &ShowRemoteBranches, window: &mut Window, cx: &mut Context<Self>) {
        self.set_active_branch_filter(branch_picker::BranchFilter::Remote, window, cx);
    }

    fn handle_cycle_branch_filter(&mut self, _: &CycleBranchFilter, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(branch_list) = &self.branch_list {
            branch_list.update(cx, |list, cx| {
                list.cycle_branch_filter(window, cx);
            });
        }
    }

    fn handle_drop_stash(&mut self, _: &DropStashItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(stash_list) = &self.stash_list {
            stash_list.update(cx, |list, cx| {
                list.handle_drop_stash(&DropStashItem, window, cx);
            });
        }
    }

    fn handle_show_stash(&mut self, _: &ShowStashItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(stash_list) = &self.stash_list {
            stash_list.update(cx, |list, cx| {
                list.handle_show_stash(&ShowStashItem, window, cx);
            });
        }
    }
}

impl ModalView for GitPicker {}
impl EventEmitter<DismissEvent> for GitPicker {}

impl Focusable for GitPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.tab {
            GitPickerTab::Branches => {
                if let Some(branch_list) = &self.branch_list {
                    return branch_list.focus_handle(cx);
                }
            }
            GitPickerTab::Stashes => {
                if let Some(stash_list) = &self.stash_list {
                    return stash_list.focus_handle(cx);
                }
            }
        }
        cx.focus_handle()
    }
}

static GIT_PICKER_ACTIONS: std::sync::LazyLock<gpui::ActionTable<GitPicker>> = std::sync::LazyLock::new(|| {
    let mut t = gpui::ActionTable::<GitPicker>::default();
    t.add(|_, _: &menu::Cancel, _, cx| {
        cx.emit(DismissEvent);
    });
    t.add(|this, _: &pane::ActivateNextItem, window, cx| {
        this.activate_next_tab(window, cx);
    });
    t.add(|this, _: &pane::ActivatePreviousItem, window, cx| {
        this.activate_previous_tab(window, cx);
    });
    t.add(|this, _: &ActivateBranchesTab, window, cx| {
        this.tab = GitPickerTab::Branches;
        this.ensure_active_picker(window, cx);
        this.focus_active_picker(window, cx);
        cx.notify();
    });
    t.add(|this, _: &ActivateStashTab, window, cx| {
        this.tab = GitPickerTab::Stashes;
        this.ensure_active_picker(window, cx);
        this.focus_active_picker(window, cx);
        cx.notify();
    });
    let branches: fn(&GitPicker, &App) -> bool = |picker, _| picker.tab == GitPickerTab::Branches;
    let stashes: fn(&GitPicker, &App) -> bool = |picker, _| picker.tab == GitPickerTab::Stashes;
    t.add_if(branches, GitPicker::handle_delete_branch);
    t.add_if(branches, GitPicker::handle_force_delete_branch);
    t.add_if(branches, GitPicker::handle_show_all_branches);
    t.add_if(branches, GitPicker::handle_show_local_branches);
    t.add_if(branches, GitPicker::handle_show_remote_branches);
    t.add_if(branches, GitPicker::handle_cycle_branch_filter);
    t.add_if(stashes, GitPicker::handle_drop_stash);
    t.add_if(stashes, GitPicker::handle_show_stash);
    t
});

impl Render for GitPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .occlude()
            .w(self.width)
            .elevation_3()
            .overflow_hidden()
            .when(self.popover_style, |el| {
                el.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this
                        .branch_list
                        .as_ref()
                        .is_some_and(|branch_list| branch_list.read(cx).branch_filter_menu_open(cx))
                    {
                        return;
                    }
                    cx.emit(DismissEvent);
                }))
            })
            .key_context({
                let mut key_context = KeyContext::new_with_defaults();
                key_context.add("Pane");
                key_context.add("GitPicker");
                match self.tab {
                    GitPickerTab::Branches => key_context.add("GitBranchSelector"),
                    GitPickerTab::Stashes => key_context.add("StashList"),
                }
                key_context
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .action_table(&cx.entity(), &GIT_PICKER_ACTIONS)
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .child(self.render_tab_bar(cx))
            .child(self.render_active_picker(window, cx))
    }
}

fn open_with_tab(workspace: &mut Workspace, tab: GitPickerTab, window: &mut Window, cx: &mut Context<Workspace>) {
    let workspace_handle = workspace.weak_handle();
    let repository = workspace.project().read(cx).active_repository(cx);

    workspace.toggle_modal(window, cx, |window, cx| {
        GitPicker::new(workspace_handle, repository, tab, rems(34.), window, cx)
    });
}

pub(crate) fn popover(
    workspace: WeakEntity<Workspace>,
    repository: Option<Entity<Repository>>,
    initial_tab: GitPickerTab,
    width: Rems,
    window: &mut Window,
    cx: &mut App,
) -> Entity<GitPicker> {
    cx.new(|cx| {
        let picker = GitPicker::new_internal(workspace, repository, initial_tab, width, true, window, cx);
        picker.focus_handle(cx).focus(window, cx);
        picker
    })
}

pub(crate) fn register(workspace_actions: &mut WorkspaceActions) {
    workspace_actions.register_action(|workspace, _: &Branch, window, cx| {
        open_with_tab(workspace, GitPickerTab::Branches, window, cx);
    });
    workspace_actions.register_action(|workspace, _: &Switch, window, cx| {
        open_with_tab(workspace, GitPickerTab::Branches, window, cx);
    });
    workspace_actions.register_action(|workspace, _: &CheckoutBranch, window, cx| {
        open_with_tab(workspace, GitPickerTab::Branches, window, cx);
    });
    workspace_actions.register_action(|workspace, _: &ViewStash, window, cx| {
        open_with_tab(workspace, GitPickerTab::Stashes, window, cx);
    });
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;

    fn conditional<V: 'static>(table: &gpui::ActionTable<V>) -> Vec<TypeId> {
        let types = table.action_types().iter().zip(table.conditional());
        types.filter_map(|(action_type, conditional)| conditional.then_some(*action_type)).collect()
    }

    #[test]
    fn tab_specific_actions_are_conditional() {
        let conditional = conditional(&GIT_PICKER_ACTIONS);

        assert_eq!(conditional.len(), 8);
        assert!(conditional.contains(&TypeId::of::<DeleteBranch>()));
        assert!(conditional.contains(&TypeId::of::<DropStashItem>()));
        assert!(!conditional.contains(&TypeId::of::<ActivateStashTab>()));
        assert!(GIT_PICKER_ACTIONS.action_types().is_sorted_by(|a, b| a < b), "action handled twice");
    }
}
