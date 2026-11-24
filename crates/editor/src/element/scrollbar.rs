use std::ops::Range;

use gpui::Along as _;
use gpui::App;
use gpui::Axis as ScrollbarAxis;
use gpui::BorderStyle;
use gpui::Bounds;
use gpui::Corners;
use gpui::CursorStyle;
use gpui::DispatchPhase;
use gpui::Edges;
use gpui::Hitbox;
use gpui::HitboxBehavior;
use gpui::Hsla;
use gpui::IsZero as _;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::MouseMoveEvent;
use gpui::MouseUpEvent;
use gpui::Pixels;
use gpui::Size;
use gpui::Window;
use gpui::px;
use gpui::quad;
use gpui::size;
use workspace::item::Item as _;
use workspace::item::ItemBufferKind;

use super::EditorElement;
use super::EditorLayout;
use crate::EditorSnapshot;
use crate::display_map::HighlightKey;
use crate::scroll::ActiveScrollbarState;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollbarThumbState;

impl EditorElement {
    pub(super) fn layout_scrollbars(
        &self,
        snapshot: &EditorSnapshot,
        scrollbar_layout_information: &ScrollbarLayoutInformation,
        content_offset: gpui::Point<Pixels>,
        scroll_position: gpui::Point<ScrollOffset>,
        non_visible_cursors: bool,
        right_margin: Pixels,
        editor_width: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<EditorScrollbars> {
        if !self.editor.read(cx).show_horizontal_scrollbar || self.style.scrollbar_width.is_zero() {
            return None;
        }

        // If a drag took place after we started dragging the scrollbar,
        // cancel the scrollbar drag.
        if cx.has_active_drag() {
            self.editor.update(cx, |editor, cx| editor.scroll_manager.reset_scrollbar_state(cx));
        }

        let show_scrollbars = {
            let editor = self.editor.read(cx);
            let is_singleton = editor.buffer_kind(cx) == ItemBufferKind::Singleton;
            let supports_git_diff_markers = is_singleton || editor.allow_git_diff_scrollbar_markers;
            // Git
            (supports_git_diff_markers && snapshot.buffer_snapshot().has_diff_hunks())
            ||
            // Buffer Search Results
            (is_singleton && editor.has_background_highlights(HighlightKey::BufferSearchHighlights))
            ||
            // Selected Text Occurrences
            (is_singleton && editor.has_background_highlights(HighlightKey::SelectedTextHighlight))
            ||
            // Selected Symbol Occurrences
            (is_singleton && (editor.has_background_highlights(HighlightKey::DocumentHighlightRead) || editor.has_background_highlights(HighlightKey::DocumentHighlightWrite)))
            ||
            // Diagnostics
            (is_singleton && snapshot.buffer_snapshot().has_diagnostics())
            ||
            // Cursors out of sight
            non_visible_cursors
            ||
            // Scrollmanager
            editor.scroll_manager.scrollbars_visible()
        };

        // The horizontal scrollbar is usually slightly offset to align nicely with
        // indent guides. However, this offset is not needed if indent guides are
        // disabled for the current editor.
        let content_offset = if self.editor.read(cx).show_indent_guides.is_none_or(|should_show| should_show) {
            content_offset
        } else {
            gpui::Point::default()
        };

        Some(EditorScrollbars::new(
            scrollbar_layout_information,
            content_offset,
            scroll_position,
            self.style.scrollbar_width,
            right_margin,
            editor_width,
            show_scrollbars,
            self.editor.read(cx).scroll_manager.active_scrollbar_state(),
            window,
        ))
    }

    pub(super) fn paint_scrollbars(&self, layout: &mut EditorLayout, window: &mut Window, cx: &App) {
        let Some(scrollbars_layout) = layout.scrollbars_layout.take() else {
            return;
        };
        let any_scrollbar_dragged = self.editor.read(cx).scroll_manager.any_scrollbar_dragged();

        // Painted like the `ui` scrollbars (e.g. the project panel): no track, and a
        // thin rounded thumb inset from the larger hitbox.
        for (scrollbar_layout, _) in scrollbars_layout.iter_scrollbars() {
            let hitbox = &scrollbar_layout.hitbox;
            if !scrollbars_layout.visible {
                continue;
            }
            let Some(thumb_bounds) = scrollbar_layout.thumb_bounds else {
                continue;
            };
            let scrollbar_thumb_color = match scrollbar_layout.thumb_state {
                ScrollbarThumbState::Dragging => theme::theme().colors().scrollbar_thumb_active_background,
                ScrollbarThumbState::Hovered => theme::theme().colors().scrollbar_thumb_hover_background,
                ScrollbarThumbState::Idle => theme::theme().colors().scrollbar_thumb_background,
            };
            let thumb_bounds = thumb_bounds.dilate(-ui::SCROLLBAR_PADDING);
            window.paint_layer(hitbox.bounds, |window| {
                window.paint_quad(quad(
                    thumb_bounds,
                    Corners::all(Pixels::MAX).clamp_radii_for_quad_size(thumb_bounds.size),
                    scrollbar_thumb_color,
                    Edges::default(),
                    Hsla::transparent_black(),
                    BorderStyle::default(),
                ));
            });

            if any_scrollbar_dragged {
                window.set_window_cursor_style(CursorStyle::Arrow);
            } else {
                window.set_cursor_style(CursorStyle::Arrow, hitbox);
            }
        }

        window.on_mouse_event({
            let editor = self.editor.clone();
            let scrollbars_layout = scrollbars_layout.clone();

            let mut mouse_position = window.mouse_position();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    return;
                }

                editor.update(cx, |editor, cx| {
                    if let Some((scrollbar_layout, axis)) = event
                        .pressed_button
                        .filter(|button| *button == MouseButton::Left)
                        .and_then(|_| editor.scroll_manager.dragging_scrollbar_axis())
                        .and_then(|axis| scrollbars_layout.iter_scrollbars().find(|(_, a)| *a == axis))
                    {
                        let ScrollbarLayout { hitbox, text_unit_size, .. } = scrollbar_layout;

                        let old_position = mouse_position.along(axis);
                        let new_position = event.position.along(axis);
                        if (hitbox.origin.along(axis)..hitbox.bottom_right().along(axis)).contains(&old_position) {
                            let position = editor.scroll_position(cx).apply_along(axis, |p| {
                                (p + ScrollOffset::from((new_position - old_position) / *text_unit_size)).max(0.)
                            });
                            editor.set_scroll_position(position, window, cx);
                        }

                        editor.scroll_manager.show_scrollbars(window, cx);
                        cx.stop_propagation();
                    } else if let Some((layout, axis)) =
                        scrollbars_layout.get_hovered_axis(window).filter(|_| !event.dragging())
                    {
                        if layout.thumb_hovered(event.position) {
                            editor.scroll_manager.set_hovered_scroll_thumb_axis(axis, cx);
                        } else {
                            editor.scroll_manager.reset_scrollbar_state(cx);
                        }

                        editor.scroll_manager.show_scrollbars(window, cx);
                    } else {
                        editor.scroll_manager.reset_scrollbar_state(cx);
                    }

                    mouse_position = event.position;
                });
            }
        });

