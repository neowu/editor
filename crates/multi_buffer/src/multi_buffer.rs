use std::hash::Hash;
use std::hash::Hasher;

use itertools::Itertools as _;
use language::OffsetRangeExt as _;

mod anchor;
mod anchor_resolve;
mod cursor;
mod dimension;
mod iterators;
mod path_key;
mod sync;
mod syntax;
mod transaction;

use std::any::type_name;
use std::borrow::Cow;
use std::cell::OnceCell;
use std::cell::Ref;
use std::cell::RefCell;
use std::cmp;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::Bound;
use std::fmt;
use std::iter;
use std::mem;
use std::ops;
use std::ops::AddAssign;
use std::ops::Range;
use std::ops::RangeBounds;
use std::ops::Sub;
use std::rc::Rc;
use std::str;
use std::sync::Arc;
use std::time::Duration;

pub use anchor::Anchor;
pub use anchor::AnchorRangeExt;
use anchor::AnchorSeekTarget;
use anchor::ExcerptAnchor;
use buffer_diff::BufferDiff;
use buffer_diff::BufferDiffEvent;
use buffer_diff::BufferDiffSnapshot;
use buffer_diff::DiffChanged;
use buffer_diff::DiffHunkSecondaryStatus;
use buffer_diff::DiffHunkStatus;
use buffer_diff::DiffHunkStatusKind;
use gpui::App;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use indexmap::IndexSet;
use language::AutoIndentExclusion;
use language::AutoindentMode;
use language::Buffer;
use language::BufferRow;
use language::BufferSnapshot;
use language::Capability;
use language::CharKind;
use language::CharScopeContext;
use language::File;
use language::IndentGuideSettings;
use language::Language;
use language::LanguageAwareStyling;
use language::Point;
use language::PointUtf16;
use language::TextDimension;
use language::ToOffset as _;
use language::ToPoint as _;
use language::TransactionId;
use language::Unclipped;
use language::language_settings::AllLanguageSettings;
use language::language_settings::LanguageSettings;
use rope::DimensionPair;
use rustc_hash::FxBuildHasher;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use sum_tree::Bias;
use sum_tree::Dimension;
use sum_tree::Dimensions;
use sum_tree::SumTree;
use sum_tree::TreeMap;
use text::BufferId;
use text::Edit;
use text::TextSummary;
use text::subscription::Subscription;
use text::subscription::Topic;

use self::cursor::MultiBufferCursor;
pub use self::dimension::BufferOffset;
pub use self::dimension::BufferOffsetUtf16;
use self::dimension::ExcerptDimension;
use self::dimension::ExcerptOffset;
pub use self::dimension::MBTextSummary;
pub use self::dimension::MultiBufferDimension;
pub use self::dimension::MultiBufferOffset;
pub use self::dimension::MultiBufferOffsetUtf16;
pub use self::dimension::MultiBufferPoint;
pub use self::dimension::MultiBufferRow;
use self::dimension::OutputDimension;
pub use self::dimension::ToOffset;
pub use self::dimension::ToPoint;
use self::iterators::ExcerptChunks;
pub use self::iterators::MultiBufferBytes;
pub use self::iterators::MultiBufferChunks;
pub use self::iterators::MultiBufferRows;
pub use self::iterators::ReversedMultiBufferBytes;
pub use self::iterators::ReversedMultiBufferChunks;
pub use self::path_key::PathKey;
use self::transaction::History;

/// Lines of context shown around each multibuffer excerpt.
pub const EXCERPT_CONTEXT_LINES: u32 = 5;

/// One or more [`Buffers`](Buffer) being edited in a single view.
///
/// See <https://zed.dev/features#multi-buffers>.
pub struct MultiBuffer {
    /// A snapshot of the [`Excerpt`]s in the MultiBuffer.
    /// Use [`MultiBuffer::snapshot`] to get a up-to-date snapshot.
    snapshot: RefCell<MultiBufferSnapshot>,
    /// Contains the state of the buffers being edited.
    buffers: BTreeMap<BufferId, BufferState>,
    /// Mapping from buffer IDs to their diff states.
    diffs: FxHashMap<BufferId, DiffState>,
    subscriptions: Topic<MultiBufferOffset>,
    /// If true, the multi-buffer only contains a single [`Buffer`] and a single [`Excerpt`].
    singleton: bool,
    /// The history of the multi-buffer.
    history: History,
    /// The explicit title of the multi-buffer.
    /// If `None`, it will be derived from the underlying path or content.
    title: Option<String>,
    /// The writing capability of the multi-buffer.
    capability: Capability,
    /// Ids of the buffers that changed or were added since the last sync. Buffers add their ids
    /// through [`Buffer::record_changes`].
    changed_buffers: Rc<RefCell<FxHashSet<BufferId>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PathKeyIndex(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    BufferRangesUpdated { buffer: Entity<Buffer>, path_key: PathKey, ranges: Vec<ExcerptRange<text::Anchor>> },
    BuffersRemoved { removed_buffer_ids: Vec<BufferId> },
    BuffersEdited { buffer_ids: Vec<BufferId> },
    DiffHunksToggled,
    Edited { edited_buffer: Option<Entity<Buffer>> },
    TransactionUndone { transaction_id: TransactionId },
    Reloaded,
    CapabilityChanged,
    LanguageChanged(BufferId, bool),
    SettingsChanged,
    Reparsed(BufferId),
    Saved,
    FileHandleChanged,
    DirtyChanged,
    DiagnosticsUpdated,
    BufferDiffChanged,
}

/// A diff hunk, representing a range of consequent lines in a multibuffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiBufferDiffHunk {
    /// The row range in the multibuffer where this diff hunk appears.
    pub row_range: Range<MultiBufferRow>,
    /// The buffer ID that this hunk belongs to.
    pub buffer_id: BufferId,
    /// The range of the underlying buffer that this hunk corresponds to.
    pub buffer_range: Range<text::Anchor>,
    /// The range within the buffer's diff base that this hunk corresponds to.
    pub diff_base_byte_range: Range<BufferOffset>,
    /// The status of this hunk (added/modified/deleted and secondary status).
    pub status: DiffHunkStatus,
    /// The word diffs for this hunk.
    pub word_diffs: Vec<Range<MultiBufferOffset>>,
    pub excerpt_range: ExcerptRange<text::Anchor>,
    pub multi_buffer_range: Range<Anchor>,
}

impl MultiBufferDiffHunk {
    pub const fn status(&self) -> DiffHunkStatus {
        self.status
    }

    pub fn is_created_file(&self) -> bool {
        self.diff_base_byte_range == (BufferOffset(0)..BufferOffset(0))
            && self.buffer_range.start.is_min()
            && self.buffer_range.end.is_max()
    }
}

struct BufferState {
    buffer: Entity<Buffer>,
    _subscriptions: [gpui::Subscription; 2],
}

struct DiffState {
    diff: Entity<BufferDiff>,
    main_buffer: Option<Entity<Buffer>>,
    _subscription: gpui::Subscription,
}

impl DiffState {
    fn snapshot(&self, buffer_id: BufferId, cx: &App) -> DiffStateSnapshot {
        DiffStateSnapshot {
            buffer_id,
            diff: self.diff.read(cx).snapshot(cx),
            main_buffer: self.main_buffer.as_ref().map(|b| b.read(cx).snapshot()),
        }
    }

    fn new(diff: Entity<BufferDiff>, cx: &mut Context<MultiBuffer>) -> Self {
        DiffState {
            _subscription: cx.subscribe(&diff, |this, diff, event, cx| match event {
                BufferDiffEvent::DiffChanged(DiffChanged {
                    changed_range,
                    base_text_changed_range: _,
                    extended_range,
                    base_text_changed: _,
                }) => {
                    let use_extended = this.snapshot.borrow().use_extended_diff_range;
                    let range = if use_extended { extended_range.clone() } else { changed_range.clone() };
                    this.buffer_diff_changed(diff, range, cx);
                    cx.emit(Event::BufferDiffChanged);
                }
                BufferDiffEvent::BaseTextChanged => {}
            }),
            diff,
            main_buffer: None,
        }
    }

    fn new_inverted(diff: Entity<BufferDiff>, main_buffer: Entity<Buffer>, cx: &mut Context<MultiBuffer>) -> Self {
        let weak_main_buffer = main_buffer.downgrade();
        DiffState {
            _subscription: cx.subscribe(&diff, {
                move |this, diff, event, cx| {
                    let Some(main_buffer) = weak_main_buffer.upgrade() else {
                        return;
                    };
                    match event {
                        BufferDiffEvent::DiffChanged(DiffChanged {
                            changed_range: _,
                            base_text_changed_range,
                            extended_range: _,
                            base_text_changed: _,
                        }) => {
                            this.inverted_buffer_diff_changed(diff, main_buffer, base_text_changed_range.clone(), cx);
                            cx.emit(Event::BufferDiffChanged);
                        }
                        BufferDiffEvent::BaseTextChanged => {}
                    }
                }
            }),
            diff,
            main_buffer: Some(main_buffer),
        }
    }
}

#[derive(Clone)]
struct DiffStateSnapshot {
    buffer_id: BufferId,
    diff: BufferDiffSnapshot,
    main_buffer: Option<BufferSnapshot>,
}

impl ops::Deref for DiffStateSnapshot {
    type Target = BufferDiffSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.diff
    }
}

#[derive(Clone, Debug, Default)]
struct DiffStateSummary {
    max_buffer_id: Option<BufferId>,
    added_rows: u32,
    removed_rows: u32,
}

impl sum_tree::ContextLessSummary for DiffStateSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.max_buffer_id = cmp::max(self.max_buffer_id, other.max_buffer_id);
        self.added_rows += other.added_rows;
        self.removed_rows += other.removed_rows;
    }
}

impl sum_tree::Item for DiffStateSnapshot {
    type Summary = DiffStateSummary;

    fn summary(&self, _cx: ()) -> DiffStateSummary {
        let (added_rows, removed_rows) = self.diff.changed_row_counts();
        DiffStateSummary { max_buffer_id: Some(self.buffer_id), added_rows, removed_rows }
    }
}

impl sum_tree::KeyedItem for DiffStateSnapshot {
    type Key = Option<BufferId>;

    fn key(&self) -> Option<BufferId> {
        Some(self.buffer_id)
    }
}

impl Dimension<'_, DiffStateSummary> for Option<BufferId> {
    fn zero(_cx: ()) -> Self {
        None
    }

    fn add_summary(&mut self, summary: &DiffStateSummary, _cx: ()) {
        *self = cmp::max(*self, summary.max_buffer_id);
    }
}

fn find_diff_state(diffs: &SumTree<DiffStateSnapshot>, buffer_id: BufferId) -> Option<&DiffStateSnapshot> {
    let key = Some(buffer_id);
    let (.., item) = diffs.find::<Option<BufferId>, _>((), &key, Bias::Left);
    item.filter(|entry| entry.buffer_id == buffer_id)
}

fn remove_diff_state(diffs: &mut SumTree<DiffStateSnapshot>, buffer_id: BufferId) {
    let key = Some(buffer_id);
    let mut cursor = diffs.cursor::<Option<BufferId>>(());
    let mut new_tree = cursor.slice(&key, Bias::Left);
    if key == cursor.end() {
        cursor.next();
    }
    new_tree.append(cursor.suffix(), ());
    drop(cursor);
    *diffs = new_tree;
}

#[derive(Clone)]
struct BufferStateSnapshot {
    pub(crate) path_key: PathKey,
    path_key_index: PathKeyIndex,
    buffer_snapshot: BufferSnapshot,
    status: BufferStatus,
}

/// Buffer flags that `MultiBufferSnapshot` aggregates across its buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferStatus {
    is_dirty: bool,
    has_deleted_file: bool,
    has_conflict: bool,
}

impl BufferStatus {
    fn new(buffer: &Buffer) -> Self {
        Self {
            is_dirty: buffer.is_dirty(),
            has_deleted_file: buffer.file().is_some_and(|file| file.disk_state().is_deleted()),
            has_conflict: buffer.has_conflict(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BufferStatusCounts {
    dirty: usize,
    deleted_file: usize,
    conflict: usize,
}

impl BufferStatusCounts {
    fn add(&mut self, status: BufferStatus) {
        self.dirty += usize::from(status.is_dirty);
        self.deleted_file += usize::from(status.has_deleted_file);
        self.conflict += usize::from(status.has_conflict);
    }

    fn remove(&mut self, status: BufferStatus) {
        self.dirty -= usize::from(status.is_dirty);
        self.deleted_file -= usize::from(status.has_deleted_file);
        self.conflict -= usize::from(status.has_conflict);
    }
}

/// Buffer state snapshots by buffer id, with counts of the buffers that have each status flag.
#[derive(Clone, Debug, Default)]
struct BufferStateSnapshots {
    snapshots: TreeMap<BufferId, BufferStateSnapshot>,
    status_counts: BufferStatusCounts,
}

impl ops::Deref for BufferStateSnapshots {
    type Target = TreeMap<BufferId, BufferStateSnapshot>;

    fn deref(&self) -> &Self::Target {
        &self.snapshots
    }
}

impl BufferStateSnapshots {
    fn insert(&mut self, buffer_id: BufferId, buffer_state: BufferStateSnapshot) {
        self.status_counts.add(buffer_state.status);
        if let Some(old_buffer_state) = self.snapshots.insert_or_replace(buffer_id, buffer_state) {
            self.status_counts.remove(old_buffer_state.status);
        }
    }

    fn remove(&mut self, buffer_id: BufferId) {
        if let Some(buffer_state) = self.snapshots.remove(&buffer_id) {
            self.status_counts.remove(buffer_state.status);
        }
    }
}

impl fmt::Debug for BufferStateSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferStateSnapshot")
            .field("path_key", &self.path_key)
            .field("buffer_id", &self.buffer_snapshot.buffer_id())
            .finish()
    }
}

/// The contents of a [`MultiBuffer`] at a single point in time.
#[derive(Clone, Default)]
pub struct MultiBufferSnapshot {
    excerpts: SumTree<Excerpt>,
    buffers: BufferStateSnapshots,
    path_keys: Arc<IndexSet<PathKey, FxBuildHasher>>,
    diffs: SumTree<DiffStateSnapshot>,
    diff_transforms: SumTree<DiffTransform>,
    non_text_state_update_count: usize,
    edit_count: usize,
    has_inverted_diff: bool,
    singleton: bool,
    trailing_excerpt_update_count: usize,
    all_diff_hunks_expanded: bool,
    show_deleted_hunks: bool,
    use_extended_diff_range: bool,
    show_headers: bool,
}

#[derive(Debug, Clone)]
enum DiffTransform {
    BufferContent {
        summary: MBTextSummary,
        inserted_hunk_info: Option<DiffTransformHunkInfo>,
    },
    DeletedHunk {
        summary: TextSummary,
        buffer_id: BufferId,
        hunk_info: DiffTransformHunkInfo,
        base_text_byte_range: Range<usize>,
        has_trailing_newline: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct DiffTransformHunkInfo {
    buffer_id: BufferId,
    hunk_start_anchor: text::Anchor,
    hunk_secondary_status: DiffHunkSecondaryStatus,
    is_logically_deleted: bool,
    excerpt_end: ExcerptAnchor,
}

impl Eq for DiffTransformHunkInfo {}

impl PartialEq for DiffTransformHunkInfo {
    fn eq(&self, other: &DiffTransformHunkInfo) -> bool {
        self.buffer_id == other.buffer_id && self.hunk_start_anchor == other.hunk_start_anchor
    }
}

impl Hash for DiffTransformHunkInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.buffer_id.hash(state);
        self.hunk_start_anchor.hash(state);
    }
}

#[derive(Clone)]
pub struct ExcerptBoundaryInfo {
    pub start_anchor: Anchor,
    pub range: ExcerptRange<text::Anchor>,
    pub end_row: MultiBufferRow,
}

impl ExcerptBoundaryInfo {
    pub const fn start_text_anchor(&self) -> text::Anchor {
        self.range.context.start
    }
    pub const fn buffer_id(&self) -> BufferId {
        self.start_text_anchor().buffer_id
    }
    pub fn buffer<'a>(&self, snapshot: &'a MultiBufferSnapshot) -> &'a BufferSnapshot {
        let buffer_id = self.buffer_id();
        snapshot.buffer_for_id(buffer_id).unwrap_or_else(|| {
            let mut excerpt_buffer_ids = Vec::new();
            for excerpt in snapshot.excerpts.iter() {
                let excerpt_ids = (excerpt.buffer_id, excerpt.range.context.start.buffer_id, excerpt.path_key_index);
                if excerpt_buffer_ids.last() != Some(&excerpt_ids) {
                    excerpt_buffer_ids.push(excerpt_ids);
                }
            }
            let present_buffer_ids =
                snapshot.buffers.iter().map(|(id, state)| (*id, state.path_key_index)).collect::<Vec<_>>();
            panic!(
                "buffer snapshot not found for excerpt boundary: sought buffer {buffer_id} at \
                 path index {:?}; buffers present (id, path index): {present_buffer_ids:?}; \
                 excerpts present (buffer id, anchor buffer id, path index): \
                 {excerpt_buffer_ids:?}; singleton={}, has_inverted_diff={}, \
                 trailing_excerpt_update_count={}",
                match self.start_anchor {
                    Anchor::Excerpt(anchor) => Some(anchor.path),
                    Anchor::Min | Anchor::Max => None,
                },
                snapshot.singleton,
                snapshot.has_inverted_diff,
                snapshot.trailing_excerpt_update_count,
            )
        })
    }
}

