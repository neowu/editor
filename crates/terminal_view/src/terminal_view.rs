use actions::menu;
use workspace::item::ItemBufferKind;
mod terminal_element;
mod terminal_mode;
pub mod terminal_panel;
mod terminal_path_like_target;
mod terminal_scrollbar;

use std::any::Any;
use std::cmp;
use std::ops::Range as StdRange;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use editor::Editor;
use editor::actions::SelectAll;
use editor::blink_manager::BlinkManager;
use gpui::Action;
use gpui::AnyElement;
use gpui::App;
use gpui::AppContext as _;
use gpui::ClipboardEntry;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::KeyContext;
use gpui::KeyDownEvent;
use gpui::Keystroke;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::Pixels;
use gpui::Point as GpuiPoint;
use gpui::Render;
use gpui::ScrollWheelEvent;
use gpui::Styled as _;
use gpui::Subscription;
use gpui::Task;
use gpui::WeakEntity;
use gpui::actions;
use gpui::anchored;
use gpui::deferred;
use gpui::div;
use project::Project;
use project::ProjectEntryId;
use project::search::SearchQuery;
use task::SpawnInTerminal;
use terminal::Clear;
use terminal::Copy;
use terminal::Event;
use terminal::HoveredWord;
use terminal::MaybeNavigationTarget;
use terminal::Modes;
use terminal::Paste;
use terminal::PasteText;
use terminal::Range;
use terminal::ScrollLineDown;
use terminal::ScrollLineUp;
use terminal::ScrollPageDown;
use terminal::ScrollPageUp;
use terminal::ScrollToBottom;
use terminal::ScrollToTop;
use terminal::Search;
use terminal::ShowCharacterPalette;
use terminal::TaskState;
use terminal::TaskStatus;
use terminal::Terminal;
use terminal::TerminalBounds;
use terminal::ToggleViMode;
use terminal::terminal_settings::CursorShape;
use terminal::terminal_settings::TerminalSettings;
use terminal_element::TerminalElement;
use terminal_panel::TerminalPanel;
use terminal_path_like_target::hover_path_like_target;
use terminal_path_like_target::open_path_like_target;
use terminal_scrollbar::TerminalScrollHandle;
use ui::ContextMenu;
use ui::Divider;
use ui::ScrollAxes;
use ui::Scrollbars;
use ui::Tooltip;
use ui::WithScrollbar as _;
use ui::prelude::ButtonCommon as _;
use ui::prelude::ButtonSize;
use ui::prelude::Clickable as _;
use ui::prelude::Color;
use ui::prelude::Context;
use ui::prelude::Element as _;
use ui::prelude::FluentBuilder as _;
use ui::prelude::Icon;
use ui::prelude::IconButton;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::InteractiveElement as _;
use ui::prelude::IntoElement;
use ui::prelude::Label;
use ui::prelude::LabelCommon as _;
use ui::prelude::LabelSize;
use ui::prelude::ParentElement as _;
use ui::prelude::SharedString;
use ui::prelude::VisibleOnHover as _;
use ui::prelude::Window;
use ui::prelude::h_flex;
use ui::prelude::v_flex;
use util::ResultExt as _;
use workspace::CloseActiveItem;
use workspace::DraggedSelection;
use workspace::DraggedTab;
use workspace::NewTerminal;
use workspace::Pane;
use workspace::Workspace;
use workspace::WorkspaceActions;
use workspace::item::Item;
use workspace::item::ItemEvent;
use workspace::item::TabContentParams;
use workspace::item::TabTooltipContent;
use workspace::searchable::Direction;
use workspace::searchable::SearchEvent;
use workspace::searchable::SearchOptions;
use workspace::searchable::SearchToken;
use workspace::searchable::SearchableItem;
use workspace::searchable::SearchableItemHandle;
use workspace::searchable::SeedQuery;

struct ImeState {
    marked_text: String,
}

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// Sends the specified text directly to the terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Action)]
#[action(namespace = terminal, no_build)]
pub struct SendText(pub String);

/// Sends a keystroke sequence to the terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Action)]
#[action(namespace = terminal, no_build)]
pub struct SendKeystroke(pub String);

actions!(
    terminal,
    [
        /// Reruns this terminal's task in place.
        pub RerunTask,
    ]
);

/// Renames the terminal tab.
#[derive(Clone, Debug, Default, PartialEq, Eq, Action)]
#[action(namespace = terminal)]
struct RenameTerminal;

pub fn init(workspace_actions: &mut WorkspaceActions, cx: &mut App) {
    terminal_panel::init(workspace_actions);
    terminal_mode::init(cx);
}

///A terminal view, maintains the PTY's file handles and communicates with the terminal.
pub(crate) struct TerminalView {
    terminal: Entity<Terminal>,
    workspace: WeakEntity<Workspace>,
    project: WeakEntity<Project>,
    focus_handle: FocusHandle,
    //Currently using iTerm bell, show bell emoji in tab until input is received
    has_bell: bool,
    context_menu: Option<(Entity<ContextMenu>, GpuiPoint<Pixels>, Subscription)>,
    cursor_shape: CursorShape,
    blink_manager: Entity<BlinkManager>,
    custom_title: Option<String>,
    hover: Option<HoverTarget>,
    hover_tooltip_update: Task<()>,
    scroll_handle: TerminalScrollHandle,
    ime_state: Option<ImeState>,
    self_handle: WeakEntity<Self>,
    rename_editor: Option<Entity<Editor>>,
    rename_editor_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
    _terminal_subscriptions: Vec<Subscription>,
}

