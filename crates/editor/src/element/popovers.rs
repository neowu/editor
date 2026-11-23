use std::cmp;
use std::ops::Range;

use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::Bounds;
use gpui::Edges;
use gpui::Hitbox;
use gpui::InteractiveElement as _;
use gpui::IntoElement as _;
use gpui::Pixels;
use gpui::Size;
use gpui::Styled as _;
use gpui::TextStyleRefinement;
use gpui::Window;
use gpui::div;
use gpui::point;
use gpui::px;
use gpui::size;
use itertools::Itertools as _;
use theme::built_in::BUFFER_LINE_HEIGHT;
use ui::POPOVER_Y_PADDING;
use ui::prelude::DefiniteLength;

use super::EditorElement;
use super::EditorLayout;
use super::LineWithInvisibles;
use crate::ContextMenuPlacement;
use crate::DisplayPoint;
use crate::DisplayRow;
use crate::EditorSnapshot;
use crate::EditorStyle;
use crate::Point;
use crate::RowExt as _;
use crate::code_context_menus::MENU_ASIDE_MAX_WIDTH;
use crate::code_context_menus::MENU_ASIDE_MIN_WIDTH;
use crate::code_context_menus::MENU_GAP;
use crate::hover_popover::HOVER_POPOVER_GAP;
use crate::hover_popover::MIN_POPOVER_CHARACTER_WIDTH;
use crate::hover_popover::MIN_POPOVER_LINE_HEIGHT;
use crate::hover_popover::POPOVER_RIGHT_OFFSET;
use crate::scroll::ScrollPixelOffset;

