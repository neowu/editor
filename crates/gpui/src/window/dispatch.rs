use std::any::Any;
use std::fmt::Debug;
use std::mem;
use std::time::Duration;

use gpui_util::ResultExt as _;
use smallvec::SmallVec;

use super::DispatchPhase;
use super::FocusHandle;
use super::FocusId;
use super::InputModality;
use super::Window;
use crate::Action;
use crate::App;
use crate::DispatchNodeId;
use crate::InputPreference;
use crate::KeyBinding;
use crate::KeyContext;
use crate::KeyDownEvent;
use crate::Keystroke;
use crate::Modifiers;
use crate::ModifiersChangedEvent;
use crate::MouseMoveEvent;
use crate::MouseUpEvent;
use crate::PlatformInput;
use crate::Replay;
use crate::Task;

const PENDING_INPUT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default, Debug)]
pub(super) struct PendingInput {
    keystrokes: SmallVec<[Keystroke; 1]>,
    focus: Option<FocusId>,
    timeout: Option<Task<()>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchEventResult {
    pub propagate: bool,
    pub default_prevented: bool,
}

impl Window {
    /// Dispatch a given keystroke as though the user had typed it.
    /// You can create a keystroke with Keystroke::parse("").
    pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
        let keystroke = keystroke.with_simulated_ime();
        let result = self.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        if !result.propagate {
            return true;
        }

        if let Some(input) = keystroke.key_char
            && let Some(mut input_handler) = self.platform_window.take_input_handler()
        {
            input_handler.dispatch_input(&input, self, cx);
            self.platform_window.set_input_handler(input_handler);
            return true;
        }

        false
    }

    /// Dispatch a mouse or keyboard event on the window.
    pub fn dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> DispatchEventResult {
        let update_count_before = self.invalidator.update_count();
        // Track input modality for focus-visible styling and hover suppression.
        // Hover is suppressed during keyboard modality so that keyboard navigation
        // doesn't show hover highlights on the item under the mouse cursor.
        let old_modality = self.last_input_modality;
        self.last_input_modality = match &event {
            PlatformInput::KeyDown(_) => InputModality::Keyboard,
            PlatformInput::MouseMove(_) | PlatformInput::MouseDown(_) => InputModality::Mouse,
            _ => self.last_input_modality,
        };
        if self.last_input_modality != old_modality {
            self.refresh();
        }

        // Handlers may set this to false by calling `stop_propagation`.
        cx.propagate_event = true;
        // Handlers may set this to true by calling `prevent_default`.
        self.default_prevented = false;

        let event = match event {
            // Track the mouse position with our own state, since accessing the platform
            // API for the mouse position can only occur on the main thread.
            PlatformInput::MouseMove(mouse_move) => {
                self.mouse_position = mouse_move.position;
                self.modifiers = mouse_move.modifiers;
                PlatformInput::MouseMove(mouse_move)
            }
            PlatformInput::MouseDown(mouse_down) => {
                self.mouse_position = mouse_down.position;
                self.modifiers = mouse_down.modifiers;
                PlatformInput::MouseDown(mouse_down)
            }
            PlatformInput::MouseUp(mouse_up) => {
                self.mouse_position = mouse_up.position;
                self.modifiers = mouse_up.modifiers;
                PlatformInput::MouseUp(mouse_up)
            }
            PlatformInput::MousePressure(mouse_pressure) => PlatformInput::MousePressure(mouse_pressure),
            PlatformInput::MouseExited(mouse_exited) => {
                self.modifiers = mouse_exited.modifiers;
                PlatformInput::MouseExited(mouse_exited)
            }
            PlatformInput::ModifiersChanged(modifiers_changed) => {
                self.modifiers = modifiers_changed.modifiers;
                self.capslock = modifiers_changed.capslock;
                PlatformInput::ModifiersChanged(modifiers_changed)
            }
            PlatformInput::ScrollWheel(scroll_wheel) => {
                self.mouse_position = scroll_wheel.position;
                self.modifiers = scroll_wheel.modifiers;
                PlatformInput::ScrollWheel(scroll_wheel)
            }
            PlatformInput::Pinch(pinch) => {
                self.mouse_position = pinch.position;
                self.modifiers = pinch.modifiers;
                PlatformInput::Pinch(pinch)
            }
            PlatformInput::KeyDown(_) | PlatformInput::KeyUp(_) => event,
        };

        if let Some(any_mouse_event) = event.mouse_event() {
            self.dispatch_mouse_event(any_mouse_event, cx);
        } else if let Some(any_key_event) = event.keyboard_event() {
            self.dispatch_key_event(any_key_event, cx);
        }

        let caused_invalidation = self.invalidator.update_count() > update_count_before;
        if caused_invalidation {
            self.input_rate_tracker.borrow_mut().record_input();
        }

        DispatchEventResult { propagate: cx.propagate_event, default_prevented: self.default_prevented }
    }

