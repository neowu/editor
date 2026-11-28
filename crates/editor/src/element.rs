mod actions;
mod blocks;
mod cursor;
mod gutter;
mod header;
mod inline_diagnostics;
mod line;
mod minimap;
mod mouse;
mod popovers;
mod scrollbar;

use std::any::TypeId;
use std::cell::Cell;
use std::cmp;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::iter;
use std::mem;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

pub use actions::install_actions;
pub(crate) use blocks::BlockLayout;
use blocks::RenderBlocksOutput;
use buffer_diff::DiffHunkStatus;
use buffer_diff::DiffHunkStatusKind;
pub use cursor::CursorLayout;
use cursor::SelectionLayout;
use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::Bounds;
use gpui::ContentMask;
use gpui::Context;
use gpui::CursorStyle;
use gpui::Edges;
use gpui::Element;
use gpui::ElementInputHandler;
use gpui::Entity;
use gpui::Focusable as _;
use gpui::FontId;
use gpui::GlobalElementId;
use gpui::Hitbox;
use gpui::HitboxBehavior;
use gpui::Hsla;
use gpui::IntoElement;
use gpui::IsZero as _;
use gpui::Pixels;
use gpui::ShapedLine;
use gpui::SharedString;
use gpui::Size;
use gpui::Style;
use gpui::TextAlign;
use gpui::TextRun;
use gpui::TextStyleRefinement;
use gpui::Window;
use gpui::fill;
use gpui::point;
use gpui::px;
use gpui::relative;
use gpui::size;
use gpui::solid_background;
use gpui::underline_y_offset;
use gutter::Gutter;
use gutter::mark_active_rows;
pub(crate) use header::header_jump_data;
pub(crate) use header::render_buffer_header;
use inline_diagnostics::DiagnosticUnderline;
use inline_diagnostics::PointDiagnostic;
use language::IndentGuideSettings;
use language::LanguageAwareStyling;
use language::language_settings::WHITESPACE_SPACE;
use language::language_settings::WHITESPACE_TAB;
use line::LineFragment;
pub(crate) use line::LineWithInvisibles;
pub(crate) use minimap::MinimapInputs;
use minimap::MinimapLayout;
pub(crate) use minimap::MinimapRowCache;
use multi_buffer::Anchor;
use multi_buffer::MultiBufferOffset;
use multi_buffer::MultiBufferRow;
use multi_buffer::RowInfo;
use rustc_hash::FxHashMap;
use scrollbar::EditorScrollbars;
use scrollbar::ScrollbarLayout;
use scrollbar::ScrollbarLayoutInformation;
use smallvec::SmallVec;
use smallvec::smallvec;
use sum_tree::Bias;
use text::BufferId;
use theme::CursorColors;
use ui::prelude::AbsoluteLength;
use ui::prelude::ElementId;
use ui::utils::merge_highlight_backgrounds;
use workspace::item::Item as _;
use workspace::item::ItemBufferKind;

use crate::CodeActionSource;
use crate::ConflictsOurs;
use crate::ConflictsOursMarker;
use crate::ConflictsOuter;
use crate::ConflictsTheirs;
use crate::ConflictsTheirsMarker;
use crate::DisplayDiffHunk;
use crate::DisplayPoint;
use crate::DisplayRow;
use crate::Editor;
use crate::EditorMode;
use crate::EditorSnapshot;
use crate::EditorStyle;
use crate::FILE_HEADER_HEIGHT;
use crate::GutterDimensions;
use crate::LineHighlight;
use crate::MAX_LINE_LEN;
use crate::Point;
use crate::RowExt as _;
use crate::RowRangeExt as _;
use crate::Selection;
use crate::SizingBehavior;
use crate::SoftWrap;
use crate::ToPoint as _;
use crate::code_context_menus::CodeActionsMenu;
use crate::column_pixels;
use crate::display_map::Block;
use crate::display_map::DisplaySnapshot;
use crate::display_map::EditorMargins;
use crate::display_map::ToDisplayPoint as _;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollPixelOffset;

/// Determines what kinds of highlights should be applied to a lines background.
#[derive(Clone, Copy, Default)]
struct LineHighlightSpec {
    selection: bool,
}

enum LineNumberStyle {
    DiffAdded,
    DiffDeleted,
    Active,
    Inactive,
}

impl LineNumberStyle {
    fn new(is_active: bool, diff_status: Option<DiffHunkStatus>) -> Self {
        match (is_active, diff_status.map(|status| status.kind)) {
            (true, _) => Self::Active,
            (_, Some(DiffHunkStatusKind::Added)) => Self::DiffAdded,
            (_, Some(DiffHunkStatusKind::Deleted)) => Self::DiffDeleted,
            (_, _) => Self::Inactive,
        }
    }

    const fn color(self, colors: &theme::ThemeColors) -> Hsla {
        match self {
            Self::DiffAdded => colors.version_control_added,
            Self::DiffDeleted => colors.version_control_deleted,
            Self::Active => colors.editor_active_line_number,
            Self::Inactive => colors.editor_line_number,
        }
    }
}

pub struct EditorElement {
    editor: Entity<Editor>,
    style: EditorStyle,
    split_side: Option<SplitSide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitSide {
    Left,
    Right,
}

impl EditorElement {
    pub(crate) const SCROLLBAR_WIDTH: Pixels = ui::SCROLLBAR_TRACK_WIDTH;

    pub fn new(editor: &Entity<Editor>, style: EditorStyle) -> Self {
        Self { editor: editor.clone(), style, split_side: None }
    }

    pub(crate) const fn set_split_side(&mut self, side: SplitSide) {
        self.split_side = Some(side);
    }

