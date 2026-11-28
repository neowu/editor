use std::any::TypeId;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::HighlightStyle;
use gpui::Hsla;
use gpui::Task;
use gpui::Window;
use language::Point;
use lsp::LanguageServerId;
use multi_buffer::Anchor;
use multi_buffer::MultiBufferOffset;
use multi_buffer::MultiBufferSnapshot;
use multi_buffer::ToPoint as _;
use project::search::SearchQuery;
use rustc_hash::FxHashMap;
use theme::Theme;
use util::ResultExt as _;
use util::paths::PathMatcher;
use util::post_inc;

use super::BackgroundHighlight;
use super::Editor;
use super::EditorMode;
use super::LSP_HIGHLIGHT_DEBOUNCE;
use super::LineHighlight;
use super::NavigationTargetOverlay;
use super::RangeToAnchorExt as _;
use super::RowHighlight;
use super::RowHighlightOptions;
use super::SELECTION_HIGHLIGHT_DEBOUNCE_TIMEOUT;
use crate::display_map::DisplayPoint;
use crate::display_map::DisplayRow;
use crate::display_map::DisplaySnapshot;
use crate::display_map::HighlightKey;
use crate::display_map::NavigationOverlayKey;
use crate::display_map::ToDisplayPoint as _;

impl Editor {
    pub(crate) fn refresh_document_highlights(&mut self, cx: &mut Context<Self>) -> Option<()> {
        if self.pending_rename.is_some() {
            return None;
        }
        // While buffer search shows results, only the matches are highlighted.
        if self.has_background_highlights(HighlightKey::BufferSearchHighlights) {
            self.document_highlights_task.take();
            self.clear_background_highlights(HighlightKey::DocumentHighlightRead, cx);
            self.clear_background_highlights(HighlightKey::DocumentHighlightWrite, cx);
            return None;
        }
        // Multibuffers (search / references results) already highlight their matches;
        // symbol occurrence highlights on top of those are just noise.
        if !self.buffer.read(cx).is_singleton() {
            return None;
        }

        let provider = self.semantics_provider.clone()?;
        let buffer = self.buffer.read(cx);
        let newest_selection = *self.selections.newest_anchor();
        let cursor_position = newest_selection.head();
        let (cursor_buffer, cursor_buffer_position) = buffer.text_anchor_for_position(cursor_position, cx)?;
        let (tail_buffer, tail_buffer_position) = buffer.text_anchor_for_position(newest_selection.tail(), cx)?;
        if cursor_buffer != tail_buffer {
            return None;
        }

        let snapshot = cursor_buffer.read(cx).snapshot();
        let word_ranges = cx.background_spawn(async move {
            // this might look odd to put on the background thread, but
            // `surrounding_word` can be quite expensive as it calls into
            // tree-sitter language scopes
            let (start_word_range, _) = snapshot.surrounding_word(cursor_buffer_position, None);
            let (end_word_range, _) = snapshot.surrounding_word(tail_buffer_position, None);
            (start_word_range, end_word_range)
        });

        self.document_highlights_task = Some(cx.spawn(async move |this, cx| {
            let (start_word_range, end_word_range) = word_ranges.await;
            if start_word_range != end_word_range {
                this.update(cx, |this, cx| {
                    this.document_highlights_task.take();
                    this.clear_background_highlights(HighlightKey::DocumentHighlightRead, cx);
                    this.clear_background_highlights(HighlightKey::DocumentHighlightWrite, cx);
                })
                .ok();
                return;
            }
            cx.background_executor().timer(LSP_HIGHLIGHT_DEBOUNCE).await;

            let highlights = if let Some(highlights) =
                cx.update(|cx| provider.document_highlights(&cursor_buffer, cursor_buffer_position, cx))
            {
                highlights.await.log_err()
            } else {
                None
            };

            if let Some(highlights) = highlights {
                this.update(cx, |this, cx| {
                    if this.pending_rename.is_some()
                        || this.has_background_highlights(HighlightKey::BufferSearchHighlights)
                    {
                        return;
                    }

                    let buffer = this.buffer.read(cx);
                    if buffer
                        .text_anchor_for_position(cursor_position, cx)
                        .is_none_or(|(buffer, _)| buffer != cursor_buffer)
                    {
                        return;
                    }

                    let mut write_ranges = Vec::new();
                    let mut read_ranges = Vec::new();
                    let multibuffer_snapshot = buffer.snapshot(cx);
                    for highlight in highlights {
                        for range in multibuffer_snapshot.buffer_range_to_excerpt_ranges(highlight.range) {
                            if highlight.kind == lsp_types::DocumentHighlightKind::WRITE {
                                write_ranges.push(range);
                            } else {
                                read_ranges.push(range);
                            }
                        }
                    }

                    this.highlight_background(
                        HighlightKey::DocumentHighlightRead,
                        &read_ranges,
                        |_, theme| theme.colors().editor_document_highlight_read_background,
                        cx,
                    );
                    this.highlight_background(
                        HighlightKey::DocumentHighlightWrite,
                        &write_ranges,
                        |_, theme| theme.colors().editor_document_highlight_write_background,
                        cx,
                    );
                })
                .log_err();
            }
        }));
        None
    }

