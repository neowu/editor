use std::cmp;
use std::ops::Range;
use std::ptr;

use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::Element as _;
use gpui::InteractiveElement as _;
use gpui::ParentElement as _;
use gpui::Pixels;
use gpui::Styled as _;
use gpui::Window;
use gpui::point;
use language::Diagnostic;
use multi_buffer::Anchor;
use multi_buffer::ExcerptRange;
use multi_buffer::MultiBufferSnapshot;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use sum_tree::Bias;
use text::ToPoint as _;
use ui::prelude::Color;
use ui::prelude::h_flex;

use super::CreaseTrailerLayout;
use super::EditorElement;
use super::EditorLayout;
use super::LineWithInvisibles;
use super::PointDiagnostic;
use crate::DisplayPoint;
use crate::DisplayRow;
use crate::EditorSnapshot;
use crate::EditorStyle;
use crate::INLINE_DIAGNOSTICS_PADDING_COLUMNS;
use crate::Point;
use crate::RowExt as _;
use crate::ToPoint as _;
use crate::display_map::ToDisplayPoint as _;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollPixelOffset;

impl EditorElement {
    pub(super) fn layout_inline_diagnostics(
        &self,
        line_layouts: &[LineWithInvisibles],
        crease_trailers: &[Option<CreaseTrailerLayout>],
        row_block_types: &FxHashMap<DisplayRow, bool>,
        content_origin: gpui::Point<Pixels>,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        start_row: DisplayRow,
        end_row: DisplayRow,
        line_height: Pixels,
        em_width: Pixels,
        style: &EditorStyle,
        window: &mut Window,
        cx: &mut App,
    ) -> FxHashMap<DisplayRow, AnyElement> {
        if !self.editor.read(cx).inline_diagnostics_enabled() {
            return FxHashMap::default();
        }

        let active_diagnostics_group = self.editor.read(cx).active_diagnostic_group_id();

        let diagnostics_by_rows = self.editor.update(cx, |editor, cx| {
            let snapshot = editor.snapshot(window, cx);
            editor
                .inline_diagnostics
                .iter()
                .filter(|(_, diagnostic)| diagnostic.severity <= lsp_types::DiagnosticSeverity::ERROR)
                .filter(|(_, diagnostic)| match active_diagnostics_group {
                    Some(active_diagnostics_group) => {
                        // Active diagnostics are all shown in the editor already, no need to display them inline
                        diagnostic.group_id != active_diagnostics_group
                    }
                    None => true,
                })
                .map(|(point, diag)| (point.to_display_point(&snapshot), diag.clone()))
                .skip_while(|(point, _)| point.row() < start_row)
                .take_while(|(point, _)| point.row() < end_row)
                .filter(|(point, _)| !row_block_types.contains_key(&point.row()))
                .fold(FxHashMap::default(), |mut acc, (point, diagnostic)| {
                    acc.entry(point.row()).or_insert_with(Vec::new).push(diagnostic);
                    acc
                })
        });

        if diagnostics_by_rows.is_empty() {
            return FxHashMap::default();
        }

        let severity_to_color = |sev: &lsp_types::DiagnosticSeverity| match *sev {
            lsp_types::DiagnosticSeverity::ERROR => Color::Error,
            lsp_types::DiagnosticSeverity::WARNING => Color::Warning,
            lsp_types::DiagnosticSeverity::INFORMATION => Color::Info,
            lsp_types::DiagnosticSeverity::HINT => Color::Hint,
            _ => Color::Error,
        };

        let padding = INLINE_DIAGNOSTICS_PADDING_COLUMNS * em_width;

        let mut elements = FxHashMap::default();
        for (row, mut diagnostics) in diagnostics_by_rows {
            diagnostics.sort_by_key(|diagnostic| {
                (
                    diagnostic.severity,
                    cmp::Reverse(diagnostic.is_primary),
                    diagnostic.start.row,
                    diagnostic.start.column,
                )
            });

            let Some(diagnostic_to_render) =
                diagnostics.iter().find(|diagnostic| diagnostic.is_primary).or_else(|| diagnostics.first())
            else {
                continue;
            };

            let pos_y = content_origin.y + line_height * (row.0 as f64 - scroll_position.y) as f32;

            let window_ix = row.0.saturating_sub(start_row.0) as usize;
            let pos_x = {
                let crease_trailer_layout = crease_trailers.get(window_ix).and_then(Option::as_ref);
                let line_layout = &line_layouts[window_ix];

                let line_end = if let Some(crease_trailer) = crease_trailer_layout {
                    crease_trailer.bounds.right()
                } else {
                    Pixels::from(
                        ScrollPixelOffset::from(content_origin.x + line_layout.width) - scroll_pixel_position.x,
                    )
                };

                line_end + padding
            };

            let mut element = h_flex()
                .id(("diagnostic", row.0))
                .h(line_height)
                .w_full()
                .px_1()
                .rounded_xs()
                .bg(severity_to_color(&diagnostic_to_render.severity).color().opacity(0.05))
                .text_color(severity_to_color(&diagnostic_to_render.severity).color())
                .text_sm()
                .font(style.text.font())
                .child(diagnostic_to_render.message.clone())
                .into_any();

            element.prepaint_as_root(point(pos_x, pos_y), AvailableSpace::min_size(), window, cx);

            elements.insert(row, element);
        }

        elements
    }