    fn prepaint_crease_trailers(
        &self,
        trailers: Vec<Option<AnyElement>>,
        lines: &[LineWithInvisibles],
        line_height: Pixels,
        content_origin: gpui::Point<Pixels>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        scroll_position: gpui::Point<ScrollOffset>,
        start_row: DisplayRow,
        em_width: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Option<CreaseTrailerLayout>> {
        trailers
            .into_iter()
            .enumerate()
            .map(|(ix, element)| {
                let mut element = element?;
                let available_space = size(AvailableSpace::MinContent, AvailableSpace::Definite(line_height));
                let size = element.layout_as_root(available_space, window, cx);

                let line = &lines[ix];
                let padding = if line.width == Pixels::ZERO { Pixels::ZERO } else { 4. * em_width };
                let position = point(
                    Pixels::from(scroll_pixel_position.x) + line.width + padding,
                    line_height * (DisplayRow(start_row.0 + ix as u32).as_f64() - scroll_position.y) as f32,
                );
                let centering_offset = point(px(0.), (line_height - size.height) / 2.);
                let origin = content_origin + position + centering_offset;
                element.prepaint_as_root(origin, available_space, window, cx);
                Some(CreaseTrailerLayout { element, bounds: Bounds::new(origin, size) })
            })
            .collect()
    }

    fn layout_inline_code_actions(
        &self,
        display_point: DisplayPoint,
        content_origin: gpui::Point<Pixels>,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        line_height: Pixels,
        snapshot: &EditorSnapshot,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        // Don't show code actions in split diff view
        if self.split_side.is_some() {
            return None;
        }

        if !snapshot.show_code_actions.unwrap_or(true) {
            return None;
        }

        let icon_size = ui::IconSize::XSmall;
        let mut button = self.editor.update(cx, |editor, cx| {
            if !editor.has_available_code_actions_for_selection() {
                return None;
            }
            let active = editor
                .context_menu
                .borrow()
                .as_ref()
                .and_then(|menu| {
                    if let crate::CodeContextMenu::CodeActions(CodeActionsMenu { deployed_from, .. }) = menu {
                        deployed_from.as_ref()
                    } else {
                        None
                    }
                })
                .is_some_and(|source| matches!(source, CodeActionSource::Indicator(..)));
            Some(editor.render_inline_code_actions(icon_size, display_point.row(), active, cx))
        })?;

        let buffer_point = display_point.to_point(&snapshot.display_snapshot);
        let new_display_row = snapshot.display_row_for_inline_code_action(buffer_point)?;

        let start_y = content_origin.y
            + (((new_display_row.as_f64() - scroll_position.y) as f32) * line_height)
            + (line_height / 2.0)
            - (icon_size.square(window, cx) / 2.);
        let start_x = (ScrollPixelOffset::from(content_origin.x) - scroll_pixel_position.x
            + ScrollPixelOffset::from(window.rem_size() * 0.1))
        .into();

        let absolute_offset = point(start_x, start_y);
        button.layout_as_root(AvailableSpace::min_size(), window, cx);
        button.prepaint_as_root(absolute_offset, AvailableSpace::min_size(), window, cx);
        Some(button)
    }

    fn layout_indent_guides(
        &self,
        content_origin: gpui::Point<Pixels>,
        text_origin: gpui::Point<Pixels>,
        visible_buffer_range: Range<MultiBufferRow>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        line_height: Pixels,
        snapshot: &DisplaySnapshot,
        window: &Window,
        cx: &mut App,
    ) -> Option<Vec<IndentGuideLayout>> {
        let indent_guides =
            self.editor.update(cx, |editor, cx| editor.indent_guides(visible_buffer_range, snapshot, cx))?;

        let active_indent_guide_indices = self.editor.update(cx, |editor, cx| {
            editor.find_active_indent_guide_indices(&indent_guides, snapshot, window, cx).unwrap_or_default()
        });

        Some(
            indent_guides
                .into_iter()
                .enumerate()
                .filter_map(|(i, indent_guide)| {
                    let single_indent_width = column_pixels(&self.style, indent_guide.tab_size as usize, window);
                    let total_width = single_indent_width * indent_guide.depth as f32;
                    let start_x =
                        Pixels::from(ScrollOffset::from(content_origin.x + total_width) - scroll_pixel_position.x);
                    (start_x >= text_origin.x).then(|| {
                        let (offset_y, length, display_row_range) = Self::calculate_indent_guide_bounds(
                            indent_guide.start_row..indent_guide.end_row,
                            line_height,
                            snapshot,
                        );

                        let start_y =
                            Pixels::from(ScrollOffset::from(content_origin.y) + offset_y - scroll_pixel_position.y);

                        IndentGuideLayout {
                            origin: point(start_x, start_y),
                            length,
                            display_row_range,
                            depth: indent_guide.depth,
                            active: active_indent_guide_indices.contains(&i),
                            settings: indent_guide.settings,
                        }
                    })
                })
                .collect(),
        )
    }

    fn depth_zero_indent_guide_padding_for_row(indent_guides: &[IndentGuideLayout], row: DisplayRow) -> Pixels {
        indent_guides
            .iter()
            .find(|guide| guide.depth == 0 && guide.display_row_range.contains(&row))
            .map_or(px(0.0), |guide| px(guide.settings.visible_line_width(guide.active) as f32 * 2.0))
    }

    fn layout_wrap_guides(
        &self,
        font_id: FontId,
        font_size: Pixels,
        em_advance: Pixels,
        scroll_position: gpui::Point<f64>,
        content_origin: gpui::Point<Pixels>,
        hitbox: &Hitbox,
        window: &Window,
        cx: &App,
    ) -> SmallVec<[(Pixels, bool); 2]> {
        let guides = self.editor.read(cx).wrap_guides(cx);
        if guides.is_empty() {
            return SmallVec::new();
        }
        let scroll_left = scroll_position.x as f32 * em_advance;
        let content_origin = content_origin.x;
        let horizontal_offset = content_origin - scroll_left;
        // A run of spaces is as wide as one space times its length.
        let space_width = window.text_system().layout_width(font_id, font_size, ' ');

        guides
            .into_iter()
            .filter_map(|(guide, active)| {
                let x = wrap_guide_x(guide, space_width, horizontal_offset, content_origin, hitbox.bounds.right())?;
                Some((x, active))
            })
            .collect()
    }

    fn calculate_indent_guide_bounds(
        row_range: Range<MultiBufferRow>,
        line_height: Pixels,
        snapshot: &DisplaySnapshot,
    ) -> (f64, Pixels, Range<DisplayRow>) {
        let start_point = Point::new(row_range.start.0, 0);
        let end_point = Point::new(row_range.end.0, 0);

        let mut row_range = start_point.to_display_point(snapshot).row()..end_point.to_display_point(snapshot).row();

        let mut prev_line = start_point;
        prev_line.row = prev_line.row.saturating_sub(1);
        let prev_line = prev_line.to_display_point(snapshot).row();

        let mut cons_line = end_point;
        cons_line.row += 1;
        let cons_line = cons_line.to_display_point(snapshot).row();

        let mut offset_y = row_range.start.as_f64() * f64::from(line_height);
        let mut length = (cons_line.0.saturating_sub(row_range.start.0)) as f32 * line_height;

        // If we are at the end of the buffer, ensure that the indent guide extends to the end of the line.
        if row_range.end == cons_line {
            length += line_height;
        }

        // If there is a block (e.g. diagnostic) in between the start of the indent guide and the line above,
        // we want to extend the indent guide to the start of the block.
        let mut block_height = 0;
        let mut block_offset = 0;
        let mut found_excerpt_header = false;
        for (_, block) in snapshot.blocks_in_range(prev_line..row_range.start) {
            if matches!(block, Block::ExcerptBoundary { .. } | Block::BufferHeader { .. }) {
                found_excerpt_header = true;
                break;
            }
            block_offset += block.height();
            block_height += block.height();
        }
        if !found_excerpt_header {
            offset_y -= block_offset as f64 * f64::from(line_height);
            length += block_height as f32 * line_height;
            row_range = DisplayRow(row_range.start.0.saturating_sub(block_offset))..row_range.end;
        }

        // If there is a block (e.g. diagnostic) at the end of an multibuffer excerpt,
        // we want to ensure that the indent guide stops before the excerpt header.
        let mut block_height = 0;
        let mut found_excerpt_header = false;
        for (_, block) in snapshot.blocks_in_range(row_range.end..cons_line) {
            if matches!(block, Block::ExcerptBoundary { .. } | Block::BufferHeader { .. }) {
                found_excerpt_header = true;
            }
            block_height += block.height();
        }
        if found_excerpt_header {
            length -= block_height as f32 * line_height;
        } else {
            row_range = row_range.start..cons_line;
        }

        (offset_y, length, row_range)
    }

    fn layout_crease_trailers(
        &self,
        buffer_rows: impl IntoIterator<Item = RowInfo>,
        snapshot: &EditorSnapshot,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Option<AnyElement>> {
        buffer_rows
            .into_iter()
            .map(|row_info| {
                if row_info.expand_info.is_some() {
                    return None;
                }
                if let Some(row) = row_info.multibuffer_row {
                    snapshot.render_crease_trailer(row, window, cx)
                } else {
                    None
                }
            })
            .collect()
    }

    fn bg_segments_per_row(
        rows: Range<DisplayRow>,
        selections: &[(CursorColors, Vec<SelectionLayout>)],
        highlight_ranges: impl IntoIterator<Item = (Range<DisplayPoint>, Hsla)>,
        base_background: Hsla,
    ) -> Vec<Vec<(Range<usize>, Hsla)>> {
        if rows.start >= rows.end {
            return Vec::new();
        }
        if !base_background.is_opaque() {
            // We don't actually know what color is behind this editor.
            return Vec::new();
        }
        let mut highlight_iter = highlight_ranges.into_iter().peekable();
        let has_selection =
            selections.iter().any(|(_, layouts)| layouts.iter().any(|layout| layout.range.start != layout.range.end));
        if highlight_iter.peek().is_none() && !has_selection {
            return Vec::new();
        }
        let selection_iter = selections.iter().flat_map(|(cursor_colors, layouts)| {
            let color = cursor_colors.selection;
            layouts.iter().filter_map(move |selection_layout| {
                if selection_layout.range.start == selection_layout.range.end {
                    None
                } else {
                    Some((selection_layout.range.clone(), color))
                }
            })
        });
        let mut per_row_map = vec![Vec::new(); rows.len()];
        for (range, color) in highlight_iter.chain(selection_iter) {
            let covered_rows = if range.end.column() == 0 {
                cmp::max(range.start.row(), rows.start)..cmp::min(range.end.row(), rows.end)
            } else {
                cmp::max(range.start.row(), rows.start)..cmp::min(range.end.row().next_row(), rows.end)
            };
            for row in covered_rows.iter_rows() {
                let seg_start = if row == range.start.row() { range.start } else { DisplayPoint::new(row, 0) };
                let seg_end = if row == range.end.row() && range.end.column() != 0 {
                    range.end
                } else {
                    DisplayPoint::new(row, u32::MAX)
                };
                let ix = row.minus(rows.start) as usize;
                per_row_map[ix].push((seg_start..seg_end, color));
            }
        }
        for row_segments in &mut per_row_map {
            if row_segments.is_empty() {
                continue;
            }
            *row_segments = merge_highlight_backgrounds(mem::take(row_segments), base_background);
        }
        per_row_map
            .into_iter()
            .map(|row_segments| {
                row_segments
                    .into_iter()
                    .map(|(range, color)| (range.start.column() as usize..range.end.column() as usize, color))
                    .collect()
            })
            .collect()
    }

    fn layout_lines(
        rows: Range<DisplayRow>,
        snapshot: &EditorSnapshot,
        style: &EditorStyle,
        is_row_soft_wrapped: impl Copy + Fn(usize) -> bool,
        bg_segments_per_row: &[Vec<(Range<usize>, Hsla)>],
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<LineWithInvisibles> {
        if rows.start >= rows.end {
            return Vec::new();
        }

        // Show the placeholder when the editor is empty
        if snapshot.is_empty() {
            let font_size = style.text.font_size.to_pixels(window.rem_size());
            let placeholder_color = theme::theme().colors().text_placeholder;
            let placeholder_text = snapshot.placeholder_text();

            let placeholder_lines =
                placeholder_text.as_ref().map_or(Vec::new(), |text| text.split('\n').collect::<Vec<_>>());

            let placeholder_line_count = placeholder_lines.len();

            placeholder_lines
                .into_iter()
                .skip(rows.start.0 as usize)
                .chain(iter::repeat(""))
                .take(cmp::max(rows.len(), placeholder_line_count))
                .map(move |line| {
                    let run = TextRun {
                        len: line.len(),
                        font: style.text.font(),
                        color: placeholder_color,
                        ..Default::default()
                    };
                    let line = window.text_system().shape_line(SharedString::new(line), font_size, &[run], None);

                    LineWithInvisibles {
                        width: line.width,
                        len: line.len,
                        fragments: smallvec![LineFragment::Text(line)],
                        invisibles: Vec::new(),
                        diagnostic_underline_severity_ranges: Vec::new(),
                        point_diagnostics: Vec::new(),
                        font_size,
                    }
                })
                .collect()
        } else {
            let language_aware = LanguageAwareStyling {
                tree_sitter: true,
                diagnostics: snapshot.display_snapshot.diagnostics_visible(),
            };
            let chunks = snapshot.highlighted_chunks(rows.clone(), language_aware, style);
            LineWithInvisibles::from_chunks(
                chunks,
                style,
                MAX_LINE_LEN,
                rows.len(),
                &snapshot.mode,
                is_row_soft_wrapped,
                bg_segments_per_row,
                window,
                cx,
            )
        }
    }

    fn prepaint_lines(
        &self,
        start_row: DisplayRow,
        line_layouts: &mut [LineWithInvisibles],
        line_height: Pixels,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        content_origin: gpui::Point<Pixels>,
        content_width: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> SmallVec<[AnyElement; 1]> {
        let mut line_elements = SmallVec::new();
        for (ix, line) in line_layouts.iter_mut().enumerate() {
            let row = start_row + DisplayRow(ix as u32);
            line.prepaint(
                line_height,
                scroll_position,
                scroll_pixel_position,
                row,
                content_origin,
                self.style.text.text_align,
                content_width,
                &mut line_elements,
                window,
                cx,
            );
        }
        line_elements
    }

    fn layout_word_diff_highlights(
        display_hunks: &[(DisplayDiffHunk, Option<Hitbox>)],
        row_infos: &[RowInfo],
        start_row: DisplayRow,
        snapshot: &EditorSnapshot,
        highlighted_ranges: &mut Vec<(Range<DisplayPoint>, Hsla)>,
    ) {
        let has_word_diffs = display_hunks.iter().any(|(hunk, _)| {
            matches!(hunk, DisplayDiffHunk::Unfolded { word_diffs, status, .. } if status.is_modified() && !word_diffs.is_empty())
        });
        if !has_word_diffs {
            return;
        }

        let colors = theme::theme().colors();

        let visible_start = DisplayPoint::new(start_row, 0).to_offset(&snapshot.display_snapshot, Bias::Left);
        let visible_end = DisplayPoint::new(DisplayRow(start_row.0 + row_infos.len() as u32), 0)
            .to_offset(&snapshot.display_snapshot, Bias::Right);

        // Gather the word diffs that intersect the viewport. A hunk stores the
        // word diffs for its entire range, so without this filter a large hunk
        // that is only partially scrolled into view would cost work
        // proportional to its whole size every frame.
        let mut visible_word_diffs: Vec<&Range<MultiBufferOffset>> = display_hunks
            .iter()
            .filter_map(|(hunk, _)| match hunk {
                DisplayDiffHunk::Unfolded { word_diffs, status, .. } if status.is_modified() => Some(word_diffs),
                // lint: expect(wildcard_match) only some variants matter here
                _ => None,
            })
            .flatten()
            .filter(|word_diff| word_diff.start < visible_end && word_diff.end > visible_start)
            .collect();

        // The converter walks each display-map layer with a forward-only cursor,
        // so it must receive ranges in non-decreasing order. Word diffs are
        // disjoint, so sorting by offset yields a monotonic sequence.
        visible_word_diffs.sort_unstable_by_key(|word_diff| (word_diff.start, word_diff.end));

        let mut converter = snapshot.display_snapshot.display_point_converter();
        for word_diff in visible_word_diffs {
            for range in converter.map(word_diff.start..word_diff.end) {
                let start_row_offset = range.start.row().0.saturating_sub(start_row.0) as usize;

                let Some(diff_status) = row_infos.get(start_row_offset).and_then(|row_info| row_info.diff_status)
                else {
                    continue;
                };

                let background_color = match diff_status.kind {
                    DiffHunkStatusKind::Added => colors.version_control_word_added,
                    DiffHunkStatusKind::Deleted => colors.version_control_word_deleted,
                    DiffHunkStatusKind::Modified => {
                        log::error!("modified diff status for row info");
                        continue;
                    }
                };

                highlighted_ranges.push((range, background_color));
            }
        }
    }

    fn layout_diff_hunk_controls(
        &self,
        row_range: Range<DisplayRow>,
        row_infos: &[RowInfo],
        text_hitbox: &Hitbox,
        newest_cursor_row: Option<DisplayRow>,
        line_height: Pixels,
        right_margin: Pixels,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        sticky_header_height: Pixels,
        display_hunks: &[(DisplayDiffHunk, Option<Hitbox>)],
        highlighted_rows: &BTreeMap<DisplayRow, LineHighlight>,
        editor: Entity<Editor>,
        window: &mut Window,
        cx: &mut App,
    ) -> (Vec<AnyElement>, Vec<(DisplayRow, Bounds<Pixels>)>) {
        let diff_hunk_renderer = editor.read(cx).diff_hunk_renderer();
        let hovered_diff_hunk_row = editor.read(cx).hovered_diff_hunk_row;
        let sticky_top = text_hitbox.bounds.top() + sticky_header_height;

        let mut controls = vec![];
        let mut control_bounds = vec![];

        let active_rows = [hovered_diff_hunk_row, newest_cursor_row];

        for (hunk, _) in display_hunks {
            if let DisplayDiffHunk::Unfolded {
                display_row_range, multi_buffer_range, status, is_created_file, ..
            } = &hunk
            {
                if display_row_range.start >= row_range.end {
                    // hunk is fully below the viewport
                    continue;
                }
                if display_row_range.end <= row_range.start {
                    // hunk is fully above the viewport
                    continue;
                }
                let row_ix = display_row_range.start.0.saturating_sub(row_range.start.0);
                if row_infos.get(row_ix as usize).and_then(|row_info| row_info.diff_status).is_none() {
                    continue;
                }
                if highlighted_rows.get(&display_row_range.start).and_then(|highlight| highlight.type_id).is_some_and(
                    |type_id| {
                        [
                            TypeId::of::<ConflictsOuter>(),
                            TypeId::of::<ConflictsOursMarker>(),
                            TypeId::of::<ConflictsOurs>(),
                            TypeId::of::<ConflictsTheirs>(),
                            TypeId::of::<ConflictsTheirsMarker>(),
                        ]
                        .contains(&type_id)
                    },
                ) {
                    continue;
                }

                if active_rows.iter().any(|row| row.is_some_and(|row| display_row_range.contains(&row))) {
                    let hunk_start_y: Pixels = (display_row_range.start.as_f64()
                        * ScrollPixelOffset::from(line_height)
                        + ScrollPixelOffset::from(text_hitbox.bounds.top())
                        - scroll_pixel_position.y)
                        .into();

                    let y: Pixels = if hunk_start_y >= sticky_top {
                        hunk_start_y
                    } else {
                        let hunk_end_y: Pixels = hunk_start_y
                            + (display_row_range.len() as f64 * ScrollPixelOffset::from(line_height)).into();
                        let max_y = hunk_end_y - line_height;
                        sticky_top.min(max_y)
                    };

                    let mut element = diff_hunk_renderer.render_hunk_controls(
                        display_row_range.start.0,
                        status,
                        multi_buffer_range.clone(),
                        *is_created_file,
                        line_height,
                        &editor,
                        window,
                        cx,
                    );
                    let size = element.layout_as_root(size(px(100.0), line_height).into(), window, cx);

                    let x = text_hitbox.bounds.right() - right_margin - px(10.) - size.width;

                    if x < text_hitbox.bounds.left() {
                        continue;
                    }

                    let bounds = Bounds::new(gpui::Point::new(x, y), size);
                    control_bounds.push((display_row_range.start, bounds));

                    window.with_absolute_element_offset(gpui::Point::new(x, y), |window| element.prepaint(window, cx));
                    controls.push(element);
                }
            }
        }

        (controls, control_bounds)
    }

    fn paint_background(&self, layout: &EditorLayout, window: &mut Window) {
        window.paint_layer(layout.hitbox.bounds, |window| {
            let scroll_top = layout.position_map.scroll_position.y;
            let gutter_bg = theme::theme().colors().editor_gutter_background;
            window.paint_quad(fill(layout.gutter_hitbox.bounds, gutter_bg));
            window.paint_quad(fill(layout.position_map.text_hitbox.bounds, self.style.background));

            if matches!(layout.mode, EditorMode::Full { .. } | EditorMode::Minimap { .. }) {
                // The parent marks the cursor row on its minimap; a minimap that drew it
                // too would have to be redrawn on every cursor move.
                let show_active_line_background = match layout.mode {
                    EditorMode::Full { show_active_line_background, .. } => show_active_line_background,
                    // lint: expect(wildcard_match) only some variants matter here
                    _ => false,
                };
                let mut active_rows = layout.active_rows.iter().peekable();
                while let Some((start_row, contains_non_empty_selection)) = active_rows.next() {
                    let mut end_row = start_row.0;
                    while active_rows.peek().is_some_and(|(active_row, has_selection)| {
                        active_row.0 == end_row + 1 && has_selection.selection == contains_non_empty_selection.selection
                    }) {
                        active_rows.next();
                        end_row += 1;
                    }

                    if show_active_line_background && !contains_non_empty_selection.selection {
                        let active_line_bg = theme::theme().colors().editor_active_line_background;
                        let bounds = Bounds {
                            origin: point(
                                layout.hitbox.left(),
                                layout.hitbox.origin.y
                                    + Pixels::from(
                                        (start_row.as_f64() - scroll_top)
                                            * ScrollPixelOffset::from(layout.position_map.line_height),
                                    ),
                            ),
                            size: size(
                                layout.hitbox.size.width,
                                layout.position_map.line_height * (end_row - start_row.0 + 1) as f32,
                            ),
                        };
                        window.paint_quad(fill(bounds, active_line_bg));
                    }
                }

                let mut paint_highlight = |highlight_row_start: DisplayRow,
                                           highlight_row_end: DisplayRow,
                                           highlight: LineHighlight,
                                           edges| {
                    let mut origin_x = layout.hitbox.left();
                    let mut width = layout.hitbox.size.width;
                    if !highlight.include_gutter {
                        origin_x += layout.gutter_hitbox.size.width;
                        width -= layout.gutter_hitbox.size.width;
                    }

                    let origin = point(
                        origin_x,
                        layout.hitbox.origin.y
                            + Pixels::from(
                                (highlight_row_start.as_f64() - scroll_top)
                                    * ScrollPixelOffset::from(layout.position_map.line_height),
                            ),
                    );
                    let size = size(
                        width,
                        layout.position_map.line_height
                            * highlight_row_end.next_row().minus(highlight_row_start) as f32,
                    );
                    let mut quad = fill(Bounds { origin, size }, highlight.background);
                    if let Some(border_color) = highlight.border {
                        quad.border_color = border_color;
                        quad.border_widths = edges;
                    }
                    window.paint_quad(quad);
                };

                let mut current_paint: Option<(LineHighlight, Range<DisplayRow>, Edges<Pixels>)> = None;
                for (&new_row, &new_background) in &layout.highlighted_rows {
                    if let &mut Some((current_background, ref mut current_range, mut edges)) = &mut current_paint {
                        let new_range_started =
                            current_background != new_background || current_range.end.next_row() != new_row;
                        if new_range_started {
                            if current_range.end.next_row() == new_row {
                                edges.bottom = px(0.);
                            }
                            paint_highlight(current_range.start, current_range.end, current_background, edges);
                            let edges = Edges {
                                top: if current_range.end.next_row() == new_row { px(0.) } else { px(1.) },
                                bottom: px(1.),
                                ..Default::default()
                            };
                            current_paint = Some((new_background, new_row..new_row, edges));
                            continue;
                        }
                        current_range.end = current_range.end.next_row();
                    } else {
                        let edges = Edges { top: px(1.), bottom: px(1.), ..Default::default() };
                        current_paint = Some((new_background, new_row..new_row, edges));
                    }
                }
                if let Some((color, range, edges)) = current_paint {
                    paint_highlight(range.start, range.end, color, edges);
                }

                for (guide_x, active) in &layout.wrap_guides {
                    let color = if *active {
                        theme::theme().colors().editor_active_wrap_guide
                    } else {
                        theme::theme().colors().editor_wrap_guide
                    };
                    window.paint_quad(fill(
                        window.pixel_snap_bounds(Bounds {
                            origin: point(*guide_x, layout.position_map.text_hitbox.origin.y),
                            size: size(px(1.), layout.position_map.text_hitbox.size.height),
                        }),
                        color,
                    ));
                }
            }
        });
    }

    fn paint_indent_guides(&self, layout: &EditorLayout, window: &mut Window) {
        let Some(indent_guides) = &layout.indent_guides else {
            return;
        };

        let faded_color = |color: Hsla, alpha: f32| {
            let mut faded = color;
            faded.a = alpha;
            faded
        };

        for indent_guide in indent_guides {
            let indent_accent_colors = theme::theme().accents().color_for_index(indent_guide.depth);
            let settings = &indent_guide.settings;

            // TODO fixed for now, expose them through themes later
            const INDENT_AWARE_ALPHA: f32 = 0.2;
            const INDENT_AWARE_ACTIVE_ALPHA: f32 = 0.4;

            let line_color = faded_color(
                indent_accent_colors,
                if indent_guide.active { INDENT_AWARE_ACTIVE_ALPHA } else { INDENT_AWARE_ALPHA },
            );

            let line_width = settings.visible_line_width(indent_guide.active);
            window.paint_quad(fill(
                window.pixel_snap_bounds(Bounds {
                    origin: indent_guide.origin,
                    size: size(px(line_width as f32), indent_guide.length),
                }),
                line_color,
            ));
        }
    }

    fn paint_text(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        window.with_content_mask(Some(ContentMask { bounds: layout.position_map.text_hitbox.bounds }), |window| {
            let editor = self.editor.read(cx);
            if editor.hovered_link_state.as_ref().is_some_and(|hovered_link_state| !hovered_link_state.links.is_empty())
            {
                window.set_cursor_style(CursorStyle::PointingHand, &layout.position_map.text_hitbox);
            } else if !window.modifiers().modified()
                && let Some(hovered_command) = editor.hovered_inlay_hint_command()
                && hovered_command.contains_glyph(
                    &layout.position_map.snapshot,
                    layout.position_map.inlay_hint_glyph_for_position(window.mouse_position()),
                )
            {
                window.set_cursor_style(CursorStyle::PointingHand, &layout.position_map.text_hitbox);
            } else {
                window.set_cursor_style(CursorStyle::IBeam, &layout.position_map.text_hitbox);
            }

            self.paint_lines_background(layout, window, cx);
            let invisible_display_ranges = self.paint_highlights(layout, window);
            self.paint_lines(&invisible_display_ranges, layout, window, cx);
            self.paint_navigation_overlays(layout, window, cx);
            self.paint_cursors(layout, window, cx);
            self.paint_inline_diagnostics(layout, window, cx);
            self.paint_inline_code_actions(layout, window, cx);
            self.paint_diff_hunk_controls(layout, window, cx);
            window.with_element_namespace("crease_trailers", |window| {
                for trailer in layout.crease_trailers.iter_mut().flatten() {
                    trailer.element.paint(window, cx);
                }
            });
        });
    }

    fn paint_highlights(&self, layout: &EditorLayout, window: &mut Window) -> SmallVec<[Range<DisplayPoint>; 32]> {
        window.paint_layer(layout.position_map.text_hitbox.bounds, |window| {
            let mut invisible_display_ranges = SmallVec::<[Range<DisplayPoint>; 32]>::new();
            let line_end_overshoot = 0.15 * layout.position_map.line_height;
            for (range, color) in &layout.highlighted_ranges {
                self.paint_highlighted_range(
                    range.clone(),
                    true,
                    *color,
                    Pixels::ZERO,
                    line_end_overshoot,
                    layout,
                    window,
                );
            }

            let corner_radius = 0.15 * layout.position_map.line_height;

            for (cursor_colors, selections) in &layout.selections {
                for selection in selections {
                    self.paint_highlighted_range(
                        selection.range.clone(),
                        true,
                        cursor_colors.selection,
                        corner_radius,
                        corner_radius * 2.,
                        layout,
                        window,
                    );

                    if !selection.range.is_empty() {
                        invisible_display_ranges.push(selection.range.clone());
                    }
                }
            }
            invisible_display_ranges
        })
    }

    fn paint_lines(
        &self,
        invisible_display_ranges: &[Range<DisplayPoint>],
        layout: &mut EditorLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (ix, line_with_invisibles) in layout.position_map.line_layouts.iter().enumerate() {
            let row = DisplayRow(layout.visible_display_row_range.start.0 + ix as u32);
            line_with_invisibles.draw(layout, row, layout.content_origin, invisible_display_ranges, window, cx);
        }

        for line_element in &mut layout.line_elements {
            line_element.paint(window, cx);
        }
    }

    fn paint_lines_background(&self, layout: &EditorLayout, window: &mut Window, cx: &mut App) {
        for (ix, line_with_invisibles) in layout.position_map.line_layouts.iter().enumerate() {
            let row = DisplayRow(layout.visible_display_row_range.start.0 + ix as u32);
            line_with_invisibles.draw_background(layout, row, layout.content_origin, window, cx);
        }
    }

    fn paint_highlighted_range(
        &self,
        range: Range<DisplayPoint>,
        fill: bool,
        color: Hsla,
        corner_radius: Pixels,
        line_end_overshoot: Pixels,
        layout: &EditorLayout,
        window: &mut Window,
    ) {
        let start_row = layout.visible_display_row_range.start;
        let end_row = layout.visible_display_row_range.end;
        if range.start != range.end {
            let row_range = if range.end.column() == 0 {
                cmp::max(range.start.row(), start_row)..cmp::min(range.end.row(), end_row)
            } else {
                cmp::max(range.start.row(), start_row)..cmp::min(range.end.row().next_row(), end_row)
            };

            let highlighted_range = HighlightedRange {
                color,
                line_height: layout.position_map.line_height,
                corner_radius,
                start_y: layout.content_origin.y
                    + Pixels::from(
                        (row_range.start.as_f64() - layout.position_map.scroll_position.y)
                            * ScrollOffset::from(layout.position_map.line_height),
                    ),
                lines: row_range
                    .iter_rows()
                    .map(|row| {
                        let line_layout = &layout.position_map.line_layouts[row.minus(start_row) as usize];
                        let alignment_offset = line_layout.alignment_offset(layout.text_align, layout.content_width);
                        HighlightedRangeLine {
                            start_x: if row == range.start.row() {
                                layout.content_origin.x
                                    + Pixels::from(
                                        ScrollPixelOffset::from(
                                            line_layout.x_for_index(range.start.column() as usize) + alignment_offset,
                                        ) - layout.position_map.scroll_pixel_position.x,
                                    )
                            } else {
                                layout.content_origin.x + alignment_offset
                                    - Pixels::from(layout.position_map.scroll_pixel_position.x)
                            },
                            end_x: if row == range.end.row() {
                                layout.content_origin.x
                                    + Pixels::from(
                                        ScrollPixelOffset::from(
                                            line_layout.x_for_index(range.end.column() as usize) + alignment_offset,
                                        ) - layout.position_map.scroll_pixel_position.x,
                                    )
                            } else {
                                Pixels::from(
                                    ScrollPixelOffset::from(
                                        layout.content_origin.x
                                            + line_layout.width
                                            + alignment_offset
                                            + line_end_overshoot,
                                    ) - layout.position_map.scroll_pixel_position.x,
                                )
                            },
                        }
                    })
                    .collect(),
            };

            highlighted_range.paint(fill, layout.position_map.text_hitbox.bounds, window);
        }
    }

    fn paint_inline_code_actions(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        if let Some(mut inline_code_actions) = layout.inline_code_actions.take() {
            window.paint_layer(layout.position_map.text_hitbox.bounds, |window| {
                inline_code_actions.paint(window, cx);
            });
        }
    }

    fn paint_diff_hunk_controls(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        for mut diff_hunk_control in layout.diff_hunk_controls.drain(..) {
            diff_hunk_control.paint(window, cx);
        }
    }

    /// Returns the rem size to use when rendering the [`EditorElement`].
    ///
    /// This allows UI elements to scale based on the `buffer_font_size`.
    fn rem_size(&self, cx: &App) -> Option<Pixels> {
        match self.editor.read(cx).mode {
            EditorMode::Full { scale_ui_elements_with_buffer_font_size: true, .. } | EditorMode::Minimap { .. } => {
                let buffer_font_size = self.style.text.font_size;
                match buffer_font_size {
                    AbsoluteLength::Pixels(pixels) => {
                        let rem_size_scale = {
                            // Our default UI font size is 14px on a 16px base scale.
                            // This means the default UI font size is 0.875rems.
                            let default_font_size_scale = 14. / ui::BASE_REM_SIZE_IN_PX;

                            // We then determine the delta between a single rem and the default font
                            // size scale.
                            let default_font_size_delta = 1. - default_font_size_scale;

                            // Finally, we add this delta to 1rem to get the scale factor that
                            // should be used to scale up the UI.
                            1. + default_font_size_delta
                        };

                        Some(pixels * rem_size_scale)
                    }
                    AbsoluteLength::Rems(rems) => Some(rems.to_pixels(ui::BASE_REM_SIZE_IN_PX.into())),
                }
            }
            // We currently use single-line and auto-height editors in UI contexts,
            // so we don't want to scale everything with the buffer font size, as it
            // ends up looking off.
            // lint: expect(wildcard_match) only some variants matter here
            _ => None,
        }
    }

    fn editor_with_selections(&self, cx: &App) -> Option<Entity<Editor>> {
        if let EditorMode::Minimap { parent } = self.editor.read(cx).mode() {
            parent.upgrade()
        } else {
            Some(self.editor.clone())
        }
    }
}

/// Height of a minimap text block, as a share of the minimap line height.
const MINIMAP_BLOCK_HEIGHT: f32 = 0.6;
/// Solid blocks cover more than the glyphs they stand for; fade them to match.
const MINIMAP_BLOCK_OPACITY: f32 = 0.6;

#[derive(Default)]
pub struct EditorRequestLayoutState {
    // We use prepaint depth to limit the number of times prepaint is
    // called recursively. We need this so that we can update stale
    // data for e.g. block heights in block map.
    prepaint_depth: Rc<Cell<usize>>,
}

impl EditorRequestLayoutState {
    // In ideal conditions we only need one more subsequent prepaint call for resize to take effect.
    // i.e. MAX_PREPAINT_DEPTH = 2, but placing near blocks can expose more lines from below, and
    // we end up querying blocks for those lines too in subsequent renders.
    // Setting MAX_PREPAINT_DEPTH = 3, passes all tests. Just to be on the safe side we set it to 5, so
    // that subsequent shrinking does not lead to incorrect block placing.
    const MAX_PREPAINT_DEPTH: usize = 5;

    fn increment_prepaint_depth(&self) -> EditorPrepaintGuard {
        let depth = self.prepaint_depth.get();
        self.prepaint_depth.set(depth + 1);
        EditorPrepaintGuard { prepaint_depth: Rc::clone(&self.prepaint_depth) }
    }

    fn has_remaining_prepaint_depth(&self) -> bool {
        self.prepaint_depth.get() < Self::MAX_PREPAINT_DEPTH
    }
}

struct EditorPrepaintGuard {
    prepaint_depth: Rc<Cell<usize>>,
}

impl Drop for EditorPrepaintGuard {
    fn drop(&mut self) {
        let depth = self.prepaint_depth.get();
        self.prepaint_depth.set(depth.saturating_sub(1));
    }
}

impl Element for EditorElement {
    type RequestLayoutState = EditorRequestLayoutState;
    type PrepaintState = EditorLayout;

    fn id(&self) -> Option<ElementId> {
        Some("editor".into())
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let rem_size = self.rem_size(cx);
        window.with_rem_size(rem_size, |window| {
            self.editor.update(cx, |editor, cx| {
                editor.set_style(self.style.clone(), window, cx);

                let layout_id = match editor.mode {
                    EditorMode::SingleLine => {
                        let rem_size = window.rem_size();
                        let height = self.style.text.line_height_in_pixels(rem_size);
                        let mut style = Style::default();
                        style.size.height = height.into();
                        style.size.width = relative(1.).into();
                        window.request_layout(style, None, cx)
                    }
                    EditorMode::AutoHeight { min_lines, max_lines } => {
                        let editor_handle = cx.entity();
                        window.request_measured_layout(
                            Style::default(),
                            move |known_dimensions, available_space, window, cx| {
                                editor_handle
                                    .update(cx, |editor, cx| {
                                        compute_auto_height_layout(
                                            editor,
                                            min_lines,
                                            max_lines,
                                            known_dimensions,
                                            available_space.width,
                                            window,
                                            cx,
                                        )
                                    })
                                    .unwrap_or_default()
                            },
                        )
                    }
                    EditorMode::Minimap { .. } => {
                        let mut style = Style::default();
                        style.size.width = relative(1.).into();
                        style.size.height = relative(1.).into();
                        window.request_layout(style, None, cx)
                    }
                    EditorMode::Full { sizing_behavior, .. } => {
                        let mut style = Style::default();
                        style.size.width = relative(1.).into();
                        if sizing_behavior == SizingBehavior::SizeByContent {
                            let snapshot = editor.snapshot(window, cx);
                            let line_height = self.style.text.line_height_in_pixels(window.rem_size());
                            let scroll_height = (snapshot.max_point().row().next_row().0 as f32) * line_height;
                            style.size.height = scroll_height.into();
                        } else {
                            style.size.height = relative(1.).into();
                        }
                        window.request_layout(style, None, cx)
                    }
                };

                (layout_id, EditorRequestLayoutState::default())
            })
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one linear layout pass; splitting would mostly thread state through helpers"
    )]
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let _prepaint_depth_guard = request_layout.increment_prepaint_depth();
        let text_style = TextStyleRefinement {
            font_size: Some(self.style.text.font_size),
            line_height: Some(self.style.text.line_height),
            ..Default::default()
        };

        let is_minimap = self.editor.read(cx).mode.is_minimap();

        if !is_minimap {
            let focus_handle = self.editor.focus_handle(cx);
            window.set_view_id(self.editor.entity_id());
            window.set_focus_handle(&focus_handle, cx);
        }

        let rem_size = self.rem_size(cx);
        window.with_rem_size(rem_size, |window| {
            window.with_text_style(Some(text_style), |window| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let (mut snapshot, is_read_only) =
                        self.editor.update(cx, |editor, cx| (editor.snapshot(window, cx), editor.read_only(cx)));
                    let style = &self.style;

                    let rem_size = window.rem_size();
                    let font_id = window.text_system().resolve_font(&style.text.font());
                    let font_size = style.text.font_size.to_pixels(rem_size);
                    let line_height = style.text.line_height_in_pixels(rem_size);
                    let ascent = window.text_system().ascent(font_id, font_size);
                    let descent = window.text_system().descent(font_id, font_size).abs();
                    let point_diagnostic_underline_offset = underline_y_offset(line_height, ascent, descent);
                    let em_width = window.text_system().em_width(font_id, font_size);
                    let em_advance = window.text_system().em_advance(font_id, font_size);
                    let em_layout_width = window.text_system().em_layout_width(font_id, font_size);
                    let glyph_grid_cell = size(em_advance, line_height);

                    let gutter_dimensions = snapshot.gutter_dimensions(font_id, font_size, window, cx);
                    let text_width = bounds.size.width - gutter_dimensions.width;

                    let minimap_width =
                        self.get_minimap_width(text_width, em_width, font_size, rem_size, cx).unwrap_or_default();

                    let right_margin = minimap_width;

                    let extended_right = 2 * em_width + right_margin;
                    let editor_width = text_width - gutter_dimensions.margin - extended_right;
                    let editor_margins =
                        EditorMargins { gutter: gutter_dimensions, right: right_margin, extended_right };

                    snapshot = self.editor.update(cx, |editor, cx| {
                        editor.last_bounds = Some(bounds);
                        editor.gutter_dimensions = gutter_dimensions;
                        editor.set_visible_line_count((bounds.size.height / line_height) as f64, window, cx);
                        editor.set_visible_column_count(f64::from(editor_width / em_advance));

                        if matches!(editor.mode, EditorMode::AutoHeight { .. } | EditorMode::Minimap { .. }) {
                            snapshot
                        } else {
                            let wrap_width =
                                Some(calculate_wrap_width(editor.soft_wrap_mode(cx), editor_width, em_layout_width));

                            if editor.set_wrap_width(wrap_width, cx) { editor.snapshot(window, cx) } else { snapshot }
                        }
                    });

                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    let gutter_hitbox =
                        window.insert_hitbox(gutter_bounds(bounds, gutter_dimensions), HitboxBehavior::Normal);
                    let text_hitbox = window.insert_hitbox(
                        Bounds { origin: gutter_hitbox.top_right(), size: size(text_width, bounds.size.height) },
                        HitboxBehavior::Normal,
                    );

                    // Offset the content_bounds from the text_bounds by the gutter margin (which
                    // is roughly half a character wide) to make hit testing work more like how we want.
                    let content_offset = point(editor_margins.gutter.margin, Pixels::ZERO);
                    let content_origin = text_hitbox.origin + content_offset;

                    let height_in_lines = f64::from(bounds.size.height / line_height);
                    let max_row = snapshot.max_point().row().as_f64();

                    // Calculate how much of the editor is clipped by parent containers (e.g., List).
                    // This allows us to only render lines that are actually visible, which is
                    // critical for performance when large content-sized editors are inside Lists.
                    let visible_bounds = window.content_mask().bounds;
                    let visible_top = bounds.top().max(visible_bounds.top());
                    let visible_bottom = bounds.bottom().min(visible_bounds.bottom());
                    let clipped_top = (visible_top - bounds.top()).max(px(0.));
                    let visible_height = (visible_bottom - visible_top).max(px(0.));
                    let clipped_top_in_lines = f64::from(clipped_top / line_height);
                    let visible_height_in_lines = f64::from(visible_height / line_height);

                    // The max scroll position for the top of the window
                    let scroll_beyond_last_line = self.editor.read(cx).scroll_beyond_last_line();
                    let max_scroll_top =
                        if scroll_beyond_last_line { max_row } else { (max_row - height_in_lines + 1.).max(0.) };

                    let (autoscroll_request, autoscroll_containing_element, needs_horizontal_autoscroll) =
                        self.editor.update(cx, |editor, cx| {
                            let autoscroll_request = editor.scroll_manager.take_autoscroll_request();

                            let autoscroll_containing_element =
                                autoscroll_request.is_some() || editor.has_pending_selection();

                            let (needs_horizontal_autoscroll, was_scrolled) = editor.autoscroll_vertically(
                                bounds,
                                line_height,
                                max_scroll_top,
                                autoscroll_request,
                                window,
                                cx,
                            );
                            if was_scrolled.0 {
                                snapshot = editor.snapshot(window, cx);
                            }
                            (autoscroll_request, autoscroll_containing_element, needs_horizontal_autoscroll)
                        });

                    let mut scroll_position = snapshot.scroll_position();
                    let smooth_scroll_offset = self.editor.read(cx).scroll_manager.smooth_scroll_offset(cx);
                    if smooth_scroll_offset != 0. {
                        scroll_position.y = (scroll_position.y - smooth_scroll_offset).clamp(0., max_scroll_top);
                        window.request_animation_frame();
                    }
                    if !line_height.is_zero() {
                        scroll_position.y =
                            window.pixel_snap_f64(scroll_position.y * f64::from(line_height)) / f64::from(line_height);
                    }
                    // The scroll position is a fractional point, the whole number of which represents
                    // the top of the window in terms of display rows.
                    // We add clipped_top_in_lines to skip rows that are clipped by parent containers,
                    // but we don't modify scroll_position itself since the parent handles positioning.
                    let max_row = snapshot.max_point().row();
                    let start_row =
                        cmp::min(DisplayRow((scroll_position.y + clipped_top_in_lines).floor() as u32), max_row);
                    let end_row = cmp::min(
                        (scroll_position.y + clipped_top_in_lines + visible_height_in_lines).ceil() as u32,
                        max_row.next_row().0,
                    );
                    let end_row = DisplayRow(end_row);

                    let row_infos = snapshot // note we only get the visual range
                        .row_infos(start_row)
                        .take((start_row..end_row).len())
                        .collect::<Vec<RowInfo>>();
                    parse_drawn_buffers(&self.editor, &row_infos, cx);
                    let is_row_soft_wrapped =
                        |row: usize| row_infos.get(row).is_none_or(|info| info.buffer_row.is_none());

                    let start_anchor = if start_row == DisplayRow::default() {
                        Anchor::Min
                    } else {
                        snapshot
                            .buffer_snapshot()
                            .anchor_before(DisplayPoint::new(start_row, 0).to_offset(&snapshot, Bias::Left))
                    };
                    let end_anchor = if end_row > max_row {
                        Anchor::Max
                    } else {
                        snapshot
                            .buffer_snapshot()
                            .anchor_before(DisplayPoint::new(end_row, 0).to_offset(&snapshot, Bias::Right))
                    };

                    let mut highlighted_rows = self.editor.read(cx).highlighted_display_rows_in_range(
                        start_anchor..end_anchor,
                        start_row..end_row,
                        &snapshot.display_snapshot,
                        cx,
                    );

                    // A minimap shares its parent's display map, so its snapshot and anchors apply to both.
                    let mut highlighted_ranges = self.editor_with_selections(cx).map_or_default(|editor| {
                        editor.read(cx).background_highlights_in_range(
                            start_anchor..end_anchor,
                            &snapshot.display_snapshot,
                            theme::theme(),
                        )
                    });

                    struct DiffHunkHighlightColors {
                        filled_background: Hsla,
                        hollow_background: Hsla,
                        hollow_border: Hsla,
                    }

                    let colors = theme::theme().colors();
                    let added_diff_hunk_colors = DiffHunkHighlightColors {
                        filled_background: colors.editor_diff_hunk_added_background,
                        hollow_background: colors.editor_diff_hunk_added_hollow_background,
                        hollow_border: colors.editor_diff_hunk_added_hollow_border,
                    };
                    let deleted_diff_hunk_colors = DiffHunkHighlightColors {
                        filled_background: colors.editor_diff_hunk_deleted_background,
                        hollow_background: colors.editor_diff_hunk_deleted_hollow_background,
                        hollow_border: colors.editor_diff_hunk_deleted_hollow_border,
                    };

                    for (ix, row_info) in row_infos.iter().enumerate() {
                        let Some(diff_status) = row_info.diff_status else {
                            continue;
                        };

                        let diff_hunk_colors = match diff_status.kind {
                            DiffHunkStatusKind::Added => &added_diff_hunk_colors,
                            DiffHunkStatusKind::Deleted => &deleted_diff_hunk_colors,
                            DiffHunkStatusKind::Modified => {
                                log::error!("modified diff status for row info");
                                continue;
                            }
                        };

                        let hollow_highlight = LineHighlight {
                            background: diff_hunk_colors.hollow_background.into(),
                            border: Some(diff_hunk_colors.hollow_border),
                            include_gutter: true,
                            type_id: None,
                        };

                        let filled_highlight = LineHighlight {
                            background: solid_background(diff_hunk_colors.filled_background),
                            border: None,
                            include_gutter: true,
                            type_id: None,
                        };

                        let background =
                            if self.diff_hunk_hollow(diff_status, cx) { hollow_highlight } else { filled_highlight };

                        let base_display_point = DisplayPoint::new(start_row + DisplayRow(ix as u32), 0);

                        highlighted_rows.entry(base_display_point.row()).or_insert(background);
                    }

                    let highlighted_gutter_ranges = self.editor.read(cx).gutter_highlights_in_range(
                        start_anchor..end_anchor,
                        &snapshot.display_snapshot,
                        cx,
                    );

                    let (local_selections, selected_buffer_ids, latest_selection_anchors): (
                        Vec<Selection<Point>>,
                        Vec<BufferId>,
                        FxHashMap<BufferId, Anchor>,
                    ) = self.editor_with_selections(cx).map_or_else(
                        || (Vec::new(), Vec::new(), FxHashMap::default()),
                        |editor| {
                            editor.update(cx, |editor, cx| {
                                let is_singleton = editor.buffer_kind(cx) == ItemBufferKind::Singleton;
                                // Both are only read by buffer headers.
                                let show_headers = snapshot.buffer_snapshot().show_headers();

                                // Singleton buffers only need the newest selection anchor here.
                                let selected_buffer_ids = if is_singleton || !show_headers {
                                    Vec::new()
                                } else {
                                    let all_selections = editor.selections.all::<Point>(&snapshot.display_snapshot);
                                    let mut selected_buffer_ids = Vec::with_capacity(all_selections.len());

                                    for selection in all_selections {
                                        for buffer_id in
                                            snapshot.buffer_snapshot().buffer_ids_for_range(selection.range())
                                        {
                                            if selected_buffer_ids.last() != Some(&buffer_id) {
                                                selected_buffer_ids.push(buffer_id);
                                            }
                                        }
                                    }

                                    selected_buffer_ids
                                };

                                let mut selections = editor
                                    .selections
                                    .disjoint_in_row_range(start_anchor..end_anchor, &snapshot.display_snapshot);
                                selections.extend(editor.selections.pending(&snapshot.display_snapshot));

                                let latest_selection_anchors: FxHashMap<BufferId, Anchor> = if !show_headers {
                                    FxHashMap::default()
                                } else if is_singleton {
                                    let head = editor.selections.newest_anchor().head();
                                    snapshot
                                        .buffer_snapshot()
                                        .anchor_to_buffer_anchor(head)
                                        .map(|(text_anchor, _)| (text_anchor.buffer_id, head))
                                        .into_iter()
                                        .collect()
                                } else {
                                    let all_anchor_selections =
                                        editor.selections.all_anchors(&snapshot.display_snapshot);
                                    let mut anchors_by_buffer: FxHashMap<BufferId, (usize, Anchor)> =
                                        FxHashMap::default();
                                    for selection in all_anchor_selections.iter() {
                                        let head = selection.head();
                                        if let Some((text_anchor, _)) =
                                            snapshot.buffer_snapshot().anchor_to_buffer_anchor(head)
                                        {
                                            anchors_by_buffer
                                                .entry(text_anchor.buffer_id)
                                                .and_modify(|(latest_id, latest_anchor)| {
                                                    if selection.id > *latest_id {
                                                        *latest_id = selection.id;
                                                        *latest_anchor = head;
                                                    }
                                                })
                                                .or_insert((selection.id, head));
                                        }
                                    }
                                    anchors_by_buffer
                                        .into_iter()
                                        .map(|(buffer_id, (_, anchor))| (buffer_id, anchor))
                                        .collect()
                                };

                                (selections, selected_buffer_ids, latest_selection_anchors)
                            })
                        },
                    );

                    let (selections, active_rows, newest_selection_head) =
                        self.layout_selections(&local_selections, &snapshot, start_row, end_row, window, cx);

                    let run_indicator_rows =
                        self.editor.read(cx).active_run_indicators(start_row..end_row, &snapshot, cx);

                    let gutter = Gutter {
                        line_height,
                        range: start_row..end_row,
                        scroll_position,
                        dimensions: &gutter_dimensions,
                        hitbox: &gutter_hitbox,
                        snapshot: &snapshot,
                        row_infos: &row_infos,
                    };

                    let line_numbers = self.layout_line_numbers(&gutter, &active_rows, window);

                    let mut expand_toggles = window.with_element_namespace("expand_toggles", |window| {
                        self.layout_expand_toggles(
                            &gutter_hitbox,
                            gutter_dimensions,
                            em_width,
                            line_height,
                            scroll_position,
                            start_row,
                            &row_infos,
                            &snapshot,
                            window,
                            cx,
                        )
                    });

                    let mut crease_toggles = window.with_element_namespace("crease_toggles", |window| {
                        self.layout_crease_toggles(start_row..end_row, &row_infos, &active_rows, &snapshot, window, cx)
                    });
                    // A minimap has no gutter and no room for trailers or word diffs.
                    let crease_trailers = if is_minimap {
                        Vec::new()
                    } else {
                        window.with_element_namespace("crease_trailers", |window| {
                            self.layout_crease_trailers(row_infos.iter().copied(), &snapshot, window, cx)
                        })
                    };

                    let display_hunks = if is_minimap {
                        Vec::new()
                    } else {
                        self.layout_gutter_diff_hunks(
                            line_height,
                            &gutter_hitbox,
                            start_row..end_row,
                            &snapshot,
                            scroll_position,
                            window,
                            cx,
                        )
                    };

                    Self::layout_word_diff_highlights(
                        &display_hunks,
                        &row_infos,
                        start_row,
                        &snapshot,
                        &mut highlighted_ranges,
                    );

                    let bg_segments_per_row = Self::bg_segments_per_row(
                        start_row..end_row,
                        &selections,
                        highlighted_ranges.iter().cloned(),
                        self.style.background,
                    );

                    let mut line_layouts = if is_minimap {
                        self.layout_minimap_lines(start_row..end_row, &snapshot, is_row_soft_wrapped, window, cx)
                    } else {
                        Self::layout_lines(
                            start_row..end_row,
                            &snapshot,
                            &self.style,
                            is_row_soft_wrapped,
                            &bg_segments_per_row,
                            window,
                            cx,
                        )
                    };
                    let new_renderer_widths = (!is_minimap).then(|| {
                        line_layouts.iter().flat_map(|layout| &layout.fragments).filter_map(|fragment| {
                            if let LineFragment::Element { id, size, .. } = fragment {
                                Some((*id, size.width))
                            } else {
                                None
                            }
                        })
                    });
                    let renderer_widths_changed = request_layout.has_remaining_prepaint_depth()
                        && new_renderer_widths.is_some_and(|new_renderer_widths| {
                            self.editor.update(cx, |editor, cx| editor.update_renderer_widths(new_renderer_widths, cx))
                        });
                    if renderer_widths_changed {
                        return self.prepaint(None, bounds, request_layout, window, cx);
                    }

                    // A minimap never scrolls sideways, so it has no use for the width.
                    let longest_row = snapshot.longest_row();
                    let longest_line_width = if is_minimap {
                        Pixels::ZERO
                    } else if let Some(line) = visible_line(&line_layouts, start_row, longest_row) {
                        line.width
                    } else {
                        layout_line(longest_row, &snapshot, style, is_row_soft_wrapped, window, cx).width
                    };

                    let scrollbar_layout_information = ScrollbarLayoutInformation::new(
                        text_hitbox.bounds,
                        glyph_grid_cell,
                        size(longest_line_width, Pixels::from(max_row.as_f64() * f64::from(line_height))),
                        scroll_beyond_last_line,
                    );

                    let mut scroll_width = scrollbar_layout_information.scroll_range.width;

                    let sticky_header_excerpt = if snapshot.buffer_snapshot().show_headers() {
                        snapshot.sticky_header_excerpt(scroll_position.y)
                    } else {
                        None
                    };
                    let sticky_header_excerpt_id = sticky_header_excerpt.as_ref().map(|top| top.excerpt.buffer_id());

                    let buffer = snapshot.buffer_snapshot();
                    let start_buffer_row = MultiBufferRow(start_anchor.to_point(buffer).row);
                    let end_buffer_row = MultiBufferRow(end_anchor.to_point(buffer).row);

                    let preliminary_scroll_pixel_position = point(
                        scroll_position.x * f64::from(em_layout_width),
                        scroll_position.y * f64::from(line_height),
                    );
                    let indent_guides = self.layout_indent_guides(
                        content_origin,
                        text_hitbox.origin,
                        start_buffer_row..end_buffer_row,
                        preliminary_scroll_pixel_position,
                        line_height,
                        &snapshot,
                        window,
                        cx,
                    );
                    let indent_guides_for_spacers = indent_guides.clone();

                    let blocks = if is_minimap {
                        RenderBlocksOutput::default()
                    } else {
                        {
                            window.with_element_namespace("blocks", |window| {
                                self.render_blocks(
                                    start_row..end_row,
                                    &snapshot,
                                    &hitbox,
                                    editor_width,
                                    &mut scroll_width,
                                    &editor_margins,
                                    em_width,
                                    gutter_dimensions.full_width(),
                                    line_height,
                                    &line_layouts,
                                    &selected_buffer_ids,
                                    &latest_selection_anchors,
                                    is_row_soft_wrapped,
                                    sticky_header_excerpt_id,
                                    indent_guides_for_spacers.as_deref(),
                                    window,
                                    cx,
                                )
                            })
                        }
                    };
                    let RenderBlocksOutput {
                        non_spacer_blocks: mut blocks,
                        mut spacer_blocks,
                        row_block_types,
                        resized_blocks,
                    } = blocks;
                    if let Some(resized_blocks) = resized_blocks {
                        if request_layout.has_remaining_prepaint_depth() {
                            self.editor.update(cx, |editor, cx| {
                                editor.resize_blocks(
                                    resized_blocks,
                                    autoscroll_request.map(|(autoscroll, _)| autoscroll),
                                    cx,
                                );
                            });
                            return self.prepaint(None, bounds, request_layout, window, cx);
                        }
                        log::error!(
                            "dropping block resize because prepaint depth \
                             limit was reached"
                        );
                    }

                    let sticky_buffer_header = if self.should_show_buffer_headers() {
                        sticky_header_excerpt.map(|sticky_header_excerpt| {
                            window.with_element_namespace("blocks", |window| {
                                self.layout_sticky_buffer_header(
                                    sticky_header_excerpt,
                                    scroll_position,
                                    line_height,
                                    right_margin,
                                    &snapshot,
                                    &hitbox,
                                    &selected_buffer_ids,
                                    &blocks,
                                    &latest_selection_anchors,
                                    window,
                                    cx,
                                )
                            })
                        })
                    } else {
                        None
                    };

                    let scroll_max: gpui::Point<ScrollPixelOffset> = point(
                        ScrollPixelOffset::from(((scroll_width - editor_width) / em_layout_width).max(0.0)),
                        max_scroll_top,
                    );

                    self.editor.update(cx, |editor, cx| {
                        if editor.scroll_manager.clamp_scroll_left(scroll_max.x, cx) {
                            scroll_position.x = scroll_max.x.min(scroll_position.x);
                        }

                        if needs_horizontal_autoscroll.0
                            && let Some(new_scroll_position) = editor.autoscroll_horizontally(
                                start_row,
                                editor_width,
                                scroll_width,
                                em_advance,
                                &line_layouts,
                                autoscroll_request,
                                window,
                                cx,
                            )
                        {
                            scroll_position.x = new_scroll_position.x;
                        }
                    });

                    if !em_layout_width.is_zero() {
                        scroll_position.x = window.pixel_snap_f64(scroll_position.x * f64::from(em_layout_width))
                            / f64::from(em_layout_width);
                    }

                    let scroll_pixel_position = point(
                        scroll_position.x * f64::from(em_layout_width),
                        scroll_position.y * f64::from(line_height),
                    );
                    let indent_guides = if scroll_pixel_position == preliminary_scroll_pixel_position {
                        indent_guides
                    } else {
                        self.layout_indent_guides(
                            content_origin,
                            text_hitbox.origin,
                            start_buffer_row..end_buffer_row,
                            scroll_pixel_position,
                            line_height,
                            &snapshot,
                            window,
                            cx,
                        )
                    };

                    let crease_trailers = window.with_element_namespace("crease_trailers", |window| {
                        self.prepaint_crease_trailers(
                            crease_trailers,
                            &line_layouts,
                            line_height,
                            content_origin,
                            scroll_pixel_position,
                            scroll_position,
                            start_row,
                            em_width,
                            window,
                            cx,
                        )
                    });

                    let inline_diagnostics = self.layout_inline_diagnostics(
                        &line_layouts,
                        &crease_trailers,
                        &row_block_types,
                        content_origin,
                        scroll_position,
                        scroll_pixel_position,
                        start_row,
                        end_row,
                        line_height,
                        em_width,
                        style,
                        window,
                        cx,
                    );

                    let mut inline_code_actions = None;
                    if let Some(newest_selection_head) = newest_selection_head {
                        let display_row = newest_selection_head.row();
                        if (start_row..end_row).contains(&display_row) && !row_block_types.contains_key(&display_row) {
                            inline_code_actions = self.layout_inline_code_actions(
                                newest_selection_head,
                                content_origin,
                                scroll_position,
                                scroll_pixel_position,
                                line_height,
                                &snapshot,
                                window,
                                cx,
                            );
                        }
                    }

                    let blamed_display_rows = self.layout_blame_entries(
                        &row_infos,
                        em_width,
                        scroll_position,
                        start_row,
                        line_height,
                        &gutter_hitbox,
                        gutter_dimensions.git_blame_entries_width,
                        window,
                        cx,
                    );

                    let line_elements = self.prepaint_lines(
                        start_row,
                        &mut line_layouts,
                        line_height,
                        scroll_position,
                        scroll_pixel_position,
                        content_origin,
                        text_hitbox.size.width,
                        window,
                        cx,
                    );

                    window.with_element_namespace("blocks", |window| {
                        self.layout_blocks(
                            &mut blocks,
                            &hitbox,
                            &gutter_hitbox,
                            line_height,
                            scroll_position,
                            scroll_pixel_position,
                            &editor_margins,
                            window,
                            cx,
                        );
                        self.layout_blocks(
                            &mut spacer_blocks,
                            &hitbox,
                            &gutter_hitbox,
                            line_height,
                            scroll_position,
                            scroll_pixel_position,
                            &editor_margins,
                            window,
                            cx,
                        );
                    });

                    let visible_row_range = start_row..end_row;
                    let (cursors, non_visible_cursors) = self.collect_cursors(&snapshot, &visible_row_range, cx);

                    let visible_cursors = self.layout_visible_cursors(
                        &snapshot,
                        &selections,
                        &row_block_types,
                        start_row..end_row,
                        &line_layouts,
                        &text_hitbox,
                        content_origin,
                        scroll_position,
                        scroll_pixel_position,
                        line_height,
                        em_width,
                        em_advance,
                        autoscroll_containing_element,
                        window,
                        cx,
                    );
                    let navigation_overlay_paint_commands = self.layout_navigation_overlays(
                        &snapshot,
                        start_row..end_row,
                        &line_layouts,
                        &text_hitbox,
                        content_origin,
                        scroll_position,
                        scroll_pixel_position,
                        line_height,
                        window,
                        cx,
                    );

                    let frozen_scroll_state = if self.editor.read(cx).search_results_hold.is_some() {
                        let is_rewrapping = self.editor.read(cx).display_map.read(cx).is_rewrapping(cx);
                        self.editor.update(cx, |editor, _| {
                            editor.frozen_scroll_range(
                                is_rewrapping,
                                scrollbar_layout_information.scroll_range,
                                editor_width,
                                scrollbar_layout_information.editor_bounds.size,
                            )
                        })
                    } else {
                        None
                    };
                    let effective_scrollbar_layout_information =
                        frozen_scroll_state.map_or(scrollbar_layout_information, |settled| {
                            ScrollbarLayoutInformation { scroll_range: settled.range, ..scrollbar_layout_information }
                        });
                    let effective_editor_width =
                        frozen_scroll_state.map_or(editor_width, |settled| settled.editor_width);

                    let scrollbars_layout = self.layout_scrollbars(
                        &snapshot,
                        &effective_scrollbar_layout_information,
                        content_offset,
                        scroll_position,
                        non_visible_cursors,
                        right_margin,
                        effective_editor_width,
                        window,
                        cx,
                    );

                    let context_menu_layout = if let Some(newest_selection_head) = newest_selection_head {
                        let newest_selection_point = newest_selection_head.to_point(&snapshot.display_snapshot);
                        if (start_row..end_row).contains(&newest_selection_head.row()) {
                            self.layout_cursor_popovers(
                                line_height,
                                &text_hitbox,
                                content_origin,
                                right_margin,
                                start_row,
                                scroll_pixel_position,
                                &line_layouts,
                                newest_selection_head,
                                newest_selection_point,
                                style,
                                window,
                                cx,
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    self.layout_gutter_menu(
                        line_height,
                        &text_hitbox,
                        content_origin,
                        right_margin,
                        scroll_pixel_position,
                        gutter_dimensions.width - gutter_dimensions.left_padding,
                        window,
                        cx,
                    );

                    let test_indicators = self.layout_run_indicators(&gutter, &run_indicator_rows, window, cx);

                    let editor = self.editor.read(cx);
                    let copied_row = editor.gutter_copied_row.0;
                    let hovered_row =
                        editor.gutter_hover_button.0.filter(|button| button.is_active).map(|button| button.display_row);
                    // Keep the "copied" check visible after the mouse leaves its row.
                    let gutter_hover_buttons = copied_row
                        .into_iter()
                        .chain(hovered_row.filter(|row| Some(*row) != copied_row))
                        .filter(|row| !run_indicator_rows.contains(row))
                        .filter_map(|row| self.layout_gutter_hover_button(&gutter, row, window, cx))
                        .collect::<Vec<_>>();

                    if !cx.has_active_drag() {
                        self.layout_hover_popovers(
                            &snapshot,
                            &hitbox,
                            start_row..end_row,
                            content_origin,
                            scroll_pixel_position,
                            &line_layouts,
                            line_height,
                            em_width,
                            context_menu_layout,
                            window,
                            cx,
                        );
                    }

                    let mouse_context_menu =
                        self.layout_mouse_context_menu(&snapshot, start_row..end_row, content_origin, window, cx);

                    window.with_element_namespace("crease_toggles", |window| {
                        self.prepaint_crease_toggles(
                            &mut crease_toggles,
                            line_height,
                            &gutter_dimensions,
                            scroll_position,
                            start_row,
                            &gutter_hitbox,
                            window,
                            cx,
                        );
                    });

                    window.with_element_namespace("expand_toggles", |window| {
                        self.prepaint_expand_toggles(&mut expand_toggles, window, cx);
                    });

                    let wrap_guides = self.layout_wrap_guides(
                        font_id,
                        font_size,
                        em_advance,
                        scroll_position,
                        content_origin,
                        &hitbox,
                        window,
                        cx,
                    );

                    let minimap = window.with_element_namespace("minimap", |window| {
                        self.layout_minimap(
                            &snapshot,
                            minimap_width,
                            scroll_position,
                            &effective_scrollbar_layout_information,
                            window,
                            cx,
                        )
                    });

                    let invisible_symbol_font_size = font_size / 2.;
                    let tab_char = SharedString::new_static(WHITESPACE_TAB);
                    let tab_len = tab_char.len();
                    let tab_invisible = window.text_system().shape_line(
                        tab_char,
                        invisible_symbol_font_size,
                        &[TextRun {
                            len: tab_len,
                            font: self.style.text.font(),
                            color: theme::theme().colors().editor_invisible,
                            ..Default::default()
                        }],
                        None,
                    );

                    let space_char = SharedString::new_static(WHITESPACE_SPACE);
                    let space_len = space_char.len();
                    let space_invisible = window.text_system().shape_line(
                        space_char,
                        invisible_symbol_font_size,
                        &[TextRun {
                            len: space_len,
                            font: self.style.text.font(),
                            color: theme::theme().colors().editor_invisible,
                            ..Default::default()
                        }],
                        None,
                    );

                    let mode = snapshot.mode.clone();

                    let has_sticky_buffer_header = sticky_buffer_header.is_some() || sticky_header_excerpt_id.is_some();
                    let sticky_header_height = if has_sticky_buffer_header {
                        let full_height = FILE_HEADER_HEIGHT as f32 * line_height;
                        let display_row = blocks
                            .iter()
                            .filter(|block| block.is_buffer_header)
                            .find_map(|block| block.row.filter(|row| row.0 > scroll_position.y as u32));
                        let offset = match display_row {
                            Some(display_row) => {
                                let max_row = display_row.0.saturating_sub(FILE_HEADER_HEIGHT);
                                let offset = (scroll_position.y - max_row as f64).max(0.0);
                                let slide_up = Pixels::from(offset * ScrollPixelOffset::from(line_height));

                                (full_height - slide_up).max(Pixels::ZERO)
                            }
                            None => full_height,
                        };
                        let card_height = header::buffer_header_card_height(line_height, window.rem_size());
                        let header_bottom_padding = (full_height - card_height) / 2.;
                        offset - header_bottom_padding
                    } else {
                        Pixels::ZERO
                    };

                    let (diff_hunk_controls, diff_hunk_control_bounds) = if display_hunks.is_empty()
                        || (is_read_only && self.editor.read(cx).diff_hunk_renderer.is_none())
                    {
                        (vec![], vec![])
                    } else {
                        // Already laid out when the newest selection is on screen.
                        let current_selection_head = newest_selection_head.map(|head| head.row()).or_else(|| {
                            let editor = self.editor.read(cx);
                            (editor.selections.count() != 0).then(|| {
                                SelectionLayout::new(
                                    editor.selections.newest::<Point>(&snapshot),
                                    editor.selections.line_mode(),
                                    editor.cursor_offset_on_selection,
                                    editor.cursor_shape,
                                    &snapshot,
                                    true,
                                )
                                .head
                                .row()
                            })
                        });
                        self.layout_diff_hunk_controls(
                            start_row..end_row,
                            &row_infos,
                            &text_hitbox,
                            current_selection_head,
                            line_height,
                            right_margin,
                            scroll_pixel_position,
                            sticky_header_height,
                            &display_hunks,
                            &highlighted_rows,
                            self.editor.clone(),
                            window,
                            cx,
                        )
                    };

                    self.populate_point_diagnostics(&snapshot, start_row..end_row, &mut line_layouts);

                    let position_map = Rc::new(PositionMap {
                        size: bounds.size,
                        visible_row_range,
                        scroll_position,
                        scroll_pixel_position,
                        scroll_max,
                        line_layouts,
                        line_height,
                        em_advance,
                        em_layout_width,
                        snapshot,
                        text_align: self.style.text.text_align,
                        content_width: text_hitbox.size.width,
                        content_origin,
                        gutter_hitbox: gutter_hitbox.clone(),
                        text_hitbox: text_hitbox.clone(),
                        display_hunks: display_hunks.clone(),
                        diff_hunk_control_bounds,
                    });

                    let visible_horizontal_scrollbar = scrollbars_layout.as_ref().is_some_and(|scrollbars_layout| {
                        scrollbars_layout.visible && scrollbars_layout.horizontal.is_some()
                    });

                    self.editor.update(cx, |editor, _| {
                        editor.last_position_map = Some(Rc::clone(&position_map));
                        editor.last_right_margin = right_margin;
                        editor.last_horizontal_scrollbar_visible = visible_horizontal_scrollbar;
                    });

                    EditorLayout {
                        mode,
                        position_map,
                        visible_display_row_range: start_row..end_row,
                        wrap_guides,
                        indent_guides,
                        hitbox,
                        gutter_hitbox,
                        display_hunks,
                        content_origin,
                        scrollbars_layout,
                        minimap,
                        active_rows,
                        highlighted_rows,
                        highlighted_ranges,
                        highlighted_gutter_ranges,
                        line_elements,
                        line_numbers,
                        blamed_display_rows,
                        inline_diagnostics,
                        point_diagnostic_underline_offset,
                        inline_code_actions,
                        blocks,
                        spacer_blocks,
                        cursors,
                        visible_cursors,
                        navigation_overlay_paint_commands,
                        selections,
                        diff_hunk_controls,
                        mouse_context_menu,
                        test_indicators,
                        gutter_hover_buttons,
                        crease_toggles,
                        crease_trailers,
                        tab_invisible,
                        space_invisible,
                        sticky_buffer_header,
                        expand_toggles,
                        text_align: self.style.text.text_align,
                        content_width: text_hitbox.size.width,
                    }
                })
            })
        })
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !layout.mode.is_minimap() {
            let focus_handle = self.editor.focus_handle(cx);
            if cx.ui_snapshots_enabled()
                && let Some(global_id) = global_id
            {
                window.record_ui_element(global_id, bounds, true, Some(&focus_handle));
            }
            let key_context = self.editor.update(cx, |editor, cx| editor.key_context(window, cx));

            window.set_key_context(key_context);
            window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.editor.clone()), cx);
            self.register_actions(window, cx);
            self.register_key_listeners(window, cx, layout);
        }

        let text_style = TextStyleRefinement {
            font_size: Some(self.style.text.font_size),
            line_height: Some(self.style.text.line_height),
            ..Default::default()
        };
        let rem_size = self.rem_size(cx);
        window.with_rem_size(rem_size, |window| {
            window.with_text_style(Some(text_style), |window| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    self.paint_mouse_listeners(layout, window);

                    self.paint_background(layout, window);

                    self.paint_indent_guides(layout, window);

                    if layout.gutter_hitbox.size.width > Pixels::ZERO {
                        self.paint_blamed_display_rows(layout, window, cx);
                        self.paint_line_numbers(layout, window, cx);
                    }

                    self.paint_text(layout, window, cx);

                    if !layout.spacer_blocks.is_empty() {
                        window.with_element_namespace("blocks", |window| {
                            self.paint_spacer_blocks(layout, window, cx);
                        });
                    }

                    if layout.gutter_hitbox.size.width > Pixels::ZERO {
                        self.paint_gutter_highlights(layout, window, cx);
                        self.paint_gutter_indicators(layout, window, cx);
                    }

                    if !layout.blocks.is_empty() {
                        window.with_element_namespace("blocks", |window| {
                            self.paint_non_spacer_blocks(layout, window, cx);
                        });
                    }

                    window.with_element_namespace("blocks", |window| {
                        if let Some(mut sticky_header) = layout.sticky_buffer_header.take() {
                            sticky_header.paint(window, cx);
                        }
                    });

                    self.paint_minimap(layout, window, cx);
                    self.paint_scrollbars(layout, window, cx);
                    self.paint_mouse_context_menu(layout, window, cx);
                });
            });
        });
    }
}