#[derive(Debug)]
struct HoverTarget {
    tooltip: String,
    hovered_word: HoveredWord,
}

impl EventEmitter<Event> for TerminalView {}
impl EventEmitter<ItemEvent> for TerminalView {}

impl EventEmitter<SearchEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    fn new(
        terminal: Entity<Terminal>,
        workspace: WeakEntity<Workspace>,
        project: WeakEntity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace_handle = workspace.clone();
        let terminal_subscriptions = subscribe_for_terminal_events(&terminal, workspace, window, cx);

        let focus_handle = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus_handle, window, |terminal_view, window, cx| {
            terminal_view.focus_in(window, cx);
        });
        let focus_out = cx.on_focus_out(&focus_handle, window, |terminal_view, _event, window, cx| {
            terminal_view.focus_out(window, cx);
        });
        let cursor_shape = TerminalSettings::get().cursor_shape;

        let scroll_handle = TerminalScrollHandle::new(terminal.read(cx));

        let blink_manager = cx.new(|_| BlinkManager::new(CURSOR_BLINK_INTERVAL, true));

        let subscriptions = vec![focus_in, focus_out, cx.observe(&blink_manager, |_, _, cx| cx.notify())];

        Self {
            terminal,
            workspace: workspace_handle,
            project,
            has_bell: false,
            focus_handle,
            context_menu: None,
            cursor_shape,
            blink_manager,
            hover: None,
            hover_tooltip_update: Task::ready(()),
            scroll_handle,
            custom_title: None,
            ime_state: None,
            self_handle: cx.entity().downgrade(),
            rename_editor: None,
            rename_editor_subscription: None,
            _subscriptions: subscriptions,
            _terminal_subscriptions: terminal_subscriptions,
        }
    }

    /// Sets the marked (pre-edit) text from the IME.
    pub(crate) fn set_marked_text(&mut self, text: String, cx: &mut Context<Self>) {
        if text.is_empty() {
            return self.clear_marked_text(cx);
        }
        self.ime_state = Some(ImeState { marked_text: text });
        cx.notify();
    }

    /// Gets the current marked range (UTF-16).
    pub(crate) fn marked_text_range(&self) -> Option<StdRange<usize>> {
        self.ime_state.as_ref().map(|state| 0..state.marked_text.encode_utf16().count())
    }

    /// Clears the marked (pre-edit) text state.
    pub(crate) fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        if self.ime_state.is_some() {
            self.ime_state = None;
            cx.notify();
        }
    }

    /// Commits (sends) the given text to the PTY. Called by InputHandler::replace_text_in_range.
    pub(crate) fn commit_text(&self, text: &str, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.terminal.update(cx, |term, _| {
                term.input(text.to_owned().into_bytes());
            });
        }
    }

    pub(crate) fn terminal_bounds(&self, cx: &App) -> TerminalBounds {
        self.terminal.read(cx).last_content().terminal_bounds
    }

    const fn has_bell(&self) -> bool {
        self.has_bell
    }

    fn set_custom_title(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        let label = label.filter(|l| !l.trim().is_empty());
        if self.custom_title != label {
            self.custom_title = label;
            cx.emit(ItemEvent::UpdateTab);
            cx.notify();
        }
    }

    const fn is_renaming(&self) -> bool {
        self.rename_editor.is_some()
    }

    fn rename_editor_is_focused(&self, window: &Window, cx: &App) -> bool {
        self.rename_editor.as_ref().is_some_and(|editor| editor.focus_handle(cx).is_focused(window))
    }

    fn finish_renaming(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.rename_editor.take() else {
            return;
        };
        self.rename_editor_subscription = None;
        if save {
            let new_label = editor.read(cx).text(cx).trim().to_owned();
            let label = if new_label.is_empty() {
                None
            } else {
                // Only set custom_title if the text differs from the terminal's dynamic title.
                // This prevents subtle layout changes when clicking away without making changes.
                let terminal_title = self.terminal.read(cx).title(true);
                if new_label == terminal_title { None } else { Some(new_label) }
            };
            self.set_custom_title(label, cx);
        }
        cx.notify();
        self.focus_handle.focus(window, cx);
    }

    fn rename_terminal(&mut self, _: &RenameTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal.read(cx).task().is_some() {
            return;
        }

        let current_label = self.custom_title.clone().unwrap_or_else(|| self.terminal.read(cx).title(true));

        let rename_editor = cx.new(|cx| Editor::single_line(window, cx));
        let rename_editor_subscription = cx.subscribe_in(&rename_editor, window, {
            let rename_editor = rename_editor.clone();
            move |_this, _, event, window, cx| {
                if matches!(event, editor::EditorEvent::Blurred) {
                    // Defer to let focus settle (avoids canceling during double-click).
                    let rename_editor = rename_editor.clone();
                    cx.defer_in(window, move |this, window, cx| {
                        let still_current =
                            this.rename_editor.as_ref().is_some_and(|current| current == &rename_editor);
                        if still_current && !rename_editor.focus_handle(cx).is_focused(window) {
                            this.finish_renaming(false, window, cx);
                        }
                    });
                }
            }
        });

        self.rename_editor = Some(rename_editor.clone());
        self.rename_editor_subscription = Some(rename_editor_subscription);

        rename_editor.update(cx, |editor, cx| {
            editor.set_text(current_label, window, cx);
            editor.select_all(&SelectAll, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn clear_bell(&mut self, cx: &mut Context<TerminalView>) {
        self.has_bell = false;
        cx.emit(Event::Wakeup);
    }

    fn deploy_context_menu(
        &mut self,
        position: GpuiPoint<Pixels>,
        _has_selection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context_menu = ContextMenu::build(window, cx, |menu, _, _| {
            menu.context(self.focus_handle.clone())
                .action("New Terminal", Box::new(NewTerminal))
                .separator()
                .action("Copy", Box::new(Copy))
                .action("Paste", Box::new(Paste))
                .action("Paste Text", Box::new(PasteText))
                .action("Select All", Box::new(SelectAll))
                .action("Clear", Box::new(Clear))
                .separator()
                .action("Close Terminal Tab", Box::new(CloseActiveItem { save_intent: None, close_pinned: true }))
        });

        window.focus(&context_menu.focus_handle(cx), cx);
        let subscription = cx.subscribe_in(&context_menu, window, |this, _, _: &DismissEvent, window, cx| {
            if this
                .context_menu
                .as_ref()
                .is_some_and(|context_menu| context_menu.0.focus_handle(cx).contains_focused(window, cx))
            {
                cx.focus_self(window);
            }
            this.context_menu.take();
            cx.notify();
        });

        self.context_menu = Some((context_menu, position, subscription));
    }

    fn show_character_palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal.read(cx).last_content.mode.contains(Modes::ALT_SCREEN) {
            self.terminal.update(cx, |term, _| {
                term.try_keystroke(
                    &Keystroke::parse("ctrl-cmd-space").expect("static keystroke parses"),
                    TerminalSettings::get().option_as_meta,
                )
            });
        } else {
            window.show_character_palette();
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.select_all());
        cx.notify();
    }

    fn rerun_task(&mut self, _: &RerunTask, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.terminal.read(cx).task() {
            rerun_in_terminal(self.workspace.clone(), task.spawned_task.clone(), window, cx);
        }
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.clear());
        cx.notify();
    }

    fn scroll_wheel(&self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |term, _| term.scroll_wheel(event, TerminalSettings::get().scroll_multiplier.max(0.01)));
    }

    fn is_alt_screen(&self, cx: &App) -> bool {
        self.terminal.read(cx).last_content.mode.contains(Modes::ALT_SCREEN)
    }

    fn scroll_line_up(&mut self, _: &ScrollLineUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_line_up());
        cx.notify();
    }

    fn scroll_line_down(&mut self, _: &ScrollLineDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_line_down());
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_page_up());
        cx.notify();
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_page_down());
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_to_top());
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_to_bottom());
        cx.notify();
    }

    fn toggle_vi_mode(&mut self, _: &ToggleViMode, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.toggle_vi_mode());
        cx.notify();
    }

    fn should_show_cursor(&self, focused: bool, cx: &Context<Self>) -> bool {
        // Always show cursor when not focused or in special modes
        if !focused || self.terminal.read(cx).last_content.mode.contains(Modes::ALT_SCREEN) {
            return true;
        }

        self.blink_manager.read(cx).visible()
    }

    fn pause_cursor_blinking(&self, _window: &mut Window, cx: &mut Context<Self>) {
        self.blink_manager.update(cx, BlinkManager::pause_blinking);
    }

    const fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    ///Attempt to paste the clipboard into the terminal.
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.copy(None));
        cx.notify();
    }

    /// Specific handler for the [`editor::actions::Copy`] action in order for
    /// the `Edit > Copy` menu item to not be disabled, as the app expects a
    /// handler for this action in order to enable/disable the menu item.
    fn editor_copy(&mut self, _: &editor::actions::Copy, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
    }

    ///Attempt to paste the clipboard into the terminal.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        match clipboard.entries().first() {
            Some(ClipboardEntry::Image(image)) if !image.bytes.is_empty() => {
                self.forward_ctrl_v(cx);
            }
            Some(ClipboardEntry::ExternalPaths(paths)) => {
                self.add_paths_to_terminal(paths.paths(), window, cx);
            }
            _ => {
                if let Some(text) = clipboard.text() {
                    self.terminal.update(cx, |terminal, _cx| terminal.paste(&text));
                }
            }
        }
    }

    /// Specific handler for the [`editor::actions::Paste`] action in order for
    /// the `Edit > Paste` menu item to not be disabled, as the app expects a
    /// handler for this action in order to enable/disable the menu item.
    fn editor_paste(&mut self, _: &editor::actions::Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.paste(&Paste, window, cx);
    }

    ///Attempt to paste the clipboard text into the terminal.
    fn paste_text(&mut self, _: &PasteText, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        if let Some(text) = clipboard.text() {
            self.terminal.update(cx, |terminal, _cx| terminal.paste(&text));
        }
    }

    /// Emits a raw Ctrl+V so TUI agents can read the OS clipboard directly
    /// and attach images using their native workflows.
    fn forward_ctrl_v(&self, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| {
            term.input(vec![0x16]);
        });
    }

    fn add_paths_to_terminal(&self, paths: &[PathBuf], window: &mut Window, cx: &mut App) {
        let mut text = paths
            .iter()
            .filter_map(|path| Some(format!(" {}", shlex::try_quote(path.to_str()?).ok()?)))
            .collect::<String>();
        text.push(' ');
        window.focus(&self.focus_handle(cx), cx);
        self.terminal.update(cx, |terminal, _| {
            terminal.paste(&text);
        });
    }

    fn send_text(&mut self, text: &SendText, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_bell(cx);
        self.blink_manager.update(cx, BlinkManager::pause_blinking);
        self.terminal.update(cx, |term, _| {
            term.input(text.0.clone().into_bytes());
        });
    }

    fn send_keystroke(&mut self, text: &SendKeystroke, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(keystroke) = Keystroke::parse(&text.0).log_err() {
            self.clear_bell(cx);
            self.blink_manager.update(cx, BlinkManager::pause_blinking);
            self.process_keystroke(&keystroke, cx);
        }
    }

    fn dispatch_context(&self, cx: &App) -> KeyContext {
        let mut dispatch_context = KeyContext::new_with_defaults();
        dispatch_context.add("Terminal");

        if self.terminal.read(cx).vi_mode_enabled() {
            dispatch_context.add("vi_mode");
        }

        let mode = self.terminal.read(cx).last_content.mode;
        dispatch_context.set("screen", if mode.contains(Modes::ALT_SCREEN) { "alt" } else { "normal" });

        if mode.contains(Modes::APP_CURSOR) {
            dispatch_context.add("DECCKM");
        }
        if mode.contains(Modes::APP_KEYPAD) {
            dispatch_context.add("DECPAM");
        } else {
            dispatch_context.add("DECPNM");
        }
        if mode.contains(Modes::SHOW_CURSOR) {
            dispatch_context.add("DECTCEM");
        }
        if mode.contains(Modes::LINE_WRAP) {
            dispatch_context.add("DECAWM");
        }
        if mode.contains(Modes::ORIGIN) {
            dispatch_context.add("DECOM");
        }
        if mode.contains(Modes::INSERT) {
            dispatch_context.add("IRM");
        }
        //LNM is apparently the name for this. https://vt100.net/docs/vt510-rm/LNM.html
        if mode.contains(Modes::LINE_FEED_NEW_LINE) {
            dispatch_context.add("LNM");
        }
        if mode.contains(Modes::FOCUS_IN_OUT) {
            dispatch_context.add("report_focus");
        }
        if mode.contains(Modes::ALTERNATE_SCROLL) {
            dispatch_context.add("alternate_scroll");
        }
        if mode.contains(Modes::BRACKETED_PASTE) {
            dispatch_context.add("bracketed_paste");
        }
        if mode.intersects(Modes::MOUSE_MODE) {
            dispatch_context.add("any_mouse_reporting");
        }
        {
            let mouse_reporting = if mode.contains(Modes::MOUSE_REPORT_CLICK) {
                "click"
            } else if mode.contains(Modes::MOUSE_DRAG) {
                "drag"
            } else if mode.contains(Modes::MOUSE_MOTION) {
                "motion"
            } else {
                "off"
            };
            dispatch_context.set("mouse_reporting", mouse_reporting);
        }
        {
            let format = if mode.contains(Modes::SGR_MOUSE) {
                "sgr"
            } else if mode.contains(Modes::UTF8_MOUSE) {
                "utf8"
            } else {
                "normal"
            };
            dispatch_context.set("mouse_format", format);
        };

        if self.terminal.read(cx).last_content.selection.is_some() {
            dispatch_context.add("selection");
        }

        dispatch_context
    }

    fn set_terminal(&mut self, terminal: Entity<Terminal>, window: &Window, cx: &mut Context<TerminalView>) {
        self._terminal_subscriptions = subscribe_for_terminal_events(&terminal, self.workspace.clone(), window, cx);
        self.terminal = terminal;
    }

    fn rerun_button(&self, task: &TaskState) -> IconButton {
        let workspace = self.workspace.clone();
        let spawned_task = task.spawned_task.clone();
        IconButton::new("rerun-icon", IconName::Rerun)
            .icon_size(IconSize::Small)
            .size(ButtonSize::Compact)
            .icon_color(Color::Default)
            .shape(ui::IconButtonShape::Square)
            .tooltip(move |_window, cx| Tooltip::for_action("Rerun task", &RerunTask, cx))
            .on_click(move |_, window, cx| {
                rerun_in_terminal(workspace.clone(), spawned_task.clone(), window, cx);
            })
    }

    /// Attempts to process a keystroke in the terminal. Returns true if handled.
    ///
    /// In vi mode, explicitly triggers a re-render because vi navigation (like j/k)
    /// updates the cursor locally without sending data to the shell, so there's no
    /// shell output to automatically trigger a re-render.
    fn process_keystroke(&self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let (handled, vi_mode_enabled) = self.terminal.update(cx, |term, _| {
            (term.try_keystroke(keystroke, TerminalSettings::get().option_as_meta), term.vi_mode_enabled())
        });

        if handled && vi_mode_enabled {
            cx.notify();
        }

        handled
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_bell(cx);
        self.pause_cursor_blinking(window, cx);

        if event.prefer_character_input
            && event.keystroke.key_char.is_some()
            && !self.terminal.read(cx).vi_mode_enabled()
        {
            return;
        }

        if self.process_keystroke(&event.keystroke, cx) {
            cx.stop_propagation();
        }
    }

    fn focus_in(&self, window: &Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| {
            terminal.set_cursor_shape(self.cursor_shape);
            terminal.focus_in();
        });

        self.blink_manager.update(cx, BlinkManager::enable);

        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn focus_out(&self, _window: &mut Window, cx: &mut Context<Self>) {
        self.blink_manager.update(cx, BlinkManager::disable);
        self.terminal.update(cx, |terminal, _| {
            terminal.focus_out();
            terminal.set_cursor_shape(CursorShape::Hollow);
        });
        cx.notify();
    }
}

