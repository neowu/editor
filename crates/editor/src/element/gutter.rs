use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::ops::RangeInclusive;
use std::sync::Arc;

use buffer_diff::DiffHunkStatus;
use buffer_diff::DiffHunkStatusKind;
use git::Oid;
use git::blame::BlameEntry;
use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::BorderStyle;
use gpui::Bounds;
use gpui::Context;
use gpui::Corners;
use gpui::CursorStyle;
use gpui::Edges;
use gpui::Entity;
use gpui::Hitbox;
use gpui::HitboxBehavior;
use gpui::Hsla;
use gpui::IntoElement as _;
use gpui::Pixels;
use gpui::ShapedLine;
use gpui::SharedString;
use gpui::TextAlign;
use gpui::TextRun;
use gpui::Window;
use gpui::fill;
use gpui::point;
use gpui::px;
use gpui::quad;
use gpui::size;
use gpui::transparent_black;
use itertools::Itertools as _;
use multi_buffer::ExpandExcerptDirection;
use multi_buffer::ExpandInfo;
use multi_buffer::MultiBufferRow;
use multi_buffer::RowInfo;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use text::BufferId;
use ui::Tooltip;
use ui::prelude::ButtonCommon as _;
use ui::prelude::Clickable as _;
use ui::prelude::Color;
use ui::prelude::FixedWidth as _;
use ui::prelude::IconButton;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::rems;
use util::ResultExt as _;
use workspace::Workspace;
use workspace::item::Item as _;
use workspace::item::ItemBufferKind;

use super::EditorElement;
use super::EditorLayout;
use super::LineHighlightSpec;
use super::LineNumberLayout;
use super::LineNumberSegment;
use super::LineNumberStyle;
use super::SplitSide;
use crate::CodeActionSource;
use crate::DisplayDiffHunk;
use crate::DisplayPoint;
use crate::DisplayRow;
use crate::Editor;
use crate::EditorSnapshot;
use crate::EditorStyle;
use crate::GutterDimensions;
use crate::MIN_LINE_NUMBER_DIGITS;
use crate::RowExt as _;
use crate::actions::ExpandExcerpts;
use crate::code_context_menus::CodeActionsMenu;
use crate::display_map::Block;
use crate::display_map::ToDisplayPoint as _;
use crate::git::blame::BlameRenderer;
use crate::git::blame::GitBlame;
use crate::git::blame::GlobalBlameRenderer;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollPixelOffset;

impl EditorElement {
    pub(super) fn prepaint_crease_toggles(
        &self,
        crease_toggles: &mut [Option<AnyElement>],
        line_height: Pixels,
        gutter_dimensions: &GutterDimensions,
        scroll_position: gpui::Point<ScrollOffset>,
        start_row: DisplayRow,
        gutter_hitbox: &Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (ix, crease_toggle) in crease_toggles.iter_mut().enumerate() {
            if let Some(crease_toggle) = crease_toggle {
                let available_space = size(AvailableSpace::MinContent, AvailableSpace::Definite(line_height * 0.55));
                let crease_toggle_size = crease_toggle.layout_as_root(available_space, window, cx);

                let display_row = DisplayRow(start_row.0 + ix as u32);
                let position = point(
                    gutter_dimensions.width - gutter_dimensions.right_padding,
                    line_height * (display_row.as_f64() - scroll_position.y) as f32,
                );
                let centering_offset = point(
                    (gutter_dimensions.fold_area_width() - crease_toggle_size.width) / 2.,
                    (line_height - crease_toggle_size.height) / 2.,
                );
                let origin = gutter_hitbox.origin + position + centering_offset;
                crease_toggle.prepaint_as_root(origin, available_space, window, cx);
            }
        }
    }

    pub(super) fn prepaint_expand_toggles(
        &self,
        expand_toggles: &mut [Option<(AnyElement, gpui::Point<Pixels>)>],
        window: &mut Window,
        cx: &mut App,
    ) {
        for (expand_toggle, origin) in expand_toggles.iter_mut().flatten() {
            let available_space = size(AvailableSpace::MinContent, AvailableSpace::MinContent);
            expand_toggle.layout_as_root(available_space, window, cx);
            expand_toggle.prepaint_as_root(*origin, available_space, window, cx);
        }
    }

