use std::cmp::{self};
use std::sync::Arc;
use std::time::Duration;

use ed_actions::command_palette::Toggle;
use fuzzy_nucleo::StringMatch;
use fuzzy_nucleo::StringMatchCandidate;
use gpui::Action;
use gpui::App;
use gpui::Context;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::ParentElement;
use gpui::Render;
use gpui::Styled;
use gpui::Task;
use gpui::WeakEntity;
use gpui::Window;
use picker::Picker;
use picker::PickerDelegate;
use settings::Settings;
use ui::HighlightedLabel;
use ui::KeyBinding;
use ui::ListItem;
use ui::ListItemSpacing;
use ui::prelude::*;
use util::ResultExt;
use workspace::ModalView;
use workspace::Workspace;
use workspace::WorkspaceSettings;

pub fn init(cx: &mut App) {
    cx.observe_new(CommandPalette::register).detach();
}

impl ModalView for CommandPalette {}

pub struct CommandPalette {
    picker: Entity<Picker<CommandPaletteDelegate>>,
}

/// Removes subsequent whitespace characters and double colons from the query, and converts
/// underscores to spaces.
///
/// This improves the likelihood of a match by either humanized name or keymap-style name.
/// Underscores are converted to spaces because `humanize_action_name` converts them to spaces
/// when building the search candidates (e.g. `terminal_panel::Toggle` -> `terminal panel: toggle`).
pub fn normalize_action_query(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last_char = None;

    for char in input.trim().chars() {
        let normalized_char = if char == '_' { ' ' } else { char };
        match (last_char, normalized_char) {
            (Some(':'), ':') => continue,
            (Some(last_char), c) if last_char.is_whitespace() && c.is_whitespace() => {
                continue;
            }
            _ => {
                last_char = Some(normalized_char);
            }
        }
        result.push(normalized_char);
    }

    result
}

impl CommandPalette {
    fn register(workspace: &mut Workspace, _window: Option<&mut Window>, _: &mut Context<Workspace>) {
        workspace.register_action(|workspace, _: &Toggle, window, cx| Self::toggle(workspace, "", window, cx));
    }

    pub fn toggle(workspace: &mut Workspace, query: &str, window: &mut Window, cx: &mut Context<Workspace>) {
        if workspace.active_modal::<CommandPalette>(cx).is_some() {
            workspace.hide_modal(window, cx);
            return;
        }

        if workspace.has_active_modal(window, cx) && !workspace.hide_modal(window, cx) {
            return;
        }

        let Some(previous_focus_handle) = window.focused(cx) else {
            return;
        };

        workspace
            .toggle_modal(window, cx, move |window, cx| CommandPalette::new(previous_focus_handle, query, window, cx));
    }

    fn new(previous_focus_handle: FocusHandle, query: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut commands: Vec<Command> = window
            .available_actions(cx)
            .into_iter()
            .map(|action| Command { name: SharedString::from(humanize_action_name(action.name())), action })
            .collect();
        sort_commands(&mut commands);

        let delegate = CommandPaletteDelegate::new(cx.entity().downgrade(), commands, previous_focus_handle);

        let picker = cx.new(|cx| {
            // One-shot action; there's nothing to reopen.
            let picker = Picker::uniform_list(delegate, window, cx)
                .initial_width(rems(38.0))
                .reopenable(false, cx)
                .show_scrollbar(true);
            picker.set_query(query, window, cx);
            picker
        });
        Self { picker }
    }

    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.picker.update(cx, |picker, cx| picker.set_query(query, window, cx))
    }
}

impl EventEmitter<DismissEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().key_context("CommandPalette").child(self.picker.clone())
    }
}

pub struct CommandPaletteDelegate {
    latest_query: String,
    command_palette: WeakEntity<CommandPalette>,
    all_commands: Vec<Command>,
    commands: Vec<Command>,
    matches: Vec<StringMatch>,
    selected_ix: usize,
    previous_focus_handle: FocusHandle,
    #[allow(clippy::type_complexity)]
    updating_matches: Option<(Task<()>, async_channel::Receiver<(Vec<Command>, Vec<StringMatch>)>)>,
}

