use std::ops::Range;

use gpui::AnyElement;
use gpui::App;
use gpui::AvailableSpace;
use gpui::ContentMask;
use gpui::CursorStyle;
use gpui::Element as _;
use gpui::Hitbox;
use gpui::InteractiveElement as _;
use gpui::IntoElement as _;
use gpui::ParentElement as _;
use gpui::Pixels;
use gpui::Size;
use gpui::Styled as _;
use gpui::Window;
use gpui::div;
use gpui::pattern_slash;
use gpui::point;
use gpui::px;
use gpui::size;
use multi_buffer::Anchor;
use multi_buffer::MultiBufferRow;
use rustc_hash::FxHashMap;
use text::BufferId;
use ui::prelude::h_flex;
use ui::prelude::v_flex;

use super::EditorElement;
use super::EditorLayout;
use super::IndentGuideLayout;
use super::LineWithInvisibles;
use super::header;
use super::layout_line;
use crate::BlockId;
use crate::CustomBlockId;
use crate::DisplayRow;
use crate::EditorSnapshot;
use crate::FILE_HEADER_HEIGHT;
use crate::FocusedBlock;
use crate::RowExt as _;
use crate::ToPoint as _;
use crate::display_map::Block;
use crate::display_map::BlockContext;
use crate::display_map::BlockStyle;
use crate::display_map::EditorMargins;
use crate::display_map::ToDisplayPoint as _;
use crate::scroll::ScrollOffset;
use crate::scroll::ScrollPixelOffset;

#[derive(Default)]
pub(super) struct RenderBlocksOutput {
    // We store spacer blocks separately because they paint in a different order
    // (spacers -> indent guides -> non-spacers)
    pub(super) non_spacer_blocks: Vec<BlockLayout>,
    pub(super) spacer_blocks: Vec<BlockLayout>,
    pub(super) row_block_types: FxHashMap<DisplayRow, bool>,
    pub(super) resized_blocks: Option<FxHashMap<CustomBlockId, u32>>,
}