        if any_scrollbar_dragged {
            window.on_mouse_event({
                let editor = self.editor.clone();
                move |_: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        return;
                    }

                    editor.update(cx, |editor, cx| {
                        if let Some((_, axis)) = scrollbars_layout.get_hovered_axis(window) {
                            editor.scroll_manager.set_hovered_scroll_thumb_axis(axis, cx);
                        } else {
                            editor.scroll_manager.reset_scrollbar_state(cx);
                        }
                        cx.stop_propagation();
                    });
                }
            });
        } else {
            window.on_mouse_event({
                let editor = self.editor.clone();

                move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        return;
                    }
                    let Some((scrollbar_layout, axis)) = scrollbars_layout.get_hovered_axis(window) else {
                        return;
                    };

                    let ScrollbarLayout { hitbox, visible_range, text_unit_size, thumb_bounds, .. } = scrollbar_layout;

                    let Some(thumb_bounds) = thumb_bounds else {
                        return;
                    };

                    editor.update(cx, |editor, cx| {
                        editor.scroll_manager.set_dragged_scroll_thumb_axis(axis, cx);

                        let event_position = event.position.along(axis);

                        if event_position < thumb_bounds.origin.along(axis)
                            || thumb_bounds.bottom_right().along(axis) < event_position
                        {
                            let center_position =
                                ((event_position - hitbox.origin.along(axis)) / *text_unit_size).round() as u32;
                            let start_position =
                                center_position.saturating_sub((visible_range.end - visible_range.start) as u32 / 2);

                            let position =
                                editor.scroll_position(cx).apply_along(axis, |_| start_position as ScrollOffset);

                            editor.set_scroll_position(position, window, cx);
                        } else {
                            editor.scroll_manager.show_scrollbars(window, cx);
                        }

                        cx.stop_propagation();
                    });
                }
            });
        }
    }
}