impl fmt::Debug for ExcerptBoundaryInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).field("buffer_id", &self.buffer_id()).field("range", &self.range).finish()
    }
}

impl PartialEq for ExcerptBoundaryInfo {
    fn eq(&self, other: &Self) -> bool {
        self.start_anchor == other.start_anchor && self.range == other.range
    }
}

impl Eq for ExcerptBoundaryInfo {}

/// A boundary between `Excerpt`s in a [`MultiBuffer`].
#[derive(Debug)]
pub struct ExcerptBoundary {
    pub prev: Option<ExcerptBoundaryInfo>,
    pub next: ExcerptBoundaryInfo,
    /// The row in the `MultiBuffer` where the boundary is located.
    pub row: MultiBufferRow,
}

impl ExcerptBoundary {
    pub fn starts_new_buffer(&self) -> bool {
        match (self.prev.as_ref(), &self.next) {
            (None, _) => true,
            (Some(prev), next) => prev.buffer_id() != next.buffer_id(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExpandInfo {
    pub direction: ExpandExcerptDirection,
    pub start_anchor: Anchor,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RowInfo {
    pub buffer_id: Option<BufferId>,
    pub buffer_row: Option<u32>,
    pub multibuffer_row: Option<MultiBufferRow>,
    pub diff_status: Option<DiffHunkStatus>,
    pub expand_info: Option<ExpandInfo>,
    pub wrapped_buffer_row: Option<u32>,
}

/// A slice into a [`Buffer`] that is being edited in a [`MultiBuffer`].
#[derive(Clone, Debug)]
pub(crate) struct Excerpt {
    /// The location of the excerpt in the [`MultiBuffer`].
    pub(crate) path_key: PathKey,
    pub(crate) path_key_index: PathKeyIndex,
    pub(crate) buffer_id: BufferId,
    /// The range of the buffer to be shown in the excerpt.
    pub(crate) range: ExcerptRange<text::Anchor>,

    /// The last row in the excerpted slice of the buffer.
    pub(crate) max_buffer_row: BufferRow,
    /// A summary of the text in the excerpt.
    pub(crate) text_summary: TextSummary,
    pub(crate) has_trailing_newline: bool,
}

/// A range of text from a single [`Buffer`], to be shown as an `Excerpt`.
/// These ranges are relative to the buffer itself.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ExcerptRange<T> {
    /// The full range of text to be shown in the excerpt.
    pub context: Range<T>,
    /// The primary range of text to be highlighted in the excerpt.
    /// In a multi-buffer search, this would be the text that matched the search.
    pub primary: Range<T>,
}

impl<T: Clone> ExcerptRange<T> {
    pub fn new(context: Range<T>) -> Self {
        Self { context: context.clone(), primary: context }
    }
}

impl ExcerptRange<text::Anchor> {
    pub fn contains(&self, t: &text::Anchor, snapshot: &BufferSnapshot) -> bool {
        self.context.start.cmp(t, snapshot).is_le() && self.context.end.cmp(t, snapshot).is_ge()
    }
}

#[derive(Clone, Debug)]
pub struct ExcerptSummary {
    path_key: PathKey,
    path_key_index: Option<PathKeyIndex>,
    max_anchor: Option<text::Anchor>,
    widest_line_number: u32,
    text: MBTextSummary,
    count: usize,
}

impl ExcerptSummary {
    pub fn min() -> Self {
        ExcerptSummary {
            path_key: PathKey::min(),
            path_key_index: None,
            max_anchor: None,
            widest_line_number: 0,
            text: MBTextSummary::default(),
            count: 0,
        }
    }

    const fn len(&self) -> ExcerptOffset {
        ExcerptDimension(self.text.len)
    }
}

#[derive(Debug, Clone)]
pub struct DiffTransformSummary {
    input: MBTextSummary,
    output: MBTextSummary,
}

#[derive(Debug)]
struct BufferEdit {
    range: Range<BufferOffset>,
    new_text: Arc<str>,
    is_insertion: bool,
    original_indent_column: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DiffChangeKind {
    BufferEdited,
    DiffUpdated { base_changed: bool },
    ExpandOrCollapseHunks { expand: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpandExcerptDirection {
    Up,
    Down,
    UpAndDown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentGuide {
    pub buffer_id: BufferId,
    pub start_row: MultiBufferRow,
    pub end_row: MultiBufferRow,
    pub depth: u32,
    pub tab_size: u32,
    pub settings: IndentGuideSettings,
}

impl IndentGuide {
    pub const fn indent_level(&self) -> u32 {
        self.depth * self.tab_size
    }
}

impl MultiBuffer {
    pub fn new(capability: Capability) -> Self {
        Self::new_(
            capability,
            MultiBufferSnapshot { show_headers: true, show_deleted_hunks: true, ..MultiBufferSnapshot::default() },
        )
    }

    pub fn without_headers(capability: Capability) -> Self {
        Self::new_(capability, MultiBufferSnapshot { show_deleted_hunks: true, ..MultiBufferSnapshot::default() })
    }

    pub fn singleton(buffer: Entity<Buffer>, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new_(
            buffer.read(cx).capability(),
            MultiBufferSnapshot { singleton: true, show_deleted_hunks: true, ..MultiBufferSnapshot::default() },
        );
        this.singleton = true;
        this.set_excerpts_for_path(
            PathKey::sorted(0),
            buffer.clone(),
            [Point::zero()..buffer.read(cx).max_point()],
            0,
            cx,
        );
        this
    }

    #[inline]
    pub fn new_(capability: Capability, snapshot: MultiBufferSnapshot) -> Self {
        Self {
            snapshot: RefCell::new(snapshot),
            buffers: BTreeMap::default(),
            diffs: FxHashMap::default(),
            subscriptions: Topic::default(),
            singleton: false,
            capability,
            title: None,
            changed_buffers: Rc::default(),
            history: History::default(),
        }
    }

    pub fn clone(&self, new_cx: &mut Context<Self>) -> Self {
        let mut buffers = BTreeMap::default();
        let changed_buffers = Rc::new(RefCell::new(self.changed_buffers.borrow().clone()));
        for (buffer_id, buffer_state) in &self.buffers {
            buffer_state.buffer.update(new_cx, |buffer, _| {
                buffer.record_changes(Rc::downgrade(&changed_buffers));
            });
            buffers.insert(
                *buffer_id,
                BufferState {
                    buffer: buffer_state.buffer.clone(),
                    _subscriptions: [
                        new_cx.observe(&buffer_state.buffer, |_, _, cx| cx.notify()),
                        new_cx.subscribe(&buffer_state.buffer, Self::on_buffer_event),
                    ],
                },
            );
        }
        let mut diff_bases = FxHashMap::default();
        for (buffer_id, diff) in &self.diffs {
            diff_bases.insert(*buffer_id, DiffState::new(diff.diff.clone(), new_cx));
        }
        Self {
            snapshot: RefCell::new(self.snapshot.borrow().clone()),
            buffers,
            diffs: diff_bases,
            subscriptions: Topic::default(),
            singleton: self.singleton,
            capability: self.capability,
            history: self.history.clone(),
            title: self.title.clone(),
            changed_buffers,
        }
    }

    pub fn set_group_interval(&mut self, group_interval: Duration, cx: &mut Context<Self>) {
        self.history.set_group_interval(group_interval);
        if self.singleton {
            for BufferState { buffer, .. } in self.buffers.values() {
                buffer.update(cx, |buffer, _| {
                    buffer.set_group_interval(group_interval);
                });
            }
        }
    }

    pub fn with_title(mut self, title: String) -> Self {
        self.title = Some(title);
        self
    }

    pub const fn read_only(&self) -> bool {
        !self.capability.editable()
    }

    pub const fn capability(&self) -> Capability {
        self.capability
    }

    /// Returns an up-to-date snapshot of the MultiBuffer.
    pub fn snapshot(&self, cx: &App) -> MultiBufferSnapshot {
        self.sync(cx);
        self.snapshot.borrow().clone()
    }

    pub fn read(&self, cx: &App) -> Ref<'_, MultiBufferSnapshot> {
        self.sync(cx);
        self.snapshot.borrow()
    }

    pub fn as_singleton(&self) -> Option<Entity<Buffer>> {
        self.singleton.then(|| self.buffers.values().next().expect("singleton has one buffer").buffer.clone())
    }

    pub const fn is_singleton(&self) -> bool {
        self.singleton
    }

    pub fn subscribe(&mut self) -> Subscription<MultiBufferOffset> {
        self.subscriptions.subscribe()
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty()
    }

    pub fn has_deleted_file(&self, cx: &App) -> bool {
        self.read(cx).has_deleted_file()
    }

    pub fn has_conflict(&self, cx: &App) -> bool {
        self.read(cx).has_conflict()
    }

    pub fn len(&self, cx: &App) -> MultiBufferOffset {
        self.read(cx).len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    pub fn edit<I, S, T>(&mut self, edits: I, autoindent_mode: Option<AutoindentMode>, cx: &mut Context<Self>)
    where
        I: IntoIterator<Item = (Range<S>, T)>,
        S: ToOffset,
        T: Into<Arc<str>>,
    {
        self.edit_internal(edits, autoindent_mode, true, AutoIndentExclusion::PrecedingLine, cx);
    }

    pub fn edit_before<I, S, T>(&mut self, edits: I, autoindent_mode: Option<AutoindentMode>, cx: &mut Context<Self>)
    where
        I: IntoIterator<Item = (Range<S>, T)>,
        S: ToOffset,
        T: Into<Arc<str>>,
    {
        self.edit_internal(edits, autoindent_mode, true, AutoIndentExclusion::FollowingLine, cx);
    }

    pub fn edit_non_coalesce<I, S, T>(
        &mut self,
        edits: I,
        autoindent_mode: Option<AutoindentMode>,
        cx: &mut Context<Self>,
    ) where
        I: IntoIterator<Item = (Range<S>, T)>,
        S: ToOffset,
        T: Into<Arc<str>>,
    {
        self.edit_internal(edits, autoindent_mode, false, AutoIndentExclusion::PrecedingLine, cx);
    }

    fn edit_internal<I, S, T>(
        &mut self,
        edits: I,
        autoindent_mode: Option<AutoindentMode>,
        coalesce_adjacent: bool,
        autoindent_exclusion: AutoIndentExclusion,
        cx: &mut Context<Self>,
    ) where
        I: IntoIterator<Item = (Range<S>, T)>,
        S: ToOffset,
        T: Into<Arc<str>>,
    {
        if self.read_only() || self.buffers.is_empty() {
            return;
        }
        self.sync_mut(cx);
        let edits = edits
            .into_iter()
            .map(|(range, new_text)| {
                let mut range =
                    range.start.to_offset(self.snapshot.get_mut())..range.end.to_offset(self.snapshot.get_mut());
                if range.start > range.end {
                    mem::swap(&mut range.start, &mut range.end);
                }
                (range, new_text.into())
            })
            .collect::<Vec<_>>();

        return edit_internal(self, edits, autoindent_mode, coalesce_adjacent, autoindent_exclusion, cx);

        // Non-generic part of edit, hoisted out to avoid blowing up LLVM IR.
        fn edit_internal(
            this: &mut MultiBuffer,
            edits: Vec<(Range<MultiBufferOffset>, Arc<str>)>,
            mut autoindent_mode: Option<AutoindentMode>,
            coalesce_adjacent: bool,
            autoindent_exclusion: AutoIndentExclusion,
            cx: &mut Context<MultiBuffer>,
        ) {
            let original_indent_columns = match &mut autoindent_mode {
                Some(AutoindentMode::Block { original_indent_columns }) => mem::take(original_indent_columns),
                _ => Vec::default(),
            };

            let buffer_edits =
                MultiBuffer::convert_edits_to_buffer_edits(edits, this.snapshot.get_mut(), &original_indent_columns);

            let mut buffer_ids = Vec::with_capacity(buffer_edits.len());
            for (buffer_id, mut edits) in buffer_edits {
                buffer_ids.push(buffer_id);
                edits.sort_by_key(|edit| edit.range.start);
                this.buffers[&buffer_id].buffer.update(cx, |buffer, cx| {
                    let mut edits = edits.into_iter().peekable();
                    let mut insertions = Vec::new();
                    let mut original_indent_columns = Vec::new();
                    let mut deletions = Vec::new();
                    let empty_str: Arc<str> = Arc::default();
                    while let Some(BufferEdit { mut range, mut new_text, mut is_insertion, original_indent_column }) =
                        edits.next()
                    {
                        while let Some(BufferEdit {
                            range: next_range,
                            is_insertion: next_is_insertion,
                            new_text: next_new_text,
                            ..
                        }) = edits.peek()
                        {
                            let should_coalesce = if coalesce_adjacent {
                                range.end >= next_range.start
                            } else {
                                range.end > next_range.start
                            };

                            if should_coalesce {
                                range.end = cmp::max(next_range.end, range.end);
                                is_insertion |= *next_is_insertion;
                                new_text = format!("{new_text}{next_new_text}").into();
                                edits.next();
                            } else {
                                break;
                            }
                        }

                        if is_insertion {
                            original_indent_columns.push(original_indent_column);
                            insertions
                                .push((buffer.anchor_before(range.start)..buffer.anchor_before(range.end), new_text));
                        } else if !range.is_empty() {
                            deletions.push((
                                buffer.anchor_before(range.start)..buffer.anchor_before(range.end),
                                Arc::clone(&empty_str),
                            ));
                        }
                    }

                    let deletion_autoindent_mode = if let Some(AutoindentMode::Block { .. }) = autoindent_mode {
                        Some(AutoindentMode::Block { original_indent_columns: Vec::default() })
                    } else {
                        autoindent_mode.clone()
                    };
                    let insertion_autoindent_mode = if let Some(AutoindentMode::Block { .. }) = autoindent_mode {
                        Some(AutoindentMode::Block { original_indent_columns })
                    } else {
                        autoindent_mode.clone()
                    };

                    if coalesce_adjacent {
                        match autoindent_exclusion {
                            AutoIndentExclusion::PrecedingLine => {
                                buffer.edit(deletions, deletion_autoindent_mode, cx);
                                buffer.edit(insertions, insertion_autoindent_mode, cx);
                            }
                            AutoIndentExclusion::FollowingLine => {
                                buffer.edit_before(deletions, deletion_autoindent_mode, cx);
                                buffer.edit_before(insertions, insertion_autoindent_mode, cx);
                            }
                        }
                    } else {
                        buffer.edit_non_coalesce(deletions, deletion_autoindent_mode, cx);
                        buffer.edit_non_coalesce(insertions, insertion_autoindent_mode, cx);
                    }
                });
            }

            cx.emit(Event::BuffersEdited { buffer_ids });
        }
    }

    fn convert_edits_to_buffer_edits(
        edits: Vec<(Range<MultiBufferOffset>, Arc<str>)>,
        snapshot: &MultiBufferSnapshot,
        original_indent_columns: &[Option<u32>],
    ) -> FxHashMap<BufferId, Vec<BufferEdit>> {
        let mut buffer_edits: FxHashMap<BufferId, Vec<BufferEdit>> = FxHashMap::default();
        let mut cursor = snapshot.cursor::<MultiBufferOffset, BufferOffset>();
        for (ix, (range, new_text)) in edits.into_iter().enumerate() {
            let original_indent_column = original_indent_columns.get(ix).copied().flatten();

            cursor.seek(&range.start);
            let mut start_region = cursor.region().expect("start offset out of bounds");
            if !start_region.is_main_buffer {
                cursor.next();
                if let Some(region) = cursor.region() {
                    start_region = region;
                } else {
                    continue;
                }
            }

            if range.end < start_region.range.start {
                continue;
            }

            let start_region = start_region.clone();
            if range.end > start_region.range.end {
                cursor.seek_forward(&range.end);
            }
            let mut end_region = cursor.region().expect("end offset out of bounds");
            if !end_region.is_main_buffer {
                cursor.prev();
                if let Some(region) = cursor.region() {
                    end_region = region;
                } else {
                    continue;
                }
            }

            if range.start > end_region.range.end {
                continue;
            }

            let start_overshoot = range.start.saturating_sub(start_region.range.start);
            let end_overshoot = range.end.saturating_sub(end_region.range.start);
            let buffer_start = (start_region.buffer_range.start + start_overshoot).min(start_region.buffer_range.end);
            let buffer_end = (end_region.buffer_range.start + end_overshoot).min(end_region.buffer_range.end);

            if start_region.excerpt == end_region.excerpt {
                if start_region.buffer.capability == Capability::ReadWrite && start_region.is_main_buffer {
                    buffer_edits.entry(start_region.buffer.buffer_id()).or_default().push(BufferEdit {
                        range: buffer_start..buffer_end,
                        new_text,
                        is_insertion: true,
                        original_indent_column,
                    });
                }
            } else {
                let start_excerpt_range = buffer_start..start_region.buffer_range.end;
                let end_excerpt_range = end_region.buffer_range.start..buffer_end;
                if start_region.buffer.capability == Capability::ReadWrite && start_region.is_main_buffer {
                    buffer_edits.entry(start_region.buffer.buffer_id()).or_default().push(BufferEdit {
                        range: start_excerpt_range,
                        new_text: Arc::clone(&new_text),
                        is_insertion: true,
                        original_indent_column,
                    });
                }
                if end_region.buffer.capability == Capability::ReadWrite && end_region.is_main_buffer {
                    buffer_edits.entry(end_region.buffer.buffer_id()).or_default().push(BufferEdit {
                        range: end_excerpt_range,
                        new_text: Arc::clone(&new_text),
                        is_insertion: false,
                        original_indent_column,
                    });
                }
                let end_region_excerpt = end_region.excerpt.clone();

                cursor.seek(&range.start);
                cursor.next_excerpt();
                while let Some(region) = cursor.region() {
                    if region.excerpt == &end_region_excerpt {
                        break;
                    }
                    if region.buffer.capability == Capability::ReadWrite && region.is_main_buffer {
                        buffer_edits.entry(region.buffer.buffer_id()).or_default().push(BufferEdit {
                            range: region.buffer_range.clone(),
                            new_text: Arc::clone(&new_text),
                            is_insertion: false,
                            original_indent_column,
                        });
                    }
                    cursor.next_excerpt();
                }
            }
        }
        buffer_edits
    }

    pub fn autoindent_ranges<I, S>(&mut self, ranges: I, cx: &mut Context<Self>)
    where
        I: IntoIterator<Item = Range<S>>,
        S: ToOffset,
    {
        if self.read_only() || self.buffers.is_empty() {
            return;
        }
        self.sync_mut(cx);
        let empty = Arc::<str>::from("");
        let edits = ranges
            .into_iter()
            .map(|range| {
                let mut range =
                    range.start.to_offset(self.snapshot.get_mut())..range.end.to_offset(self.snapshot.get_mut());
                if range.start > range.end {
                    mem::swap(&mut range.start, &mut range.end);
                }
                (range, Arc::clone(&empty))
            })
            .collect::<Vec<_>>();

        return autoindent_ranges_internal(self, edits, cx);

        fn autoindent_ranges_internal(
            this: &mut MultiBuffer,
            edits: Vec<(Range<MultiBufferOffset>, Arc<str>)>,
            cx: &mut Context<MultiBuffer>,
        ) {
            let buffer_edits = MultiBuffer::convert_edits_to_buffer_edits(edits, this.snapshot.get_mut(), &[]);

            let mut buffer_ids = Vec::new();
            for (buffer_id, mut edits) in buffer_edits {
                buffer_ids.push(buffer_id);
                edits.sort_unstable_by_key(|edit| edit.range.start);

                let mut ranges: Vec<Range<BufferOffset>> = Vec::new();
                for edit in edits {
                    if let Some(last_range) = ranges.last_mut()
                        && edit.range.start <= last_range.end
                    {
                        last_range.end = last_range.end.max(edit.range.end);
                        continue;
                    }
                    ranges.push(edit.range);
                }

                this.buffers[&buffer_id].buffer.update(cx, |buffer, cx| {
                    buffer.autoindent_ranges(ranges, cx);
                });
            }

            cx.emit(Event::BuffersEdited { buffer_ids });
        }
    }

    fn merge_excerpt_ranges<'a>(
        expanded_ranges: impl IntoIterator<Item = &'a ExcerptRange<Point>> + 'a,
    ) -> Vec<ExcerptRange<Point>> {
        let mut sorted: Vec<_> = expanded_ranges.into_iter().collect();
        sorted.sort_by_key(|range| range.context.start);
        let mut merged_ranges: Vec<ExcerptRange<Point>> = Vec::new();
        for range in sorted {
            if let Some(last_range) = merged_ranges.last_mut()
                && (last_range.context.end >= range.context.start
                    || last_range.context.end.row + 1 == range.context.start.row)
            {
                last_range.context.end = range.context.end.max(last_range.context.end);
                continue;
            }
            merged_ranges.push(range.clone());
        }
        merged_ranges
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.sync_mut(cx);
        let removed_buffer_ids = mem::take(&mut self.buffers).into_keys().collect();
        self.diffs.clear();
        let MultiBufferSnapshot {
            excerpts,
            diffs,
            diff_transforms: _,
            non_text_state_update_count: _,
            edit_count: _,
            has_inverted_diff,
            singleton: _,
            trailing_excerpt_update_count,
            all_diff_hunks_expanded: _,
            show_deleted_hunks: _,
            use_extended_diff_range: _,
            show_headers: _,
            path_keys: _,
            buffers,
        } = self.snapshot.get_mut();
        let start = ExcerptDimension(MultiBufferOffset::ZERO);
        let prev_len = ExcerptDimension(excerpts.summary().text.len);
        *excerpts = SumTree::default();
        *buffers = BufferStateSnapshots::default();
        *diffs = SumTree::default();
        *trailing_excerpt_update_count += 1;
        *has_inverted_diff = false;

        let edits = Self::sync_diff_transforms(
            self.snapshot.get_mut(),
            vec![Edit { old: start..prev_len, new: start..start }],
            DiffChangeKind::BufferEdited,
        );
        if !edits.is_empty() {
            self.subscriptions.publish(edits);
        }
        cx.emit(Event::Edited { edited_buffer: None });
        cx.emit(Event::BuffersRemoved { removed_buffer_ids });
        cx.notify();
    }

    // If point is at the end of the buffer, the last excerpt is returned
    pub fn point_to_buffer_offset<T: ToOffset>(&self, point: T, cx: &App) -> Option<(Entity<Buffer>, BufferOffset)> {
        let snapshot = self.read(cx);
        let (buffer, offset) = snapshot.point_to_buffer_offset(point)?;
        Some((self.buffers.get(&buffer.buffer_id())?.buffer.clone(), offset))
    }

    // If point is at the end of the buffer, the last excerpt is returned
    pub fn point_to_buffer_point<T: ToPoint>(&self, point: T, cx: &App) -> Option<(Entity<Buffer>, Point)> {
        let snapshot = self.read(cx);
        let (buffer, point) = snapshot.point_to_buffer_point(point.to_point(&snapshot))?;
        Some((self.buffers.get(&buffer.buffer_id())?.buffer.clone(), point))
    }

    pub fn buffer_point_to_anchor(
        &self,
        // todo(lw): We shouldn't need this?
        buffer: &Entity<Buffer>,
        point: Point,
        cx: &App,
    ) -> Option<Anchor> {
        let mut found = None;
        let buffer_snapshot = buffer.read(cx).snapshot();
        let text_anchor = buffer_snapshot.anchor_after(point);
        let snapshot = self.snapshot(cx);
        let path_key_index = snapshot.path_key_index_for_buffer(buffer_snapshot.buffer_id())?;
        for excerpt in snapshot.excerpts_for_buffer(buffer_snapshot.buffer_id()) {
            if excerpt.context.start.cmp(&text_anchor, &buffer_snapshot).is_gt() {
                found = Some(Anchor::in_buffer(path_key_index, excerpt.context.start));
                break;
            } else if excerpt.context.end.cmp(&text_anchor, &buffer_snapshot).is_ge() {
                found = Some(Anchor::in_buffer(path_key_index, text_anchor));
                break;
            }
            found = Some(Anchor::in_buffer(path_key_index, excerpt.context.end));
        }

        found
    }

    pub fn text_anchor_for_position<T: ToOffset>(
        &self,
        position: T,
        cx: &App,
    ) -> Option<(Entity<Buffer>, text::Anchor)> {
        let snapshot = self.read(cx);
        let anchor = snapshot.anchor_before(position).excerpt_anchor()?;
        let buffer = self.buffers.get(&anchor.text_anchor.buffer_id)?.buffer.clone();
        Some((buffer, anchor.text_anchor()))
    }

    fn on_buffer_event(&mut self, buffer: Entity<Buffer>, event: &language::BufferEvent, cx: &mut Context<Self>) {
        use language::BufferEvent;
        let buffer_id = buffer.read(cx).buffer_id();
        cx.emit(match event {
            BufferEvent::Edited => Event::Edited { edited_buffer: Some(buffer) },
            BufferEvent::DirtyChanged => Event::DirtyChanged,
            BufferEvent::Saved => Event::Saved,
            BufferEvent::FileHandleChanged => Event::FileHandleChanged,
            BufferEvent::Reloaded => Event::Reloaded,
            BufferEvent::LanguageChanged(has_language) => Event::LanguageChanged(buffer_id, *has_language),
            BufferEvent::SettingsChanged => Event::SettingsChanged,
            BufferEvent::Reparsed => Event::Reparsed(buffer_id),
            BufferEvent::DiagnosticsUpdated => Event::DiagnosticsUpdated,
            BufferEvent::CapabilityChanged => {
                if self.singleton {
                    self.capability = buffer.read(cx).capability();
                }
                Event::CapabilityChanged
            }
            BufferEvent::LineEndingChanged | BufferEvent::ReloadNeeded => return,
        });
    }

    fn buffer_diff_changed(
        &mut self,
        diff: Entity<BufferDiff>,
        range: Option<Range<text::Anchor>>,
        cx: &mut Context<Self>,
    ) {
        let Some(buffer) = self.buffer(diff.read(cx).buffer_id) else {
            return;
        };
        let snapshot = self.sync_mut(cx);

        let diff = diff.read(cx);
        let buffer_id = diff.buffer_id;

        let Some(path) = snapshot.path_for_buffer(buffer_id).cloned() else {
            return;
        };
        let new_diff = DiffStateSnapshot { buffer_id, diff: diff.snapshot(cx), main_buffer: None };
        let snapshot = self.snapshot.get_mut();
        let base_text_changed = find_diff_state(&snapshot.diffs, buffer_id)
            .is_none_or(|old_diff| !new_diff.base_texts_definitely_eq(old_diff));
        snapshot.diffs.insert_or_replace(new_diff, ());

        let buffer = buffer.read(cx);
        let Some(range) = range else {
            return;
        };
        let diff_change_range = range.to_offset(buffer);

        let excerpt_edits = snapshot.excerpt_edits_for_diff_change(&path, diff_change_range);
        let edits = Self::sync_diff_transforms(
            snapshot,
            excerpt_edits,
            DiffChangeKind::DiffUpdated { base_changed: base_text_changed },
        );
        if !edits.is_empty() {
            self.subscriptions.publish(edits);
        }
        cx.emit(Event::Edited { edited_buffer: None });
    }

    fn inverted_buffer_diff_changed(
        &mut self,
        diff: Entity<BufferDiff>,
        main_buffer: Entity<Buffer>,
        diff_change_range: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.sync_mut(cx);

        let base_text_buffer_id = diff.read(cx).base_text_buffer().read(cx).buffer_id();
        let Some(path) = snapshot.path_for_buffer(base_text_buffer_id).cloned() else {
            return;
        };

        let main_buffer_snapshot = main_buffer.read(cx).snapshot();
        let diff = diff.read(cx);
        let new_diff = DiffStateSnapshot {
            buffer_id: base_text_buffer_id,
            diff: diff.snapshot(cx),
            main_buffer: Some(main_buffer_snapshot),
        };
        let snapshot = self.snapshot.get_mut();
        snapshot.diffs.insert_or_replace(new_diff, ());

        let Some(diff_change_range) = diff_change_range else {
            return;
        };

        let excerpt_edits = snapshot.excerpt_edits_for_diff_change(&path, diff_change_range);
        let edits = Self::sync_diff_transforms(
            snapshot,
            excerpt_edits,
            DiffChangeKind::DiffUpdated {
                // We don't read this field for inverted diffs.
                base_changed: false,
            },
        );
        if !edits.is_empty() {
            self.subscriptions.publish(edits);
        }
        cx.emit(Event::Edited { edited_buffer: None });
    }

    pub fn all_buffers_iter(&self) -> impl Iterator<Item = Entity<Buffer>> {
        self.buffers.values().map(|state| state.buffer.clone())
    }

    pub fn all_buffers(&self) -> FxHashSet<Entity<Buffer>> {
        self.all_buffers_iter().collect()
    }

    pub fn buffer(&self, buffer_id: BufferId) -> Option<Entity<Buffer>> {
        self.buffers.get(&buffer_id).map(|state| state.buffer.clone())
    }

    pub fn language_at<T: ToOffset>(&self, point: T, cx: &App) -> Option<Arc<Language>> {
        let (buffer, offset) = self.point_to_buffer_offset(point, cx)?;
        buffer.read(cx).language_at(offset)
    }

    pub fn language_settings(&self, cx: &App) -> Arc<LanguageSettings> {
        let first_buffer_id = self.read(cx).excerpts.first().map(|excerpt| excerpt.range.context.start.buffer_id);
        first_buffer_id.and_then(|buffer_id| self.buffer(buffer_id)).map_or_else(
            move || self.language_settings_at(MultiBufferOffset::default(), cx),
            |buffer| LanguageSettings::for_buffer(buffer.read(cx), cx),
        )
    }

    pub fn language_settings_at<T: ToOffset>(&self, point: T, cx: &App) -> Arc<LanguageSettings> {
        if let Some((buffer, offset)) = self.point_to_buffer_offset(point, cx) {
            LanguageSettings::for_buffer_at(buffer.read(cx), offset, cx)
        } else {
            Arc::clone(&AllLanguageSettings::get_global(cx).fallback)
        }
    }

    pub fn for_each_buffer(&self, f: &mut dyn FnMut(&Entity<Buffer>)) {
        self.buffers.values().for_each(|state| f(&state.buffer));
    }

    /// The title used for buffers not backed by a file and with no title of their own.
    pub const DEFAULT_TITLE: &str = "untitled";

    pub fn title<'a>(&'a self, cx: &'a App) -> Cow<'a, str> {
        if let Some(title) = self.title.as_ref() {
            return title.into();
        }

        if let Some(buffer) = self.as_singleton() {
            let buffer = buffer.read(cx);

            if let Some(file) = buffer.file() {
                return file.file_name(cx).into();
            }

            if let Some(title) = self.buffer_content_title(buffer) {
                return title;
            }
        }

        Self::DEFAULT_TITLE.into()
    }

    fn buffer_content_title(&self, buffer: &Buffer) -> Option<Cow<'_, str>> {
        let mut is_leading_whitespace = true;
        let mut count = 0;
        let mut prev_was_space = false;
        let mut title = String::new();

        for ch in buffer.snapshot().chars() {
            if is_leading_whitespace && ch.is_whitespace() {
                continue;
            }

            is_leading_whitespace = false;

            if ch == '\n' || count >= 40 {
                break;
            }

            if ch.is_whitespace() {
                if !prev_was_space {
                    title.push(' ');
                    count += 1;
                    prev_was_space = true;
                }
            } else {
                title.push(ch);
                count += 1;
                prev_was_space = false;
            }
        }

        let title = title.trim_end().to_owned();

        if title.is_empty() {
            return None;
        }

        Some(title.into())
    }

    pub fn set_title(&mut self, title: String, cx: &mut Context<Self>) {
        self.title = Some(title);
        cx.notify();
    }

    /// Whether we should preserve the preview status of a tab containing this multi-buffer.
    pub fn preserve_preview(&self, cx: &App) -> bool {
        self.buffers.values().all(|state| state.buffer.read(cx).preserve_preview())
    }

    pub fn add_diff(&mut self, diff: Entity<BufferDiff>, cx: &mut Context<Self>) {
        let buffer_id = diff.read(cx).buffer_id;

        if let Some(existing_diff) = self.diff_for(buffer_id)
            && diff.entity_id() == existing_diff.entity_id()
        {
            return;
        }

        self.buffer_diff_changed(diff.clone(), Some(text::Anchor::min_max_range_for_buffer(buffer_id)), cx);
        self.diffs.insert(buffer_id, DiffState::new(diff, cx));
    }

    pub fn add_inverted_diff(&mut self, diff: Entity<BufferDiff>, main_buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        let snapshot = diff.read(cx).base_text(cx);
        let base_text_buffer_id = snapshot.buffer_id();
        let diff_change_range = 0..snapshot.len();
        self.snapshot.get_mut().has_inverted_diff = true;
        self.inverted_buffer_diff_changed(diff.clone(), main_buffer.clone(), Some(diff_change_range), cx);
        self.diffs.insert(base_text_buffer_id, DiffState::new_inverted(diff, main_buffer, cx));
    }

    pub fn diff_for(&self, buffer_id: BufferId) -> Option<Entity<BufferDiff>> {
        self.diffs.get(&buffer_id).map(|state| state.diff.clone())
    }

    pub fn expand_diff_hunks(&mut self, ranges: Vec<Range<Anchor>>, cx: &mut Context<Self>) {
        self.expand_or_collapse_diff_hunks(ranges, true, cx);
    }

    pub fn collapse_diff_hunks(&mut self, ranges: Vec<Range<Anchor>>, cx: &mut Context<Self>) {
        self.expand_or_collapse_diff_hunks(ranges, false, cx);
    }

    pub fn set_all_diff_hunks_expanded(&mut self, cx: &mut Context<Self>) {
        self.snapshot.get_mut().all_diff_hunks_expanded = true;
        self.expand_or_collapse_diff_hunks(vec![Anchor::Min..Anchor::Max], true, cx);
    }

    pub fn all_diff_hunks_expanded(&self) -> bool {
        self.snapshot.borrow().all_diff_hunks_expanded
    }

    pub fn set_all_diff_hunks_collapsed(&mut self, cx: &mut Context<Self>) {
        self.snapshot.get_mut().all_diff_hunks_expanded = false;
        self.expand_or_collapse_diff_hunks(vec![Anchor::Min..Anchor::Max], false, cx);
    }

    pub fn set_show_deleted_hunks(&mut self, show: bool, cx: &mut Context<Self>) {
        self.snapshot.get_mut().show_deleted_hunks = show;

        self.sync_mut(cx);

        let old_len = self.snapshot.borrow().len();

        let ranges = iter::once((Point::zero()..Point::MAX, None));
        self.expand_or_collapse_diff_hunks_inner(ranges, true, cx);

        let new_len = self.snapshot.borrow().len();

        self.subscriptions
            .publish(vec![Edit { old: MultiBufferOffset(0)..old_len, new: MultiBufferOffset(0)..new_len }]);

        cx.emit(Event::DiffHunksToggled);
        cx.emit(Event::Edited { edited_buffer: None });
    }

    pub fn set_use_extended_diff_range(&mut self, use_extended: bool, _cx: &mut Context<Self>) {
        self.snapshot.get_mut().use_extended_diff_range = use_extended;
    }

    pub fn single_hunk_is_expanded(&self, range: Range<Anchor>, cx: &App) -> bool {
        let snapshot = self.read(cx);
        let mut cursor = snapshot.diff_transforms.cursor::<MultiBufferOffset>(());
        let offset_range = range.to_offset(&snapshot);
        cursor.seek(&offset_range.start, Bias::Left);
        while let Some(item) = cursor.item() {
            if *cursor.start() >= offset_range.end && *cursor.start() > offset_range.start {
                break;
            }
            if item.hunk_info().is_some() {
                return true;
            }
            cursor.next();
        }
        false
    }

    pub fn has_expanded_diff_hunks_in_ranges(&self, ranges: &[Range<Anchor>], cx: &App) -> bool {
        let snapshot = self.read(cx);
        let mut cursor = snapshot.diff_transforms.cursor::<MultiBufferOffset>(());
        for range in ranges {
            let range = range.to_point(&snapshot);
            let start = snapshot.point_to_offset(Point::new(range.start.row, 0));
            let end = (snapshot.point_to_offset(Point::new(range.end.row + 1, 0)) + 1_usize).min(snapshot.len());
            cursor.seek(&start, Bias::Right);
            while let Some(item) = cursor.item() {
                if *cursor.start() >= end {
                    break;
                }
                if item.hunk_info().is_some() {
                    return true;
                }
                cursor.next();
            }
        }
        false
    }

    pub fn expand_or_collapse_diff_hunks_inner(
        &mut self,
        ranges: impl IntoIterator<Item = (Range<Point>, Option<Anchor>)>,
        expand: bool,
        cx: &mut Context<Self>,
    ) -> Vec<Edit<MultiBufferOffset>> {
        if self.snapshot.borrow().all_diff_hunks_expanded && !expand {
            return Vec::new();
        }
        self.sync_mut(cx);
        let snapshot = self.snapshot.get_mut();
        let mut excerpt_edits = Vec::new();
        let mut last_hunk_row = None;
        for (range, end_anchor) in ranges {
            for diff_hunk in snapshot.diff_hunks_in_range(range) {
                if let Some(end_anchor) = &end_anchor
                    && let Some(hunk_end_anchor) = snapshot.anchor_in_excerpt(diff_hunk.excerpt_range.context.end)
                    && hunk_end_anchor.cmp(end_anchor, snapshot).is_gt()
                {
                    continue;
                }
                let hunk_range = diff_hunk.multi_buffer_range;
                if let Some(excerpt_start_anchor) = snapshot.anchor_in_excerpt(diff_hunk.excerpt_range.context.start)
                    && hunk_range.start.to_point(snapshot) < excerpt_start_anchor.to_point(snapshot)
                {
                    continue;
                }
                if last_hunk_row.is_some_and(|row| row >= diff_hunk.row_range.start) {
                    continue;
                }
                let mut start = snapshot.excerpt_offset_for_anchor(&hunk_range.start);
                let mut end = snapshot.excerpt_offset_for_anchor(&hunk_range.end);
                if let Some(excerpt_end_anchor) = snapshot.anchor_in_excerpt(diff_hunk.excerpt_range.context.end) {
                    let excerpt_end = snapshot.excerpt_offset_for_anchor(&excerpt_end_anchor);
                    start = start.min(excerpt_end);
                    end = end.min(excerpt_end);
                }
                last_hunk_row = Some(diff_hunk.row_range.start);
                excerpt_edits.push(Edit { old: start..end, new: start..end });
            }
        }

        Self::sync_diff_transforms(snapshot, excerpt_edits, DiffChangeKind::ExpandOrCollapseHunks { expand })
    }

    pub fn expand_or_collapse_diff_hunks(&mut self, ranges: Vec<Range<Anchor>>, expand: bool, cx: &mut Context<Self>) {
        let snapshot = self.snapshot.borrow().clone();
        let ranges = ranges.iter().map(move |range| {
            let excerpt_end = snapshot
                .excerpt_containing(range.end..range.end)
                .and_then(|(_, excerpt_range)| snapshot.anchor_in_excerpt(excerpt_range.context.end));
            let range = range.to_point(&snapshot);
            let peek_end =
                if range.end.row < snapshot.max_row().0 { Point::new(range.end.row + 1, 0) } else { range.end };
            (range.start..peek_end, excerpt_end)
        });
        let edits = self.expand_or_collapse_diff_hunks_inner(ranges, expand, cx);
        if !edits.is_empty() {
            self.subscriptions.publish(edits);
        }
        cx.emit(Event::DiffHunksToggled);
        cx.emit(Event::Edited { edited_buffer: None });
    }

    pub fn toggle_single_diff_hunk(&mut self, range: Range<Anchor>, cx: &mut Context<Self>) {
        let snapshot = self.snapshot(cx);
        let excerpt_end = snapshot
            .excerpt_containing(range.end..range.end)
            .and_then(|(_, excerpt_range)| snapshot.anchor_in_excerpt(excerpt_range.context.end));
        let point_range = range.to_point(&snapshot);
        let expand = !self.single_hunk_is_expanded(range, cx);
        let edits = self.expand_or_collapse_diff_hunks_inner([(point_range, excerpt_end)], expand, cx);
        if !edits.is_empty() {
            self.subscriptions.publish(edits);
        }
        cx.emit(Event::DiffHunksToggled);
        cx.emit(Event::Edited { edited_buffer: None });
    }
}

pub fn build_excerpt_ranges(
    ranges: impl IntoIterator<Item = Range<Point>>,
    context_line_count: u32,
    buffer_snapshot: &BufferSnapshot,
) -> Vec<ExcerptRange<Point>> {
    ranges
        .into_iter()
        .map(|range| {
            let start_row = range.start.row.saturating_sub(context_line_count);
            let start = Point::new(start_row, 0);
            let end_row = (range.end.row + context_line_count).min(buffer_snapshot.max_point().row);
            let end = Point::new(end_row, buffer_snapshot.line_len(end_row));
            ExcerptRange { context: start..end, primary: range }
        })
        .collect()
}

impl EventEmitter<Event> for MultiBuffer {}

impl MultiBufferSnapshot {
    pub fn text(&self) -> String {
        self.chunks(
            MultiBufferOffset::ZERO..self.len(),
            LanguageAwareStyling { tree_sitter: false, diagnostics: false },
        )
        .map(|chunk| chunk.text)
        .collect()
    }

    pub fn reversed_chars_at<T: ToOffset>(&self, position: T) -> impl Iterator<Item = char> + '_ {
        self.reversed_chunks_in_range(MultiBufferOffset::ZERO..position.to_offset(self)).flat_map(|c| c.chars().rev())
    }

    fn reversed_chunks_in_range(&self, range: Range<MultiBufferOffset>) -> ReversedMultiBufferChunks<'_> {
        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        cursor.seek(&range.end);
        let current_chunks = cursor.region().as_ref().map(|region| {
            let start_overshoot = range.start.saturating_sub(region.range.start);
            let end_overshoot = range.end - region.range.start;
            let end = (region.buffer_range.start + end_overshoot).min(region.buffer_range.end);
            let start = region.buffer_range.start + start_overshoot;
            region.buffer.reversed_chunks_in_range(start..end)
        });
        ReversedMultiBufferChunks { cursor, current_chunks, start: range.start, offset: range.end }
    }

    pub fn chars_at<T: ToOffset>(&self, position: T) -> impl Iterator<Item = char> + '_ {
        let offset = position.to_offset(self);
        self.text_for_range(offset..self.len()).flat_map(|chunk| chunk.chars())
    }

    pub fn text_for_range<T: ToOffset>(&self, range: Range<T>) -> impl Iterator<Item = &str> + '_ {
        self.chunks(range, LanguageAwareStyling { tree_sitter: false, diagnostics: false }).map(|chunk| chunk.text)
    }

    pub fn is_line_blank(&self, row: MultiBufferRow) -> bool {
        self.text_for_range(Point::new(row.0, 0)..Point::new(row.0, self.line_len(row)))
            .all(|chunk| chunk.matches(|c: char| !c.is_whitespace()).next().is_none())
    }

    pub fn contains_str_at<T>(&self, position: T, needle: &str) -> bool
    where
        T: ToOffset,
    {
        let position = position.to_offset(self);
        position == self.clip_offset(position, Bias::Left)
            && self.bytes_in_range(position..self.len()).flatten().copied().take(needle.len()).eq(needle.bytes())
    }

    pub fn diff_hunks(&self) -> impl Iterator<Item = MultiBufferDiffHunk> + '_ {
        self.diff_hunks_in_range(Anchor::Min..Anchor::Max)
    }

    pub fn diff_hunks_in_range<T: ToPoint>(&self, range: Range<T>) -> impl Iterator<Item = MultiBufferDiffHunk> + '_ {
        let query_range = range.start.to_point(self)..range.end.to_point(self);
        self.lift_buffer_metadata(query_range.clone(), move |buffer, buffer_range| {
            let diff = self.diff_state(buffer.buffer_id())?;
            let iter = if let Some(main_buffer) = &diff.main_buffer {
                let buffer_start = buffer.point_to_offset(buffer_range.start);
                let buffer_end = buffer.point_to_offset(buffer_range.end);
                itertools::Either::Left(
                    diff.hunks_intersecting_base_text_range(buffer_start..buffer_end, main_buffer)
                        .map(move |hunk| (hunk, buffer, true)),
                )
            } else {
                let buffer_start = buffer.anchor_before(buffer_range.start);
                let buffer_end = buffer.anchor_after(buffer_range.end);
                itertools::Either::Right(
                    diff.hunks_intersecting_range(buffer_start..buffer_end, buffer)
                        .map(move |hunk| (hunk, buffer, false)),
                )
            };
            Some(iter.filter_map(|(hunk, buffer, is_inverted)| {
                if hunk.is_created_file() && !self.all_diff_hunks_expanded {
                    return None;
                }
                let range = if is_inverted { hunk.diff_base_byte_range.to_point(buffer) } else { hunk.range.clone() };
                Some((range, (hunk, is_inverted)))
            }))
        })
        .filter_map(move |(range, (hunk, is_inverted), excerpt)| {
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            if range.start != range.end && range.end == query_range.start && !hunk.range.is_empty() {
                return None;
            }
            let end_row = if range.end.column == 0 { range.end.row } else { range.end.row + 1 };

            let word_diffs = if !hunk.base_word_diffs.is_empty() || !hunk.buffer_word_diffs.is_empty() {
                {
                    let mut word_diffs = Vec::new();

                    let hunk_start_offset = if is_inverted {
                        Anchor::in_buffer(
                            excerpt.path_key_index,
                            buffer_snapshot.anchor_after(hunk.diff_base_byte_range.start),
                        )
                        .to_offset(self)
                    } else {
                        Anchor::in_buffer(excerpt.path_key_index, hunk.buffer_range.start).to_offset(self)
                    };

                    // `show_deleted_hunks` only says deleted text *can* be shown; base-side offsets are valid
                    // only when this hunk is actually expanded, otherwise they'd land in the modified text.
                    let hunk_has_deleted_text = !is_inverted && {
                        let mut cursor = self.diff_transforms.cursor::<MultiBufferOffset>(());
                        cursor.seek(&hunk_start_offset, Bias::Right);
                        matches!(
                            cursor.item(),
                            Some(DiffTransform::DeletedHunk { buffer_id, hunk_info, .. })
                                if *buffer_id == buffer_snapshot.buffer_id()
                                    && hunk_info.hunk_start_anchor == hunk.buffer_range.start
                        )
                    };

                    if hunk_has_deleted_text || is_inverted {
                        word_diffs.extend(
                            hunk.base_word_diffs
                                .iter()
                                .map(|diff| hunk_start_offset + diff.start..hunk_start_offset + diff.end),
                        );
                    }

                    if !is_inverted {
                        word_diffs.extend(
                            hunk.buffer_word_diffs
                                .into_iter()
                                .map(|diff| Anchor::range_in_buffer(excerpt.path_key_index, diff).to_offset(self)),
                        );
                    }
                    word_diffs
                }
            } else {
                Vec::default()
            };

            let buffer_range = if is_inverted {
                buffer_snapshot.anchor_after(hunk.diff_base_byte_range.start)
                    ..buffer_snapshot.anchor_before(hunk.diff_base_byte_range.end)
            } else {
                hunk.buffer_range.clone()
            };
            let status_kind = if hunk.buffer_range.start == hunk.buffer_range.end {
                DiffHunkStatusKind::Deleted
            } else if hunk.diff_base_byte_range.is_empty() {
                DiffHunkStatusKind::Added
            } else {
                DiffHunkStatusKind::Modified
            };
            let multi_buffer_range = Anchor::range_in_buffer(excerpt.path_key_index, buffer_range.clone());
            Some(MultiBufferDiffHunk {
                row_range: MultiBufferRow(range.start.row)..MultiBufferRow(end_row),
                buffer_id: buffer_snapshot.buffer_id(),
                buffer_range,
                word_diffs,
                diff_base_byte_range: BufferOffset(hunk.diff_base_byte_range.start)
                    ..BufferOffset(hunk.diff_base_byte_range.end),
                status: DiffHunkStatus { kind: status_kind, secondary: hunk.secondary_status },
                excerpt_range: excerpt.range.clone(),
                multi_buffer_range,
            })
        })
    }

