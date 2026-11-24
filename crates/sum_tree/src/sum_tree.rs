mod cursor;
mod tree_map;

use std::cmp::Ordering;
use std::fmt;
use std::iter::FromIterator as _;
use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;

pub use cursor::Cursor;
pub use cursor::FilterCursor;
pub use cursor::Iter;
use heapless::Vec as ArrayVec;
pub use tree_map::TreeMap;
pub use tree_map::TreeSet;

pub const TREE_BASE: usize = 6;

// Helper for when we cannot use ArrayVec::<T>::push().unwrap() as T doesn't impl Debug
trait CapacityResultExt {
    fn unwrap_oob(self);
}

impl<T> CapacityResultExt for Result<(), T> {
    fn unwrap_oob(self) {
        self.unwrap_or_else(|_| panic!("item should fit into fixed size ArrayVec"));
    }
}

/// An item that can be stored in a [`SumTree`].
///
/// Must be summarized by a type that implements [`Summary`].
pub trait Item: Clone {
    type Summary: Summary;

    fn summary(&self, cx: <Self::Summary as Summary>::Context<'_>) -> Self::Summary;
}

/// An [`Item`] whose summary has a specific key that can be used to identify it.
pub trait KeyedItem: Item {
    type Key: for<'a> Dimension<'a, Self::Summary> + Ord;

