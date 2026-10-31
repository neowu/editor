mod preview;

use ed_actions::outline::ToggleOutline;
use editor::Editor;
use editor::EditorSettings;
use editor::SoftWrap;
use editor::actions::AddSelectionAbove;
use editor::actions::AddSelectionBelow;
use editor::actions::CodeActionSource;
use editor::actions::DuplicateLineDown;
use editor::actions::GoToDiagnostic;
use editor::actions::GoToHunk;
use editor::actions::GoToPreviousDiagnostic;
use editor::actions::GoToPreviousHunk;
use editor::actions::MoveLineDown;
use editor::actions::MoveLineUp;
use editor::actions::SelectAll;
use editor::actions::SelectLargerSyntaxNode;
use editor::actions::SelectNext;
use editor::actions::SelectSmallerSyntaxNode;
use editor::actions::ToggleCodeActions;
use editor::actions::ToggleDiagnostics;
use editor::actions::ToggleGoToLine;
use editor::actions::ToggleSoftWrap;
use editor::code_context_menus::CodeContextMenu;
use editor::code_context_menus::ContextMenuOrigin;
use gpui::Action;
use gpui::Anchor;
use gpui::AnchoredPositionMode;
use gpui::ClickEvent;
use gpui::Context;
use gpui::ElementId;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::InteractiveElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Styled;
use gpui::WeakEntity;
use gpui::Window;
use gpui::anchored;
use gpui::deferred;
use gpui::point;
use language::DiskState;
use project::project_settings::DiagnosticSeverity;
use search::BufferSearchBar;
use search::buffer_search;
use settings::Settings;
use settings::SettingsStore;
use ui::ButtonStyle;
use ui::ContextMenu;
use ui::IconButton;
use ui::IconName;
use ui::IconSize;
use ui::PopoverMenu;
use ui::PopoverMenuHandle;
use ui::Tooltip;
use ui::prelude::*;
use workspace::ToolbarItemEvent;
use workspace::ToolbarItemLocation;
use workspace::ToolbarItemView;
use workspace::Workspace;
use workspace::item::ItemBufferKind;
use workspace::item::ItemHandle;

const MAX_CODE_ACTION_MENU_LINES: u32 = 16;

pub struct QuickActionBar {
    active_item: Option<Box<dyn ItemHandle>>,
    buffer_search_bar: Entity<BufferSearchBar>,
    show: bool,
    toggle_selections_handle: PopoverMenuHandle<ContextMenu>,
    toggle_settings_handle: PopoverMenuHandle<ContextMenu>,
    workspace: WeakEntity<Workspace>,
}