const fn gutter_bounds(editor_bounds: Bounds<Pixels>, gutter_dimensions: GutterDimensions) -> Bounds<Pixels> {
    Bounds { origin: editor_bounds.origin, size: size(gutter_dimensions.width, editor_bounds.size.height) }
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub struct EditorLayout {
    position_map: Rc<PositionMap>,
    hitbox: Hitbox,
    gutter_hitbox: Hitbox,
    content_origin: gpui::Point<Pixels>,
    scrollbars_layout: Option<EditorScrollbars>,
    minimap: Option<MinimapLayout>,
    mode: EditorMode,
    wrap_guides: SmallVec<[(Pixels, bool); 2]>,
    indent_guides: Option<Vec<IndentGuideLayout>>,
    visible_display_row_range: Range<DisplayRow>,
    active_rows: BTreeMap<DisplayRow, LineHighlightSpec>,
    highlighted_rows: BTreeMap<DisplayRow, LineHighlight>,
    line_elements: SmallVec<[AnyElement; 1]>,
    line_numbers: Arc<FxHashMap<MultiBufferRow, LineNumberLayout>>,
    display_hunks: Vec<(DisplayDiffHunk, Option<Hitbox>)>,
    blamed_display_rows: Option<Vec<AnyElement>>,
    inline_diagnostics: FxHashMap<DisplayRow, AnyElement>,
    point_diagnostic_underline_offset: Pixels,
    inline_code_actions: Option<AnyElement>,
    blocks: Vec<BlockLayout>,
    spacer_blocks: Vec<BlockLayout>,
    highlighted_ranges: Vec<(Range<DisplayPoint>, Hsla)>,
    highlighted_gutter_ranges: Vec<(Range<DisplayPoint>, Hsla)>,
    cursors: Vec<(DisplayPoint, Hsla)>,
    visible_cursors: Vec<CursorLayout>,
    navigation_overlay_paint_commands: Vec<AnyElement>,
    selections: Vec<(CursorColors, Vec<SelectionLayout>)>,
    test_indicators: Vec<AnyElement>,
    gutter_hover_buttons: Vec<AnyElement>,
    crease_toggles: Vec<Option<AnyElement>>,
    expand_toggles: Vec<Option<(AnyElement, gpui::Point<Pixels>)>>,
    diff_hunk_controls: Vec<AnyElement>,
    crease_trailers: Vec<Option<CreaseTrailerLayout>>,
    mouse_context_menu: Option<AnyElement>,
    tab_invisible: ShapedLine,
    space_invisible: ShapedLine,
    sticky_buffer_header: Option<AnyElement>,
    #[allow(clippy::type_complexity)]
    text_align: TextAlign,
    content_width: Pixels,
}

#[derive(Debug)]
struct LineNumberSegment {
    shaped_line: ShapedLine,
    hitbox: Option<Hitbox>,
}

#[derive(Debug)]
struct LineNumberLayout {
    segments: SmallVec<[LineNumberSegment; 1]>,
}

struct ColoredRange<T> {
    start: T,
    end: T,
    color: Hsla,
}

struct CreaseTrailerLayout {
    element: AnyElement,
    bounds: Bounds<Pixels>,
}

pub(crate) struct PositionMap {
    size: Size<Pixels>,
    line_height: Pixels,
    scroll_position: gpui::Point<ScrollOffset>,
    scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
    scroll_max: gpui::Point<ScrollOffset>,
    pub(crate) em_advance: Pixels,
    em_layout_width: Pixels,
    visible_row_range: Range<DisplayRow>,
    line_layouts: Vec<LineWithInvisibles>,
    pub(crate) snapshot: EditorSnapshot,
    text_align: TextAlign,
    content_width: Pixels,
    content_origin: gpui::Point<Pixels>,
    pub(crate) text_hitbox: Hitbox,
    gutter_hitbox: Hitbox,
    display_hunks: Vec<(DisplayDiffHunk, Option<Hitbox>)>,
    diff_hunk_control_bounds: Vec<(DisplayRow, Bounds<Pixels>)>,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct PointForPosition {
    pub(crate) previous_valid: DisplayPoint,
    pub(crate) next_valid: DisplayPoint,
    nearest_valid: DisplayPoint,
    pub(crate) exact_unclipped: DisplayPoint,
}

impl PointForPosition {
    pub(crate) fn as_valid(&self) -> Option<DisplayPoint> {
        (self.previous_valid == self.exact_unclipped && self.next_valid == self.exact_unclipped)
            .then_some(self.previous_valid)
    }
}

impl PositionMap {
    pub(crate) fn inlay_hint_glyph_for_position(&self, position: gpui::Point<Pixels>) -> Option<DisplayPoint> {
        let position = position - self.content_origin;
        if position.y < Pixels::ZERO {
            return None;
        }
        let row = ((position.y / self.line_height) as f64 + self.scroll_position.y) as u32;
        let line_index = row.checked_sub(self.visible_row_range.start.0)?;
        let line = self.line_layouts.get(line_index as usize)?;
        let x = position.x + (self.scroll_position.x as f32 * self.em_layout_width)
            - line.alignment_offset(self.text_align, self.content_width);
        if x < Pixels::ZERO {
            return None;
        }
        let glyph = DisplayPoint::new(DisplayRow(row), line.index_for_x(x)? as u32);
        self.snapshot.inlay_hint_at(glyph).map(|_| glyph)
    }

    pub(crate) fn point_for_position(&self, position: gpui::Point<Pixels>) -> PointForPosition {
        let text_bounds = self.text_hitbox.bounds;
        let scroll_position = self.scroll_position;
        let position = position - text_bounds.origin;
        let y = position.y.max(px(0.)).min(self.size.height);
        let x = position.x + (scroll_position.x as f32 * self.em_layout_width);
        let row = ((y / self.line_height) as f64 + scroll_position.y) as u32;

        let (column, x_overshoot_after_line_end) = if let Some(line_index) =
            row.checked_sub(self.visible_row_range.start.0)
            && let Some(line) = self.line_layouts.get(line_index as usize)
        {
            let alignment_offset = line.alignment_offset(self.text_align, self.content_width);
            let x_relative_to_text = x - alignment_offset;
            if let Some(ix) = line.index_for_x(x_relative_to_text) {
                (ix as u32, px(0.))
            } else {
                (line.len as u32, px(0.).max(x_relative_to_text - line.width))
            }
        } else {
            (0, x)
        };

        let mut exact_unclipped = DisplayPoint::new(DisplayRow(row), column);
        let previous_valid = self.snapshot.clip_point(exact_unclipped, Bias::Left);
        let next_valid = self.snapshot.clip_point(exact_unclipped, Bias::Right);

        let nearest_valid = if previous_valid == next_valid {
            previous_valid
        } else {
            match self.snapshot.inlay_bias_at(exact_unclipped) {
                Some(Bias::Left) => next_valid,
                Some(Bias::Right) => previous_valid,
                None => previous_valid,
            }
        };

        let column_overshoot_after_line_end = (x_overshoot_after_line_end / self.em_layout_width) as u32;
        *exact_unclipped.column_mut() += column_overshoot_after_line_end;
        PointForPosition { previous_valid, next_valid, nearest_valid, exact_unclipped }
    }
}

/// X of the wrap guide at `column`, unless it falls outside the text area.
fn wrap_guide_x(
    column: usize,
    space_width: Pixels,
    horizontal_offset: Pixels,
    content_left: Pixels,
    content_right: Pixels,
) -> Option<Pixels> {
    let x = space_width * column as f32 + horizontal_offset;
    (x >= content_left && x <= content_right).then_some(x)
}

/// The layout of `row` among `lines`, which were laid out from `start_row` on.
fn visible_line<L>(lines: &[L], start_row: DisplayRow, row: DisplayRow) -> Option<&L> {
    lines.get(row.0.checked_sub(start_row.0)? as usize)
}

fn layout_line(
    row: DisplayRow,
    snapshot: &EditorSnapshot,
    style: &EditorStyle,
    is_row_soft_wrapped: impl Copy + Fn(usize) -> bool,
    window: &mut Window,
    cx: &mut App,
) -> LineWithInvisibles {
    let language_aware =
        LanguageAwareStyling { tree_sitter: true, diagnostics: snapshot.display_snapshot.diagnostics_visible() };
    let chunks = snapshot.highlighted_chunks(row..row + DisplayRow(1), language_aware, style);
    LineWithInvisibles::from_chunks(
        chunks,
        style,
        MAX_LINE_LEN,
        1,
        &snapshot.mode,
        is_row_soft_wrapped,
        &[],
        window,
        cx,
    )
    .pop()
    .expect("from_chunks yields at least one line")
}

#[derive(Debug, Clone)]
pub(crate) struct IndentGuideLayout {
    origin: gpui::Point<Pixels>,
    length: Pixels,
    display_row_range: Range<DisplayRow>,
    depth: u32,
    active: bool,
    settings: IndentGuideSettings,
}

#[derive(Debug)]
pub struct HighlightedRange {
    pub start_y: Pixels,
    pub line_height: Pixels,
    pub lines: Vec<HighlightedRangeLine>,
    pub color: Hsla,
    pub corner_radius: Pixels,
}

#[derive(Debug)]
pub struct HighlightedRangeLine {
    pub start_x: Pixels,
    pub end_x: Pixels,
}

impl HighlightedRange {
    pub fn paint(&self, fill: bool, bounds: Bounds<Pixels>, window: &mut Window) {
        if self.lines.len() >= 2 && self.lines[0].start_x > self.lines[1].end_x {
            self.paint_lines(self.start_y, &self.lines[0..1], fill, bounds, window);
            self.paint_lines(self.start_y + self.line_height, &self.lines[1..], fill, bounds, window);
        } else {
            self.paint_lines(self.start_y, &self.lines, fill, bounds, window);
        }
    }

    fn paint_lines(
        &self,
        start_y: Pixels,
        lines: &[HighlightedRangeLine],
        fill: bool,
        _bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        if lines.is_empty() {
            return;
        }

        let first_line = lines.first().expect("lines are non-empty");
        let last_line = lines.last().expect("lines are non-empty");

        let first_top_left = point(first_line.start_x, start_y);
        let first_top_right = point(first_line.end_x, start_y);

        let curve_height = point(Pixels::ZERO, self.corner_radius);
        let curve_width = |start_x: Pixels, end_x: Pixels| {
            let max = (end_x - start_x) / 2.;
            let width = if max < self.corner_radius { max } else { self.corner_radius };

            point(width, Pixels::ZERO)
        };

        let top_curve_width = curve_width(first_line.start_x, first_line.end_x);
        let mut builder = if fill { gpui::PathBuilder::fill() } else { gpui::PathBuilder::stroke(px(1.)) };
        builder.move_to(first_top_right - top_curve_width);
        builder.curve_to(first_top_right + curve_height, first_top_right);

        let mut iter = lines.iter().enumerate().peekable();
        while let Some((ix, line)) = iter.next() {
            let bottom_right = point(line.end_x, start_y + (ix + 1) as f32 * self.line_height);

            if let Some((_, next_line)) = iter.peek() {
                let next_top_right = point(next_line.end_x, bottom_right.y);

                match next_top_right.x.partial_cmp(&bottom_right.x).expect("pixel coordinates are not NaN") {
                    Ordering::Equal => {
                        builder.line_to(bottom_right);
                    }
                    Ordering::Less => {
                        let curve_width = curve_width(next_top_right.x, bottom_right.x);
                        builder.line_to(bottom_right - curve_height);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(bottom_right - curve_width, bottom_right);
                        }
                        builder.line_to(next_top_right + curve_width);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(next_top_right + curve_height, next_top_right);
                        }
                    }
                    Ordering::Greater => {
                        let curve_width = curve_width(bottom_right.x, next_top_right.x);
                        builder.line_to(bottom_right - curve_height);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(bottom_right + curve_width, bottom_right);
                        }
                        builder.line_to(next_top_right - curve_width);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(next_top_right + curve_height, next_top_right);
                        }
                    }
                }
            } else {
                let curve_width = curve_width(line.start_x, line.end_x);
                builder.line_to(bottom_right - curve_height);
                if self.corner_radius > Pixels::ZERO {
                    builder.curve_to(bottom_right - curve_width, bottom_right);
                }

                let bottom_left = point(line.start_x, bottom_right.y);
                builder.line_to(bottom_left + curve_width);
                if self.corner_radius > Pixels::ZERO {
                    builder.curve_to(bottom_left - curve_height, bottom_left);
                }
            }
        }

        if first_line.start_x > last_line.start_x {
            let curve_width = curve_width(last_line.start_x, first_line.start_x);
            let second_top_left = point(last_line.start_x, start_y + self.line_height);
            builder.line_to(second_top_left + curve_height);
            if self.corner_radius > Pixels::ZERO {
                builder.curve_to(second_top_left + curve_width, second_top_left);
            }
            let first_bottom_left = point(first_line.start_x, second_top_left.y);
            builder.line_to(first_bottom_left - curve_width);
            if self.corner_radius > Pixels::ZERO {
                builder.curve_to(first_bottom_left - curve_height, first_bottom_left);
            }
        }

        builder.line_to(first_top_left + curve_height);
        if self.corner_radius > Pixels::ZERO {
            builder.curve_to(first_top_left + top_curve_width, first_top_left);
        }
        builder.line_to(first_top_right - top_curve_width);

        if let Ok(path) = builder.build() {
            window.paint_path(path, self.color);
        }
    }
}

