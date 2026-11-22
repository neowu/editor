use std::any::Any;
use std::any::TypeId;
use std::rc::Rc;
use std::sync::LazyLock;

use gpui::Action;
use gpui::App;
use gpui::Context;
use gpui::DispatchPhase;
use gpui::Entity;
use gpui::ModifiersChangedEvent;
use gpui::TaskExt as _;
use gpui::Window;
use util::ResultExt as _;
use workspace::item::Item as _;
use workspace::item::ItemBufferKind;

use super::EditorElement;
use super::EditorLayout;
use crate::Editor;
use crate::HalfPageDown;
use crate::HalfPageUp;
use crate::HandleInput;
use crate::LineDown;
use crate::LineUp;
use crate::PageDown;
use crate::PageUp;
use crate::hover_popover;
use crate::rust_analyzer_ext;
use crate::scroll::scroll_amount::ScrollAmount;

impl EditorElement {
    pub(super) fn register_actions(&self, window: &mut Window, cx: &mut App) {
        let editor = &self.editor;
        editor.update(cx, |editor, cx| {
            for action in editor.editor_actions.borrow().values() {
                (action)(editor, window, cx);
            }
        });

        rust_analyzer_ext::apply_related_actions(editor, window, cx);

        register_action_table(editor, window, &ACTIONS);
        if editor.read(cx).buffer_kind(cx) == ItemBufferKind::Multibuffer {
            register_action(editor, window, Editor::expand_excerpts);
            register_action(editor, window, Editor::expand_excerpts_up);
            register_action(editor, window, Editor::expand_excerpts_down);
        }
        if editor.read(cx).lsp_data_enabled() {
            if editor.read(cx).can_accept_inlay_hint(cx) {
                register_action(editor, window, Editor::accept_inlay_hint);
            }
            register_action(editor, window, Editor::toggle_diagnostics);
        }
        if editor.read(cx).supports_minimap(cx) {
            register_action(editor, window, Editor::toggle_minimap);
        }
        if editor.read(cx).blame().is_some() {
            register_action(editor, window, Editor::open_git_blame_commit);
            if editor.update(cx, |editor, cx| editor.blame_revision_target(window, cx).is_some()) {
                register_action(editor, window, Editor::blame_revision);
            }
            if editor.update(cx, |editor, cx| editor.blame_previous_revision_target(window, cx).is_some()) {
                register_action(editor, window, Editor::blame_previous_revision);
            }
        }
        if !editor.read(cx).read_only(cx) {
            register_action_table(editor, window, &EDIT_ACTIONS);
            if editor.read(cx).enable_wrap_selections_in_tag(cx) {
                register_action(editor, window, Editor::wrap_selections_in_tag);
            }
            if editor.read(cx).can_format_selections(cx) {
                register_action(editor, window, |editor, action, window, cx| {
                    if let Some(task) = editor.format_selections(action, window, cx) {
                        editor.detach_and_notify_err(task, window, cx);
                    } else {
                        cx.propagate();
                    }
                });
            }
        }
    }

    pub(super) fn register_key_listeners(&self, window: &mut Window, _: &mut App, layout: &EditorLayout) {
        let position_map = Rc::clone(&layout.position_map);
        window.on_key_event({
            let editor = self.editor.clone();
            move |event: &ModifiersChangedEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                editor.update(cx, |editor, cx| {
                    if editor.hover_state.focused(window, cx) {
                        return;
                    }

                    editor.handle_modifiers_changed(event.modifiers, &position_map, window, cx);
                });
            }
        });
    }
}