impl QuickActionBar {
    pub fn new(buffer_search_bar: Entity<BufferSearchBar>, workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            active_item: None,
            buffer_search_bar,
            show: true,
            toggle_selections_handle: PopoverMenuHandle::default(),
            toggle_settings_handle: PopoverMenuHandle::default(),
            workspace: workspace.weak_handle(),
        };
        this.apply_settings(cx);
        cx.observe_global::<SettingsStore>(|this, cx| this.apply_settings(cx)).detach();
        this
    }

    fn active_editor(&self) -> Option<Entity<Editor>> {
        self.active_item.as_ref().and_then(|item| item.downcast::<Editor>())
    }

    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let new_show = EditorSettings::get_global(cx).toolbar.quick_actions;
        if new_show != self.show {
            self.show = new_show;
            cx.emit(ToolbarItemEvent::ChangeLocation(self.get_toolbar_item_location()));
        }
    }

    fn get_toolbar_item_location(&self) -> ToolbarItemLocation {
        if self.show && self.active_editor().is_some() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for QuickActionBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.active_editor() else {
            return div().id("empty quick action bar");
        };

        let editor_value = editor.read(cx);
        let is_full = editor_value.mode().is_full();
        let diagnostics_enabled =
            editor_value.diagnostics_enabled() && editor_value.diagnostics_max_severity != DiagnosticSeverity::Off;
        let show_git_blame_gutter = editor_value.show_git_blame_gutter();
        let is_historic = editor_value.buffer().read(cx).all_buffers_iter().all(|buffer| {
            buffer.read(cx).file().is_some_and(|file| matches!(file.disk_state(), DiskState::Historic { .. }))
        });
        let (show_diagnostics_toggle, show_git_blame_toggle) = editor_control_items(is_full, is_historic);

        let supports_minimap = editor_value.supports_minimap(cx);
        let minimap_enabled = supports_minimap && editor_value.minimap().is_some();
        let soft_wrap_enabled = !matches!(editor_value.soft_wrap_mode(cx), SoftWrap::None);
        let has_available_code_actions = editor_value.has_available_code_actions_for_selection();
        let code_action_enabled = editor_value.code_actions_enabled_for_toolbar(cx);
        let focus_handle = editor_value.focus_handle(cx);

        let search_button = (editor.buffer_kind(cx) == ItemBufferKind::Singleton).then(|| {
            QuickActionBarButton::new(
                "toggle buffer search",
                search::SEARCH_ICON,
                !self.buffer_search_bar.read(cx).is_dismissed(),
                Box::new(buffer_search::Deploy::find()),
                focus_handle.clone(),
                "Buffer Search",
                {
                    let buffer_search_bar = self.buffer_search_bar.clone();
                    move |_, window, cx| {
                        buffer_search_bar
                            .update(cx, |search_bar, cx| search_bar.toggle(&buffer_search::Deploy::find(), window, cx));
                    }
                },
            )
        });

        let code_actions_dropdown = code_action_enabled.then(|| {
            let is_deployed = {
                let menu_ref = editor.read(cx).context_menu().borrow();
                let code_action_menu =
                    menu_ref.as_ref().filter(|menu| matches!(menu, CodeContextMenu::CodeActions(..)));
                code_action_menu.as_ref().is_some_and(|menu| matches!(menu.origin(), ContextMenuOrigin::QuickActionBar))
            };
            let code_action_element = is_deployed
                .then(|| {
                    editor.update(cx, |editor, cx| editor.render_context_menu(MAX_CODE_ACTION_MENU_LINES, window, cx))
                })
                .flatten();
            v_flex()
                .child(
                    IconButton::new("toggle_code_actions_icon", IconName::BoltOutlined)
                        .icon_size(IconSize::Small)
                        .style(ButtonStyle::Subtle)
                        .disabled(!has_available_code_actions)
                        .toggle_state(is_deployed)
                        .when(!is_deployed, |this| {
                            this.when(has_available_code_actions, |this| {
                                this.tooltip(Tooltip::for_action_title("Code Actions", &ToggleCodeActions::default()))
                            })
                            .when(!has_available_code_actions, |this| {
                                this.tooltip(Tooltip::for_action_title(
                                    "No Code Actions Available",
                                    &ToggleCodeActions::default(),
                                ))
                            })
                        })
                        .on_click({
                            let editor = editor.clone();
                            move |_, window, cx| {
                                editor.update(cx, |editor, cx| {
                                    editor.toggle_code_actions(
                                        &ToggleCodeActions {
                                            deployed_from: Some(CodeActionSource::QuickActionBar),
                                            quick_launch: false,
                                        },
                                        window,
                                        cx,
                                    );
                                })
                            }
                        }),
                )
                .children(code_action_element.map(|menu| {
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .position(point(px(20.), px(20.)))
                            .anchor(Anchor::TopRight)
                            .child(menu),
                    )
                }))
        });

        let editor_selections_dropdown = {
            let multibuffer = editor.read(cx).buffer().read(cx);
            let has_diff_hunks = multibuffer.snapshot(cx).has_diff_hunks();
            let tracks_diff = multibuffer.all_buffers_iter().any(|buffer| multibuffer.diff_for(buffer.read(cx).remote_id()).is_some());
            let is_read_only = editor.read(cx).read_only(cx);
            let (show_problems, show_hunks) = navigation_items(diagnostics_enabled, is_historic, tracks_diff);

            let focus = editor.focus_handle(cx);

            PopoverMenu::new("editor-selections-dropdown")
                .trigger_with_tooltip(
                    IconButton::new("toggle_editor_selections_icon", IconName::CursorIBeam)
                        .icon_size(IconSize::Small)
                        .style(ButtonStyle::Subtle)
                        .toggle_state(self.toggle_selections_handle.is_deployed()),
                    Tooltip::text("Selection Controls"),
                )
                .with_handle(self.toggle_selections_handle.clone())
                .anchor(Anchor::TopRight)
                .menu(move |window, cx| {
                    let focus = focus.clone();
                    let menu = ContextMenu::build(window, cx, move |menu, _, _| {
                        menu.context(focus.clone())
                            .action("Select All", Box::new(SelectAll))
                            .action("Select Next Occurrence", Box::new(SelectNext { replace_newest: false }))
                            .action("Expand Selection", Box::new(SelectLargerSyntaxNode))
                            .action("Shrink Selection", Box::new(SelectSmallerSyntaxNode))
                            .action("Add Cursor Above", Box::new(AddSelectionAbove { skip_soft_wrap: true }))
                            .action("Add Cursor Below", Box::new(AddSelectionBelow { skip_soft_wrap: true }))
                            .separator()
                            .action("Go to Symbol", Box::new(ToggleOutline))
                            .action("Go to Line/Column", Box::new(ToggleGoToLine))
                            .when(show_problems, |menu| {
                                menu.separator()
                                    .action("Next Problem", Box::new(GoToDiagnostic::default()))
                                    .action("Previous Problem", Box::new(GoToPreviousDiagnostic::default()))
                            })
                            .when(show_hunks, |menu| {
                                menu.separator()
                                    .action_disabled_when(!has_diff_hunks, "Next Hunk", Box::new(GoToHunk))
                                    .action_disabled_when(!has_diff_hunks, "Previous Hunk", Box::new(GoToPreviousHunk))
                            })
                            .when(!is_read_only, |menu| {
                                menu.separator()
                                    .action("Move Line Up", Box::new(MoveLineUp))
                                    .action("Move Line Down", Box::new(MoveLineDown))
                                    .action("Duplicate Selection", Box::new(DuplicateLineDown))
                            })
                    });
                    Some(menu)
                })
        };

        let editor_focus_handle = editor.focus_handle(cx);
        let editor = editor.downgrade();
        let editor_settings_dropdown = {
            PopoverMenu::new("editor-settings")
                .trigger_with_tooltip(
                    IconButton::new("toggle_editor_settings_icon", IconName::Filter)
                        .icon_size(IconSize::Small)
                        .toggle_state(self.toggle_settings_handle.is_deployed()),
                    Tooltip::text("Editor Controls"),
                )
                .anchor(Anchor::TopRight)
                .with_handle(self.toggle_settings_handle.clone())
                .menu(move |window, cx| {
                    let menu = ContextMenu::build(window, cx, {
                        let focus_handle = editor_focus_handle.clone();
                        |mut menu, _, _| {
                            menu = menu.context(focus_handle);

                            if supports_minimap {
                                menu = menu.toggleable_entry(
                                    "Minimap",
                                    minimap_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleMinimap.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_minimap(&editor::actions::ToggleMinimap, window, cx);
                                                })
                                                .ok();
                                        }
                                    },
                                )
                            }

                            if show_diagnostics_toggle {
                                menu = menu.toggleable_entry(
                                    "Diagnostics",
                                    diagnostics_enabled,
                                    IconPosition::Start,
                                    Some(ToggleDiagnostics.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_diagnostics(&ToggleDiagnostics, window, cx);
                                                })
                                                .ok();
                                        }
                                    },
                                );
                            }

                            menu = menu.separator();

                            menu = menu.toggleable_entry(
                                "Soft Wrap",
                                soft_wrap_enabled,
                                IconPosition::Start,
                                Some(ToggleSoftWrap.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_soft_wrap(&ToggleSoftWrap, window, cx);
                                            })
                                            .ok();
                                    }
                                },
                            );

                            if show_git_blame_toggle {
                                menu = menu.separator();

                                menu = menu.toggleable_entry(
                                    "Git Blame",
                                    show_git_blame_gutter,
                                    IconPosition::Start,
                                    Some(git::Blame.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| editor.toggle_git_blame(&git::Blame, window, cx))
                                                .ok();
                                        }
                                    },
                                );
                            }

                            menu
                        }
                    });
                    Some(menu)
                })
        };

        h_flex()
            .id("quick action bar")
            .gap(DynamicSpacing::Base01.rems())
            .children(self.render_preview_button(cx))
            .children(search_button)
            .children(code_actions_dropdown)
            .child(editor_selections_dropdown)
            .child(editor_settings_dropdown)
    }
}

