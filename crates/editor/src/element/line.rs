use std::borrow::Cow;
use std::fmt;
use std::mem;
use std::ops::Range;
use std::sync::Arc;

use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::FontId;
use gpui::Hsla;
use gpui::Pixels;
use gpui::ShapedLine;
use gpui::SharedString;
use gpui::Size;
use gpui::TextAlign;
use gpui::TextRun;
use gpui::Window;
use gpui::point;
use gpui::px;
use gpui::size;
use gpui::underline_y_offset;
use smallvec::SmallVec;
use ui::utils::ensure_highlight_contrast;
use util::ResultExt as _;

use super::DiagnosticUnderline;
use super::EditorLayout;
use super::MINIMAP_BLOCK_HEIGHT;
use super::MINIMAP_BLOCK_OPACITY;
use super::PointDiagnostic;
use crate::ChunkRendererContext;
use crate::ChunkReplacement;
use crate::DisplayPoint;
use crate::DisplayRow;
use crate::EditorMode;
use crate::EditorStyle;
use crate::RowExt as _;
use crate::display_map::ChunkRendererId;
use crate::display_map::HighlightedChunk;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollPixelOffset;

#[derive(Debug)]
pub(crate) struct LineWithInvisibles {
    pub(super) fragments: SmallVec<[LineFragment; 1]>,
    pub(super) invisibles: Vec<Invisible>,
    pub(super) diagnostic_underline_severity_ranges: Vec<(Range<usize>, lsp::DiagnosticSeverity)>,
    pub(super) point_diagnostics: Vec<PointDiagnostic>,
    pub(super) len: usize,
    pub(crate) width: Pixels,
    pub(super) font_size: Pixels,
}

#[allow(clippy::large_enum_variant)]
pub(super) enum LineFragment {
    Text(ShapedLine),
    Element { id: ChunkRendererId, element: Option<AnyElement>, size: Size<Pixels>, len: usize },
}

impl fmt::Debug for LineFragment {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LineFragment::Text(line) => f.debug_tuple("Text").field(line).finish(),
            LineFragment::Element { size, len, .. } => {
                f.debug_struct("Element").field("size", size).field("len", len).finish()
            }
        }
    }
}

/// Minimap text is too small to read, so it's laid out on a monospace grid
/// without shaping and painted as blocks (see `LineWithInvisibles::draw`).
pub(super) fn shape_editor_line(
    text: SharedString,
    font_size: Pixels,
    runs: &[TextRun],
    editor_mode: &EditorMode,
    window: &Window,
) -> ShapedLine {
    if editor_mode.is_minimap() {
        window.text_system().layout_monospace_line(text, font_size, runs)
    } else {
        window.text_system().shape_line(text, font_size, runs, None)
    }
}

