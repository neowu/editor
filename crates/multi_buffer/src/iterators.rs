use std::cmp;
use std::io;
use std::ops::Range;
use std::str;

use language::BufferChunks;
use language::Chunk;
use language::LanguageAwareStyling;
use language::Point;
use language::ToPoint as _;
use sum_tree::Bias;
use sum_tree::Cursor;
use sum_tree::Dimensions;
use text::BufferId;

use super::Anchor;
use super::BufferOffset;
use super::DiffTransform;
use super::Excerpt;
use super::ExcerptAnchor;
use super::ExpandExcerptDirection;
use super::ExpandInfo;
use super::MultiBufferOffset;
use super::MultiBufferRow;
use super::MultiBufferSnapshot;
use super::RowInfo;
use super::cursor::MultiBufferCursor;
use super::dimension::ExcerptOffset;
use super::find_diff_state;

#[derive(Clone)]
pub struct MultiBufferRows<'a> {
    pub(super) point: Point,
    pub(super) is_empty: bool,
    pub(super) is_singleton: bool,
    pub(super) cursor: MultiBufferCursor<'a, Point, Point>,
}

pub struct MultiBufferChunks<'a> {
    pub(super) excerpts: Cursor<'a, 'static, Excerpt, ExcerptOffset>,
    pub(super) diff_transforms: Cursor<'a, 'static, DiffTransform, Dimensions<MultiBufferOffset, ExcerptOffset>>,
    pub(super) diff_base_chunks: Option<(BufferId, BufferChunks<'a>)>,
    pub(super) buffer_chunk: Option<Chunk<'a>>,
    pub(super) range: Range<MultiBufferOffset>,
    pub(super) excerpt_offset_range: Range<ExcerptOffset>,
    pub(super) excerpt_chunks: Option<ExcerptChunks<'a>>,
    pub(super) language_aware: LanguageAwareStyling,
    pub(super) snapshot: &'a MultiBufferSnapshot,
}

pub(crate) struct ReversedMultiBufferChunks<'a> {
    pub(super) cursor: MultiBufferCursor<'a, MultiBufferOffset, BufferOffset>,
    pub(super) current_chunks: Option<rope::Chunks<'a>>,
    pub(super) start: MultiBufferOffset,
    pub(super) offset: MultiBufferOffset,
}

pub struct MultiBufferBytes<'a> {
    pub(super) range: Range<MultiBufferOffset>,
    pub(super) cursor: MultiBufferCursor<'a, MultiBufferOffset, BufferOffset>,
    pub(super) excerpt_bytes: Option<text::Bytes<'a>>,
    pub(super) has_trailing_newline: bool,
    pub(super) chunk: &'a [u8],
}

pub struct ReversedMultiBufferBytes<'a> {
    pub(super) range: Range<MultiBufferOffset>,
    pub(super) chunks: ReversedMultiBufferChunks<'a>,
    pub(super) chunk: &'a [u8],
}

pub(super) struct ExcerptChunks<'a> {
    pub(super) content_chunks: BufferChunks<'a>,
    pub(super) end: ExcerptAnchor,
    pub(super) has_footer: bool,
}

impl MultiBufferRows<'_> {
    pub fn seek(&mut self, MultiBufferRow(row): MultiBufferRow) {
        self.point = Point::new(row, 0);
        self.cursor.seek(&self.point);
    }
}