    pub(super) fn refresh_document_highlights_for_server(
        &mut self,
        server_id: Option<LanguageServerId>,
        cx: &mut Context<Self>,
    ) {
        let server_relevant = server_id.is_none_or(|server_id| {
            let Some(project) = self.project.as_ref() else {
                return false;
            };
            let cursor_position = self.selections.newest_anchor().head();
            self.buffer.read(cx).text_anchor_for_position(cursor_position, cx).is_some_and(|(cursor_buffer, _)| {
                project
                    .read(cx)
                    .lsp_store()
                    .read(cx)
                    .relevant_server_ids_for_capability_check(&cursor_buffer, cx)
                    .contains(&server_id)
            })
        });
        if server_relevant {
            self.refresh_document_highlights(cx);
        }
    }

    fn prepare_highlight_query_from_selection(
        &self,
        snapshot: &DisplaySnapshot,
        cx: &mut Context<Editor>,
    ) -> Option<(String, Range<Anchor>)> {
        if matches!(self.mode, EditorMode::SingleLine) {
            return None;
        }
        if !self.use_selection_highlight {
            return None;
        }
        // Same as document highlights: no occurrence highlights in multibuffers.
        if !self.buffer.read(cx).is_singleton() {
            return None;
        }
        // LSP document highlights take precedence; selected-text highlights are only a
        // fallback for files without a server that provides them (both at once is noise).
        if let Some(provider) = self.semantics_provider.clone()
            && let Some(buffer) = self.buffer.read(cx).as_singleton()
            && provider.supports_document_highlights(&buffer, cx)
        {
            return None;
        }
        // Same as document highlights: while buffer search shows results, only the matches
        // are highlighted (a literal selection highlight would also mark non-matches, e.g.
        // mid-line occurrences for `^something`).
        if self.has_background_highlights(HighlightKey::BufferSearchHighlights) {
            return None;
        }
        if self.selections.count() != 1 || self.selections.line_mode() {
            return None;
        }
        let selection = self.selections.newest::<Point>(snapshot);
        // If the selection spans multiple rows OR it is empty
        if selection.start.row != selection.end.row || selection.start.column == selection.end.column {
            return None;
        }
        let selection_anchor_range = selection.range().to_anchors(snapshot.buffer_snapshot());
        let query = snapshot.buffer_snapshot().text_for_range(selection_anchor_range.clone()).collect::<String>();
        if query.trim().is_empty() {
            return None;
        }
        Some((query, selection_anchor_range))
    }

