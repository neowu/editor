use std::cmp;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::BorderStyle;
use gpui::Bounds;
use gpui::Font;
use gpui::Hitbox;
use gpui::Hsla;
use gpui::InteractiveElement as _;
use gpui::IntoElement as _;
use gpui::ParentElement as _;
use gpui::Pixels;
use gpui::ShapedLine;
use gpui::Styled as _;
use gpui::TextAlign;
use gpui::TextRun;
use gpui::Window;
use gpui::div;
use gpui::fill;
use gpui::outline;
use gpui::point;
use gpui::px;
use gpui::size;
use multi_buffer::ToOffset as _;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use sum_tree::Bias;
use theme::Appearance;
use theme::CursorColors;
use unicode_segmentation::UnicodeSegmentation as _;
use util::ResultExt as _;

use super::EditorElement;
use super::EditorLayout;
use super::LineHighlightSpec;
use super::LineWithInvisibles;
use super::mark_active_rows;
use crate::CursorShape;
use crate::DisplayPoint;
use crate::DisplayRow;
use crate::EditorSnapshot;
use crate::Point;
use crate::RowExt as _;
use crate::Selection;
use crate::ToPoint;
use crate::cursor_motion::CursorViewport;
use crate::cursor_motion::LogicalCursorPosition;
use crate::cursor_motion::animated_corners_overlap_target;
use crate::display_map::DisplaySnapshot;
use crate::display_map::ToDisplayPoint;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollPixelOffset;

#[derive(Debug)]
pub(super) struct SelectionLayout {
    id: usize,
    pub(super) head: DisplayPoint,
    cursor_shape: CursorShape,
    is_newest: bool,
    pub(super) range: Range<DisplayPoint>,
    active_rows: Range<DisplayRow>,
}

impl SelectionLayout {
    pub(super) fn new<T: ToPoint + ToDisplayPoint + Clone>(
        selection: Selection<T>,
        line_mode: bool,
        cursor_offset: bool,
        cursor_shape: CursorShape,
        map: &DisplaySnapshot,
        is_newest: bool,
    ) -> Self {
        let id = selection.id;
        let buffer_snapshot = map.buffer_snapshot();
        let point_selection = selection.map(|p| p.to_point(buffer_snapshot));
        let display_selection = point_selection.map(|p| p.to_display_point(map));
        let mut range = display_selection.range();
        let mut head = display_selection.head();
        if !line_mode {
            let offset_range =
                point_selection.start.to_offset(buffer_snapshot)..point_selection.end.to_offset(buffer_snapshot);
            if let Some(contiguous_range) = map.contiguous_display_point_range_for_buffer_range(offset_range) {
                range = contiguous_range;
                // Keep the cursor attached to the highlight boundary; the
                // anchor-bias display position may sit on the far side of a
                // boundary inlay the highlight excludes.
                head = if selection.reversed { range.start } else { range.end };
            }
        }
        let mut active_rows =
            map.prev_line_boundary(point_selection.start).1.row()..map.next_line_boundary(point_selection.end).1.row();

        // vim visual line mode
        if line_mode {
            let point_range = map.expand_to_line(point_selection.range());
            range = point_range.start.to_display_point(map)..point_range.end.to_display_point(map);
        }

        // any vim visual mode (including line mode)
        if cursor_offset && !range.is_empty() && !selection.reversed {
            if head.column() > 0 {
                head = map.clip_point(DisplayPoint::new(head.row(), head.column() - 1), Bias::Left);
            } else if head.row().0 > 0 {
                head = map.clip_point(
                    DisplayPoint::new(head.row().previous_row(), map.line_len(head.row().previous_row())),
                    Bias::Left,
                );
                // updating range.end is a no-op unless you're cursor is
                // on the newline containing a multi-buffer divider
                // in which case the clip_point may have moved the head up
                // an additional row.
                range.end = DisplayPoint::new(head.row().next_row(), 0);
                active_rows.end = head.row();
            }
        }

        Self { id, head, cursor_shape, is_newest, range, active_rows }
    }
}

