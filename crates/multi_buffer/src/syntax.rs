use std::cmp;
use std::collections::BTreeMap;
use std::fmt;
use std::iter;
use std::ops;
use std::ops::AddAssign;
use std::ops::ControlFlow;
use std::ops::Range;
use std::ops::Sub;
use std::sync::Arc;

use async_macos::yield_now;
use gpui::App;
use itertools::Itertools as _;
use language::BufferSnapshot;
use language::CharClassifier;
use language::DiagnosticEntryRef;
use language::IndentSize;
use language::Language;
use language::LanguageScope;
use language::Outline;
use language::OutlineItem;
use language::Point;
use language::TextObject;
use language::TreeSitterOptions;
use language::language_settings::AllLanguageSettings;
use language::language_settings::INDENT_GUIDES;
use language::language_settings::LanguageSettings;
use smallvec::SmallVec;
use sum_tree::Bias;
use text::BufferId;
use text::LineIndent;
use theme::SyntaxTheme;

use super::Anchor;
use super::BufferOffset;
use super::IndentGuide;
use super::MultiBufferDimension;
use super::MultiBufferOffset;
use super::MultiBufferRow;
use super::MultiBufferSnapshot;
use super::ToOffset;
use super::ToPoint;

impl MultiBufferSnapshot {
    pub fn suggested_indents(
        &self,
        rows: impl IntoIterator<Item = u32>,
        cx: &App,
    ) -> BTreeMap<MultiBufferRow, IndentSize> {
        let mut result = BTreeMap::new();
        self.suggested_indents_callback(
            rows,
            &mut |row, indent| {
                result.insert(row, indent);
                ControlFlow::Continue(())
            },
            cx,
        );
        result
    }

    // move this to be a generator once those are a thing
    pub fn suggested_indents_callback(
        &self,
        rows: impl IntoIterator<Item = u32>,
        cb: &mut dyn FnMut(MultiBufferRow, IndentSize) -> ControlFlow<()>,
        cx: &App,
    ) {
        let mut rows_for_excerpt = Vec::new();
        let mut cursor = self.cursor::<Point, Point>();
        let mut rows = rows.into_iter().peekable();
        let mut prev_row = u32::MAX;
        let mut prev_language_indent_size = IndentSize::default();

        while let Some(row) = rows.next() {
            cursor.seek(&Point::new(row, 0));
            let Some(region) = cursor.region() else {
                continue;
            };

            // Retrieve the language and indent size once for each disjoint region being indented.
            let single_indent_size = if row.saturating_sub(1) == prev_row {
                prev_language_indent_size
            } else {
                region.buffer.language_indent_size_at(Point::new(row, 0), cx)
            };
            prev_language_indent_size = single_indent_size;
            prev_row = row;

            let start_buffer_row = region.buffer_range.start.row;
            let start_multibuffer_row = region.range.start.row;
            let end_multibuffer_row = region.range.end.row;

            rows_for_excerpt.push(row);
            while let Some(next_row) = rows.peek().copied() {
                if end_multibuffer_row > next_row {
                    rows_for_excerpt.push(next_row);
                    rows.next();
                } else {
                    break;
                }
            }

            let buffer_rows = rows_for_excerpt.drain(..).map(|row| start_buffer_row + row - start_multibuffer_row);
            let buffer_indents = region.buffer.suggested_indents(buffer_rows, single_indent_size);
            for (row, indent) in buffer_indents {
                if cb(MultiBufferRow(start_multibuffer_row + row - start_buffer_row), indent).is_break() {
                    return;
                }
            }
        }
    }

    pub fn indent_size_for_line(&self, row: MultiBufferRow) -> IndentSize {
        if let Some((buffer, range)) = self.buffer_line_for_row(row) {
            let mut size = buffer.indent_size_for_line(range.start.row);
            size.len = size.len.min(range.end.column).saturating_sub(range.start.column);
            size
        } else {
            IndentSize::spaces(0)
        }
    }