impl EditorElement {
    pub(super) fn layout_cursor_popovers(
        &self,
        line_height: Pixels,
        text_hitbox: &Hitbox,
        content_origin: gpui::Point<Pixels>,
        right_margin: Pixels,
        start_row: DisplayRow,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        line_layouts: &[LineWithInvisibles],
        cursor: DisplayPoint,
        _cursor_point: Point,
        _style: &EditorStyle,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<ContextMenuLayout> {
        let mut min_menu_height = Pixels::ZERO;
        let mut max_menu_height = Pixels::ZERO;
        let height_above_menu = Pixels::ZERO;
        let height_below_menu = Pixels::ZERO;
        let mut context_menu_visible = false;
        let context_menu_placement;

        {
            let editor = self.editor.read(cx);
            if editor.context_menu_visible()
                && matches!(editor.context_menu_origin(), Some(crate::ContextMenuOrigin::Cursor))
            {
                let (min_height_in_lines, max_height_in_lines) = editor
                    .context_menu_options
                    .as_ref()
                    .map_or((3, 12), |options| (options.min_entries_visible, options.max_entries_visible));

                min_menu_height += line_height * min_height_in_lines as f32 + POPOVER_Y_PADDING;
                max_menu_height += line_height * max_height_in_lines as f32 + POPOVER_Y_PADDING;
                context_menu_visible = true;
            }
            context_menu_placement = editor.context_menu_options.as_ref().and_then(|options| options.placement.clone());
        }

        let visible = context_menu_visible;
        if !visible {
            return None;
        }

        let cursor_row_layout = &line_layouts[cursor.row().minus(start_row) as usize];
        let target_position = content_origin
            + gpui::Point {
                x: cmp::max(
                    px(0.),
                    Pixels::from(
                        ScrollPixelOffset::from(cursor_row_layout.x_for_index(cursor.column() as usize))
                            - scroll_pixel_position.x,
                    ),
                ),
                y: cmp::max(
                    px(0.),
                    Pixels::from(
                        cursor.row().next_row().as_f64() * ScrollPixelOffset::from(line_height)
                            - scroll_pixel_position.y,
                    ),
                ),
            };

        let viewport_bounds = Bounds::new(gpui::Point::default(), window.viewport_size())
            .extend(Edges { right: -right_margin - MENU_GAP, ..Default::default() });

        let min_height = height_above_menu + min_menu_height + height_below_menu;
        let max_height = height_above_menu + max_menu_height + height_below_menu;
        let (laid_out_popovers, y_flipped) = self.layout_popovers_above_or_below_line(
            target_position,
            line_height,
            min_height,
            max_height,
            context_menu_placement,
            text_hitbox,
            viewport_bounds,
            window,
            cx,
            |height, max_width_for_stable_x, y_flipped, window, cx| {
                // First layout the menu to get its size - others can be at least this wide.
                let context_menu = context_menu_visible.then(|| {
                    let menu_height = if y_flipped { height - height_below_menu } else { height - height_above_menu };
                    let mut element = self
                        .render_context_menu(line_height, menu_height, window, cx)
                        .expect("Visible context menu should always render.");
                    let size = element.layout_as_root(AvailableSpace::min_size(), window, cx);
                    (CursorPopoverType::CodeContextMenu, element, size)
                });
                let _min_width = context_menu.as_ref().map_or(px(0.), |(_, _, size)| size.width);
                let _max_width =
                    max_width_for_stable_x.max(context_menu.as_ref().map_or(px(0.), |(_, _, size)| size.width));

                context_menu.into_iter().collect::<Vec<_>>()
            },
        )?;

        let (menu_ix, (_, menu_bounds)) =
            laid_out_popovers.iter().find_position(|(x, _)| matches!(x, CursorPopoverType::CodeContextMenu))?;
        let last_ix = laid_out_popovers.len() - 1;
        let menu_is_last = menu_ix == last_ix;
        let first_popover_bounds = laid_out_popovers[0].1;
        let last_popover_bounds = laid_out_popovers[last_ix].1;

        // Bounds to layout the aside around. When y_flipped, the aside goes either above or to the
        // right, and otherwise it goes below or to the right.
        let mut target_bounds = Bounds::from_corners(first_popover_bounds.origin, last_popover_bounds.bottom_right());
        target_bounds.size.width = menu_bounds.size.width;

        // Like `target_bounds`, but with the max height it could occupy. Choosing an aside position
        // based on this is preferred for layout stability.
        let mut max_target_bounds = target_bounds;
        max_target_bounds.size.height = max_height;
        if y_flipped {
            max_target_bounds.origin.y -= max_height - target_bounds.size.height;
        }

        // Add spacing around `target_bounds` and `max_target_bounds`.
        let mut extend_amount = Edges::all(MENU_GAP);
        if y_flipped {
            extend_amount.bottom = line_height;
        } else {
            extend_amount.top = line_height;
        }
        let target_bounds = target_bounds.extend(extend_amount);
        let max_target_bounds = max_target_bounds.extend(extend_amount);

        let must_place_above_or_below = if y_flipped && !menu_is_last && menu_bounds.size.height < max_menu_height {
            laid_out_popovers[menu_ix + 1..]
                .iter()
                .any(|(_, popover_bounds)| popover_bounds.size.width > menu_bounds.size.width)
        } else {
            false
        };

        let aside_bounds = self.layout_context_menu_aside(
            y_flipped,
            *menu_bounds,
            target_bounds,
            max_target_bounds,
            max_menu_height,
            must_place_above_or_below,
            text_hitbox,
            viewport_bounds,
            window,
            cx,
        );

        if let Some(menu_bounds) = laid_out_popovers.iter().find_map(|(popover_type, bounds)| {
            matches!(popover_type, CursorPopoverType::CodeContextMenu).then_some(*bounds)
        }) {
            let bounds =
                if let Some(aside_bounds) = aside_bounds { menu_bounds.union(&aside_bounds) } else { menu_bounds };
            return Some(ContextMenuLayout { y_flipped, bounds });
        }

        None
    }

    pub(super) fn layout_gutter_menu(
        &self,
        line_height: Pixels,
        text_hitbox: &Hitbox,
        content_origin: gpui::Point<Pixels>,
        right_margin: Pixels,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        gutter_overshoot: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let editor = self.editor.read(cx);
        if !editor.context_menu_visible() {
            return;
        }
        let Some(crate::ContextMenuOrigin::GutterIndicator(gutter_row)) = editor.context_menu_origin() else {
            return;
        };
        // Context menu was spawned via a click on a gutter. Ensure it's a bit closer to the
        // indicator than just a plain first column of the text field.
        let target_position = content_origin
            + gpui::Point {
                x: -gutter_overshoot,
                y: Pixels::from(
                    gutter_row.next_row().as_f64() * ScrollPixelOffset::from(line_height) - scroll_pixel_position.y,
                ),
            };

        let (min_height_in_lines, max_height_in_lines) = editor
            .context_menu_options
            .as_ref()
            .map_or((3, 12), |options| (options.min_entries_visible, options.max_entries_visible));

        let min_height = line_height * min_height_in_lines as f32 + POPOVER_Y_PADDING;
        let max_height = line_height * max_height_in_lines as f32 + POPOVER_Y_PADDING;
        let viewport_bounds = Bounds::new(gpui::Point::default(), window.viewport_size())
            .extend(Edges { right: -right_margin - MENU_GAP, ..Default::default() });
        self.layout_popovers_above_or_below_line(
            target_position,
            line_height,
            min_height,
            max_height,
            editor.context_menu_options.as_ref().and_then(|options| options.placement.clone()),
            text_hitbox,
            viewport_bounds,
            window,
            cx,
            move |height, _max_width_for_stable_x, _, window, cx| {
                let mut element = self
                    .render_context_menu(line_height, height, window, cx)
                    .expect("Visible context menu should always render.");
                let size = element.layout_as_root(AvailableSpace::min_size(), window, cx);
                vec![(CursorPopoverType::CodeContextMenu, element, size)]
            },
        );
    }

    #[allow(clippy::type_complexity)]
    fn layout_popovers_above_or_below_line(
        &self,
        target_position: gpui::Point<Pixels>,
        line_height: Pixels,
        min_height: Pixels,
        max_height: Pixels,
        placement: Option<ContextMenuPlacement>,
        text_hitbox: &Hitbox,
        viewport_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
        make_sized_popovers: impl FnOnce(
            Pixels,
            Pixels,
            bool,
            &mut Window,
            &mut App,
        ) -> Vec<(CursorPopoverType, AnyElement, Size<Pixels>)>,
    ) -> Option<(Vec<(CursorPopoverType, Bounds<Pixels>)>, bool)> {
        let text_style = TextStyleRefinement {
            line_height: Some(DefiniteLength::Fraction(BUFFER_LINE_HEIGHT)),
            ..Default::default()
        };
        window.with_text_style(Some(text_style), |window| {
            // If the max height won't fit below and there is more space above, put it above the line.
            let bottom_y_when_flipped = target_position.y - line_height;
            let available_above = bottom_y_when_flipped - text_hitbox.top();
            let available_below = text_hitbox.bottom() - target_position.y;
            let y_overflows_below = max_height > available_below;
            let mut y_flipped = match placement {
                Some(ContextMenuPlacement::Above) => true,
                Some(ContextMenuPlacement::Below) => false,
                None => y_overflows_below && available_above > available_below,
            };
            let mut height = cmp::min(max_height, if y_flipped { available_above } else { available_below });

            // If the min height doesn't fit within text bounds, instead fit within the window.
            if height < min_height {
                let available_above = bottom_y_when_flipped;
                let available_below = viewport_bounds.bottom() - target_position.y;
                let (y_flipped_override, height_override) = match placement {
                    Some(ContextMenuPlacement::Above) => (true, cmp::min(available_above, min_height)),
                    Some(ContextMenuPlacement::Below) => (false, cmp::min(available_below, min_height)),
                    None => {
                        if available_below > min_height {
                            (false, min_height)
                        } else if available_above > min_height {
                            (true, min_height)
                        } else if available_above > available_below {
                            (true, available_above)
                        } else {
                            (false, available_below)
                        }
                    }
                };
                y_flipped = y_flipped_override;
                height = height_override;
            }

            let max_width_for_stable_x = viewport_bounds.right() - target_position.x;

            // TODO: Use viewport_bounds.width as a max width so that it doesn't get clipped on the left
            // for very narrow windows.
            let popovers = make_sized_popovers(height, max_width_for_stable_x, y_flipped, window, cx);
            if popovers.is_empty() {
                return None;
            }

            let max_width = popovers.iter().map(|(_, _, size)| size.width).max().unwrap_or_default();

            let mut current_position = gpui::Point {
                // Snap the right edge of the list to the right edge of the window if its horizontal bounds
                // overflow. Include space for the scrollbar.
                x: target_position.x.min((viewport_bounds.right() - max_width).max(Pixels::ZERO)),
                y: if y_flipped { bottom_y_when_flipped } else { target_position.y },
            };

            let mut laid_out_popovers = popovers
                .into_iter()
                .map(|(popover_type, element, size)| {
                    if y_flipped {
                        current_position.y -= size.height;
                    }
                    let position = current_position;
                    window.defer_draw(element, current_position, 1, None);
                    if y_flipped {
                        current_position.y -= MENU_GAP;
                    } else {
                        current_position.y += size.height + MENU_GAP;
                    }
                    (popover_type, Bounds::new(position, size))
                })
                .collect::<Vec<_>>();

            if y_flipped {
                laid_out_popovers.reverse();
            }

            Some((laid_out_popovers, y_flipped))
        })
    }

    fn layout_context_menu_aside(
        &self,
        y_flipped: bool,
        menu_bounds: Bounds<Pixels>,
        target_bounds: Bounds<Pixels>,
        max_target_bounds: Bounds<Pixels>,
        max_height: Pixels,
        must_place_above_or_below: bool,
        text_hitbox: &Hitbox,
        viewport_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let available_within_viewport = target_bounds.space_within(&viewport_bounds);
        let positioned_aside = if available_within_viewport.right >= MENU_ASIDE_MIN_WIDTH && !must_place_above_or_below
        {
            let max_width = cmp::min(available_within_viewport.right - px(1.), MENU_ASIDE_MAX_WIDTH);
            let mut aside =
                self.render_context_menu_aside(size(max_width, max_height - POPOVER_Y_PADDING), window, cx)?;
            let size = aside.layout_as_root(AvailableSpace::min_size(), window, cx);
            let right_position = point(target_bounds.right(), menu_bounds.origin.y);
            Some((aside, right_position, size))
        } else {
            let max_size = size(
                // TODO(mgsloan): Once the menu is bounded by viewport width the bound on viewport
                // won't be needed here.
                cmp::min(cmp::max(menu_bounds.size.width - px(2.), MENU_ASIDE_MIN_WIDTH), viewport_bounds.right()),
                cmp::min(max_height, cmp::max(available_within_viewport.top, available_within_viewport.bottom))
                    - POPOVER_Y_PADDING,
            );
            let mut aside = self.render_context_menu_aside(max_size, window, cx)?;
            let actual_size = aside.layout_as_root(AvailableSpace::min_size(), window, cx);

            let top_position = point(menu_bounds.origin.x, target_bounds.top() - actual_size.height);
            let bottom_position = point(menu_bounds.origin.x, target_bounds.bottom());

            let fit_within = |available: Edges<Pixels>, wanted: Size<Pixels>| {
                // Prefer to fit on the same side of the line as the menu, then on the other side of
                // the line.
                if !y_flipped && wanted.height < available.bottom {
                    Some(bottom_position)
                } else if wanted.height < available.top {
                    Some(top_position)
                } else if y_flipped && wanted.height < available.bottom {
                    Some(bottom_position)
                } else {
                    None
                }
            };

            // Prefer choosing a direction using max sizes rather than actual size for stability.
            let available_within_text = max_target_bounds.space_within(&text_hitbox.bounds);
            let wanted = size(MENU_ASIDE_MAX_WIDTH, max_height);
            let aside_position = fit_within(available_within_text, wanted)
                // Fallback: fit max size in window.
                .or_else(|| fit_within(max_target_bounds.space_within(&viewport_bounds), wanted))
                // Fallback: fit actual size in window.
                .or_else(|| fit_within(available_within_viewport, actual_size));

            aside_position.map(|position| (aside, position, actual_size))
        };

        // Skip drawing if it doesn't fit anywhere.
        if let Some((aside, position, size)) = positioned_aside {
            let aside_bounds = Bounds::new(position, size);
            window.defer_draw(aside, position, 2, None);
            return Some(aside_bounds);
        }

        None
    }

    fn render_context_menu(
        &self,
        line_height: Pixels,
        height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let max_height_in_lines = ((height - POPOVER_Y_PADDING) / line_height).floor() as u32;
        self.editor.update(cx, |editor, cx| editor.render_context_menu(max_height_in_lines, window, cx))
    }

    fn render_context_menu_aside(
        &self,
        max_size: Size<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if max_size.width < px(100.) || max_size.height < px(12.) {
            None
        } else {
            self.editor.update(cx, |editor, cx| editor.render_context_menu_aside(max_size, window, cx))
        }
    }

    pub(super) fn layout_hover_popovers(
        &self,
        snapshot: &EditorSnapshot,
        hitbox: &Hitbox,
        visible_display_row_range: Range<DisplayRow>,
        content_origin: gpui::Point<Pixels>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        line_layouts: &[LineWithInvisibles],
        line_height: Pixels,
        em_width: Pixels,
        context_menu_layout: Option<ContextMenuLayout>,
        window: &mut Window,
        cx: &mut App,
    ) {
        struct MeasuredHoverPopover {
            element: AnyElement,
            size: Size<Pixels>,
            horizontal_offset: Pixels,
        }

        let max_size = size(
            (120. * em_width) // Default size
                .min(hitbox.size.width / 2.) // Shrink to half of the editor width
                .max(MIN_POPOVER_CHARACTER_WIDTH * em_width), // Apply minimum width of 20 characters
            (16. * line_height) // Default size
                .min(hitbox.size.height / 2.) // Shrink to half of the editor height
                .max(MIN_POPOVER_LINE_HEIGHT * line_height), // Apply minimum height of 4 lines
        );

        // Don't show hover popovers when context menu is open to avoid overlap
        let has_context_menu = self.editor.read(cx).mouse_context_menu.is_some();
        if has_context_menu {
            return;
        }

        let hover_popovers = self.editor.update(cx, |editor, cx| {
            if !editor.hover_state.visible() {
                return None;
            }
            editor.hover_state.render(
                snapshot,
                visible_display_row_range.clone(),
                max_size,
                &editor.text_layout_details(window, cx),
                window,
                cx,
            )
        });
        let Some((popover_position, hover_popovers)) = hover_popovers else {
            return;
        };

        // This is safe because we check on layout whether the required row is available
        let hovered_row_layout = &line_layouts[popover_position.row().minus(visible_display_row_range.start) as usize];

        // Compute Hovered Point
        let x =
            hovered_row_layout.x_for_index(popover_position.column() as usize) - Pixels::from(scroll_pixel_position.x);
        let y = Pixels::from(
            popover_position.row().as_f64() * ScrollPixelOffset::from(line_height) - scroll_pixel_position.y,
        );
        let hovered_point = content_origin + point(x, y);

        let mut overall_height = Pixels::ZERO;

        let measured_hover_popovers = hover_popovers
            .into_iter()
            .with_position()
            .map(|(position, mut hover_popover)| {
                let size = hover_popover.layout_as_root(AvailableSpace::min_size(), window, cx);
                let horizontal_offset =
                    (hitbox.top_right().x - POPOVER_RIGHT_OFFSET - (hovered_point.x + size.width)).min(Pixels::ZERO);
                if !position.is_first() {
                    overall_height += HOVER_POPOVER_GAP;
                }
                overall_height += size.height;
                MeasuredHoverPopover { element: hover_popover, size, horizontal_offset }
            })
            .collect::<Vec<_>>();

        fn draw_occluder(width: Pixels, origin: gpui::Point<Pixels>, window: &mut Window, cx: &mut App) {
            let mut occlusion =
                div().size_full().occlude().on_mouse_move(|_, _, cx| cx.stop_propagation()).into_any_element();
            occlusion.layout_as_root(size(width, HOVER_POPOVER_GAP).into(), window, cx);
            window.defer_draw(occlusion, origin, 2, None);
        }

        fn place_popovers_above(
            hovered_point: gpui::Point<Pixels>,
            measured_hover_popovers: Vec<MeasuredHoverPopover>,
            window: &mut Window,
            cx: &mut App,
        ) {
            let mut current_y = hovered_point.y;
            for (position, popover) in measured_hover_popovers.into_iter().with_position() {
                let size = popover.size;
                let popover_origin = point(hovered_point.x + popover.horizontal_offset, current_y - size.height);

                window.defer_draw(popover.element, popover_origin, 2, None);
                if position.is_first() || !position.is_last() {
                    let origin = point(popover_origin.x, popover_origin.y - HOVER_POPOVER_GAP);
                    draw_occluder(size.width, origin, window, cx);
                }

                current_y = popover_origin.y - HOVER_POPOVER_GAP;
            }
        }

        fn place_popovers_below(
            hovered_point: gpui::Point<Pixels>,
            measured_hover_popovers: Vec<MeasuredHoverPopover>,
            line_height: Pixels,
            window: &mut Window,
            cx: &mut App,
        ) {
            let mut current_y = hovered_point.y + line_height;
            for (position, popover) in measured_hover_popovers.into_iter().with_position() {
                let size = popover.size;
                let popover_origin = point(hovered_point.x + popover.horizontal_offset, current_y);

                window.defer_draw(popover.element, popover_origin, 2, None);
                if position.is_first() || !position.is_last() {
                    let origin = point(popover_origin.x, popover_origin.y + size.height);
                    draw_occluder(size.width, origin, window, cx);
                }

                current_y = popover_origin.y + size.height + HOVER_POPOVER_GAP;
            }
        }

        let intersects_menu = |bounds: Bounds<Pixels>| -> bool {
            context_menu_layout.as_ref().is_some_and(|menu| bounds.intersects(&menu.bounds))
        };

        let can_place_above = {
            let mut current_y = hovered_point.y;
            measured_hover_popovers.iter().all(|popover| {
                let size = popover.size;
                let popover_origin = point(hovered_point.x + popover.horizontal_offset, current_y - size.height);
                let bounds = Bounds::new(popover_origin, size);
                current_y = popover_origin.y - HOVER_POPOVER_GAP;
                bounds.is_contained_within(hitbox) && !intersects_menu(bounds)
            })
        };

        let can_place_below = || {
            let mut current_y = hovered_point.y + line_height;
            measured_hover_popovers.iter().all(|popover| {
                let size = popover.size;
                let popover_origin = point(hovered_point.x + popover.horizontal_offset, current_y);
                let bounds = Bounds::new(popover_origin, size);
                current_y = popover_origin.y + size.height + HOVER_POPOVER_GAP;
                bounds.is_contained_within(hitbox) && !intersects_menu(bounds)
            })
        };

        if can_place_above {
            // try placing above hovered point
            place_popovers_above(hovered_point, measured_hover_popovers, window, cx);
        } else if can_place_below() {
            // try placing below hovered point
            place_popovers_below(hovered_point, measured_hover_popovers, line_height, window, cx);
        } else {
            // try to place popovers around the context menu
            let origin_surrounding_menu = context_menu_layout.as_ref().and_then(|menu| {
                let total_width = measured_hover_popovers.iter().map(|p| p.size.width).max().unwrap_or(Pixels::ZERO);
                let y_for_horizontal_positioning =
                    if menu.y_flipped { menu.bounds.bottom() - overall_height } else { menu.bounds.top() };
                let possible_origins = [
                    // left of context menu
                    point(menu.bounds.left() - total_width - HOVER_POPOVER_GAP, y_for_horizontal_positioning),
                    // right of context menu
                    point(menu.bounds.right() + HOVER_POPOVER_GAP, y_for_horizontal_positioning),
                    // top of context menu
                    point(menu.bounds.left(), menu.bounds.top() - overall_height - HOVER_POPOVER_GAP),
                    // bottom of context menu
                    point(menu.bounds.left(), menu.bounds.bottom() + HOVER_POPOVER_GAP),
                ];
                possible_origins
                    .into_iter()
                    .find(|&origin| Bounds::new(origin, size(total_width, overall_height)).is_contained_within(hitbox))
            });
            if let Some(origin) = origin_surrounding_menu {
                let mut current_y = origin.y;
                for (position, popover) in measured_hover_popovers.into_iter().with_position() {
                    let size = popover.size;
                    let popover_origin = point(origin.x, current_y);

                    window.defer_draw(popover.element, popover_origin, 2, None);
                    if position.is_first() || !position.is_last() {
                        let origin = point(popover_origin.x, popover_origin.y + size.height);
                        draw_occluder(size.width, origin, window, cx);
                    }

                    current_y = popover_origin.y + size.height + HOVER_POPOVER_GAP;
                }
            } else {
                // fallback to existing above/below cursor logic
                // this might overlap menu or overflow in rare case
                if can_place_above {
                    place_popovers_above(hovered_point, measured_hover_popovers, window, cx);
                } else {
                    place_popovers_below(hovered_point, measured_hover_popovers, line_height, window, cx);
                }
            }
        }
    }

    pub(super) fn paint_mouse_context_menu(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        if let Some(mouse_context_menu) = layout.mouse_context_menu.as_mut() {
            mouse_context_menu.paint(window, cx);
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ContextMenuLayout {
    y_flipped: bool,
    bounds: Bounds<Pixels>,
}

pub(super) enum CursorPopoverType {
    CodeContextMenu,
}