struct Command {
    name: SharedString,
    action: Box<dyn Action>,
}

/// Orders commands alphabetically by their humanized name, the palette's only ranking.
fn sort_commands(commands: &mut [Command]) {
    commands.sort_by(|a, b| a.name.cmp(&b.name));
}

impl Clone for Command {
    fn clone(&self) -> Self {
        Self { name: self.name.clone(), action: self.action.boxed_clone() }
    }
}

impl CommandPaletteDelegate {
    fn new(
        command_palette: WeakEntity<CommandPalette>,
        commands: Vec<Command>,
        previous_focus_handle: FocusHandle,
    ) -> Self {
        Self {
            command_palette,
            all_commands: commands.clone(),
            matches: Vec::new(),
            commands,
            selected_ix: 0,
            previous_focus_handle,
            latest_query: String::new(),
            updating_matches: None,
        }
    }

    fn matches_updated(
        &mut self,
        query: String,
        commands: Vec<Command>,
        matches: Vec<StringMatch>,
        _: &mut Context<Picker<Self>>,
    ) {
        self.updating_matches.take();
        self.latest_query = query;

        self.commands = commands;
        self.matches = matches;
        if self.matches.is_empty() {
            self.selected_ix = 0;
        } else {
            self.selected_ix = cmp::min(self.selected_ix, self.matches.len() - 1);
        }
    }

    fn selected_command(&self) -> Option<&Command> {
        if self.matches.is_empty() {
            return None;
        }
        let action_ix = self.matches.get(self.selected_ix).map(|m| m.candidate_id).unwrap_or(self.selected_ix);
        // this gets called in headless tests where there are no commands loaded
        // so we need to return an Option here
        self.commands.get(action_ix)
    }
}

impl PickerDelegate for CommandPaletteDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Execute a command...".into()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_ix
    }

    fn set_selected_index(&mut self, ix: usize, _window: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected_ix = ix;
    }

    fn update_matches(
        &mut self,
        mut query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> gpui::Task<()> {
        let settings = WorkspaceSettings::get_global(cx);
        if let Some(alias) = settings.command_aliases.get(&query) {
            alias.as_ref().clone_into(&mut query);
        }

        let (tx, rx) = async_channel::bounded(1);

        let query_str = query.as_str();

        let task = cx.background_spawn({
            let commands = self.all_commands.clone();
            let executor = cx.background_executor().clone();
            let query = normalize_action_query(query_str);
            async move {
                let candidates = commands
                    .iter()
                    .enumerate()
                    .map(|(ix, command)| StringMatchCandidate::new(ix, &command.name))
                    .collect::<Vec<_>>();

                let matches = fuzzy_nucleo::match_strings_async(
                    &candidates,
                    &query,
                    fuzzy_nucleo::Case::Smart,
                    fuzzy_nucleo::LengthPenalty::On,
                    10000,
                    &std::sync::atomic::AtomicBool::default(),
                    executor,
                )
                .await;

                tx.send((commands, matches)).await.log_err();
            }
        });

        self.updating_matches = Some((task, rx.clone()));

        cx.spawn_in(window, async move |picker, cx| {
            let Ok((commands, matches)) = rx.recv().await else {
                return;
            };

            picker.update(cx, |picker, cx| picker.delegate.matches_updated(query, commands, matches, cx)).ok();
        })
    }

    fn finalize_update_matches(
        &mut self,
        query: String,
        duration: Duration,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> bool {
        let Some((task, rx)) = self.updating_matches.take() else {
            return true;
        };

        match cx.foreground_executor().block_with_timeout(duration, rx.clone().recv()) {
            Ok(Ok((commands, matches))) => {
                self.matches_updated(query, commands, matches, cx);
                true
            }
            _ => {
                self.updating_matches = Some((task, rx));
                false
            }
        }
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.command_palette.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if self.matches.is_empty() {
            self.dismissed(window, cx);
            return;
        }

        let action_ix = self.matches[self.selected_ix].candidate_id;
        let command = self.commands.swap_remove(action_ix);

        self.matches.clear();
        self.commands.clear();
        let action = command.action;
        window.focus(&self.previous_focus_handle, cx);
        self.dismissed(window, cx);
        window.dispatch_action(action, cx);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let matching_command = self.matches.get(ix)?;
        let command = self.commands.get(matching_command.candidate_id)?;

        Some(
            ListItem::new(ix).inset(true).spacing(ListItemSpacing::Sparse).toggle_state(selected).child(
                h_flex()
                    .w_full()
                    .py_px()
                    .justify_between()
                    .gap_2()
                    .child(HighlightedLabel::new(command.name.clone(), matching_command.positions.clone()).truncate())
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .child(KeyBinding::for_action_in(&*command.action, &self.previous_focus_handle, cx)),
                    ),
            ),
        )
    }

    fn render_footer(&self, _window: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        self.selected_command()?;
        let focus_handle = &self.previous_focus_handle;

        Some(
            h_flex()
                .w_full()
                .p_1p5()
                .gap_1()
                .justify_end()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    Button::new("run-action", "Run")
                        .key_binding(
                            KeyBinding::for_action_in(&menu::Confirm, focus_handle, cx)
                                .map(|kb| kb.size(rems_from_px(12_f32))),
                        )
                        .on_click(|_, window, cx| window.dispatch_action(menu::Confirm.boxed_clone(), cx)),
                )
                .into_any(),
        )
    }
}