impl LineWithInvisibles {
    pub(super) fn from_chunks<'a>(
        chunks: impl Iterator<Item = HighlightedChunk<'a>>,
        editor_style: &EditorStyle,
        max_line_len: usize,
        max_line_count: usize,
        editor_mode: &EditorMode,
        text_width: Pixels,
        is_row_soft_wrapped: impl Copy + Fn(usize) -> bool,
        bg_segments_per_row: &[Vec<(Range<usize>, Hsla)>],
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Self> {
        let text_style = &editor_style.text;
        let mut layouts = Vec::with_capacity(max_line_count);
        let mut fragments: SmallVec<[LineFragment; 1]> = SmallVec::new();
        let mut line = String::new();
        // Byte offset into the logical line used to position invisible markers.
        // Unlike `line`, this is not cleared when we flush `shape_line` for
        // mid-line inlays/replacements, so marker offsets stay correct in that case.
        let mut line_byte_offset: usize = 0;
        let mut invisibles = Vec::new();
        let mut diagnostic_underline_severity_ranges = Vec::new();
        let mut width = Pixels::ZERO;
        let mut len = 0;
        let mut styles = Vec::new();
        let mut non_whitespace_added = false;
        let mut row = 0;
        let mut line_exceeded_max_len = false;
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let min_contrast = crate::MINIMUM_CONTRAST_FOR_HIGHLIGHTS;

        let ellipsis = SharedString::from("⋯");

        for highlighted_chunk in chunks.chain([HighlightedChunk {
            text: "\n",
            style: None,
            diagnostic_underline_severity: None,
            is_tab: false,
            is_inlay: false,
            replacement: None,
        }]) {
            if let Some(replacement) = highlighted_chunk.replacement {
                if line_exceeded_max_len {
                    continue;
                }

                if len + line.len() + highlighted_chunk.text.len() > max_line_len {
                    line_exceeded_max_len = true;
                    continue;
                }

                if !line.is_empty() {
                    let segments = bg_segments_per_row.get(row).map_or(&[][..], |v| &**v);
                    let text_runs: &[TextRun] = if segments.is_empty() {
                        &styles
                    } else {
                        &ensure_highlight_contrast(&styles, segments, min_contrast, len)
                    };
                    let shaped_line =
                        shape_editor_line(line.as_str().into(), font_size, text_runs, editor_mode, window);
                    width += shaped_line.width;
                    len += shaped_line.len;

                    fragments.push(LineFragment::Text(shaped_line));
                    line.clear();
                    styles.clear();
                }

                match replacement {
                    ChunkReplacement::Renderer(renderer) => {
                        let available_width = if renderer.constrain_width {
                            let chunk = if highlighted_chunk.text == ellipsis.as_ref() {
                                ellipsis.clone()
                            } else {
                                SharedString::from(Arc::from(highlighted_chunk.text))
                            };
                            let shaped_line = window.text_system().shape_line(
                                chunk,
                                font_size,
                                &[text_style.to_run(highlighted_chunk.text.len())],
                                None,
                            );
                            AvailableSpace::Definite(shaped_line.width)
                        } else {
                            AvailableSpace::MinContent
                        };

                        let mut element =
                            (renderer.render)(&mut ChunkRendererContext { context: cx, window, max_width: text_width });
                        let line_height = text_style.line_height_in_pixels(window.rem_size());
                        let size = element.layout_as_root(
                            size(available_width, AvailableSpace::Definite(line_height)),
                            window,
                            cx,
                        );

                        width += size.width;
                        len += highlighted_chunk.text.len();
                        line_byte_offset += highlighted_chunk.text.len();
                        fragments.push(LineFragment::Element {
                            id: renderer.id,
                            element: Some(element),
                            size,
                            len: highlighted_chunk.text.len(),
                        });
                    }
                    ChunkReplacement::Str(x) => {
                        let text_style = if let Some(style) = highlighted_chunk.style {
                            Cow::Owned(text_style.clone().highlight(style))
                        } else {
                            Cow::Borrowed(text_style)
                        };

                        let run = TextRun {
                            len: x.len(),
                            font: text_style.font(),
                            color: text_style.color,
                            background_color: text_style.background_color,
                            underline: text_style.underline,
                            strikethrough: text_style.strikethrough,
                        };

                        let line_layout = shape_editor_line(x, font_size, &[run], editor_mode, window)
                            .with_len(highlighted_chunk.text.len());

                        if let Some(severity) = highlighted_chunk.diagnostic_underline_severity {
                            diagnostic_underline_severity_ranges
                                .push((line_byte_offset..line_byte_offset + highlighted_chunk.text.len(), severity));
                        }
                        width += line_layout.width;
                        len += highlighted_chunk.text.len();
                        line_byte_offset += highlighted_chunk.text.len();
                        fragments.push(LineFragment::Text(line_layout));
                    }
                }
            } else {
                for (ix, mut line_chunk) in highlighted_chunk.text.split('\n').enumerate() {
                    if ix > 0 {
                        let segments = bg_segments_per_row.get(row).map_or(&[][..], |v| &**v);
                        let text_runs = if segments.is_empty() {
                            &styles
                        } else {
                            &ensure_highlight_contrast(&styles, segments, min_contrast, len)
                        };
                        let shaped_line =
                            shape_editor_line(line.as_str().into(), font_size, text_runs, editor_mode, window);
                        width += shaped_line.width;
                        len += shaped_line.len;

                        fragments.push(LineFragment::Text(shaped_line));
                        layouts.push(Self {
                            width: mem::take(&mut width),
                            len: mem::take(&mut len),
                            fragments: mem::take(&mut fragments),
                            invisibles: mem::take(&mut invisibles),
                            diagnostic_underline_severity_ranges: mem::take(&mut diagnostic_underline_severity_ranges),
                            point_diagnostics: Vec::new(),
                            font_size,
                        });

                        line.clear();
                        line_byte_offset = 0;
                        styles.clear();
                        row += 1;
                        line_exceeded_max_len = false;
                        non_whitespace_added = false;
                        if row == max_line_count {
                            return layouts;
                        }
                    }

                    if !line_chunk.is_empty() && !line_exceeded_max_len {
                        let text_style = if let Some(style) = highlighted_chunk.style {
                            Cow::Owned(text_style.clone().highlight(style))
                        } else {
                            Cow::Borrowed(text_style)
                        };

                        let current_line_len = len + line.len();
                        if current_line_len + line_chunk.len() > max_line_len {
                            let mut chunk_len = max_line_len - current_line_len;
                            while !line_chunk.is_char_boundary(chunk_len) {
                                chunk_len -= 1;
                            }
                            line_chunk = &line_chunk[..chunk_len];
                            line_exceeded_max_len = true;
                        }

                        if line_chunk.is_empty() {
                            continue;
                        }

                        styles.push(TextRun {
                            len: line_chunk.len(),
                            font: text_style.font(),
                            color: text_style.color,
                            background_color: text_style.background_color,
                            underline: text_style.underline,
                            strikethrough: text_style.strikethrough,
                        });

                        if let Some(severity) = highlighted_chunk.diagnostic_underline_severity {
                            diagnostic_underline_severity_ranges
                                .push((line_byte_offset..line_byte_offset + line_chunk.len(), severity));
                        }

                        if editor_mode.is_full() && !highlighted_chunk.is_inlay {
                            // Line wrap pads its contents with fake whitespaces,
                            // avoid printing them
                            let is_soft_wrapped = is_row_soft_wrapped(row);
                            if highlighted_chunk.is_tab {
                                if non_whitespace_added || !is_soft_wrapped {
                                    invisibles.push(Invisible::Tab {
                                        line_start_offset: line_byte_offset,
                                        line_end_offset: line_byte_offset + line_chunk.len(),
                                    });
                                }
                            } else {
                                invisibles.extend(line_chunk.char_indices().filter_map(|(index, c)| {
                                    let is_whitespace = c.is_whitespace();
                                    non_whitespace_added |= !is_whitespace;
                                    (is_whitespace && (non_whitespace_added || !is_soft_wrapped)).then(|| {
                                        Invisible::Whitespace {
                                            line_start_offset: line_byte_offset + index,
                                            line_end_offset: line_byte_offset + index + c.len_utf8(),
                                        }
                                    })
                                }));
                            }
                        }

                        line.push_str(line_chunk);
                        line_byte_offset += line_chunk.len();
                    }
                }
            }
        }

        layouts
    }