    fn excerpts_for_range<T: ToOffset>(&self, range: Range<T>) -> impl Iterator<Item = &Excerpt> + '_ {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        cursor.seek(&range.start);
        iter::from_fn(move || {
            let region = cursor.region()?;
            if region.range.start > range.end || region.range.start == range.end && region.range.start > range.start {
                return None;
            }
            let excerpt = region.excerpt;
            cursor.next_excerpt_forwards();
            Some(excerpt)
        })
    }

    pub fn buffer_ids_for_range<T: ToOffset>(&self, range: Range<T>) -> impl Iterator<Item = BufferId> + '_ {
        self.excerpts_for_range(range).map(|excerpt| excerpt.buffer_snapshot(self).buffer_id())
    }

    /// Resolves the given [`text::Anchor`]s to [`crate::Anchor`]s if the anchor is within a visible excerpt.
    ///
    /// The passed in anchors must be ordered.
    pub fn text_anchors_to_visible_anchors(
        &self,
        anchors: impl IntoIterator<Item = text::Anchor>,
    ) -> Vec<Option<Anchor>> {
        let anchors = anchors.into_iter();
        let mut result = Vec::with_capacity(anchors.size_hint().0);
        let mut anchors = anchors.peekable();
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        'anchors: while let Some(anchor) = anchors.peek() {
            let buffer_id = anchor.buffer_id;
            let mut same_buffer_anchors = anchors.peeking_take_while(|a| a.buffer_id == buffer_id);

            if let Some(buffer) = self.buffers.get(&buffer_id) {
                let path = &buffer.path_key;
                let Some(mut next) = same_buffer_anchors.next() else {
                    continue 'anchors;
                };
                cursor.seek_forward(path, Bias::Left);
                'excerpts: while let Some(excerpt) = cursor.item() {
                    if excerpt.path_key != *path {
                        break;
                    }
                    let buffer_snapshot = excerpt.buffer_snapshot(self);

                    loop {
                        // anchor is before the first excerpt
                        if excerpt.range.context.start.cmp(&next, buffer_snapshot).is_gt() {
                            // so we skip it and try the next anchor
                            result.push(None);
                            match same_buffer_anchors.next() {
                                Some(anchor) => next = anchor,
                                None => continue 'anchors,
                            }
                        // anchor is within the excerpt
                        } else if excerpt.range.context.end.cmp(&next, buffer_snapshot).is_ge() {
                            // record it and all following anchors that are within
                            result.push(Some(Anchor::in_buffer(excerpt.path_key_index, next)));
                            result.extend(
                                same_buffer_anchors
                                    .peeking_take_while(|a| excerpt.range.context.end.cmp(a, buffer_snapshot).is_ge())
                                    .map(|a| Some(Anchor::in_buffer(excerpt.path_key_index, a))),
                            );
                            match same_buffer_anchors.next() {
                                Some(anchor) => next = anchor,
                                None => continue 'anchors,
                            }
                        // anchor is after the excerpt, try the next one
                        } else {
                            cursor.next();
                            continue 'excerpts;
                        }
                    }
                }
                // account for `next`
                result.push(None);
            }
            result.extend(same_buffer_anchors.map(|_| None));
        }

        result
    }

    /// Callers should not provide a range where `end < start`.
    pub fn range_to_buffer_ranges<T: ToOffset>(
        &self,
        range: Range<T>,
    ) -> Vec<(&BufferSnapshot, Range<BufferOffset>, ExcerptRange<text::Anchor>)> {
        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        let start = range.start.to_offset(self);
        let end = range.end.to_offset(self);
        let range_non_empty = end > start;
        cursor.seek(&start);

        let mut result: Vec<(&BufferSnapshot, Range<BufferOffset>, ExcerptRange<text::Anchor>)> = Vec::new();
        while let Some(region) = cursor.region() {
            if region.range.start > end || (region.range.start == end && range_non_empty) {
                break;
            }
            if region.is_main_buffer {
                let start_overshoot = start.saturating_sub(region.range.start);
                let end_offset = end;
                let end_overshoot = end_offset.saturating_sub(region.range.start);
                let start = region.buffer_range.end.min(region.buffer_range.start + start_overshoot);
                let end = region.buffer_range.end.min(region.buffer_range.start + end_overshoot);
                let excerpt_range = region.excerpt.range.clone();
                if let Some(prev) = result.last_mut().filter(|(prev_buffer, prev_range, prev_excerpt)| {
                    prev_buffer.buffer_id() == region.buffer.buffer_id()
                        && prev_range.end == start
                        && prev_excerpt.context.start == excerpt_range.context.start
                }) {
                    prev.1.end = end;
                } else {
                    result.push((region.buffer, start..end, excerpt_range));
                }
            }
            cursor.next();
        }

        if let Some(excerpt) = cursor.excerpt()
            && excerpt.text_summary.len == 0
            && end == self.len()
        {
            let buffer_snapshot = excerpt.buffer_snapshot(self);

            let buffer_offset = BufferOffset(excerpt.range.context.start.to_offset(buffer_snapshot));
            let excerpt_range = excerpt.range.clone();
            if result.last_mut().is_none_or(|(prev_buffer, prev_range, prev_excerpt)| {
                prev_buffer.buffer_id() != buffer_snapshot.buffer_id()
                    || prev_range.end != buffer_offset
                    || prev_excerpt.context.start != excerpt_range.context.start
            }) {
                result.push((buffer_snapshot, buffer_offset..buffer_offset, excerpt_range));
            }
        }

        result
    }

    pub fn range_to_buffer_ranges_with_deleted_hunks<T: ToOffset>(
        &self,
        range: Range<T>,
    ) -> impl Iterator<Item = (&BufferSnapshot, Range<BufferOffset>, Option<Anchor>)> + '_ {
        let start = range.start.to_offset(self);
        let end = range.end.to_offset(self);

        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        cursor.seek(&start);

        iter::from_fn(move || {
            let Some(region) = cursor
                .region()
                .filter(|region| region.range.start < end || (region.range.start == end && start == end))
            else {
                let excerpt = cursor.excerpt()?;
                if excerpt.text_summary.len != 0 || end != self.len() {
                    return None;
                }
                let buffer = excerpt.buffer_snapshot(self);
                let offset = BufferOffset(excerpt.range.context.start.to_offset(buffer));
                cursor.next_excerpt();
                return Some((buffer, offset..offset, None));
            };
            let start_overshoot = start.saturating_sub(region.range.start);
            let end_overshoot = end.saturating_sub(region.range.start);
            let start = region.buffer_range.end.min(region.buffer_range.start + start_overshoot);
            let end = region.buffer_range.end.min(region.buffer_range.start + end_overshoot);

            let deleted_hunk_anchor =
                if region.is_main_buffer { None } else { Some(self.anchor_before(region.range.start)) };
            let result = (region.buffer, start..end, deleted_hunk_anchor);
            cursor.next();
            Some(result)
        })
    }

    /// Retrieves buffer metadata for the given range, and converts it into multi-buffer
    /// coordinates.
    ///
    /// The given callback will be called for every excerpt intersecting the given range. It will
    /// be passed the excerpt's buffer and the buffer range that the input range intersects.
    /// The callback should return an iterator of metadata items from that buffer, each paired
    /// with a buffer range.
    ///
    /// The returned iterator yields each of these metadata items, paired with its range in
    /// multi-buffer coordinates.
    fn lift_buffer_metadata<'a, MBD, M, I>(
        &'a self,
        query_range: Range<MBD>,
        get_buffer_metadata: impl 'a + Fn(&'a BufferSnapshot, Range<MBD::TextDimension>) -> Option<I>,
    ) -> impl Iterator<Item = (Range<MBD>, M, &'a Excerpt)> + 'a
    where
        I: Iterator<Item = (Range<MBD::TextDimension>, M)> + 'a,
        MBD: MultiBufferDimension
            + Ord
            + Sub<Output = MBD::TextDimension>
            + ops::Add<MBD::TextDimension, Output = MBD>
            + AddAssign<MBD::TextDimension>,
        MBD::TextDimension: Sub<Output = MBD::TextDimension>
            + ops::Add<Output = MBD::TextDimension>
            + AddAssign<MBD::TextDimension>
            + Ord,
    {
        let mut current_excerpt_metadata: Option<(ExcerptRange<text::Anchor>, I)> = None;
        let mut cursor = self.cursor::<MBD, MBD::TextDimension>();

        // Find the excerpt and buffer offset where the given range ends.
        cursor.seek(&query_range.end);
        let mut range_end = None;
        while let Some(region) = cursor.region() {
            if region.is_main_buffer {
                let mut buffer_end = region.buffer_range.start;
                let overshoot = if query_range.end > region.range.start {
                    query_range.end - region.range.start
                } else {
                    <MBD::TextDimension>::default()
                };
                buffer_end += overshoot;
                range_end = Some((region.excerpt.range.clone(), buffer_end));
                break;
            }
            cursor.next();
        }

        cursor.seek(&query_range.start);

        if let Some(region) = cursor.region().filter(|region| !region.is_main_buffer)
            && region.range.start > MBD::default()
        {
            cursor.prev();
        } else if let Some(region) = cursor.region()
            && region.is_main_buffer
            && region.diff_hunk_status.is_some()
        {
            cursor.prev();
            if cursor.region().is_none_or(|region| region.is_main_buffer) {
                cursor.next();
            }
        }

        iter::from_fn(move || {
            loop {
                let excerpt = cursor.excerpt()?;
                let buffer_snapshot = excerpt.buffer_snapshot(self);

                // If we have already retrieved metadata for this excerpt, continue to use it.
                let metadata_iter = if let Some((_, metadata)) =
                    current_excerpt_metadata.as_mut().filter(|(excerpt_info, _)| excerpt_info == &excerpt.range)
                {
                    Some(metadata)
                }
                // Otherwise, compute the intersection of the input range with the excerpt's range,
                // and retrieve the metadata for the resulting range.
                else {
                    let region = cursor.region()?;
                    let mut buffer_start;
                    if region.is_main_buffer {
                        buffer_start = region.buffer_range.start;
                        if query_range.start > region.range.start {
                            let overshoot = query_range.start - region.range.start;
                            buffer_start += overshoot;
                        }
                        buffer_start = buffer_start.min(region.buffer_range.end);
                    } else {
                        buffer_start = cursor.main_buffer_position()?;
                    }
                    let mut buffer_end = excerpt.range.context.end.summary::<MBD::TextDimension>(buffer_snapshot);
                    if let Some((end_excerpt, end_buffer_offset)) = &range_end
                        && &excerpt.range == end_excerpt
                    {
                        buffer_end = buffer_end.min(*end_buffer_offset);
                    }

                    get_buffer_metadata(buffer_snapshot, buffer_start..buffer_end)
                        .map(|iterator| &mut current_excerpt_metadata.insert((excerpt.range.clone(), iterator)).1)
                };

                // Visit each metadata item.
                if let Some((metadata_buffer_range, metadata)) = metadata_iter.and_then(Iterator::next) {
                    // Find the multibuffer regions that contain the start and end of
                    // the metadata item's range.
                    if metadata_buffer_range.start > <MBD::TextDimension>::default() {
                        while let Some(region) = cursor.region() {
                            if (region.is_main_buffer
                                && (region.buffer_range.end >= metadata_buffer_range.start
                                    || cursor.is_at_end_of_excerpt()))
                                || (!region.is_main_buffer && region.buffer_range.start == metadata_buffer_range.start)
                            {
                                break;
                            }
                            cursor.next();
                        }
                    }
                    let start_region = cursor.region()?.clone();
                    while let Some(region) = cursor.region() {
                        if region.is_main_buffer
                            && (region.buffer_range.end > metadata_buffer_range.end || cursor.is_at_end_of_excerpt())
                        {
                            break;
                        }
                        cursor.next();
                    }
                    let end_region = cursor.region();

                    // Convert the metadata item's range into multibuffer coordinates.
                    let mut start_position = start_region.range.start;
                    let region_buffer_start = start_region.buffer_range.start;
                    if start_region.is_main_buffer && metadata_buffer_range.start > region_buffer_start {
                        start_position += metadata_buffer_range.start - region_buffer_start;
                        start_position = start_position.min(start_region.range.end);
                    }

                    let mut end_position = self.max_position();
                    if let Some(end_region) = &end_region {
                        end_position = end_region.range.start;
                        let region_buffer_start = end_region.buffer_range.start;
                        if metadata_buffer_range.end > region_buffer_start {
                            end_position += metadata_buffer_range.end - region_buffer_start;
                        }
                        end_position = end_position.min(end_region.range.end);
                    }

                    if start_position <= query_range.end && end_position >= query_range.start {
                        return Some((start_position..end_position, metadata, excerpt));
                    }
                }
                // When there are no more metadata items for this excerpt, move to the next excerpt.
                else {
                    current_excerpt_metadata.take();
                    if let Some((end_excerpt, _)) = &range_end
                        && &excerpt.range == end_excerpt
                    {
                        return None;
                    }
                    cursor.next_excerpt_forwards();
                }
            }
        })
    }

    pub fn diff_hunk_before<T: ToOffset>(&self, position: T) -> Option<MultiBufferRow> {
        let offset = position.to_offset(self);

        let mut cursor = self.cursor::<DimensionPair<MultiBufferOffset, Point>, DimensionPair<BufferOffset, Point>>();
        cursor.seek(&DimensionPair { key: offset, value: None });
        cursor.seek_to_start_of_current_excerpt();
        let excerpt = cursor.excerpt()?;

        let buffer = excerpt.buffer_snapshot(self);
        let excerpt_start = excerpt.range.context.start.to_offset(buffer);
        let excerpt_end = excerpt.range.context.end.to_offset(buffer);
        let current_position = match self.anchor_before(offset) {
            Anchor::Min => 0,
            Anchor::Excerpt(excerpt_anchor) => excerpt_anchor.text_anchor().to_offset(buffer),
            Anchor::Max => unreachable!(),
        };

        if let Some(diff) = self.diff_state(excerpt.buffer_id) {
            if let Some(main_buffer) = &diff.main_buffer {
                for hunk in diff.hunks_intersecting_base_text_range_rev(excerpt_start..excerpt_end, main_buffer) {
                    if hunk.diff_base_byte_range.end >= current_position {
                        continue;
                    }
                    let hunk_start = buffer.anchor_after(hunk.diff_base_byte_range.start);
                    let start = Anchor::in_buffer(excerpt.path_key_index, hunk_start).to_point(self);
                    return Some(MultiBufferRow(start.row));
                }
            } else {
                let excerpt_end = buffer.anchor_before(excerpt_end.min(current_position));
                for hunk in diff.hunks_intersecting_range_rev(excerpt.range.context.start..excerpt_end, buffer) {
                    let hunk_end = hunk.buffer_range.end.to_offset(buffer);
                    if hunk_end >= current_position {
                        continue;
                    }
                    let start = Anchor::in_buffer(excerpt.path_key_index, hunk.buffer_range.start).to_point(self);
                    return Some(MultiBufferRow(start.row));
                }
            }
        }

        loop {
            cursor.prev_excerpt();
            let excerpt = cursor.excerpt()?;
            let buffer = excerpt.buffer_snapshot(self);

            let Some(diff) = self.diff_state(excerpt.buffer_id) else {
                continue;
            };
            if let Some(main_buffer) = &diff.main_buffer {
                let Some(hunk) = diff
                    .hunks_intersecting_base_text_range_rev(excerpt.range.context.to_offset(buffer), main_buffer)
                    .next()
                else {
                    continue;
                };
                let hunk_start = buffer.anchor_after(hunk.diff_base_byte_range.start);
                let start = Anchor::in_buffer(excerpt.path_key_index, hunk_start).to_point(self);
                return Some(MultiBufferRow(start.row));
            }
            let Some(hunk) = diff.hunks_intersecting_range_rev(excerpt.range.context.clone(), buffer).next() else {
                continue;
            };
            let start = Anchor::in_buffer(excerpt.path_key_index, hunk.buffer_range.start).to_point(self);
            return Some(MultiBufferRow(start.row));
        }
    }

    pub fn has_diff_hunks(&self) -> bool {
        self.diffs.iter().any(|diff| !diff.is_empty())
    }

    pub fn is_inside_word<T: ToOffset>(&self, position: T, scope_context: Option<CharScopeContext>) -> bool {
        let position = position.to_offset(self);
        let classifier = self.char_classifier_at(position).scope_context(scope_context);
        let next_char_kind = self.chars_at(position).next().map(|c| classifier.kind(c));
        let prev_char_kind = self.reversed_chars_at(position).next().map(|c| classifier.kind(c));
        prev_char_kind.zip(next_char_kind) == Some((CharKind::Word, CharKind::Word))
    }

    pub fn surrounding_word<T: ToOffset>(
        &self,
        start: T,
        scope_context: Option<CharScopeContext>,
    ) -> (Range<MultiBufferOffset>, Option<CharKind>) {
        let mut start = start.to_offset(self);
        let mut end = start;
        let mut next_chars = self.chars_at(start).peekable();
        let mut prev_chars = self.reversed_chars_at(start).peekable();

        let classifier = self.char_classifier_at(start).scope_context(scope_context);

        let word_kind = cmp::max(
            prev_chars.peek().copied().map(|c| classifier.kind(c)),
            next_chars.peek().copied().map(|c| classifier.kind(c)),
        );

        for ch in prev_chars {
            if Some(classifier.kind(ch)) == word_kind && ch != '\n' {
                start -= ch.len_utf8();
            } else {
                break;
            }
        }

        for ch in next_chars {
            if Some(classifier.kind(ch)) == word_kind && ch != '\n' {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        (start..end, word_kind)
    }

    pub fn char_kind_before<T: ToOffset>(&self, start: T, scope_context: Option<CharScopeContext>) -> Option<CharKind> {
        let start = start.to_offset(self);
        let classifier = self.char_classifier_at(start).scope_context(scope_context);
        self.reversed_chars_at(start).next().map(|ch| classifier.kind(ch))
    }

    pub fn all_buffer_ids(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffers.iter().map(|(id, _)| *id)
    }

    pub const fn is_singleton(&self) -> bool {
        self.singleton
    }

    pub fn as_singleton(&self) -> Option<&BufferSnapshot> {
        if self.is_singleton() { Some(self.excerpts.first()?.buffer_snapshot(self)) } else { None }
    }

    pub fn len(&self) -> MultiBufferOffset {
        self.diff_transforms.summary().output.len
    }

    pub fn max_position<MBD: MultiBufferDimension>(&self) -> MBD {
        MBD::from_summary(&self.text_summary())
    }

    pub fn is_empty(&self) -> bool {
        self.diff_transforms.summary().output.len == MultiBufferOffset(0)
    }

    pub fn widest_line_number(&self) -> u32 {
        // widest_line_number is 0-based, so 1 is added to get the displayed line number.
        self.excerpts.summary().widest_line_number + 1
    }

    pub fn bytes_in_range<T: ToOffset>(&self, range: Range<T>) -> MultiBufferBytes<'_> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let mut excerpts = self.cursor::<MultiBufferOffset, BufferOffset>();
        excerpts.seek(&range.start);

        let mut chunk;
        let mut has_trailing_newline;
        let excerpt_bytes;
        if let Some(region) = excerpts.region() {
            let mut bytes = region.buffer.bytes_in_range(
                region.buffer_range.start + (range.start - region.range.start)
                    ..(region.buffer_range.start + (range.end - region.range.start)).min(region.buffer_range.end),
            );
            chunk = bytes.next().unwrap_or(&[][..]);
            excerpt_bytes = Some(bytes);
            has_trailing_newline = region.has_trailing_newline && range.end >= region.range.end;
            if chunk.is_empty() && has_trailing_newline {
                chunk = b"\n";
                has_trailing_newline = false;
            }
        } else {
            chunk = &[][..];
            excerpt_bytes = None;
            has_trailing_newline = false;
        }

        MultiBufferBytes { range, cursor: excerpts, excerpt_bytes, has_trailing_newline, chunk }
    }

    pub fn reversed_bytes_in_range<T: ToOffset>(&self, range: Range<T>) -> ReversedMultiBufferBytes<'_> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let mut chunks = self.reversed_chunks_in_range(range.clone());
        let chunk = chunks.next().map_or(&[][..], |c| c.as_bytes());
        ReversedMultiBufferBytes { range, chunks, chunk }
    }

    pub fn row_infos(&self, start_row: MultiBufferRow) -> MultiBufferRows<'_> {
        let mut cursor = self.cursor::<Point, Point>();
        cursor.seek(&Point::new(start_row.0, 0));
        let mut result = MultiBufferRows {
            point: Point::new(0, 0),
            is_empty: self.excerpts.is_empty(),
            is_singleton: self.is_singleton(),
            cursor,
        };
        result.seek(start_row);
        result
    }

    pub fn chunks<T: ToOffset>(&self, range: Range<T>, language_aware: LanguageAwareStyling) -> MultiBufferChunks<'_> {
        let mut chunks = MultiBufferChunks {
            excerpt_offset_range: ExcerptDimension(MultiBufferOffset::ZERO)..ExcerptDimension(MultiBufferOffset::ZERO),
            range: MultiBufferOffset::ZERO..MultiBufferOffset::ZERO,
            excerpts: self.excerpts.cursor(()),
            diff_transforms: self.diff_transforms.cursor(()),
            diff_base_chunks: None,
            excerpt_chunks: None,
            buffer_chunk: None,
            language_aware,
            snapshot: self,
        };
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        chunks.seek(range);
        chunks
    }

    pub fn clip_offset(&self, offset: MultiBufferOffset, bias: Bias) -> MultiBufferOffset {
        self.clip_dimension(offset, bias, text::BufferSnapshot::clip_offset)
    }

    pub fn clip_point(&self, point: Point, bias: Bias) -> Point {
        self.clip_dimension(point, bias, text::BufferSnapshot::clip_point)
    }

    pub fn clip_offset_utf16(&self, offset: MultiBufferOffsetUtf16, bias: Bias) -> MultiBufferOffsetUtf16 {
        self.clip_dimension(offset, bias, text::BufferSnapshot::clip_offset_utf16)
    }

    pub fn clip_point_utf16(&self, point: Unclipped<PointUtf16>, bias: Bias) -> PointUtf16 {
        self.clip_dimension(point.0, bias, |buffer, point, bias| buffer.clip_point_utf16(Unclipped(point), bias))
    }

    pub fn offset_to_point(&self, offset: MultiBufferOffset) -> Point {
        self.convert_dimension(offset, text::BufferSnapshot::offset_to_point)
    }

    pub fn offset_to_point_utf16(&self, offset: MultiBufferOffset) -> PointUtf16 {
        self.convert_dimension(offset, text::BufferSnapshot::offset_to_point_utf16)
    }

    pub fn point_to_point_utf16(&self, point: Point) -> PointUtf16 {
        self.convert_dimension(point, text::BufferSnapshot::point_to_point_utf16)
    }

    pub fn point_utf16_to_point(&self, point: PointUtf16) -> Point {
        self.convert_dimension(point, text::BufferSnapshot::point_utf16_to_point)
    }

    pub fn point_to_offset(&self, point: Point) -> MultiBufferOffset {
        self.convert_dimension(point, text::BufferSnapshot::point_to_offset)
    }

    pub fn point_to_offset_utf16(&self, point: Point) -> MultiBufferOffsetUtf16 {
        self.convert_dimension(point, text::BufferSnapshot::point_to_offset_utf16)
    }

    pub fn offset_utf16_to_offset(&self, offset: MultiBufferOffsetUtf16) -> MultiBufferOffset {
        self.convert_dimension(offset, text::BufferSnapshot::offset_utf16_to_offset)
    }

    pub fn offset_to_offset_utf16(&self, offset: MultiBufferOffset) -> MultiBufferOffsetUtf16 {
        self.convert_dimension(offset, text::BufferSnapshot::offset_to_offset_utf16)
    }

    pub fn point_utf16_to_offset(&self, point: PointUtf16) -> MultiBufferOffset {
        self.convert_dimension(point, text::BufferSnapshot::point_utf16_to_offset)
    }

    pub fn point_utf16_to_offset_utf16(&self, point: PointUtf16) -> MultiBufferOffsetUtf16 {
        self.convert_dimension(point, text::BufferSnapshot::point_utf16_to_offset_utf16)
    }

    fn clip_dimension<MBD, BD>(
        &self,
        position: MBD,
        bias: Bias,
        clip_buffer_position: fn(&text::BufferSnapshot, BD, Bias) -> BD,
    ) -> MBD
    where
        MBD: MultiBufferDimension + Ord + Sub + AddAssign<<MBD as Sub>::Output>,
        BD: TextDimension + Sub<Output = <MBD as Sub>::Output> + AddAssign<<MBD as Sub>::Output>,
    {
        let mut cursor = self.cursor::<MBD, BD>();
        cursor.seek(&position);
        if let Some(region) = cursor.region() {
            if position >= region.range.end {
                return region.range.end;
            }
            let overshoot = position - region.range.start;
            let mut buffer_position = region.buffer_range.start;
            buffer_position += overshoot;
            let clipped_buffer_position = clip_buffer_position(region.buffer, buffer_position, bias);
            let mut position = region.range.start;
            position += clipped_buffer_position - region.buffer_range.start;
            position
        } else {
            self.max_position()
        }
    }

    fn convert_dimension<MBR1, MBR2, BR1, BR2>(
        &self,
        key: MBR1,
        convert_buffer_dimension: fn(&text::BufferSnapshot, BR1) -> BR2,
    ) -> MBR2
    where
        MBR1: MultiBufferDimension + Ord + Sub + AddAssign<<MBR1 as Sub>::Output>,
        BR1: TextDimension + Sub<Output = <MBR1 as Sub>::Output> + AddAssign<<MBR1 as Sub>::Output>,
        MBR2: MultiBufferDimension + Ord + Sub + AddAssign<<MBR2 as Sub>::Output>,
        BR2: TextDimension + Sub<Output = <MBR2 as Sub>::Output> + AddAssign<<MBR2 as Sub>::Output>,
    {
        let mut cursor = self.cursor::<DimensionPair<MBR1, MBR2>, DimensionPair<BR1, BR2>>();
        cursor.seek(&DimensionPair { key, value: None });
        if let Some(region) = cursor.region() {
            if key >= region.range.end.key {
                return region.range.end.value.expect("seeked region has a value");
            }
            let start_key = region.range.start.key;
            let start_value = region.range.start.value.expect("seeked region has a value");
            let buffer_start_key = region.buffer_range.start.key;
            let buffer_start_value = region.buffer_range.start.value.expect("seeked region has a value");
            let mut buffer_key = buffer_start_key;
            buffer_key += key - start_key;
            let buffer_value = convert_buffer_dimension(region.buffer, buffer_key);
            let mut result = start_value;
            result += buffer_value - buffer_start_value;
            result
        } else {
            self.max_position()
        }
    }

    pub fn point_to_buffer_offset<T: ToOffset>(&self, point: T) -> Option<(&BufferSnapshot, BufferOffset)> {
        let offset = point.to_offset(self);
        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        cursor.seek(&offset);
        let region = cursor.region()?;
        let overshoot = offset - region.range.start;
        let buffer_offset = region.buffer_range.start + overshoot;
        if buffer_offset == BufferOffset(region.buffer.len() + 1)
            && region.has_trailing_newline
            && !region.is_main_buffer
        {
            let main_buffer_position = cursor.main_buffer_position()?;
            let buffer_snapshot = cursor.excerpt()?.buffer_snapshot(self);
            return Some((buffer_snapshot, main_buffer_position));
        } else if buffer_offset > BufferOffset(region.buffer.len()) {
            return None;
        }
        Some((region.buffer, buffer_offset))
    }

    pub fn point_to_buffer_point(&self, point: Point) -> Option<(&BufferSnapshot, Point)> {
        let mut cursor = self.cursor::<Point, Point>();
        cursor.seek(&point);
        let region = cursor.region()?;
        let overshoot = point - region.range.start;
        let buffer_point = region.buffer_range.start + overshoot;
        let excerpt = cursor.excerpt()?;
        if buffer_point == region.buffer.max_point() + Point::new(1, 0)
            && region.has_trailing_newline
            && !region.is_main_buffer
        {
            return Some((excerpt.buffer_snapshot(self), cursor.main_buffer_position()?));
        } else if buffer_point > region.buffer.max_point() {
            return None;
        }
        Some((region.buffer, buffer_point))
    }

    pub fn line_len(&self, row: MultiBufferRow) -> u32 {
        if let Some((_, range)) = self.buffer_line_for_row(row) { range.end.column - range.start.column } else { 0 }
    }

    pub fn buffer_line_for_row(&self, row: MultiBufferRow) -> Option<(&BufferSnapshot, Range<Point>)> {
        let mut cursor = self.cursor::<Point, Point>();
        let point = Point::new(row.0, 0);
        cursor.seek(&point);
        let region = cursor.region()?;
        let overshoot = point.min(region.range.end) - region.range.start;
        let buffer_point = region.buffer_range.start + overshoot;
        if buffer_point.row > region.buffer_range.end.row {
            return None;
        }
        let line_start = Point::new(buffer_point.row, 0).max(region.buffer_range.start);
        let line_end =
            Point::new(buffer_point.row, region.buffer.line_len(buffer_point.row)).min(region.buffer_range.end);
        Some((region.buffer, line_start..line_end))
    }

    pub fn max_point(&self) -> Point {
        self.text_summary().lines
    }

    pub fn max_row(&self) -> MultiBufferRow {
        MultiBufferRow(self.text_summary().lines.row)
    }

    pub fn text_summary(&self) -> MBTextSummary {
        self.diff_transforms.summary().output
    }

    pub fn text_summary_for_range<MBD, O>(&self, range: Range<O>) -> MBD
    where
        MBD: MultiBufferDimension + AddAssign,
        O: ToOffset,
    {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let mut cursor = self.diff_transforms.cursor::<Dimensions<MultiBufferOffset, ExcerptOffset>>(());
        cursor.seek(&range.start, Bias::Right);

        let Some(first_transform) = cursor.item() else {
            return MBD::from_summary(&MBTextSummary::default());
        };

        let diff_transform_start = cursor.start().0;
        let diff_transform_end = cursor.end().0;
        let diff_start = range.start;
        let start_overshoot = diff_start - diff_transform_start;
        let end_overshoot = cmp::min(range.end, diff_transform_end) - diff_transform_start;

        let mut result = match first_transform {
            DiffTransform::BufferContent { .. } => {
                let excerpt_start = cursor.start().1 + start_overshoot;
                let excerpt_end = cursor.start().1 + end_overshoot;
                self.text_summary_for_excerpt_offset_range(excerpt_start..excerpt_end)
            }
            DiffTransform::DeletedHunk { buffer_id, base_text_byte_range, has_trailing_newline, .. } => {
                let buffer_start = base_text_byte_range.start + start_overshoot;
                let mut buffer_end = base_text_byte_range.start + end_overshoot;
                let Some(base_text) = self.diff_state(*buffer_id).map(|diff| diff.base_text()) else {
                    panic!("{:?} is in non-existent deleted hunk", range.start)
                };

                let include_trailing_newline = *has_trailing_newline && range.end >= diff_transform_end;
                if include_trailing_newline {
                    buffer_end -= 1;
                }

                let mut summary = base_text.text_summary_for_range::<MBD::TextDimension, _>(buffer_start..buffer_end);

                if include_trailing_newline {
                    summary.add_assign(&<MBD::TextDimension>::from_text_summary(&TextSummary::newline()));
                }

                let mut result = MBD::default();
                result.add_text_dim(&summary);
                result
            }
        };
        if range.end < diff_transform_end {
            return result;
        }

        cursor.next();
        result.add_mb_text_summary(&cursor.summary::<_, OutputDimension<_>>(&range.end, Bias::Right).0);

        let Some(last_transform) = cursor.item() else {
            return result;
        };

        let overshoot = range.end - cursor.start().0;
        let suffix = match last_transform {
            DiffTransform::BufferContent { .. } => {
                let end = cursor.start().1 + overshoot;
                self.text_summary_for_excerpt_offset_range::<MBD>(cursor.start().1..end)
            }
            DiffTransform::DeletedHunk { base_text_byte_range, buffer_id, has_trailing_newline, .. } => {
                let buffer_end = base_text_byte_range.start + overshoot;
                let Some(base_text) = self.diff_state(*buffer_id).map(|diff| diff.base_text()) else {
                    panic!("{:?} is in non-existent deleted hunk", range.end)
                };

                let mut suffix =
                    base_text.text_summary_for_range::<MBD::TextDimension, _>(base_text_byte_range.start..buffer_end);
                if *has_trailing_newline && buffer_end == base_text_byte_range.end + 1 {
                    suffix.add_assign(&<MBD::TextDimension>::from_text_summary(&TextSummary::from("\n")));
                }

                let mut result = MBD::default();
                result.add_text_dim(&suffix);
                result
            }
        };

        result += suffix;
        result
    }

    fn text_summary_for_excerpt_offset_range<MBD>(&self, mut range: Range<ExcerptOffset>) -> MBD
    where
        MBD: MultiBufferDimension + AddAssign,
    {
        let mut summary = MBD::default();
        let mut cursor = self.excerpts.cursor::<ExcerptOffset>(());
        cursor.seek(&range.start, Bias::Right);
        if let Some(excerpt) = cursor.item() {
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            let mut end_before_newline = cursor.end();
            if excerpt.has_trailing_newline {
                end_before_newline -= 1;
            }

            let excerpt_start = excerpt.range.context.start.to_offset(buffer_snapshot);
            let start_in_excerpt = excerpt_start + (range.start - *cursor.start());
            let end_in_excerpt = excerpt_start + (cmp::min(end_before_newline, range.end) - *cursor.start());
            summary.add_text_dim(
                &buffer_snapshot.text_summary_for_range::<MBD::TextDimension, _>(start_in_excerpt..end_in_excerpt),
            );

            if range.end > end_before_newline {
                summary.add_mb_text_summary(&MBTextSummary::from(TextSummary::newline()));
            }

            cursor.next();
        }

        if range.end > *cursor.start() {
            summary += cursor.summary::<_, ExcerptDimension<MBD>>(&range.end, Bias::Right).0;
            if let Some(excerpt) = cursor.item() {
                let buffer_snapshot = excerpt.buffer_snapshot(self);
                range.end = cmp::max(*cursor.start(), range.end);

                let excerpt_start = excerpt.range.context.start.to_offset(buffer_snapshot);
                let end_in_excerpt = excerpt_start + (range.end - *cursor.start());
                summary.add_text_dim(
                    &buffer_snapshot.text_summary_for_range::<MBD::TextDimension, _>(excerpt_start..end_in_excerpt),
                );
            }
        }

        summary
    }

    pub fn excerpts(&self) -> impl Iterator<Item = ExcerptRange<text::Anchor>> {
        self.excerpts.iter().map(|excerpt| excerpt.range.clone())
    }

    fn cursor<MBD, BD>(&self) -> MultiBufferCursor<'_, MBD, BD>
    where
        MBD: MultiBufferDimension + Ord + Sub + AddAssign<<MBD as Sub>::Output>,
        BD: TextDimension + AddAssign<<MBD as Sub>::Output>,
    {
        let excerpts = self.excerpts.cursor(());
        let diff_transforms = self.diff_transforms.cursor(());
        MultiBufferCursor { excerpts, diff_transforms, cached_region: OnceCell::new(), snapshot: self }
    }

    pub fn excerpt_before(&self, anchor: Anchor) -> Option<ExcerptRange<text::Anchor>> {
        let target = anchor.try_seek_target(self)?;
        let mut excerpts = self.excerpts.cursor::<ExcerptSummary>(());
        excerpts.seek(&target, Bias::Left);
        excerpts.prev();
        Some(excerpts.item()?.range.clone())
    }

    pub fn excerpt_boundaries_in_range<R, T>(&self, range: R) -> impl Iterator<Item = ExcerptBoundary> + '_
    where
        R: RangeBounds<T>,
        T: ToOffset,
    {
        let start_offset;
        let start = match range.start_bound() {
            Bound::Included(start) => {
                start_offset = start.to_offset(self);
                Bound::Included(start_offset)
            }
            Bound::Excluded(_) => {
                panic!("not supported")
            }
            Bound::Unbounded => {
                start_offset = MultiBufferOffset::ZERO;
                Bound::Unbounded
            }
        };
        let end = match range.end_bound() {
            Bound::Included(end) => Bound::Included(end.to_offset(self)),
            Bound::Excluded(end) => Bound::Excluded(end.to_offset(self)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let bounds = (start, end);
        let mut cursor = self.cursor::<DimensionPair<MultiBufferOffset, Point>, BufferOffset>();
        cursor.seek(&DimensionPair { key: start_offset, value: None });

        // The cursor is on the diff transform containing `start_offset`. Seek to the start of
        // its excerpt, so a range starting on a transform in the middle of an excerpt is not
        // mistaken for one starting at the excerpt.
        cursor.seek_to_start_of_current_excerpt();
        if cursor.fetch_excerpt_with_range().is_some_and(|(_, range)| bounds.contains(&range.start.key)) {
            cursor.prev_excerpt();
        }
        let mut prev_excerpt = cursor.fetch_excerpt_with_range().map(|(excerpt, _)| excerpt);

        cursor.next_excerpt_forwards();

        iter::from_fn(move || {
            if self.singleton {
                return None;
            }

            let (next_excerpt, next_range) = cursor.fetch_excerpt_with_range()?;
            cursor.next_excerpt_forwards();
            // The walk starts at or after the range start, so the first excerpt outside the
            // range is past its end.
            if !bounds.contains(&next_range.start.key) {
                return None;
            }

            let next_region_start = next_range.start.value.expect("fetched region has a value");
            let next_region_end = if let Some((_, range)) = cursor.fetch_excerpt_with_range() {
                range.start.value.expect("fetched region has a value")
            } else {
                self.max_point()
            };

            let prev = prev_excerpt.as_ref().map(|excerpt| ExcerptBoundaryInfo {
                start_anchor: Anchor::in_buffer(excerpt.path_key_index, excerpt.range.context.start),
                range: excerpt.range.clone(),
                end_row: MultiBufferRow(next_region_start.row),
            });

            let next = ExcerptBoundaryInfo {
                start_anchor: Anchor::in_buffer(next_excerpt.path_key_index, next_excerpt.range.context.start),
                range: next_excerpt.range.clone(),
                end_row: if next_excerpt.has_trailing_newline {
                    MultiBufferRow(next_region_end.row - 1)
                } else {
                    MultiBufferRow(next_region_end.row)
                },
            };

            let row = MultiBufferRow(next_region_start.row);

            prev_excerpt = Some(next_excerpt);

            Some(ExcerptBoundary { prev, next, row })
        })
    }

    pub const fn edit_count(&self) -> usize {
        self.edit_count
    }

    pub const fn non_text_state_update_count(&self) -> usize {
        self.non_text_state_update_count
    }

    /// Allows converting several ranges within the same excerpt between buffer offsets and multibuffer offsets.
    ///
    /// If the input range is contained in a single excerpt, invokes the callback with the full range of that excerpt
    /// and the input range both converted to buffer coordinates. The buffer ranges returned by the callback are lifted back
    /// to multibuffer offsets and returned.
    ///
    /// Returns `None` if the input range spans multiple excerpts.
    pub fn map_excerpt_ranges<'a, T>(
        &'a self,
        position: Range<MultiBufferOffset>,
        f: impl FnOnce(&'a BufferSnapshot, ExcerptRange<BufferOffset>, Range<BufferOffset>) -> Vec<(Range<BufferOffset>, T)>,
    ) -> Option<Vec<(Range<MultiBufferOffset>, T)>> {
        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        cursor.seek(&position.start);

        let region = cursor.region()?;
        if !region.is_main_buffer {
            return None;
        }
        let excerpt = cursor.excerpt()?;
        let excerpt_start = *cursor.excerpts.start();
        let input_buffer_start = cursor.buffer_position_at(&position.start)?;

        cursor.seek_forward(&position.end);
        if cursor.excerpt()? != excerpt {
            return None;
        }
        let region = cursor.region()?;
        if !region.is_main_buffer {
            return None;
        }
        let input_buffer_end = cursor.buffer_position_at(&position.end)?;
        let input_buffer_range = input_buffer_start..input_buffer_end;
        let buffer = excerpt.buffer_snapshot(self);
        let excerpt_context_range = excerpt.range.context.to_offset(buffer);
        let excerpt_context_range = BufferOffset(excerpt_context_range.start)..BufferOffset(excerpt_context_range.end);
        let excerpt_primary_range = excerpt.range.primary.to_offset(buffer);
        let excerpt_primary_range = BufferOffset(excerpt_primary_range.start)..BufferOffset(excerpt_primary_range.end);
        let results = f(
            buffer,
            ExcerptRange { context: excerpt_context_range.clone(), primary: excerpt_primary_range },
            input_buffer_range,
        );
        let mut diff_transforms = cursor.diff_transforms;
        Some(
            results
                .into_iter()
                .map(|(buffer_range, metadata)| {
                    let clamped_start =
                        buffer_range.start.max(excerpt_context_range.start).min(excerpt_context_range.end);
                    let clamped_end = buffer_range.end.max(clamped_start).min(excerpt_context_range.end);
                    let excerpt_offset_start = excerpt_start + (clamped_start.0 - excerpt_context_range.start.0);
                    let excerpt_offset_end = excerpt_start + (clamped_end.0 - excerpt_context_range.start.0);

                    diff_transforms.seek(&excerpt_offset_start, Bias::Right);
                    let mut output_start = diff_transforms.start().output_dimension;
                    output_start += excerpt_offset_start - diff_transforms.start().excerpt_dimension;

                    diff_transforms.seek_forward(&excerpt_offset_end, Bias::Right);
                    let mut output_end = diff_transforms.start().output_dimension;
                    output_end += excerpt_offset_end - diff_transforms.start().excerpt_dimension;

                    (output_start.0..output_end.0, metadata)
                })
                .collect(),
        )
    }

    pub const fn trailing_excerpt_update_count(&self) -> usize {
        self.trailing_excerpt_update_count
    }

    pub fn file_at<T: ToOffset>(&self, point: T) -> Option<&Arc<dyn File>> {
        let (buffer, _) = self.point_to_buffer_offset(point)?;
        buffer.file()
    }

    pub const fn is_dirty(&self) -> bool {
        self.buffers.status_counts.dirty > 0
    }

    pub const fn has_deleted_file(&self) -> bool {
        self.buffers.status_counts.deleted_file > 0
    }

    pub const fn has_conflict(&self) -> bool {
        self.buffers.status_counts.conflict > 0
    }

    pub fn path_for_buffer(&self, buffer_id: BufferId) -> Option<&PathKey> {
        Some(&self.buffers.get(&buffer_id)?.path_key)
    }

    pub(crate) fn path_key_index_for_buffer(&self, buffer_id: BufferId) -> Option<PathKeyIndex> {
        let snapshot = self.buffers.get(&buffer_id)?;
        Some(snapshot.path_key_index)
    }

    fn first_excerpt_for_buffer(&self, buffer_id: BufferId) -> Option<&Excerpt> {
        let path_key = &self.buffers.get(&buffer_id)?.path_key;
        self.first_excerpt_for_path(path_key)
    }

    fn first_excerpt_for_path(&self, path_key: &PathKey) -> Option<&Excerpt> {
        let (_, _, first_excerpt) = self.excerpts.find::<PathKey, _>((), path_key, Bias::Left);
        first_excerpt
    }

    pub fn buffer_for_id(&self, id: BufferId) -> Option<&BufferSnapshot> {
        self.buffers.get(&id).map(|state| &state.buffer_snapshot)
    }

    fn try_path_for_anchor(&self, anchor: ExcerptAnchor) -> Option<&PathKey> {
        self.path_keys.get_index(anchor.path.0 as usize)
    }

    /// Returns the excerpt containing range and its offset start within the multibuffer or none if `range` spans multiple excerpts.
    pub fn excerpt_containing<T: ToOffset>(
        &self,
        range: Range<T>,
    ) -> Option<(&BufferSnapshot, ExcerptRange<text::Anchor>)> {
        let range = range.start.to_offset(self)..range.end.to_offset(self);
        let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
        cursor.seek(&range.start);

        let start_excerpt = cursor.excerpt()?;
        if range.end != range.start {
            cursor.seek_forward(&range.end);
            if cursor.excerpt()? != start_excerpt {
                return None;
            }
        }

        Some((start_excerpt.buffer_snapshot(self), start_excerpt.range.clone()))
    }

    pub const fn show_headers(&self) -> bool {
        self.show_headers
    }

    pub fn diff_for_buffer_id(&self, buffer_id: BufferId) -> Option<&BufferDiffSnapshot> {
        self.diff_state(buffer_id).map(|diff| &diff.diff)
    }

    fn diff_state(&self, buffer_id: BufferId) -> Option<&DiffStateSnapshot> {
        find_diff_state(&self.diffs, buffer_id)
    }

    pub fn total_changed_lines(&self) -> (u32, u32) {
        let summary = self.diffs.summary();
        (summary.added_rows, summary.removed_rows)
    }

    pub const fn all_diff_hunks_expanded(&self) -> bool {
        self.all_diff_hunks_expanded
    }

    fn excerpt_edits_for_diff_change(
        &self,
        path: &PathKey,
        diff_change_range: Range<usize>,
    ) -> Vec<Edit<ExcerptDimension<MultiBufferOffset>>> {
        let mut excerpt_edits = Vec::new();
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(path, Bias::Left);
        while let Some(excerpt) = cursor.item()
            && &excerpt.path_key == path
        {
            let buffer_snapshot = excerpt.buffer_snapshot(self);
            let excerpt_buffer_range = excerpt.range.context.to_offset(buffer_snapshot);
            let excerpt_start = cursor.start().clone();
            let excerpt_len = excerpt.text_summary.len;
            cursor.next();
            if diff_change_range.end < excerpt_buffer_range.start || diff_change_range.start > excerpt_buffer_range.end
            {
                continue;
            }
            let diff_change_start_in_excerpt = diff_change_range.start.saturating_sub(excerpt_buffer_range.start);
            let diff_change_end_in_excerpt = diff_change_range.end.saturating_sub(excerpt_buffer_range.start);
            let edit_start = excerpt_start.len() + diff_change_start_in_excerpt.min(excerpt_len);
            let edit_end = excerpt_start.len() + diff_change_end_in_excerpt.min(excerpt_len);
            excerpt_edits.push(Edit { old: edit_start..edit_end, new: edit_start..edit_end });
        }
        excerpt_edits
    }

    fn excerpts_for_path<'a>(&'a self, path_key: &'a PathKey) -> impl Iterator<Item = ExcerptRange<text::Anchor>> + 'a {
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(path_key, Bias::Left);
        cursor.take_while(move |item| &item.path_key == path_key).map(|excerpt| excerpt.range.clone())
    }

    /// If the given multibuffer range is contained in a single excerpt and contains no deleted hunks,
    /// returns the corresponding buffer range.
    ///
    /// Otherwise, returns None.
    pub fn range_to_buffer_range<MBD>(&self, range: Range<MBD>) -> Option<(&BufferSnapshot, Range<MBD::TextDimension>)>
    where
        MBD: MultiBufferDimension + Ord + Sub + AddAssign<<MBD as Sub>::Output>,
        MBD::TextDimension: AddAssign<<MBD as Sub>::Output>,
    {
        let mut cursor = self.cursor::<MBD, MBD::TextDimension>();
        cursor.seek(&range.start);

        let start_region = cursor.region()?.clone();

        while let Some(region) = cursor.region()
            && region.range.end < range.end
        {
            if !region.is_main_buffer {
                return None;
            }
            cursor.next();
        }

        let end_region = cursor.region()?;
        if end_region.buffer.buffer_id() != start_region.buffer.buffer_id() {
            return None;
        }

        let mut buffer_start = start_region.buffer_range.start;
        buffer_start += range.start - start_region.range.start;
        let mut buffer_end = end_region.buffer_range.start;
        buffer_end += range.end - end_region.range.start;

        Some((start_region.buffer, buffer_start..buffer_end))
    }

    /// If the two endpoints of the range lie in the same excerpt, return the corresponding
    /// buffer range. Intervening deleted hunks are allowed.
    pub fn anchor_range_to_buffer_anchor_range(
        &self,
        range: Range<Anchor>,
    ) -> Option<(&BufferSnapshot, Range<text::Anchor>)> {
        let mut cursor = self.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(&range.start.seek_target(self), Bias::Left);

        let start_excerpt = cursor.item()?;

        let snapshot = start_excerpt.buffer_snapshot(self);

        cursor.seek(&range.end.seek_target(self), Bias::Left);

        let end_excerpt = cursor.item()?;

        if start_excerpt != end_excerpt {
            return None;
        }

        if let Anchor::Excerpt(excerpt_anchor) = range.start
            && (excerpt_anchor.path != start_excerpt.path_key_index
                || excerpt_anchor.buffer_id() != snapshot.buffer_id())
        {
            return None;
        }
        if let Anchor::Excerpt(excerpt_anchor) = range.end
            && (excerpt_anchor.path != end_excerpt.path_key_index || excerpt_anchor.buffer_id() != snapshot.buffer_id())
        {
            return None;
        }

        Some((snapshot, range.start.text_anchor_in(snapshot)..range.end.text_anchor_in(snapshot)))
    }

    /// Returns all nonempty intersections of the given buffer range with excerpts in the multibuffer in order.
    ///
    /// The multibuffer ranges are split to not intersect deleted hunks.
    pub fn buffer_range_to_excerpt_ranges(&self, range: Range<text::Anchor>) -> impl Iterator<Item = Range<Anchor>> {
        assert!(range.start.buffer_id == range.end.buffer_id);

        let buffer_id = range.start.buffer_id;
        self.buffers
            .get(&buffer_id)
            .map(|buffer_state_snapshot| {
                let path_key_index = buffer_state_snapshot.path_key_index;
                let buffer_snapshot = &buffer_state_snapshot.buffer_snapshot;
                let buffer_range = range.to_offset(buffer_snapshot);

                let start = Anchor::in_buffer(path_key_index, range.start).to_offset(self);
                let mut cursor = self.cursor::<MultiBufferOffset, BufferOffset>();
                cursor.seek(&start);
                iter::from_fn(move || {
                    while let Some(region) = cursor.region()
                        && !region.is_main_buffer
                    {
                        cursor.next();
                    }

                    let region = cursor.region()?;
                    if region.buffer.buffer_id() != buffer_id
                        || region.buffer_range.start > BufferOffset(buffer_range.end)
                    {
                        return None;
                    }

                    let start = region.buffer_range.start.max(BufferOffset(buffer_range.start));
                    let mut end = region.buffer_range.end.min(BufferOffset(buffer_range.end));

                    cursor.next();
                    while let Some(region) = cursor.region()
                        && region.is_main_buffer
                        && region.buffer.buffer_id() == buffer_id
                        && region.buffer_range.start <= end
                    {
                        end = end.max(region.buffer_range.end).min(BufferOffset(buffer_range.end));
                        cursor.next();
                    }

                    let multibuffer_range =
                        Anchor::range_in_buffer(path_key_index, buffer_snapshot.anchor_range_inside(start..end));
                    Some(multibuffer_range)
                })
            })
            .into_iter()
            .flatten()
    }

    pub fn buffers_with_paths(&self) -> impl '_ + Iterator<Item = (&BufferSnapshot, &PathKey)> {
        self.buffers.values().map(|buffer| (&buffer.buffer_snapshot, &buffer.path_key))
    }

    pub fn range_for_buffer(&self, buffer_id: BufferId) -> Option<Range<Point>> {
        let path_key = self.path_key_index_for_buffer(buffer_id)?;
        let start = Anchor::in_buffer(path_key, text::Anchor::min_for_buffer(buffer_id));
        let end = Anchor::in_buffer(path_key, text::Anchor::max_for_buffer(buffer_id));
        Some((start..end).to_point(self))
    }
}