    fn update_selection_occurrence_highlights(
        &self,
        multi_buffer_snapshot: MultiBufferSnapshot,
        query_text: String,
        query_range: Range<Anchor>,
        multi_buffer_range_to_query: Range<Point>,
        use_debounce: bool,
        window: &Window,
        cx: &Context<Editor>,
    ) -> Task<()> {
        cx.spawn_in(window, async move |editor, cx| {
            if use_debounce {
                cx.background_executor().timer(SELECTION_HIGHLIGHT_DEBOUNCE_TIMEOUT).await;
            }
            let match_task = cx.background_spawn(async move {
                let buffer_ranges = multi_buffer_snapshot
                    .range_to_buffer_ranges(multi_buffer_range_to_query.clone())
                    .into_iter()
                    .filter(|(_, excerpt_visible_range, _)| !excerpt_visible_range.is_empty());
                let mut match_ranges = Vec::new();
                let Ok(regex) = SearchQuery::text(
                    query_text,
                    false,
                    false,
                    false,
                    PathMatcher::default(),
                    PathMatcher::default(),
                    false,
                    None,
                ) else {
                    return Vec::default();
                };
                let query_range = query_range.to_anchors(&multi_buffer_snapshot);
                for (buffer_snapshot, search_range, _) in buffer_ranges {
                    match_ranges.extend(
                        regex
                            .search(buffer_snapshot, Some(search_range.start.0..search_range.end.0))
                            .await
                            .into_iter()
                            .filter_map(|match_range| {
                                let match_start = buffer_snapshot.anchor_after(search_range.start + match_range.start);
                                let match_end = buffer_snapshot.anchor_before(search_range.start + match_range.end);
                                {
                                    let range = multi_buffer_snapshot.anchor_in_buffer(match_start)?
                                        ..multi_buffer_snapshot.anchor_in_buffer(match_end)?;
                                    Some(range).filter(|match_anchor_range| match_anchor_range != &query_range)
                                }
                            }),
                    );
                }
                match_ranges
            });
            let match_ranges = match_task.await;
            editor
                .update_in(cx, |editor, _, cx| {
                    if use_debounce {
                        editor.clear_background_highlights(HighlightKey::SelectedTextHighlight, cx);
                        editor.debounced_selection_highlight_complete = true;
                    } else if editor.debounced_selection_highlight_complete {
                        return;
                    }
                    if !match_ranges.is_empty() {
                        editor.highlight_background(
                            HighlightKey::SelectedTextHighlight,
                            &match_ranges,
                            |_, theme| theme.colors().editor_document_highlight_bracket_background,
                            cx,
                        );
                    }
                })
                .log_err();
        })
    }

    pub(crate) fn refresh_selected_text_highlights(
        &mut self,
        snapshot: &DisplaySnapshot,
        on_buffer_edit: bool,
        window: &Window,
        cx: &mut Context<Editor>,
    ) {
        let Some((query_text, query_range)) = self.prepare_highlight_query_from_selection(snapshot, cx) else {
            self.clear_background_highlights(HighlightKey::SelectedTextHighlight, cx);
            self.quick_selection_highlight_task.take();
            self.debounced_selection_highlight_task.take();
            self.debounced_selection_highlight_complete = false;
            return;
        };
        let display_snapshot = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let multi_buffer_snapshot = self.buffer().read(cx).snapshot(cx);
        let query_changed = self
            .quick_selection_highlight_task
            .as_ref()
            .is_none_or(|(prev_anchor_range, _)| prev_anchor_range != &query_range);
        if query_changed {
            self.debounced_selection_highlight_complete = false;
        }
        if on_buffer_edit || query_changed {
            self.quick_selection_highlight_task = Some((
                query_range.clone(),
                self.update_selection_occurrence_highlights(
                    snapshot.buffer.clone(),
                    query_text.clone(),
                    query_range.clone(),
                    self.multi_buffer_visible_range(&display_snapshot, cx),
                    false,
                    window,
                    cx,
                ),
            ));
        }
        if on_buffer_edit
            || self
                .debounced_selection_highlight_task
                .as_ref()
                .is_none_or(|(prev_anchor_range, _)| prev_anchor_range != &query_range)
        {
            let multi_buffer_start =
                multi_buffer_snapshot.anchor_before(MultiBufferOffset(0)).to_point(&multi_buffer_snapshot);
            let multi_buffer_end =
                multi_buffer_snapshot.anchor_after(multi_buffer_snapshot.len()).to_point(&multi_buffer_snapshot);
            let multi_buffer_full_range = multi_buffer_start..multi_buffer_end;
            self.debounced_selection_highlight_task = Some((
                query_range.clone(),
                self.update_selection_occurrence_highlights(
                    snapshot.buffer.clone(),
                    query_text,
                    query_range,
                    multi_buffer_full_range,
                    true,
                    window,
                    cx,
                ),
            ));
        }
    }

    /// Adds a row highlight for the given range. If a row has multiple highlights, the
    /// last highlight added will be used.
    ///
    /// If the range ends at the beginning of a line, then that line will not be highlighted.
    pub fn highlight_rows<T: 'static>(
        &mut self,
        range: Range<Anchor>,
        color: fn(&App) -> Hsla,
        options: RowHighlightOptions,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.buffer().read(cx).snapshot(cx);
        let row_highlights = self.highlighted_rows.entry(TypeId::of::<T>()).or_default();
        let ix = row_highlights.binary_search_by(|highlight| {
            Ordering::Equal
                .then_with(|| highlight.range.start.cmp(&range.start, &snapshot))
                .then_with(|| highlight.range.end.cmp(&range.end, &snapshot))
        });