    /// A copy for caching, unless the line holds rendered elements, which can't be copied.
    pub(super) fn cacheable_clone(&self) -> Option<LineWithInvisibles> {
        let fragments = self
            .fragments
            .iter()
            .map(|fragment| match fragment {
                LineFragment::Text(line) => Some(LineFragment::Text(line.clone())),
                LineFragment::Element { .. } => None,
            })
            .collect::<Option<SmallVec<[LineFragment; 1]>>>()?;
        Some(LineWithInvisibles {
            fragments,
            invisibles: self.invisibles.clone(),
            diagnostic_underline_severity_ranges: self.diagnostic_underline_severity_ranges.clone(),
            point_diagnostics: self.point_diagnostics.clone(),
            len: self.len,
            width: self.width,
            font_size: self.font_size,
        })
    }

    pub(super) fn add_point_diagnostic(&mut self, point_diagnostic: PointDiagnostic) {
        if let Some(existing) =
            self.point_diagnostics.iter_mut().find(|existing| existing.column == point_diagnostic.column)
        {
            if point_diagnostic.severity < existing.severity {
                *existing = point_diagnostic;
            }
        } else {
            self.point_diagnostics.push(point_diagnostic);
        }
    }

    /// Takes text runs and non-overlapping left-to-right background ranges with color.
    /// Returns new text runs with adjusted contrast as per background ranges.
    pub(super) fn prepaint(
        &mut self,
        line_height: Pixels,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        row: DisplayRow,
        content_origin: gpui::Point<Pixels>,
        text_align: TextAlign,
        content_width: Pixels,
        line_elements: &mut SmallVec<[AnyElement; 1]>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_y = f32::from(line_height) * Pixels::from(row.as_f64() - scroll_position.y);
        let mut fragment_origin = content_origin
            + point(self.alignment_offset(text_align, content_width) - Pixels::from(scroll_pixel_position.x), line_y);
        for fragment in &mut self.fragments {
            match fragment {
                LineFragment::Text(line) => {
                    fragment_origin.x += line.width;
                }
                LineFragment::Element { element, size, .. } => {
                    let mut element = element.take().expect("you can't prepaint LineWithInvisibles twice");

                    // Center the element vertically within the line.
                    let mut element_origin = fragment_origin;
                    element_origin.y += (line_height - size.height) / 2.;
                    element.prepaint_at(element_origin, window, cx);
                    line_elements.push(element);

                    fragment_origin.x += size.width;
                }
            }
        }
    }