/// Spawns the terminal's task again, replacing the output in the same tab.
fn rerun_in_terminal(
    workspace: WeakEntity<Workspace>,
    mut spawned_task: SpawnInTerminal,
    window: &Window,
    cx: &mut App,
) {
    spawned_task.allow_concurrent_runs = true;
    // Deferred: spawning replaces this view's terminal, which must not happen mid-update.
    window.defer(cx, move |window, cx| {
        workspace.update(cx, |workspace, cx| workspace.schedule_task(spawned_task, window, cx)).ok();
    });
}

fn subscribe_for_terminal_events(
    terminal: &Entity<Terminal>,
    workspace: WeakEntity<Workspace>,
    window: &Window,
    cx: &mut Context<TerminalView>,
) -> Vec<Subscription> {
    let terminal_subscription = cx.observe(terminal, |_, _, cx| cx.notify());
    let terminal_events_subscription =
        cx.subscribe_in(terminal, window, move |terminal_view, terminal, event, window, cx| match event {
            Event::Wakeup => {
                cx.notify();
                window.invalidate_character_coordinates();
                cx.emit(Event::Wakeup);
                cx.emit(ItemEvent::UpdateTab);
                cx.emit(SearchEvent::MatchesInvalidated);
            }

            Event::OutputReplaced => {
                terminal_view.hover = None;
                terminal_view.hover_tooltip_update = Task::ready(());
                terminal_view.scroll_handle.future_display_offset.set(None);
                terminal_view.scroll_handle.update(terminal.read(cx));
                window.invalidate_character_coordinates();
                cx.emit(SearchEvent::MatchesInvalidated);
                cx.emit(SearchEvent::ActiveMatchChanged);
                cx.emit(ItemEvent::UpdateTab);
                cx.notify();
            }

            Event::Bell => {
                terminal_view.has_bell = true;
                cx.emit(Event::Wakeup);
            }

            Event::TitleChanged => {
                cx.emit(ItemEvent::UpdateTab);
            }

            Event::NewNavigationTarget(maybe_navigation_target) => {
                match maybe_navigation_target.as_ref().zip(terminal.read(cx).last_content.last_hovered_word.as_ref()) {
                    Some((MaybeNavigationTarget::Url(url), hovered_word)) => {
                        if Some(hovered_word) != terminal_view.hover.as_ref().map(|hover| &hover.hovered_word) {
                            terminal_view.hover =
                                Some(HoverTarget { tooltip: url.clone(), hovered_word: hovered_word.clone() });
                            terminal_view.hover_tooltip_update = Task::ready(());
                            cx.notify();
                        }
                    }
                    Some((MaybeNavigationTarget::PathLike(path_like_target), hovered_word)) => {
                        if Some(hovered_word) != terminal_view.hover.as_ref().map(|hover| &hover.hovered_word) {
                            terminal_view.hover = None;
                            terminal_view.hover_tooltip_update =
                                hover_path_like_target(&workspace, hovered_word.clone(), path_like_target, cx);
                            cx.notify();
                        }
                    }
                    None => {
                        terminal_view.hover = None;
                        terminal_view.hover_tooltip_update = Task::ready(());
                        cx.notify();
                    }
                }
            }

            Event::Open(maybe_navigation_target) => match maybe_navigation_target {
                MaybeNavigationTarget::Url(url) => cx.open_url(url),
                MaybeNavigationTarget::PathLike(path_like_target) => {
                    open_path_like_target(&workspace, terminal_view, path_like_target, window, cx);
                }
            },
            Event::CloseTerminal => cx.emit(ItemEvent::CloseItem),
            Event::SelectionsChanged => {
                window.invalidate_character_coordinates();
                cx.emit(SearchEvent::ActiveMatchChanged);
            }
        });
    vec![terminal_subscription, terminal_events_subscription]
}

