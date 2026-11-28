use std::cmp;
use std::iter;

use base64::engine::general_purpose::STANDARD;
use convert_case::Case;
use convert_case::Casing as _;
use gpui::Context;
use gpui::Pixels;
use gpui::Window;
use gpui::px;
use itertools::Itertools as _;
use language::Point;
use language::Selection;
use language::SelectionGoal;
use multi_buffer::Anchor;
use multi_buffer::MultiBufferOffset;
use multi_buffer::ToOffset as _;
use multi_buffer::ToPoint as _;

use super::Editor;
use super::SelectionEffects;
use crate::actions::AlignSelections;
use crate::actions::ConvertFromBase64;
use crate::actions::ConvertToBase64;
use crate::actions::ConvertToKebabCase;
use crate::actions::ConvertToLowerCamelCase;
use crate::actions::ConvertToLowerCase;
use crate::actions::ConvertToOppositeCase;
use crate::actions::ConvertToRot13;
use crate::actions::ConvertToRot47;
use crate::actions::ConvertToSentenceCase;
use crate::actions::ConvertToSnakeCase;
use crate::actions::ConvertToTitleCase;
use crate::actions::ConvertToUpperCamelCase;
use crate::actions::ConvertToUpperCase;
use crate::actions::ToggleCase;
use crate::actions::WrapSelectionsInTag;
use crate::scroll::Autoscroll;