    fn key(&self) -> Self::Key;
}

/// A type that describes the Sum of all [`Item`]s in a subtree of the [`SumTree`].
///
/// Each Summary type can have multiple [`Dimension`]s that it measures,
/// which can be used to navigate the tree.
pub trait Summary: Clone {
    type Context<'a>: Copy;
    fn zero(cx: Self::Context<'_>) -> Self;
    fn add_summary(&mut self, summary: &Self, cx: Self::Context<'_>);
}

pub trait ContextLessSummary: Clone {
    fn zero() -> Self;
    fn add_summary(&mut self, summary: &Self);
}

impl<T: ContextLessSummary> Summary for T {
    type Context<'a> = ();

    fn zero((): ()) -> Self {
        T::zero()
    }

    fn add_summary(&mut self, summary: &Self, (): ()) {
        T::add_summary(self, summary);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoSummary;

/// Catch-all implementation for when you need something that implements [`Summary`] without a specific type.
/// We implement it on a `NoSummary` instead of re-using `()`, as that avoids blanket impl collisions with `impl<T: Summary> Dimension for T`
/// (as we also need unit type to be a fill-in dimension).
impl ContextLessSummary for NoSummary {
    fn zero() -> Self {
        NoSummary
    }

    fn add_summary(&mut self, _: &Self) {}
}

/// Each [`Summary`] type can have more than one [`Dimension`] type that it measures.
///
/// You can use dimensions to seek to a specific location in the [`SumTree`].
///
/// # Example:
/// ed's rope has a `TextSummary` type that summarizes lines, characters, and bytes.
/// Each of these are different dimensions we may want to seek to.
pub trait Dimension<'a, S: Summary>: Clone {
    fn zero(cx: S::Context<'_>) -> Self;

    fn add_summary(&mut self, summary: &'a S, cx: S::Context<'_>);
    #[must_use]
    fn with_added_summary(mut self, summary: &'a S, cx: S::Context<'_>) -> Self {
        self.add_summary(summary, cx);
        self
    }

    fn from_summary(summary: &'a S, cx: S::Context<'_>) -> Self {
        let mut dimension = Self::zero(cx);
        dimension.add_summary(summary, cx);
        dimension
    }
}

impl<'a, T: Summary> Dimension<'a, T> for T {
    fn zero(cx: T::Context<'_>) -> Self {
        Summary::zero(cx)
    }

    fn add_summary(&mut self, summary: &'a T, cx: T::Context<'_>) {
        Summary::add_summary(self, summary, cx);
    }
}

pub trait SeekTarget<'a, S: Summary, D: Dimension<'a, S>> {
    fn cmp(&self, cursor_location: &D, cx: S::Context<'_>) -> Ordering;
}

impl<'a, S: Summary, D: Dimension<'a, S> + Ord> SeekTarget<'a, S, D> for D {
    fn cmp(&self, cursor_location: &Self, _: S::Context<'_>) -> Ordering {
        Ord::cmp(self, cursor_location)
    }
}

impl<'a, T: Summary> Dimension<'a, T> for () {
    fn zero(_: T::Context<'_>) -> Self {}

    fn add_summary(&mut self, _: &'a T, _: T::Context<'_>) {}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dimensions<D1, D2, D3 = ()>(pub D1, pub D2, pub D3);

impl<'a, T: Summary, D1: Dimension<'a, T>, D2: Dimension<'a, T>, D3: Dimension<'a, T>> Dimension<'a, T>
    for Dimensions<D1, D2, D3>
{
    fn zero(cx: T::Context<'_>) -> Self {
        Dimensions(D1::zero(cx), D2::zero(cx), D3::zero(cx))
    }

    fn add_summary(&mut self, summary: &'a T, cx: T::Context<'_>) {
        self.0.add_summary(summary, cx);
        self.1.add_summary(summary, cx);
        self.2.add_summary(summary, cx);
    }
}

impl<'a, S, D1, D2, D3> SeekTarget<'a, S, Dimensions<D1, D2, D3>> for D1
where
    S: Summary,
    D1: SeekTarget<'a, S, D1> + Dimension<'a, S>,
    D2: Dimension<'a, S>,
    D3: Dimension<'a, S>,
{
    fn cmp(&self, cursor_location: &Dimensions<D1, D2, D3>, cx: S::Context<'_>) -> Ordering {
        self.cmp(&cursor_location.0, cx)
    }
}

/// Bias is used to settle ambiguities when determining positions in an ordered sequence.
///
/// The primary use case is for text, where Bias influences
/// which character an offset or anchor is associated with.
///
/// # Examples
/// Given the buffer `AˇBCD`:
/// - The offset of the cursor is 1
/// - [Bias::Left] would attach the cursor to the character `A`
/// - [Bias::Right] would attach the cursor to the character `B`
///
/// Given the buffer `A«BCˇ»D`:
/// - The offset of the cursor is 3, and the selection is from 1 to 3
/// - The left anchor of the selection has [Bias::Right], attaching it to the character `B`
/// - The right anchor of the selection has [Bias::Left], attaching it to the character `C`
///
/// Given the buffer `{ˇ<...>`, where `<...>` is a folded region:
/// - The display offset of the cursor is 1, but the offset in the buffer is determined by the bias
/// - [Bias::Left] would attach the cursor to the character `{`, with a buffer offset of 1
/// - [Bias::Right] would attach the cursor to the first character of the folded region,
///   and the buffer offset would be the offset of the first character of the folded region
#[derive(Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Debug, Hash, Default)]
pub enum Bias {
    /// Attach to the character on the left.
    #[default]
    Left,
    /// Attach to the character on the right.
    Right,
}

impl Bias {
    pub const fn invert(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// A B+ tree in which each leaf node contains `Item`s of type `T` and a `Summary`s for each `Item`.
/// Each internal node contains a `Summary` of the items in its subtree.
///
/// The maximum number of items per node is `TREE_BASE * 2`.
///
/// Any [`Dimension`] supported by the [`Summary`] type can be used to seek to a specific location in the tree.
#[derive(Clone)]
pub struct SumTree<T: Item>(Arc<Node<T>>);

impl<T> fmt::Debug for SumTree<T>
where
    T: fmt::Debug + Item,
    T::Summary: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("SumTree").field(&self.0).finish()
    }
}

impl<T: Item> SumTree<T> {
    /// Whether both trees share the same root, i.e. one is an unmodified clone of the other.
    pub fn ptr_eq(&self, other: &SumTree<T>) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn new(cx: <T::Summary as Summary>::Context<'_>) -> Self {
        SumTree(Arc::new(Node::Leaf {
            summary: <T::Summary as Summary>::zero(cx),
            items: ArrayVec::new(),
            item_summaries: ArrayVec::new(),
        }))
    }

    /// Useful in cases where the item type has a non-trivial context type, but the zero value of the summary type doesn't depend on that context.
    pub fn from_summary(summary: T::Summary) -> Self {
        SumTree(Arc::new(Node::Leaf { summary, items: ArrayVec::new(), item_summaries: ArrayVec::new() }))
    }

    pub fn from_item(item: T, cx: <T::Summary as Summary>::Context<'_>) -> Self {
        let mut tree = Self::new(cx);
        tree.push(item, cx);
        tree
    }

    pub fn from_iter<I: IntoIterator<Item = T>>(iter: I, cx: <T::Summary as Summary>::Context<'_>) -> Self {
        let mut nodes = Vec::new();

        let mut iter = iter.into_iter().fuse().peekable();
        while iter.peek().is_some() {
            let items: ArrayVec<T, { 2 * TREE_BASE }, u8> = iter.by_ref().take(2 * TREE_BASE).collect();
            let item_summaries: ArrayVec<T::Summary, { 2 * TREE_BASE }, u8> =
                items.iter().map(|item| item.summary(cx)).collect();

            let mut summary = item_summaries[0].clone();
            for item_summary in &item_summaries[1..] {
                <T::Summary as Summary>::add_summary(&mut summary, item_summary, cx);
            }

            nodes.push(SumTree(Arc::new(Node::Leaf { summary, items, item_summaries })));
        }

        let mut parent_nodes = Vec::new();
        let mut height = 0;
        while nodes.len() > 1 {
            height += 1;
            let mut current_parent_node = None;
            for child_node in nodes.drain(..) {
                let parent_node = current_parent_node.get_or_insert_with(|| {
                    SumTree(Arc::new(Node::Internal {
                        summary: <T::Summary as Summary>::zero(cx),
                        height,
                        child_summaries: ArrayVec::new(),
                        child_trees: ArrayVec::new(),
                    }))
                });
                let Node::Internal { summary, child_summaries, child_trees, .. } =
                    Arc::get_mut(&mut parent_node.0).expect("parent node is uniquely owned")
                else {
                    unreachable!()
                };
                let child_summary = child_node.summary();
                <T::Summary as Summary>::add_summary(summary, child_summary, cx);
                child_summaries.push(child_summary.clone()).unwrap_oob();
                child_trees.push(child_node.clone()).unwrap_oob();

                if child_trees.len() == 2 * TREE_BASE {
                    parent_nodes.extend(current_parent_node.take());
                }
            }
            parent_nodes.extend(current_parent_node.take());
            mem::swap(&mut nodes, &mut parent_nodes);
        }

        if nodes.is_empty() { Self::new(cx) } else { nodes.pop().expect("checked non-empty") }
    }

    pub fn items<'a>(&'a self, cx: <T::Summary as Summary>::Context<'a>) -> Vec<T> {
        let mut items = Vec::new();
        let mut cursor = self.cursor::<()>(cx);
        cursor.next();
        while let Some(item) = cursor.item() {
            items.push(item.clone());
            cursor.next();
        }
        items
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter::new(self)
    }

    /// A more efficient version of `Cursor::new()` + `Cursor::seek()` + `Cursor::item()`.
    ///
    /// Only returns the item that exactly has the target match.
    pub fn find_exact<'a, 'slf, D, Target>(
        &'slf self,
        cx: <T::Summary as Summary>::Context<'a>,
        target: &Target,
        bias: Bias,
    ) -> (D, D, Option<&'slf T>)
    where
        D: Dimension<'slf, T::Summary>,
        Target: SeekTarget<'slf, T::Summary, D>,
    {
        let tree_end = D::zero(cx).with_added_summary(self.summary(), cx);
        let comparison = target.cmp(&tree_end, cx);
        if comparison == Ordering::Greater || (comparison == Ordering::Equal && bias == Bias::Right) {
            return (tree_end.clone(), tree_end, None);
        }

        let mut pos = D::zero(cx);
        return match Self::find_iterate::<_, _, true>(cx, target, bias, &mut pos, self) {
            Some((item, end)) => (pos, end, Some(item)),
            None => (pos.clone(), pos, None),
        };
    }

    /// A more efficient version of `Cursor::new()` + `Cursor::seek()` + `Cursor::item()`.
    pub fn find<'a, 'slf, D, Target>(
        &'slf self,
        cx: <T::Summary as Summary>::Context<'a>,
        target: &Target,
        bias: Bias,
    ) -> (D, D, Option<&'slf T>)
    where
        D: Dimension<'slf, T::Summary>,
        Target: SeekTarget<'slf, T::Summary, D>,
    {
        let tree_end = D::zero(cx).with_added_summary(self.summary(), cx);
        let comparison = target.cmp(&tree_end, cx);
        if comparison == Ordering::Greater || (comparison == Ordering::Equal && bias == Bias::Right) {
            return (tree_end.clone(), tree_end, None);
        }

        let mut pos = D::zero(cx);
        return match Self::find_iterate::<_, _, false>(cx, target, bias, &mut pos, self) {
            Some((item, end)) => (pos, end, Some(item)),
            None => (pos.clone(), pos, None),
        };
    }

    fn find_iterate<'tree, 'a, D, Target, const EXACT: bool>(
        cx: <T::Summary as Summary>::Context<'a>,
        target: &Target,
        bias: Bias,
        position: &mut D,
        mut this: &'tree SumTree<T>,
    ) -> Option<(&'tree T, D)>
    where
        D: Dimension<'tree, T::Summary>,
        Target: SeekTarget<'tree, T::Summary, D>,
    {
        'iterate: loop {
            match &*this.0 {
                Node::Internal { child_summaries, child_trees, .. } => {
                    for (child_tree, child_summary) in child_trees.iter().zip(child_summaries) {
                        let child_end = position.clone().with_added_summary(child_summary, cx);

                        let comparison = target.cmp(&child_end, cx);
                        let target_in_child =
                            comparison == Ordering::Less || (comparison == Ordering::Equal && bias == Bias::Left);
                        if target_in_child {
                            this = child_tree;
                            continue 'iterate;
                        }
                        *position = child_end;
                    }
                }
                Node::Leaf { items, item_summaries, .. } => {
                    for (item, item_summary) in items.iter().zip(item_summaries) {
                        let mut child_end = position.clone();
                        child_end.add_summary(item_summary, cx);

                        let comparison = target.cmp(&child_end, cx);
                        let entry_found = if EXACT {
                            comparison == Ordering::Equal
                        } else {
                            comparison == Ordering::Less || (comparison == Ordering::Equal && bias == Bias::Left)
                        };
                        if entry_found {
                            return Some((item, child_end));
                        }

                        *position = child_end;
                    }
                }
            }
            return None;
        }
    }

    /// A more efficient version of `Cursor::new()` + `Cursor::seek()` + `Cursor::item()`.
    pub fn find_with_prev<'a, 'slf, D, Target>(
        &'slf self,
        cx: <T::Summary as Summary>::Context<'a>,
        target: &Target,
        bias: Bias,
    ) -> (D, D, Option<(Option<&'slf T>, &'slf T)>)
    where
        D: Dimension<'slf, T::Summary>,
        Target: SeekTarget<'slf, T::Summary, D>,
    {
        let tree_end = D::zero(cx).with_added_summary(self.summary(), cx);
        let comparison = target.cmp(&tree_end, cx);
        if comparison == Ordering::Greater || (comparison == Ordering::Equal && bias == Bias::Right) {
            return (tree_end.clone(), tree_end, None);
        }

        let mut pos = D::zero(cx);
        return match Self::find_with_prev_iterate::<_, _, false>(cx, target, bias, &mut pos, self) {
            Some((prev, item, end)) => (pos, end, Some((prev, item))),
            None => (pos.clone(), pos, None),
        };
    }

    fn find_with_prev_iterate<'tree, 'a, D, Target, const EXACT: bool>(
        cx: <T::Summary as Summary>::Context<'a>,
        target: &Target,
        bias: Bias,
        position: &mut D,
        mut this: &'tree SumTree<T>,
    ) -> Option<(Option<&'tree T>, &'tree T, D)>
    where
        D: Dimension<'tree, T::Summary>,
        Target: SeekTarget<'tree, T::Summary, D>,
    {
        let mut prev = None;
        'iterate: loop {
            match &*this.0 {
                Node::Internal { child_summaries, child_trees, .. } => {
                    for (child_tree, child_summary) in child_trees.iter().zip(child_summaries) {
                        let child_end = position.clone().with_added_summary(child_summary, cx);

                        let comparison = target.cmp(&child_end, cx);
                        let target_in_child =
                            comparison == Ordering::Less || (comparison == Ordering::Equal && bias == Bias::Left);
                        if target_in_child {
                            this = child_tree;
                            continue 'iterate;
                        }
                        prev = child_tree.last();
                        *position = child_end;
                    }
                }
                Node::Leaf { items, item_summaries, .. } => {
                    for (item, item_summary) in items.iter().zip(item_summaries) {
                        let mut child_end = position.clone();
                        child_end.add_summary(item_summary, cx);

                        let comparison = target.cmp(&child_end, cx);
                        let entry_found = if EXACT {
                            comparison == Ordering::Equal
                        } else {
                            comparison == Ordering::Less || (comparison == Ordering::Equal && bias == Bias::Left)
                        };
                        if entry_found {
                            return Some((prev, item, child_end));
                        }

                        prev = Some(item);
                        *position = child_end;
                    }
                }
            }
            return None;
        }
    }

    pub fn cursor<'a, 'b, D>(&'a self, cx: <T::Summary as Summary>::Context<'b>) -> Cursor<'a, 'b, T, D>
    where
        D: Dimension<'a, T::Summary>,
    {
        Cursor::new(self, cx)
    }

    /// Note: If the summary type requires a non `()` context, then the filter cursor
    /// that is returned cannot be used with Rust's iterators.
    pub fn filter<'a, 'b, F, U>(
        &'a self,
        cx: <T::Summary as Summary>::Context<'b>,
        filter_node: F,
    ) -> FilterCursor<'a, 'b, F, T, U>
    where
        F: FnMut(&T::Summary) -> bool,
        U: Dimension<'a, T::Summary>,
    {
        FilterCursor::new(self, cx, filter_node)
    }

    pub fn first(&self) -> Option<&T> {
        self.leftmost_leaf().0.items().first()
    }

    pub fn last(&self) -> Option<&T> {
        self.rightmost_leaf().0.items().last()
    }

    pub fn update_last(&mut self, f: impl FnOnce(&mut T), cx: <T::Summary as Summary>::Context<'_>) {
        self.update_last_recursive(f, cx);
    }

    fn update_last_recursive(
        &mut self,
        f: impl FnOnce(&mut T),
        cx: <T::Summary as Summary>::Context<'_>,
    ) -> Option<T::Summary> {
        match Arc::make_mut(&mut self.0) {
            Node::Internal { summary, child_summaries, child_trees, .. } => {
                let last_summary = child_summaries.last_mut().expect("internal nodes are non-empty");
                let last_child = child_trees.last_mut().expect("internal nodes are non-empty");
                *last_summary = last_child.update_last_recursive(f, cx).expect("internal nodes are non-empty");
                *summary = sum(child_summaries.iter(), cx);
                Some(summary.clone())
            }
            Node::Leaf { summary, items, item_summaries } => {
                if let Some((item, item_summary)) = items.last_mut().zip(item_summaries.last_mut()) {
                    (f)(item);
                    *item_summary = item.summary(cx);
                    *summary = sum(item_summaries.iter(), cx);
                    Some(summary.clone())
                } else {
                    None
                }
            }
        }
    }

    pub fn update_first(&mut self, f: impl FnOnce(&mut T), cx: <T::Summary as Summary>::Context<'_>) {
        self.update_first_recursive(f, cx);
    }

    fn update_first_recursive(
        &mut self,
        f: impl FnOnce(&mut T),
        cx: <T::Summary as Summary>::Context<'_>,
    ) -> Option<T::Summary> {
        match Arc::make_mut(&mut self.0) {
            Node::Internal { summary, child_summaries, child_trees, .. } => {
                let first_summary = child_summaries.first_mut().expect("internal nodes are non-empty");
                let first_child = child_trees.first_mut().expect("internal nodes are non-empty");
                *first_summary = first_child.update_first_recursive(f, cx).expect("internal nodes are non-empty");
                *summary = sum(child_summaries.iter(), cx);
                Some(summary.clone())
            }
            Node::Leaf { summary, items, item_summaries } => {
                if let Some((item, item_summary)) = items.first_mut().zip(item_summaries.first_mut()) {
                    (f)(item);
                    *item_summary = item.summary(cx);
                    *summary = sum(item_summaries.iter(), cx);
                    Some(summary.clone())
                } else {
                    None
                }
            }
        }
    }

    pub fn extent<'a, D: Dimension<'a, T::Summary>>(&'a self, cx: <T::Summary as Summary>::Context<'_>) -> D {
        let mut extent = D::zero(cx);
        match self.0.as_ref() {
            Node::Internal { summary, .. } | Node::Leaf { summary, .. } => {
                extent.add_summary(summary, cx);
            }
        }
        extent
    }

    pub fn summary(&self) -> &T::Summary {
        match self.0.as_ref() {
            Node::Internal { summary, .. } => summary,
            Node::Leaf { summary, .. } => summary,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self.0.as_ref() {
            Node::Internal { .. } => false,
            Node::Leaf { items, .. } => items.is_empty(),
        }
    }

    pub fn extend<I>(&mut self, iter: I, cx: <T::Summary as Summary>::Context<'_>)
    where
        I: IntoIterator<Item = T>,
    {
        self.append(Self::from_iter(iter, cx), cx);
    }

    pub fn push(&mut self, item: T, cx: <T::Summary as Summary>::Context<'_>) {
        let summary = item.summary(cx);
        self.append(
            SumTree(Arc::new(Node::Leaf {
                summary: summary.clone(),
                items: ArrayVec::from_iter(Some(item)),
                item_summaries: ArrayVec::from_iter(Some(summary)),
            })),
            cx,
        );
    }

    pub fn append(&mut self, mut other: Self, cx: <T::Summary as Summary>::Context<'_>) {
        if self.is_empty() {
            *self = other;
        } else if !other.0.is_leaf() || !other.0.items().is_empty() {
            if self.0.height() < other.0.height() {
                if let Some(tree) = Self::append_large(self.clone(), &mut other, cx) {
                    *self = Self::from_child_trees(tree, other, cx);
                } else {
                    *self = other;
                }
            } else if let Some(split_tree) = self.push_tree_recursive(other, cx) {
                *self = Self::from_child_trees(self.clone(), split_tree, cx);
            }
        }
    }

    fn push_tree_recursive(
        &mut self,
        other: SumTree<T>,
        cx: <T::Summary as Summary>::Context<'_>,
    ) -> Option<SumTree<T>> {
        match Arc::make_mut(&mut self.0) {
            Node::Internal { height, summary, child_summaries, child_trees, .. } => {
                let other_node = Arc::clone(&other.0);
                <T::Summary as Summary>::add_summary(summary, other_node.summary(), cx);

                let height_delta = *height - other_node.height();
                let mut summaries_to_append = ArrayVec::<T::Summary, { 2 * TREE_BASE }, u8>::new();
                let mut trees_to_append = ArrayVec::<SumTree<T>, { 2 * TREE_BASE }, u8>::new();
                if height_delta == 0 {
                    summaries_to_append.extend(other_node.child_summaries().iter().cloned());
                    trees_to_append.extend(other_node.child_trees().iter().cloned());
                } else if height_delta == 1 && !other_node.is_underflowing() {
                    summaries_to_append.push(other_node.summary().clone()).unwrap_oob();
                    trees_to_append.push(other).unwrap_oob();
                } else {
                    let tree_to_append =
                        child_trees.last_mut().expect("internal nodes are non-empty").push_tree_recursive(other, cx);
                    *child_summaries.last_mut().expect("internal nodes are non-empty") =
                        child_trees.last().expect("internal nodes are non-empty").0.summary().clone();

                    if let Some(split_tree) = tree_to_append {
                        summaries_to_append.push(split_tree.0.summary().clone()).unwrap_oob();
                        trees_to_append.push(split_tree).unwrap_oob();
                    }
                }

                let child_count = child_trees.len() + trees_to_append.len();
                if child_count > 2 * TREE_BASE {
                    let left_summaries: ArrayVec<_, { 2 * TREE_BASE }, u8>;
                    let right_summaries: ArrayVec<_, { 2 * TREE_BASE }, u8>;
                    let left_trees;
                    let right_trees;

                    let midpoint = usize::midpoint(child_count, child_count % 2);
                    {
                        let mut all_summaries = child_summaries.iter().chain(summaries_to_append.iter()).cloned();
                        left_summaries = all_summaries.by_ref().take(midpoint).collect();
                        right_summaries = all_summaries.collect();
                        let mut all_trees = child_trees.iter().chain(trees_to_append.iter()).cloned();
                        left_trees = all_trees.by_ref().take(midpoint).collect();
                        right_trees = all_trees.collect();
                    }
                    *summary = sum(left_summaries.iter(), cx);
                    *child_summaries = left_summaries;
                    *child_trees = left_trees;

                    Some(SumTree(Arc::new(Node::Internal {
                        height: *height,
                        summary: sum(right_summaries.iter(), cx),
                        child_summaries: right_summaries,
                        child_trees: right_trees,
                    })))
                } else {
                    child_summaries.extend(summaries_to_append);
                    child_trees.extend(trees_to_append);
                    None
                }
            }
            Node::Leaf { summary, items, item_summaries } => {
                let other_node = other.0;

                let child_count = items.len() + other_node.items().len();
                if child_count > 2 * TREE_BASE {
                    let left_items;
                    let right_items;
                    let left_summaries;
                    let right_summaries: ArrayVec<T::Summary, { 2 * TREE_BASE }, u8>;

                    let midpoint = usize::midpoint(child_count, child_count % 2);
                    {
                        let mut all_items = items.iter().chain(other_node.items().iter()).cloned();
                        left_items = all_items.by_ref().take(midpoint).collect();
                        right_items = all_items.collect();

                        let mut all_summaries = item_summaries.iter().chain(other_node.child_summaries()).cloned();
                        left_summaries = all_summaries.by_ref().take(midpoint).collect();
                        right_summaries = all_summaries.collect();
                    }
                    *items = left_items;
                    *item_summaries = left_summaries;
                    *summary = sum(item_summaries.iter(), cx);
                    Some(SumTree(Arc::new(Node::Leaf {
                        items: right_items,
                        summary: sum(right_summaries.iter(), cx),
                        item_summaries: right_summaries,
                    })))
                } else {
                    <T::Summary as Summary>::add_summary(summary, other_node.summary(), cx);
                    items.extend(other_node.items().iter().cloned());
                    item_summaries.extend(other_node.child_summaries().iter().cloned());
                    None
                }
            }
        }
    }

    // appends the `large` tree to a `small` tree, assumes small.height() <= large.height()
    fn append_large(small: Self, large: &mut Self, cx: <T::Summary as Summary>::Context<'_>) -> Option<Self> {
        if small.0.height() == large.0.height() {
            if small.0.is_underflowing() { Self::merge_into_right(small, large, cx) } else { Some(small) }
        } else {
            let Node::Internal { height, summary, child_summaries, child_trees } = Arc::make_mut(&mut large.0) else {
                unreachable!();
            };
            let mut full_summary = small.summary().clone();
            Summary::add_summary(&mut full_summary, summary, cx);
            *summary = full_summary;

            let first = child_trees.first_mut().expect("internal nodes are non-empty");
            let res = Self::append_large(small, first, cx);
            *child_summaries.first_mut().expect("internal nodes are non-empty") = first.summary().clone();
            if let Some(tree) = res {
                if child_trees.len() < 2 * TREE_BASE {
                    child_summaries.insert(0, tree.summary().clone()).unwrap_oob();
                    child_trees.insert(0, tree).unwrap_oob();
                    None
                } else {
                    let new_child_summaries = {
                        let mut res = ArrayVec::from_iter([tree.summary().clone()]);
                        res.extend(child_summaries.drain(..TREE_BASE));
                        res
                    };
                    let tree = SumTree(Arc::new(Node::Internal {
                        height: *height,
                        summary: sum(new_child_summaries.iter(), cx),
                        child_summaries: new_child_summaries,
                        child_trees: {
                            let mut res = ArrayVec::from_iter([tree]);
                            res.extend(child_trees.drain(..TREE_BASE));
                            res
                        },
                    }));

                    *summary = sum(child_summaries.iter(), cx);
                    Some(tree)
                }
            } else {
                None
            }
        }
    }

    // Merge two nodes into `large`.
    //
    // `large` will contain the contents of `small` followed by its own data.
    // If the combined data exceed the node capacity, returns a new node that
    // holds the first half of the merged items and `large` is left with the
    // second half
    //
    // The nodes must be on the same height
    // It only makes sense to call this when `small` is underflowing
    fn merge_into_right(
        small: Self,
        large: &mut Self,
        cx: <<T as Item>::Summary as Summary>::Context<'_>,
    ) -> Option<SumTree<T>> {
        match (small.0.as_ref(), Arc::make_mut(&mut large.0)) {
            (
                Node::Internal {
                    summary: small_summary,
                    child_summaries: small_child_summaries,
                    child_trees: small_child_trees,
                    ..
                },
                Node::Internal { summary, child_summaries, child_trees, height },
            ) => {
                let total_child_count = child_trees.len() + small_child_trees.len();
                if total_child_count <= 2 * TREE_BASE {
                    let mut all_trees = small_child_trees.clone();
                    all_trees.extend(child_trees.drain(..));
                    *child_trees = all_trees;

                    let mut all_summaries = small_child_summaries.clone();
                    all_summaries.extend(child_summaries.drain(..));
                    *child_summaries = all_summaries;

                    let mut full_summary = small_summary.clone();
                    Summary::add_summary(&mut full_summary, summary, cx);
                    *summary = full_summary;
                    None
                } else {
                    let midpoint = total_child_count.div_ceil(2);
                    let mut all_trees = small_child_trees.iter().chain(child_trees.iter()).cloned();
                    let left_trees = all_trees.by_ref().take(midpoint).collect();
                    *child_trees = all_trees.collect();

                    let mut all_summaries = small_child_summaries.iter().chain(child_summaries.iter()).cloned();
                    let left_summaries: ArrayVec<_, { 2 * TREE_BASE }, u8> =
                        all_summaries.by_ref().take(midpoint).collect();
                    *child_summaries = all_summaries.collect();

                    *summary = sum(child_summaries.iter(), cx);
                    Some(SumTree(Arc::new(Node::Internal {
                        height: *height,
                        summary: sum(left_summaries.iter(), cx),
                        child_summaries: left_summaries,
                        child_trees: left_trees,
                    })))
                }
            }
            (
                Node::Leaf { summary: small_summary, items: small_items, item_summaries: small_item_summaries },
                Node::Leaf { summary, items, item_summaries },
            ) => {
                let total_child_count = small_items.len() + items.len();
                if total_child_count <= 2 * TREE_BASE {
                    let mut all_items = small_items.clone();
                    all_items.extend(items.drain(..));
                    *items = all_items;

                    let mut all_summaries = small_item_summaries.clone();
                    all_summaries.extend(item_summaries.drain(..));
                    *item_summaries = all_summaries;

                    let mut full_summary = small_summary.clone();
                    Summary::add_summary(&mut full_summary, summary, cx);
                    *summary = full_summary;
                    None
                } else {
                    let midpoint = total_child_count.div_ceil(2);
                    let mut all_items = small_items.iter().chain(items.iter()).cloned();
                    let left_items = all_items.by_ref().take(midpoint).collect();
                    *items = all_items.collect();

                    let mut all_summaries = small_item_summaries.iter().chain(item_summaries.iter()).cloned();
                    let left_summaries: ArrayVec<_, { 2 * TREE_BASE }, u8> =
                        all_summaries.by_ref().take(midpoint).collect();
                    *item_summaries = all_summaries.collect();

                    *summary = sum(item_summaries.iter(), cx);
                    Some(SumTree(Arc::new(Node::Leaf {
                        items: left_items,
                        summary: sum(left_summaries.iter(), cx),
                        item_summaries: left_summaries,
                    })))
                }
            }
            _ => unreachable!(),
        }
    }

    fn from_child_trees(left: SumTree<T>, right: SumTree<T>, cx: <T::Summary as Summary>::Context<'_>) -> Self {
        let height = left.0.height() + 1;
        let mut child_summaries = ArrayVec::new();
        child_summaries.push(left.0.summary().clone()).unwrap_oob();
        child_summaries.push(right.0.summary().clone()).unwrap_oob();
        let mut child_trees = ArrayVec::new();
        child_trees.push(left).unwrap_oob();
        child_trees.push(right).unwrap_oob();
        SumTree(Arc::new(Node::Internal {
            height,
            summary: sum(child_summaries.iter(), cx),
            child_summaries,
            child_trees,
        }))
    }

    fn leftmost_leaf(&self) -> &Self {
        match *self.0 {
            Node::Leaf { .. } => self,
            Node::Internal { ref child_trees, .. } => {
                child_trees.first().expect("internal nodes are non-empty").leftmost_leaf()
            }
        }
    }

    fn rightmost_leaf(&self) -> &Self {
        match *self.0 {
            Node::Leaf { .. } => self,
            Node::Internal { ref child_trees, .. } => {
                child_trees.last().expect("internal nodes are non-empty").rightmost_leaf()
            }
        }
    }
}

impl<T: Item + PartialEq> PartialEq for SumTree<T> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl<T: Item + Eq> Eq for SumTree<T> {}

impl<T: KeyedItem> SumTree<T> {
    pub fn insert_or_replace(&mut self, item: T, cx: <T::Summary as Summary>::Context<'_>) -> Option<T> {
        self.insert_sorted(std::iter::once(item), cx).pop()
    }

    pub fn remove(&mut self, key: &T::Key, cx: <T::Summary as Summary>::Context<'_>) -> Option<T> {
        let mut removed = None;
        *self = {
            let mut cursor = self.cursor::<T::Key>(cx);
            let mut new_tree = cursor.slice(key, Bias::Left);
            if let Some(item) = cursor.item()
                && item.key() == *key
            {
                removed = Some(item.clone());
                cursor.next();
            }
            new_tree.append(cursor.suffix(), cx);
            new_tree
        };
        removed
    }

    pub fn edit(&mut self, mut edits: Vec<Edit<T>>, cx: <T::Summary as Summary>::Context<'_>) -> Vec<T> {
        if edits.is_empty() {
            return Vec::new();
        }

        let mut removed = Vec::new();
        edits.sort_unstable_by_key(Edit::key);

        // A repeated key can replace items in more than one leaf, which only the slicing below handles.
        if edits.iter().all(|edit| matches!(edit, Edit::Insert(_)))
            && edits.is_sorted_by(|left, right| left.key() < right.key())
        {
            let items = edits.into_iter().filter_map(|edit| match edit {
                Edit::Insert(item) => Some(item),
                Edit::Remove(_) => None,
            });
            return self.insert_sorted(items, cx);
        }

        *self = {
            let mut cursor = self.cursor::<T::Key>(cx);
            let mut new_tree = SumTree::new(cx);
            let mut buffered_items = Vec::new();

            cursor.seek(&T::Key::zero(cx), Bias::Left);
            for edit in edits {
                let new_key = edit.key();
                let mut old_item = cursor.item();

                if old_item.as_ref().is_some_and(|old_item| old_item.key() < new_key) {
                    new_tree.extend(buffered_items.drain(..), cx);
                    let slice = cursor.slice(&new_key, Bias::Left);
                    new_tree.append(slice, cx);
                    old_item = cursor.item();
                }

                if let Some(old_item) = old_item
                    && old_item.key() == new_key
                {
                    removed.push(old_item.clone());
                    cursor.next();
                }

                match edit {
                    Edit::Insert(item) => {
                        buffered_items.push(item);
                    }
                    Edit::Remove(_) => {}
                }
            }

            new_tree.extend(buffered_items, cx);
            new_tree.append(cursor.suffix(), cx);
            new_tree
        };

        removed
    }

    /// Inserts `items`, which are in key order with distinct keys, and returns the items they
    /// replace. Changes the tree in place, cloning only the nodes that other trees share.
    fn insert_sorted(&mut self, items: impl Iterator<Item = T>, cx: <T::Summary as Summary>::Context<'_>) -> Vec<T> {
        let mut removed = Vec::new();
        let mut items = items.map(|item| (item.key(), item)).peekable();
        let siblings = self.insert_recursive(&mut items, T::Key::zero(cx), None, &mut removed, cx);
        if !siblings.is_empty() {
            let mut height = self.0.height();
            let mut nodes = vec![self.clone()];
            nodes.extend(siblings);
            while nodes.len() > 1 {
                height += 1;
                nodes = Self::group_children(nodes, height, cx).collect();
            }
            if let Some(root) = nodes.pop() {
                *self = root;
            }
        }
        removed
    }

    /// Inserts the leading `items` with keys up to `bound` into this subtree, which starts at
    /// `position`. Returns the siblings that the subtree splits off, in order.
    fn insert_recursive<I: Iterator<Item = (T::Key, T)>>(
        &mut self,
        items: &mut std::iter::Peekable<I>,
        mut position: T::Key,
        bound: Option<&T::Key>,
        removed: &mut Vec<T>,
        cx: <T::Summary as Summary>::Context<'_>,
    ) -> Vec<SumTree<T>> {
        let node = Arc::make_mut(&mut self.0);
        let (height, children) = match node {
            Node::Internal { height, summary, child_summaries, child_trees } => {
                let mut splits = Vec::new();
                let child_count = child_trees.len();
                for (index, (child_tree, child_summary)) in
                    child_trees.iter_mut().zip(child_summaries.iter_mut()).enumerate()
                {
                    let Some((key, _)) = items.peek() else {
                        break;
                    };
                    let mut child_end = position.clone();
                    child_end.add_summary(child_summary, cx);
                    // Keys past the end of the tree go to the last child.
                    let child_bound = if index + 1 == child_count { bound } else { Some(&child_end) };
                    if child_bound.is_none_or(|bound| key <= bound) {
                        let siblings = child_tree.insert_recursive(items, position, child_bound, removed, cx);
                        *child_summary = child_tree.summary().clone();
                        if !siblings.is_empty() {
                            splits.push((index, siblings));
                        }
                    }
                    position = child_end;
                }
                if splits.is_empty() {
                    *summary = sum(child_summaries.iter(), cx);
                    return Vec::new();
                }

                let mut children = Vec::new();
                let mut splits = splits.into_iter().peekable();
                for (index, child_tree) in mem::take(child_trees).into_iter().enumerate() {
                    children.push(child_tree);
                    if let Some((_, siblings)) = splits.next_if(|split| split.0 == index) {
                        children.extend(siblings);
                    }
                }
                (*height, children)
            }
            Node::Leaf { items: old_items, .. } => {
                let mut old_items = mem::take(old_items).into_iter().peekable();
                let mut old_key = old_items.peek().map(T::key);
                let mut merged = Vec::new();
                while let Some((key, item)) = items.next_if(|(key, _)| bound.is_none_or(|bound| key <= bound)) {
                    while old_key.as_ref().is_some_and(|old_key| *old_key < key) {
                        merged.extend(old_items.next());
                        old_key = old_items.peek().map(T::key);
                    }
                    if old_key.as_ref() == Some(&key) {
                        removed.extend(old_items.next());
                        old_key = old_items.peek().map(T::key);
                    }
                    merged.push(item);
                }
                merged.extend(old_items);

                let mut merged = merged.into_iter();
                let mut leaves = node_sizes(merged.len()).map(|size| {
                    let items: ArrayVec<T, { 2 * TREE_BASE }, u8> = merged.by_ref().take(size).collect();
                    let item_summaries: ArrayVec<T::Summary, { 2 * TREE_BASE }, u8> =
                        items.iter().map(|item| item.summary(cx)).collect();
                    Node::Leaf { summary: sum(item_summaries.iter(), cx), items, item_summaries }
                });
                if let Some(first) = leaves.next() {
                    *node = first;
                }
                return leaves.map(|leaf| SumTree(Arc::new(leaf))).collect();
            }
        };

        let mut parents = Self::group_children(children, height, cx);
        if let Some(first) = parents.next() {
            *self = first;
        }
        parents.collect()
    }

    /// Groups nodes of one height under the fewest parents of the given height.
    fn group_children(
        children: Vec<SumTree<T>>,
        height: u8,
        cx: <T::Summary as Summary>::Context<'_>,
    ) -> impl Iterator<Item = SumTree<T>> {
        let mut children = children.into_iter();
        node_sizes(children.len()).map(move |size| {
            let child_trees: ArrayVec<SumTree<T>, { 2 * TREE_BASE }, u8> = children.by_ref().take(size).collect();
            let child_summaries: ArrayVec<T::Summary, { 2 * TREE_BASE }, u8> =
                child_trees.iter().map(|tree| tree.summary().clone()).collect();
            SumTree(Arc::new(Node::Internal {
                height,
                summary: sum(child_summaries.iter(), cx),
                child_summaries,
                child_trees,
            }))
        })
    }

    pub fn get<'a>(&'a self, key: &T::Key, cx: <T::Summary as Summary>::Context<'a>) -> Option<&'a T> {
        if let (_, _, Some(item)) = self.find_exact::<T::Key, _>(cx, key, Bias::Left) { Some(item) } else { None }
    }
}

impl<T, S> Default for SumTree<T>
where
    T: Item<Summary = S>,
    S: for<'a> Summary<Context<'a> = ()>,
{
    fn default() -> Self {
        Self::new(())
    }
}

#[derive(Clone)]
pub enum Node<T: Item> {
    Internal {
        height: u8,
        summary: T::Summary,
        child_summaries: ArrayVec<T::Summary, { 2 * TREE_BASE }, u8>,
        child_trees: ArrayVec<SumTree<T>, { 2 * TREE_BASE }, u8>,
    },
    Leaf {
        summary: T::Summary,
        items: ArrayVec<T, { 2 * TREE_BASE }, u8>,
        item_summaries: ArrayVec<T::Summary, { 2 * TREE_BASE }, u8>,
    },
}

impl<T> fmt::Debug for Node<T>
where
    T: Item + fmt::Debug,
    T::Summary: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Internal { height, summary, child_summaries, child_trees } => f
                .debug_struct("Internal")
                .field("height", height)
                .field("summary", summary)
                .field("child_summaries", child_summaries)
                .field("child_trees", child_trees)
                .finish(),
            Node::Leaf { summary, items, item_summaries } => f
                .debug_struct("Leaf")
                .field("summary", summary)
                .field("items", items)
                .field("item_summaries", item_summaries)
                .finish(),
        }
    }
}

impl<T: Item> Node<T> {
    const fn is_leaf(&self) -> bool {
        matches!(self, Node::Leaf { .. })
    }

    const fn height(&self) -> u8 {
        match self {
            Node::Internal { height, .. } => *height,
            Node::Leaf { .. } => 0,
        }
    }

    const fn summary(&self) -> &T::Summary {
        match self {
            Node::Internal { summary, .. } => summary,
            Node::Leaf { summary, .. } => summary,
        }
    }

    fn child_summaries(&self) -> &[T::Summary] {
        match self {
            Node::Internal { child_summaries, .. } => child_summaries.as_slice(),
            Node::Leaf { item_summaries, .. } => item_summaries.as_slice(),
        }
    }

    fn child_trees(&self) -> &ArrayVec<SumTree<T>, { 2 * TREE_BASE }, u8> {
        match self {
            Node::Internal { child_trees, .. } => child_trees,
            Node::Leaf { .. } => panic!("Leaf nodes have no child trees"),
        }
    }

    fn items(&self) -> &ArrayVec<T, { 2 * TREE_BASE }, u8> {
        match self {
            Node::Leaf { items, .. } => items,
            Node::Internal { .. } => panic!("Internal nodes have no items"),
        }
    }

    fn is_underflowing(&self) -> bool {
        match self {
            Node::Internal { child_trees, .. } => child_trees.len() < TREE_BASE,
            Node::Leaf { items, .. } => items.len() < TREE_BASE,
        }
    }
}

#[derive(Debug)]
pub enum Edit<T: KeyedItem> {
    Insert(T),
    Remove(T::Key),
}

impl<T: KeyedItem> Edit<T> {
    fn key(&self) -> T::Key {
        match self {
            Edit::Insert(item) => item.key(),
            Edit::Remove(key) => key.clone(),
        }
    }
}

fn sum<'a, T, I>(iter: I, cx: T::Context<'_>) -> T
where
    T: 'a + Summary,
    I: Iterator<Item = &'a T>,
{
    let mut sum = T::zero(cx);
    for value in iter {
        sum.add_summary(value, cx);
    }
    sum
}

/// Returns the sizes of the fewest nodes that hold `count` children, spread evenly so that none
/// underflows.
fn node_sizes(count: usize) -> impl Iterator<Item = usize> {
    let node_count = count.div_ceil(2 * TREE_BASE);
    (0..node_count).map(move |index| count / node_count + usize::from(index < count % node_count))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Max {
        max: u8,
        count: usize,
    }

    impl ContextLessSummary for Max {
        fn zero() -> Self {
            Self { max: 0, count: 0 }
        }

        fn add_summary(&mut self, summary: &Self) {
            self.max = self.max.max(summary.max);
            self.count += summary.count;
        }
    }

    impl Item for u8 {
        type Summary = Max;

        fn summary(&self, (): ()) -> Max {
            Max { max: *self, count: 1 }
        }
    }

    impl KeyedItem for u8 {
        type Key = u8;

        fn key(&self) -> u8 {
            *self
        }
    }

    impl Dimension<'_, Max> for u8 {
        fn zero((): ()) -> Self {
            0
        }

        fn add_summary(&mut self, summary: &Max, (): ()) {
            *self = summary.max;
        }
    }

    // xorshift, as the crate has no rand dependency
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: u64) -> u64 {
            (self.next() >> 16) % bound
        }
    }

    fn check(tree: &SumTree<u8>, is_root: bool, may_underflow: bool) -> u8 {
        let node = tree.0.as_ref();
        let child_summaries = node.child_summaries();
        assert!(is_root || may_underflow || !node.is_underflowing());
        assert_eq!(node.summary(), &sum(child_summaries.iter(), ()));
        match node {
            Node::Leaf { items, .. } => {
                assert!(items.iter().map(|item| item.summary(())).eq(child_summaries.iter().cloned()));
                0
            }
            Node::Internal { height, child_trees, .. } => {
                for (child_tree, child_summary) in child_trees.iter().zip(child_summaries) {
                    assert_eq!(child_tree.summary(), child_summary);
                    assert_eq!(check(child_tree, false, may_underflow) + 1, *height);
                }
                *height
            }
        }
    }

    #[test]
    fn edit_replaces_a_repeated_key_across_leaves() {
        // Each insert of a repeated key replaces one item, here in two different leaves.
        let mut tree = SumTree::from_iter([1_u8, 2, 3, 5, 5, 6, 7, 8], ());
        let removed = tree.edit(vec![Edit::Insert(5), Edit::Insert(5)], ());
        assert_eq!(removed, vec![5, 5]);
        assert_eq!(tree.items(()), vec![1, 2, 3, 5, 5, 6, 7, 8]);
    }

    #[test]
    fn inserts_in_place() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..1000 {
            let mut tree = SumTree::<u8>::new(());
            let mut expected = BTreeSet::new();
            let mut snapshots = Vec::new();
            // Removals slice the tree, which leaves nodes that underflow.
            let with_removals = rng.below(2) == 0;
            for _ in 0..=rng.below(9) {
                // A narrow range puts a whole batch in a few leaves.
                let (start, width) = (rng.below(256) as u8, if rng.below(2) == 0 { 9 } else { 256 });
                let len = rng.below(39) as usize + 1;
                let batch = std::iter::repeat_with(|| start.wrapping_add(rng.below(width) as u8))
                    .take(len)
                    .collect::<BTreeSet<u8>>();
                let remove = with_removals && rng.below(2) == 0;
                let edit: fn(u8) -> Edit<u8> = if remove { Edit::Remove } else { Edit::Insert };
                // Reversed, so `edit` has to sort.
                let edits = batch.iter().rev().copied().map(edit).collect::<Vec<_>>();

                let replaced = match edits.as_slice() {
                    [Edit::Insert(item)] => tree.insert_or_replace(*item, ()).into_iter().collect(),
                    _ => tree.edit(edits, ()),
                };
                assert!(replaced.iter().eq(batch.intersection(&expected)));
                if remove {
                    expected.retain(|item| !batch.contains(item));
                } else {
                    expected.extend(batch);
                }
                check(&tree, true, with_removals);
                assert!(tree.iter().eq(&expected));
                if rng.below(2) == 0 {
                    snapshots.push((tree.clone(), expected.clone()));
                }
                for (snapshot, expected) in &snapshots {
                    assert!(snapshot.iter().eq(expected));
                }
            }
        }
    }

    #[test]
    fn node_sizes_fill_the_fewest_nodes_evenly() {
        assert_eq!(node_sizes(0).count(), 0);
        assert_eq!(node_sizes(12).collect::<Vec<_>>(), vec![12]);
        assert_eq!(node_sizes(13).collect::<Vec<_>>(), vec![7, 6]);
        assert_eq!(node_sizes(25).collect::<Vec<_>>(), vec![9, 8, 8]);
    }
}