impl Excerpt {
    fn new(
        path_key: PathKey,
        path_key_index: PathKeyIndex,
        buffer_snapshot: &BufferSnapshot,
        range: ExcerptRange<text::Anchor>,
        has_trailing_newline: bool,
    ) -> Self {
        Excerpt {
            path_key,
            path_key_index,
            buffer_id: buffer_snapshot.buffer_id(),
            max_buffer_row: range.context.end.to_point(buffer_snapshot).row,
            text_summary: buffer_snapshot
                .text_summary_for_range::<TextSummary, _>(range.context.to_offset(buffer_snapshot)),
            range,
            has_trailing_newline,
        }
    }

    fn buffer_snapshot<'a>(&self, snapshot: &'a MultiBufferSnapshot) -> &'a BufferSnapshot {
        &snapshot.buffers.get(&self.buffer_id).expect("buffer snapshot not found for excerpt").buffer_snapshot
    }

    fn buffer(&self, multibuffer: &MultiBuffer) -> Entity<Buffer> {
        multibuffer.buffer(self.buffer_id).expect("buffer entity not found for excerpt")
    }

    fn chunks_in_range<'a>(
        &'a self,
        range: Range<usize>,
        language_aware: LanguageAwareStyling,
        snapshot: &'a MultiBufferSnapshot,
    ) -> ExcerptChunks<'a> {
        let buffer = self.buffer_snapshot(snapshot);
        let content_start = self.range.context.start.to_offset(buffer);
        let chunks_start = content_start + range.start;
        let chunks_end = content_start + cmp::min(range.end, self.text_summary.len);

        let has_footer =
            self.has_trailing_newline && range.start <= self.text_summary.len && range.end > self.text_summary.len;

        let content_chunks = buffer.chunks(chunks_start..chunks_end, language_aware);

        ExcerptChunks { content_chunks, has_footer, end: self.end_anchor() }
    }

    fn seek_chunks(&self, excerpt_chunks: &mut ExcerptChunks, range: Range<usize>, snapshot: &MultiBufferSnapshot) {
        let buffer = self.buffer_snapshot(snapshot);
        let content_start = self.range.context.start.to_offset(buffer);
        let chunks_start = content_start + range.start;
        let chunks_end = content_start + cmp::min(range.end, self.text_summary.len);
        excerpt_chunks.content_chunks.seek(chunks_start..chunks_end);
        excerpt_chunks.has_footer =
            self.has_trailing_newline && range.start <= self.text_summary.len && range.end > self.text_summary.len;
    }

    fn clip_anchor(&self, text_anchor: text::Anchor, snapshot: &MultiBufferSnapshot) -> text::Anchor {
        let buffer = self.buffer_snapshot(snapshot);
        if text_anchor.cmp(&self.range.context.start, buffer).is_lt() {
            self.range.context.start
        } else if text_anchor.cmp(&self.range.context.end, buffer).is_gt() {
            self.range.context.end
        } else {
            text_anchor
        }
    }

    pub(crate) fn contains(&self, anchor: &ExcerptAnchor, snapshot: &MultiBufferSnapshot) -> bool {
        self.path_key_index == anchor.path
            && self.buffer_id == anchor.text_anchor.buffer_id
            && self.range.contains(&anchor.text_anchor(), self.buffer_snapshot(snapshot))
    }

    const fn start_anchor(&self) -> ExcerptAnchor {
        ExcerptAnchor::in_buffer(self.path_key_index, self.range.context.start)
    }

    const fn end_anchor(&self) -> ExcerptAnchor {
        ExcerptAnchor::in_buffer(self.path_key_index, self.range.context.end)
    }
}

