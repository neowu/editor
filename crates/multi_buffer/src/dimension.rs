use std::cmp;
use std::fmt;
use std::hash::Hash;
use std::ops;
use std::ops::AddAssign;
use std::ops::Sub;
use std::ops::SubAssign;
use std::str;

use language::OffsetUtf16;
use language::Point;
use language::PointUtf16;
use language::TextDimension;
use rope::DimensionPair;
use text::TextSummary;

use super::MultiBufferSnapshot;

pub type MultiBufferPoint = Point;
/// ExcerptOffset is offset into the non-deleted text of the multibuffer.
pub(super) type ExcerptOffset = ExcerptDimension<MultiBufferOffset>;
/// ExcerptOffset is based on the non-deleted text of the multibuffer.

#[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq, Hash, serde::Deserialize)]
#[serde(transparent)]
pub struct MultiBufferRow(pub u32);

impl MultiBufferRow {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(u32::MAX);
}

impl ops::Add<usize> for MultiBufferRow {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        MultiBufferRow(self.0 + rhs as u32)
    }
}

pub trait MultiBufferDimension: 'static + Copy + Default + fmt::Debug {
    type TextDimension: TextDimension;
    fn from_summary(summary: &MBTextSummary) -> Self;

    fn add_text_dim(&mut self, summary: &Self::TextDimension);

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary);
}

// todo(lw): MultiBufferPoint
impl MultiBufferDimension for Point {
    type TextDimension = Point;
    fn from_summary(summary: &MBTextSummary) -> Self {
        summary.lines
    }

    fn add_text_dim(&mut self, other: &Self::TextDimension) {
        *self += *other;
    }

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary) {
        *self += summary.lines;
    }
}

// todo(lw): MultiBufferPointUtf16
impl MultiBufferDimension for PointUtf16 {
    type TextDimension = PointUtf16;
    fn from_summary(summary: &MBTextSummary) -> Self {
        summary.lines_utf16()
    }

    fn add_text_dim(&mut self, other: &Self::TextDimension) {
        *self += *other;
    }

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary) {
        *self += summary.lines_utf16();
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq, Hash, serde::Deserialize)]
pub struct MultiBufferOffset(pub usize);

impl fmt::Display for MultiBufferOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl MultiBufferDimension for MultiBufferOffset {
    type TextDimension = usize;
    fn from_summary(summary: &MBTextSummary) -> Self {
        summary.len
    }

    fn add_text_dim(&mut self, other: &Self::TextDimension) {
        self.0 += *other;
    }

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary) {
        *self += summary.len;
    }
}
impl MultiBufferDimension for MultiBufferOffsetUtf16 {
    type TextDimension = OffsetUtf16;
    fn from_summary(summary: &MBTextSummary) -> Self {
        MultiBufferOffsetUtf16(summary.len_utf16)
    }

    fn add_text_dim(&mut self, other: &Self::TextDimension) {
        self.0 += *other;
    }

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary) {
        self.0 += summary.len_utf16;
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq, Hash, serde::Deserialize)]
pub struct BufferOffset(pub usize);

impl TextDimension for BufferOffset {
    fn from_text_summary(summary: &TextSummary) -> Self {
        BufferOffset(usize::from_text_summary(summary))
    }
    fn from_chunk(chunk: rope::ChunkSlice) -> Self {
        BufferOffset(usize::from_chunk(chunk))
    }
    fn add_assign(&mut self, other: &Self) {
        TextDimension::add_assign(&mut self.0, &other.0);
    }
}
impl<'a> sum_tree::Dimension<'a, rope::ChunkSummary> for BufferOffset {
    fn zero(cx: ()) -> Self {
        BufferOffset(<usize as sum_tree::Dimension<'a, rope::ChunkSummary>>::zero(cx))
    }

    fn add_summary(&mut self, summary: &'a rope::ChunkSummary, cx: ()) {
        usize::add_summary(&mut self.0, summary, cx);
    }
}

impl Sub for BufferOffset {
    type Output = usize;

