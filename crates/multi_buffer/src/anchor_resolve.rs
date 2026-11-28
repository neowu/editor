use std::cmp;
use std::iter;
use std::ops::Add;
use std::ops::AddAssign;
use std::ops::Range;
use std::ops::Sub;

use language::BufferSnapshot;
use language::Point;
use language::ToOffset as _;
use rope::DimensionPair;
use sum_tree::Bias;
use sum_tree::Cursor;
use sum_tree::Dimensions;
use text::BufferId;
use text::TextSummary;

use super::Anchor;
use super::DiffTransform;
use super::ExcerptAnchor;
use super::ExcerptRange;
use super::ExcerptSummary;
use super::MBTextSummary;
use super::MultiBufferDimension;
use super::MultiBufferOffset;
use super::MultiBufferSnapshot;
use super::PathKey;
use super::ToOffset;
use super::dimension::ExcerptDimension;
use super::dimension::ExcerptOffset;
use super::dimension::OutputDimension;

impl MultiBufferSnapshot {
    pub(crate) fn summary_for_anchor<MBD>(&self, anchor: &Anchor) -> MBD
    where
        MBD: MultiBufferDimension
            + Ord
            + Sub<Output = MBD::TextDimension>
            + Sub<MBD::TextDimension, Output = MBD>
            + AddAssign<MBD::TextDimension>
            + Add<MBD::TextDimension, Output = MBD>,
        MBD::TextDimension: Sub<Output = MBD::TextDimension> + Ord,
    {
        let target = anchor.seek_target(self);
        let anchor = match anchor {
            Anchor::Min => {
                return MBD::default();
            }
            Anchor::Excerpt(excerpt_anchor) => excerpt_anchor,
            Anchor::Max => {
                return MBD::from_summary(&self.text_summary());
            }
        };

        let (start, _, item) = self.excerpts.find::<ExcerptSummary, _>((), &target, Bias::Left);
        let start = MBD::from_summary(&start.text);

        let excerpt_start_position = ExcerptDimension(start);
        if self.diff_transforms.is_empty() {
            if let Some(excerpt) = item {
                if !excerpt.contains(anchor, self) {
                    return excerpt_start_position.0;
                }
                let buffer_snapshot = excerpt.buffer_snapshot(self);
                let excerpt_buffer_start = excerpt.range.context.start.summary::<MBD::TextDimension>(buffer_snapshot);
                let excerpt_buffer_end = excerpt.range.context.end.summary::<MBD::TextDimension>(buffer_snapshot);
                let buffer_summary = anchor.text_anchor().summary::<MBD::TextDimension>(buffer_snapshot);
                let summary = cmp::min(excerpt_buffer_end, buffer_summary);
                let mut position = excerpt_start_position;
                if summary > excerpt_buffer_start {
                    position += summary - excerpt_buffer_start;
                }

                position.0
            } else {
                excerpt_start_position.0
            }
        } else {
            let mut diff_transforms_cursor =
                self.diff_transforms.cursor::<Dimensions<ExcerptDimension<MBD>, OutputDimension<MBD>>>(());

            if let Some(excerpt) = item {
                if !excerpt.contains(anchor, self) {
                    diff_transforms_cursor.seek(&excerpt_start_position, Bias::Left);
                    return self.summary_for_excerpt_position_without_hunks(
                        Bias::Left,
                        excerpt_start_position,
                        &mut diff_transforms_cursor,
                    );
                }
                let buffer_snapshot = excerpt.buffer_snapshot(self);
                let excerpt_buffer_start = excerpt.range.context.start.summary::<MBD::TextDimension>(buffer_snapshot);
                let excerpt_buffer_end = excerpt.range.context.end.summary::<MBD::TextDimension>(buffer_snapshot);
                let buffer_summary = anchor.text_anchor().summary::<MBD::TextDimension>(buffer_snapshot);
                let summary = cmp::min(excerpt_buffer_end, buffer_summary);
                let mut position = excerpt_start_position;
                if summary > excerpt_buffer_start {
                    position += summary - excerpt_buffer_start;
                }

                diff_transforms_cursor.seek(&position, Bias::Left);
                self.summary_for_anchor_with_excerpt_position(
                    *anchor,
                    position,
                    &mut diff_transforms_cursor,
                    buffer_snapshot,
                )
            } else {
                diff_transforms_cursor.seek(&excerpt_start_position, Bias::Left);
                self.summary_for_excerpt_position_without_hunks(
                    Bias::Right,
                    excerpt_start_position,
                    &mut diff_transforms_cursor,
                )
            }
        }
    }