impl PartialEq for Excerpt {
    fn eq(&self, other: &Self) -> bool {
        self.path_key_index == other.path_key_index
            && self.buffer_id == other.buffer_id
            && self.range.context == other.range.context
    }
}

impl sum_tree::Item for Excerpt {
    type Summary = ExcerptSummary;

    fn summary(&self, _cx: ()) -> Self::Summary {
        let mut text = self.text_summary;
        if self.has_trailing_newline {
            text += TextSummary::from("\n");
        }
        ExcerptSummary {
            path_key: self.path_key.clone(),
            path_key_index: Some(self.path_key_index),
            max_anchor: Some(self.range.context.end),
            widest_line_number: self.max_buffer_row,
            text: text.into(),
            count: 1,
        }
    }
}

impl DiffTransform {
    const fn hunk_info(&self) -> Option<DiffTransformHunkInfo> {
        match self {
            DiffTransform::DeletedHunk { hunk_info, .. } => Some(*hunk_info),
            DiffTransform::BufferContent { inserted_hunk_info, .. } => *inserted_hunk_info,
        }
    }
}

impl sum_tree::Item for DiffTransform {
    type Summary = DiffTransformSummary;

    fn summary(&self, (): <Self::Summary as sum_tree::Summary>::Context<'_>) -> Self::Summary {
        match self {
            DiffTransform::BufferContent { summary, .. } => DiffTransformSummary { input: *summary, output: *summary },
            &DiffTransform::DeletedHunk { summary, .. } => {
                DiffTransformSummary { input: MBTextSummary::default(), output: summary.into() }
            }
        }
    }
}