fn regex_search_for_query(query: &SearchQuery) -> Option<Search> {
    let str = query.as_str();
    if query.is_regex() {
        if str == "." {
            return None;
        }
        Search::new(str)
    } else {
        Search::new(&regex::escape(str))
    }
}

/// On the tab itself, which is a dispatch node of its own outside the terminal's.
static TERMINAL_TAB_ACTIONS: std::sync::LazyLock<gpui::ActionTable<TerminalView>> = std::sync::LazyLock::new(|| {
    let mut t = gpui::ActionTable::<TerminalView>::default();
    t.add(TerminalView::rename_terminal);
    t
});

/// Around the tab's rename editor.
static TERMINAL_TAB_RENAME_ACTIONS: std::sync::LazyLock<gpui::ActionTable<TerminalView>> =
    std::sync::LazyLock::new(|| {
        let mut t = gpui::ActionTable::<TerminalView>::default();
        t.add(|this, _: &menu::Confirm, window, cx| this.finish_renaming(true, window, cx));
        t.add(|this, _: &menu::Cancel, window, cx| this.finish_renaming(false, window, cx));
        t
    });

static TERMINAL_VIEW_ACTIONS: std::sync::LazyLock<gpui::ActionTable<TerminalView>> = std::sync::LazyLock::new(|| {
    let mut t = gpui::ActionTable::<TerminalView>::default();
    t.add(TerminalView::send_text);
    t.add(TerminalView::send_keystroke);
    t.add(TerminalView::copy);
    t.add(TerminalView::editor_copy);
    t.add(TerminalView::paste);
    t.add(TerminalView::editor_paste);
    t.add(TerminalView::paste_text);
    t.add(TerminalView::clear);
    t.add(TerminalView::scroll_line_up);
    t.add(TerminalView::scroll_line_down);
    t.add(TerminalView::scroll_page_up);
    t.add(TerminalView::scroll_page_down);
    t.add(TerminalView::scroll_to_top);
    t.add(TerminalView::scroll_to_bottom);
    t.add(TerminalView::toggle_vi_mode);
    t.add(TerminalView::show_character_palette);
    t.add(TerminalView::select_all);
    t.add(TerminalView::rerun_task);
    t.add(TerminalView::rename_terminal);
    t
});

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // TODO: this should be moved out of render
        self.scroll_handle.update(self.terminal.read(cx));

        if let Some(new_display_offset) = self.scroll_handle.future_display_offset.take() {
            self.terminal.update(cx, |term, _| {
                let delta =
                    (new_display_offset as u32).cast_signed() - (term.last_content.display_offset as u32).cast_signed();
                match delta.cmp(&0) {
                    cmp::Ordering::Greater => term.scroll_up_by(delta as usize),
                    cmp::Ordering::Less => term.scroll_down_by(-delta as usize),
                    cmp::Ordering::Equal => {}
                }
            });
        }

        let terminal_handle = self.terminal.clone();
        let terminal_view_handle = cx.entity();

        let focused = self.focus_handle.is_focused(window);

        div()
            .id("terminal-view")
            .size_full()
            .relative()
            .track_focus(&self.focus_handle(cx))
            .key_context(self.dispatch_context(cx))
            .action_table(&cx.entity(), &TERMINAL_VIEW_ACTIONS)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if !this.terminal.read(cx).mouse_mode(event.modifiers.shift) {
                        let had_selection = this.terminal.read(cx).last_content.selection.is_some();
                        if !had_selection {
                            this.terminal.update(cx, |terminal, _| {
                                terminal.select_word_at_event_position(event);
                            });
                        }
                        let has_selection = !had_selection
                            || this
                                .terminal
                                .read(cx)
                                .last_content
                                .selection_text
                                .as_ref()
                                .is_some_and(|text| !text.is_empty());
                        this.deploy_context_menu(event.position, has_selection, window, cx);
                        cx.notify();
                    }
                }),
            )
            .child(
                // TODO: Oddly this wrapper div is needed for TerminalElement to not steal events from the context menu
                div()
                    .id("terminal-view-container")
                    .size_full()
                    .bg(theme::theme().colors().editor_background)
                    .child(div().size_full().bg(theme::theme().colors().terminal_background).child(
                        TerminalElement::new(
                            terminal_handle,
                            terminal_view_handle,
                            self.workspace.clone(),
                            self.focus_handle.clone(),
                            focused,
                            self.should_show_cursor(focused, cx),
                        ),
                    ))
                    .custom_scrollbars(
                        Scrollbars::new(ScrollAxes::Both)
                            .show_along(ScrollAxes::Vertical)
                            .with_track_along(ScrollAxes::Vertical, theme::theme().colors().terminal_background)
                            .tracked_scroll_handle(&self.scroll_handle),
                        window,
                        cx,
                    ),
            )
            .children(self.context_menu.as_ref().map(|(menu, position, _)| {
                deferred(anchored().position(*position).anchor(gpui::Anchor::TopLeft).child(menu.clone()))
                    .with_priority(1)
            }))
    }
}