/// Shared between `prepaint` and `compute_auto_height_layout` to ensure
/// both full and auto-height editors compute wrap widths consistently.
fn calculate_wrap_width(soft_wrap: SoftWrap, editor_width: Pixels, em_width: Pixels) -> Pixels {
    let wrap_width_for = |column: u32| (column as f32 * em_width).ceil();

    match soft_wrap {
        SoftWrap::None => wrap_width_for(MAX_LINE_LEN as u32 / 2),
        SoftWrap::EditorWidth => editor_width,
    }
}

fn compute_auto_height_layout(
    editor: &mut Editor,
    min_lines: usize,
    max_lines: Option<usize>,
    known_dimensions: Size<Option<Pixels>>,
    available_width: AvailableSpace,
    window: &Window,
    cx: &mut Context<Editor>,
) -> Option<Size<Pixels>> {
    let width = known_dimensions.width.or({
        if let AvailableSpace::Definite(available_width) = available_width { Some(available_width) } else { None }
    })?;
    if let Some(height) = known_dimensions.height {
        return Some(size(width, height));
    }

    let style = editor.style.as_ref().expect("style is set before rendering");
    let font_id = window.text_system().resolve_font(&style.text.font());
    let font_size = style.text.font_size.to_pixels(window.rem_size());
    let line_height = style.text.line_height_in_pixels(window.rem_size());
    let em_width = window.text_system().em_width(font_id, font_size);

    let mut snapshot = editor.snapshot(window, cx);
    let gutter_dimensions = snapshot.gutter_dimensions(font_id, font_size, window, cx);

    editor.gutter_dimensions = gutter_dimensions;
    let text_width = width - gutter_dimensions.width;
    let overscroll = size(em_width, px(0.));

    let editor_width = text_width - gutter_dimensions.margin - overscroll.width - em_width;
    let wrap_width = Some(calculate_wrap_width(editor.soft_wrap_mode(cx), editor_width, em_width).min(editor_width));
    if wrap_width.is_some() && editor.set_wrap_width(wrap_width, cx) {
        snapshot = editor.snapshot(window, cx);
    }

    let scroll_height = (snapshot.max_point().row().next_row().0 as f32) * line_height;

    let min_height = line_height * min_lines as f32;
    let content_height = scroll_height.max(min_height);

    let final_height = if let Some(max_lines) = max_lines {
        let max_height = line_height * max_lines as f32;
        content_height.min(max_height)
    } else {
        content_height
    };

    Some(size(width, final_height))
}