    pub fn line_indent_for_row(&self, row: MultiBufferRow) -> LineIndent {
        if let Some((buffer, range)) = self.buffer_line_for_row(row) {
            LineIndent::from_iter(buffer.text_for_range(range).flat_map(|s| s.chars()))
        } else {
            LineIndent::spaces(0)
        }
    }

    pub fn prev_non_blank_row(&self, mut row: MultiBufferRow) -> Option<MultiBufferRow> {
        while row.0 > 0 {
            row.0 -= 1;
            if !self.is_line_blank(row) {
                return Some(row);
            }
        }
        None
    }

    /// Returns the smallest enclosing bracket ranges containing the given range or
    /// None if no brackets contain range or the range is not contained in a single
    /// excerpt.
    ///
    /// Can optionally pass a range_filter to filter the ranges of brackets to consider.
    #[allow(clippy::type_complexity)]
    pub fn innermost_enclosing_bracket_ranges<T: ToOffset>(
        &self,
        range: Range<T>,
        range_filter: Option<&dyn Fn(&BufferSnapshot, Range<BufferOffset>, Range<BufferOffset>) -> bool>,
    ) -> Option<(Range<MultiBufferOffset>, Range<MultiBufferOffset>)> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let results = self.map_excerpt_ranges(range, |buffer, excerpt_range, input_buffer_range| {
            let filter = |open: Range<usize>, close: Range<usize>| -> bool {
                excerpt_range.context.start.0 <= open.start
                    && close.end <= excerpt_range.context.end.0
                    && range_filter.is_none_or(|filter| {
                        filter(
                            buffer,
                            BufferOffset(open.start)..BufferOffset(close.end),
                            BufferOffset(close.start)..BufferOffset(close.end),
                        )
                    })
            };
            let Some((open, close)) = buffer.innermost_enclosing_bracket_ranges(input_buffer_range, Some(&filter))
            else {
                return Vec::new();
            };
            vec![
                (BufferOffset(open.start)..BufferOffset(open.end), ()),
                (BufferOffset(close.start)..BufferOffset(close.end), ()),
            ]
        })?;
        let [(open, ()), (close, ())] = results.try_into().ok()?;
        Some((open, close))
    }

    /// Returns enclosing bracket ranges containing the given range or returns None if the range is
    /// not contained in a single excerpt.
    pub fn enclosing_bracket_ranges<T: ToOffset>(
        &self,
        range: Range<T>,
    ) -> Option<impl Iterator<Item = (Range<MultiBufferOffset>, Range<MultiBufferOffset>)>> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let results = self.map_excerpt_ranges(range, |buffer, excerpt_range, input_buffer_range| {
            buffer
                .enclosing_bracket_ranges(input_buffer_range)
                .filter(|pair| {
                    excerpt_range.context.start.0 <= pair.open_range.start
                        && pair.close_range.end <= excerpt_range.context.end.0
                })
                .flat_map(|pair| {
                    [
                        (BufferOffset(pair.open_range.start)..BufferOffset(pair.open_range.end), ()),
                        (BufferOffset(pair.close_range.start)..BufferOffset(pair.close_range.end), ()),
                    ]
                })
                .collect()
        })?;
        Some(results.into_iter().map(|(range, ())| range).tuples())
    }

    /// Returns enclosing bracket ranges containing the given range or returns None if the range is
    /// not contained in a single excerpt.
    pub fn text_object_ranges<T: ToOffset>(
        &self,
        range: Range<T>,
        options: TreeSitterOptions,
    ) -> impl Iterator<Item = (Range<MultiBufferOffset>, TextObject)> + '_ {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        self.map_excerpt_ranges(range, |buffer, excerpt_range, input_buffer_range| {
            buffer
                .text_object_ranges(input_buffer_range, options)
                .filter(|(range, _)| {
                    excerpt_range.context.start.0 <= range.start && range.end <= excerpt_range.context.end.0
                })
                .map(|(range, text_object)| (BufferOffset(range.start)..BufferOffset(range.end), text_object))
                .collect()
        })
        .into_iter()
        .flatten()
    }

    pub fn bracket_ranges<T: ToOffset>(
        &self,
        range: Range<T>,
    ) -> Option<impl Iterator<Item = (Range<MultiBufferOffset>, Range<MultiBufferOffset>)>> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let results = self.map_excerpt_ranges(range, |buffer, excerpt_range, input_buffer_range| {
            buffer
                .bracket_ranges(input_buffer_range)
                .filter(|pair| {
                    excerpt_range.context.start.0 <= pair.open_range.start
                        && pair.close_range.end <= excerpt_range.context.end.0
                })
                .flat_map(|pair| {
                    [
                        (BufferOffset(pair.open_range.start)..BufferOffset(pair.open_range.end), ()),
                        (BufferOffset(pair.close_range.start)..BufferOffset(pair.close_range.end), ()),
                    ]
                })
                .collect()
        })?;
        Some(results.into_iter().map(|(range, ())| range).tuples())
    }

    pub fn runnable_ranges(
        &self,
        range: Range<Anchor>,
    ) -> impl Iterator<Item = (Range<Anchor>, language::RunnableRange)> + '_ {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        self.lift_buffer_metadata(range, move |buffer, range| {
            Some(
                buffer
                    .runnable_ranges(range.clone())
                    .filter(move |runnable| {
                        runnable.run_range.start >= range.start && runnable.run_range.end < range.end
                    })
                    .map(|runnable| (runnable.run_range.clone(), runnable)),
            )
        })
        .map(|(run_range, runnable, _)| {
            (self.anchor_after(run_range.start)..self.anchor_before(run_range.end), runnable)
        })
    }

    pub fn line_indents(
        &self,
        start_row: MultiBufferRow,
        buffer_filter: impl Fn(&BufferSnapshot) -> bool,
    ) -> impl Iterator<Item = (MultiBufferRow, LineIndent, &BufferSnapshot)> {
        let max_point = self.max_point();
        let mut cursor = self.cursor::<Point, Point>();
        cursor.seek(&Point::new(start_row.0, 0));
        iter::from_fn(move || {
            let mut region = cursor.region()?;
            while !buffer_filter(region.excerpt.buffer_snapshot(self)) {
                cursor.next();
                region = cursor.region()?;
            }
            let region = cursor.region()?;
            let overshoot = start_row.0.saturating_sub(region.range.start.row);
            let buffer_start_row = (region.buffer_range.start.row + overshoot).min(region.buffer_range.end.row);

            let buffer_end_row =
                if region.is_main_buffer && (region.has_trailing_newline || region.range.end == max_point) {
                    region.buffer_range.end.row
                } else {
                    region.buffer_range.end.row.saturating_sub(1)
                };

            let line_indents = region.buffer.line_indents_in_row_range(buffer_start_row..buffer_end_row);
            let region_buffer_row = region.buffer_range.start.row;
            let region_row = region.range.start.row;
            let region_buffer = region.excerpt.buffer_snapshot(self);
            cursor.next();
            Some(line_indents.map(move |(buffer_row, indent)| {
                let row = region_row + (buffer_row - region_buffer_row);
                (MultiBufferRow(row), indent, region_buffer)
            }))
        })
        .flatten()
    }

    pub fn reversed_line_indents(
        &self,
        end_row: MultiBufferRow,
        buffer_filter: impl Fn(&BufferSnapshot) -> bool,
    ) -> impl Iterator<Item = (MultiBufferRow, LineIndent, &BufferSnapshot)> {
        let max_point = self.max_point();
        let mut cursor = self.cursor::<Point, Point>();
        cursor.seek(&Point::new(end_row.0, 0));
        iter::from_fn(move || {
            let mut region = cursor.region()?;
            while !buffer_filter(region.excerpt.buffer_snapshot(self)) {
                cursor.prev();
                region = cursor.region()?;
            }
            let region = cursor.region()?;

            let buffer_start_row = region.buffer_range.start.row;
            let buffer_end_row =
                if region.is_main_buffer && (region.has_trailing_newline || region.range.end == max_point) {
                    region.buffer_range.end.row + 1
                } else {
                    region.buffer_range.end.row
                };

            let overshoot = end_row.0 - region.range.start.row;
            let buffer_end_row = (region.buffer_range.start.row + overshoot + 1).min(buffer_end_row);

            let line_indents = region.buffer.reversed_line_indents_in_row_range(buffer_start_row..buffer_end_row);
            let region_buffer_row = region.buffer_range.start.row;
            let region_row = region.range.start.row;
            let region_buffer = region.excerpt.buffer_snapshot(self);
            cursor.prev();
            Some(line_indents.map(move |(buffer_row, indent)| {
                let row = region_row + (buffer_row - region_buffer_row);
                (MultiBufferRow(row), indent, region_buffer)
            }))
        })
        .flatten()
    }

    pub async fn enclosing_indent(
        &self,
        mut target_row: MultiBufferRow,
    ) -> Option<(Range<MultiBufferRow>, LineIndent)> {
        let max_row = MultiBufferRow(self.max_point().row);
        if target_row >= max_row {
            return None;
        }

        let mut target_indent = self.line_indent_for_row(target_row);

        // If the current row is at the start of an indented block, we want to return this
        // block as the enclosing indent.
        if !target_indent.is_line_empty() && target_row < max_row {
            let next_line_indent = self.line_indent_for_row(MultiBufferRow(target_row.0 + 1));
            if !next_line_indent.is_line_empty() && target_indent.raw_len() < next_line_indent.raw_len() {
                target_indent = next_line_indent;
                target_row.0 += 1;
            }
        }

        const SEARCH_ROW_LIMIT: u32 = 25000;
        const SEARCH_WHITESPACE_ROW_LIMIT: u32 = 2500;
        const YIELD_INTERVAL: u32 = 100;

        let mut accessed_row_counter = 0;

        // If there is a blank line at the current row, search for the next non indented lines
        if target_indent.is_line_empty() {
            let start = MultiBufferRow(target_row.0.saturating_sub(SEARCH_WHITESPACE_ROW_LIMIT));
            let end = MultiBufferRow((max_row.0 + 1).min(target_row.0 + SEARCH_WHITESPACE_ROW_LIMIT));

            let mut non_empty_line_above = None;
            for (row, indent, _) in self.reversed_line_indents(target_row, |_| true) {
                if row < start {
                    break;
                }
                accessed_row_counter += 1;
                if accessed_row_counter == YIELD_INTERVAL {
                    accessed_row_counter = 0;
                    yield_now().await;
                }
                if !indent.is_line_empty() {
                    non_empty_line_above = Some((row, indent));
                    break;
                }
            }

            let mut non_empty_line_below = None;
            for (row, indent, _) in self.line_indents(target_row, |_| true) {
                if row > end {
                    break;
                }
                accessed_row_counter += 1;
                if accessed_row_counter == YIELD_INTERVAL {
                    accessed_row_counter = 0;
                    yield_now().await;
                }
                if !indent.is_line_empty() {
                    non_empty_line_below = Some((row, indent));
                    break;
                }
            }

            let (row, indent) = match (non_empty_line_above, non_empty_line_below) {
                (Some((above_row, above_indent)), Some((below_row, below_indent))) => {
                    if above_indent.raw_len() >= below_indent.raw_len() {
                        (above_row, above_indent)
                    } else {
                        (below_row, below_indent)
                    }
                }
                (Some(above), None) => above,
                (None, Some(below)) => below,
                _ => return None,
            };

            target_indent = indent;
            target_row = row;
        }

        let start = MultiBufferRow(target_row.0.saturating_sub(SEARCH_ROW_LIMIT));
        let end = MultiBufferRow((max_row.0 + 1).min(target_row.0 + SEARCH_ROW_LIMIT));

        let mut start_indent = None;
        for (row, indent, _) in self.reversed_line_indents(target_row, |_| true) {
            if row < start {
                break;
            }
            accessed_row_counter += 1;
            if accessed_row_counter == YIELD_INTERVAL {
                accessed_row_counter = 0;
                yield_now().await;
            }
            if !indent.is_line_empty() && indent.raw_len() < target_indent.raw_len() {
                start_indent = Some((row, indent));
                break;
            }
        }
        let (start_row, start_indent_size) = start_indent?;

        let mut end_indent = (end, None);
        for (row, indent, _) in self.line_indents(target_row, |_| true) {
            if row > end {
                break;
            }
            accessed_row_counter += 1;
            if accessed_row_counter == YIELD_INTERVAL {
                accessed_row_counter = 0;
                yield_now().await;
            }
            if !indent.is_line_empty() && indent.raw_len() < target_indent.raw_len() {
                end_indent = (MultiBufferRow(row.0.saturating_sub(1)), Some(indent));
                break;
            }
        }
        let (end_row, end_indent_size) = end_indent;

        let indent = if let Some(end_indent_size) = end_indent_size {
            if start_indent_size.raw_len() > end_indent_size.raw_len() { start_indent_size } else { end_indent_size }
        } else {
            start_indent_size
        };

        Some((start_row..end_row, indent))
    }

    pub fn indent_guides_in_range<T: ToPoint>(
        &self,
        range: Range<T>,
        ignore_disabled_for_language: bool,
        cx: &App,
    ) -> impl Iterator<Item = IndentGuide> {
        let range = range.start.to_point(self)..range.end.to_point(self);
        let start_row = MultiBufferRow(range.start.row);
        let end_row = MultiBufferRow(range.end.row);

        let mut row_indents = self.line_indents(start_row, |_| INDENT_GUIDES.enabled || ignore_disabled_for_language);

        let mut result = Vec::new();
        let mut indent_stack = SmallVec::<[IndentGuide; 8]>::new();

        let mut prev_settings = None;
        while let Some((first_row, mut line_indent, buffer)) = row_indents.next() {
            if first_row > end_row {
                break;
            }
            let current_depth = indent_stack.len() as u32;

            // Avoid retrieving the language settings repeatedly for every buffer row.
            if let Some((prev_buffer_id, _)) = &prev_settings
                && prev_buffer_id != &buffer.buffer_id()
            {
                prev_settings.take();
            }
            let settings = &prev_settings
                .get_or_insert_with(|| (buffer.buffer_id(), LanguageSettings::for_buffer_snapshot(buffer, None, cx)))
                .1;
            let tab_size = settings.tab_size.get();

            // When encountering empty, continue until found useful line indent
            // then add to the indent stack with the depth found
            let mut found_indent = false;
            let mut last_row = first_row;
            if line_indent.is_line_blank() {
                while !found_indent {
                    let Some((target_row, new_line_indent, _)) = row_indents.next() else {
                        break;
                    };
                    const TRAILING_ROW_SEARCH_LIMIT: u32 = 25;
                    if target_row > MultiBufferRow(end_row.0 + TRAILING_ROW_SEARCH_LIMIT) {
                        break;
                    }

                    if new_line_indent.is_line_blank() {
                        continue;
                    }
                    last_row = target_row.min(end_row);
                    line_indent = new_line_indent;
                    found_indent = true;
                    break;
                }
            } else {
                found_indent = true;
            }

            let depth = if found_indent { line_indent.len(tab_size) / tab_size } else { 0 };

            match depth.cmp(&current_depth) {
                cmp::Ordering::Less => {
                    for _ in 0..(current_depth - depth) {
                        let mut indent = indent_stack.pop().expect("indent stack is as deep as current depth");
                        if last_row != first_row {
                            // In this case, we landed on an empty row, had to seek forward,
                            // and discovered that the indent we where on is ending.
                            // This means that the last display row must
                            // be on line that ends this indent range, so we
                            // should display the range up to the first non-empty line
                            indent.end_row = MultiBufferRow(first_row.0.saturating_sub(1));
                        }

                        result.push(indent);
                    }
                }
                cmp::Ordering::Greater => {
                    for next_depth in current_depth..depth {
                        indent_stack.push(IndentGuide {
                            buffer_id: buffer.buffer_id(),
                            start_row: first_row,
                            end_row: last_row,
                            depth: next_depth,
                            tab_size,
                            settings: INDENT_GUIDES,
                        });
                    }
                }
                cmp::Ordering::Equal => {}
            }

            for indent in &mut indent_stack {
                indent.end_row = last_row;
            }
        }

        result.extend(indent_stack);
        result.into_iter()
    }

    pub fn language_at<T: ToOffset>(&self, offset: T) -> Option<&Arc<Language>> {
        let (buffer, offset) = self.point_to_buffer_offset(offset)?;
        buffer.language_at(offset)
    }

    pub fn language_settings_at<T: ToOffset>(&self, point: T, cx: &App) -> Arc<LanguageSettings> {
        if let Some((buffer, offset)) = self.point_to_buffer_offset(point) {
            buffer.settings_at(offset, cx)
        } else {
            Arc::clone(&AllLanguageSettings::get_global(cx).fallback)
        }
    }

    pub fn language_scope_at<T: ToOffset>(&self, point: T) -> Option<LanguageScope> {
        let (buffer, offset) = self.point_to_buffer_offset(point)?;
        buffer.language_scope_at(offset)
    }

    pub fn char_classifier_at<T: ToOffset>(&self, point: T) -> CharClassifier {
        self.point_to_buffer_offset(point).map_or_default(|(buffer, offset)| buffer.char_classifier_at(offset))
    }

    pub fn language_indent_size_at<T: ToOffset>(&self, position: T, cx: &App) -> Option<IndentSize> {
        let (buffer_snapshot, offset) = self.point_to_buffer_offset(position)?;
        Some(buffer_snapshot.language_indent_size_at(offset, cx))
    }

    pub fn has_diagnostics(&self) -> bool {
        self.excerpts.iter().any(|excerpt| excerpt.buffer_snapshot(self).has_diagnostics())
    }

    pub fn diagnostic_group(
        &self,
        buffer_id: BufferId,
        group_id: usize,
    ) -> impl Iterator<Item = DiagnosticEntryRef<'_, Point>> + '_ {
        self.lift_buffer_metadata::<Point, _, _>(Point::zero()..self.max_point(), move |buffer, range| {
            if buffer.buffer_id() != buffer_id {
                return None;
            }
            Some(
                buffer
                    .diagnostics_in_range(range, false)
                    .filter(move |diagnostic| diagnostic.diagnostic.group_id == group_id)
                    .map(move |DiagnosticEntryRef { diagnostic, range }| (range, diagnostic)),
            )
        })
        .map(|(range, diagnostic, _)| DiagnosticEntryRef { diagnostic, range })
    }

    pub fn diagnostics_in_range<'a, MBD>(
        &'a self,
        range: Range<MBD>,
    ) -> impl Iterator<Item = DiagnosticEntryRef<'a, MBD>> + 'a
    where
        MBD::TextDimension: 'a
            + text::ToOffset
            + text::FromAnchor
            + Sub<Output = MBD::TextDimension>
            + fmt::Debug
            + ops::Add<Output = MBD::TextDimension>
            + AddAssign
            + Ord,
        MBD: MultiBufferDimension
            + Ord
            + Sub<Output = MBD::TextDimension>
            + ops::Add<MBD::TextDimension, Output = MBD>
            + AddAssign<MBD::TextDimension>
            + 'a,
    {
        self.lift_buffer_metadata::<MBD, _, _>(range, move |buffer, buffer_range| {
            Some(buffer.diagnostics_in_range(buffer_range, false).map(|entry| (entry.range, entry.diagnostic)))
        })
        .map(|(range, diagnostic, _)| DiagnosticEntryRef { diagnostic, range })
    }

    pub fn diagnostics_with_buffer_ids_in_range<MBD>(
        &self,
        range: Range<MBD>,
    ) -> impl Iterator<Item = (BufferId, DiagnosticEntryRef<'_, MBD>)> + '_
    where
        MBD: MultiBufferDimension
            + Ord
            + Sub<Output = MBD::TextDimension>
            + ops::Add<MBD::TextDimension, Output = MBD>
            + AddAssign<MBD::TextDimension>,
        MBD::TextDimension: Sub<Output = MBD::TextDimension>
            + ops::Add<Output = MBD::TextDimension>
            + text::ToOffset
            + text::FromAnchor
            + AddAssign<MBD::TextDimension>
            + Ord,
    {
        self.lift_buffer_metadata::<MBD, _, _>(range, move |buffer, buffer_range| {
            Some(buffer.diagnostics_in_range(buffer_range, false).map(|entry| (entry.range, entry.diagnostic)))
        })
        .map(|(range, diagnostic, excerpt)| {
            (excerpt.buffer_snapshot(self).buffer_id(), DiagnosticEntryRef { diagnostic, range })
        })
    }

    pub fn syntax_ancestor<T: ToOffset>(
        &self,
        range: Range<T>,
    ) -> Option<(tree_sitter::Node<'_>, Range<MultiBufferOffset>)> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let results = self.map_excerpt_ranges(range, |buffer, excerpt_range, input_buffer_range| {
            let Some(node) = buffer.syntax_ancestor(input_buffer_range) else {
                return vec![];
            };
            let node_range = node.byte_range();
            if excerpt_range.context.start.0 <= node_range.start && node_range.end <= excerpt_range.context.end.0 {
                vec![(BufferOffset(node_range.start)..BufferOffset(node_range.end), node)]
            } else {
                vec![]
            }
        })?;
        let (output_range, node) = results.into_iter().next()?;
        Some((node, output_range))
    }

    pub fn outline(&self, theme: Option<&SyntaxTheme>) -> Option<Outline<Anchor>> {
        let buffer_snapshot = self.as_singleton()?;
        let excerpt = self.excerpts.first()?;
        let path_key_index = excerpt.path_key_index;
        let outline = buffer_snapshot.outline(theme);
        Some(Outline::new(
            outline
                .items
                .into_iter()
                .map(|item| OutlineItem {
                    depth: item.depth,
                    range: Anchor::range_in_buffer(path_key_index, item.range),
                    selection_range: Anchor::range_in_buffer(path_key_index, item.selection_range),
                    source_range_for_text: Anchor::range_in_buffer(path_key_index, item.source_range_for_text),
                    text: item.text,
                    highlight_ranges: item.highlight_ranges,
                    name_ranges: item.name_ranges,
                    body_range: item.body_range.map(|body_range| Anchor::range_in_buffer(path_key_index, body_range)),
                    annotation_range: item
                        .annotation_range
                        .map(|annotation_range| Anchor::range_in_buffer(path_key_index, annotation_range)),
                })
                .collect(),
        ))
    }

    pub fn symbols_containing<T: ToOffset>(
        &self,
        offset: T,
        theme: Option<&SyntaxTheme>,
    ) -> Option<(BufferId, Vec<OutlineItem<Anchor>>)> {
        let anchor = self.anchor_before(offset);
        let target = anchor.try_seek_target(self)?;
        let (_, _, excerpt) = self.excerpts.find((), &target, Bias::Left);
        let excerpt = excerpt?;
        let buffer_snapshot = excerpt.buffer_snapshot(self);
        Some((
            buffer_snapshot.buffer_id(),
            buffer_snapshot
                .symbols_containing(
                    anchor.excerpt_anchor().map_or_else(
                        || text::Anchor::min_for_buffer(buffer_snapshot.buffer_id()),
                        |anchor| anchor.text_anchor(),
                    ),
                    theme,
                )
                .into_iter()
                .map(|item| OutlineItem {
                    depth: item.depth,
                    selection_range: Anchor::range_in_buffer(excerpt.path_key_index, item.selection_range),
                    source_range_for_text: Anchor::range_in_buffer(excerpt.path_key_index, item.source_range_for_text),
                    range: Anchor::range_in_buffer(excerpt.path_key_index, item.range),
                    text: item.text,
                    highlight_ranges: item.highlight_ranges,
                    name_ranges: item.name_ranges,
                    body_range: item
                        .body_range
                        .map(|body_range| Anchor::range_in_buffer(excerpt.path_key_index, body_range)),
                    annotation_range: item
                        .annotation_range
                        .map(|body_range| Anchor::range_in_buffer(excerpt.path_key_index, body_range)),
                })
                .collect(),
        ))
    }
}