impl EditorElement {
    fn render_block(
        &self,
        block: &Block,
        available_width: AvailableSpace,
        block_id: BlockId,
        block_row_start: DisplayRow,
        snapshot: &EditorSnapshot,
        text_x: Pixels,
        rows: &Range<DisplayRow>,
        line_layouts: &[LineWithInvisibles],
        editor_margins: &EditorMargins,
        line_height: Pixels,
        em_width: Pixels,
        editor_width: Pixels,
        resized_blocks: &mut FxHashMap<CustomBlockId, u32>,
        row_block_types: &mut FxHashMap<DisplayRow, bool>,
        selected_buffer_ids: &[BufferId],
        latest_selection_anchors: &FxHashMap<BufferId, Anchor>,
        is_row_soft_wrapped: impl Copy + Fn(usize) -> bool,
        sticky_header_excerpt_id: Option<BufferId>,
        indent_guides: Option<&[IndentGuideLayout]>,
        block_resize_offset: &mut i32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(AnyElement, Size<Pixels>, DisplayRow, Pixels)> {
        let mut x_position = None;
        let mut element = match block {
            Block::Custom(custom) => {
                let block_start = custom.start().to_point(snapshot.buffer_snapshot());
                if block.place_near() && snapshot.is_line_folded(MultiBufferRow(block_start.row)) {
                    return None;
                }
                let align_to = block_start.to_display_point(snapshot);
                let x_and_width = |layout: &LineWithInvisibles| {
                    (text_x + layout.x_for_index(align_to.column() as usize), text_x + layout.width)
                };
                let line_ix = align_to.row().0.checked_sub(rows.start.0);
                let custom_block_x_position = if let Some(layout) = line_ix.and_then(|ix| line_layouts.get(ix as usize))
                {
                    x_and_width(layout)
                } else {
                    x_and_width(&layout_line(align_to.row(), snapshot, &self.style, is_row_soft_wrapped, window, cx))
                };

                let anchor_x = custom_block_x_position.0;
                x_position = Some(custom_block_x_position);

                div()
                    .size_full()
                    .child(custom.render(&mut BlockContext {
                        window,
                        app: cx,
                        anchor_x,
                        margins: editor_margins,
                        line_height,
                        em_width,
                        block_id,
                        height: custom.height.unwrap_or(1),
                        editor_style: &self.style,
                        indent_guide_padding: indent_guides.map_or(px(0.0), |guides| {
                            Self::depth_zero_indent_guide_padding_for_row(guides, block_row_start)
                        }),
                    }))
                    .into_any()
            }

            Block::FoldedBuffer { first_excerpt, height, .. } => {
                let mut result = v_flex().id(block_id).w_full().pr(editor_margins.right);

                if self.should_show_buffer_headers() {
                    let selected = selected_buffer_ids.contains(&first_excerpt.buffer_id());
                    let jump_data = header::header_jump_data(
                        snapshot,
                        block_row_start,
                        *height,
                        first_excerpt,
                        latest_selection_anchors,
                    );
                    result = result.child(header::render_buffer_header(
                        &self.editor,
                        first_excerpt,
                        true,
                        selected,
                        false,
                        jump_data,
                        window,
                        cx,
                    ));
                } else {
                    result = result.child(div().h(FILE_HEADER_HEIGHT as f32 * window.line_height()));
                }

                result.into_any_element()
            }

            Block::ExcerptBoundary { .. } => {
                let color = theme::theme().colors().clone();
                let mut result = v_flex().id(block_id).w_full();

                result = result.child(
                    h_flex()
                        .relative()
                        .child(div().top(line_height / 2.).absolute().w_full().h_px().bg(color.border_variant)),
                );

                result.into_any()
            }

            Block::BufferHeader { excerpt, height } => {
                let mut result = v_flex().id(block_id).w_full();

                if self.should_show_buffer_headers() {
                    let jump_data =
                        header::header_jump_data(snapshot, block_row_start, *height, excerpt, latest_selection_anchors);

                    if sticky_header_excerpt_id == Some(excerpt.buffer_id()) {
                        result = result.child(div().h(FILE_HEADER_HEIGHT as f32 * window.line_height()));
                    } else {
                        let selected = selected_buffer_ids.contains(&excerpt.buffer_id());

                        result = result.child(div().pr(editor_margins.right).child(header::render_buffer_header(
                            &self.editor,
                            excerpt,
                            false,
                            selected,
                            false,
                            jump_data,
                            window,
                            cx,
                        )));
                    }
                } else {
                    result = result.child(div().h(FILE_HEADER_HEIGHT as f32 * window.line_height()));
                }

                result.into_any()
            }

            Block::Spacer { height, .. } => {
                let indent_guide_padding = indent_guides
                    .map_or(px(0.0), |guides| Self::depth_zero_indent_guide_padding_for_row(guides, block_row_start));
                Self::render_spacer_block(block_id, *height, line_height, indent_guide_padding, window)
            }
        };

        // Discover the element's content height, then round up to the nearest multiple of line height.
        let preliminary_size = element.layout_as_root(size(available_width, AvailableSpace::MinContent), window, cx);
        let quantized_height = (preliminary_size.height / line_height).ceil() * line_height;
        let final_size = if preliminary_size.height == quantized_height {
            preliminary_size
        } else {
            element.layout_as_root(size(available_width, quantized_height.into()), window, cx)
        };
        let mut element_height_in_lines = ((final_size.height / line_height).ceil() as u32).max(1);

        let effective_row_start = block_row_start.0.cast_signed() + *block_resize_offset;
        let mut row = DisplayRow(effective_row_start.max(0) as u32);

        let mut x_offset = px(0.);
        let mut is_block = true;

        if let BlockId::Custom(custom_block_id) = block_id
            && block.has_height()
        {
            if block.place_near()
                && let Some((x_target, line_width)) = x_position
            {
                let margin = em_width * 2;
                if line_width + final_size.width + margin < editor_width + editor_margins.gutter.full_width()
                    && !row_block_types.contains_key(&(row - 1))
                    && element_height_in_lines == 1
                {
                    // Render inline at end of line (for diagnostic blocks that fit)
                    x_offset = line_width + margin;
                    row = row - 1;
                    is_block = false;
                    element_height_in_lines = 0;
                    row_block_types.insert(row, is_block);
                } else {
                    let max_offset = editor_width + editor_margins.gutter.full_width() - final_size.width;
                    let min_offset = (x_target + em_width - final_size.width).max(editor_margins.gutter.full_width());
                    x_offset = x_target.min(max_offset).max(min_offset);
                }
            }
            if element_height_in_lines != block.height() {
                *block_resize_offset += element_height_in_lines.cast_signed() - block.height().cast_signed();
                resized_blocks.insert(custom_block_id, element_height_in_lines);
            }
        }
        for i in 0..element_height_in_lines {
            row_block_types.insert(row + i, is_block);
        }

        Some((element, final_size, row, x_offset))
    }

    /// The spacer pattern period must be an even factor of the line height, so
    /// that two consecutive spacer blocks can render contiguously without an
    /// obvious break in the pattern.
    ///
    /// Two consecutive spacers can appear when the other side has a diff hunk
    /// and a custom block next to each other (e.g. merge conflict buttons).
    fn spacer_pattern_period(line_height: f32, target_height: f32) -> f32 {
        let k_approx = line_height / (2.0 * target_height);
        let k_floor = (k_approx.floor() as u32).max(1);
        let k_ceil = (k_approx.ceil() as u32).max(1);

        let size_floor = line_height / (2 * k_floor) as f32;
        let size_ceil = line_height / (2 * k_ceil) as f32;

        if (size_floor - target_height).abs() <= (size_ceil - target_height).abs() { size_floor } else { size_ceil }
    }

    pub(crate) fn render_spacer_block(
        block_id: BlockId,
        block_height: u32,
        line_height: Pixels,
        indent_guide_padding: Pixels,
        window: &Window,
    ) -> AnyElement {
        let target_size = 16.0;
        let scale = window.scale_factor();
        let pattern_size = Self::spacer_pattern_period(f32::from(line_height) * scale, target_size * scale);
        let color = theme::theme().colors().panel_background;
        let background = pattern_slash(color, 2.0, pattern_size - 2.0);

        div()
            .id(block_id)
            .cursor(CursorStyle::Arrow)
            .w_full()
            .h((block_height as f32) * line_height)
            .flex()
            .flex_row()
            .child(div().flex_shrink_0().w(indent_guide_padding).h_full())
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .relative()
                    .overflow_x_hidden()
                    .child(div().absolute().top_0().bottom_0().right_0().left(-indent_guide_padding).bg(background)),
            )
            .into_any()
    }