impl Iterator for MultiBufferRows<'_> {
    type Item = RowInfo;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_empty && self.point.row == 0 {
            self.point += Point::new(1, 0);
            return Some(RowInfo {
                buffer_id: None,
                buffer_row: Some(0),
                multibuffer_row: Some(MultiBufferRow(0)),
                diff_status: None,
                expand_info: None,
                wrapped_buffer_row: None,
            });
        }

        let mut region = self.cursor.region()?.clone();
        while self.point >= region.range.end {
            self.cursor.next();
            if let Some(next_region) = self.cursor.region() {
                region = next_region.clone();
            } else if self.point == self.cursor.diff_transforms.end().output_dimension.0 {
                let multibuffer_row = MultiBufferRow(self.point.row);
                let last_excerpt = self.cursor.excerpts.item().or_else(|| self.cursor.excerpts.prev_item())?;
                let buffer_snapshot = last_excerpt.buffer_snapshot(self.cursor.snapshot);
                let last_row = last_excerpt.range.context.end.to_point(buffer_snapshot).row;

                let first_row = last_excerpt.range.context.start.to_point(buffer_snapshot).row;

                let expand_info = if self.is_singleton {
                    None
                } else {
                    let needs_expand_up = first_row == last_row
                        && last_row > 0
                        && !region.diff_hunk_status.is_some_and(|d| d.is_deleted());
                    let needs_expand_down = last_row < buffer_snapshot.max_point().row;

                    if needs_expand_up && needs_expand_down {
                        Some(ExpandExcerptDirection::UpAndDown)
                    } else if needs_expand_up {
                        Some(ExpandExcerptDirection::Up)
                    } else if needs_expand_down {
                        Some(ExpandExcerptDirection::Down)
                    } else {
                        None
                    }
                    .map(|direction| ExpandInfo {
                        direction,
                        start_anchor: Anchor::Excerpt(last_excerpt.start_anchor()),
                    })
                };
                self.point += Point::new(1, 0);
                return Some(RowInfo {
                    buffer_id: Some(last_excerpt.buffer_id),
                    buffer_row: Some(last_row),
                    multibuffer_row: Some(multibuffer_row),
                    diff_status: None,
                    wrapped_buffer_row: None,
                    expand_info,
                });
            } else {
                return None;
            }
        }

        let overshoot = self.point - region.range.start;
        let buffer_point = region.buffer_range.start + overshoot;
        let expand_info = if self.is_singleton {
            None
        } else {
            let needs_expand_up = self.point.row == region.range.start.row
                && self.cursor.is_at_start_of_excerpt()
                && buffer_point.row > 0;
            let needs_expand_down = (region.excerpt.has_trailing_newline && self.point.row + 1 == region.range.end.row
                || !region.excerpt.has_trailing_newline && self.point.row == region.range.end.row)
                && self.cursor.is_at_end_of_excerpt()
                && buffer_point.row < region.buffer.max_point().row;

            if needs_expand_up && needs_expand_down {
                Some(ExpandExcerptDirection::UpAndDown)
            } else if needs_expand_up {
                Some(ExpandExcerptDirection::Up)
            } else if needs_expand_down {
                Some(ExpandExcerptDirection::Down)
            } else {
                None
            }
            .map(|direction| ExpandInfo { direction, start_anchor: Anchor::Excerpt(region.excerpt.start_anchor()) })
        };

        let result = Some(RowInfo {
            buffer_id: Some(region.buffer.buffer_id()),
            buffer_row: Some(buffer_point.row),
            multibuffer_row: Some(MultiBufferRow(self.point.row)),
            diff_status: region.diff_hunk_status.filter(|_| self.point < region.range.end),
            expand_info,
            wrapped_buffer_row: None,
        });
        self.point += Point::new(1, 0);
        result
    }
}

impl<'a> MultiBufferChunks<'a> {
    pub const fn offset(&self) -> MultiBufferOffset {
        self.range.start
    }

    pub fn seek(&mut self, range: Range<MultiBufferOffset>) {
        self.diff_transforms.seek(&range.end, Bias::Right);
        let mut excerpt_end = self.diff_transforms.start().1;
        if let Some(DiffTransform::BufferContent { .. }) = self.diff_transforms.item() {
            let overshoot = range.end - self.diff_transforms.start().0;
            excerpt_end += overshoot;
        }

        self.diff_transforms.seek(&range.start, Bias::Right);
        let mut excerpt_start = self.diff_transforms.start().1;
        if let Some(DiffTransform::BufferContent { .. }) = self.diff_transforms.item() {
            let overshoot = range.start - self.diff_transforms.start().0;
            excerpt_start += overshoot;
        }

        self.seek_to_excerpt_offset_range(excerpt_start..excerpt_end);
        self.buffer_chunk.take();
        self.range = range;
    }