impl EditorElement {
    #[allow(clippy::type_complexity)]
    pub(super) fn layout_selections(
        &self,
        local_selections: &[Selection<Point>],
        snapshot: &EditorSnapshot,
        start_row: DisplayRow,
        end_row: DisplayRow,
        window: &Window,
        cx: &mut App,
    ) -> (Vec<(CursorColors, Vec<SelectionLayout>)>, BTreeMap<DisplayRow, LineHighlightSpec>, Option<DisplayPoint>)
    {
        let mut selections: Vec<(CursorColors, Vec<SelectionLayout>)> = Vec::new();
        let mut active_rows = BTreeMap::new();
        let mut newest_selection_head = None;

        let Some(editor_with_selections) = self.editor_with_selections(cx) else {
            return (selections, active_rows, newest_selection_head);
        };

        editor_with_selections.update(cx, |editor, cx| {
            if editor.show_local_selections {
                let mut layouts = Vec::new();
                let newest_id = editor.selections.newest_anchor().id;
                for selection in local_selections.iter().copied() {
                    let is_empty = selection.start == selection.end;
                    let is_newest = selection.id == newest_id;

                    let layout = SelectionLayout::new(
                        selection,
                        editor.selections.line_mode(),
                        editor.cursor_offset_on_selection,
                        editor.cursor_shape,
                        &snapshot.display_snapshot,
                        is_newest,
                    );
                    if is_newest {
                        newest_selection_head = Some(layout.head);
                    }

                    mark_active_rows(
                        &mut active_rows,
                        cmp::max(layout.active_rows.start.0, start_row.0)
                            ..=cmp::min(layout.active_rows.end.0, end_row.0),
                        !is_empty,
                        |row| snapshot.is_block_line(row),
                    );
                    layouts.push(layout);
                }

                let mut colors = editor.current_cursor_colors(cx);
                if !editor.is_focused(window) {
                    const UNFOCUS_EDITOR_SELECTION_OPACITY: f32 = 0.5;
                    colors.selection = colors.selection.opacity(UNFOCUS_EDITOR_SELECTION_OPACITY);
                }
                selections.push((colors, layouts));
            }
        });

        (selections, active_rows, newest_selection_head)
    }

    /// Cursor positions for the minimap markers, and whether any cursor is outside `visible_row_range`.
    pub(super) fn collect_cursors(
        &self,
        snapshot: &EditorSnapshot,
        visible_row_range: &Range<DisplayRow>,
        cx: &App,
    ) -> (Vec<(DisplayPoint, Hsla)>, bool) {
        let editor = self.editor.read(cx);
        let selections = editor.selections.disjoint_anchors();
        let pending = editor.selections.pending_anchor();
        let positions = selections
            .iter()
            .chain(pending)
            .map(|selection| selection.head().to_display_point(&snapshot.display_snapshot));
        let (markers, any_non_visible) =
            cursor_markers(positions, selections.len() + usize::from(pending.is_some()), visible_row_range);
        let color = theme::theme().cursor_colors().cursor;
        (markers.into_iter().map(|position| (position, color)).collect(), any_non_visible)
    }