/// Buffers load with parsing deferred (see `Buffer::defer_parsing`); start the
/// parse for the ones this frame draws.
fn parse_drawn_buffers(editor: &Entity<Editor>, row_infos: &[RowInfo], cx: &mut App) {
    let multi_buffer = editor.read(cx).buffer().clone();
    for buffer_id in drawn_buffer_ids(row_infos) {
        if let Some(buffer) = multi_buffer.read(cx).buffer(buffer_id)
            && buffer.read(cx).is_parse_deferred()
        {
            buffer.update(cx, |buffer, cx| buffer.ensure_parsed(cx));
        }
    }
}

/// The distinct buffers behind the given rows, in first-drawn order.
fn drawn_buffer_ids(row_infos: &[RowInfo]) -> Vec<BufferId> {
    let mut buffer_ids: Vec<BufferId> = Vec::new();
    for buffer_id in row_infos.iter().filter_map(|row_info| row_info.buffer_id) {
        if !buffer_ids.contains(&buffer_id) {
            buffer_ids.push(buffer_id);
        }
    }
    buffer_ids
}

#[cfg(test)]
mod tests {

    use super::BTreeMap;
    use super::Bounds;
    use super::BufferId;
    use super::DisplayPoint;
    use super::DisplayRow;
    use super::EditorElement;
    use super::RowInfo;
    use super::ScrollbarLayoutInformation;
    use super::drawn_buffer_ids;
    use super::mark_active_rows;
    use super::point;
    use super::px;
    use super::size;
    use super::visible_line;
    use super::wrap_guide_x;