impl DiffTransformSummary {
    const fn excerpt_len(&self) -> ExcerptOffset {
        ExcerptDimension(self.input.len)
    }
}

impl sum_tree::ContextLessSummary for DiffTransformSummary {
    fn zero() -> Self {
        DiffTransformSummary { input: MBTextSummary::default(), output: MBTextSummary::default() }
    }

    fn add_summary(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
    }
}

impl Dimension<'_, ExcerptSummary> for PathKey {
    fn zero((): <ExcerptSummary as sum_tree::Summary>::Context<'_>) -> Self {
        PathKey::min()
    }

    fn add_summary(&mut self, summary: &'_ ExcerptSummary, _cx: <ExcerptSummary as sum_tree::Summary>::Context<'_>) {
        *self = summary.path_key.clone();
    }
}

impl Dimension<'_, ExcerptSummary> for MultiBufferOffset {
    fn zero((): <ExcerptSummary as sum_tree::Summary>::Context<'_>) -> Self {
        MultiBufferOffset::ZERO
    }

    fn add_summary(&mut self, summary: &'_ ExcerptSummary, _cx: <ExcerptSummary as sum_tree::Summary>::Context<'_>) {
        *self += summary.text.len;
    }
}

impl sum_tree::ContextLessSummary for ExcerptSummary {
    fn zero() -> Self {
        Self::min()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.path_key = summary.path_key.clone();
        self.path_key_index = summary.path_key_index;
        self.max_anchor = summary.max_anchor;
        self.text += summary.text;
        self.widest_line_number = cmp::max(self.widest_line_number, summary.widest_line_number);
        self.count += summary.count;
    }
}