    pub(super) fn draw(
        &self,
        layout: &EditorLayout,
        row: DisplayRow,
        content_origin: gpui::Point<Pixels>,
        selection_ranges: &[Range<DisplayPoint>],
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_y = layout.position_map.line_height * (row.as_f64() - layout.position_map.scroll_position.y) as f32;
        let line_height = layout.position_map.line_height;
        let mut fragment_origin = content_origin
            + point(
                self.alignment_offset(layout.text_align, layout.content_width)
                    - Pixels::from(layout.position_map.scroll_pixel_position.x),
                line_y,
            );
        let mut points = self
            .point_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.column as usize <= self.len)
            .collect::<SmallVec<[_; 4]>>();
        points.sort_unstable_by_key(|diagnostic| diagnostic.column);
        let has_points = !points.is_empty();
        let mut points = points.into_iter().peekable();
        let mut underlines = SmallVec::<[DiagnosticUnderline; 4]>::new();
        let mut diagnostic_ranges = self.diagnostic_underline_severity_ranges.iter().peekable();
        let mut fragment_start = 0;
        let mut end_underline_offset = layout.point_diagnostic_underline_offset;

        for fragment in &self.fragments {
            let (fragment_len, fragment_width, line) = match fragment {
                LineFragment::Text(line) if layout.mode.is_minimap() => {
                    let block_top = fragment_origin.y + line_height * (1. - MINIMAP_BLOCK_HEIGHT) / 2.;
                    line.paint_blocks(
                        point(fragment_origin.x, block_top),
                        line_height * MINIMAP_BLOCK_HEIGHT,
                        MINIMAP_BLOCK_OPACITY,
                        window,
                    );
                    (line.len(), line.width, Some(line))
                }
                LineFragment::Text(line) => {
                    if has_points {
                        let mut glyphs = line.runs.iter().flat_map(|run| &run.glyphs).peekable();
                        line.paint_with_underline_handler(
                            fragment_origin,
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                            |range, origin, width, style, window| {
                                let bounds = window.underline_bounds(origin, width, style);
                                if bounds.is_empty() {
                                    return;
                                }
                                let source_range = if line.text.len() == line.len() {
                                    fragment_start + range.start..fragment_start + range.end
                                } else {
                                    fragment_start..fragment_start + line.len()
                                };
                                if source_range.is_empty() {
                                    window.paint_underline(origin, width, style);
                                    return;
                                }
                                let mut start = source_range.start;
                                let mut span_start = bounds.left();
                                while start < source_range.end {
                                    while diagnostic_ranges.peek().is_some_and(|(range, _)| range.end <= start) {
                                        diagnostic_ranges.next();
                                    }
                                    let (end, severity) = match diagnostic_ranges.peek().copied() {
                                        Some((range, severity)) if range.start <= start => {
                                            (range.end.min(source_range.end), Some(*severity))
                                        }
                                        Some((range, _)) => (range.start.min(source_range.end), None),
                                        None => (source_range.end, None),
                                    };
                                    let span_end = if end == source_range.end {
                                        bounds.right()
                                    } else {
                                        let index = end - fragment_start;
                                        while glyphs.peek().is_some_and(|glyph| glyph.index < index) {
                                            glyphs.next();
                                        }
                                        let x = fragment_origin.x
                                            + glyphs.peek().map_or(line.width, |glyph| glyph.position.x);
                                        window
                                            .underline_bounds(point(x, origin.y), Pixels::ZERO, style)
                                            .left()
                                            .clamp(span_start, bounds.right())
                                    };
                                    if span_start < span_end {
                                        if let Some(severity) = severity {
                                            underlines.push(DiagnosticUnderline {
                                                origin,
                                                width,
                                                style: *style,
                                                bounds,
                                                span: span_start..span_end,
                                                severity,
                                                is_point: false,
                                            });
                                        } else {
                                            window.paint_underline_with_exclusions(
                                                origin,
                                                width,
                                                style,
                                                &[bounds.left()..span_start, span_end..bounds.right()],
                                            );
                                        }
                                    }
                                    start = end;
                                    span_start = span_end;
                                }
                            },
                        )
                        .log_err();
                    } else {
                        line.paint(fragment_origin, line_height, TextAlign::Left, None, window, cx).log_err();
                    }
                    (line.len(), line.width, Some(line))
                }
                LineFragment::Element { size, len, .. } => (*len, size.width, None),
            };
            let fragment_end = fragment_start + fragment_len;
            if has_points {
                let mut glyphs = line.into_iter().flat_map(|line| &line.runs).flat_map(|run| &run.glyphs).peekable();
                let has_glyphs = glyphs.peek().is_some();
                let underline_offset =
                    line.filter(|_| has_glyphs).map_or(layout.point_diagnostic_underline_offset, |line| {
                        underline_y_offset(line_height, line.ascent, line.descent)
                    });
                if fragment_len > 0 || fragment_width != Pixels::ZERO || has_glyphs {
                    end_underline_offset = underline_offset;
                }
                while points.peek().is_some_and(|diagnostic| (diagnostic.column as usize) < fragment_end) {
                    let Some(diagnostic) = points.next() else {
                        break;
                    };
                    let index = diagnostic.column as usize - fragment_start;
                    let x = if let Some(line) = line {
                        let index = if line.text.len() == line.len() { index } else { 0 };
                        while glyphs.peek().is_some_and(|glyph| glyph.index < index) {
                            glyphs.next();
                        }
                        glyphs.peek().map_or(line.width, |glyph| glyph.position.x)
                    } else {
                        Pixels::ZERO
                    };
                    underlines.push(DiagnosticUnderline::point(
                        diagnostic,
                        fragment_origin + point(x, underline_offset),
                        layout.position_map.em_advance,
                        window,
                    ));
                }
            }
            fragment_origin.x += fragment_width;
            fragment_start = fragment_end;
        }
        for diagnostic in points {
            underlines.push(DiagnosticUnderline::point(
                diagnostic,
                fragment_origin + point(Pixels::ZERO, end_underline_offset),
                layout.position_map.em_advance,
                window,
            ));
        }