    pub(super) fn populate_point_diagnostics(
        &self,
        snapshot: &EditorSnapshot,
        visible_rows: Range<DisplayRow>,
        line_layouts: &mut [LineWithInvisibles],
    ) {
        if !snapshot.display_snapshot.diagnostics_visible() || visible_rows.is_empty() {
            return;
        }

        let display_snapshot = &snapshot.display_snapshot;
        let buffer_snapshot = snapshot.buffer_snapshot();

        let query_start = display_snapshot.display_point_to_point(DisplayPoint::new(visible_rows.start, 0), Bias::Left);
        let query_end_display =
            display_snapshot.clip_ignoring_line_ends(DisplayPoint::new(visible_rows.end, 0), Bias::Right);
        let query_end = display_snapshot.display_point_to_point(query_end_display, Bias::Right);

        for (point, diagnostic) in Self::point_diagnostics_in_range(buffer_snapshot, query_start..query_end) {
            if display_snapshot.intersects_fold(point) {
                continue;
            }

            let Some(underline) = display_snapshot.diagnostic_underline_style(
                diagnostic.severity,
                diagnostic.underline,
                diagnostic.is_unnecessary,
                &self.style,
            ) else {
                continue;
            };

            let display_point = point.to_display_point(display_snapshot);
            if !visible_rows.contains(&display_point.row()) {
                continue;
            }

            let line_ix = display_point.row().minus(visible_rows.start) as usize;
            let Some(line_layout) = line_layouts.get_mut(line_ix) else {
                continue;
            };
            let point_diagnostic =
                PointDiagnostic { column: display_point.column(), underline, severity: diagnostic.severity };

            line_layout.add_point_diagnostic(point_diagnostic);
        }
    }

    fn point_diagnostic_anchor(
        snapshot: &MultiBufferSnapshot,
        excerpt: &ExcerptRange<text::Anchor>,
        text_anchor: text::Anchor,
    ) -> Option<Anchor> {
        let buffer = snapshot.buffer_for_id(text_anchor.buffer_id)?;
        if !excerpt.contains(&text_anchor, buffer) {
            return None;
        }
        let anchor = text_anchor.bias_right(buffer);
        let anchor = if excerpt.context.start.cmp(&anchor, buffer).is_gt() {
            excerpt.context.start
        } else if excerpt.context.end.cmp(&anchor, buffer).is_lt() {
            excerpt.context.end
        } else {
            anchor
        };
        snapshot.anchor_in_buffer(anchor)
    }

    fn add_point_diagnostic<'a>(
        point_diagnostics: &mut Vec<(Point, &'a Diagnostic)>,
        seen: &mut FxHashSet<(Point, *const Diagnostic)>,
        range: &Range<Point>,
        point: Point,
        diagnostic: &'a Diagnostic,
    ) {
        if point >= range.start && point <= range.end && seen.insert((point, ptr::from_ref(diagnostic))) {
            point_diagnostics.push((point, diagnostic));
        }
    }

    fn point_diagnostics_in_range(snapshot: &MultiBufferSnapshot, range: Range<Point>) -> Vec<(Point, &Diagnostic)> {
        let mut point_diagnostics = Vec::new();
        let mut seen = FxHashSet::default();

        for (buffer, buffer_range, excerpt) in snapshot.range_to_buffer_ranges(range.clone()) {
            for entry in buffer
                .diagnostic_entries_in_range(buffer_range, false)
                .filter(|entry| entry.range.start == entry.range.end)
            {
                let Some(anchor) = Self::point_diagnostic_anchor(snapshot, &excerpt, entry.range.start) else {
                    continue;
                };
                Self::add_point_diagnostic(
                    &mut point_diagnostics,
                    &mut seen,
                    &range,
                    anchor.to_point(snapshot),
                    &entry.diagnostic,
                );
            }
        }

        for (_, _, deleted_hunk_anchor) in snapshot.range_to_buffer_ranges_with_deleted_hunks(range.clone()) {
            let Some(deleted_hunk_anchor) = deleted_hunk_anchor else {
                continue;
            };
            let Some((live_anchor, buffer)) = snapshot.anchor_to_buffer_anchor(deleted_hunk_anchor) else {
                continue;
            };
            let Some((_, excerpt)) = snapshot.excerpt_containing(deleted_hunk_anchor..deleted_hunk_anchor) else {
                continue;
            };
            let live_point = live_anchor.to_point(buffer);

            for entry in buffer
                .diagnostic_entries_in_range(live_point..live_point, false)
                .filter(|entry| entry.range.start == entry.range.end)
            {
                if entry.range.start.to_point(buffer) != live_point {
                    continue;
                }
                let Some(anchor) = Self::point_diagnostic_anchor(snapshot, &excerpt, entry.range.start) else {
                    continue;
                };
                Self::add_point_diagnostic(
                    &mut point_diagnostics,
                    &mut seen,
                    &range,
                    anchor.to_point(snapshot),
                    &entry.diagnostic,
                );
            }
        }

        point_diagnostics
    }

    pub(super) fn paint_inline_diagnostics(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        for mut inline_diagnostic in layout.inline_diagnostics.drain() {
            inline_diagnostic.1.paint(window, cx);
        }
    }
}
