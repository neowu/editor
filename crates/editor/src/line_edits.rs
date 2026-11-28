use std::borrow::Cow;
use std::cmp;
use std::iter::Peekable;
use std::num::NonZeroU32;
use std::ops::Range;
use std::slice::Iter;
use std::sync::Arc;

use gpui::App;
use gpui::Context;
use gpui::Window;
use language::Buffer;
use language::IndentKind;
use language::IndentSize;
use language::Point;
use language::Selection;
use language::SelectionGoal;
use multi_buffer::MultiBuffer;
use multi_buffer::MultiBufferOffset;
use multi_buffer::MultiBufferPoint;
use multi_buffer::MultiBufferRow;
use multi_buffer::MultiBufferSnapshot;
use multi_buffer::ToOffset;
use multi_buffer::ToPoint as _;
use rustc_hash::FxHashSet;
use text::Bias;

use super::Editor;
use super::MAX_LINE_LEN;
use super::RowExt as _;
use super::RowRangeExt as _;
use super::SelectionEffects;
use super::SelectionExt as _;
use crate::actions::AutoIndent;
use crate::actions::Backspace;
use crate::actions::Backtab;
use crate::actions::ConvertIndentationToSpaces;
use crate::actions::ConvertIndentationToTabs;
use crate::actions::Delete;
use crate::actions::DeleteLine;
use crate::actions::DuplicateLineDown;
use crate::actions::DuplicateLineUp;
use crate::actions::DuplicateSelection;
use crate::actions::Indent;
use crate::actions::JoinLines;
use crate::actions::MoveLineDown;
use crate::actions::MoveLineUp;
use crate::actions::Outdent;
use crate::actions::SelectAll;
use crate::actions::SortLinesByLength;
use crate::actions::SortLinesCaseInsensitive;
use crate::actions::SortLinesCaseSensitive;
use crate::actions::Tab;
use crate::actions::Transpose;
use crate::actions::UniqueLinesCaseInsensitive;
use crate::actions::UniqueLinesCaseSensitive;
use crate::display_map::Crease;
use crate::display_map::DisplayPoint;
use crate::display_map::DisplaySnapshot;
use crate::display_map::ToDisplayPoint as _;
use crate::input;
use crate::linked_editing_ranges::refresh_linked_ranges;
use crate::movement;
use crate::scroll::Autoscroll;