impl sum_tree::SeekTarget<'_, ExcerptSummary, ExcerptSummary> for AnchorSeekTarget<'_> {
    fn cmp(
        &self,
        cursor_location: &ExcerptSummary,
        _cx: <ExcerptSummary as sum_tree::Summary>::Context<'_>,
    ) -> Ordering {
        match self {
            AnchorSeekTarget::Missing { path_key, buffer_id } => match Ord::cmp(*path_key, &cursor_location.path_key) {
                Ordering::Less => Ordering::Less,
                Ordering::Equal => match &cursor_location.max_anchor {
                    Some(max_anchor) if *buffer_id < max_anchor.buffer_id => Ordering::Less,
                    _ => Ordering::Greater,
                },
                Ordering::Greater => Ordering::Greater,
            },
            AnchorSeekTarget::Excerpt { path_key, path_key_index, anchor, snapshot } => {
                if Some(*path_key_index) != cursor_location.path_key_index {
                    Ord::cmp(*path_key, &cursor_location.path_key)
                } else if let Some(max_anchor) = cursor_location.max_anchor {
                    anchor.cmp(&max_anchor, snapshot)
                } else {
                    Ordering::Greater
                }
            }
            AnchorSeekTarget::Empty => Ordering::Greater,
        }
    }
}