    fn sub(self, other: BufferOffset) -> Self::Output {
        self.0 - other.0
    }
}

impl AddAssign<DimensionPair<usize, Point>> for BufferOffset {
    fn add_assign(&mut self, other: DimensionPair<usize, Point>) {
        self.0 += other.key;
    }
}

impl language::ToPoint for BufferOffset {
    fn to_point(&self, snapshot: &text::BufferSnapshot) -> Point {
        self.0.to_point(snapshot)
    }
}

impl language::ToPointUtf16 for BufferOffset {
    fn to_point_utf16(&self, snapshot: &text::BufferSnapshot) -> PointUtf16 {
        self.0.to_point_utf16(snapshot)
    }
}

impl language::ToOffset for BufferOffset {
    fn to_offset(&self, snapshot: &text::BufferSnapshot) -> usize {
        self.0.to_offset(snapshot)
    }
}

impl language::ToOffsetUtf16 for BufferOffset {
    fn to_offset_utf16(&self, snapshot: &text::BufferSnapshot) -> OffsetUtf16 {
        self.0.to_offset_utf16(snapshot)
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct MultiBufferOffsetUtf16(pub OffsetUtf16);

impl ops::Add<usize> for MultiBufferOffsetUtf16 {
    type Output = MultiBufferOffsetUtf16;

    fn add(self, rhs: usize) -> Self::Output {
        MultiBufferOffsetUtf16(OffsetUtf16(self.0.0 + rhs))
    }
}

impl ops::Add<OffsetUtf16> for MultiBufferOffsetUtf16 {
    type Output = Self;

    fn add(self, rhs: OffsetUtf16) -> Self::Output {
        MultiBufferOffsetUtf16(self.0 + rhs)
    }
}

impl AddAssign<OffsetUtf16> for MultiBufferOffsetUtf16 {
    fn add_assign(&mut self, rhs: OffsetUtf16) {
        self.0 += rhs;
    }
}

impl AddAssign<usize> for MultiBufferOffsetUtf16 {
    fn add_assign(&mut self, rhs: usize) {
        self.0.0 += rhs;
    }
}

impl Sub for MultiBufferOffsetUtf16 {
    type Output = OffsetUtf16;

    fn sub(self, other: MultiBufferOffsetUtf16) -> Self::Output {
        self.0 - other.0
    }
}

impl Sub<OffsetUtf16> for MultiBufferOffsetUtf16 {
    type Output = MultiBufferOffsetUtf16;

    fn sub(self, other: OffsetUtf16) -> Self::Output {
        MultiBufferOffsetUtf16(self.0 - other)
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct BufferOffsetUtf16(pub OffsetUtf16);

impl MultiBufferOffset {
    pub(super) const ZERO: Self = Self(0);
    pub const fn saturating_sub(self, other: MultiBufferOffset) -> usize {
        self.0.saturating_sub(other.0)
    }
    pub const fn saturating_sub_usize(self, other: usize) -> MultiBufferOffset {
        MultiBufferOffset(self.0.saturating_sub(other))
    }
}

impl Sub for MultiBufferOffset {
    type Output = usize;

    fn sub(self, other: MultiBufferOffset) -> Self::Output {
        self.0 - other.0
    }
}

impl Sub<usize> for MultiBufferOffset {
    type Output = Self;

    fn sub(self, other: usize) -> Self::Output {
        MultiBufferOffset(self.0 - other)
    }
}

impl SubAssign<usize> for MultiBufferOffset {
    fn sub_assign(&mut self, other: usize) {
        self.0 -= other;
    }
}

impl ops::Add<usize> for BufferOffset {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        BufferOffset(self.0 + rhs)
    }
}

impl AddAssign<usize> for BufferOffset {
    fn add_assign(&mut self, other: usize) {
        self.0 += other;
    }
}

impl ops::Add<usize> for MultiBufferOffset {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        MultiBufferOffset(self.0 + rhs)
    }
}

impl AddAssign<usize> for MultiBufferOffset {
    fn add_assign(&mut self, other: usize) {
        self.0 += other;
    }
}

impl ops::Add<isize> for MultiBufferOffset {
    type Output = Self;

    fn add(self, rhs: isize) -> Self::Output {
        MultiBufferOffset((self.0.cast_signed() + rhs) as usize)
    }
}

impl ops::Add for MultiBufferOffset {
    type Output = Self;

    fn add(self, rhs: MultiBufferOffset) -> Self::Output {
        MultiBufferOffset(self.0 + rhs.0)
    }
}

impl AddAssign<MultiBufferOffset> for MultiBufferOffset {
    fn add_assign(&mut self, other: MultiBufferOffset) {
        self.0 += other.0;
    }
}

pub trait ToOffset: 'static + fmt::Debug {
    fn to_offset(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffset;
    fn to_offset_utf16(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffsetUtf16;
}

pub trait ToPoint: 'static + fmt::Debug {
    fn to_point(&self, snapshot: &MultiBufferSnapshot) -> Point;
    fn to_point_utf16(&self, snapshot: &MultiBufferSnapshot) -> PointUtf16;
}

/// Summary of a string of text.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct MBTextSummary {
    /// Length in bytes.
    pub len: MultiBufferOffset,
    /// Length in UTF-8.
    pub chars: usize,
    /// Length in UTF-16 code units.
    pub len_utf16: OffsetUtf16,
    /// A point representing the number of lines and the length of the last line.
    ///
    /// In other words, it marks the point after the last byte in the text, (if
    /// EOF was a character, this would be its position).
    pub lines: Point,
    /// How many `char`s are in the first line.
    pub first_line_chars: u32,
    /// How many `char`s are in the last line.
    pub last_line_chars: u32,
    /// How many UTF-16 code units are in the last line.
    pub last_line_len_utf16: u32,
    /// The row idx of the longest row.
    pub longest_row: u32,
    /// How many `char`s are in the longest row.
    pub longest_row_chars: u32,
}

impl From<TextSummary> for MBTextSummary {
    fn from(summary: TextSummary) -> Self {
        MBTextSummary {
            len: MultiBufferOffset(summary.len),
            chars: summary.chars,
            len_utf16: summary.len_utf16,
            lines: summary.lines,
            first_line_chars: summary.first_line_chars,
            last_line_chars: summary.last_line_chars,
            last_line_len_utf16: summary.last_line_len_utf16,
            longest_row: summary.longest_row,
            longest_row_chars: summary.longest_row_chars,
        }
    }
}

impl From<MBTextSummary> for TextSummary {
    fn from(summary: MBTextSummary) -> Self {
        TextSummary {
            len: summary.len.0,
            chars: summary.chars,
            len_utf16: summary.len_utf16,
            lines: summary.lines,
            first_line_chars: summary.first_line_chars,
            last_line_chars: summary.last_line_chars,
            last_line_len_utf16: summary.last_line_len_utf16,
            longest_row: summary.longest_row,
            longest_row_chars: summary.longest_row_chars,
        }
    }
}

impl From<&str> for MBTextSummary {
    fn from(text: &str) -> Self {
        MBTextSummary::from(TextSummary::from(text))
    }
}

impl MultiBufferDimension for MBTextSummary {
    type TextDimension = TextSummary;

