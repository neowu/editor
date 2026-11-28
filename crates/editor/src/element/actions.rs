use std::rc::Rc;
use std::sync::OnceLock;

use gpui::Action;
use gpui::ActionTable;
use gpui::App;
use gpui::Context;
use gpui::DispatchPhase;
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
    pub(super) fn register_actions(&self, window: &mut Window, cx: &App) {
        window.on_action_table(&self.editor, actions(), cx);
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

static ACTIONS: OnceLock<ActionTable<Editor>> = OnceLock::new();

/// Makes `table` the actions every editor handles, followed by the editor's own. Startup hands
/// the table to each crate that adds editor actions, then installs it before any editor paints.
/// Handlers for the same action run in registration order, so before the editor's own.
pub fn install_actions(mut table: ActionTable<Editor>) {
    add_built_in_actions(&mut table);
    assert!(ACTIONS.set(table).is_ok(), "editor actions are installed once, before any editor paints");
}

fn actions() -> &'static ActionTable<Editor> {
    // Only tests get here without `install_actions`; they handle the editor's own actions.
    ACTIONS.get_or_init(|| {
        let mut table = ActionTable::default();
        add_built_in_actions(&mut table);
        table
    })
}

fn add_built_in_actions(t: &mut ActionTable<Editor>) {
    add_actions(t);
    add_edit_actions(&mut Editable(t));
}

fn editable(editor: &Editor, cx: &App) -> bool {
    !editor.read_only(cx)
}

/// Adds handlers that only apply to editors that are not read-only.
struct Editable<'a>(&'a mut ActionTable<Editor>);

impl Editable<'_> {
    fn add<A: Action>(
        &mut self,
        handler: impl Fn(&mut Editor, &A, &mut Window, &mut Context<Editor>) + Send + Sync + 'static,
    ) {
        self.0.add_if(editable, handler);
    }
}