impl Editor {
    pub(super) fn wrap_selections_in_tag(
        &mut self,
        _: &WrapSelectionsInTag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only(cx) {
            return;
        }

        let snapshot = self.buffer.read(cx).snapshot(cx);

        let mut edits = Vec::new();
        let mut boundaries = Vec::new();

        for selection in &self.selections.all_adjusted(&self.display_snapshot(cx)) {
            let Some(wrap_config) =
                snapshot.language_at(selection.start).and_then(|lang| lang.config().wrap_characters.clone())
            else {
                continue;
            };

            let open_tag = format!("{}{}", wrap_config.start_prefix, wrap_config.start_suffix);
            let close_tag = format!("{}{}", wrap_config.end_prefix, wrap_config.end_suffix);

            let start_before = snapshot.anchor_before(selection.start);
            let end_after = snapshot.anchor_after(selection.end);

            edits.push((start_before..start_before, open_tag));
            edits.push((end_after..end_after, close_tag));

            boundaries.push((start_before, end_after, wrap_config.start_prefix.len(), wrap_config.end_suffix.len()));
        }

        // No selection is in a language with wrap characters: leave the keystroke to others.
        if edits.is_empty() {
            cx.propagate();
            return;
        }

        self.transact(window, cx, |this, window, cx| {
            let buffer = this.buffer.update(cx, |buffer, cx| {
                buffer.edit(edits, None, cx);
                buffer.snapshot(cx)
            });

            let mut new_selections = Vec::with_capacity(boundaries.len() * 2);
            for (start_before, end_after, start_prefix_len, end_suffix_len) in boundaries {
                let open_offset = start_before.to_offset(&buffer) + start_prefix_len;
                let close_offset = end_after.to_offset(&buffer).saturating_sub_usize(end_suffix_len);
                new_selections.push(open_offset..open_offset);
                new_selections.push(close_offset..close_offset);
            }

            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select_ranges(new_selections);
            });

            this.request_autoscroll(Autoscroll::fit(), cx);
        });
    }

    pub(crate) fn align_selections(&mut self, _: &AlignSelections, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }

        let display_snapshot = self.display_snapshot(cx);
        let text_layout_details = self.text_layout_details(window, cx);

        let font_id = text_layout_details.text_system.resolve_font(&text_layout_details.editor_style.text.font());
        let font_size = text_layout_details.editor_style.text.font_size.to_pixels(text_layout_details.rem_size);
        let Ok(space_width) =
            text_layout_details.text_system.advance(font_id, font_size, ' ').map(|advance| advance.width)
        else {
            return;
        };
        if space_width <= px(0.) {
            return;
        }
        let buffer_snapshot = display_snapshot.buffer_snapshot();
        let tab_size = display_snapshot.tab_snapshot().tab_size.get();

        struct CursorData {
            anchor: Anchor,
            row: u32,
            x: Pixels,
        }
        let cursor_data: Vec<CursorData> = self
            .selections
            .disjoint_anchors()
            .iter()
            .map(|selection| {
                let anchor = if selection.reversed { selection.head() } else { selection.tail() };
                let point = anchor.to_point(buffer_snapshot);
                let mut prefix = String::new();
                let mut column = 0;
                for chunk in buffer_snapshot.text_for_range(Point::new(point.row, 0)..point) {
                    for ch in chunk.chars() {
                        if ch == '\t' {
                            let tab_len = tab_size - column % tab_size;
                            prefix.extend(iter::repeat_n(' ', tab_len as usize));
                            column += tab_len;
                        } else {
                            prefix.push(ch);
                            column += 1;
                        }
                    }
                }
                let run = text_layout_details.editor_style.text.to_run(prefix.len());
                let x = text_layout_details.text_system.layout_line(&prefix, font_size, &[run], None).width;
                CursorData { anchor, row: point.row, x }
            })
            .collect();

        let rows_anchors_count: Vec<usize> = cursor_data
            .iter()
            .map(|cursor| cursor.row)
            .chunk_by(|&row| row)
            .into_iter()
            .map(|(_, group)| group.count())
            .collect();
        let max_columns = rows_anchors_count.iter().max().copied().unwrap_or(0);
        let mut rows_x_offset = vec![px(0.); rows_anchors_count.len()];
        let mut edits = Vec::new();

        for column_idx in 0..max_columns {
            let mut cursor_index = 0;

            // Calculate target_x => position that the selections will go
            let mut target_x = px(0.);
            for (row_idx, cursor_count) in rows_anchors_count.iter().enumerate() {
                // Skip rows that don't have this column
                if column_idx >= *cursor_count {
                    cursor_index += cursor_count;
                    continue;
                }

                let adjusted_x = cursor_data[cursor_index + column_idx].x + rows_x_offset[row_idx];
                if adjusted_x > target_x {
                    target_x = adjusted_x;
                }
                cursor_index += cursor_count;
            }

            // Collect edits for this column
            cursor_index = 0;
            for (row_idx, cursor_count) in rows_anchors_count.iter().enumerate() {
                // Skip rows that don't have this column
                if column_idx >= *cursor_count {
                    cursor_index += *cursor_count;
                    continue;
                }

                let cursor = &cursor_data[cursor_index + column_idx];
                let spaces_needed =
                    ((target_x - cursor.x - rows_x_offset[row_idx]) / space_width).round().max(0.) as u32;
                if spaces_needed > 0 {
                    let anchor = cursor.anchor.bias_left(&display_snapshot);
                    edits.push((anchor..anchor, " ".repeat(spaces_needed as usize)));
                }
                rows_x_offset[row_idx] += space_width * spaces_needed as f32;

                cursor_index += *cursor_count;
            }
        }

        if !edits.is_empty() {
            self.transact(window, cx, |editor, _window, cx| {
                editor.edit(edits, cx);
            });
        }
    }

    pub(crate) fn convert_to_upper_case(
        &mut self,
        _: &ConvertToUpperCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| text.to_uppercase());
    }

    pub(crate) fn convert_to_lower_case(
        &mut self,
        _: &ConvertToLowerCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| text.to_lowercase());
    }

    pub(crate) fn convert_to_title_case(
        &mut self,
        _: &ConvertToTitleCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| Self::convert_text_case(text, Case::Title));
    }

    pub(crate) fn convert_to_snake_case(
        &mut self,
        _: &ConvertToSnakeCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| Self::convert_text_case(text, Case::Snake));
    }

    pub(crate) fn convert_to_kebab_case(
        &mut self,
        _: &ConvertToKebabCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| Self::convert_text_case(text, Case::Kebab));
    }

    pub(crate) fn convert_to_upper_camel_case(
        &mut self,
        _: &ConvertToUpperCamelCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| Self::convert_text_case(text, Case::UpperCamel));
    }

    pub(crate) fn convert_to_lower_camel_case(
        &mut self,
        _: &ConvertToLowerCamelCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| Self::convert_text_case(text, Case::Camel));
    }

    pub(crate) fn convert_to_opposite_case(
        &mut self,
        _: &ConvertToOppositeCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| {
            text.chars().fold(String::with_capacity(text.len()), |mut t, c| {
                if c.is_uppercase() {
                    t.extend(c.to_lowercase());
                } else {
                    t.extend(c.to_uppercase());
                }
                t
            })
        });
    }

    pub(crate) fn convert_to_sentence_case(
        &mut self,
        _: &ConvertToSentenceCase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_text(window, cx, |text| Self::convert_text_case(text, Case::Sentence));
    }

    pub(crate) fn toggle_case(&mut self, _: &ToggleCase, window: &mut Window, cx: &mut Context<Self>) {
        self.manipulate_text(window, cx, |text| {
            let has_upper_case_characters = text.chars().any(|c| c.is_uppercase());
            if has_upper_case_characters { text.to_lowercase() } else { text.to_uppercase() }
        });
    }

    pub(crate) fn convert_to_rot13(&mut self, _: &ConvertToRot13, window: &mut Window, cx: &mut Context<Self>) {
        self.manipulate_text(window, cx, |text| {
            text.chars()
                .map(|c| match c {
                    'A'..='M' | 'a'..='m' => ((c as u8) + 13) as char,
                    'N'..='Z' | 'n'..='z' => ((c as u8) - 13) as char,
                    _ => c,
                })
                .collect()
        });
    }

    fn convert_text_case(text: &str, case: Case) -> String {
        text.split('\n')
            .map(|line| {
                let trimmed_start = line.trim_start();
                let leading = &line[..line.len() - trimmed_start.len()];
                let trimmed = trimmed_start.trim_end();
                let trailing = &trimmed_start[trimmed.len()..];
                format!("{}{}{}", leading, trimmed.to_case(case), trailing)
            })
            .join("\n")
    }

    pub(crate) fn convert_to_rot47(&mut self, _: &ConvertToRot47, window: &mut Window, cx: &mut Context<Self>) {
        self.manipulate_text(window, cx, |text| {
            text.chars()
                .map(|c| {
                    let code_point = c as u32;
                    if (33..=126).contains(&code_point) {
                        return char::from_u32(33 + ((code_point + 14) % 94)).expect("printable ascii");
                    }
                    c
                })
                .collect()
        });
    }

    pub(crate) fn convert_to_base64(&mut self, _: &ConvertToBase64, window: &mut Window, cx: &mut Context<Self>) {
        use base64::Engine as _;
        self.manipulate_text(window, cx, |text| STANDARD.encode(text));
    }

    pub(crate) fn convert_from_base64(&mut self, _: &ConvertFromBase64, window: &mut Window, cx: &mut Context<Self>) {
        use base64::Engine as _;
        self.manipulate_text(window, cx, |text| match STANDARD.decode(text) {
            Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| text.to_owned()),
            Err(_) => text.to_owned(),
        });
    }

    fn manipulate_text<Fn>(&mut self, window: &mut Window, cx: &mut Context<Self>, mut callback: Fn)
    where
        Fn: FnMut(&str) -> String,
    {
        if self.read_only(cx) {
            return;
        }
        let buffer = self.buffer.read(cx).snapshot(cx);

        let mut selections = Vec::new();
        for selection in self.selections.all_adjusted(&self.display_snapshot(cx)) {
            let (start, end) = if selection.is_empty() {
                let (word_range, _) = buffer.surrounding_word(selection.start, None);
                (word_range.start, word_range.end)
            } else {
                (buffer.point_to_offset(selection.start), buffer.point_to_offset(selection.end))
            };
            selections.push(Selection {
                id: selection.id,
                start,
                end,
                reversed: selection.reversed,
                goal: SelectionGoal::None,
            });
        }

        // Expanding cursors to words can make selections overlap, e.g. several
        // cursors in one word. Overlapping edits would be applied once per
        // selection and duplicate the text, so transform each merged range once.
        selections.sort_by_key(|selection| selection.start);
        let mut merged_selections: Vec<Selection<MultiBufferOffset>> = Vec::new();
        for selection in selections {
            if let Some(previous) = merged_selections.last_mut()
                && selection.start < previous.end
            {
                previous.end = cmp::max(previous.end, selection.end);
            } else {
                merged_selections.push(selection);
            }
        }

        let mut new_selections = Vec::new();
        let mut edits = Vec::new();
        for selection in merged_selections {
            let old_text = buffer.text_for_range(selection.start..selection.end).collect::<String>();
            let new_text = callback(&old_text);

            new_selections.push(Selection {
                start: buffer.anchor_before(selection.start),
                end: buffer.anchor_after(selection.end),
                goal: SelectionGoal::None,
                id: selection.id,
                reversed: selection.reversed,
            });

            if new_text != old_text {
                edits.push((selection.start..selection.end, new_text));
            }
        }

        if edits.is_empty() {
            return;
        }

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |buffer, cx| {
                buffer.edit(edits, None, cx);
            });

            this.change_selections(SelectionEffects::default(), window, cx, |s| {
                s.select(new_selections);
            });

            this.request_autoscroll(Autoscroll::fit(), cx);
        });
    }
}