impl Editor {
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transact(window, cx, |this, window, cx| {
            this.select_all(&SelectAll, window, cx);
            this.insert("", window, cx);
        });
    }

    pub(crate) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        self.transact(window, cx, |this, window, cx| {
            this.select_autoclose_pair(window, cx);

            let linked_edits = this.linked_edits_for_selections(Arc::from(""), cx);

            let display_map = this.display_map.update(cx, |map, cx| map.snapshot(cx));
            let mut selections = this.selections.all::<MultiBufferPoint>(&display_map);
            for selection in &mut selections {
                if selection.is_empty() {
                    let old_head = selection.head();
                    let mut new_head =
                        movement::left(&display_map, old_head.to_display_point(&display_map)).to_point(&display_map);
                    if let Some((buffer, line_buffer_range)) =
                        display_map.buffer_snapshot().buffer_line_for_row(MultiBufferRow(old_head.row))
                    {
                        let indent_size = buffer.indent_size_for_line(line_buffer_range.start.row);
                        let indent_len = match indent_size.kind {
                            IndentKind::Space => buffer.settings_at(line_buffer_range.start, cx).tab_size,
                            IndentKind::Tab => NonZeroU32::new(1).expect("1 is non-zero"),
                        };
                        if old_head.column <= indent_size.len && old_head.column > 0 {
                            let indent_len = indent_len.get();
                            new_head = cmp::min(
                                new_head,
                                MultiBufferPoint::new(old_head.row, ((old_head.column - 1) / indent_len) * indent_len),
                            );
                        }
                    }

                    selection.set_head(new_head, SelectionGoal::None);
                }
            }

            this.change_selections(SelectionEffects::default(), window, cx, |s| s.select(selections));
            this.insert("", window, cx);
            linked_edits.apply_with_left_expansion(cx);

            refresh_linked_ranges(this, window, cx);
        });
    }

    pub(crate) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        self.transact(window, cx, |this, window, cx| {
            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.move_with(&mut |map, selection| {
                    if selection.is_empty() {
                        let cursor = movement::right(map, selection.head());
                        selection.end = cursor;
                        selection.reversed = true;
                        selection.goal = SelectionGoal::None;
                    }
                });
            });
            let linked_edits = this.linked_edits_for_selections(Arc::from(""), cx);
            this.insert("", window, cx);
            linked_edits.apply(cx);

            refresh_linked_ranges(this, window, cx);
        });
    }

    pub(crate) fn backtab(&mut self, _: &Backtab, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        if self.move_to_prev_snippet_tabstop(window, cx) {
            return;
        }
        self.outdent(&Outdent, window, cx);
    }

    pub(crate) fn tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        if self.move_to_next_snippet_tabstop(window, cx) {
            return;
        }
        if self.read_only(cx) {
            return;
        }
        let mut selections = self.selections.all_adjusted(&self.display_snapshot(cx));
        let buffer = self.buffer.read(cx);
        let snapshot = buffer.snapshot(cx);
        let rows_iter = selections.iter().map(|s| s.head().row);
        let suggested_indents = snapshot.suggested_indents(rows_iter, cx);

        let has_some_cursor_in_whitespace =
            selections.iter().filter(|selection| selection.is_empty()).any(|selection| {
                let cursor = selection.head();
                let current_indent = snapshot.indent_size_for_line(MultiBufferRow(cursor.row));
                cursor.column < current_indent.len
            });

        let mut edits = Vec::new();
        let mut prev_edited_row = 0;
        let mut row_delta = 0;
        for selection in &mut selections {
            if selection.start.row != prev_edited_row {
                row_delta = 0;
            }
            prev_edited_row = selection.end.row;

            // If cursor is after a list prefix, make selection non-empty to trigger line indent
            if selection.is_empty() {
                let cursor = selection.head();
                if let Some(language) = snapshot.language_scope_at(Point::new(cursor.row, 0))
                    && input::is_list_prefix_row(MultiBufferRow(cursor.row), &snapshot, &language)
                {
                    row_delta = Self::indent_selection(buffer, &snapshot, selection, &mut edits, row_delta, cx);
                    continue;
                }
            }

            // If the selection is non-empty, then increase the indentation of the selected lines.
            if !selection.is_empty() {
                row_delta = Self::indent_selection(buffer, &snapshot, selection, &mut edits, row_delta, cx);
                continue;
            }

            let cursor = selection.head();
            let current_indent = snapshot.indent_size_for_line(MultiBufferRow(cursor.row));
            if let Some(suggested_indent) = suggested_indents.get(&MultiBufferRow(cursor.row)).copied() {
                // Don't do anything if already at suggested indent
                // and there is any other cursor which is not
                if has_some_cursor_in_whitespace
                    && cursor.column == current_indent.len
                    && current_indent.len == suggested_indent.len
                {
                    continue;
                }

                // Adjust line and move cursor to suggested indent
                // if cursor is not at suggested indent
                if cursor.column < suggested_indent.len
                    && cursor.column <= current_indent.len
                    && current_indent.len <= suggested_indent.len
                {
                    selection.start = Point::new(cursor.row, suggested_indent.len);
                    selection.end = selection.start;
                    if row_delta == 0 {
                        edits.extend(Buffer::edit_for_indent_size_adjustment(
                            cursor.row,
                            current_indent,
                            suggested_indent,
                        ));
                        row_delta = suggested_indent.len - current_indent.len;
                    }
                    continue;
                }

                // If current indent is more than suggested indent
                // only move cursor to current indent and skip indent
                if cursor.column < current_indent.len && current_indent.len > suggested_indent.len {
                    selection.start = Point::new(cursor.row, current_indent.len);
                    selection.end = selection.start;
                    continue;
                }
            }

            // Otherwise, insert a hard or soft tab.
            let settings = buffer.language_settings_at(cursor, cx);
            let tab_size = if settings.hard_tabs {
                IndentSize::tab()
            } else {
                let tab_size = settings.tab_size.get();
                let indent_remainder = snapshot
                    .text_for_range(Point::new(cursor.row, 0)..cursor)
                    .flat_map(str::chars)
                    .fold(row_delta % tab_size, |counter: u32, c| if c == '\t' { 0 } else { (counter + 1) % tab_size });

                let chars_to_next_tab_stop = tab_size - indent_remainder;
                IndentSize::spaces(chars_to_next_tab_stop)
            };
            selection.start = Point::new(cursor.row, cursor.column + row_delta + tab_size.len);
            selection.end = selection.start;
            edits.push((cursor..cursor, tab_size.chars().collect::<String>()));
            row_delta += tab_size.len;
        }

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |b, cx| b.edit(edits, None, cx));
            this.change_selections(SelectionEffects::default(), window, cx, |s| s.select(selections));
        });
    }

    pub(crate) fn indent(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        let mut selections = self.selections.all::<Point>(&self.display_snapshot(cx));
        let mut prev_edited_row = 0;
        let mut row_delta = 0;
        let mut edits = Vec::new();
        let buffer = self.buffer.read(cx);
        let snapshot = buffer.snapshot(cx);
        for selection in &mut selections {
            if selection.start.row != prev_edited_row {
                row_delta = 0;
            }
            prev_edited_row = selection.end.row;

            row_delta = Self::indent_selection(buffer, &snapshot, selection, &mut edits, row_delta, cx);
        }

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |b, cx| b.edit(edits, None, cx));
            this.change_selections(SelectionEffects::default(), window, cx, |s| s.select(selections));
        });
    }

    fn indent_selection(
        buffer: &MultiBuffer,
        snapshot: &MultiBufferSnapshot,
        selection: &mut Selection<Point>,
        edits: &mut Vec<(Range<Point>, String)>,
        delta_for_start_row: u32,
        cx: &App,
    ) -> u32 {
        let settings = buffer.language_settings_at(selection.start, cx);
        let tab_size = settings.tab_size.get();
        let indent_kind = if settings.hard_tabs { IndentKind::Tab } else { IndentKind::Space };
        let mut start_row = selection.start.row;
        let mut end_row = selection.end.row + 1;

        // If a selection ends at the beginning of a line, don't indent
        // that last line.
        if selection.end.column == 0 && selection.end.row > selection.start.row {
            end_row -= 1;
        }

        // Avoid re-indenting a row that has already been indented by a
        // previous selection, but still update this selection's column
        // to reflect that indentation.
        if delta_for_start_row > 0 {
            start_row += 1;
            selection.start.column += delta_for_start_row;
            if selection.end.row == selection.start.row {
                selection.end.column += delta_for_start_row;
            }
        }

        let mut delta_for_end_row = 0;
        let has_multiple_rows = start_row + 1 != end_row;
        for row in start_row..end_row {
            let current_indent = snapshot.indent_size_for_line(MultiBufferRow(row));
            let indent_delta = match (current_indent.kind, indent_kind) {
                (IndentKind::Space, IndentKind::Space) => {
                    let columns_to_next_tab_stop = if delta_for_start_row > 0 {
                        delta_for_start_row
                    } else if has_multiple_rows {
                        tab_size
                    } else {
                        tab_size - (current_indent.len % tab_size)
                    };
                    IndentSize::spaces(columns_to_next_tab_stop)
                }
                (IndentKind::Tab, IndentKind::Space) => IndentSize::spaces(tab_size),
                (_, IndentKind::Tab) => IndentSize::tab(),
            };

            let start = if has_multiple_rows || current_indent.len < selection.start.column {
                0
            } else {
                selection.start.column
            };
            let row_start = Point::new(row, start);
            edits.push((row_start..row_start, indent_delta.chars().collect::<String>()));

            // Update this selection's endpoints to reflect the indentation.
            if row == selection.start.row {
                selection.start.column += indent_delta.len;
            }
            if row == selection.end.row {
                selection.end.column += indent_delta.len;
                delta_for_end_row = indent_delta.len;
            }
        }

        if selection.start.row == selection.end.row {
            delta_for_start_row + delta_for_end_row
        } else {
            delta_for_end_row
        }
    }

    pub(crate) fn outdent(&mut self, _: &Outdent, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        let display_map = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let selections = self.selections.all::<Point>(&display_map);
        let mut deletion_ranges = Vec::new();
        let mut last_outdent = None;
        {
            let buffer = self.buffer.read(cx);
            let snapshot = buffer.snapshot(cx);
            for selection in &selections {
                let settings = buffer.language_settings_at(selection.start, cx);
                let tab_size = settings.tab_size;
                let mut rows = selection.spanned_rows(false, &display_map);

                // Avoid re-outdenting a row that has already been outdented by a
                // previous selection.
                if let Some(last_row) = last_outdent
                    && last_row == rows.start
                {
                    rows.start = rows.start.next_row();
                }
                let has_multiple_rows = rows.len() > 1;
                for row in rows.iter_rows() {
                    let indent_size = snapshot.indent_size_for_line(row);
                    if indent_size.len > 0 {
                        let deletion_len = indent_size.outdent_len(tab_size);
                        let start = if has_multiple_rows
                            || deletion_len > selection.start.column
                            || indent_size.len < selection.start.column
                        {
                            0
                        } else {
                            selection.start.column - deletion_len
                        };
                        deletion_ranges.push(Point::new(row.0, start)..Point::new(row.0, start + deletion_len));
                        last_outdent = Some(row);
                    }
                }
            }
        }

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |buffer, cx| {
                let empty_str: Arc<str> = Arc::default();
                buffer.edit(deletion_ranges.into_iter().map(|range| (range, Arc::clone(&empty_str))), None, cx);
            });
            let selections = this.selections.all::<MultiBufferOffset>(&this.display_snapshot(cx));
            this.change_selections(SelectionEffects::default(), window, cx, |s| s.select(selections));
        });
    }

    pub(crate) fn autoindent(&mut self, _: &AutoIndent, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        let selections =
            self.selections.all::<MultiBufferOffset>(&self.display_snapshot(cx)).into_iter().map(|s| s.range());

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |buffer, cx| {
                buffer.autoindent_ranges(selections, cx);
            });
            let selections = this.selections.all::<MultiBufferOffset>(&this.display_snapshot(cx));
            this.change_selections(SelectionEffects::default(), window, cx, |s| s.select(selections));
        });
    }

    pub(crate) fn delete_line(&mut self, _: &DeleteLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let display_map = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let selections = self.selections.all::<Point>(&display_map);

        let mut new_cursors = Vec::new();
        let mut edit_ranges = Vec::new();
        let mut selections = selections.iter().peekable();
        while let Some(selection) = selections.next() {
            let mut rows = selection.spanned_rows(false, &display_map);

            // Accumulate contiguous regions of rows that we want to delete.
            while let Some(next_selection) = selections.peek() {
                let next_rows = next_selection.spanned_rows(false, &display_map);
                if next_rows.start <= rows.end {
                    rows.end = next_rows.end;
                    selections.next();
                } else {
                    break;
                }
            }

            let buffer = display_map.buffer_snapshot();
            let mut edit_start = ToOffset::to_offset(&Point::new(rows.start.0, 0), buffer);
            let (edit_end, target_row) = if buffer.max_point().row >= rows.end.0 {
                // If there's a line after the range, delete the \n from the end of the row range
                (ToOffset::to_offset(&Point::new(rows.end.0, 0), buffer), rows.end)
            } else {
                // If there isn't a line after the range, delete the \n from the line before the
                // start of the row range
                edit_start = edit_start.saturating_sub_usize(1);
                (buffer.len(), rows.start.previous_row())
            };

            let text_layout_details = self.text_layout_details(window, cx);
            let x =
                display_map.x_for_display_point(selection.head().to_display_point(&display_map), &text_layout_details);
            let row = Point::new(target_row.0, 0).to_display_point(&display_map).row();
            let column = display_map.display_column_for_x(row, x, &text_layout_details);

            new_cursors.push((
                selection.id,
                buffer.anchor_after(DisplayPoint::new(row, column).to_point(&display_map)),
                SelectionGoal::None,
            ));
            edit_ranges.push(edit_start..edit_end);
        }

        self.transact(window, cx, |this, window, cx| {
            let buffer = this.buffer.update(cx, |buffer, cx| {
                let empty_str: Arc<str> = Arc::default();
                buffer.edit(edit_ranges.into_iter().map(|range| (range, Arc::clone(&empty_str))), None, cx);
                buffer.snapshot(cx)
            });
            let new_selections = new_cursors
                .into_iter()
                .map(|(id, cursor, goal)| {
                    let cursor = cursor.to_point(&buffer);
                    Selection { id, start: cursor, end: cursor, reversed: false, goal }
                })
                .collect();

            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select(new_selections);
            });
        });
    }

    fn join_lines_impl(&mut self, insert_whitespace: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let mut row_ranges = Vec::<Range<MultiBufferRow>>::new();
        for selection in self.selections.all::<Point>(&self.display_snapshot(cx)) {
            let start = MultiBufferRow(selection.start.row);
            // Treat single line selections as if they include the next line. Otherwise this action
            // would do nothing for single line selections individual cursors.
            let end = if selection.start.row == selection.end.row {
                MultiBufferRow(selection.start.row + 1)
            } else if selection.end.column == 0 {
                // If the selection ends at the start of a line, it's logically at the end of the
                // previous line (plus its newline).
                // Don't include the end line unless there's only one line selected.
                if selection.start.row + 1 == selection.end.row {
                    MultiBufferRow(selection.end.row)
                } else {
                    MultiBufferRow(selection.end.row - 1)
                }
            } else {
                MultiBufferRow(selection.end.row)
            };

            if let Some(last_row_range) = row_ranges.last_mut()
                && start <= last_row_range.end
            {
                last_row_range.end = end;
                continue;
            }
            row_ranges.push(start..end);
        }

        let snapshot = self.buffer.read(cx).snapshot(cx);
        let mut cursor_positions = Vec::new();
        for row_range in &row_ranges {
            let anchor = snapshot.anchor_before(Point::new(
                row_range.end.previous_row().0,
                snapshot.line_len(row_range.end.previous_row()),
            ));
            cursor_positions.push(anchor..anchor);
        }

        self.transact(window, cx, |this, window, cx| {
            for row_range in row_ranges.into_iter().rev() {
                for row in row_range.iter_rows().rev() {
                    let end_of_line = Point::new(row.0, snapshot.line_len(row));
                    let next_line_row = row.next_row();
                    let indent = snapshot.indent_size_for_line(next_line_row);
                    let mut join_start_column = indent.len;

                    if let Some(language_scope) = snapshot.language_scope_at(Point::new(next_line_row.0, indent.len)) {
                        let line_end = Point::new(next_line_row.0, snapshot.line_len(next_line_row));
                        let line_text_after_indent = snapshot
                            .text_for_range(Point::new(next_line_row.0, indent.len)..line_end)
                            .collect::<String>();

                        if !line_text_after_indent.is_empty() {
                            let block_prefixes = language_scope
                                .block_comment()
                                .into_iter()
                                .chain(language_scope.documentation_comment())
                                .filter(|comment| {
                                    language_scope.override_name() == Some("comment")
                                        && !comment.prefix.is_empty()
                                        && !line_text_after_indent.starts_with(comment.end.as_ref())
                                })
                                .map(|comment| comment.prefix.as_ref());
                            let comment_prefixes = language_scope
                                .line_comment_prefixes()
                                .iter()
                                .map(|p| p.as_ref())
                                .chain(block_prefixes)
                                .map(|prefix| (prefix, false));
                            let all_prefixes = comment_prefixes
                                .chain(language_scope.unordered_list().iter().map(|prefix| (prefix.as_ref(), true)));

                            let mut longest_prefix_len = None;
                            for (prefix, is_unordered_list) in all_prefixes {
                                let trimmed = prefix.trim_end();
                                let matches_full_prefix = line_text_after_indent.starts_with(prefix);
                                let nextline_is_bare_prefix = line_text_after_indent == trimmed;
                                if matches_full_prefix
                                    || (!is_unordered_list && line_text_after_indent.starts_with(trimmed))
                                    || nextline_is_bare_prefix
                                {
                                    let candidate_len = if matches_full_prefix { prefix.len() } else { trimmed.len() };
                                    if longest_prefix_len.is_none_or(|len| candidate_len > len) {
                                        longest_prefix_len = Some(candidate_len);
                                    }
                                }
                            }

                            if let Some(prefix_len) = longest_prefix_len {
                                join_start_column = join_start_column.saturating_add(prefix_len as u32);
                            }
                        }
                    }

                    let start_of_next_line = Point::new(next_line_row.0, join_start_column);

                    let replace = if snapshot.line_len(next_line_row) > join_start_column && insert_whitespace {
                        " "
                    } else {
                        ""
                    };

                    this.buffer
                        .update(cx, |buffer, cx| buffer.edit([(end_of_line..start_of_next_line, replace)], None, cx));
                }
            }

            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select_anchor_ranges(cursor_positions);
            });
        });
    }

    pub(crate) fn join_lines(&mut self, _: &JoinLines, window: &mut Window, cx: &mut Context<Self>) {
        self.join_lines_impl(true, window, cx);
    }

    pub(crate) fn sort_lines_case_sensitive(
        &mut self,
        _: &SortLinesCaseSensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_immutable_lines(window, cx, |lines| lines.sort_unstable());
    }

    pub(crate) fn sort_lines_by_length(&mut self, _: &SortLinesByLength, window: &mut Window, cx: &mut Context<Self>) {
        self.manipulate_immutable_lines(window, cx, |lines| lines.sort_by_key(|&line| line.chars().count()));
    }

    pub(crate) fn sort_lines_case_insensitive(
        &mut self,
        _: &SortLinesCaseInsensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_immutable_lines(window, cx, |lines| lines.sort_by_key(|line| line.to_lowercase()));
    }

    pub(crate) fn unique_lines_case_insensitive(
        &mut self,
        _: &UniqueLinesCaseInsensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_immutable_lines(window, cx, |lines| {
            let mut seen = FxHashSet::default();
            lines.retain(|line| seen.insert(line.to_lowercase()));
        });
    }

    pub(crate) fn unique_lines_case_sensitive(
        &mut self,
        _: &UniqueLinesCaseSensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_immutable_lines(window, cx, |lines| {
            let mut seen = FxHashSet::default();
            lines.retain(|line| seen.insert(*line));
        });
    }

    fn manipulate_lines<M>(&mut self, window: &mut Window, cx: &mut Context<Self>, mut manipulate: M)
    where
        M: FnMut(&str) -> LineManipulationResult,
    {
        if self.read_only(cx) {
            return;
        }

        let display_map = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let buffer = self.buffer.read(cx).snapshot(cx);

        let mut edits = Vec::new();

        let selections = self.selections.all::<Point>(&display_map);
        let mut selections = selections.iter().peekable();
        let mut contiguous_row_selections = Vec::new();
        let mut new_selections = Vec::new();
        let mut added_lines = 0;
        let mut removed_lines = 0;

        while let Some(selection) = selections.next() {
            let (start_row, end_row) =
                consume_contiguous_rows(&mut contiguous_row_selections, selection, &display_map, &mut selections);

            let start_point = Point::new(start_row.0, 0);
            let end_point = Point::new(end_row.previous_row().0, buffer.line_len(end_row.previous_row()));
            let text = buffer.text_for_range(start_point..end_point).collect::<String>();

            let LineManipulationResult { new_text, line_count_before, line_count_after } = manipulate(&text);

            edits.push((start_point..end_point, new_text));

            // Selections must change based on added and removed line count
            let start_row = MultiBufferRow(start_point.row + added_lines as u32 - removed_lines as u32);
            let end_row = MultiBufferRow(start_row.0 + line_count_after.saturating_sub(1) as u32);
            new_selections.push(Selection {
                id: selection.id,
                start: start_row,
                end: end_row,
                goal: SelectionGoal::None,
                reversed: selection.reversed,
            });

            if line_count_after > line_count_before {
                added_lines += line_count_after - line_count_before;
            } else if line_count_before > line_count_after {
                removed_lines += line_count_before - line_count_after;
            }
        }

        self.transact(window, cx, |this, window, cx| {
            let buffer = this.buffer.update(cx, |buffer, cx| {
                buffer.edit(edits, None, cx);
                buffer.snapshot(cx)
            });

            // Recalculate offsets on newly edited buffer
            let new_selections = new_selections
                .iter()
                .map(|s| {
                    let start_point = Point::new(s.start.0, 0);
                    let end_point = Point::new(s.end.0, buffer.line_len(s.end));
                    Selection {
                        id: s.id,
                        start: buffer.point_to_offset(start_point),
                        end: buffer.point_to_offset(end_point),
                        goal: s.goal,
                        reversed: s.reversed,
                    }
                })
                .collect();

            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select(new_selections);
            });

            this.request_autoscroll(Autoscroll::fit(), cx);
        });
    }

    fn manipulate_immutable_lines<Fn>(&mut self, window: &mut Window, cx: &mut Context<Self>, mut callback: Fn)
    where
        Fn: FnMut(&mut Vec<&str>),
    {
        self.manipulate_lines(window, cx, |text| {
            let mut lines: Vec<&str> = text.split('\n').collect();
            let line_count_before = lines.len();

            callback(&mut lines);

            LineManipulationResult { new_text: lines.join("\n"), line_count_before, line_count_after: lines.len() }
        });
    }

    pub(super) fn manipulate_mutable_lines<Fn>(&mut self, window: &mut Window, cx: &mut Context<Self>, mut callback: Fn)
    where
        Fn: FnMut(&mut Vec<Cow<'_, str>>),
    {
        self.manipulate_lines(window, cx, |text| {
            let mut lines: Vec<Cow<str>> = text.split('\n').map(Cow::from).collect();
            let line_count_before = lines.len();

            callback(&mut lines);

            LineManipulationResult { new_text: lines.join("\n"), line_count_before, line_count_after: lines.len() }
        });
    }

    pub(crate) fn convert_indentation_to_spaces(
        &mut self,
        _: &ConvertIndentationToSpaces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.buffer.read(cx).language_settings(cx);
        let tab_size = settings.tab_size.get() as usize;

        self.manipulate_mutable_lines(window, cx, |lines| {
            // Allocates a reasonably sized scratch buffer once for the whole loop
            let mut reindented_line = String::with_capacity(MAX_LINE_LEN);
            // Avoids recomputing spaces that could be inserted many times
            let space_cache: Vec<Vec<char>> =
                (1..=tab_size).map(|n| IndentSize::spaces(n as u32).chars().collect()).collect();

            for line in lines.iter_mut().filter(|line| !line.is_empty()) {
                let mut chars = line.as_ref().chars();
                let mut col = 0;
                let mut changed = false;

                for ch in chars.by_ref() {
                    match ch {
                        ' ' => {
                            reindented_line.push(' ');
                            col += 1;
                        }
                        '\t' => {
                            // \t are converted to spaces depending on the current column
                            let spaces_len = tab_size - (col % tab_size);
                            reindented_line.extend(&space_cache[spaces_len - 1]);
                            col += spaces_len;
                            changed = true;
                        }
                        _ => {
                            // If we dont append before break, the character is consumed
                            reindented_line.push(ch);
                            break;
                        }
                    }
                }

                if !changed {
                    reindented_line.clear();
                    continue;
                }
                // Append the rest of the line and replace old reference with new one
                reindented_line.extend(chars);
                *line = Cow::Owned(reindented_line.clone());
                reindented_line.clear();
            }
        });
    }

    pub(crate) fn convert_indentation_to_tabs(
        &mut self,
        _: &ConvertIndentationToTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.buffer.read(cx).language_settings(cx);
        let tab_size = settings.tab_size.get() as usize;

        self.manipulate_mutable_lines(window, cx, |lines| {
            // Allocates a reasonably sized buffer once for the whole loop
            let mut reindented_line = String::with_capacity(MAX_LINE_LEN);
            // Avoids recomputing spaces that could be inserted many times
            let space_cache: Vec<Vec<char>> =
                (1..=tab_size).map(|n| IndentSize::spaces(n as u32).chars().collect()).collect();

            for line in lines.iter_mut().filter(|line| !line.is_empty()) {
                let mut chars = line.chars();
                let mut spaces_count = 0;
                let mut first_non_indent_char = None;
                let mut changed = false;

                for ch in chars.by_ref() {
                    match ch {
                        ' ' => {
                            // Keep track of spaces. Append \t when we reach tab_size
                            spaces_count += 1;
                            changed = true;
                            if spaces_count == tab_size {
                                reindented_line.push('\t');
                                spaces_count = 0;
                            }
                        }
                        '\t' => {
                            reindented_line.push('\t');
                            spaces_count = 0;
                        }
                        _ => {
                            // Dont append it yet, we might have remaining spaces
                            first_non_indent_char = Some(ch);
                            break;
                        }
                    }
                }

                if !changed {
                    reindented_line.clear();
                    continue;
                }
                // Remaining spaces that didn't make a full tab stop
                if spaces_count > 0 {
                    reindented_line.extend(&space_cache[spaces_count - 1]);
                }
                // If we consume an extra character that was not indentation, add it back
                if let Some(extra_char) = first_non_indent_char {
                    reindented_line.push(extra_char);
                }
                // Append the rest of the line and replace old reference with new one
                reindented_line.extend(chars);
                *line = Cow::Owned(reindented_line.clone());
                reindented_line.clear();
            }
        });
    }

    fn duplicate(&mut self, upwards: bool, whole_lines: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }

        let display_map = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let buffer = display_map.buffer_snapshot();
        let selections = self.selections.all::<Point>(&display_map);

        let mut edits = Vec::new();
        let mut selections_iter = selections.iter().peekable();
        while let Some(selection) = selections_iter.next() {
            let mut rows = selection.spanned_rows(false, &display_map);
            // duplicate line-wise
            if whole_lines || selection.start == selection.end {
                // Avoid duplicating the same lines twice.
                while let Some(next_selection) = selections_iter.peek() {
                    let next_rows = next_selection.spanned_rows(false, &display_map);
                    if next_rows.start < rows.end {
                        rows.end = next_rows.end;
                        selections_iter.next();
                    } else {
                        break;
                    }
                }

                // Copy the text from the selected row region and splice it either at the start
                // or end of the region.
                let start = Point::new(rows.start.0, 0);
                let end = Point::new(rows.end.previous_row().0, buffer.line_len(rows.end.previous_row()));

                let mut text = buffer.text_for_range(start..end).collect::<String>();

                let insert_location = if upwards {
                    // When duplicating upward, we need to insert before the current line.
                    // If we're on the last line and it doesn't end with a newline,
                    // we need to add a newline before the duplicated content.
                    let needs_leading_newline =
                        rows.end.0 >= buffer.max_point().row && buffer.max_point().column > 0 && !text.ends_with('\n');

                    if needs_leading_newline {
                        text.insert(0, '\n');
                        end
                    } else {
                        text.push('\n');
                        Point::new(rows.start.0, 0)
                    }
                } else {
                    text.push('\n');
                    start
                };
                edits.push((insert_location..insert_location, text));
            } else {
                // duplicate character-wise
                let start = selection.start;
                let end = selection.end;
                let text = buffer.text_for_range(start..end).collect::<String>();
                edits.push((selection.end..selection.end, text));
            }
        }

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |buffer, cx| {
                buffer.edit(edits, None, cx);
            });

            // When duplicating upward with whole lines, move the cursor to the duplicated line
            if upwards && whole_lines {
                let display_map = this.display_map.update(cx, |map, cx| map.snapshot(cx));

                this.change_selections(SelectionEffects::no_scroll(), window, cx, |s| {
                    let mut new_ranges = Vec::new();
                    let selections = s.all::<Point>(&display_map);
                    let mut selections_iter = selections.iter().peekable();

                    while let Some(first_selection) = selections_iter.next() {
                        // Group contiguous selections together to find the total row span
                        let mut group_selections = vec![first_selection];
                        let mut rows = first_selection.spanned_rows(false, &display_map);

                        while let Some(next_selection) = selections_iter.peek() {
                            let next_rows = next_selection.spanned_rows(false, &display_map);
                            if next_rows.start < rows.end {
                                rows.end = next_rows.end;
                                group_selections.push(selections_iter.next().expect("peeked selection"));
                            } else {
                                break;
                            }
                        }

                        let row_count = rows.end.0 - rows.start.0;

                        // Move all selections in this group up by the total number of duplicated rows
                        for selection in group_selections {
                            let new_start =
                                Point::new(selection.start.row.saturating_sub(row_count), selection.start.column);

                            let new_end = Point::new(selection.end.row.saturating_sub(row_count), selection.end.column);

                            new_ranges.push(new_start..new_end);
                        }
                    }

                    s.select_ranges(new_ranges);
                });
            }

            this.request_autoscroll(Autoscroll::fit(), cx);
        });
    }

    pub(crate) fn duplicate_line_up(&mut self, _: &DuplicateLineUp, window: &mut Window, cx: &mut Context<Self>) {
        self.duplicate(true, true, window, cx);
    }

    pub(crate) fn duplicate_line_down(&mut self, _: &DuplicateLineDown, window: &mut Window, cx: &mut Context<Self>) {
        self.duplicate(false, true, window, cx);
    }

    pub(crate) fn duplicate_selection(&mut self, _: &DuplicateSelection, window: &mut Window, cx: &mut Context<Self>) {
        self.duplicate(false, false, window, cx);
    }

    pub(crate) fn move_line_up(&mut self, _: &MoveLineUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        let display_map = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let buffer = self.buffer.read(cx).snapshot(cx);

        let mut edits = Vec::new();
        let mut unfold_ranges = Vec::new();
        let mut refold_creases = Vec::new();

        let selections = self.selections.all::<Point>(&display_map);
        let mut selections = selections.iter().peekable();
        let mut contiguous_row_selections = Vec::new();
        let mut new_selections = Vec::new();

        while let Some(selection) = selections.next() {
            // Find all the selections that span a contiguous row range
            let (start_row, end_row) =
                consume_contiguous_rows(&mut contiguous_row_selections, selection, &display_map, &mut selections);

            // Move the text spanned by the row range to be before the line preceding the row range
            if start_row.0 > 0 {
                let range_to_move = Point::new(start_row.previous_row().0, buffer.line_len(start_row.previous_row()))
                    ..Point::new(end_row.previous_row().0, buffer.line_len(end_row.previous_row()));
                let insertion_point = display_map.prev_line_boundary(Point::new(start_row.previous_row().0, 0)).0;

                // Don't move lines across excerpts
                if buffer.excerpt_containing(insertion_point..range_to_move.end).is_some() {
                    let text = buffer
                        .text_for_range(range_to_move.clone())
                        .flat_map(|s| s.chars())
                        .skip(1)
                        .chain(['\n'])
                        .collect::<String>();

                    edits.push((
                        buffer.anchor_after(range_to_move.start)..buffer.anchor_before(range_to_move.end),
                        String::new(),
                    ));
                    let insertion_anchor = buffer.anchor_after(insertion_point);
                    edits.push((insertion_anchor..insertion_anchor, text));

                    let row_delta = range_to_move.start.row - insertion_point.row + 1;

                    // Move selections up
                    new_selections.extend(contiguous_row_selections.drain(..).map(|mut selection| {
                        selection.start.row -= row_delta;
                        selection.end.row -= row_delta;
                        selection
                    }));

                    // Move folds up
                    unfold_ranges.push(range_to_move.clone());
                    for fold in display_map.folds_in_range(
                        buffer.anchor_before(range_to_move.start)..buffer.anchor_after(range_to_move.end),
                    ) {
                        let mut start = fold.range.start.to_point(&buffer);
                        let mut end = fold.range.end.to_point(&buffer);
                        start.row -= row_delta;
                        end.row -= row_delta;
                        refold_creases.push(Crease::simple(start..end, fold.placeholder.clone()));
                    }
                }
            }

            // If we didn't move line(s), preserve the existing selections
            new_selections.append(&mut contiguous_row_selections);
        }

        self.transact(window, cx, |this, window, cx| {
            this.unfold_ranges(&unfold_ranges, true, true, cx);
            this.buffer.update(cx, |buffer, cx| {
                for (range, text) in edits {
                    buffer.edit([(range, text)], None, cx);
                }
            });
            this.fold_creases(refold_creases, true, window, cx);
            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select(new_selections);
            });
        });
    }

    pub(crate) fn move_line_down(&mut self, _: &MoveLineDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }

        let display_map = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let buffer = self.buffer.read(cx).snapshot(cx);

        let mut edits = Vec::new();
        let mut unfold_ranges = Vec::new();
        let mut refold_creases = Vec::new();

        let selections = self.selections.all::<Point>(&display_map);
        let mut selections = selections.iter().peekable();
        let mut contiguous_row_selections = Vec::new();
        let mut new_selections = Vec::new();

        while let Some(selection) = selections.next() {
            // Find all the selections that span a contiguous row range
            let (start_row, end_row) =
                consume_contiguous_rows(&mut contiguous_row_selections, selection, &display_map, &mut selections);

            // Move the text spanned by the row range to be after the last line of the row range
            if end_row.0 <= buffer.max_point().row {
                let range_to_move = MultiBufferPoint::new(start_row.0, 0)..MultiBufferPoint::new(end_row.0, 0);
                let insertion_point = display_map.next_line_boundary(MultiBufferPoint::new(end_row.0, 0)).0;

                // Don't move lines across excerpt boundaries
                if buffer.excerpt_containing(range_to_move.start..insertion_point).is_some() {
                    let mut text = String::from("\n");
                    text.extend(buffer.text_for_range(range_to_move.clone()));
                    text.pop(); // Drop trailing newline
                    edits.push((
                        buffer.anchor_after(range_to_move.start)..buffer.anchor_before(range_to_move.end),
                        String::new(),
                    ));
                    let insertion_anchor = buffer.anchor_after(insertion_point);
                    edits.push((insertion_anchor..insertion_anchor, text));

                    let row_delta = insertion_point.row - range_to_move.end.row + 1;

                    // Move selections down
                    new_selections.extend(contiguous_row_selections.drain(..).map(|mut selection| {
                        selection.start.row += row_delta;
                        selection.end.row += row_delta;
                        selection
                    }));

                    // Move folds down
                    unfold_ranges.push(range_to_move.clone());
                    for fold in display_map.folds_in_range(
                        buffer.anchor_before(range_to_move.start)..buffer.anchor_after(range_to_move.end),
                    ) {
                        let mut start = fold.range.start.to_point(&buffer);
                        let mut end = fold.range.end.to_point(&buffer);
                        start.row += row_delta;
                        end.row += row_delta;
                        refold_creases.push(Crease::simple(start..end, fold.placeholder.clone()));
                    }
                }
            }

            // If we didn't move line(s), preserve the existing selections
            new_selections.append(&mut contiguous_row_selections);
        }

        self.transact(window, cx, |this, window, cx| {
            this.unfold_ranges(&unfold_ranges, true, true, cx);
            this.buffer.update(cx, |buffer, cx| {
                for (range, text) in edits {
                    buffer.edit([(range, text)], None, cx);
                }
            });
            this.fold_creases(refold_creases, true, window, cx);
            this.change_selections(SelectionEffects::default(), window, cx, |s| s.select(new_selections));
        });
    }

    pub(crate) fn transpose(&mut self, _: &Transpose, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let text_layout_details = &self.text_layout_details(window, cx);
        self.transact(window, cx, |this, window, cx| {
            let edits = this.change_selections(SelectionEffects::default(), window, cx, |s| {
                let mut edits: Vec<(Range<MultiBufferOffset>, String)> = Vec::default();
                s.move_with(&mut |display_map, selection| {
                    if !selection.is_empty() {
                        return;
                    }

                    let mut head = selection.head();
                    let mut transpose_offset = head.to_offset(display_map, Bias::Right);
                    if head.column() == display_map.line_len(head.row()) {
                        transpose_offset = display_map
                            .buffer_snapshot()
                            .clip_offset(transpose_offset.saturating_sub_usize(1), Bias::Left);
                    }

                    if transpose_offset == MultiBufferOffset(0) {
                        return;
                    }

                    *head.column_mut() += 1;
                    head = display_map.clip_point(head, Bias::Right);
                    let goal = SelectionGoal::HorizontalPosition(
                        display_map.x_for_display_point(head, text_layout_details).into(),
                    );
                    selection.collapse_to(head, goal);

                    let transpose_start =
                        display_map.buffer_snapshot().clip_offset(transpose_offset.saturating_sub_usize(1), Bias::Left);
                    if edits.last().is_none_or(|e| e.0.end <= transpose_start) {
                        let transpose_end =
                            display_map.buffer_snapshot().clip_offset(transpose_offset + 1_usize, Bias::Right);
                        if let Some(ch) = display_map.buffer_snapshot().chars_at(transpose_start).next() {
                            edits.push((transpose_start..transpose_offset, String::new()));
                            edits.push((transpose_end..transpose_end, ch.to_string()));
                        }
                    }
                });
                edits
            });
            this.buffer.update(cx, |buffer, cx| buffer.edit(edits, None, cx));
            let selections = this.selections.all::<MultiBufferOffset>(&this.display_snapshot(cx));
            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select(selections);
            });
        });
    }
}