        if let Err(mut ix) = ix {
            let index = post_inc(&mut self.highlight_order);

            // If this range intersects with the preceding highlight, then merge it with
            // the preceding highlight. Otherwise insert a new highlight.
            let mut merged = false;
            if ix > 0 {
                let prev_highlight = &mut row_highlights[ix - 1];
                if prev_highlight.range.end.cmp(&range.start, &snapshot).is_ge() {
                    ix -= 1;
                    if prev_highlight.range.end.cmp(&range.end, &snapshot).is_lt() {
                        prev_highlight.range.end = range.end;
                    }
                    merged = true;
                    prev_highlight.index = index;
                    prev_highlight.color = color;
                    prev_highlight.options = options;
                }
            }

            if !merged {
                row_highlights.insert(ix, RowHighlight { range, index, color, options, type_id: TypeId::of::<T>() });
            }

            // If any of the following highlights intersect with this one, merge them.
            while let Some(next_highlight) = row_highlights.get(ix + 1) {
                let highlight = &row_highlights[ix];
                if next_highlight.range.start.cmp(&highlight.range.end, &snapshot).is_le() {
                    if next_highlight.range.end.cmp(&highlight.range.end, &snapshot).is_gt() {
                        row_highlights[ix].range.end = next_highlight.range.end;
                    }
                    row_highlights.remove(ix + 1);
                } else {
                    break;
                }
            }
        }
    }

    /// Remove any highlighted row ranges of the given type that intersect the
    /// given ranges.
    pub fn remove_highlighted_rows<T: 'static>(
        &mut self,
        ranges_to_remove: Vec<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.buffer().read(cx).snapshot(cx);
        let row_highlights = self.highlighted_rows.entry(TypeId::of::<T>()).or_default();
        let mut ranges_to_remove = ranges_to_remove.iter().peekable();
        row_highlights.retain(|highlight| {
            while let Some(range_to_remove) = ranges_to_remove.peek() {
                match range_to_remove.end.cmp(&highlight.range.start, &snapshot) {
                    Ordering::Less | Ordering::Equal => {
                        ranges_to_remove.next();
                    }
                    Ordering::Greater => match range_to_remove.start.cmp(&highlight.range.end, &snapshot) {
                        Ordering::Less | Ordering::Equal => {
                            return false;
                        }
                        Ordering::Greater => break,
                    },
                }
            }

            true
        });
    }

    /// Clear all anchor ranges for a certain highlight context type, so no corresponding rows will be highlighted.
    pub fn clear_row_highlights<T: 'static>(&mut self) {
        self.highlighted_rows.remove(&TypeId::of::<T>());
    }

    /// For a highlight given context type, gets all anchor ranges that will be used for row highlighting.
    pub fn highlighted_rows<'a, T: 'static>(&'a self, cx: &'a App) -> impl 'a + Iterator<Item = (Range<Anchor>, Hsla)> {
        self.highlighted_rows
            .get(&TypeId::of::<T>())
            .map_or(&[] as &[_], |highlights| highlights.as_slice())
            .iter()
            .map(|highlight| (highlight.range.clone(), (highlight.color)(cx)))
    }

    pub(crate) fn highlighted_display_rows_in_range(
        &self,
        anchor_range: Range<Anchor>,
        display_row_range: Range<DisplayRow>,
        snapshot: &DisplaySnapshot,
        cx: &App,
    ) -> BTreeMap<DisplayRow, LineHighlight> {
        if display_row_range.is_empty() {
            return BTreeMap::default();
        }

        let buffer_snapshot = snapshot.buffer_snapshot();
        let mut used_highlight_orders = FxHashMap::default();
        self.highlighted_rows
            .values()
            .flat_map(|highlighted_rows| {
                let end_index = highlighted_rows
                    .partition_point(|highlight| highlight.range.start.cmp(&anchor_range.end, buffer_snapshot).is_le());
                // Search within `..end_index` so a highlight whose anchors
                // have drifted to `start > end` can't produce an inverted
                // slice; the filter below drops it either way.
                let start_index = highlighted_rows[..end_index]
                    .partition_point(|highlight| highlight.range.end.cmp(&anchor_range.start, buffer_snapshot).is_lt());
                highlighted_rows[start_index..end_index].iter().filter(|highlight| {
                    highlight.range.end.cmp(&anchor_range.start, buffer_snapshot).is_ge()
                        && highlight.range.start.cmp(&anchor_range.end, buffer_snapshot).is_le()
                })
            })
            .fold(BTreeMap::<DisplayRow, LineHighlight>::new(), |mut unique_rows, highlight| {
                let start = highlight.range.start.to_display_point(snapshot);
                let end = highlight.range.end.to_display_point(snapshot);
                let start_row = start.row().0.max(display_row_range.start.0);
                let mut end_row = if !highlight.range.end.is_max() && end.column() == 0 {
                    end.row().0.saturating_sub(1)
                } else {
                    end.row().0
                };
                end_row = end_row.min(display_row_range.end.0.saturating_sub(1));
                if start_row > end_row {
                    return unique_rows;
                }
                let mut header_rows = snapshot
                    .blocks_in_range(DisplayRow(start_row)..DisplayRow(end_row.saturating_add(1)))
                    .filter(|(_, block)| block.is_header())
                    .map(|(block_row, block)| block_row.0..block_row.0.saturating_add(block.height()))
                    .peekable();
                for row in start_row..=end_row {
                    while header_rows.next_if(|header_range| header_range.end <= row).is_some() {}
                    if header_rows.peek().is_some_and(|header_range| header_range.contains(&row)) {
                        continue;
                    }
                    let used_index = used_highlight_orders.entry(row).or_insert(highlight.index);
                    if highlight.index >= *used_index {
                        *used_index = highlight.index;
                        unique_rows.insert(
                            DisplayRow(row),
                            LineHighlight {
                                include_gutter: highlight.options.include_gutter,
                                border: None,
                                background: (highlight.color)(cx).into(),
                                type_id: Some(highlight.type_id),
                            },
                        );
                    }
                }
                unique_rows
            })
    }

    pub(crate) fn highlighted_display_row_for_autoscroll(&self, snapshot: &DisplaySnapshot) -> Option<DisplayRow> {
        self.highlighted_rows
            .values()
            .flat_map(|highlighted_rows| highlighted_rows.iter())
            .filter(|&highlight| highlight.options.autoscroll)
            .map(|highlight| highlight.range.start.to_display_point(snapshot).row())
            .min()
    }

    pub(crate) fn set_search_within_ranges(&mut self, ranges: &[Range<Anchor>], cx: &mut Context<Self>) {
        self.highlight_background(
            HighlightKey::SearchWithinRange,
            ranges,
            |_, colors| colors.colors().editor_document_highlight_read_background,
            cx,
        );
    }

    pub fn highlight_background(
        &mut self,
        key: HighlightKey,
        ranges: &[Range<Anchor>],
        color_fetcher: impl Fn(&usize, &Theme) -> Hsla + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        self.background_highlights.insert(key, (Arc::new(color_fetcher), Arc::from(ranges)));
        self.scrollbar_marker_state.dirty = true;
        cx.notify();
    }

    pub fn clear_background_highlights(
        &mut self,
        key: HighlightKey,
        cx: &mut Context<Self>,
    ) -> Option<BackgroundHighlight> {
        let text_highlights = self.background_highlights.remove(&key)?;
        if !text_highlights.1.is_empty() {
            self.scrollbar_marker_state.dirty = true;
            cx.notify();
        }
        Some(text_highlights)
    }

    pub fn insert_gutter_highlight<T: 'static>(
        &mut self,
        range: Range<Anchor>,
        color_fetcher: fn(&App) -> Hsla,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.buffer().read(cx).snapshot(cx);
        let mut highlights =
            self.gutter_highlights.remove(&TypeId::of::<T>()).map_or_default(|(_, highlights)| highlights);
        let ix = highlights.binary_search_by(|highlight| {
            Ordering::Equal
                .then_with(|| highlight.start.cmp(&range.start, &snapshot))
                .then_with(|| highlight.end.cmp(&range.end, &snapshot))
        });
        if let Err(ix) = ix {
            highlights.insert(ix, range);
        }
        self.gutter_highlights.insert(TypeId::of::<T>(), (color_fetcher, highlights));
    }

    pub fn remove_gutter_highlights<T: 'static>(
        &mut self,
        ranges_to_remove: Vec<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.buffer().read(cx).snapshot(cx);
        let Some((color_fetcher, mut gutter_highlights)) = self.gutter_highlights.remove(&TypeId::of::<T>()) else {
            return;
        };
        let mut ranges_to_remove = ranges_to_remove.iter().peekable();
        gutter_highlights.retain(|highlight| {
            while let Some(range_to_remove) = ranges_to_remove.peek() {
                match range_to_remove.end.cmp(&highlight.start, &snapshot) {
                    Ordering::Less | Ordering::Equal => {
                        ranges_to_remove.next();
                    }
                    Ordering::Greater => match range_to_remove.start.cmp(&highlight.end, &snapshot) {
                        Ordering::Less | Ordering::Equal => {
                            return false;
                        }
                        Ordering::Greater => break,
                    },
                }
            }

            true
        });
        self.gutter_highlights.insert(TypeId::of::<T>(), (color_fetcher, gutter_highlights));
    }

    pub(crate) fn has_background_highlights(&self, key: HighlightKey) -> bool {
        self.background_highlights.get(&key).is_some_and(|(_, highlights)| !highlights.is_empty())
    }

    /// Returns all background highlights for a given range.
    ///
    /// The order of highlights is not deterministic, do sort the ranges if needed for the logic.
    pub(crate) fn background_highlights_in_range(
        &self,
        search_range: Range<Anchor>,
        display_snapshot: &DisplaySnapshot,
        theme: &Theme,
    ) -> Vec<(Range<DisplayPoint>, Hsla)> {
        let mut results = Vec::new();
        for (color_fetcher, ranges) in self.background_highlights.values() {
            let start_ix = match ranges.binary_search_by(|probe| {
                let cmp = probe.end.cmp(&search_range.start, display_snapshot.buffer_snapshot());
                if cmp.is_gt() { Ordering::Greater } else { Ordering::Less }
            }) {
                Ok(i) | Err(i) => i,
            };
            for (index, range) in ranges[start_ix..].iter().enumerate() {
                if range.start.cmp(&search_range.end, display_snapshot.buffer_snapshot()).is_ge() {
                    break;
                }

                let color = color_fetcher(&(start_ix + index), theme);
                let start = range.start.to_display_point(display_snapshot);
                let end = range.end.to_display_point(display_snapshot);
                results.push((start..end, color));
            }
        }
        results
    }

    pub(crate) fn gutter_highlights_in_range(
        &self,
        search_range: Range<Anchor>,
        display_snapshot: &DisplaySnapshot,
        cx: &App,
    ) -> Vec<(Range<DisplayPoint>, Hsla)> {
        let mut results = Vec::new();
        for (color_fetcher, ranges) in self.gutter_highlights.values() {
            let color = color_fetcher(cx);
            let start_ix = match ranges.binary_search_by(|probe| {
                let cmp = probe.end.cmp(&search_range.start, display_snapshot.buffer_snapshot());
                if cmp.is_gt() { Ordering::Greater } else { Ordering::Less }
            }) {
                Ok(i) | Err(i) => i,
            };
            for range in &ranges[start_ix..] {
                if range.start.cmp(&search_range.end, display_snapshot.buffer_snapshot()).is_ge() {
                    break;
                }

                let start = range.start.to_display_point(display_snapshot);
                let end = range.end.to_display_point(display_snapshot);
                results.push((start..end, color));
            }
        }
        results
    }

    pub(crate) fn highlight_text(
        &self,
        key: HighlightKey,
        ranges: Vec<Range<Anchor>>,
        style: HighlightStyle,
        cx: &mut Context<Self>,
    ) {
        self.display_map.update(cx, |map, cx| map.highlight_text(key, ranges, style, false, cx));
        cx.notify();
    }

    pub(crate) fn text_highlights<'a>(
        &'a self,
        key: HighlightKey,
        cx: &'a App,
    ) -> Option<(HighlightStyle, &'a [Range<Anchor>])> {
        self.display_map.read(cx).text_highlights(key)
    }

    pub(crate) const fn navigation_overlay_sets(
        &self,
    ) -> &FxHashMap<NavigationOverlayKey, Arc<[NavigationTargetOverlay]>> {
        &self.navigation_overlays
    }

    pub(crate) fn clear_highlights(&self, key: HighlightKey, cx: &mut Context<Self>) {
        let cleared = self.display_map.update(cx, |map, _| map.clear_highlights(key));
        if cleared {
            cx.notify();
        }
    }

    pub fn clear_highlights_with(&mut self, f: &mut dyn FnMut(&HighlightKey) -> bool, cx: &mut Context<Self>) {
        let cleared = self.display_map.update(cx, |map, _| map.clear_highlights_with(f));
        if cleared {
            cx.notify();
        }
    }
}
