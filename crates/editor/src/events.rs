use gpui::Context;
use gpui::Entity;
use gpui::FocusOutEvent;
use gpui::MouseMoveEvent;
use gpui::Window;
use multi_buffer::MultiBuffer;
use workspace::searchable::SearchEvent;

use super::Editor;
use super::EditorEvent;
use crate::blink_manager::BlinkManager;
use crate::display_map::DisplayMap;
// lint: expect(module_cycle)
use crate::element::EditorElement;
use crate::git::blame::GitBlame;
use crate::hover_popover::hide_hover;
use crate::inlays::inlay_hints::InlayHintRefreshReason;
use crate::jsx_tag_auto_close;

impl Editor {
    pub(super) fn on_buffer_changed(&mut self, _: Entity<MultiBuffer>, cx: &mut Context<Self>) {
        cx.notify();
    }

    pub(super) fn on_buffer_event(
        &mut self,
        multibuffer: &Entity<MultiBuffer>,
        event: &multi_buffer::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            multi_buffer::Event::Edited { edited_buffer } => {
                self.scrollbar_marker_state.dirty = true;
                // lint: expect(module_cycle)
                self.active_indent_guides_state.dirty = true;
                self.fit_gutter_line_number_width(false, cx);
                self.refresh_active_diagnostics(cx);
                self.refresh_code_actions_for_selection(window, cx);
                self.refresh_single_line_folds(window, cx);
                let snapshot = self.snapshot(window, cx);
                self.refresh_matching_bracket_highlights(&snapshot, cx);
                self.refresh_spell_check(cx);

                if let Some(buffer) = edited_buffer {
                    if buffer.read(cx).file().is_none() {
                        cx.emit(EditorEvent::TitleChanged);
                    }

                    let buffer_id = buffer.read(cx).buffer_id();

                    if self.project.is_some() {
                        self.register_buffer(buffer_id, cx);
                        self.update_lsp_data(Some(buffer_id), window, cx);
                        self.refresh_inlay_hints(InlayHintRefreshReason::BufferEdited(buffer_id), cx);
                    }
                }

                self.inlay_hint_visibility_changed(cx);
                cx.emit(EditorEvent::BufferEdited);
                cx.emit(SearchEvent::MatchesInvalidated);
            }
            multi_buffer::Event::BufferRangesUpdated { buffer, ranges, path_key } => {
                if let Some(hovered_link_state) = self.hovered_link_state.as_mut() {
                    hovered_link_state.symbol_range = None;
                }
                self.refresh_document_highlights(cx);
                let buffer_id = buffer.read(cx).buffer_id();
                self.remove_invalid_lsp_data(&[buffer_id], false, cx);
                if self.buffer.read(cx).diff_for(buffer_id).is_none()
                    && let Some(project) = self.project.clone()
                {
                    self.update_uncommitted_diff_for_buffer(&project, [buffer.clone()], cx).detach();
                }
                self.register_visible_buffers(cx);
                self.update_lsp_data(Some(buffer_id), window, cx);
                self.refresh_inlay_hints(InlayHintRefreshReason::NewLinesShown, cx);
                self.inlay_hint_visibility_changed(cx);
                self.refresh_runnables(None, window, cx);
                self.refresh_spell_check(cx);
                self.refresh_selected_text_highlights(&self.display_snapshot(cx), true, window, cx);
                self.semantic_token_state.invalidate_buffer(buffer_id);
                cx.emit(EditorEvent::BufferRangesUpdated {
                    buffer: buffer.clone(),
                    ranges: ranges.clone(),
                    path_key: path_key.clone(),
                });
            }
            multi_buffer::Event::BuffersRemoved { removed_buffer_ids } => {
                self.remove_invalid_lsp_data(removed_buffer_ids, true, cx);
                if let Some(inlay_hints) = &mut self.inlay_hints {
                    inlay_hints.remove_inlay_chunk_data(removed_buffer_ids);
                }
                self.refresh_inlay_hints(InlayHintRefreshReason::BuffersRemoved(removed_buffer_ids.clone()), cx);
                for buffer_id in removed_buffer_ids {
                    self.registered_buffers.remove(buffer_id);
                    self.clear_runnables(Some(*buffer_id));
                    self.semantic_token_state.invalidate_buffer(*buffer_id);
                    self.lsp_document_symbols.remove(buffer_id);
                    self.lsp_document_links.per_buffer.remove(buffer_id);
                    self.display_map.update(cx, |display_map, cx| {
                        display_map.invalidate_semantic_highlights(*buffer_id);
                        display_map.clear_lsp_folding_ranges(*buffer_id, cx);
                    });
                }

                self.display_map.update(cx, |display_map, cx| {
                    display_map.unfold_buffers(removed_buffer_ids.iter().copied(), cx);
                });
                self.inlay_hint_visibility_changed(cx);

                jsx_tag_auto_close::refresh_enabled_in_any_buffer(self, multibuffer, cx);
                cx.emit(EditorEvent::BuffersRemoved { removed_buffer_ids: removed_buffer_ids.clone() });
            }
            multi_buffer::Event::BuffersEdited { buffer_ids } => {
                self.display_map.update(cx, |map, cx| map.unfold_buffers(buffer_ids.iter().copied(), cx));
                self.inlay_hint_visibility_changed(cx);
                cx.emit(EditorEvent::BuffersEdited { buffer_ids: buffer_ids.clone() });
            }
            multi_buffer::Event::Reparsed(buffer_id) => {
                self.refresh_runnables(Some(*buffer_id), window, cx);
                self.refresh_selected_text_highlights(&self.display_snapshot(cx), true, window, cx);
                self.refresh_spell_check(cx);
                jsx_tag_auto_close::refresh_enabled_in_any_buffer(self, multibuffer, cx);

                cx.emit(EditorEvent::Reparsed(*buffer_id));
            }
            multi_buffer::Event::DiffHunksToggled => {
                self.refresh_runnables(None, window, cx);
            }
            multi_buffer::Event::LanguageChanged(buffer_id, is_fresh_language) => {
                if !is_fresh_language {
                    self.registered_buffers.remove(buffer_id);
                }
                // No event exists for a buffer detaching from a language server: a switch
                // to a language with no server emits neither LanguageServerRemoved nor
                // LanguageServerBufferRegistered, so the language change itself is the only
                // signal that this buffer's server set changed. Run the same LSP data sweep
                // as those events do, or hints and colors from the old server stay visible.
                if self.project.is_some() {
                    self.register_buffer(*buffer_id, cx);
                    self.invalidate_semantic_tokens(Some(*buffer_id));
                    self.update_lsp_data(Some(*buffer_id), window, cx);
                    self.refresh_inlay_hints(InlayHintRefreshReason::ServerRemoved, cx);
                    self.refresh_document_highlights(cx);
                }
                jsx_tag_auto_close::refresh_enabled_in_any_buffer(self, multibuffer, cx);
                self.refresh_spell_check(cx);
                cx.emit(EditorEvent::Reparsed(*buffer_id));
                cx.emit(EditorEvent::LanguageChanged);

                cx.notify();
            }
            multi_buffer::Event::SettingsChanged => {
                let new_language_settings = self.fetch_applicable_language_settings(cx);
                if new_language_settings != self.applicable_language_settings {
                    self.applicable_language_settings = new_language_settings;
                    cx.notify();
                }
            }
            multi_buffer::Event::DirtyChanged => cx.emit(EditorEvent::DirtyChanged),
            multi_buffer::Event::Saved => cx.emit(EditorEvent::Saved),
            multi_buffer::Event::FileHandleChanged => {
                cx.emit(EditorEvent::TitleChanged);
                cx.emit(EditorEvent::FileHandleChanged);
            }
            multi_buffer::Event::CapabilityChanged => {
                cx.emit(EditorEvent::CapabilityChanged);
                cx.notify();
            }
            multi_buffer::Event::Reloaded | multi_buffer::Event::BufferDiffChanged => {
                cx.emit(EditorEvent::TitleChanged);
            }
            multi_buffer::Event::DiagnosticsUpdated => {
                self.update_diagnostics_state(window, cx);
            }
            multi_buffer::Event::TransactionUndone { .. } => {}
        }
    }

    pub(super) fn on_display_map_changed(&mut self, _: Entity<DisplayMap>, _: &mut Window, cx: &mut Context<Self>) {
        self.inlay_hint_visibility_changed(cx);
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub(super) fn handle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_motions.clear();
        cx.emit(EditorEvent::Focused);

        if let Some(descendant) = self.last_focused_descendant.take().and_then(|descendant| descendant.upgrade()) {
            window.focus(&descendant, cx);
        } else {
            if let Some(blame) = self.blame.as_ref() {
                blame.update(cx, GitBlame::focus);
            }

            self.blink_manager.update(cx, BlinkManager::enable);

            self.buffer.update(cx, |buffer, cx| {
                buffer.finalize_last_transaction(cx);
            });

            if cx.is_cursor_visible()
                && let Some(position_map) = self.last_position_map.clone()
            {
                // lint: expect(module_cycle)
                EditorElement::mouse_moved(
                    self,
                    &MouseMoveEvent {
                        position: window.mouse_position(),
                        pressed_button: None,
                        modifiers: window.modifiers(),
                    },
                    &position_map,
                    None,
                    window,
                    cx,
                );
            }
        }
    }

    pub(super) fn handle_focus_in(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::FocusedIn);
    }

    pub(super) fn handle_focus_out(&mut self, event: FocusOutEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        if event.blurred != self.focus_handle {
            self.last_focused_descendant = Some(event.blurred);
        }
    }

    pub(crate) fn handle_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_motions.clear();
        self.blink_manager.update(cx, BlinkManager::disable);

        if let Some(blame) = self.blame.as_ref() {
            blame.update(cx, GitBlame::blur);
        }
        if !self.hover_state.focused(window, cx) {
            hide_hover(self, cx);
        }
        if !self.context_menu.borrow().as_ref().is_some_and(|context_menu| context_menu.focused(window, cx)) {
            self.hide_context_menu(cx);
        }

        cx.emit(EditorEvent::Blurred);
        cx.notify();
    }
}