    /// Maps an anchor's excerpt-space position to its output-space position by
    /// walking the diff transforms. The cursor is shared across consecutive
    /// calls, so it may already be partway through the transform list.
    fn summary_for_anchor_with_excerpt_position<MBD>(
        &self,
        anchor: ExcerptAnchor,
        excerpt_position: ExcerptDimension<MBD>,
        diff_transforms: &mut Cursor<DiffTransform, Dimensions<ExcerptDimension<MBD>, OutputDimension<MBD>>>,
        excerpt_buffer: &text::BufferSnapshot,
    ) -> MBD
    where
        MBD: MultiBufferDimension + Ord + Sub + AddAssign<<MBD as Sub>::Output>,
    {
        loop {
            let transform_end_position = diff_transforms.end().0;
            let item = diff_transforms.item();
            let at_transform_end = transform_end_position == excerpt_position && item.is_some();

            // A right-biased anchor at a transform boundary belongs to the
            // *next* transform, so advance past the current one.
            if anchor.text_anchor.bias == Bias::Right && at_transform_end {
                diff_transforms.next();
                continue;
            }

            let mut position = diff_transforms.start().1;
            if let Some(DiffTransform::DeletedHunk { buffer_id, base_text_byte_range, hunk_info, .. }) = item {
                let base_text = self.diff_state(*buffer_id).map(|diff| diff.base_text());
                if let Some(diff_base_anchor) = anchor.diff_base_anchor
                    && let Some(base_text) = base_text
                    && base_text.can_resolve(&diff_base_anchor)
                {
                    // The anchor carries a diff-base position — resolve it
                    // to a location inside the deleted hunk.
                    let base_text_offset = diff_base_anchor.to_offset(base_text);
                    if base_text_offset >= base_text_byte_range.start && base_text_offset <= base_text_byte_range.end {
                        let position_in_hunk = base_text.text_summary_for_range::<MBD::TextDimension, _>(
                            base_text_byte_range.start..base_text_offset,
                        );
                        position.0.add_text_dim(&position_in_hunk);
                    } else if at_transform_end && base_text_offset > base_text_byte_range.end {
                        // diff_base offset falls outside this hunk's range;
                        // advance to see if the next transform is a better fit.
                        diff_transforms.next();
                        continue;
                    }
                } else {
                    let departed_base_sorts_after =
                        anchor.diff_base_anchor.zip(base_text).is_some_and(|(diff_base_anchor, base_text)| {
                            diff_base_anchor.buffer_id > base_text.buffer_id()
                        });
                    if at_transform_end
                        && (departed_base_sorts_after
                            || anchor.text_anchor().cmp(&hunk_info.hunk_start_anchor, excerpt_buffer).is_gt())
                    {
                        diff_transforms.next();
                        continue;
                    }
                }
            } else {
                // On a BufferContent (or no transform). If the anchor
                // carries a diff_base_anchor it needs a DeletedHunk, so
                // advance to find one.
                if at_transform_end
                    && (anchor.diff_base_anchor.is_some()
                        || matches!(
                            diff_transforms.next_item(),
                            Some(DiffTransform::DeletedHunk { buffer_id, .. })
                                if *buffer_id == anchor.text_anchor.buffer_id
                        ))
                {
                    diff_transforms.next();
                    continue;
                }
                let overshoot = excerpt_position - diff_transforms.start().0;
                position += overshoot;
            }

            return position.0;
        }
    }