    #[test]
    fn visible_line_indexes_from_the_start_row() {
        let lines = ["a", "b", "c"];
        assert_eq!(visible_line(&lines, DisplayRow(5), DisplayRow(5)), Some(&"a"));
        assert_eq!(visible_line(&lines, DisplayRow(5), DisplayRow(7)), Some(&"c"));
        assert_eq!(visible_line(&lines, DisplayRow(5), DisplayRow(8)), None);
        assert_eq!(visible_line(&lines, DisplayRow(5), DisplayRow(4)), None);
    }

    #[test]
    fn wrap_guide_x_scales_with_column_and_clips_to_text_area() {
        // 80 columns of 8px spaces, text starting at x=50.
        assert_eq!(wrap_guide_x(80, px(8.), px(50.), px(50.), px(1000.)), Some(px(690.)));
        // Scrolled so the guide is left of the text area.
        assert_eq!(wrap_guide_x(80, px(8.), px(-700.), px(50.), px(1000.)), None);
        // Past the right edge.
        assert_eq!(wrap_guide_x(200, px(8.), px(50.), px(50.), px(1000.)), None);
        // The edges themselves are inside.
        assert_eq!(wrap_guide_x(0, px(8.), px(50.), px(50.), px(1000.)), Some(px(50.)));
    }