    pub(super) fn render_blocks(
        &self,
        rows: Range<DisplayRow>,
        snapshot: &EditorSnapshot,
        hitbox: &Hitbox,
        editor_width: Pixels,
        scroll_width: &mut Pixels,
        editor_margins: &EditorMargins,
        em_width: Pixels,
        text_x: Pixels,
        line_height: Pixels,
        line_layouts: &[LineWithInvisibles],
        selected_buffer_ids: &[BufferId],
        latest_selection_anchors: &FxHashMap<BufferId, Anchor>,
        is_row_soft_wrapped: impl Copy + Fn(usize) -> bool,
        sticky_header_excerpt_id: Option<BufferId>,
        indent_guides: Option<&[IndentGuideLayout]>,
        window: &mut Window,
        cx: &mut App,
    ) -> RenderBlocksOutput {
        let (fixed_blocks, non_fixed_blocks) = snapshot
            .blocks_in_range(rows.clone())
            .partition::<Vec<_>, _>(|(_, block)| block.style() == BlockStyle::Fixed);

        let mut focused_block = self.editor.update(cx, |editor, _| editor.take_focused_block());
        let mut fixed_block_max_width = Pixels::ZERO;
        let mut blocks = Vec::new();
        let mut spacer_blocks = Vec::new();
        let mut resized_blocks = FxHashMap::default();
        let mut row_block_types = FxHashMap::default();
        let mut block_resize_offset: i32 = 0;

        for (row, block) in fixed_blocks {
            let block_id = block.id();

            if focused_block.as_ref().is_some_and(|b| b.id == block_id) {
                focused_block = None;
            }

            if let Some((element, element_size, row, x_offset)) = self.render_block(
                block,
                AvailableSpace::MinContent,
                block_id,
                row,
                snapshot,
                text_x,
                &rows,
                line_layouts,
                editor_margins,
                line_height,
                em_width,
                editor_width,
                &mut resized_blocks,
                &mut row_block_types,
                selected_buffer_ids,
                latest_selection_anchors,
                is_row_soft_wrapped,
                sticky_header_excerpt_id,
                indent_guides,
                &mut block_resize_offset,
                window,
                cx,
            ) {
                fixed_block_max_width = fixed_block_max_width.max(element_size.width + em_width);
                blocks.push(BlockLayout {
                    id: block_id,
                    x_offset,
                    row: Some(row),
                    element,
                    available_space: size(AvailableSpace::MinContent, element_size.height.into()),
                    style: BlockStyle::Fixed,
                    overlaps_gutter: true,
                    is_buffer_header: block.is_buffer_header(),
                });
            }
        }

        for (row, block) in non_fixed_blocks {
            let style = block.style();
            let width = match (style, block.place_near()) {
                (_, true) => AvailableSpace::MinContent,
                (BlockStyle::Sticky, _) => hitbox.size.width.into(),
                (BlockStyle::Flex, _) => hitbox
                    .size
                    .width
                    .max(fixed_block_max_width)
                    .max(editor_margins.gutter.width + *scroll_width + editor_margins.extended_right)
                    .into(),
                (BlockStyle::Spacer, _) => hitbox
                    .size
                    .width
                    .max(fixed_block_max_width)
                    .max(*scroll_width + editor_margins.extended_right)
                    .into(),
                (BlockStyle::Fixed, _) => unreachable!(),
            };
            let block_id = block.id();

            if focused_block.as_ref().is_some_and(|b| b.id == block_id) {
                focused_block = None;
            }

            if let Some((element, element_size, row, x_offset)) = self.render_block(
                block,
                width,
                block_id,
                row,
                snapshot,
                text_x,
                &rows,
                line_layouts,
                editor_margins,
                line_height,
                em_width,
                editor_width,
                &mut resized_blocks,
                &mut row_block_types,
                selected_buffer_ids,
                latest_selection_anchors,
                is_row_soft_wrapped,
                sticky_header_excerpt_id,
                indent_guides,
                &mut block_resize_offset,
                window,
                cx,
            ) {
                let layout = BlockLayout {
                    id: block_id,
                    x_offset,
                    row: Some(row),
                    element,
                    available_space: size(width, element_size.height.into()),
                    style,
                    overlaps_gutter: !block.place_near() && style != BlockStyle::Spacer,
                    is_buffer_header: block.is_buffer_header(),
                };
                if style == BlockStyle::Spacer {
                    spacer_blocks.push(layout);
                } else {
                    blocks.push(layout);
                }
            }
        }

        if let Some(focused_block) = focused_block
            && let Some(focus_handle) = focused_block.focus_handle.upgrade()
            && focus_handle.is_focused(window)
            && let Some(block) = snapshot.block_for_id(focused_block.id)
        {
            let style = block.style();
            let width = match style {
                BlockStyle::Fixed => AvailableSpace::MinContent,
                BlockStyle::Flex => AvailableSpace::Definite(
                    hitbox
                        .size
                        .width
                        .max(fixed_block_max_width)
                        .max(editor_margins.gutter.width + *scroll_width + editor_margins.extended_right),
                ),
                BlockStyle::Spacer => AvailableSpace::Definite(
                    hitbox.size.width.max(fixed_block_max_width).max(*scroll_width + editor_margins.extended_right),
                ),
                BlockStyle::Sticky => AvailableSpace::Definite(hitbox.size.width),
            };

            if let Some((element, element_size, _, x_offset)) = self.render_block(
                &block,
                width,
                focused_block.id,
                rows.end,
                snapshot,
                text_x,
                &rows,
                line_layouts,
                editor_margins,
                line_height,
                em_width,
                editor_width,
                &mut resized_blocks,
                &mut row_block_types,
                selected_buffer_ids,
                latest_selection_anchors,
                is_row_soft_wrapped,
                sticky_header_excerpt_id,
                indent_guides,
                &mut block_resize_offset,
                window,
                cx,
            ) {
                blocks.push(BlockLayout {
                    id: block.id(),
                    x_offset,
                    row: None,
                    element,
                    available_space: size(width, element_size.height.into()),
                    style,
                    overlaps_gutter: true,
                    is_buffer_header: block.is_buffer_header(),
                });
            }
        }

        if resized_blocks.is_empty() {
            *scroll_width = (*scroll_width).max(fixed_block_max_width - editor_margins.gutter.width);
        }

        RenderBlocksOutput {
            non_spacer_blocks: blocks,
            spacer_blocks,
            row_block_types,
            resized_blocks: (!resized_blocks.is_empty()).then_some(resized_blocks),
        }
    }