    fn dispatch_mouse_event(&mut self, event: &dyn Any, cx: &mut App) {
        self.mouse_hit_test = self.rendered_frame.hit_test(self.mouse_position());

        let mut mouse_listeners = mem::take(&mut self.rendered_frame.mouse_listeners);

        // Capture phase, events bubble from back to front. Handlers for this phase are used for
        // special purposes, such as detecting events outside of a given Bounds.
        for listener in &mut mouse_listeners {
            let listener = listener.as_mut().expect("mouse listener is present during dispatch");
            listener(event, DispatchPhase::Capture, self, cx);
            if !cx.propagate_event {
                break;
            }
        }

        // Bubble phase, where most normal handlers do their work.
        if cx.propagate_event {
            for listener in mouse_listeners.iter_mut().rev() {
                let listener = listener.as_mut().expect("mouse listener is present during dispatch");
                listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    break;
                }
            }
        }

        self.rendered_frame.mouse_listeners = mouse_listeners;

        if cx.has_active_drag() {
            if event.is::<MouseMoveEvent>() {
                // If this was a mouse move event, redraw the window so that the
                // active drag can follow the mouse cursor.
                self.refresh();
            } else if event.is::<MouseUpEvent>() {
                // If this was a mouse up event, cancel the active drag and redraw
                // the window.
                cx.active_drag = None;
                self.refresh();
            }
        }