impl Item for TerminalView {
    type Event = ItemEvent;

    fn tab_tooltip_content(&self, cx: &App) -> Option<TabTooltipContent> {
        Some(TabTooltipContent::Custom(Box::new(Tooltip::element({
            let terminal = self.terminal().read(cx);
            let title = terminal.title(false);
            let pid = terminal.pid_getter()?.fallback_pid();

            move |_, _| {
                v_flex()
                    .gap_1()
                    .child(Label::new(title.clone()))
                    .child(h_flex().flex_grow_1().child(Divider::horizontal()))
                    .child(Label::new(format!("Process ID (PID): {pid}")).color(Color::Muted).size(LabelSize::Small))
                    .into_any_element()
            }
        }))))
    }

    fn tab_content(&self, params: TabContentParams, _window: &Window, cx: &App) -> AnyElement {
        let terminal = self.terminal().read(cx);
        let title = self
            .custom_title
            .as_ref()
            .filter(|title| !title.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| terminal.title(true));

        let (icon, icon_color, rerun_button) = match terminal.task() {
            Some(terminal_task) => match &terminal_task.status {
                TaskStatus::Running => (IconName::PlayFilled, Color::Disabled, Some(self.rerun_button(terminal_task))),
                TaskStatus::Unknown => (IconName::Warning, Color::Warning, Some(self.rerun_button(terminal_task))),
                TaskStatus::Completed { success } => {
                    let rerun_button = Some(self.rerun_button(terminal_task));

                    if *success {
                        (IconName::Check, Color::Success, rerun_button)
                    } else {
                        (IconName::XCircle, Color::Error, rerun_button)
                    }
                }
            },
            None => (IconName::Terminal, params.icon_color(), None),
        };

        let view = self.self_handle.upgrade();
        h_flex()
            .gap_1()
            .group("term-tab-icon")
            .when(!params.selected, |this| this.track_focus(&self.focus_handle))
            .when_some(view.as_ref(), |this, view| this.action_table(view, &TERMINAL_TAB_ACTIONS))
            .child(
                h_flex()
                    .group("term-tab-icon")
                    .child(
                        div()
                            .when(rerun_button.is_some(), |this| this.hover(|style| style.invisible().w_0()))
                            .child(Icon::new(icon).color(icon_color)),
                    )
                    .when_some(rerun_button, |this, rerun_button| {
                        this.child(div().absolute().visible_on_hover("term-tab-icon").child(rerun_button))
                    }),
            )
            .child(
                div()
                    .relative()
                    .child(
                        Label::new(title)
                            .single_line()
                            .color(params.text_color())
                            .when(self.is_renaming(), |this| this.alpha(0.)),
                    )
                    .when_some(self.rename_editor.clone().zip(view), |this, (editor, view)| {
                        this.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .child(editor)
                                .action_table(&view, &TERMINAL_TAB_RENAME_ACTIONS),
                        )
                    }),
            )
            .into_any()
    }

    fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString {
        if let Some(custom_title) = self.custom_title.as_ref().filter(|l| !l.trim().is_empty()) {
            return custom_title.clone().into();
        }
        let terminal = self.terminal().read(cx);
        terminal.title(detail == 0).into()
    }

    fn handle_drop(&self, active_pane: &Pane, dropped: &dyn Any, window: &mut Window, cx: &mut App) -> bool {
        let Some(project) = self.project.upgrade() else {
            return false;
        };

        if let Some(tab) = dropped.downcast_ref::<DraggedTab>() {
            let Some(self_handle) = self.self_handle.upgrade() else {
                return false;
            };

            let Some(workspace) = self.workspace.upgrade() else {
                return false;
            };

            let Some(this_pane) = workspace.read(cx).pane_for(&self_handle) else {
                return false;
            };

            let item = if tab.pane == this_pane {
                active_pane.item_for_index(tab.ix)
            } else {
                tab.pane.read(cx).item_for_index(tab.ix)
            };

            let Some(item) = item else {
                return false;
            };

            if item.downcast::<TerminalView>().is_some() {
                let Some(split_direction) = active_pane.drag_split_direction() else {
                    return false;
                };

                let Some(terminal_panel) = workspace.read(cx).panel::<TerminalPanel>(cx) else {
                    return false;
                };

                if !terminal_panel.read(cx).center.panes().contains(&&this_pane) {
                    return false;
                }

                let source = tab.pane.clone();
                let item_id_to_move = item.item_id();
                let is_zoomed = {
                    let terminal_panel = terminal_panel.read(cx);
                    if terminal_panel.active_pane == this_pane {
                        active_pane.is_zoomed()
                    } else {
                        terminal_panel.active_pane.read(cx).is_zoomed()
                    }
                };

                let workspace = workspace.downgrade();
                let terminal_panel = terminal_panel.downgrade();
                // Defer the split and move to avoid a re-entrancy panic: the pane
                // may be the one currently being updated, and moving the item
                // updates it.
                window
                    .spawn(cx, async move |cx| {
                        cx.update(|window, cx| {
                            let Ok(new_pane) = terminal_panel.update(cx, |terminal_panel, cx| {
                                let new_pane =
                                    terminal_panel::new_terminal_pane(workspace, project, is_zoomed, window, cx);
                                terminal_panel.center.split(&this_pane, &new_pane, split_direction);
                                anyhow::Ok(new_pane)
                            }) else {
                                return;
                            };

                            let Some(new_pane) = new_pane.log_err() else {
                                return;
                            };

                            workspace::move_item(
                                &source,
                                &new_pane,
                                item_id_to_move,
                                new_pane.read(cx).active_item_index(),
                                true,
                                window,
                                cx,
                            );
                        })
                        .ok();
                    })
                    .detach();

                return true;
            } else if let Some(project_path) = item.project_path(cx)
                && let Some(path) = project.read(cx).absolute_path(&project_path, cx)
            {
                self.add_paths_to_terminal(&[path], window, cx);
                return true;
            }

            return false;
        } else if let Some(selection) = dropped.downcast_ref::<DraggedSelection>() {
            let project = project.read(cx);
            let paths = selection
                .items()
                .map(|selected_entry| selected_entry.entry_id)
                .filter_map(|entry_id| project.path_for_entry(entry_id, cx))
                .filter_map(|project_path| project.absolute_path(&project_path, cx))
                .collect::<Vec<_>>();

            if !paths.is_empty() {
                self.add_paths_to_terminal(&paths, window, cx);
            }

            return true;
        } else if let Some(&entry_id) = dropped.downcast_ref::<ProjectEntryId>() {
            let project = project.read(cx);
            if let Some(path) =
                project.path_for_entry(entry_id, cx).and_then(|project_path| project.absolute_path(&project_path, cx))
            {
                self.add_paths_to_terminal(&[path], window, cx);
            }

            return true;
        }

        false
    }

    fn tab_extra_context_menu_actions(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(SharedString, Box<dyn Action>)> {
        let terminal = self.terminal.read(cx);
        if terminal.task().is_none() { vec![("Rename".into(), Box::new(RenameTerminal))] } else { Vec::new() }
    }

    fn buffer_kind(&self, _: &App) -> ItemBufferKind {
        ItemBufferKind::Singleton
    }

    fn is_dirty(&self, cx: &App) -> bool {
        match self.terminal.read(cx).task() {
            Some(task) => task.status == TaskStatus::Running,
            None => self.has_bell(),
        }
    }

    fn has_running_process(&self, cx: &App) -> bool {
        self.terminal.read(cx).has_running_process()
    }

    fn has_conflict(&self, _cx: &App) -> bool {
        false
    }

    fn can_save_as(&self, _cx: &App) -> bool {
        false
    }

    fn as_searchable(&self, handle: &Entity<Self>, _: &App) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(handle.clone()))
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event);
    }
}

