use std::ops::Deref;
use std::sync::Arc;

use gpui::App;
use gpui::FontId;
use gpui::Pixels;
use gpui::Window;
use gpui::px;
use language::Language;
use language::Point;
use multi_buffer::MultiBufferPoint;
use multi_buffer::MultiBufferRow;
use multi_buffer::ToOffset;

use super::Editor;
use super::EditorSnapshot;
use super::GutterDimensions;
use super::MIN_LINE_NUMBER_DIGITS;
use crate::display_map::DisplayRow;
use crate::display_map::DisplaySnapshot;
use crate::git::blame::GlobalBlameRenderer;
use crate::scroll::ScrollOffset;

impl Editor {
    pub fn snapshot(&self, window: &Window, cx: &mut App) -> EditorSnapshot {
        let git_blame_gutter_max_author_length = self
            .render_git_blame_gutter(cx)
            .then(|| {
                if let Some(blame) = self.blame.as_ref() {
                    let max_author_length = blame.update(cx, |blame, cx| blame.max_author_length(cx));
                    Some(max_author_length)
                } else {
                    None
                }
            })
            .flatten();

        let display_snapshot = self.display_map.update(cx, |map, cx| map.snapshot(cx));

        EditorSnapshot {
            mode: self.mode.clone(),
            show_gutter: self.show_gutter,
            offset_content: self.offset_content,
            sticky_line_number_digits: self.search_results_hold.map_or(0, |hold| hold.min_line_number_digits),
            show_line_numbers: self.show_line_numbers,
            number_deleted_lines: self.number_deleted_lines,
            show_git_diff_gutter: self.show_git_diff_gutter,
            show_code_actions: self.show_code_actions,
            show_runnables: self.show_runnables,
            git_blame_gutter_max_author_length,
            scroll_anchor: self.scroll_manager.shared_scroll_anchor(cx),
            display_snapshot,
            placeholder_display_snapshot: self
                .placeholder_display_map
                .as_ref()
                .map(|display_map| display_map.update(cx, |map, cx| map.snapshot(cx))),
            is_focused: self.focus_handle.is_focused(window),
            gutter_hovered: self.gutter_hovered,
        }
    }
}

impl EditorSnapshot {
    pub fn language_at<T: ToOffset>(&self, position: T) -> Option<&Arc<Language>> {
        self.display_snapshot.buffer_snapshot().language_at(position)
    }