/// Actions every editor handles, some only in a certain state.
fn add_actions(t: &mut ActionTable<Editor>) {
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
        editor.scroll_screen_with_cursor_margin(ScrollAmount::Line(1.), window, cx);
    });
    t.add(|editor, _: &LineUp, window, cx| {
        editor.scroll_screen_with_cursor_margin(ScrollAmount::Line(-1.), window, cx);
    });
    t.add(|editor, _: &HalfPageDown, window, cx| {
        editor.scroll_screen_with_cursor_margin(ScrollAmount::Page(0.5), window, cx);
    });
    t.add(|editor, _: &HalfPageUp, window, cx| {
        editor.scroll_screen_with_cursor_margin(ScrollAmount::Page(-0.5), window, cx);
    });
    t.add(|editor, _: &PageDown, window, cx| {
        editor.scroll_screen_with_cursor_margin(ScrollAmount::Page(1.), window, cx);
    });
    t.add(|editor, _: &PageUp, window, cx| {
        editor.scroll_screen_with_cursor_margin(ScrollAmount::Page(-1.), window, cx);
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
        editor.go_to_implementation(action, window, cx).detach_and_log_err(cx);
    });
    t.add(|editor, action, window, cx| {
        editor.go_to_type_definition(action, window, cx).detach_and_log_err(cx);
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
    t.add(Editor::toggle_test_references);
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

    let is_multibuffer = |editor: &Editor, cx: &App| editor.buffer_kind(cx) == ItemBufferKind::Multibuffer;
    t.add_if(is_multibuffer, Editor::expand_excerpts);
    t.add_if(is_multibuffer, Editor::expand_excerpts_up);
    t.add_if(is_multibuffer, Editor::expand_excerpts_down);
    t.add_if(Editor::can_accept_inlay_hint, Editor::accept_inlay_hint);
    t.add_if(|editor, _| editor.lsp_data_enabled(), Editor::toggle_diagnostics);
    t.add_if(Editor::supports_minimap, Editor::toggle_minimap);
    // The blame handlers propagate when the cursor is not on a blamed line.
    let has_blame = |editor: &Editor, _: &App| editor.blame().is_some();
    t.add_if(has_blame, Editor::open_git_blame_commit);
    t.add_if(has_blame, Editor::blame_revision);
    t.add_if(has_blame, Editor::blame_previous_revision);
    t.add_if(has_rust_buffer, rust_analyzer_ext::go_to_related_tests);
    t.add_if(has_rust_buffer, rust_analyzer_ext::view_hir);
    t.add_if(has_rust_buffer, rust_analyzer_ext::view_mir);
    t.add_if(has_rust_buffer, rust_analyzer_ext::view_memory_layout);
    t.add_if(has_rust_buffer, rust_analyzer_ext::expand_macro_recursively);
    t.add_if(has_rust_buffer, rust_analyzer_ext::open_docs);
}

/// Actions only editors that are not read-only handle.
fn add_edit_actions(t: &mut Editable) {
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

    // Whether these two can do anything is costly to tell per frame, so they are gated like any
    // edit and propagate when they have nothing to do.
    t.add(Editor::wrap_selections_in_tag);
    t.add(|editor, action, window, cx| {
        if let Some(task) = editor.format_selections(action, window, cx) {
            editor.detach_and_notify_err(task, window, cx);
        } else {
            cx.propagate();
        }
    });
}

fn has_rust_buffer(editor: &Editor, cx: &App) -> bool {
    editor
        .buffer()
        .read(cx)
        .all_buffers_iter()
        .any(|buffer| buffer.read(cx).language().is_some_and(|language| rust_analyzer_ext::is_rust_language(language)))
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;
    use crate::actions;

    fn built_in() -> ActionTable<Editor> {
        let mut table = ActionTable::default();
        add_built_in_actions(&mut table);
        table
    }

    /// Whether the table's only handler for `A` carries a condition.
    fn conditional<A: Action>(table: &ActionTable<Editor>) -> bool {
        let ix = table.action_types().binary_search(&TypeId::of::<A>()).expect("action is handled");
        table.conditional().nth(ix).expect("one condition slot per handler")
    }

    #[test]
    fn built_in_actions_handle_each_action_once() {
        assert!(built_in().action_types().is_sorted_by(|a, b| a < b), "action handled twice");
    }

    #[test]
    fn navigation_applies_to_every_editor() {
        let table = built_in();

        assert!(!conditional::<actions::MoveLeft>(&table));
        assert!(!conditional::<actions::Copy>(&table));
        assert!(!conditional::<actions::GoToDefinition>(&table));
    }

    #[test]
    fn editing_depends_on_the_editor_being_editable() {
        let table = built_in();

        assert!(conditional::<actions::Newline>(&table));
        assert!(conditional::<HandleInput>(&table));
        assert!(conditional::<actions::Paste>(&table));
        assert!(conditional::<actions::WrapSelectionsInTag>(&table));
        assert!(conditional::<actions::FormatSelections>(&table));
    }

    #[test]
    fn state_dependent_actions_are_conditional() {
        let table = built_in();

        assert!(conditional::<actions::ExpandExcerpts>(&table));
        assert!(conditional::<actions::AcceptInlayHint>(&table));
        assert!(conditional::<actions::ToggleDiagnostics>(&table));
        assert!(conditional::<actions::ToggleMinimap>(&table));
        assert!(conditional::<actions::BlameRevision>(&table));
        assert!(conditional::<actions::GoToRelatedTests>(&table));
    }

    #[test]
    fn built_in_handlers_follow_registered_ones() {
        let mut table = ActionTable::default();
        table.add_if(|_, _| false, |_, _: &actions::GoToDefinition, _, _| {});
        add_built_in_actions(&mut table);

        let definition = TypeId::of::<actions::GoToDefinition>();
        let handlers: Vec<bool> = table
            .action_types()
            .iter()
            .zip(table.conditional())
            .filter_map(|(action_type, conditional)| (*action_type == definition).then_some(conditional))
            .collect();
        // The registered (conditional) handler runs before the editor's own.
        assert_eq!(handlers, [true, false]);
        assert_eq!(table.action_types().len(), built_in().action_types().len() + 1);
    }
}