    fn seek_to_excerpt_offset_range(&mut self, new_range: Range<ExcerptOffset>) {
        self.excerpt_offset_range = new_range.clone();
        self.excerpts.seek(&new_range.start, Bias::Right);
        if let Some(excerpt) = self.excerpts.item() {
            let excerpt_start = *self.excerpts.start();
            if let Some(excerpt_chunks) =
                self.excerpt_chunks.as_mut().filter(|chunks| excerpt.end_anchor() == chunks.end)
            {
                excerpt.seek_chunks(
                    excerpt_chunks,
                    (self.excerpt_offset_range.start - excerpt_start)..(self.excerpt_offset_range.end - excerpt_start),
                    self.snapshot,
                );
            } else {
                self.excerpt_chunks = Some(excerpt.chunks_in_range(
                    (self.excerpt_offset_range.start - excerpt_start)..(self.excerpt_offset_range.end - excerpt_start),
                    self.language_aware,
                    self.snapshot,
                ));
            }
        } else {
            self.excerpt_chunks = None;
        }
    }

    fn next_excerpt_chunk(&mut self) -> Option<Chunk<'a>> {
        loop {
            if self.excerpt_offset_range.is_empty() {
                return None;
            }
            if let Some(chunk) = self.excerpt_chunks.as_mut()?.next() {
                self.excerpt_offset_range.start += chunk.text.len();
                return Some(chunk);
            }
            self.excerpts.next();
            let excerpt = self.excerpts.item()?;
            self.excerpt_chunks = Some(excerpt.chunks_in_range(
                0..(self.excerpt_offset_range.end - *self.excerpts.start()),
                self.language_aware,
                self.snapshot,
            ));
        }
    }
}

impl<'a> Iterator for ReversedMultiBufferChunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.cursor.region()?;
        if self.offset == region.range.start {
            self.cursor.prev();
            region = self.cursor.region()?;
            let start_overshoot = self.start.saturating_sub(region.range.start);
            self.current_chunks = Some(
                region
                    .buffer
                    .reversed_chunks_in_range(region.buffer_range.start + start_overshoot..region.buffer_range.end),
            );
        }

        if self.offset == region.range.end && region.has_trailing_newline {
            self.offset -= 1;
            Some("\n")
        } else {
            let chunk = self.current_chunks.as_mut().expect("current chunks set above").next()?;
            self.offset -= chunk.len();
            Some(chunk)
        }
    }
}

impl<'a> Iterator for MultiBufferChunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Chunk<'a>> {
        if self.range.start >= self.range.end {
            return None;
        }
        while self.diff_transforms.item().is_some_and(|_| self.range.start >= self.diff_transforms.end().0) {
            self.diff_transforms.next();
        }

        let diff_transform_start = self.diff_transforms.start().0;
        let diff_transform_end = self.diff_transforms.end().0;