    /// Like `resolve_summary_for_anchor` but optimized for min/max anchors.
    fn summary_for_excerpt_position_without_hunks<MBD>(
        &self,
        bias: Bias,
        excerpt_position: ExcerptDimension<MBD>,
        diff_transforms: &mut Cursor<DiffTransform, Dimensions<ExcerptDimension<MBD>, OutputDimension<MBD>>>,
    ) -> MBD
    where
        MBD: MultiBufferDimension + Ord + Sub + AddAssign<<MBD as Sub>::Output>,
    {
        loop {
            let transform_end_position = diff_transforms.end().0;
            let item = diff_transforms.item();
            let at_transform_end = transform_end_position == excerpt_position && item.is_some();

            // A right-biased anchor at a transform boundary belongs to the
            // *next* transform, so advance past the current one.
            if bias == Bias::Right && at_transform_end {
                diff_transforms.next();
                continue;
            }

            let mut position = diff_transforms.start().1;
            if let Some(DiffTransform::BufferContent { .. }) | None = item {
                let overshoot = excerpt_position - diff_transforms.start().0;
                position += overshoot;
            }

            return position.0;
        }
    }

    pub(super) fn excerpt_offset_for_anchor(&self, anchor: &Anchor) -> ExcerptOffset {
        let anchor = match anchor {
            Anchor::Min => return ExcerptOffset::default(),
            Anchor::Excerpt(excerpt_anchor) => excerpt_anchor,
            Anchor::Max => return self.excerpts.summary().len(),
        };
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        let target = anchor.seek_target(self);

        cursor.seek(&target, Bias::Left);

        let mut position = cursor.start().len();
        if let Some(excerpt) = cursor.item()
            && excerpt.contains(anchor, self)
        {
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            let excerpt_buffer_start = buffer_snapshot.offset_for_anchor(&excerpt.range.context.start);
            let excerpt_buffer_end = buffer_snapshot.offset_for_anchor(&excerpt.range.context.end);
            let buffer_position =
                cmp::min(excerpt_buffer_end, buffer_snapshot.offset_for_anchor(&anchor.text_anchor()));
            if buffer_position > excerpt_buffer_start {
                position += buffer_position - excerpt_buffer_start;
            }
        }
        position
    }