/// Holds information required for layouting the editor scrollbars.
#[derive(Clone, Copy)]
pub(super) struct ScrollbarLayoutInformation {
    /// The bounds of the editor area (excluding the content offset).
    pub(super) editor_bounds: Bounds<Pixels>,
    /// The available range to scroll within the document.
    pub(super) scroll_range: Size<Pixels>,
    /// The space available for one glyph in the editor.
    pub(super) glyph_grid_cell: Size<Pixels>,
}

impl ScrollbarLayoutInformation {
    pub(super) fn new(
        editor_bounds: Bounds<Pixels>,
        glyph_grid_cell: Size<Pixels>,
        document_size: Size<Pixels>,
        scroll_beyond_last_line: bool,
    ) -> Self {
        let vertical_overscroll =
            if scroll_beyond_last_line { editor_bounds.size.height } else { glyph_grid_cell.height };

        let overscroll = size(Pixels::ZERO, vertical_overscroll);

        ScrollbarLayoutInformation { editor_bounds, scroll_range: document_size + overscroll, glyph_grid_cell }
    }
}

/// Only the horizontal axis gets a scrollbar; vertical scrolling and its
/// markers live on the minimap.
#[derive(Clone)]
pub(super) struct EditorScrollbars {
    pub horizontal: Option<ScrollbarLayout>,
    pub visible: bool,
}

impl EditorScrollbars {
    pub(super) fn new(
        layout_information: &ScrollbarLayoutInformation,
        content_offset: gpui::Point<Pixels>,
        scroll_position: gpui::Point<f64>,
        scrollbar_width: Pixels,
        right_margin: Pixels,
        editor_width: Pixels,
        show_scrollbars: bool,
        scrollbar_state: Option<&ActiveScrollbarState>,
        window: &mut Window,
    ) -> Self {
        const AXIS: ScrollbarAxis = ScrollbarAxis::Horizontal;
        let ScrollbarLayoutInformation { editor_bounds, scroll_range, glyph_grid_cell } = layout_information;

        let horizontal = (scroll_range.width > editor_width).then(|| {
            let bounds = Bounds::from_anchor_and_size(
                gpui::Anchor::BottomLeft,
                editor_bounds.bottom_left(),
                size(
                    // The horizontal viewport size differs from the space available for the
                    // horizontal scrollbar, so we have to manually stitch it together here.
                    editor_bounds.size.width - right_margin,
                    scrollbar_width,
                ),
            );
            ScrollbarLayout::new(
                window.insert_hitbox(bounds, HitboxBehavior::Normal),
                editor_width,
                scroll_range.width,
                glyph_grid_cell.along(AXIS),
                content_offset.along(AXIS),
                scroll_position.along(AXIS),
                show_scrollbars,
                AXIS,
            )
            .with_thumb_state(scrollbar_state.and_then(|state| state.thumb_state_for_axis(AXIS)))
        });

        Self { horizontal, visible: show_scrollbars }
    }

    pub(super) fn iter_scrollbars(&self) -> impl Iterator<Item = (&ScrollbarLayout, ScrollbarAxis)> + '_ {
        self.horizontal.iter().map(|scrollbar| (scrollbar, ScrollbarAxis::Horizontal))
    }

    /// Returns the currently hovered scrollbar axis, if any.
    pub(super) fn get_hovered_axis(&self, window: &Window) -> Option<(&ScrollbarLayout, ScrollbarAxis)> {
        self.iter_scrollbars().find(|s| s.0.hitbox.is_hovered(window))
    }
}

#[derive(Clone)]
pub(super) struct ScrollbarLayout {
    pub(super) hitbox: Hitbox,
    pub(super) visible_range: Range<ScrollOffset>,
    pub(super) text_unit_size: Pixels,
    pub(super) thumb_bounds: Option<Bounds<Pixels>>,
    pub(super) thumb_state: ScrollbarThumbState,
}

impl ScrollbarLayout {
    const MIN_THUMB_SIZE: Pixels = px(25.0);

    fn new(
        scrollbar_track_hitbox: Hitbox,
        viewport_size: Pixels,
        scroll_range: Pixels,
        glyph_space: Pixels,
        content_offset: Pixels,
        scroll_position: ScrollOffset,
        show_thumb: bool,
        axis: ScrollbarAxis,
    ) -> Self {
        let track_bounds = scrollbar_track_hitbox.bounds;
        // The length of the track available to the scrollbar thumb. We deliberately
        // exclude the content size here so that the thumb aligns with the content.
        let track_length = track_bounds.size.along(axis) - content_offset;

        Self::new_with_hitbox_and_track_length(
            scrollbar_track_hitbox,
            track_length,
            viewport_size,
            scroll_range.into(),
            glyph_space,
            content_offset.into(),
            scroll_position,
            show_thumb,
            axis,
        )
    }