impl sum_tree::ContextLessSummary for PathKey {
    fn zero() -> Self {
        PathKey::min()
    }

    fn add_summary(&mut self, summary: &Self) {
        *self = summary.clone();
    }
}

impl sum_tree::SeekTarget<'_, ExcerptSummary, ExcerptSummary> for PathKey {
    fn cmp(
        &self,
        cursor_location: &ExcerptSummary,
        _cx: <ExcerptSummary as sum_tree::Summary>::Context<'_>,
    ) -> Ordering {
        Ord::cmp(self, &cursor_location.path_key)
    }
}

impl<'a, MBD> Dimension<'a, ExcerptSummary> for ExcerptDimension<MBD>
where
    MBD: MultiBufferDimension + Default,
{
    fn zero((): ()) -> Self {
        ExcerptDimension(MBD::default())
    }

    fn add_summary(&mut self, summary: &'a ExcerptSummary, (): ()) {
        MultiBufferDimension::add_mb_text_summary(&mut self.0, &summary.text);
    }
}

#[cfg(test)]
mod tests {
    use super::BufferStatus;
    use super::BufferStatusCounts;

    #[test]
    fn buffer_status_counts_add_and_remove() {
        let clean = BufferStatus { is_dirty: false, has_deleted_file: false, has_conflict: false };
        let dirty = BufferStatus { is_dirty: true, ..clean };
        let conflicted = BufferStatus { is_dirty: true, has_conflict: true, ..clean };
        let deleted = BufferStatus { has_deleted_file: true, ..clean };

        let mut counts = BufferStatusCounts::default();
        for status in [clean, dirty, conflicted, deleted] {
            counts.add(status);
        }
        assert_eq!(counts, BufferStatusCounts { dirty: 2, deleted_file: 1, conflict: 1 });

        // Replacing a buffer's status adds the new one and removes the old one.
        counts.add(clean);
        counts.remove(conflicted);
        assert_eq!(counts, BufferStatusCounts { dirty: 1, deleted_file: 1, conflict: 0 });

        for status in [clean, clean, dirty, deleted] {
            counts.remove(status);
        }
        assert_eq!(counts, BufferStatusCounts::default());
    }
}
