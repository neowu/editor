use std::cmp;
use std::collections::BTreeSet;
use std::ops::Range;
use std::ptr;

use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::Bounds;
use gpui::Element as _;
use gpui::InteractiveElement as _;
use gpui::ParentElement as _;
use gpui::Pixels;
use gpui::ScaledPixels;
use gpui::Styled as _;
use gpui::UnderlineStyle;
use gpui::Window;
use gpui::point;
use language::Diagnostic;
use multi_buffer::Anchor;
use multi_buffer::ExcerptRange;
use multi_buffer::MultiBufferSnapshot;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use sum_tree::Bias;
use text::ToPoint as _;
use ui::prelude::Color;
use ui::prelude::h_flex;

use super::CreaseTrailerLayout;
use super::EditorElement;
use super::EditorLayout;
use super::LineWithInvisibles;
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

#[derive(Debug, Clone, Copy)]
pub(super) struct PointDiagnostic {
    pub(super) column: u32,
    underline: UnderlineStyle,
    pub(super) severity: lsp::DiagnosticSeverity,
}

pub(super) struct DiagnosticUnderline {
    pub(super) origin: gpui::Point<Pixels>,
    pub(super) width: Pixels,
    pub(super) style: UnderlineStyle,
    pub(super) bounds: Bounds<ScaledPixels>,
    pub(super) span: Range<ScaledPixels>,
    pub(super) severity: lsp::DiagnosticSeverity,
    pub(super) is_point: bool,
}

impl DiagnosticUnderline {
    pub(super) fn point(
        diagnostic: &PointDiagnostic,
        origin: gpui::Point<Pixels>,
        width: Pixels,
        window: &Window,
    ) -> Self {
        let bounds = window.underline_bounds(origin, width, &diagnostic.underline);
        Self {
            origin,
            width,
            style: diagnostic.underline,
            bounds,
            span: bounds.left()..bounds.right(),
            severity: diagnostic.severity,
            is_point: true,
        }
    }

    fn paint(&self, span: Range<ScaledPixels>, window: &mut Window) {
        window.paint_underline_with_exclusions(
            self.origin,
            self.width,
            &self.style,
            &[self.bounds.left()..span.start, span.end..self.bounds.right()],
        );
    }

    pub(super) fn paint_all(underlines: &[Self], window: &mut Window) {
        if underlines.is_empty() {
            return;
        }
        let mask = window.content_mask().bounds;
        let scale = window.scale_factor();
        let left = ScaledPixels((f32::from(mask.left()) * scale).floor());
        let top = ScaledPixels((f32::from(mask.top()) * scale).floor());
        let right = ScaledPixels((f32::from(mask.right()) * scale).ceil()).max(left);
        let bottom = ScaledPixels((f32::from(mask.bottom()) * scale).ceil()).max(top);
        let mask = Bounds::from_corners(point(left, top), point(right, bottom));
        let mut events = SmallVec::<[_; 8]>::new();
        for (index, underline) in underlines.iter().enumerate() {
            let bounds = underline.bounds.intersect(&mask);
            let start = underline.span.start.max(bounds.left());
            let end = underline.span.end.min(bounds.right());
            if !bounds.is_empty() && start < end {
                events.push((start, true, index));
                events.push((end, false, index));
            }
        }
        events.sort_unstable();

        let mut active = BTreeSet::new();
        let mut previous = None;
        let mut visible: Option<(usize, Range<ScaledPixels>)> = None;
        for (position, is_start, index) in events {
            if let Some(start) = previous
                && start < position
                && let Some(&(_, _, winner)) = active.first()
            {
                if let Some((current, span)) = visible.as_mut()
                    && *current == winner
                    && span.end == start
                {
                    span.end = position;
                } else {
                    if let Some((current, span)) = visible.take() {
                        underlines[current].paint(span, window);
                    }
                    visible = Some((winner, start..position));
                }
            }
            let underline = &underlines[index];
            let key = (underline.severity, underline.is_point, index);
            if is_start {
                active.insert(key);
            } else {
                active.remove(&key);
            }
            previous = Some(position);
        }
        if let Some((winner, span)) = visible {
            underlines[winner].paint(span, window);
        }
    }
}

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
                .filter(|(_, diagnostic)| diagnostic.severity <= lsp::DiagnosticSeverity::ERROR)
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

        let severity_to_color = |sev: &lsp::DiagnosticSeverity| match *sev {
            lsp::DiagnosticSeverity::ERROR => Color::Error,
            lsp::DiagnosticSeverity::WARNING => Color::Warning,
            lsp::DiagnosticSeverity::INFORMATION => Color::Info,
            lsp::DiagnosticSeverity::HINT => Color::Hint,
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
                .bg(severity_to_color(&diagnostic_to_render.severity).color(cx).opacity(0.05))
                .text_color(severity_to_color(&diagnostic_to_render.severity).color(cx))
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