/// Actions every editor handles.
static ACTIONS: LazyLock<ActionTable> = LazyLock::new(|| {
    let mut t = ActionTableBuilder::default();
    t.add(Editor::open_context_menu);
    t.add(Editor::move_left);
    t.add(Editor::move_right);
    t.add(Editor::move_down);
    t.add(Editor::move_down_by_lines);
    t.add(Editor::select_down_by_lines);
    t.add(Editor::move_up);
    t.add(Editor::move_up_by_lines);
    t.add(Editor::select_up_by_lines);
    t.add(Editor::select_page_down);
    t.add(Editor::select_page_up);
    t.add(Editor::cancel);
    t.add(Editor::next_snippet_tabstop);
    t.add(Editor::previous_snippet_tabstop);
    t.add(Editor::copy);
    t.add(Editor::diff_clipboard_with_selection);
    t.add(Editor::move_page_up);
    t.add(Editor::move_page_down);
    t.add(Editor::next_screen);
    t.add(Editor::scroll_cursor_top);
    t.add(Editor::scroll_cursor_center);
    t.add(Editor::scroll_cursor_bottom);
    t.add(Editor::scroll_cursor_center_top_bottom);
    t.add(|editor, _: &LineDown, window, cx| {
        editor.scroll_screen_with_cursor_margin(&ScrollAmount::Line(1.), window, cx);
    });
    t.add(|editor, _: &LineUp, window, cx| {
        editor.scroll_screen_with_cursor_margin(&ScrollAmount::Line(-1.), window, cx);
    });
    t.add(|editor, _: &HalfPageDown, window, cx| {
        editor.scroll_screen_with_cursor_margin(&ScrollAmount::Page(0.5), window, cx);
    });
    t.add(|editor, _: &HalfPageUp, window, cx| {
        editor.scroll_screen_with_cursor_margin(&ScrollAmount::Page(-0.5), window, cx);
    });
    t.add(|editor, _: &PageDown, window, cx| {
        editor.scroll_screen_with_cursor_margin(&ScrollAmount::Page(1.), window, cx);
    });
    t.add(|editor, _: &PageUp, window, cx| {
        editor.scroll_screen_with_cursor_margin(&ScrollAmount::Page(-1.), window, cx);
    });
    t.add(Editor::move_to_previous_word_start);
    t.add(Editor::move_to_previous_subword_start);
    t.add(Editor::move_to_next_word_end);
    t.add(Editor::move_to_next_subword_end);
    t.add(Editor::move_to_beginning_of_line);
    t.add(Editor::move_to_end_of_line);
    t.add(Editor::move_to_start_of_paragraph);
    t.add(Editor::move_to_end_of_paragraph);
    t.add(Editor::move_to_next_comment_paragraph);
    t.add(Editor::move_to_previous_comment_paragraph);
    t.add(Editor::move_to_beginning);
    t.add(Editor::move_to_end);
    t.add(Editor::move_to_start_of_excerpt);
    t.add(Editor::move_to_start_of_next_excerpt);
    t.add(Editor::move_to_end_of_excerpt);
    t.add(Editor::move_to_end_of_previous_excerpt);
    t.add(Editor::select_up);
    t.add(Editor::select_down);
    t.add(Editor::select_left);
    t.add(Editor::select_right);
    t.add(Editor::select_to_previous_word_start);
    t.add(Editor::select_to_previous_subword_start);
    t.add(Editor::select_to_next_word_end);
    t.add(Editor::select_to_next_subword_end);
    t.add(Editor::select_to_beginning_of_line);
    t.add(Editor::select_to_end_of_line);
    t.add(Editor::select_to_start_of_paragraph);
    t.add(Editor::select_to_end_of_paragraph);
    t.add(Editor::select_to_start_of_excerpt);
    t.add(Editor::select_to_start_of_next_excerpt);
    t.add(Editor::select_to_end_of_excerpt);
    t.add(Editor::select_to_end_of_previous_excerpt);
    t.add(Editor::select_to_beginning);
    t.add(Editor::select_to_end);
    t.add(Editor::select_all);
    t.add(|editor, action, window, cx| {
        editor.select_all_matches(action, window, cx).log_err();
    });
    t.add(Editor::select_line);
    t.add(Editor::split_selection_into_lines);
    t.add(Editor::add_selection_above);
    t.add(Editor::add_selection_below);
    t.add(Editor::insert_snippet_at_selections);
    t.add(|editor, action, window, cx| {
        editor.select_next(action, window, cx).log_err();
    });
    t.add(|editor, action, window, cx| {
        editor.select_previous(action, window, cx).log_err();
    });
    t.add(|editor, action, window, cx| {
        editor.find_next_match(action, window, cx).log_err();
    });
    t.add(|editor, action, window, cx| {
        editor.find_previous_match(action, window, cx).log_err();
    });
    t.add(Editor::select_larger_syntax_node);
    t.add(Editor::select_smaller_syntax_node);
    t.add(Editor::select_next_syntax_node);
    t.add(Editor::select_prev_syntax_node);
    t.add(Editor::select_to_start_of_larger_syntax_node);
    t.add(Editor::select_to_end_of_larger_syntax_node);
    t.add(Editor::move_to_start_of_larger_syntax_node);
    t.add(Editor::move_to_end_of_larger_syntax_node);
    t.add(Editor::select_enclosing_symbol);
    t.add(Editor::move_to_enclosing_bracket);
    t.add(Editor::select_inside_delimiters);
    t.add(Editor::select_around_delimiters);
    t.add(Editor::undo_selection);
    t.add(Editor::redo_selection);
    t.add(Editor::go_to_diagnostic);
    t.add(Editor::go_to_prev_diagnostic);
    t.add(Editor::go_to_next_hunk);
    t.add(Editor::go_to_prev_hunk);
    t.add(Editor::go_to_next_document_highlight);
    t.add(Editor::go_to_prev_document_highlight);
    t.add(|editor, action, window, cx| {
        editor.go_to_definition(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_definition_split(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_declaration(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_declaration_split(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_implementation(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_implementation_split(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_type_definition(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_type_definition_split(action, window, cx).detach_and_log_err(cx);
    });
    t.add(Editor::open_url);
    t.add(Editor::open_selected_filename);
    t.add(Editor::fold);
    t.add(Editor::fold_at_level);
    t.add(Editor::fold_at_level_1);
    t.add(Editor::fold_at_level_2);
    t.add(Editor::fold_at_level_3);
    t.add(Editor::fold_at_level_4);
    t.add(Editor::fold_at_level_5);
    t.add(Editor::fold_at_level_6);
    t.add(Editor::fold_at_level_7);
    t.add(Editor::fold_at_level_8);
    t.add(Editor::fold_at_level_9);
    t.add(Editor::fold_all);
    t.add(Editor::fold_function_bodies);
    t.add(Editor::fold_recursive);
    t.add(Editor::toggle_fold);
    t.add(Editor::toggle_fold_recursive);
    t.add(Editor::toggle_fold_all);
    t.add(Editor::unfold_lines);
    t.add(Editor::unfold_recursive);
    t.add(Editor::unfold_all);
    t.add(Editor::fold_selected_ranges);
    t.add(Editor::set_mark);
    t.add(Editor::save_location);
    t.add(Editor::swap_selection_ends);
    t.add(Editor::show_completions);
    t.add(Editor::show_word_completions);
    t.add(Editor::toggle_code_actions);
    t.add(Editor::toggle_soft_wrap);
    t.add(Editor::toggle_indent_guides);

    t.add(hover_popover::hover);
    t.add(Editor::reveal_in_finder);
    t.add(Editor::copy_path);
    t.add(Editor::copy_relative_path);
    t.add(Editor::copy_file_name);
    t.add(Editor::copy_file_name_without_extension);
    t.add(Editor::copy_highlight_json);
    t.add(Editor::copy_file_location);
    t.add(Editor::toggle_git_blame);
    t.add(Editor::toggle_selected_diff_hunks);
    t.add(Editor::toggle_staged_selected_diff_hunks);
    t.add(Editor::stage_and_next);
    t.add(Editor::unstage_and_next);
    t.add(Editor::expand_all_diff_hunks);
    t.add(Editor::collapse_all_diff_hunks);
    t.add(Editor::toggle_all_diff_hunks);
    t.add(Editor::go_to_previous_change);
    t.add(Editor::go_to_next_change);
    t.add(Editor::go_to_prev_reference);
    t.add(Editor::go_to_next_reference);
    t.add(Editor::go_to_previous_symbol);
    t.add(Editor::go_to_next_symbol);
    t.add(Editor::restart_language_server);
    t.add(Editor::stop_language_server);
    t.add(Editor::show_character_palette);
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.compose_completion(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.find_all_references(action, window, cx) {
            task.detach_and_log_err(cx);
        } else {
            cx.propagate();
        }
    });

    t.add(Editor::context_menu_first);
    t.add(Editor::context_menu_prev);
    t.add(Editor::context_menu_next);
    t.add(Editor::context_menu_last);
    t.add(Editor::open_active_item_in_terminal);
    t.add(Editor::open_selections_in_multibuffer);
    t.add(Editor::reload_file);

    t.build()
});

/// Actions only editors that are not read-only handle.
static EDIT_ACTIONS: LazyLock<ActionTable> = LazyLock::new(|| {
    let mut t = ActionTableBuilder::default();
    t.add(Editor::newline);
    t.add(Editor::newline_above);
    t.add(Editor::newline_below);
    t.add(Editor::backspace);
    t.add(Editor::delete);
    t.add(Editor::tab);
    t.add(Editor::backtab);
    t.add(Editor::indent);
    t.add(Editor::outdent);
    t.add(Editor::autoindent);
    t.add(Editor::delete_line);
    t.add(Editor::join_lines);
    t.add(Editor::sort_lines_by_length);
    t.add(Editor::sort_lines_case_sensitive);
    t.add(Editor::sort_lines_case_insensitive);
    t.add(Editor::unique_lines_case_insensitive);
    t.add(Editor::unique_lines_case_sensitive);
    t.add(Editor::convert_indentation_to_spaces);
    t.add(Editor::convert_indentation_to_tabs);
    t.add(Editor::convert_to_upper_case);
    t.add(Editor::convert_to_lower_case);
    t.add(Editor::convert_to_title_case);
    t.add(Editor::convert_to_snake_case);
    t.add(Editor::convert_to_kebab_case);
    t.add(Editor::convert_to_upper_camel_case);
    t.add(Editor::convert_to_lower_camel_case);
    t.add(Editor::convert_to_opposite_case);
    t.add(Editor::convert_to_sentence_case);
    t.add(Editor::toggle_case);
    t.add(Editor::convert_to_rot13);
    t.add(Editor::convert_to_rot47);
    t.add(Editor::convert_to_base64);
    t.add(Editor::convert_from_base64);
    t.add(Editor::delete_to_previous_word_start);
    t.add(Editor::delete_to_previous_subword_start);
    t.add(Editor::delete_to_next_word_end);
    t.add(Editor::delete_to_next_subword_end);
    t.add(Editor::delete_to_beginning_of_line);
    t.add(Editor::delete_to_end_of_line);
    t.add(Editor::cut_to_end_of_line);
    t.add(Editor::duplicate_line_up);
    t.add(Editor::duplicate_line_down);
    t.add(Editor::duplicate_selection);
    t.add(Editor::move_line_up);
    t.add(Editor::move_line_down);
    t.add(Editor::transpose);
    t.add(|editor, _: &crate::Rewrap, _, cx| {
        editor.rewrap(crate::RewrapOptions::default(), cx);
    });
    t.add(Editor::cut);
    t.add(Editor::kill_ring_cut);
    t.add(Editor::kill_ring_yank);
    t.add(Editor::paste);
    t.add(Editor::undo);
    t.add(Editor::redo);
    t.add(Editor::toggle_comments);
    t.add(Editor::toggle_block_comments);
    t.add(Editor::toggle_markdown_block_quote);
    t.add(Editor::unwrap_syntax_node);

    t.add(Editor::restore_file);
    t.add(Editor::git_restore);
    t.add(Editor::restore_and_next);
    t.add(Editor::insert_uuid_v4);
    t.add(Editor::insert_uuid_v7);
    t.add(Editor::align_selections);
    t.add(|editor, HandleInput(text): &HandleInput, window, cx| {
        if text.is_empty() {
            return;
        }
        editor.handle_input(text, window, cx);
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.format(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.organize_imports(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.confirm_completion(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.confirm_completion_replace(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.confirm_completion_insert(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.confirm_code_action(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.rename(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.confirm_rename(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
    t.build()
});

type ActionHandler = Box<dyn Fn(&mut Editor, &dyn Any, &mut Window, &mut Context<Editor>) + Send + Sync>;

/// Action types (sorted) with their handlers, built once so painting an editor registers a
/// single listener instead of one per action.
pub(super) struct ActionTable {
    types: Vec<TypeId>,
    handlers: Vec<ActionHandler>,
}

#[derive(Default)]
struct ActionTableBuilder(Vec<(TypeId, ActionHandler)>);

impl ActionTableBuilder {
    fn add<T: Action>(
        &mut self,
        listener: impl Fn(&mut Editor, &T, &mut Window, &mut Context<Editor>) + Send + Sync + 'static,
    ) {
        self.0.push((
            TypeId::of::<T>(),
            Box::new(move |editor, action, window, cx| {
                listener(editor, action.downcast_ref().expect("action type matches listener"), window, cx);
            }),
        ));
    }

    fn build(mut self) -> ActionTable {
        self.0.sort_by_key(|(action_type, _)| *action_type);
        assert!(self.0.is_sorted_by(|a, b| a.0 < b.0), "action registered twice in one table");
        let (types, handlers) = self.0.into_iter().unzip();
        ActionTable { types, handlers }
    }
}

fn register_action_table(editor: &Entity<Editor>, window: &mut Window, table: &'static ActionTable) {
    let editor = editor.clone();
    window.on_action_table(&table.types, move |ix, action, window, cx| {
        editor.update(cx, |editor, cx| (table.handlers[ix])(editor, action, window, cx));
    });
}

pub(crate) fn register_action<T: Action>(
    editor: &Entity<Editor>,
    window: &mut Window,
    listener: impl Fn(&mut Editor, &T, &mut Window, &mut Context<Editor>) + 'static,
) {
    register_action_erased(
        editor,
        window,
        TypeId::of::<T>(),
        Box::new(move |editor, action, window, cx| {
            listener(editor, action.downcast_ref().expect("action type matches listener"), window, cx);
        }),
    );
}

#[allow(clippy::type_complexity)]
pub(super) fn register_action_erased(
    editor: &Entity<Editor>,
    window: &mut Window,
    action_type: TypeId,
    listener: Box<dyn Fn(&mut Editor, &dyn Any, &mut Window, &mut Context<Editor>)>,
) {
    let editor = editor.clone();
    window.on_action(action_type, move |action, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            editor.update(cx, |editor, cx| {
                listener(editor, action, window, cx);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_tables_are_sorted_and_disjoint() {
        // Building a table asserts it has no duplicate action types.
        for table in [&*ACTIONS, &*EDIT_ACTIONS] {
            assert!(table.types.is_sorted());
            assert_eq!(table.types.len(), table.handlers.len());
        }
        assert!(ACTIONS.types.iter().all(|action_type| EDIT_ACTIONS.types.binary_search(action_type).is_err()));
    }

    #[test]
    fn action_tables_split_navigation_from_editing() {
        let in_table = |table: &ActionTable, action_type: TypeId| table.types.binary_search(&action_type).is_ok();

        assert!(in_table(&ACTIONS, TypeId::of::<crate::actions::MoveLeft>()));
        assert!(in_table(&ACTIONS, TypeId::of::<crate::actions::Copy>()));
        assert!(in_table(&EDIT_ACTIONS, TypeId::of::<crate::actions::Newline>()));
        assert!(in_table(&EDIT_ACTIONS, TypeId::of::<HandleInput>()));
        assert!(!in_table(&ACTIONS, TypeId::of::<crate::actions::Newline>()));
    }

    #[test]
    fn builder_sorts_by_action_type() {
        let mut builder = ActionTableBuilder::default();
        builder.add(|_, _: &LineDown, _, _| {});
        builder.add(|_, _: &LineUp, _, _| {});
        builder.add(|_, _: &PageDown, _, _| {});
        let table = builder.build();

        assert_eq!(table.types.len(), 3);
        assert!(table.types.is_sorted());
        assert!(table.types.contains(&TypeId::of::<LineUp>()));
    }

    #[test]
    #[should_panic(expected = "action registered twice")]
    fn builder_rejects_duplicate_action_types() {
        let mut builder = ActionTableBuilder::default();
        builder.add(|_, _: &LineDown, _, _| {});
        builder.add(|_, _: &LineDown, _, _| {});
        builder.build();
    }
}