    #[test]
    fn bg_segments_are_empty_without_highlights_or_selections() {
        let rows = DisplayRow(0)..DisplayRow(3);
        assert!(EditorElement::bg_segments_per_row(rows, &[], [], gpui::black()).is_empty());
    }

    #[test]
    fn bg_segments_split_a_highlight_per_row() {
        let rows = DisplayRow(0)..DisplayRow(3);
        let color = gpui::red();
        let highlight = DisplayPoint::new(DisplayRow(0), 2)..DisplayPoint::new(DisplayRow(1), 4);
        let segments =
            EditorElement::bg_segments_per_row(rows.clone(), &[], [(highlight.clone(), color)], gpui::black());
        assert_eq!(segments.len(), 3);
        assert_eq!(
            segments[0].iter().map(|(range, _)| (range.start, range.end)).collect::<Vec<_>>(),
            [(2, u32::MAX as usize)]
        );
        assert_eq!(segments[1].iter().map(|(range, _)| (range.start, range.end)).collect::<Vec<_>>(), [(0, 4)]);
        assert!(segments[2].is_empty());

        // A translucent editor background can't be blended against.
        assert!(
            EditorElement::bg_segments_per_row(rows, &[], [(highlight, color)], gpui::transparent_black()).is_empty()
        );
    }