        self.draw_invisibles(selection_ranges, layout, content_origin, line_y, row, line_height, window, cx);
        DiagnosticUnderline::paint_all(&underlines, window);
    }

    pub(super) fn draw_background(
        &self,
        layout: &EditorLayout,
        row: DisplayRow,
        content_origin: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = layout.position_map.line_height;
        let line_y = line_height * (row.as_f64() - layout.position_map.scroll_position.y) as f32;

        let mut fragment_origin = content_origin
            + point(
                self.alignment_offset(layout.text_align, layout.content_width)
                    - Pixels::from(layout.position_map.scroll_pixel_position.x),
                line_y,
            );

        for fragment in &self.fragments {
            match fragment {
                LineFragment::Text(line) => {
                    line.paint_background(fragment_origin, line_height, TextAlign::Left, None, window, cx).log_err();
                    fragment_origin.x += line.width;
                }
                LineFragment::Element { size, .. } => {
                    fragment_origin.x += size.width;
                }
            }
        }
    }

    fn draw_invisibles(
        &self,
        selection_ranges: &[Range<DisplayPoint>],
        layout: &EditorLayout,
        content_origin: gpui::Point<Pixels>,
        line_y: Pixels,
        row: DisplayRow,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_origin = content_origin
            + point(
                self.alignment_offset(layout.text_align, layout.content_width)
                    - Pixels::from(layout.position_map.scroll_pixel_position.x),
                line_y,
            );
        let extract_whitespace_info = |invisible: &Invisible| {
            let (token_offset, token_end_offset, invisible_symbol) = match invisible {
                Invisible::Tab { line_start_offset, line_end_offset } => {
                    (*line_start_offset, *line_end_offset, &layout.tab_invisible)
                }
                Invisible::Whitespace { line_start_offset, line_end_offset } => {
                    (*line_start_offset, *line_end_offset, &layout.space_invisible)
                }
            };

            let token_x = self.x_for_index(token_offset);
            // Center the marker inside the actual glyph's width so it lines up with
            // proportional fonts instead of assuming a monospace `em_width` cell.
            let glyph_width = (self.x_for_index(token_end_offset) - token_x).max(Pixels::ZERO);
            let invisible_offset = (glyph_width - invisible_symbol.width).max(Pixels::ZERO) / 2.0;
            let origin = line_origin + point(token_x + invisible_offset, Pixels::ZERO);

            (
                [token_offset, token_end_offset],
                Box::new(move |window: &mut Window, cx: &mut App| {
                    invisible_symbol.paint(origin, line_height, TextAlign::Left, None, window, cx).log_err();
                }),
            )
        };

        // Whitespace is only drawn inside selections.
        for ([start, _], paint) in self.invisibles.iter().map(extract_whitespace_info) {
            let invisible_point = DisplayPoint::new(row, start as u32);
            if selection_ranges.iter().any(|region| region.start <= invisible_point && invisible_point < region.end) {
                paint(window, cx);
            }
        }
    }

    pub(crate) fn x_for_index(&self, index: usize) -> Pixels {
        let mut fragment_start_x = Pixels::ZERO;
        let mut fragment_start_index = 0;

        for fragment in &self.fragments {
            match fragment {
                LineFragment::Text(shaped_line) => {
                    let fragment_end_index = fragment_start_index + shaped_line.len;
                    if index < fragment_end_index {
                        return fragment_start_x + shaped_line.x_for_index(index - fragment_start_index);
                    }
                    fragment_start_x += shaped_line.width;
                    fragment_start_index = fragment_end_index;
                }
                LineFragment::Element { len, size, .. } => {
                    let fragment_end_index = fragment_start_index + len;
                    if index < fragment_end_index {
                        return fragment_start_x;
                    }
                    fragment_start_x += size.width;
                    fragment_start_index = fragment_end_index;
                }
            }
        }

        fragment_start_x
    }

    pub(crate) fn index_for_x(&self, x: Pixels) -> Option<usize> {
        let mut fragment_start_x = Pixels::ZERO;
        let mut fragment_start_index = 0;

        for fragment in &self.fragments {
            match fragment {
                LineFragment::Text(shaped_line) => {
                    let fragment_end_x = fragment_start_x + shaped_line.width;
                    if x < fragment_end_x {
                        return Some(fragment_start_index + shaped_line.index_for_x(x - fragment_start_x)?);
                    }
                    fragment_start_x = fragment_end_x;
                    fragment_start_index += shaped_line.len;
                }
                LineFragment::Element { len, size, .. } => {
                    let fragment_end_x = fragment_start_x + size.width;
                    if x < fragment_end_x {
                        return Some(fragment_start_index);
                    }
                    fragment_start_index += len;
                    fragment_start_x = fragment_end_x;
                }
            }
        }

        None
    }

    pub(crate) fn font_id_for_index(&self, index: usize) -> Option<FontId> {
        let mut fragment_start_index = 0;

        for fragment in &self.fragments {
            match fragment {
                LineFragment::Text(shaped_line) => {
                    let fragment_end_index = fragment_start_index + shaped_line.len;
                    if index < fragment_end_index {
                        return shaped_line.font_id_for_index(index - fragment_start_index);
                    }
                    fragment_start_index = fragment_end_index;
                }
                LineFragment::Element { len, .. } => {
                    let fragment_end_index = fragment_start_index + len;
                    if index < fragment_end_index {
                        return None;
                    }
                    fragment_start_index = fragment_end_index;
                }
            }
        }

        None
    }

    pub(crate) fn alignment_offset(&self, text_align: TextAlign, content_width: Pixels) -> Pixels {
        let line_width = self.width;
        match text_align {
            TextAlign::Left => px(0.0),
            TextAlign::Center => (content_width - line_width) / 2.0,
            TextAlign::Right => content_width - line_width,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Invisible {
    /// A tab character.
    ///
    /// A tab character is internally represented by spaces (configured by the user's tab width)
    /// aligned to the nearest column, so it's necessary to store the start and end offset for
    /// adjacency checks.
    Tab { line_start_offset: usize, line_end_offset: usize },
    /// A whitespace character (ASCII space or any other Unicode whitespace).
    ///
    /// Storing both offsets correctly accounts for multi-byte whitespace characters
    /// such as U+00A0 NO-BREAK SPACE, keeping adjacency checks correct.
    Whitespace { line_start_offset: usize, line_end_offset: usize },
}