    // Folds contained in a hunk are ignored apart from shrinking visual size
    // If a fold contains any hunks then that fold line is marked as modified
    pub(super) fn layout_gutter_diff_hunks(
        &self,
        line_height: Pixels,
        gutter_hitbox: &Hitbox,
        display_rows: Range<DisplayRow>,
        snapshot: &EditorSnapshot,
        scroll_position: gpui::Point<ScrollOffset>,
        window: &mut Window,
        cx: &App,
    ) -> Vec<(DisplayDiffHunk, Option<Hitbox>)> {
        let folded_buffers = self.editor.read(cx).folded_buffers(cx);
        let mut display_hunks = snapshot
            .display_diff_hunks_for_rows(display_rows, folded_buffers)
            .map(|hunk| (hunk, None))
            .collect::<Vec<_>>();
        for (hunk, hitbox) in &mut display_hunks {
            if matches!(hunk, DisplayDiffHunk::Unfolded { .. }) {
                let hunk_bounds =
                    Self::diff_hunk_bounds(scroll_position, line_height, gutter_hitbox.bounds, hunk, snapshot);
                *hitbox = Some(window.insert_hitbox(hunk_bounds, HitboxBehavior::BlockMouse));
            }
        }

        display_hunks
    }

    pub(super) fn layout_blame_entries(
        &self,
        buffer_rows: &[RowInfo],
        em_width: Pixels,
        scroll_position: gpui::Point<ScrollOffset>,
        start_row: DisplayRow,
        line_height: Pixels,
        gutter_hitbox: &Hitbox,
        max_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Vec<AnyElement>> {
        if !self.editor.update(cx, |editor, cx| editor.render_git_blame_gutter(cx)) {
            return None;
        }

        let blame = self.editor.read(cx).blame.clone()?;
        let workspace = self.editor.read(cx).workspace()?;
        let blamed_rows: Vec<_> = blame.update(cx, |blame, cx| blame.blame_for_rows(buffer_rows, cx).collect());

        let width = if let Some(max_width) = max_width {
            AvailableSpace::Definite(max_width)
        } else {
            AvailableSpace::MaxContent
        };
        let start_x = em_width;

        let mut last_used_color: Option<(Hsla, Oid)> = None;
        let blame_renderer = Arc::clone(&cx.global::<GlobalBlameRenderer>().0);

        let shaped_lines = blamed_rows
            .into_iter()
            .enumerate()
            .filter_map(|(ix, blame_entry)| {
                let (buffer_id, blame_entry) = blame_entry?;
                let mut element = render_blame_entry(
                    ix,
                    &blame,
                    blame_entry,
                    &self.style,
                    &mut last_used_color,
                    self.editor.clone(),
                    workspace.clone(),
                    buffer_id,
                    &*blame_renderer,
                    window,
                    cx,
                )?;

                let start_y = line_height * (DisplayRow(start_row.0 + ix as u32).as_f64() - scroll_position.y) as f32;
                let absolute_offset = gutter_hitbox.origin + point(start_x, start_y);

                element.prepaint_as_root(absolute_offset, size(width, AvailableSpace::MinContent), window, cx);

                Some(element)
            })
            .collect();

        Some(shaped_lines)
    }

    pub(super) fn layout_gutter_hover_button(
        &self,
        gutter: &Gutter,
        row: DisplayRow,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if self.split_side == Some(SplitSide::Left) {
            return None;
        }

        self.editor.update(cx, |editor, cx| {
            gutter.layout_item_skipping_folds(
                row,
                |cx, _| editor.render_gutter_hover_button(row, cx).into_any_element(),
                window,
                cx,
            )
        })
    }

    pub(super) fn layout_run_indicators(
        &self,
        gutter: &Gutter,
        run_indicators: &FxHashSet<DisplayRow>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        if self.split_side == Some(SplitSide::Left) {
            return Vec::new();
        }

        self.editor.update(cx, |editor, cx| {
            let active_task_indicator_row =
                // TODO: add edit button on the right side of each row in the context menu
                if let Some(crate::CodeContextMenu::CodeActions(CodeActionsMenu {
                    deployed_from,
                    actions,
                    ..
                })) = editor.context_menu.borrow().as_ref()
                {
                    actions
                        .tasks()
                        .map(|tasks| tasks.position.to_display_point(gutter.snapshot).row())
                        .or(match deployed_from {
                            Some(CodeActionSource::Indicator(row)) => Some(*row),
                            _ => None,
                        })
                } else {
                    None
                };

            run_indicators
                .iter()
                .filter_map(|display_row| {
                    let task_status = gutter
                        .row_infos
                        .get((display_row.0.saturating_sub(gutter.range.start.0)) as usize)
                        .and_then(|row_info| Some((row_info.buffer_id?, row_info.buffer_row?)))
                        .and_then(|(buffer_id, buffer_row)| editor.runnable_task_status(buffer_id, buffer_row));

                    gutter.layout_item(
                        *display_row,
                        |cx, _| {
                            editor
                                .render_run_indicator(
                                    &self.style,
                                    Some(*display_row) == active_task_indicator_row,
                                    task_status,
                                    *display_row,
                                    cx,
                                )
                                .into_any_element()
                        },
                        window,
                        cx,
                    )
                })
                .collect_vec()
        })
    }

    pub(super) fn layout_expand_toggles(
        &self,
        gutter_hitbox: &Hitbox,
        gutter_dimensions: GutterDimensions,
        em_width: Pixels,
        line_height: Pixels,
        scroll_position: gpui::Point<ScrollOffset>,
        start_row: DisplayRow,
        buffer_rows: &[RowInfo],
        snapshot: &EditorSnapshot,
        window: &Window,
        cx: &App,
    ) -> Vec<Option<(AnyElement, gpui::Point<Pixels>)>> {
        if self.editor.read(cx).disable_expand_excerpt_buttons || !has_expand_toggles(buffer_rows) {
            return vec![];
        }

        let editor_font_size = self.style.text.font_size.to_pixels(window.rem_size()) * 1.2;

        let max_line_number_length = snapshot.widest_line_number().ilog10() + 1;

        let git_gutter_width =
            Self::gutter_strip_width(line_height) + gutter_dimensions.git_blame_entries_width.unwrap_or_default();
        let available_width = gutter_dimensions.left_padding - git_gutter_width;

        buffer_rows
            .iter()
            .enumerate()
            .map(|(ix, row_info)| {
                let ExpandInfo { direction, start_anchor } = row_info.expand_info?;

                let icon_name = match direction {
                    ExpandExcerptDirection::Up => IconName::ExpandUp,
                    ExpandExcerptDirection::Down => IconName::ExpandDown,
                    ExpandExcerptDirection::UpAndDown => IconName::ExpandVertical,
                };

                let editor = self.editor.clone();
                let is_wide = max_line_number_length >= MIN_LINE_NUMBER_DIGITS as u32
                    && row_info.buffer_row.is_some_and(|row| (row + 1).ilog10() + 1 == max_line_number_length)
                    || gutter_dimensions.right_padding == px(0.);

                let width = if is_wide { available_width - px(5.) } else { available_width + em_width - px(5.) };

                let toggle = IconButton::new(("expand", ix), icon_name)
                    .icon_color(Color::Custom(theme::theme().colors().editor_line_number))
                    .hover_color(Color::Default)
                    .icon_size(IconSize::Custom(rems(editor_font_size / window.rem_size())))
                    .width(width)
                    .on_click(move |_, window, cx| {
                        editor.update(cx, |editor, cx| {
                            editor.expand_excerpt(start_anchor, direction, window, cx);
                        });
                    })
                    .tooltip(Tooltip::for_action_title("Expand Excerpt", &ExpandExcerpts::default()))
                    .into_any_element();

                let position = point(
                    git_gutter_width + px(1.),
                    line_height * (DisplayRow(start_row.0 + ix as u32).as_f64() - scroll_position.y) as f32 + px(1.),
                );
                let origin = gutter_hitbox.origin + position;

                Some((toggle, origin))
            })
            .collect()
    }

    pub(super) fn layout_line_numbers(
        &self,
        gutter: &Gutter<'_>,
        active_rows: &BTreeMap<DisplayRow, LineHighlightSpec>,
        window: &mut Window,
    ) -> Arc<FxHashMap<MultiBufferRow, LineNumberLayout>> {
        let include_line_numbers = gutter.snapshot.show_line_numbers;
        if !include_line_numbers {
            return Arc::default();
        }

        let mut line_number = String::new();
        let segments = gutter.row_infos.iter().enumerate().filter_map(|(ix, row_info)| {
            let display_row = DisplayRow(gutter.range.start.0 + ix as u32);
            line_number.clear();
            let number = row_info.buffer_row? + 1;
            let multi_buffer_row = row_info.multibuffer_row?;
            if !gutter.snapshot.number_deleted_lines && row_info.diff_status.is_some_and(|status| status.is_deleted()) {
                return None;
            }

            write!(&mut line_number, "{number}").expect("writing to a String can't fail");

            let spec = active_rows.get(&display_row);
            let color = LineNumberStyle::new(spec.is_some(), row_info.diff_status).color(theme::theme().colors());

            let shaped_line = self.shape_line_number(SharedString::from(&line_number), color, window);
            let scroll_top = gutter.scroll_position.y * ScrollPixelOffset::from(gutter.line_height);
            #[expect(clippy::modulo_arithmetic, reason = "scroll position and line height are non-negative")]
            let line_origin = gutter.hitbox.origin
                + point(
                    gutter.hitbox.size.width - shaped_line.width - gutter.dimensions.right_padding,
                    ix as f32 * gutter.line_height
                        - Pixels::from(scroll_top % ScrollPixelOffset::from(gutter.line_height)),
                );

            let hitbox = Some(window.insert_hitbox(
                Bounds::new(line_origin, size(shaped_line.width, gutter.line_height)),
                HitboxBehavior::Normal,
            ));

            let segment = LineNumberSegment { shaped_line, hitbox };

            Some((multi_buffer_row, segment))
        });

        let mut line_numbers: FxHashMap<MultiBufferRow, LineNumberLayout> = FxHashMap::default();
        for (buffer_row, segment) in segments {
            line_numbers
                .entry(buffer_row)
                .or_insert_with(|| LineNumberLayout { segments: SmallVec::default() })
                .segments
                .push(segment);
        }
        Arc::new(line_numbers)
    }

    pub(super) fn layout_crease_toggles(
        &self,
        rows: Range<DisplayRow>,
        row_infos: &[RowInfo],
        active_rows: &BTreeMap<DisplayRow, LineHighlightSpec>,
        snapshot: &EditorSnapshot,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Option<AnyElement>> {
        let include_fold_statuses = snapshot.mode.is_full()
            && snapshot.display_snapshot.companion_snapshot().is_none()
            && self.editor.read(cx).buffer_kind(cx) == ItemBufferKind::Singleton;
        if include_fold_statuses {
            row_infos
                .iter()
                .enumerate()
                .map(|(ix, info)| {
                    if info.expand_info.is_some() {
                        return None;
                    }
                    let row = info.multibuffer_row?;
                    let display_row = DisplayRow(rows.start.0 + ix as u32);
                    let active = active_rows.contains_key(&display_row);

                    snapshot.render_crease_toggle(row, active, self.editor.clone(), window, cx)
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    pub(super) fn paint_line_numbers(&self, layout: &EditorLayout, window: &mut Window, cx: &mut App) {
        let is_singleton = self.editor.read(cx).buffer_kind(cx) == ItemBufferKind::Singleton;

        let line_height = layout.position_map.line_height;
        window.set_cursor_style(CursorStyle::Arrow, &layout.gutter_hitbox);

        for line_layout in layout.line_numbers.values() {
            for LineNumberSegment { shaped_line, hitbox } in &line_layout.segments {
                let Some(hitbox) = hitbox else {
                    continue;
                };

                let Some(()) = (if !is_singleton && hitbox.is_hovered(window) {
                    let color = theme::theme().colors().editor_hover_line_number;

                    let line = self.shape_line_number(shaped_line.text.clone(), color, window);
                    line.paint(hitbox.origin, line_height, TextAlign::Left, None, window, cx).log_err()
                } else {
                    shaped_line.paint(hitbox.origin, line_height, TextAlign::Left, None, window, cx).log_err()
                }) else {
                    continue;
                };

                // In singleton buffers, we select corresponding lines on the line number click, so use | -like cursor.
                // In multi buffers, we open file at the line number clicked, so use a pointing hand cursor.
                if is_singleton {
                    window.set_cursor_style(CursorStyle::IBeam, hitbox);
                } else {
                    window.set_cursor_style(CursorStyle::PointingHand, hitbox);
                }
            }
        }
    }

    fn paint_gutter_diff_hunks(
        &self,
        layout: &EditorLayout,
        split_side: Option<SplitSide>,
        window: &mut Window,
        cx: &App,
    ) {
        if layout.display_hunks.is_empty() {
            return;
        }

        let line_height = layout.position_map.line_height;
        window.paint_layer(layout.gutter_hitbox.bounds, |window| {
            for (hunk, hitbox) in &layout.display_hunks {
                let hunk_to_paint = match hunk {
                    DisplayDiffHunk::Folded { .. } => {
                        let hunk_bounds = Self::diff_hunk_bounds(
                            layout.position_map.scroll_position,
                            line_height,
                            layout.gutter_hitbox.bounds,
                            hunk,
                            &layout.position_map.snapshot,
                        );
                        Some((
                            hunk_bounds,
                            theme::theme().colors().version_control_modified,
                            Corners::all(px(0.)),
                            DiffHunkStatus::modified_none(),
                        ))
                    }
                    DisplayDiffHunk::Unfolded { status, display_row_range, .. } => hitbox.as_ref().map(|hunk_hitbox| {
                        let color = match split_side {
                            Some(SplitSide::Left) => theme::theme().colors().version_control_deleted,
                            Some(SplitSide::Right) => theme::theme().colors().version_control_added,
                            None => match status.kind {
                                DiffHunkStatusKind::Added => theme::theme().colors().version_control_added,
                                DiffHunkStatusKind::Modified => theme::theme().colors().version_control_modified,
                                DiffHunkStatusKind::Deleted => theme::theme().colors().version_control_deleted,
                            },
                        };
                        match status.kind {
                            DiffHunkStatusKind::Deleted if display_row_range.is_empty() => (
                                Bounds::new(
                                    point(hunk_hitbox.origin.x - hunk_hitbox.size.width, hunk_hitbox.origin.y),
                                    size(hunk_hitbox.size.width * 2., hunk_hitbox.size.height),
                                ),
                                color,
                                Corners::all(1. * line_height),
                                *status,
                            ),
                            // lint: expect(wildcard_match) only some variants matter here
                            _ => (hunk_hitbox.bounds, color, Corners::all(px(0.)), *status),
                        }
                    }),
                };

                if let Some((hunk_bounds, background_color, corner_radii, status)) = hunk_to_paint {
                    // Flatten the background color with the editor color to prevent
                    // elements below transparent hunks from showing through
                    let flattened_background_color = theme::theme().colors().editor_background.blend(background_color);

                    if self.diff_hunk_hollow(status, cx) {
                        let flattened_unstaged_background_color =
                            theme::theme().colors().editor_background.blend(background_color.opacity(0.3));

                        window.paint_quad(quad(
                            hunk_bounds,
                            corner_radii,
                            flattened_unstaged_background_color,
                            Edges::all(px(1.0)),
                            flattened_background_color,
                            BorderStyle::Solid,
                        ));
                    } else {
                        window.paint_quad(quad(
                            hunk_bounds,
                            corner_radii,
                            flattened_background_color,
                            Edges::default(),
                            transparent_black(),
                            BorderStyle::default(),
                        ));
                    }
                }
            }
        });
    }

    const DEFAULT_STRIP_WIDTH_RATIO: f32 = 0.275;
    const DELETED_MARKER_WIDTH_RATIO: f32 = 0.35 / Self::DEFAULT_STRIP_WIDTH_RATIO;

    fn gutter_strip_width(line_height: Pixels) -> Pixels {
        (Self::DEFAULT_STRIP_WIDTH_RATIO * line_height).floor()
    }

    fn deleted_marker_base_width(line_height: Pixels) -> Pixels {
        (Self::DEFAULT_STRIP_WIDTH_RATIO * line_height * Self::DELETED_MARKER_WIDTH_RATIO).floor()
    }

    fn diff_hunk_bounds(
        scroll_position: gpui::Point<ScrollOffset>,
        line_height: Pixels,
        gutter_bounds: Bounds<Pixels>,
        hunk: &DisplayDiffHunk,
        snapshot: &EditorSnapshot,
    ) -> Bounds<Pixels> {
        let scroll_top = scroll_position.y * ScrollPixelOffset::from(line_height);
        let gutter_strip_width = Self::gutter_strip_width(line_height);

        match hunk {
            DisplayDiffHunk::Folded { display_row, .. } => {
                let start_y = (display_row.as_f64() * ScrollPixelOffset::from(line_height) - scroll_top).into();
                let end_y = start_y + line_height;
                let highlight_origin = gutter_bounds.origin + point(px(0.), start_y);
                let highlight_size = size(gutter_strip_width, end_y - start_y);
                Bounds::new(highlight_origin, highlight_size)
            }
            DisplayDiffHunk::Unfolded { display_row_range, status, .. } => {
                if status.is_deleted() && display_row_range.is_empty() {
                    let row = display_row_range.start;

                    let offset = ScrollPixelOffset::from(line_height / 2.);
                    let start_y = (row.as_f64() * ScrollPixelOffset::from(line_height) - offset - scroll_top).into();
                    let end_y = start_y + line_height;

                    let width = Self::deleted_marker_base_width(line_height);
                    let highlight_origin = gutter_bounds.origin + point(px(0.), start_y);
                    let highlight_size = size(width, end_y - start_y);
                    Bounds::new(highlight_origin, highlight_size)
                } else {
                    let start_row = display_row_range.start;
                    let end_row = display_row_range.end;
                    // If we're in a multibuffer, row range span might include an
                    // excerpt header, so if we were to draw the marker straight away,
                    // the hunk might include the rows of that header.
                    // Making the range inclusive doesn't quite cut it, as we rely on the exclusivity for the soft wrap.
                    // Instead, we simply check whether the range we're dealing with includes
                    // any excerpt headers and if so, we stop painting the diff hunk on the first row of that header.
                    let end_row_in_current_excerpt = snapshot
                        .blocks_in_range(start_row..end_row)
                        .find_map(|(start_row, block)| {
                            matches!(block, Block::ExcerptBoundary { .. } | Block::BufferHeader { .. })
                                .then_some(start_row)
                        })
                        .unwrap_or(end_row);

                    let start_y = (start_row.as_f64() * ScrollPixelOffset::from(line_height) - scroll_top).into();
                    let end_y = Pixels::from(
                        end_row_in_current_excerpt.as_f64() * ScrollPixelOffset::from(line_height) - scroll_top,
                    );

                    let highlight_origin = gutter_bounds.origin + point(px(0.), start_y);
                    let highlight_size = size(gutter_strip_width, end_y - start_y);
                    Bounds::new(highlight_origin, highlight_size)
                }
            }
        }
    }

    pub(super) fn paint_gutter_indicators(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        window.paint_layer(layout.gutter_hitbox.bounds, |window| {
            window.with_element_namespace("crease_toggles", |window| {
                for crease_toggle in layout.crease_toggles.iter_mut().flatten() {
                    crease_toggle.paint(window, cx);
                }
            });

            window.with_element_namespace("expand_toggles", |window| {
                for (expand_toggle, _) in layout.expand_toggles.iter_mut().flatten() {
                    expand_toggle.paint(window, cx);
                }
            });

            for test_indicator in &mut layout.test_indicators {
                test_indicator.paint(window, cx);
            }

            for gutter_hover_button in &mut layout.gutter_hover_buttons {
                gutter_hover_button.paint(window, cx);
            }
        });
    }

    pub(super) fn paint_gutter_highlights(&self, layout: &EditorLayout, window: &mut Window, cx: &App) {
        for (_, hunk_hitbox) in &layout.display_hunks {
            if let Some(hunk_hitbox) = hunk_hitbox
                && !self.editor.read(cx).buffer().read(cx).all_diff_hunks_expanded()
            {
                window.set_cursor_style(CursorStyle::PointingHand, hunk_hitbox);
            }
        }

        if layout.position_map.snapshot.show_git_diff_gutter.unwrap_or(true) {
            self.paint_gutter_diff_hunks(layout, self.split_side, window, cx);
        }

        let highlight_width = 0.275 * layout.position_map.line_height;
        let highlight_corner_radii = Corners::all(0.05 * layout.position_map.line_height);
        window.paint_layer(layout.gutter_hitbox.bounds, |window| {
            for (range, color) in &layout.highlighted_gutter_ranges {
                let start_row = if range.start.row() < layout.visible_display_row_range.start {
                    layout.visible_display_row_range.start - DisplayRow(1)
                } else {
                    range.start.row()
                };
                let end_row = if range.end.row() > layout.visible_display_row_range.end {
                    layout.visible_display_row_range.end + DisplayRow(1)
                } else {
                    range.end.row()
                };

                let start_y = layout.gutter_hitbox.top()
                    + Pixels::from(
                        start_row.0 as f64 * ScrollPixelOffset::from(layout.position_map.line_height)
                            - layout.position_map.scroll_pixel_position.y,
                    );
                let end_y = layout.gutter_hitbox.top()
                    + Pixels::from(
                        (end_row.0 + 1) as f64 * ScrollPixelOffset::from(layout.position_map.line_height)
                            - layout.position_map.scroll_pixel_position.y,
                    );
                let bounds = Bounds::from_corners(
                    point(layout.gutter_hitbox.left(), start_y),
                    point(layout.gutter_hitbox.left() + highlight_width, end_y),
                );
                window.paint_quad(fill(bounds, *color).corner_radii(highlight_corner_radii));
            }
        });
    }

    pub(super) fn paint_blamed_display_rows(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        let Some(blamed_display_rows) = layout.blamed_display_rows.take() else {
            return;
        };

        window.paint_layer(layout.gutter_hitbox.bounds, |window| {
            for mut blame_element in blamed_display_rows {
                blame_element.paint(window, cx);
            }
        });
    }

    fn shape_line_number(&self, text: SharedString, color: Hsla, window: &Window) -> ShapedLine {
        let run = TextRun { len: text.len(), font: self.style.text.font(), color, ..Default::default() };
        window.text_system().shape_line(text, self.style.text.font_size.to_pixels(window.rem_size()), &[run], None)
    }

    pub(super) fn diff_hunk_hollow(&self, status: DiffHunkStatus, cx: &App) -> bool {
        // Unstaged hunks are filled, staged hunks are hollow.
        self.editor.read(cx).diff_hunk_renderer().render_hunk_as_staged(&status, cx)
    }
}

fn has_expand_toggles(row_infos: &[RowInfo]) -> bool {
    row_infos.iter().any(|row_info| row_info.expand_info.is_some())
}

pub(super) struct Gutter<'a> {
    pub(super) line_height: Pixels,
    pub(super) range: Range<DisplayRow>,
    pub(super) scroll_position: gpui::Point<ScrollOffset>,
    pub(super) dimensions: &'a GutterDimensions,
    pub(super) hitbox: &'a Hitbox,
    pub(super) snapshot: &'a EditorSnapshot,
    pub(super) row_infos: &'a [RowInfo],
}

impl Gutter<'_> {
    fn layout_item_skipping_folds(
        &self,
        display_row: DisplayRow,
        render_item: impl Fn(&mut Context<'_, Editor>, &mut Window) -> AnyElement,
        window: &mut Window,
        cx: &mut Context<'_, Editor>,
    ) -> Option<AnyElement> {
        let row = MultiBufferRow(DisplayPoint::new(display_row, 0).to_point(self.snapshot).row);
        if self.snapshot.is_line_folded(row) {
            return None;
        }

        self.layout_item(display_row, render_item, window, cx)
    }

    fn layout_item(
        &self,
        display_row: DisplayRow,
        render_item: impl Fn(&mut Context<'_, Editor>, &mut Window) -> AnyElement,
        window: &mut Window,
        cx: &mut Context<'_, Editor>,
    ) -> Option<AnyElement> {
        if !self.range.contains(&display_row) {
            return None;
        }

        if self.row_infos.get((display_row.0.saturating_sub(self.range.start.0)) as usize).is_some_and(|row_info| {
            row_info.expand_info.is_some() || row_info.diff_status.is_some_and(|status| status.is_deleted())
        }) {
            return None;
        }

        let button = self.prepaint_button(render_item(cx, window), display_row, window, cx);
        Some(button)
    }

    fn prepaint_button(
        &self,
        mut button: AnyElement,
        row: DisplayRow,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let available_space = size(AvailableSpace::MinContent, AvailableSpace::Definite(self.line_height));
        let indicator_size = button.layout_as_root(available_space, window, cx);
        let git_gutter_width = EditorElement::gutter_strip_width(self.line_height)
            + self.dimensions.git_blame_entries_width.unwrap_or_default();

        // center in the slot between the git strip and the line numbers, so the gap to the
        // numbers stays even once they fill their column (4+ digit files)
        let slack = self.dimensions.left_padding - git_gutter_width - indicator_size.width;
        let x = git_gutter_width + (slack / 2.).max(Pixels::ZERO);

        let mut y = Pixels::from((row.as_f64() - self.scroll_position.y) * ScrollPixelOffset::from(self.line_height));
        y += (self.line_height - indicator_size.height) / 2.;

        button.prepaint_as_root(self.hitbox.origin + point(x, y), available_space, window, cx);
        button
    }
}

/// Marks `rows` as holding a selection head. Block rows (buffer headers,
/// split spacers) are skipped: nothing on them can be "the current line", and
/// the split's left side draws no header over them, so a highlight there would
/// show as a stray band around a folded file's header.
pub(super) fn mark_active_rows(
    active_rows: &mut BTreeMap<DisplayRow, LineHighlightSpec>,
    rows: RangeInclusive<u32>,
    has_selection: bool,
    is_block_line: impl Fn(DisplayRow) -> bool,
) {
    for row in rows.map(DisplayRow).filter(|row| !is_block_line(*row)) {
        active_rows.entry(row).or_default().selection |= has_selection;
    }
}

fn render_blame_entry(
    ix: usize,
    blame: &Entity<GitBlame>,
    blame_entry: BlameEntry,
    style: &EditorStyle,
    last_used_color: &mut Option<(Hsla, Oid)>,
    editor: Entity<Editor>,
    workspace: Entity<Workspace>,
    buffer: BufferId,
    renderer: &dyn BlameRenderer,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let index: u32 = blame_entry.sha.into();
    let mut sha_color = theme::theme().accents().color_for_index(index);

    // If the last color we used is the same as the one we get for this line, but
    // the commit SHAs are different, then we try again to get a different color.
    if let Some((color, sha)) = *last_used_color
        && sha != blame_entry.sha
        && color == sha_color
    {
        sha_color = theme::theme().accents().color_for_index(index + 1);
    }
    last_used_color.replace((sha_color, blame_entry.sha));

    let blame = blame.read(cx);
    let details = blame.details_for_entry(buffer, &blame_entry);
    let tag_names = blame.tag_names_for_entry(buffer, &blame_entry);
    let repository = blame.repository(cx, buffer)?;
    renderer.render_blame_entry(
        &style.text,
        blame_entry,
        details,
        tag_names,
        repository,
        workspace.downgrade(),
        editor,
        ix,
        sha_color,
        window,
        cx,
    )
}

#[cfg(test)]
mod tests {
    use multi_buffer::Anchor;
    use multi_buffer::ExpandExcerptDirection;
    use multi_buffer::ExpandInfo;
    use multi_buffer::RowInfo;

    use super::has_expand_toggles;

    #[test]
    fn has_expand_toggles_needs_a_row_with_expand_info() {
        let plain = RowInfo { buffer_row: Some(1), ..RowInfo::default() };
        let expandable = RowInfo {
            expand_info: Some(ExpandInfo { direction: ExpandExcerptDirection::Up, start_anchor: Anchor::Min }),
            ..RowInfo::default()
        };
        assert!(!has_expand_toggles(&[]));
        assert!(!has_expand_toggles(&[plain, plain]));
        assert!(has_expand_toggles(&[plain, expandable]));
    }
}