    #[test]
    fn drawn_buffer_ids_are_distinct_in_draw_order() {
        let row =
            |id: Option<u64>| RowInfo { buffer_id: id.map(|id| BufferId::new(id).unwrap()), ..RowInfo::default() };
        let rows = [row(Some(2)), row(Some(2)), row(None), row(Some(1)), row(Some(2)), row(Some(3))];
        let ids = drawn_buffer_ids(&rows).into_iter().map(u64::from).collect::<Vec<_>>();
        assert_eq!(ids, [2, 1, 3]);
        assert!(drawn_buffer_ids(&[row(None)]).is_empty());
    }

    #[test]
    fn active_rows_skip_block_rows() {
        let mut active_rows = BTreeMap::new();
        mark_active_rows(&mut active_rows, 3..=6, false, |row| row.0 == 4 || row.0 == 5);
        assert_eq!(active_rows.keys().map(|row| row.0).collect::<Vec<_>>(), [3, 6]);
    }

    #[test]
    fn active_rows_keep_selection_flag() {
        let mut active_rows = BTreeMap::new();
        mark_active_rows(&mut active_rows, 2..=2, false, |_| false);
        mark_active_rows(&mut active_rows, 2..=2, true, |_| false);
        assert!(active_rows[&DisplayRow(2)].selection);
    }

    #[test]
    fn scroll_range_overscrolls_one_page_or_one_line() {
        let editor_bounds = Bounds { origin: point(px(0.), px(0.)), size: size(px(400.), px(300.)) };
        let glyph_grid_cell = size(px(8.), px(20.));
        let document_size = size(px(500.), px(1000.));

        let one_page = ScrollbarLayoutInformation::new(editor_bounds, glyph_grid_cell, document_size, true);
        assert_eq!(one_page.scroll_range, size(px(500.), px(1300.)));

        let one_line = ScrollbarLayoutInformation::new(editor_bounds, glyph_grid_cell, document_size, false);
        assert_eq!(one_line.scroll_range, size(px(500.), px(1020.)));
    }
}