    pub(super) fn layout_blocks(
        &self,
        blocks: &mut Vec<BlockLayout>,
        hitbox: &Hitbox,
        gutter_hitbox: &Hitbox,
        line_height: Pixels,
        scroll_position: gpui::Point<ScrollOffset>,
        scroll_pixel_position: gpui::Point<ScrollPixelOffset>,
        editor_margins: &EditorMargins,
        window: &mut Window,
        cx: &mut App,
    ) {
        for block in blocks {
            let mut origin = if let Some(row) = block.row {
                hitbox.origin
                    + point(
                        block.x_offset,
                        Pixels::from((row.as_f64() - scroll_position.y) * ScrollPixelOffset::from(line_height)),
                    )
            } else {
                // Position the block outside the visible area
                hitbox.origin + point(Pixels::ZERO, hitbox.size.height)
            };

            if block.style == BlockStyle::Spacer {
                origin += point(gutter_hitbox.size.width + editor_margins.gutter.margin, Pixels::ZERO);
            }

            if !matches!(block.style, BlockStyle::Sticky) {
                origin += point(Pixels::from(-scroll_pixel_position.x), Pixels::ZERO);
            }

            let focus_handle = block.element.prepaint_as_root(origin, block.available_space, window, cx);

            if let Some(focus_handle) = focus_handle {
                self.editor.update(cx, |editor, _cx| {
                    editor.set_focused_block(FocusedBlock { id: block.id, focus_handle: focus_handle.downgrade() });
                });
            }
        }
    }

    pub(super) fn paint_spacer_blocks(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        for mut block in layout.spacer_blocks.drain(..) {
            let mut bounds = layout.hitbox.bounds;
            bounds.origin.x += layout.gutter_hitbox.bounds.size.width;
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                block.element.paint(window, cx);
            });
        }
    }

    pub(super) fn paint_non_spacer_blocks(&self, layout: &mut EditorLayout, window: &mut Window, cx: &mut App) {
        for mut block in layout.blocks.drain(..) {
            if block.overlaps_gutter {
                block.element.paint(window, cx);
            } else {
                let mut bounds = layout.hitbox.bounds;
                bounds.origin.x += layout.gutter_hitbox.bounds.size.width;
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    block.element.paint(window, cx);
                });
            }
        }
    }
}

pub(crate) struct BlockLayout {
    id: BlockId,
    x_offset: Pixels,
    pub(crate) row: Option<DisplayRow>,
    element: AnyElement,
    available_space: Size<AvailableSpace>,
    style: BlockStyle,
    overlaps_gutter: bool,
    pub(crate) is_buffer_header: bool,
}
