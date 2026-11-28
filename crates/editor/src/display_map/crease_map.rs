use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::ops::Range;
use std::sync::Arc;

use gpui::AnyElement;
use multi_buffer::Anchor;
use multi_buffer::AnchorRangeExt;
use multi_buffer::MultiBufferRow;
use multi_buffer::MultiBufferSnapshot;
use multi_buffer::ToPoint as _;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use sum_tree::Bias;
use sum_tree::SeekTarget;
use sum_tree::SumTree;
use text::Point;
use ui::App;
use ui::Window;

use crate::FoldPlaceholder;

#[derive(Copy, Clone, Default, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub(crate) struct CreaseId(usize);

pub(crate) struct CreaseMap {
    snapshot: CreaseSnapshot,
    next_id: CreaseId,
    id_to_range: FxHashMap<CreaseId, Range<Anchor>>,
}

impl CreaseMap {
    pub(crate) fn new(snapshot: &MultiBufferSnapshot) -> Self {
        CreaseMap {
            snapshot: CreaseSnapshot::new(snapshot),
            next_id: CreaseId::default(),
            id_to_range: FxHashMap::default(),
        }
    }

    pub(crate) fn snapshot(&self) -> CreaseSnapshot {
        self.snapshot.clone()
    }

    pub(crate) fn insert(
        &mut self,
        creases: impl IntoIterator<Item = Crease<Anchor>>,
        snapshot: &MultiBufferSnapshot,
    ) -> Vec<CreaseId> {
        let mut new_ids = Vec::new();
        self.snapshot.creases = {
            let mut new_creases = SumTree::new(snapshot);
            let mut cursor = self.snapshot.creases.cursor::<ItemSummary>(snapshot);
            for crease in creases {
                let crease_range = crease.range().clone();
                new_creases.append(cursor.slice(&crease_range, Bias::Left), snapshot);

                let id = self.next_id;
                self.next_id.0 += 1;
                self.id_to_range.insert(id, crease_range);
                new_creases.push(CreaseItem { id, crease }, snapshot);
                new_ids.push(id);
            }
            new_creases.append(cursor.suffix(), snapshot);
            new_creases
        };
        new_ids
    }

    pub(crate) fn remove(
        &mut self,
        ids: impl IntoIterator<Item = CreaseId>,
        snapshot: &MultiBufferSnapshot,
    ) -> Vec<(CreaseId, Range<Anchor>)> {
        let mut removals = Vec::new();
        let mut ids_to_remove = FxHashSet::default();
        for id in ids {
            if let Some(range) = self.id_to_range.remove(&id) {
                ids_to_remove.insert(id);
                removals.push((id, range));
            }
        }

        if !ids_to_remove.is_empty() {
            self.snapshot.creases = {
                let mut new_creases = SumTree::new(snapshot);
                for item in self.snapshot.creases.iter() {
                    if !ids_to_remove.contains(&item.id) {
                        new_creases.push(item.clone(), snapshot);
                    }
                }
                new_creases
            };
        }

        removals
    }
}

#[derive(Clone)]
pub(crate) struct CreaseSnapshot {
    creases: SumTree<CreaseItem>,
}

impl CreaseSnapshot {
    fn new(snapshot: &MultiBufferSnapshot) -> Self {
        CreaseSnapshot { creases: SumTree::new(snapshot) }
    }

    /// Returns the first Crease starting on the specified buffer row.
    pub(crate) fn query_row<'a>(
        &'a self,
        row: MultiBufferRow,
        snapshot: &'a MultiBufferSnapshot,
    ) -> Option<&'a Crease<Anchor>> {
        let start = snapshot.anchor_before(Point::new(row.0, 0));
        let mut cursor = self.creases.cursor::<ItemSummary>(snapshot);
        cursor.seek(&start, Bias::Left);
        while let Some(item) = cursor.item() {
            match Ord::cmp(&item.crease.range().start.to_point(snapshot).row, &row.0) {
                Ordering::Less => cursor.next(),
                Ordering::Equal => {
                    if item.crease.range().start.is_valid(snapshot) {
                        return Some(&item.crease);
                    }
                    cursor.next();
                }
                Ordering::Greater => break,
            }
        }
        None
    }
}

type RenderToggleFn = Arc<
    dyn Send
        + Sync
        + Fn(
            MultiBufferRow,
            bool,
            Arc<dyn Send + Sync + Fn(bool, &mut Window, &mut App)>,
            &mut Window,
            &mut App,
        ) -> AnyElement,
>;
type RenderTrailerFn = Arc<dyn Send + Sync + Fn(MultiBufferRow, bool, &mut Window, &mut App) -> AnyElement>;

#[derive(Clone)]
pub(crate) struct Crease<T> {
    pub(crate) range: Range<T>,
    pub(crate) placeholder: FoldPlaceholder,
    pub(crate) render_toggle: Option<RenderToggleFn>,
    pub(crate) render_trailer: Option<RenderTrailerFn>,
}

impl<T> Crease<T> {
    pub(crate) const fn simple(range: Range<T>, placeholder: FoldPlaceholder) -> Self {
        Crease { range, placeholder, render_toggle: None, render_trailer: None }
    }

    pub(crate) const fn range(&self) -> &Range<T> {
        &self.range
    }
}

impl<T> Debug for Crease<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Crease").field("range", &self.range).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
struct CreaseItem {
    id: CreaseId,
    crease: Crease<Anchor>,
}

#[derive(Debug, Clone)]
struct ItemSummary {
    range: Range<Anchor>,
}

impl Default for ItemSummary {
    fn default() -> Self {
        Self { range: Anchor::Min..Anchor::Min }
    }
}

impl sum_tree::Summary for ItemSummary {
    type Context<'a> = &'a MultiBufferSnapshot;

    fn zero(_cx: Self::Context<'_>) -> Self {
        ItemSummary::default()
    }

    fn add_summary(&mut self, other: &Self, _snapshot: &MultiBufferSnapshot) {
        self.range = other.range.clone();
    }
}

impl sum_tree::Item for CreaseItem {
    type Summary = ItemSummary;

    fn summary(&self, _cx: &MultiBufferSnapshot) -> Self::Summary {
        ItemSummary { range: self.crease.range().clone() }
    }
}

/// Implements `SeekTarget` for `Range<Anchor>` to enable seeking within a `SumTree` of `CreaseItem`s.
impl SeekTarget<'_, ItemSummary, ItemSummary> for Range<Anchor> {
    fn cmp(&self, cursor_location: &ItemSummary, snapshot: &MultiBufferSnapshot) -> Ordering {
        AnchorRangeExt::cmp(self, &cursor_location.range, snapshot)
    }
}

impl SeekTarget<'_, ItemSummary, ItemSummary> for Anchor {
    fn cmp(&self, other: &ItemSummary, snapshot: &MultiBufferSnapshot) -> Ordering {
        self.cmp(&other.range.start, snapshot)
    }
}