fn consume_contiguous_rows(
    contiguous_row_selections: &mut Vec<Selection<Point>>,
    selection: &Selection<Point>,
    display_map: &DisplaySnapshot,
    selections: &mut Peekable<Iter<Selection<Point>>>,
) -> (MultiBufferRow, MultiBufferRow) {
    contiguous_row_selections.push(*selection);
    let start_row = starting_row(selection, display_map);
    let mut end_row = ending_row(selection, display_map);

    while let Some(next_selection) = selections.peek() {
        if next_selection.start.row <= end_row.0 {
            end_row = ending_row(next_selection, display_map);
            contiguous_row_selections.push(*selections.next().expect("peeked selection"));
        } else {
            break;
        }
    }
    (start_row, end_row)
}

fn starting_row(selection: &Selection<Point>, display_map: &DisplaySnapshot) -> MultiBufferRow {
    if selection.start.column > 0 {
        MultiBufferRow(display_map.prev_line_boundary(selection.start).0.row)
    } else {
        MultiBufferRow(selection.start.row)
    }
}

fn ending_row(next_selection: &Selection<Point>, display_map: &DisplaySnapshot) -> MultiBufferRow {
    if next_selection.end.column > 0 || next_selection.is_empty() {
        MultiBufferRow(display_map.next_line_boundary(next_selection.end).0.row + 1)
    } else {
        MultiBufferRow(next_selection.end.row)
    }
}

struct LineManipulationResult {
    new_text: String,
    line_count_before: usize,
    line_count_after: usize,
}