impl EventEmitter<ToolbarItemEvent> for QuickActionBar {}

#[derive(IntoElement)]
struct QuickActionBarButton {
    id: ElementId,
    icon: IconName,
    toggled: bool,
    action: Box<dyn Action>,
    focus_handle: FocusHandle,
    tooltip: SharedString,
    #[allow(clippy::type_complexity)]
    on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

impl QuickActionBarButton {
    fn new(
        id: impl Into<ElementId>,
        icon: IconName,
        toggled: bool,
        action: Box<dyn Action>,
        focus_handle: FocusHandle,
        tooltip: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            toggled,
            action,
            focus_handle,
            tooltip: tooltip.into(),
            on_click: Box::new(on_click),
        }
    }
}

impl RenderOnce for QuickActionBarButton {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let tooltip = self.tooltip.clone();
        let action = self.action.boxed_clone();

        IconButton::new(self.id.clone(), self.icon)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .toggle_state(self.toggled)
            .tooltip(move |_window, cx| Tooltip::for_action_in(tooltip.clone(), &*action, &self.focus_handle, cx))
            .on_click(move |event, window, cx| (self.on_click)(event, window, cx))
    }
}

impl ToolbarItemView for QuickActionBar {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_item = active_pane_item.map(ItemHandle::boxed_clone);
        self.get_toolbar_item_location()
    }
}