    pub(super) fn layout_visible_cursors(
        &self,
        snapshot: &EditorSnapshot,
        selections: &[(CursorColors, Vec<SelectionLayout>)],
        row_block_types: &FxHashMap<DisplayRow, bool>,
        visible_display_row_range: Range<DisplayRow>,
        line_layouts: &[LineWithInvisibles],
        text_hitbox: &Hitbox,
        content_origin: gpui::Point<Pixels>,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        line_height: Pixels,
        em_width: Pixels,
        em_advance: Pixels,
        autoscroll_containing_element: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<CursorLayout> {
        let mut autoscroll_bounds = None;
        let cursor_layouts = self.editor.update(cx, |editor, cx| {
            let mut cursors = Vec::new();
            let mut handled_animation_cursors = FxHashSet::default();
            let mut request_animation_frame = false;

            let show_cursors = editor.show_cursors(window, cx);
            let cursor_viewport = CursorViewport::new(
                content_origin,
                text_hitbox.bounds,
                scroll_position,
                scroll_pixel_position,
                line_height,
                em_advance,
            );
            let animation_now = Instant::now();

            let newest_animation_selection_id =
                cursor_shape_supports_cursor_motion(editor.cursor_shape).then(|| editor.selections.newest_anchor().id);
            editor.cursor_motions.reconcile_newest_selection(newest_animation_selection_id);

            for (cursor_colors, selections) in selections {
                for selection in selections {
                    let cursor_position = selection.head;

                    let in_range = visible_display_row_range.contains(&cursor_position.row());
                    if !show_cursors || !in_range || row_block_types.get(&cursor_position.row()) == Some(&true) {
                        continue;
                    }

                    let cursor_row_layout =
                        &line_layouts[cursor_position.row().minus(visible_display_row_range.start) as usize];
                    let cursor_column = cursor_position.column() as usize;

                    let cursor_character_x = cursor_row_layout.x_for_index(cursor_column)
                        + cursor_row_layout.alignment_offset(self.style.text.text_align, text_hitbox.size.width);
                    let cursor_next_x = cursor_row_layout.x_for_index(cursor_column + 1)
                        + cursor_row_layout.alignment_offset(self.style.text.text_align, text_hitbox.size.width);
                    let mut cell_width = cursor_next_x - cursor_character_x;
                    if cell_width == Pixels::ZERO {
                        cell_width = em_advance;
                    }

                    let mut block_width = cell_width;
                    let block_text = if selection.cursor_shape == CursorShape::Block
                        && let Some(text) = snapshot.grapheme_at(cursor_position).or_else(|| {
                            if snapshot.is_empty() {
                                let s = snapshot.placeholder_text()?;
                                s.graphemes(true).next().map(|s| s.to_owned().into())
                            } else {
                                None
                            }
                        }) {
                        let is_ascii_whitespace_only = text.as_ref().chars().all(|c| c.is_ascii_whitespace());
                        let len = text.len();

                        let mut font = cursor_row_layout
                            .font_id_for_index(cursor_column)
                            .and_then(|cursor_font_id| window.text_system().get_font_for_id(cursor_font_id))
                            .unwrap_or_else(|| self.style.text.font());
                        font.features = self.style.text.font_features.clone();

                        // Invert the text color for the block cursor. Ensure that the text
                        // color is opaque enough to be visible against the background color.
                        //
                        // 0.75 is an arbitrary threshold to determine if the background color is
                        // opaque enough to use as a text color.
                        //
                        // TODO: In the future we should ensure themes have a `text_inverse` color.
                        let color = if theme::theme().colors().editor_background.a < 0.75 {
                            match theme::theme().appearance {
                                Appearance::Dark => Hsla::black(),
                                Appearance::Light => Hsla::white(),
                            }
                        } else {
                            theme::theme().colors().editor_background
                        };

                        let shaped = window.text_system().shape_line(
                            text,
                            cursor_row_layout.font_size,
                            &[TextRun { len, font, color, ..Default::default() }],
                            None,
                        );
                        if !is_ascii_whitespace_only {
                            block_width = block_width.max(shaped.width);
                        }
                        Some(shaped)
                    } else {
                        None
                    };

                    let x = cursor_character_x - scroll_pixel_position.x.into();
                    let y = ((cursor_position.row().as_f64() - scroll_position.y)
                        * ScrollPixelOffset::from(line_height))
                    .into();
                    if selection.is_newest {
                        editor.pixel_position_of_newest_cursor = Some(point(
                            text_hitbox.origin.x + x + block_width / 2.,
                            text_hitbox.origin.y + y + line_height / 2.,
                        ));

                        if autoscroll_containing_element {
                            let top = text_hitbox.origin.y
                                + ((cursor_position.row().as_f64() - scroll_position.y - 3.).max(0.)
                                    * ScrollPixelOffset::from(line_height))
                                .into();
                            let left = text_hitbox.origin.x
                                + ((cursor_position.column() as ScrollOffset - scroll_position.x - 3.).max(0.)
                                    * ScrollPixelOffset::from(em_width))
                                .into();

                            let bottom = text_hitbox.origin.y
                                + ((cursor_position.row().as_f64() - scroll_position.y + 4.)
                                    * ScrollPixelOffset::from(line_height))
                                .into();
                            let right = text_hitbox.origin.x
                                + ((cursor_position.column() as ScrollOffset - scroll_position.x + 4.)
                                    * ScrollPixelOffset::from(em_width))
                                .into();

                            autoscroll_bounds = Some(Bounds::from_corners(point(left, top), point(right, bottom)));
                        }
                    }

                    let mut cursor = CursorLayout {
                        color: cursor_colors.cursor,
                        block_width,
                        origin: point(x, y),
                        line_height,
                        shape: selection.cursor_shape,
                        block_text,
                        animated_corners: None,
                    };
                    if cursor_shape_supports_cursor_motion(selection.cursor_shape) {
                        handled_animation_cursors.insert(selection.id);
                        let target_bounds = window.pixel_snap_bounds(cursor.bounds(content_origin));
                        cursor.animated_corners = editor.cursor_motions.update(
                            selection.id,
                            LogicalCursorPosition { row: cursor_position.row().0, column: cursor_position.column() },
                            target_bounds,
                            cursor_viewport,
                            animation_now,
                        );
                        request_animation_frame |= cursor.animated_corners.is_some();
                    } else {
                        editor.cursor_motions.remove(selection.id);
                    }
                    cursors.push(cursor);
                }
            }

            editor.cursor_motions.capture_newest_state();
            editor.cursor_motions.retain(|selection_id| handled_animation_cursors.contains(&selection_id));
            if request_animation_frame {
                window.request_animation_frame();
            }

            cursors
        });

        if let Some(bounds) = autoscroll_bounds {
            window.request_autoscroll(bounds);
        }

        cursor_layouts
    }

    pub(super) fn layout_navigation_overlays(
        &self,
        snapshot: &EditorSnapshot,
        visible_display_row_range: Range<DisplayRow>,
        line_layouts: &[LineWithInvisibles],
        text_hitbox: &Hitbox,
        content_origin: gpui::Point<Pixels>,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let mut overlay_sets = self
            .editor
            .read(cx)
            .navigation_overlay_sets()
            .iter()
            .map(|(key, overlays)| (*key, Arc::clone(overlays)))
            .collect::<Vec<_>>();
        if overlay_sets.is_empty() {
            return Vec::new();
        }
        overlay_sets.sort_by_key(|(key, _)| *key);

        let layout_context = NavigationOverlayLayoutContext {
            display_snapshot: &snapshot.display_snapshot,
            visible_display_row_range: &visible_display_row_range,
            line_layouts,
            text_align: self.style.text.text_align,
            content_width: text_hitbox.size.width,
            content_origin,
            scroll_position,
            scroll_pixel_position,
            line_height,
            editor_font: self.style.text.font(),
            editor_font_size: self.style.text.font_size.to_pixels(window.rem_size()),
        };
        let mut navigation_overlay_paint_commands = Vec::new();

        for (_, overlays) in overlay_sets {
            for overlay in overlays.as_ref() {
                Self::layout_navigation_label(
                    overlay,
                    &layout_context,
                    window,
                    cx,
                    &mut navigation_overlay_paint_commands,
                );
            }
        }

        navigation_overlay_paint_commands
    }

    fn layout_navigation_label(
        overlay: &crate::NavigationTargetOverlay,
        context: &NavigationOverlayLayoutContext<'_>,
        window: &mut Window,
        cx: &mut App,
        paint_commands: &mut Vec<AnyElement>,
    ) {
        let label = &overlay.label;
        let label_display_point = overlay.target_range.start.to_display_point(context.display_snapshot);
        let label_row = label_display_point.row();
        if !context.visible_display_row_range.contains(&label_row) {
            return;
        }

        let row_index = label_row.minus(context.visible_display_row_range.start) as usize;
        let row_layout = &context.line_layouts[row_index];
        let label_column = label_display_point.column().min(row_layout.len as u32) as usize;
        let label_x = row_layout.x_for_index(label_column)
            + row_layout.alignment_offset(context.text_align, context.content_width)
            - context.scroll_pixel_position.x.into()
            + label.x_offset;
        let label_y =
            ((label_row.as_f64() - context.scroll_position.y) * ScrollPixelOffset::from(context.line_height)).into();
        let label_text_size = (context.editor_font_size * label.scale_factor.max(0.0)).max(px(1.0));
        let origin = context.content_origin + point(label_x, label_y);

        let mut element = div()
            .block_mouse_except_scroll()
            .font(context.editor_font.clone())
            .text_size(label_text_size)
            .text_color(label.text_color)
            .line_height(context.line_height)
            .child(label.text.clone())
            .into_any_element();
        element.prepaint_as_root(origin, AvailableSpace::min_size(), window, cx);

        paint_commands.push(element);
    }

    pub(super) fn paint_navigation_overlays(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        window.with_element_namespace("navigation_overlays", |window| {
            for element in &mut layout.navigation_overlay_paint_commands {
                element.paint(window, cx);
            }
        });
    }

    pub(super) fn paint_cursors(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        for cursor in &mut layout.visible_cursors {
            cursor.paint(layout.content_origin, window, cx);
        }
    }
}

pub(super) struct NavigationOverlayLayoutContext<'a> {
    display_snapshot: &'a DisplaySnapshot,
    visible_display_row_range: &'a Range<DisplayRow>,
    line_layouts: &'a [LineWithInvisibles],
    text_align: TextAlign,
    content_width: Pixels,
    content_origin: gpui::Point<Pixels>,
    scroll_position: gpui::Point<ScrollOffset>,
    scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
    line_height: Pixels,
    editor_font: Font,
    editor_font_size: Pixels,
}