    fn from_summary(summary: &MBTextSummary) -> Self {
        *summary
    }

    fn add_text_dim(&mut self, summary: &Self::TextDimension) {
        *self += *summary;
    }

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary) {
        *self += *summary;
    }
}

impl AddAssign for MBTextSummary {
    fn add_assign(&mut self, other: MBTextSummary) {
        let joined_chars = self.last_line_chars + other.first_line_chars;
        if joined_chars > self.longest_row_chars {
            self.longest_row = self.lines.row;
            self.longest_row_chars = joined_chars;
        }
        if other.longest_row_chars > self.longest_row_chars {
            self.longest_row = self.lines.row + other.longest_row;
            self.longest_row_chars = other.longest_row_chars;
        }

        if self.lines.row == 0 {
            self.first_line_chars += other.first_line_chars;
        }

        if other.lines.row == 0 {
            self.last_line_chars += other.first_line_chars;
            self.last_line_len_utf16 += other.last_line_len_utf16;
        } else {
            self.last_line_chars = other.last_line_chars;
            self.last_line_len_utf16 = other.last_line_len_utf16;
        }

        self.chars += other.chars;
        self.len += other.len;
        self.len_utf16 += other.len_utf16;
        self.lines += other.lines;
    }
}

impl AddAssign<TextSummary> for MBTextSummary {
    fn add_assign(&mut self, other: TextSummary) {
        *self += MBTextSummary::from(other);
    }
}

impl MBTextSummary {
    pub const fn lines_utf16(&self) -> PointUtf16 {
        PointUtf16 { row: self.lines.row, column: self.last_line_len_utf16 }
    }
}

impl<K, V> MultiBufferDimension for DimensionPair<K, V>
where
    K: MultiBufferDimension,
    V: MultiBufferDimension,
{
    type TextDimension = DimensionPair<K::TextDimension, V::TextDimension>;

    fn from_summary(summary: &MBTextSummary) -> Self {
        Self { key: K::from_summary(summary), value: Some(V::from_summary(summary)) }
    }

    fn add_text_dim(&mut self, summary: &Self::TextDimension) {
        self.key.add_text_dim(&summary.key);
        if let Some(value) = &mut self.value
            && let Some(other_value) = summary.value.as_ref()
        {
            value.add_text_dim(other_value);
        }
    }

    fn add_mb_text_summary(&mut self, summary: &MBTextSummary) {
        self.key.add_mb_text_summary(summary);
        if let Some(value) = &mut self.value {
            value.add_mb_text_summary(summary);
        }
    }
}

#[derive(Copy, Clone, PartialOrd, Ord, Eq, PartialEq, Debug)]
pub(super) struct OutputDimension<T>(pub(super) T);

impl<T: PartialEq> PartialEq<T> for OutputDimension<T> {
    fn eq(&self, other: &T) -> bool {
        self.0 == *other
    }
}

impl<T: PartialOrd> PartialOrd<T> for OutputDimension<T> {
    fn partial_cmp(&self, other: &T) -> Option<cmp::Ordering> {
        self.0.partial_cmp(other)
    }
}

impl<R, T, U> Sub<OutputDimension<U>> for OutputDimension<T>
where
    T: Sub<U, Output = R>,
{
    type Output = R;

    fn sub(self, other: OutputDimension<U>) -> Self::Output {
        self.0 - other.0
    }
}

impl<R, T, U> ops::Add<U> for OutputDimension<T>
where
    T: ops::Add<U, Output = R>,
{
    type Output = OutputDimension<R>;

    fn add(self, other: U) -> Self::Output {
        OutputDimension(self.0 + other)
    }
}

impl<T, U> AddAssign<U> for OutputDimension<T>
where
    T: AddAssign<U>,
{
    fn add_assign(&mut self, other: U) {
        self.0 += other;
    }
}

impl<T, U> SubAssign<U> for OutputDimension<T>
where
    T: SubAssign<U>,
{
    fn sub_assign(&mut self, other: U) {
        self.0 -= other;
    }
}

#[derive(Copy, Clone, PartialOrd, Ord, Eq, PartialEq, Debug, Default)]
pub(super) struct ExcerptDimension<T>(pub(super) T);

impl<T: PartialEq> PartialEq<T> for ExcerptDimension<T> {
    fn eq(&self, other: &T) -> bool {
        self.0 == *other
    }
}

impl<T: PartialOrd> PartialOrd<T> for ExcerptDimension<T> {
    fn partial_cmp(&self, other: &T) -> Option<cmp::Ordering> {
        self.0.partial_cmp(other)
    }
}

impl ExcerptOffset {
    pub(super) const fn saturating_sub(self, other: ExcerptOffset) -> usize {
        self.0.saturating_sub(other.0)
    }
}

impl<R, T, U> Sub<ExcerptDimension<U>> for ExcerptDimension<T>
where
    T: Sub<U, Output = R>,
{
    type Output = R;

    fn sub(self, other: ExcerptDimension<U>) -> Self::Output {
        self.0 - other.0
    }
}

impl<R, T, U> ops::Add<U> for ExcerptDimension<T>
where
    T: ops::Add<U, Output = R>,
{
    type Output = ExcerptDimension<R>;

    fn add(self, other: U) -> Self::Output {
        ExcerptDimension(self.0 + other)
    }
}

impl<T, U> AddAssign<U> for ExcerptDimension<T>
where
    T: AddAssign<U>,
{
    fn add_assign(&mut self, other: U) {
        self.0 += other;
    }
}

impl<T, U> SubAssign<U> for ExcerptDimension<T>
where
    T: SubAssign<U>,
{
    fn sub_assign(&mut self, other: U) {
        self.0 -= other;
    }
}

impl ToOffset for Point {
    fn to_offset(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffset {
        snapshot.point_to_offset(*self)
    }
    fn to_offset_utf16(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffsetUtf16 {
        snapshot.point_to_offset_utf16(*self)
    }
}

impl ToOffset for MultiBufferOffset {
    #[track_caller]
    fn to_offset(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffset {
        assert!(*self <= snapshot.len(), "offset {} is greater than the snapshot.len() {}", self.0, snapshot.len().0);
        *self
    }
    fn to_offset_utf16(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffsetUtf16 {
        snapshot.offset_to_offset_utf16(*self)
    }
}

impl ToOffset for MultiBufferOffsetUtf16 {
    fn to_offset(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffset {
        snapshot.offset_utf16_to_offset(*self)
    }

    fn to_offset_utf16(&self, _snapshot: &MultiBufferSnapshot) -> MultiBufferOffsetUtf16 {
        *self
    }
}

impl ToOffset for PointUtf16 {
    fn to_offset(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffset {
        snapshot.point_utf16_to_offset(*self)
    }
    fn to_offset_utf16(&self, snapshot: &MultiBufferSnapshot) -> MultiBufferOffsetUtf16 {
        snapshot.point_utf16_to_offset_utf16(*self)
    }
}

impl ToPoint for MultiBufferOffset {
    fn to_point(&self, snapshot: &MultiBufferSnapshot) -> Point {
        snapshot.offset_to_point(*self)
    }
    fn to_point_utf16(&self, snapshot: &MultiBufferSnapshot) -> PointUtf16 {
        snapshot.offset_to_point_utf16(*self)
    }
}

impl ToPoint for Point {
    fn to_point(&self, _: &MultiBufferSnapshot) -> Point {
        *self
    }
    fn to_point_utf16(&self, snapshot: &MultiBufferSnapshot) -> PointUtf16 {
        snapshot.point_to_point_utf16(*self)
    }
}

impl ToPoint for PointUtf16 {
    fn to_point(&self, snapshot: &MultiBufferSnapshot) -> Point {
        snapshot.point_utf16_to_point(*self)
    }
    fn to_point_utf16(&self, _: &MultiBufferSnapshot) -> PointUtf16 {
        *self
    }
}