/// Which navigation groups the selection menu offers, as `(problems, hunks)`.
/// Problems need live diagnostics, which historic (git revision) snapshots never
/// get; hunks need a diff tracked for the editor's buffers.
const fn navigation_items(diagnostics_enabled: bool, is_historic: bool, tracks_diff: bool) -> (bool, bool) {
    (diagnostics_enabled && !is_historic, tracks_diff)
}

/// Which "Editor Controls" toggles apply, as `(diagnostics, git_blame)`.
/// Historic (git revision) snapshots get neither diagnostics nor blame.
const fn editor_control_items(is_full: bool, is_historic: bool) -> (bool, bool) {
    (is_full && !is_historic, !is_historic)
}

#[cfg(test)]
mod tests {
    use super::editor_control_items;
    use super::navigation_items;

    #[test]
    fn full_project_editor_offers_diagnostics_and_blame() {
        assert_eq!(editor_control_items(true, false), (true, true));
    }

    #[test]
    fn historic_revision_hides_diagnostics_and_blame() {
        assert_eq!(editor_control_items(true, true), (false, false));
    }

    #[test]
    fn non_full_editor_hides_diagnostics_only() {
        assert_eq!(editor_control_items(false, false), (false, true));
    }

    #[test]
    fn project_file_offers_problems_and_hunks() {
        assert_eq!(navigation_items(true, false, true), (true, true));
    }

    #[test]
    fn historic_revision_hides_problems_and_hunks() {
        assert_eq!(navigation_items(true, true, false), (false, false));
    }

    #[test]
    fn disabled_diagnostics_hide_problems() {
        assert_eq!(navigation_items(false, false, true), (false, true));
    }

    #[test]
    fn untracked_buffer_hides_hunks() {
        assert_eq!(navigation_items(true, false, false), (true, false));
    }
}