        // Runs after the listeners so that a drag they start takes effect in this event. Resolving
        // the cursor any earlier would briefly select whatever is under the pointer instead.
        self.reset_cursor_style(cx);
    }

    // If modifiers are not excessive (e.g. AltGr), and the input handler is accepting text input,
    // we prefer the text input over bindings.
    fn input_preference(&mut self, event: &dyn Any, cx: &mut App) -> InputPreference {
        let prefer_character_input =
            event.downcast_ref::<KeyDownEvent>().is_some_and(|key_down_event| key_down_event.prefer_character_input);
        if !prefer_character_input {
            return InputPreference::KeyBindings;
        }
        let Some(mut input_handler) = self.platform_window.take_input_handler() else {
            return InputPreference::KeyBindings;
        };
        let accepts_text_input = input_handler.accepts_text_input(self, cx);
        self.platform_window.set_input_handler(input_handler);
        if accepts_text_input { InputPreference::CharacterInput } else { InputPreference::KeyBindings }
    }

    fn dispatch_key_event(&mut self, event: &dyn Any, cx: &mut App) {
        if self.invalidator.is_dirty() {
            self.draw(cx).clear(cx);
        }

        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        let mut keystroke: Option<Keystroke> = None;

        if let Some(event) = event.downcast_ref::<ModifiersChangedEvent>() {
            if event.modifiers.number_of_modifiers() == 0
                && self.pending_modifier.modifiers.number_of_modifiers() == 1
                && !self.pending_modifier.saw_other_input
            {
                let key = match self.pending_modifier.modifiers {
                    modifiers if modifiers.shift => Some("shift"),
                    modifiers if modifiers.control => Some("control"),
                    modifiers if modifiers.alt => Some("alt"),
                    modifiers if modifiers.platform => Some("platform"),
                    modifiers if modifiers.function => Some("function"),
                    _ => None,
                };
                if let Some(key) = key {
                    keystroke =
                        Some(Keystroke { key: key.to_owned(), key_char: None, modifiers: Modifiers::default() });
                }
            }

            if self.pending_modifier.modifiers.number_of_modifiers() == 0 && event.modifiers.number_of_modifiers() == 1
            {
                self.pending_modifier.saw_other_input = false;
            } else if event.modifiers.number_of_modifiers() > 1 {
                self.pending_modifier.saw_other_input = true;
            }
            self.pending_modifier.modifiers = event.modifiers;
        } else if let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() {
            self.pending_modifier.saw_other_input = true;
            keystroke = Some(key_down_event.keystroke.clone());
            if key_down_event.keystroke.key_char.is_some() {
                cx.platform.hide_cursor_until_mouse_moves();
            }
        }

        let Some(keystroke) = keystroke else {
            self.finish_dispatch_key_event(event, &dispatch_path, cx);
            return;
        };

        cx.propagate_event = true;

        let mut currently_pending = self.pending_input.take().unwrap_or_default();
        if currently_pending.focus.is_some() && currently_pending.focus != self.focus {
            currently_pending = PendingInput::default();
        }

        let match_result =
            self.rendered_frame.dispatch_tree.dispatch_key(currently_pending.keystrokes, keystroke, &dispatch_path);

        if !match_result.to_replay.is_empty() {
            self.replay_pending_input(match_result.to_replay, cx);
            cx.propagate_event = true;
        }

        if !match_result.pending.is_empty() {
            let previous_timeout = currently_pending.timeout.take();
            currently_pending.keystrokes = match_result.pending;
            currently_pending.focus = self.focus;

            let text_input_requires_timeout = event
                .downcast_ref::<KeyDownEvent>()
                .filter(|key_down| key_down.keystroke.key_char.is_some())
                .and_then(|_| self.platform_window.take_input_handler())
                .is_some_and(|mut input_handler| {
                    let accepts = input_handler.accepts_text_input(self, cx);
                    self.platform_window.set_input_handler(input_handler);
                    accepts
                });

            // A lone modifier tap (e.g. switching an IME with shift) must time
            // out like any other pending prefix, or the next tap completes a
            // `shift shift` binding however much later it comes.
            let standalone_modifier =
                matches!(currently_pending.keystrokes.as_slice(), [keystroke] if keystroke.is_modifier_key());
            let needs_timeout = previous_timeout.is_some()
                || match_result.pending_has_binding
                || text_input_requires_timeout
                || standalone_modifier;
            // Dropping the previous task cancels it, so each keystroke restarts the timeout.
            currently_pending.timeout = needs_timeout.then(|| self.start_pending_input_timeout(cx));
            self.pending_input = Some(currently_pending);
            self.pending_input_changed(cx);
            cx.propagate_event = false;
            return;
        }

        // Replayed actions may have changed whether the input handler accepts text.
        let input_preference = self.input_preference(event, cx);
        if input_preference == InputPreference::KeyBindings {
            for binding in match_result.bindings {
                self.dispatch_action_on_node(node_id, binding.action.as_ref(), cx);
                if !cx.propagate_event {
                    self.pending_input_changed(cx);
                    return;
                }
            }
        }

        self.finish_dispatch_key_event(event, &dispatch_path, cx);
        self.pending_input_changed(cx);
    }

    fn start_pending_input_timeout(&self, cx: &App) -> Task<()> {
        self.spawn(cx, async move |cx| {
            cx.background_executor.timer(PENDING_INPUT_TIMEOUT).await;
            cx.update(move |window, cx| {
                let Some(currently_pending) =
                    window.pending_input.take().filter(|pending| pending.focus == window.focus)
                else {
                    return;
                };

                let node_id = window.focus_node_id_in_rendered_frame(window.focus);
                let dispatch_path = window.rendered_frame.dispatch_tree.dispatch_path(node_id);

                let to_replay =
                    window.rendered_frame.dispatch_tree.flush_dispatch(currently_pending.keystrokes, &dispatch_path);

                window.pending_input_changed(cx);
                window.replay_pending_input(to_replay, cx);
            })
            .log_err();
        })
    }

    fn finish_dispatch_key_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        self.dispatch_key_down_up_event(event, dispatch_path, cx);
        if !cx.propagate_event {
            return;
        }

        self.dispatch_modifiers_changed_event(event, dispatch_path, cx);
    }

    pub(crate) fn pending_input_changed(&mut self, cx: &mut App) {
        self.pending_input_observers.clone().retain(|callback| callback(self, cx));
    }

    fn defer_pending_input_changed(&self, cx: &mut App) {
        // Avoid re-entrant entity updates by deferring observer notifications to the end of the
        // current effect cycle, and only for this window.
        let window_handle = self.handle;
        cx.defer(move |cx| {
            window_handle
                .update(cx, |_, window, cx| {
                    window.pending_input_changed(cx);
                })
                .ok();
        });
    }

    fn dispatch_key_down_up_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        // Capture phase
        for node_id in dispatch_path {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);

            for key_listener in node.key_listeners.clone() {
                key_listener(event, DispatchPhase::Capture, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }

        // Bubble phase
        for node_id in dispatch_path.iter().rev() {
            // Handle low level key events
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for key_listener in node.key_listeners.clone() {
                key_listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }
    }

    fn dispatch_modifiers_changed_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        let Some(event) = event.downcast_ref::<ModifiersChangedEvent>() else {
            return;
        };
        for node_id in dispatch_path.iter().rev() {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for listener in node.modifiers_changed_listeners.clone() {
                listener(event, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }
    }

    /// Determine whether a potential multi-stroke key binding is in progress on this window.
    pub fn has_pending_keystrokes(&self) -> bool {
        self.pending_input_keystrokes().is_some()
    }

    pub(crate) fn clear_pending_keystrokes(&mut self, cx: &mut App) {
        if self.pending_input.take().is_some() {
            self.defer_pending_input_changed(cx);
        }
    }

    /// Returns the pending keystrokes that can still complete a multi-stroke key binding. Input
    /// left over from a previous focus can never complete one.
    pub fn pending_input_keystrokes(&self) -> Option<&[Keystroke]> {
        self.pending_input
            .as_ref()
            .filter(|pending_input| pending_input.focus == self.focus)
            .map(|pending_input| pending_input.keystrokes.as_slice())
    }

    fn replay_pending_input(&mut self, replays: SmallVec<[Replay; 1]>, cx: &mut App) {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        'replay: for replay in replays {
            let event =
                KeyDownEvent { keystroke: replay.keystroke.clone(), is_held: false, prefer_character_input: true };

            cx.propagate_event = true;
            for binding in replay.bindings {
                self.dispatch_action_on_node(node_id, binding.action.as_ref(), cx);
                if !cx.propagate_event {
                    continue 'replay;
                }
            }

            self.dispatch_key_down_up_event(&event, &dispatch_path, cx);
            if !cx.propagate_event {
                continue 'replay;
            }
            if let Some(input) = replay.keystroke.key_char
                && let Some(mut input_handler) = self.platform_window.take_input_handler()
            {
                input_handler.dispatch_input(&input, self, cx);
                self.platform_window.set_input_handler(input_handler);
            }
        }
    }

    pub(super) fn focus_node_id_in_rendered_frame(&self, focus_id: Option<FocusId>) -> DispatchNodeId {
        focus_id
            .and_then(|focus_id| self.rendered_frame.dispatch_tree.focusable_node_id(focus_id))
            .unwrap_or_else(|| self.rendered_frame.dispatch_tree.root_node_id())
    }

    pub(super) fn dispatch_action_on_node(&mut self, node_id: DispatchNodeId, action: &dyn Action, cx: &mut App) {
        self.dispatch_action_on_node_inner(node_id, action, cx);

        if !cx.propagate_event && self.last_input_was_keyboard() {
            cx.platform.hide_cursor_until_mouse_moves();
        }
    }

    /// Run the handlers for `action` in the tables attached to `node_id`, for the capture or the
    /// bubble phase. Returns whether the action still propagates.
    fn dispatch_action_on_tables(
        &mut self,
        node_id: DispatchNodeId,
        action: &dyn Action,
        capture: bool,
        cx: &mut App,
    ) -> bool {
        let any_action = action.as_any();
        let mut table_ix = 0;
        while let Some(table) = self.rendered_frame.dispatch_tree.node(node_id).action_tables.get(table_ix) {
            table_ix += 1;
            let table = table.clone();
            for ix in table.handlers_for(any_action.type_id()) {
                // A handler that does not apply behaves as if it were not registered.
                if table.is_capture(ix) != capture || !table.applies(ix) {
                    continue;
                }
                // Actions stop propagation by default during the bubble phase only.
                cx.propagate_event = capture;
                table.dispatch(ix, any_action, self, cx);

                if !cx.propagate_event {
                    return false;
                }
            }
        }
        true
    }

    fn dispatch_action_on_node_inner(&mut self, node_id: DispatchNodeId, action: &dyn Action, cx: &mut App) {
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        cx.propagate_event = true;

        // Capture phase for window actions.
        for node_id in &dispatch_path {
            if !self.dispatch_action_on_tables(*node_id, action, true, cx) {
                return;
            }
        }

        // Bubble phase for window actions.
        for node_id in dispatch_path.iter().rev() {
            if !self.dispatch_action_on_tables(*node_id, action, false, cx) {
                return;
            }
        }

        // Global listener runs last.
        if let Some(listener) = cx.global_action_listeners.get(&action.as_any().type_id()).cloned() {
            cx.propagate_event = false; // Actions stop propagation by default during the bubble phase
            listener(action.as_any(), cx);
        }
    }

    /// Returns the current context stack.
    pub fn context_stack(&self) -> Vec<KeyContext> {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree
            .dispatch_path(node_id)
            .iter()
            .filter_map(move |&node_id| dispatch_tree.node(node_id).context.clone())
            .collect()
    }

    /// Returns all available actions for the focused element.
    pub fn available_actions(&self, cx: &App) -> Vec<Box<dyn Action>> {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let mut actions = self.rendered_frame.dispatch_tree.available_actions(node_id);
        for action_type in cx.global_action_listeners.keys() {
            if let Err(ix) = actions.binary_search_by_key(action_type, |a| a.as_any().type_id()) {
                let action = cx.actions.build_action_type(action_type).ok();
                if let Some(action) = action {
                    actions.insert(ix, action);
                }
            }
        }
        actions
    }

    /// Returns key bindings that invoke an action on the currently focused element. Bindings are
    /// returned in the order they were added. For display, the last binding should take precedence.
    pub fn bindings_for_action(&self, action: &dyn Action) -> Vec<KeyBinding> {
        self.rendered_frame.dispatch_tree.bindings_for_action(action, &self.rendered_frame.dispatch_tree.context_stack)
    }

    /// Returns the highest precedence key binding that invokes an action on the currently focused
    /// element. This is more efficient than getting the last result of `bindings_for_action`.
    pub fn highest_precedence_binding_for_action(&self, action: &dyn Action) -> Option<KeyBinding> {
        self.rendered_frame
            .dispatch_tree
            .highest_precedence_binding_for_action(action, &self.rendered_frame.dispatch_tree.context_stack)
    }

    /// Returns the highest precedence key binding that would invoke an action on the given focus
    /// handle if it were focused. This is more efficient than getting the last result of
    /// `bindings_for_action_in`.
    pub fn highest_precedence_binding_for_action_in(
        &self,
        action: &dyn Action,
        focus_handle: &FocusHandle,
    ) -> Option<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let context_stack = self.context_stack_for_focus_handle(focus_handle)?;
        dispatch_tree.highest_precedence_binding_for_action(action, &context_stack)
    }

    fn context_stack_for_focus_handle(&self, focus_handle: &FocusHandle) -> Option<Vec<KeyContext>> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let node_id = dispatch_tree.focusable_node_id(focus_handle.id)?;
        let context_stack: Vec<_> = dispatch_tree
            .dispatch_path(node_id)
            .into_iter()
            .filter_map(|node_id| dispatch_tree.node(node_id).context.clone())
            .collect();
        Some(context_stack)
    }
}