    pub fn display_row_for_inline_code_action(&self, buffer_point: Point) -> Option<DisplayRow> {
        if self.is_line_folded(MultiBufferRow(buffer_point.row)) {
            return None;
        }

        let line_indent = self.display_snapshot.buffer_snapshot().line_indent_for_row(MultiBufferRow(buffer_point.row));
        if line_indent.is_line_blank() {
            return None;
        }

        const INLINE_SLOT_CHAR_LIMIT: u32 = 4;
        const MAX_ALTERNATE_DISTANCE: u32 = 8;

        let is_valid_row = |row_candidate: u32| -> bool {
            if self.is_line_folded(MultiBufferRow(row_candidate)) {
                return false;
            }
            if buffer_point.row == row_candidate {
                if buffer_point.column < INLINE_SLOT_CHAR_LIMIT {
                    return false;
                }
            } else {
                let candidate_point = MultiBufferPoint { row: row_candidate, column: 0 };
                let range = if candidate_point < buffer_point {
                    candidate_point..buffer_point
                } else {
                    buffer_point..candidate_point
                };
                if self.display_snapshot.buffer_snapshot().excerpt_containing(range).is_none() {
                    return false;
                }
            }
            let line_indent =
                self.display_snapshot.buffer_snapshot().line_indent_for_row(MultiBufferRow(row_candidate));
            if line_indent.is_line_blank() {
                true
            } else {
                let indent_size =
                    self.display_snapshot.buffer_snapshot().indent_size_for_line(MultiBufferRow(row_candidate));
                if indent_size.len >= INLINE_SLOT_CHAR_LIMIT {
                    true
                } else if row_candidate == buffer_point.row {
                    let display_row =
                        self.display_snapshot.point_to_display_point(buffer_point, text::Bias::Left).row();
                    let line_start_display_row = self
                        .display_snapshot
                        .point_to_display_point(Point::new(buffer_point.row, 0), text::Bias::Left)
                        .row();
                    if display_row > line_start_display_row {
                        self.display_snapshot
                            .soft_wrap_indent(DisplayRow(display_row.0 - 1))
                            .is_some_and(|indent| indent >= INLINE_SLOT_CHAR_LIMIT)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        };

        let new_buffer_row = if is_valid_row(buffer_point.row) {
            Some(buffer_point.row)
        } else {
            let max_row = self.display_snapshot.buffer_snapshot().max_point().row;
            (1..=MAX_ALTERNATE_DISTANCE).find_map(|offset| {
                let row_above = buffer_point.row.saturating_sub(offset);
                let row_below = buffer_point.row + offset;
                if row_above != buffer_point.row && is_valid_row(row_above) {
                    Some(row_above)
                } else if row_below <= max_row && is_valid_row(row_below) {
                    Some(row_below)
                } else {
                    None
                }
            })
        }?;

        let mut new_display_row = self
            .display_snapshot
            .point_to_display_point(Point { row: new_buffer_row, column: buffer_point.column }, text::Bias::Left)
            .row();

        let line_start_display_row = self
            .display_snapshot
            .point_to_display_point(Point { row: new_buffer_row, column: 0 }, text::Bias::Left)
            .row();

        if new_display_row > line_start_display_row
            && self
                .display_snapshot
                .soft_wrap_indent(DisplayRow(new_display_row.0 - 1))
                .is_some_and(|indent| indent < INLINE_SLOT_CHAR_LIMIT)
        {
            new_display_row = line_start_display_row;
        }

        Some(new_display_row)
    }

    pub const fn is_focused(&self) -> bool {
        self.is_focused
    }

    pub fn placeholder_text(&self) -> Option<String> {
        self.placeholder_display_snapshot.as_ref().map(|display_map| display_map.text())
    }

    pub fn scroll_position(&self) -> gpui::Point<ScrollOffset> {
        self.scroll_anchor.scroll_position(&self.display_snapshot)
    }

    pub fn max_line_number_width(&self, ch_advance: Pixels) -> Pixels {
        line_number_width(self.widest_line_number(), ch_advance)
    }

    pub fn gutter_dimensions(
        &self,
        font_id: FontId,
        font_size: Pixels,
        window: &mut Window,
        cx: &App,
    ) -> GutterDimensions {
        if self.show_gutter {
            let ch_width = cx.text_system().ch_width(font_id, font_size);
            let ch_advance = cx.text_system().ch_advance(font_id, font_size);
            let show_git_gutter = self.show_git_diff_gutter.unwrap_or(true);
            let show_line_numbers = self.show_line_numbers;
            let line_gutter_width = if show_line_numbers {
                // Avoid flicker-like gutter resizes when the line number gains another digit by
                // only resizing the gutter on files with > 10**MIN_LINE_NUMBER_DIGITS lines.
                let min_digits = MIN_LINE_NUMBER_DIGITS.max(self.sticky_line_number_digits);
                let min_width_for_number_on_gutter = ch_advance * min_digits as f32;
                self.max_line_number_width(ch_advance).max(min_width_for_number_on_gutter)
            } else {
                0.0.into()
            };

            let git_blame_entries_width = self.git_blame_gutter_max_author_length.map(|max_author_length| {
                let renderer = Arc::clone(&cx.global::<GlobalBlameRenderer>().0);
                const MAX_RELATIVE_TIMESTAMP: &str = "2 years, 11 months ago";

                let max_char_count = max_author_length.min(renderer.max_author_length())
                    + ::git::SHORT_SHA_LENGTH
                    + MAX_RELATIVE_TIMESTAMP.len();

                ch_advance * max_char_count + renderer.blame_entry_non_text_width(window, cx)
            });

            let is_singleton = self.buffer_snapshot().is_singleton();

            let left_padding = git_blame_entries_width.unwrap_or(Pixels::ZERO)
                + if !is_singleton {
                    ch_width * 4.0
                // runnables and the copy-file-location hover button share this space;
                // the hover button is always available, so only an explicit opt-out
                // (e.g. previews) drops it
                } else if self.show_runnables != Some(false) {
                    ch_width * 3.0
                } else if show_git_gutter && show_line_numbers {
                    ch_width * 2.0
                } else if show_git_gutter || show_line_numbers {
                    ch_width
                } else {
                    px(0.)
                };

            let shows_folds = is_singleton;

            let right_padding = if shows_folds && show_line_numbers {
                ch_width * 4.0
            } else if shows_folds || (!is_singleton && show_line_numbers) {
                ch_width * 3.0
            } else if show_line_numbers {
                ch_width
            } else {
                px(0.)
            };

            GutterDimensions {
                left_padding,
                right_padding,
                width: line_gutter_width + left_padding + right_padding,
                margin: GutterDimensions::default_gutter_margin(font_id, font_size, cx),
                git_blame_entries_width,
            }
        } else if self.offset_content {
            GutterDimensions::default_with_margin(font_id, font_size, cx)
        } else {
            GutterDimensions::default()
        }
    }
}

impl Deref for EditorSnapshot {
    type Target = DisplaySnapshot;

    fn deref(&self) -> &Self::Target {
        &self.display_snapshot
    }
}

/// Width of line number `number` (at least 1), as one digit advance per digit.
fn line_number_width(number: u32, ch_advance: Pixels) -> Pixels {
    ch_advance * (number.ilog10() + 1) as f32
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::line_number_width;

    #[test]
    fn line_number_width_is_one_advance_per_digit() {
        assert_eq!(line_number_width(1, px(8.)), px(8.));
        assert_eq!(line_number_width(9, px(8.)), px(8.));
        assert_eq!(line_number_width(10, px(8.)), px(16.));
        assert_eq!(line_number_width(12345, px(8.)), px(40.));
    }
}