pub struct CursorLayout {
    origin: gpui::Point<Pixels>,
    block_width: Pixels,
    line_height: Pixels,
    color: Hsla,
    shape: CursorShape,
    block_text: Option<ShapedLine>,
    animated_corners: Option<[gpui::Point<Pixels>; 4]>,
}

impl CursorLayout {
    pub const fn new(
        origin: gpui::Point<Pixels>,
        block_width: Pixels,
        line_height: Pixels,
        color: Hsla,
        shape: CursorShape,
        block_text: Option<ShapedLine>,
    ) -> CursorLayout {
        CursorLayout { origin, block_width, line_height, color, shape, block_text, animated_corners: None }
    }

    fn bounds(&self, origin: gpui::Point<Pixels>) -> Bounds<Pixels> {
        match self.shape {
            CursorShape::Bar => Bounds { origin: self.origin + origin, size: size(px(2.0), self.line_height) },
            CursorShape::Block | CursorShape::Hollow => {
                Bounds { origin: self.origin + origin, size: size(self.block_width, self.line_height) }
            }
            CursorShape::Underline => Bounds {
                origin: self.origin + origin + gpui::Point::new(Pixels::ZERO, self.line_height - px(2.0)),
                size: size(self.block_width, px(2.0)),
            },
        }
    }