pub fn humanize_action_name(name: &str) -> String {
    let chars = name.chars().collect::<Vec<_>>();
    let capacity = name.len() + chars.iter().filter(|c| c.is_uppercase()).count();
    let mut result = String::with_capacity(capacity);
    let mut index = 0;

    while index < chars.len() {
        let char = chars[index];
        if char == ':' {
            if result.ends_with(':') {
                result.push(' ');
            } else {
                result.push(':');
            }
            index += 1;
        } else if char == '_' {
            result.push(' ');
            index += 1;
        } else if char.is_uppercase() {
            let start = index;
            index += 1;
            while chars.get(index).is_some_and(|next_char| next_char.is_uppercase()) {
                index += 1;
            }

            let uppercase_run = &chars[start..index];
            if uppercase_run.len() > 1 {
                let split_before_last = chars.get(index).is_some_and(|next_char| next_char.is_lowercase());
                let acronym_end = if split_before_last { uppercase_run.len() - 1 } else { uppercase_run.len() };

                if acronym_end > 0 {
                    if !result.ends_with(' ') {
                        result.push(' ');
                    }
                    result.extend(&uppercase_run[..acronym_end]);
                }

                if split_before_last {
                    if !result.ends_with(' ') {
                        result.push(' ');
                    }
                    result.extend(uppercase_run[acronym_end].to_lowercase());
                }
            } else {
                if !result.ends_with(' ') {
                    result.push(' ');
                }
                result.extend(char.to_lowercase());
            }
        } else {
            result.push(char);
            index += 1;
        }
    }

    result
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command").field("name", &self.name).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use gpui::NoAction;

    use super::*;

    #[test]
    fn sort_commands_orders_by_name() {
        let mut commands = ["pane: split", "editor: copy", "buffer search: deploy"]
            .into_iter()
            .map(|name| Command { name: name.into(), action: Box::new(NoAction) })
            .collect::<Vec<_>>();
        sort_commands(&mut commands);
        let names = commands.iter().map(|command| command.name.as_ref()).collect::<Vec<_>>();
        assert_eq!(names, ["buffer search: deploy", "editor: copy", "pane: split"]);
    }
}