    pub(super) fn for_minimap(
        minimap_track_hitbox: Hitbox,
        visible_lines: f64,
        total_editor_lines: f64,
        minimap_line_height: Pixels,
        scroll_position: ScrollOffset,
        minimap_scroll_top: ScrollOffset,
    ) -> Self {
        // The scrollbar thumb size is calculated as
        // (visible_content/total_content) × scrollbar_track_length.
        //
        // For the minimap's thumb layout, we leverage this by setting the
        // scrollbar track length to the entire document size (using minimap line
        // height). This creates a thumb that exactly represents the editor
        // viewport scaled to minimap proportions.
        //
        // We adjust the thumb position relative to `minimap_scroll_top` to
        // accommodate for the deliberately oversized track.
        //
        // This approach ensures that the minimap thumb accurately reflects the
        // editor's current scroll position whilst nicely synchronizing the minimap
        // thumb and scrollbar thumb.
        let scroll_range = total_editor_lines * f64::from(minimap_line_height);
        let viewport_size = visible_lines * f64::from(minimap_line_height);

        let track_top_offset = -minimap_scroll_top * f64::from(minimap_line_height);

        Self::new_with_hitbox_and_track_length(
            minimap_track_hitbox,
            Pixels::from(scroll_range),
            Pixels::from(viewport_size),
            scroll_range,
            minimap_line_height,
            track_top_offset,
            scroll_position,
            true,
            ScrollbarAxis::Vertical,
        )
    }

    fn new_with_hitbox_and_track_length(
        scrollbar_track_hitbox: Hitbox,
        track_length: Pixels,
        viewport_size: Pixels,
        scroll_range: f64,
        glyph_space: Pixels,
        content_offset: ScrollOffset,
        scroll_position: ScrollOffset,
        show_thumb: bool,
        axis: ScrollbarAxis,
    ) -> Self {
        let text_units_per_page = viewport_size.to_f64() / glyph_space.to_f64();
        let visible_range = scroll_position..scroll_position + text_units_per_page;
        let total_text_units = scroll_range / glyph_space.to_f64();

        let thumb_percentage = text_units_per_page / total_text_units;
        let thumb_size = Pixels::from(ScrollOffset::from(track_length) * thumb_percentage)
            .max(ScrollbarLayout::MIN_THUMB_SIZE)
            .min(track_length);

        let text_unit_divisor = (total_text_units - text_units_per_page).max(0.);

        let content_larger_than_viewport = text_unit_divisor > 0.;

        let text_unit_size = if content_larger_than_viewport {
            Pixels::from(ScrollOffset::from(track_length - thumb_size) / text_unit_divisor)
        } else {
            glyph_space
        };

        let thumb_bounds = (show_thumb && content_larger_than_viewport).then(|| {
            Self::thumb_bounds(
                &scrollbar_track_hitbox,
                content_offset,
                visible_range.start,
                text_unit_size,
                thumb_size,
                axis,
            )
        });

        ScrollbarLayout {
            hitbox: scrollbar_track_hitbox,
            visible_range,
            text_unit_size,
            thumb_bounds,
            thumb_state: ScrollbarThumbState::default(),
        }
    }

    pub(super) const fn with_thumb_state(self, thumb_state: Option<ScrollbarThumbState>) -> Self {
        if let Some(thumb_state) = thumb_state { Self { thumb_state, ..self } } else { self }
    }

    fn thumb_bounds(
        scrollbar_track: &Hitbox,
        content_offset: f64,
        visible_range_start: f64,
        text_unit_size: Pixels,
        thumb_size: Pixels,
        axis: ScrollbarAxis,
    ) -> Bounds<Pixels> {
        let thumb_origin = scrollbar_track.origin.apply_along(axis, |origin| {
            origin + Pixels::from(content_offset + visible_range_start * ScrollOffset::from(text_unit_size))
        });
        Bounds::new(thumb_origin, scrollbar_track.size.apply_along(axis, |_| thumb_size))
    }

    fn thumb_hovered(&self, position: gpui::Point<Pixels>) -> bool {
        self.thumb_bounds.is_some_and(|bounds| bounds.contains(&position))
    }
}