    pub fn paint(&mut self, origin: gpui::Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = window.pixel_snap_bounds(self.bounds(origin));

        if let Some(corners) = self.animated_corners {
            let mut builder = gpui::PathBuilder::fill();
            builder.add_polygon(&corners, true);
            if let Ok(path) = builder.build() {
                window.paint_path(path, self.color);

                if !animated_corners_overlap_target(bounds, &corners) {
                    return;
                }
            }
        } else {
            //Draw background or border quad
            let cursor = if matches!(self.shape, CursorShape::Hollow) {
                outline(bounds, self.color, BorderStyle::Solid)
            } else {
                fill(bounds, self.color)
            };

            window.paint_quad(cursor);
        }

        if let Some(block_text) = &self.block_text {
            block_text.paint(self.origin + origin, self.line_height, TextAlign::Left, None, window, cx).log_err();
        }
    }

    pub const fn shape(&self) -> CursorShape {
        self.shape
    }
}

pub(super) const fn cursor_shape_supports_cursor_motion(shape: CursorShape) -> bool {
    matches!(shape, CursorShape::Bar | CursorShape::Block)
}

/// No markers are drawn past `MARKER_LIMIT` cursors, so positions are then resolved only until one is
/// found out of sight.
fn cursor_markers(
    mut positions: impl Iterator<Item = DisplayPoint>,
    cursor_count: usize,
    visible_row_range: &Range<DisplayRow>,
) -> (Vec<DisplayPoint>, bool) {
    const MARKER_LIMIT: usize = 100;
    if cursor_count > MARKER_LIMIT {
        (Vec::new(), positions.any(|position| !visible_row_range.contains(&position.row())))
    } else {
        let markers = positions.collect::<Vec<_>>();
        let any_non_visible = markers.iter().any(|position| !visible_row_range.contains(&position.row()));
        (markers, any_non_visible)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn cursor_markers_stop_resolving_past_the_limit() {
        let resolved = Cell::new(0);
        let markers = |cursor_count: u32, visible_rows: Range<u32>| {
            resolved.set(0);
            let positions = (0..cursor_count).map(|row| {
                resolved.set(resolved.get() + 1);
                DisplayPoint::new(DisplayRow(row), 0)
            });
            cursor_markers(
                positions,
                cursor_count as usize,
                &(DisplayRow(visible_rows.start)..DisplayRow(visible_rows.end)),
            )
        };

        let (positions, any_non_visible) = markers(100, 0..10);
        assert_eq!(positions.len(), 100);
        assert!(any_non_visible);
        assert!(!markers(100, 0..200).1);

        let (positions, any_non_visible) = markers(101, 0..10);
        assert!(positions.is_empty());
        assert!(any_non_visible);
        assert_eq!(resolved.get(), 11);
        assert!(!markers(101, 0..200).1);
    }
}