impl SearchableItem for TerminalView {
    type Match = Range;

    fn supported_options(&self) -> SearchOptions {
        SearchOptions {
            case: false,
            word: false,
            regex: true,
            replacement: false,
            selection: false,
            select_all: false,
            find_in_results: false,
        }
    }

    /// Clear stored matches.
    fn clear_matches(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.terminal().update(cx, |term, _| term.matches.clear());
    }

    /// Store matches returned from find_matches somewhere for rendering.
    fn update_matches(
        &mut self,
        matches: &[Self::Match],
        _active_match_index: Option<usize>,
        _token: SearchToken,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal().update(cx, |term, _| term.matches = matches.to_vec());
    }

    /// Returns the selection content to pre-load into this search.
    fn query_suggestion(
        &mut self,
        _seed_query_override: Option<SeedQuery>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> String {
        self.terminal().read(cx).last_content.selection_text.clone().unwrap_or_default()
    }

    /// Focus match at given index into the Vec of matches.
    fn activate_match(
        &mut self,
        index: usize,
        _: &[Self::Match],
        _token: SearchToken,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal().update(cx, |term, _| term.activate_match(index));
        cx.notify();
    }

    /// Add selections for all matches given.
    fn select_matches(&mut self, matches: &[Self::Match], _token: SearchToken, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal().update(cx, |term, _| term.select_matches(matches));
        cx.notify();
    }

    /// Get all of the matches for this query, should be done on the background.
    fn find_matches(
        &mut self,
        query: Arc<SearchQuery>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<Self::Match>> {
        if let Some(s) = regex_search_for_query(&query) {
            self.terminal().update(cx, |term, cx| term.find_matches(s, cx))
        } else {
            Task::ready(vec![])
        }
    }

    /// Reports back to the search toolbar what the active match should be (the selection).
    fn active_match_index(
        &mut self,
        direction: Direction,
        matches: &[Self::Match],
        _token: SearchToken,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        // Selection head might have a value if there's a selection that isn't
        // associated with a match. Therefore, if there are no matches, we should
        // report None, no matter the state of the terminal

        if matches.is_empty() {
            None
        } else if let Some(selection_head) = self.terminal().read(cx).selection_head {
            // If selection head is contained in a match. Return that match
            match direction {
                Direction::Prev => {
                    // If no selection before selection head, return the first match
                    Some(
                        matches
                            .iter()
                            .enumerate()
                            .rev()
                            .find(|(_, search_match)| {
                                search_match.contains(selection_head) || search_match.start() < selection_head
                            })
                            .map_or(0, |(ix, _)| ix),
                    )
                }
                Direction::Next => {
                    // If no selection after selection head, return the last match
                    Some(
                        matches
                            .iter()
                            .enumerate()
                            .find(|(_, search_match)| {
                                search_match.contains(selection_head) || search_match.start() > selection_head
                            })
                            .map_or_else(|| matches.len().saturating_sub(1), |(ix, _)| ix),
                    )
                }
            }
        } else {
            // Matches found but no active selection, return the first last one (closest to cursor)
            Some(matches.len().saturating_sub(1))
        }
    }
    fn replace(
        &mut self,
        _: &Self::Match,
        _: &SearchQuery,
        _token: SearchToken,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
        // Replacement is not supported in terminal view, so this is a no-op.
    }
}

/// Gets the working directory for the given workspace, respecting the user's settings.
/// Falls back to home directory when no project directory is available.
fn default_working_directory(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    current_project_directory(workspace, cx).or_else(|| Some(util::paths::home_dir().clone()))
}

fn current_project_directory(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    workspace
        .project()
        .read(cx)
        .active_project_directory(cx)
        .as_deref()
        .map(Path::to_path_buf)
        .or_else(|| first_project_directory(workspace, cx))
}

///Gets the first project's home directory, or the home directory.
fn first_project_directory(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    let worktree = workspace.worktrees(cx).next()?.read(cx);
    let worktree_path = worktree.abs_path();
    if worktree.root_entry()?.is_dir() {
        Some(worktree_path.to_path_buf())
    } else {
        // If worktree is a file, return its parent directory
        worktree_path.parent().map(|x| x.to_path_buf())
    }
}