        let diff_transform = self.diff_transforms.item()?;
        match diff_transform {
            DiffTransform::BufferContent { .. } => {
                let chunk = if let Some(chunk) = &mut self.buffer_chunk {
                    chunk
                } else {
                    let chunk = self.next_excerpt_chunk().expect("diff transform has an excerpt chunk");
                    self.buffer_chunk.insert(chunk)
                };

                let chunk_end = self.range.start + chunk.text.len();
                let diff_transform_end = diff_transform_end.min(self.range.end);

                let split_idx = if diff_transform_end < chunk_end {
                    chunk.text.ceil_char_boundary(diff_transform_end - self.range.start)
                } else {
                    chunk.text.len()
                };

                if split_idx < chunk.text.len() {
                    let (before, after) = chunk.text.split_at(split_idx);
                    self.range.start += split_idx;
                    let mask = 1_u128.unbounded_shl(split_idx as u32).wrapping_sub(1);
                    let chars = chunk.chars & mask;
                    let tabs = chunk.tabs & mask;
                    let newlines = chunk.newlines & mask;

                    chunk.text = after;
                    chunk.chars >>= split_idx;
                    chunk.tabs >>= split_idx;
                    chunk.newlines >>= split_idx;

                    Some(Chunk { text: before, chars, tabs, newlines, ..chunk.clone() })
                } else {
                    self.range.start = chunk_end;
                    self.buffer_chunk.take()
                }
            }
            DiffTransform::DeletedHunk { buffer_id, base_text_byte_range, .. } => {
                let base_text_start = base_text_byte_range.start + (self.range.start - diff_transform_start);
                let base_text_end = base_text_byte_range.start + (self.range.end - diff_transform_start);
                let base_text_end = base_text_end.min(base_text_byte_range.end);

                let mut chunks =
                    if let Some((_, mut chunks)) = self.diff_base_chunks.take().filter(|(id, _)| id == buffer_id) {
                        if chunks.range().start != base_text_start || chunks.range().end < base_text_end {
                            chunks.seek(base_text_start..base_text_end);
                        }
                        chunks
                    } else {
                        let base_buffer = &find_diff_state(&self.snapshot.diffs, *buffer_id)?.base_text();
                        base_buffer.chunks(base_text_start..base_text_end, self.language_aware)
                    };

                let chunk = if let Some(chunk) = chunks.next() {
                    self.range.start += chunk.text.len();
                    self.diff_base_chunks = Some((*buffer_id, chunks));
                    chunk
                } else {
                    self.range.start += "\n".len();
                    Chunk { text: "\n", chars: 1_u128, newlines: 1_u128, ..Default::default() }
                };
                Some(chunk)
            }
        }
    }
}

impl MultiBufferBytes<'_> {
    fn consume(&mut self, len: usize) {
        self.range.start += len;
        self.chunk = &self.chunk[len..];

        if !self.range.is_empty() && self.chunk.is_empty() {
            if let Some(chunk) = self.excerpt_bytes.as_mut().and_then(Iterator::next) {
                self.chunk = chunk;
            } else if self.has_trailing_newline {
                self.has_trailing_newline = false;
                self.chunk = b"\n";
            } else {
                self.cursor.next();
                if let Some(region) = self.cursor.region() {
                    let mut excerpt_bytes = region.buffer.bytes_in_range(
                        region.buffer_range.start
                            ..(region.buffer_range.start + (self.range.end - region.range.start))
                                .min(region.buffer_range.end),
                    );
                    self.chunk = excerpt_bytes.next().unwrap_or(&[]);
                    self.excerpt_bytes = Some(excerpt_bytes);
                    self.has_trailing_newline = region.has_trailing_newline && self.range.end >= region.range.end;
                    if self.chunk.is_empty() && self.has_trailing_newline {
                        self.has_trailing_newline = false;
                        self.chunk = b"\n";
                    }
                }
            }
        }
    }
}

impl<'a> Iterator for MultiBufferBytes<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let chunk = self.chunk;
        if chunk.is_empty() {
            None
        } else {
            self.consume(chunk.len());
            Some(chunk)
        }
    }
}

impl io::Read for MultiBufferBytes<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = cmp::min(buf.len(), self.chunk.len());
        buf[..len].copy_from_slice(&self.chunk[..len]);
        if len > 0 {
            self.consume(len);
        }
        Ok(len)
    }
}

impl io::Read for ReversedMultiBufferBytes<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = cmp::min(buf.len(), self.chunk.len());
        buf[..len].copy_from_slice(&self.chunk[..len]);
        buf[..len].reverse();
        if len > 0 {
            self.range.end -= len;
            self.chunk = &self.chunk[..self.chunk.len() - len];
            if !self.range.is_empty()
                && self.chunk.is_empty()
                && let Some(chunk) = self.chunks.next()
            {
                self.chunk = chunk.as_bytes();
            }
        }
        Ok(len)
    }
}

impl<'a> Iterator for ExcerptChunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(chunk) = self.content_chunks.next() {
            return Some(chunk);
        }

        if self.has_footer {
            let text = "\n";
            let chars = 0b1;
            let newlines = 0b1;
            self.has_footer = false;
            return Some(Chunk { text, chars, newlines, ..Default::default() });
        }

        None
    }
}