    pub(crate) fn summaries_for_anchors<'a, MBD, I>(&'a self, anchors: I) -> Vec<MBD>
    where
        MBD: MultiBufferDimension + Ord + Sub<Output = MBD::TextDimension> + AddAssign<MBD::TextDimension>,
        MBD::TextDimension: Sub<Output = MBD::TextDimension> + Ord,
        I: 'a + IntoIterator<Item = &'a Anchor>,
    {
        let mut summaries = Vec::new();
        self.summaries_for_anchors_cb(anchors, |summary| summaries.push(summary));
        summaries
    }

    pub fn summaries_for_anchors_cb<'a, MBD, I>(&'a self, anchors: I, mut cb: impl FnMut(MBD))
    where
        MBD: MultiBufferDimension + Ord + Sub<Output = MBD::TextDimension> + AddAssign<MBD::TextDimension>,
        MBD::TextDimension: Sub<Output = MBD::TextDimension> + Ord,
        I: 'a + IntoIterator<Item = &'a Anchor>,
    {
        let mut anchors = anchors.into_iter().peekable();
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        let mut diff_transforms_cursor =
            self.diff_transforms.cursor::<Dimensions<ExcerptDimension<MBD>, OutputDimension<MBD>>>(());
        diff_transforms_cursor.next();

        while let Some(anchor) = anchors.peek() {
            let target = anchor.seek_target(self);
            let excerpt_anchor = match anchor {
                Anchor::Min => {
                    cb(MBD::default());
                    anchors.next();
                    continue;
                }
                Anchor::Excerpt(excerpt_anchor) => excerpt_anchor,
                Anchor::Max => {
                    cb(MBD::from_summary(&self.text_summary()));
                    anchors.next();
                    continue;
                }
            };

            cursor.seek_forward(&target, Bias::Left);

            let excerpt_start_position = ExcerptDimension(MBD::from_summary(&cursor.start().text));
            if let Some(excerpt) = cursor.item() {
                let buffer_snapshot = excerpt.buffer_snapshot(self);
                if !excerpt.contains(excerpt_anchor, self) {
                    diff_transforms_cursor.seek_forward(&excerpt_start_position, Bias::Left);
                    let position = self.summary_for_excerpt_position_without_hunks(
                        Bias::Left,
                        excerpt_start_position,
                        &mut diff_transforms_cursor,
                    );
                    cb(position);
                    anchors.next();
                    continue;
                }
                let excerpt_buffer_start = excerpt.range.context.start.summary::<MBD::TextDimension>(buffer_snapshot);
                let excerpt_buffer_end = excerpt.range.context.end.summary::<MBD::TextDimension>(buffer_snapshot);
                for (buffer_summary, excerpt_anchor) in buffer_snapshot
                    .summaries_for_anchors_with_payload::<MBD::TextDimension, _, _>(iter::from_fn(|| {
                        let excerpt_anchor = anchors.peek()?.excerpt_anchor()?;
                        if !excerpt.contains(&excerpt_anchor, self) {
                            return None;
                        }
                        anchors.next();
                        Some((excerpt_anchor.text_anchor(), excerpt_anchor))
                    }))
                {
                    let summary = cmp::min(excerpt_buffer_end, buffer_summary);
                    let mut position = excerpt_start_position;
                    if summary > excerpt_buffer_start {
                        position += summary - excerpt_buffer_start;
                    }

                    if diff_transforms_cursor.start().0 < position {
                        diff_transforms_cursor.seek_forward(&position, Bias::Left);
                    }

                    cb(self.summary_for_anchor_with_excerpt_position(
                        excerpt_anchor,
                        position,
                        &mut diff_transforms_cursor,
                        buffer_snapshot,
                    ));
                }
            } else {
                diff_transforms_cursor.seek_forward(&excerpt_start_position, Bias::Left);
                let position = self.summary_for_excerpt_position_without_hunks(
                    Bias::Right,
                    excerpt_start_position,
                    &mut diff_transforms_cursor,
                );
                cb(position);
                anchors.next();
            }
        }
    }

    pub fn dimensions_from_points<'a, MBD>(
        &'a self,
        points: impl 'a + IntoIterator<Item = Point>,
    ) -> impl 'a + Iterator<Item = MBD>
    where
        MBD: MultiBufferDimension + Sub + AddAssign<<MBD as Sub>::Output>,
    {
        let mut cursor = self.cursor::<DimensionPair<Point, MBD>, Point>();
        cursor.seek(&DimensionPair { key: Point::default(), value: None });
        let mut points = points.into_iter();
        iter::from_fn(move || {
            let point = points.next()?;

            cursor.seek_forward(&DimensionPair { key: point, value: None });

            if let Some(region) = cursor.region() {
                let overshoot = point - region.range.start.key;
                let buffer_point = region.buffer_range.start + overshoot;
                let mut position = region.range.start.value.expect("seeked region has a value");
                position.add_text_dim(&region.buffer.text_summary_for_range(region.buffer_range.start..buffer_point));
                if point == region.range.end.key && region.has_trailing_newline {
                    position.add_mb_text_summary(&MBTextSummary::from(TextSummary::newline()));
                }
                Some(position)
            } else {
                Some(MBD::from_summary(&self.text_summary()))
            }
        })
    }

    pub fn excerpts_for_buffer(&self, buffer_id: BufferId) -> impl Iterator<Item = ExcerptRange<text::Anchor>> {
        if let Some(buffer_state) = self.buffers.get(&buffer_id) {
            let path_key = buffer_state.path_key.clone();
            let mut cursor = self.excerpts.cursor::<PathKey>(());
            cursor.seek_forward(&path_key, Bias::Left);
            Some(iter::from_fn(move || {
                let excerpt = cursor.item()?;
                if excerpt.path_key != path_key {
                    return None;
                }
                cursor.next();
                Some(excerpt.range.clone())
            }))
        } else {
            None
        }
        .into_iter()
        .flatten()
    }

    pub fn anchor_before<T: ToOffset>(&self, position: T) -> Anchor {
        self.anchor_at(position, Bias::Left)
    }

    pub fn anchor_after<T: ToOffset>(&self, position: T) -> Anchor {
        self.anchor_at(position, Bias::Right)
    }

    pub fn anchor_at<T: ToOffset>(&self, position: T, mut bias: Bias) -> Anchor {
        let offset = position.to_offset(self);

        // Find the given position in the diff transforms. Determine the corresponding
        // offset in the excerpts, and whether the position is within a deleted hunk.
        let mut diff_transforms = self.diff_transforms.cursor::<Dimensions<MultiBufferOffset, ExcerptOffset>>(());
        diff_transforms.seek(&offset, Bias::Right);

        if offset == diff_transforms.start().0
            && bias == Bias::Left
            && let Some(prev_item) = diff_transforms.prev_item()
            && let DiffTransform::DeletedHunk { .. } = prev_item
        {
            diff_transforms.prev();
        }
        let offset_in_transform = offset - diff_transforms.start().0;
        let mut excerpt_offset = diff_transforms.start().1;
        let mut diff_base_anchor = None;
        if let Some(DiffTransform::DeletedHunk { buffer_id, base_text_byte_range, .. }) = diff_transforms.item() {
            let diff = self.diff_state(*buffer_id).expect("missing diff");
            if offset_in_transform > base_text_byte_range.len() {
                bias = Bias::Right;
            } else {
                diff_base_anchor =
                    Some(diff.base_text().anchor_at(base_text_byte_range.start + offset_in_transform, bias));
                bias = Bias::Left;
            }
        } else {
            excerpt_offset += MultiBufferOffset(offset_in_transform);
        }

        let mut excerpts = self.excerpts.cursor::<Dimensions<ExcerptOffset, ExcerptSummary>>(());
        excerpts.seek(&excerpt_offset, Bias::Right);
        if excerpts.item().is_none() && excerpt_offset == excerpts.start().0 && bias == Bias::Left {
            excerpts.prev();
        }
        if let Some(excerpt) = excerpts.item() {
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            let mut overshoot = excerpt_offset.saturating_sub(excerpts.start().0);
            if excerpt.has_trailing_newline && excerpt_offset == excerpts.end().0 {
                overshoot -= 1;
                bias = Bias::Right;
            }

            let buffer_start = excerpt.range.context.start.to_offset(buffer_snapshot);
            let text_anchor = excerpt.clip_anchor(buffer_snapshot.anchor_at(buffer_start + overshoot, bias), self);
            let anchor = ExcerptAnchor::in_buffer(excerpt.path_key_index, text_anchor);
            let anchor = match diff_base_anchor {
                Some(diff_base_anchor) => anchor.with_diff_base_anchor(diff_base_anchor),
                None => anchor,
            };
            anchor.into()
        } else if excerpt_offset == ExcerptDimension(MultiBufferOffset::ZERO) && bias == Bias::Left {
            Anchor::Min
        } else {
            Anchor::Max
        }
    }

    /// Lifts a buffer anchor to a multibuffer anchor without checking against excerpt boundaries. Returns `None` if there are no excerpts for the buffer.
    pub fn anchor_in_buffer(&self, anchor: text::Anchor) -> Option<Anchor> {
        let path_key_index = self.path_key_index_for_buffer(anchor.buffer_id)?;
        Some(Anchor::in_buffer(path_key_index, anchor))
    }

    /// Lifts a buffer anchor range to a multibuffer anchor range without checking against excerpt boundaries. Returns `None` if there are no excerpts for the buffer.
    pub fn anchor_range_in_buffer(&self, range: Range<text::Anchor>) -> Option<Range<Anchor>> {
        if range.start.buffer_id != range.end.buffer_id {
            return None;
        }

        let path_key_index = self.path_key_index_for_buffer(range.start.buffer_id)?;
        Some(Anchor::range_in_buffer(path_key_index, range))
    }

    /// Creates a multibuffer anchor for the given buffer anchor, if it is contained in any excerpt.
    pub fn anchor_in_excerpt(&self, text_anchor: text::Anchor) -> Option<Anchor> {
        let excerpts = {
            let buffer_id = text_anchor.buffer_id;
            if let Some(buffer_state) = self.buffers.get(&buffer_id) {
                let path_key = buffer_state.path_key.clone();
                let mut cursor = self.excerpts.cursor::<PathKey>(());
                cursor.seek_forward(&path_key, Bias::Left);
                Some(iter::from_fn(move || {
                    let excerpt = cursor.item()?;
                    if excerpt.path_key != path_key {
                        return None;
                    }
                    cursor.next();
                    Some(excerpt)
                }))
            } else {
                None
            }
            .into_iter()
            .flatten()
        };
        for excerpt in excerpts {
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            if excerpt.range.contains(&text_anchor, buffer_snapshot) {
                return Some(Anchor::in_buffer(excerpt.path_key_index, text_anchor));
            }
        }

        None
    }

    /// Creates a multibuffer anchor for the given buffer anchor, if it is contained in any excerpt.
    pub fn buffer_anchor_range_to_anchor_range(&self, text_anchor: Range<text::Anchor>) -> Option<Range<Anchor>> {
        if self.is_singleton() {
            let excerpt = self.excerpts.first()?;
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            if excerpt.range.contains(&text_anchor.start, buffer_snapshot)
                && excerpt.range.contains(&text_anchor.end, buffer_snapshot)
            {
                return Some(Anchor::range_in_buffer(excerpt.path_key_index, text_anchor));
            }
        }

        // for each search match

        let mut buffer_snapshot = None;
        for excerpt in {
            let this = &self;
            let buffer_id = text_anchor.start.buffer_id;
            if let Some(buffer_state) = this.buffers.get(&buffer_id) {
                let path_key = buffer_state.path_key.clone();
                let mut cursor = this.excerpts.cursor::<PathKey>(());
                cursor.seek_forward(&path_key, Bias::Left);
                Some(iter::from_fn(move || {
                    let excerpt = cursor.item()?;
                    if excerpt.path_key != path_key {
                        return None;
                    }
                    cursor.next();
                    Some(excerpt)
                }))
            } else {
                None
            }
            .into_iter()
            .flatten()
        } {
            let buffer_snapshot = buffer_snapshot.get_or_insert_with(|| excerpt.buffer_snapshot(self));
            if excerpt.range.contains(&text_anchor.start, buffer_snapshot)
                && excerpt.range.contains(&text_anchor.end, buffer_snapshot)
            {
                return Some(Anchor::range_in_buffer(excerpt.path_key_index, text_anchor));
            }
        }

        None
    }

    /// Converts ranges from one buffer in start-anchor order without restarting the excerpt scan
    /// for every range. Unordered or mixed-buffer input falls back to independent conversions.
    pub fn ordered_buffer_anchor_ranges_to_anchor_ranges(
        &self,
        text_anchor_ranges: impl IntoIterator<Item = Range<text::Anchor>>,
    ) -> Vec<Option<Range<Anchor>>> {
        let text_anchor_ranges = text_anchor_ranges.into_iter().collect::<Vec<_>>();
        let Some(first_range) = text_anchor_ranges.first() else {
            return Vec::new();
        };
        let buffer_id = first_range.start.buffer_id;
        let ranges_share_buffer = text_anchor_ranges
            .iter()
            .all(|range| range.start.buffer_id == buffer_id && range.end.buffer_id == buffer_id);
        if !ranges_share_buffer {
            return text_anchor_ranges
                .into_iter()
                .map(|range| self.buffer_anchor_range_to_anchor_range(range))
                .collect();
        }

        let Some(buffer_state) = self.buffers.get(&buffer_id) else {
            return vec![None; text_anchor_ranges.len()];
        };
        let buffer_snapshot = &buffer_state.buffer_snapshot;
        let ranges_are_ordered = text_anchor_ranges.is_sorted_by(|a, b| a.start.cmp(&b.start, buffer_snapshot).is_le());
        if !ranges_are_ordered {
            return text_anchor_ranges
                .into_iter()
                .map(|range| self.buffer_anchor_range_to_anchor_range(range))
                .collect();
        }

        let path_key = buffer_state.path_key.clone();
        let mut cursor = self.excerpts.cursor::<PathKey>(());
        cursor.seek_forward(&path_key, Bias::Left);
        let excerpts = iter::from_fn(move || {
            let excerpt = cursor.item()?;
            if excerpt.path_key != path_key {
                return None;
            }
            cursor.next();
            Some(excerpt)
        })
        .collect::<Vec<_>>();
        let mut first_candidate_index = 0;
        text_anchor_ranges
            .into_iter()
            .map(|range| {
                while let Some(excerpt) = excerpts.get(first_candidate_index) {
                    let buffer_snapshot = excerpt.buffer_snapshot(self);
                    if excerpt.range.context.end.cmp(&range.start, buffer_snapshot).is_lt() {
                        first_candidate_index += 1;
                        continue;
                    }
                    break;
                }

                for excerpt in &excerpts[first_candidate_index..] {
                    let buffer_snapshot = excerpt.buffer_snapshot(self);
                    if excerpt.range.context.start.cmp(&range.start, buffer_snapshot).is_gt() {
                        break;
                    }
                    if excerpt.range.contains(&range.start, buffer_snapshot)
                        && excerpt.range.contains(&range.end, buffer_snapshot)
                    {
                        return Some(Anchor::range_in_buffer(excerpt.path_key_index, range));
                    }
                }
                None
            })
            .collect()
    }

    /// Returns a buffer anchor and its buffer snapshot for the given anchor, if it is in the multibuffer.
    pub fn anchor_to_buffer_anchor(&self, anchor: Anchor) -> Option<(text::Anchor, &BufferSnapshot)> {
        match anchor {
            Anchor::Min => {
                let excerpt = self.excerpts.first()?;
                let buffer = excerpt.buffer_snapshot(self);
                Some((excerpt.range.context.start, buffer))
            }
            Anchor::Excerpt(excerpt_anchor) => {
                let buffer = self.buffer_for_id(excerpt_anchor.buffer_id())?;
                Some((excerpt_anchor.text_anchor, buffer))
            }
            Anchor::Max => {
                let excerpt = self.excerpts.last()?;
                let buffer = excerpt.buffer_snapshot(self);
                Some((excerpt.range.context.end, buffer))
            }
        }
    }

    pub fn can_resolve(&self, anchor: &Anchor) -> bool {
        match anchor {
            // todo(lw): should be `!self.is_empty()`
            Anchor::Min | Anchor::Max => true,
            Anchor::Excerpt(excerpt_anchor) => {
                let Some(target) = excerpt_anchor.try_seek_target(self) else {
                    return false;
                };
                let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
                cursor.seek(&target, Bias::Left);
                let Some(excerpt) = cursor.item() else {
                    return false;
                };
                excerpt.buffer_snapshot(self).can_resolve(&excerpt_anchor.text_anchor())
            }
        }
    }
}
