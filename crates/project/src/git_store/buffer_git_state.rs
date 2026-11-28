use std::mem;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use async_macos::yield_now;
use buffer_diff::BufferDiff;
use buffer_diff::DiffOperations;
use futures::channel::oneshot;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use language::Anchor;
use language::Buffer;
use language::Capability;
use language::Language;
use language::LanguageRegistry;
use language::WorktreeId;
use rustc_hash::FxHashMap;
use text::Rope;
use util::ResultExt as _;
use util::rel_path::RelPath;

use super::DiffKind;
use super::GitStore;
use super::conflict_set::ConflictSet;
use crate::Project;

pub(super) struct BufferGitState {
    pub(super) unstaged_diff: Option<WeakEntity<BufferDiff>>,
    pub(super) staged_diff: Option<(WeakEntity<BufferDiff>, Entity<Buffer>)>,
    pub(super) uncommitted_diff: Option<WeakEntity<BufferDiff>>,
    pub(super) oid_diffs: FxHashMap<Option<git::Oid>, WeakEntity<BufferDiff>>,
    pub(super) conflict_set: Option<WeakEntity<ConflictSet>>,
    recalculate_diff_task: Option<Task<Result<()>>>,
    reparse_conflict_markers_task: Option<Task<Result<()>>>,
    pub(super) language: Option<Arc<Language>>,
    pub(super) language_registry: Option<Arc<LanguageRegistry>>,
    conflict_updated_futures: Vec<oneshot::Sender<()>>,
    recalculating: (watch::Sender<bool>, watch::Receiver<bool>),

    /// These operation counts are used to ensure that head and index text
    /// values read from the git repository are up-to-date with any hunk staging
    /// operations that have been performed on the BufferDiff.
    ///
    /// The operation count is incremented immediately when the user initiates a
    /// hunk stage/unstage operation. Then, upon finishing writing the new index
    /// text do disk, the `operation count as of write` is updated to reflect
    /// the operation count that prompted the write.
    pub(super) hunk_staging_operation_count: usize,
    pub(super) hunk_staging_operation_count_as_of_write: usize,

    pub(super) head_text: Option<Arc<str>>,
    pub(super) index_text: Option<Arc<str>>,
    /// The optimistic, in-flight index state: the sole input to the index write,
    /// expressed relative to the currently-loaded index text (`I0`). Never shown
    /// to any view (views use per-diff pending hunks). Cleared when a
    /// recalculation settles.
    ///
    /// `I0` is immutable for the lifetime of any pending edit (the index text
    /// buffer is only fast-forwarded when a recalculation settles, which clears
    /// this state in the same update), so byte offsets stay valid for the whole
    /// window.
    pending_index_edits: Option<Vec<(Range<usize>, Arc<str>)>>,
    pub(super) oid_texts: FxHashMap<git::Oid, Arc<str>>,
    head_text_buffer: WeakEntity<Buffer>,
    index_text_buffer: WeakEntity<Buffer>,
    pub(super) index_text_buffer_language_enabled: bool,
    /// Set once a diff against HEAD can show deleted lines; until then the
    /// HEAD text is kept unparsed.
    head_text_buffer_language_enabled: bool,
    head_changed: bool,
    index_changed: bool,
    language_changed: bool,
}

/// Whether a diff against the HEAD text can show deleted lines, which then need
/// syntax highlighting. When the buffer (and index) still match HEAD there are
/// no hunks, so parsing HEAD would only duplicate the buffer's own syntax tree.
fn head_text_shows_deletions(
    buffer_text: &Rope,
    head_text: Option<&str>,
    index_matches_head: bool,
    has_staged_diff: bool,
) -> bool {
    let Some(head_text) = head_text else {
        return false;
    };
    (has_staged_diff && !index_matches_head) || !buffer_text.chunks().equals_str(head_text)
}

#[derive(Clone, Debug)]
pub(super) enum DiffBasesChange {
    Index(Option<String>),
    Head(Option<String>),
    Each { index: Option<String>, head: Option<String> },
    Both(Option<String>),
}

pub(super) struct GitDiffOperations {
    pub(super) project: WeakEntity<Project>,
    pub(super) kind: DiffKind,
}

impl DiffOperations for GitDiffOperations {
    fn supports_staging(&self) -> bool {
        matches!(self.kind, DiffKind::Unstaged | DiffKind::Uncommitted)
    }

    fn supports_unstaging(&self) -> bool {
        matches!(self.kind, DiffKind::Staged | DiffKind::Uncommitted)
    }

    fn supports_restore(&self) -> bool {
        matches!(self.kind, DiffKind::Unstaged | DiffKind::Uncommitted)
    }

    fn stage(
        &self,
        diff: Entity<BufferDiff>,
        buffer: Option<Entity<Buffer>>,
        buffer_ranges: Vec<Range<Anchor>>,
        cx: &mut App,
    ) {
        let result = self.project.update(cx, |project, cx| match self.kind {
            DiffKind::Unstaged => {
                let buffer = buffer.context("unstaged diff has no worktree buffer")?;
                project.stage_hunks(buffer, diff, buffer_ranges, cx)
            }
            DiffKind::Uncommitted => {
                let buffer = buffer.context("uncommitted diff has no worktree buffer")?;
                let secondary_diff = diff.read(cx).secondary_diff().context("diff has no unstaged secondary")?;
                project.stage_hunks(buffer, secondary_diff, buffer_ranges, cx)
            }
            DiffKind::Staged | DiffKind::SinceOid(..) => Ok(()),
        });
        result.and_then(|result| result).log_err();
    }

    fn unstage(
        &self,
        diff: Entity<BufferDiff>,
        buffer: Option<Entity<Buffer>>,
        buffer_ranges: Vec<Range<Anchor>>,
        cx: &mut App,
    ) {
        let result = self.project.update(cx, |project, cx| match self.kind {
            DiffKind::Staged => project.unstage_staged_hunks(diff, buffer_ranges, cx),
            DiffKind::Uncommitted => {
                let buffer = buffer.context("uncommitted diff has no worktree buffer")?;
                project.unstage_uncommitted_hunks(buffer, diff, buffer_ranges, cx)
            }
            DiffKind::Unstaged | DiffKind::SinceOid(..) => Ok(()),
        });
        result.and_then(|result| result).log_err();
    }
}

pub(super) struct IndexTextFile {
    path: Arc<RelPath>,
    full_path: PathBuf,
    file_name: String,
    worktree_id: WorktreeId,
}

impl IndexTextFile {
    pub(super) fn new(file: &dyn language::File, cx: &App) -> Self {
        Self {
            path: Arc::clone(file.path()),
            full_path: file.full_path(cx),
            file_name: file.file_name(cx).to_owned(),
            worktree_id: file.worktree_id(cx),
        }
    }
}

impl language::File for IndexTextFile {
    fn as_local(&self) -> Option<&dyn language::LocalFile> {
        None
    }

    fn disk_state(&self) -> language::DiskState {
        language::DiskState::Historic { was_deleted: false }
    }

    fn path(&self) -> &Arc<RelPath> {
        &self.path
    }

    fn full_path(&self, _: &App) -> PathBuf {
        self.full_path.clone()
    }

    fn file_name<'a>(&'a self, _: &'a App) -> &'a str {
        &self.file_name
    }

    fn worktree_id(&self, _: &App) -> WorktreeId {
        self.worktree_id
    }
}

impl BufferGitState {
    pub(super) fn new(_git_store: WeakEntity<GitStore>, _cx: &mut Context<Self>) -> Self {
        Self {
            unstaged_diff: Option::default(),
            staged_diff: Option::default(),
            uncommitted_diff: Option::default(),
            oid_diffs: FxHashMap::default(),
            recalculate_diff_task: Option::default(),
            language: Option::default(),
            language_registry: Option::default(),
            recalculating: watch::channel(false),
            hunk_staging_operation_count: 0,
            hunk_staging_operation_count_as_of_write: 0,
            head_text: Option::default(),
            index_text: Option::default(),
            pending_index_edits: Some(Vec::new()),
            oid_texts: FxHashMap::default(),
            head_text_buffer: WeakEntity::new_invalid(),
            index_text_buffer: WeakEntity::new_invalid(),
            index_text_buffer_language_enabled: Default::default(),
            head_text_buffer_language_enabled: Default::default(),
            head_changed: Default::default(),
            index_changed: Default::default(),
            language_changed: Default::default(),
            conflict_updated_futures: Vec::default(),
            conflict_set: Option::default(),
            reparse_conflict_markers_task: Option::default(),
        }
    }

    pub(super) fn get_or_create_head_text_buffer(&mut self, cx: &mut Context<Self>) -> Entity<Buffer> {
        if let Some(buffer) = self.head_text_buffer.upgrade() {
            return buffer;
        }
        let head_text = self.head_text.clone();
        let buffer = cx.new(|cx| {
            let mut buffer = Buffer::local(head_text.as_deref().unwrap_or(""), cx);
            buffer.set_capability(Capability::ReadOnly, cx);
            buffer
        });
        self.head_text_buffer = buffer.downgrade();
        buffer
    }

    pub(super) fn index_text_buffer(&self) -> Option<Entity<Buffer>> {
        self.index_text_buffer.upgrade()
    }

    pub(super) fn get_or_create_index_text_buffer(
        &mut self,
        file: Option<Arc<dyn language::File>>,
        cx: &mut Context<Self>,
    ) -> Entity<Buffer> {
        if let Some(buffer) = self.index_text_buffer.upgrade() {
            if let Some(file) = file {
                buffer.update(cx, |buffer, cx| buffer.file_updated(file, cx));
            }
            return buffer;
        }
        let index_text = self.index_text.clone();
        let buffer = cx.new(|cx| {
            let mut buffer = Buffer::local(index_text.as_deref().unwrap_or(""), cx);
            if let Some(file) = file {
                buffer.file_updated(file, cx);
            }
            buffer.set_capability(Capability::ReadOnly, cx);
            buffer
        });
        self.index_text_buffer = buffer.downgrade();
        buffer
    }

    pub(super) fn buffer_language_changed(&mut self, buffer: Entity<Buffer>, cx: &Context<Self>) {
        self.language = buffer.read(cx).language().cloned();
        self.language_changed = true;
        self.recalculate_diffs(buffer.read(cx).text_snapshot(), cx);
    }

    pub(super) fn reparse_conflict_markers(
        &mut self,
        buffer: text::BufferSnapshot,
        cx: &Context<Self>,
    ) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();

        let Some(conflict_set) = self.conflict_set.as_ref().and_then(WeakEntity::upgrade) else {
            return rx;
        };

        let has_conflict = conflict_set.read_with(cx, |conflict_set, _| conflict_set.has_conflict);
        if !has_conflict {
            return rx;
        }

        let old_snapshot = conflict_set.read_with(cx, |conflict_set, _| conflict_set.snapshot());
        self.conflict_updated_futures.push(tx);
        self.reparse_conflict_markers_task = Some(cx.spawn(async move |this, cx| {
            let (snapshot, changed_range) = cx
                .background_spawn(async move {
                    let new_snapshot = ConflictSet::parse(&buffer);
                    let changed_range = old_snapshot.compare(&new_snapshot, &buffer);
                    (new_snapshot, changed_range)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(conflict_set) = &this.conflict_set {
                    conflict_set
                        .update(cx, |conflict_set, cx| {
                            conflict_set.set_snapshot(snapshot, changed_range, cx);
                        })
                        .ok();
                }
                let futures = mem::take(&mut this.conflict_updated_futures);
                for tx in futures {
                    tx.send(()).ok();
                }
            })
        }));

        rx
    }

    pub(super) fn unstaged_diff(&self) -> Option<Entity<BufferDiff>> {
        self.unstaged_diff.as_ref().and_then(WeakEntity::upgrade)
    }

    fn staged_diff(&self) -> Option<Entity<BufferDiff>> {
        let (set, _) = self.staged_diff.as_ref()?;
        set.upgrade()
    }

    pub(super) fn staged_diff_and_index_text_buffer(&self) -> Option<(Entity<BufferDiff>, Entity<Buffer>)> {
        let (diff, index_text_buffer) = self.staged_diff.as_ref()?;
        Some((diff.upgrade()?, index_text_buffer.clone()))
    }

    fn uncommitted_diff(&self) -> Option<Entity<BufferDiff>> {
        self.uncommitted_diff.as_ref().and_then(WeakEntity::upgrade)
    }

    pub(super) fn oid_diff(&self, oid: Option<git::Oid>) -> Option<Entity<BufferDiff>> {
        self.oid_diffs.get(&oid).and_then(WeakEntity::upgrade)
    }

    /// Whether the index text is known to match the committed text, without
    /// comparing their contents. Always true when both texts were set by a
    /// single `DiffBasesChange::Both`, which shares one allocation between
    /// them. May be false even when the contents are equal, if the texts were
    /// loaded separately.
    fn index_matches_head(&self) -> bool {
        match (self.index_text.as_ref(), self.head_text.as_ref()) {
            (Some(index), Some(head)) => Arc::ptr_eq(index, head),
            (None, None) => true,
            _ => false,
        }
    }

    pub(super) fn remove_overlapping_pending_index_edits(&mut self, ranges: &[Range<usize>]) {
        if let Some(edits) = &mut self.pending_index_edits {
            edits.retain(|(existing, _)| {
                ranges.iter().all(|footprint| existing.end < footprint.start || footprint.end < existing.start)
            });
        }
    }

    pub(super) fn insert_pending_index_edits(&mut self, edits: Option<Vec<(Range<usize>, Arc<str>)>>) {
        match edits {
            None => {
                self.pending_index_edits = None;
            }
            Some(new_edits) => {
                let mut edits = self.pending_index_edits.take().unwrap_or_default();
                for (range, replacement) in new_edits {
                    edits.retain(|(existing, _)| existing.end < range.start || range.end < existing.start);
                    let position = edits.partition_point(|(existing, _)| existing.start < range.start);
                    edits.insert(position, (range, replacement));
                }
                self.pending_index_edits = Some(edits);
            }
        }
    }

    pub(super) fn pending_index_text(&self, cx: &App) -> Option<Rope> {
        let index_text_buffer = self.index_text_buffer.upgrade()?;
        let edits = self.pending_index_edits.as_ref()?;
        let mut index_text = index_text_buffer.read(cx).text_snapshot().as_rope().clone();
        for (old_range, replacement_text) in edits.iter().rev() {
            index_text.replace(old_range.clone(), replacement_text);
        }
        Some(index_text)
    }

    fn clear_pending_index_edits(&mut self) {
        self.pending_index_edits = Some(Vec::new());
    }

    pub(super) fn clear_pending_hunks(&self, cx: &mut Context<Self>) {
        for diff in [self.uncommitted_diff(), self.unstaged_diff(), self.staged_diff()].into_iter().flatten() {
            diff.update(cx, BufferDiff::clear_pending_hunks);
        }
    }

    pub(super) fn mark_whole_file_stage_or_unstage_pending(
        &self,
        stage: bool,
        buffer_snapshot: &text::BufferSnapshot,
        cx: &mut Context<Self>,
    ) {
        if let Some(uncommitted_diff) = self.uncommitted_diff() {
            uncommitted_diff.update(cx, |uncommitted_diff, cx| {
                uncommitted_diff.mark_all_hunks_pending(stage, buffer_snapshot, cx);
            });
        }

        if stage {
            if let Some(unstaged_diff) = self.unstaged_diff() {
                unstaged_diff.update(cx, |unstaged_diff, cx| {
                    unstaged_diff.suppress_all_hunks_pending(buffer_snapshot, cx);
                });
            }
        } else if let Some(staged_diff) = self.staged_diff()
            && let Some(index_text_buffer) = self.index_text_buffer()
        {
            let index_snapshot = index_text_buffer.read(cx).text_snapshot();
            staged_diff.update(cx, |staged_diff, cx| {
                staged_diff.suppress_all_hunks_pending(&index_snapshot, cx);
            });
        }
    }

    pub(super) fn clear_pending_index_edits_and_hunks(&mut self, cx: &mut Context<Self>) {
        self.clear_pending_index_edits();
        self.clear_pending_hunks(cx);
    }

    pub(super) fn wait_for_recalculation(&mut self) -> Option<impl Future<Output = ()> + use<>> {
        (*watch::Receiver::borrow(&mut self.recalculating.1)).then(|| {
            let mut rx = self.recalculating.0.receiver();
            async move { while rx.changed().await.is_ok() && *watch::Receiver::borrow(&mut rx) {} }
        })
    }

    pub(super) fn diff_bases_changed(
        &mut self,
        buffer: text::BufferSnapshot,
        diff_bases_change: Option<DiffBasesChange>,
        cx: &Context<Self>,
    ) {
        match diff_bases_change {
            Some(DiffBasesChange::Index(index)) => {
                self.index_text = index.map(|mut index| {
                    text::LineEnding::normalize(&mut index);
                    Arc::from(index.as_str())
                });
                self.index_changed = true;
            }
            Some(DiffBasesChange::Head(head)) => {
                self.head_text = head.map(|mut head| {
                    text::LineEnding::normalize(&mut head);
                    Arc::from(head.as_str())
                });
                self.head_changed = true;
            }
            Some(DiffBasesChange::Both(text)) => {
                let text = text.map(|mut text| {
                    text::LineEnding::normalize(&mut text);
                    Arc::from(text.as_str())
                });
                self.head_text = text.clone();
                self.index_text = text;
                self.head_changed = true;
                self.index_changed = true;
            }
            Some(DiffBasesChange::Each { index, head }) => {
                self.index_text = index.map(|mut index| {
                    text::LineEnding::normalize(&mut index);
                    Arc::from(index.as_str())
                });
                self.index_changed = true;
                self.head_text = head.map(|mut head| {
                    text::LineEnding::normalize(&mut head);
                    Arc::from(head.as_str())
                });
                self.head_changed = true;
            }
            None => {}
        }

        self.recalculate_diffs(buffer, cx);
    }

    pub(super) fn recalculate_diffs(&mut self, buffer: text::BufferSnapshot, cx: &Context<Self>) {
        self.recalculating.0.send(true).ok();

        let language = self.language.clone();
        let language_registry = self.language_registry.clone();
        let unstaged_diff = self.unstaged_diff();
        let staged_diff = self.staged_diff();
        let uncommitted_diff = self.uncommitted_diff();
        let head = self.head_text.clone();
        let index = self.index_text.clone();
        let head_text_buffer = self.head_text_buffer.upgrade();
        let index_text_buffer = self.index_text_buffer.upgrade();
        let index_text_buffer_language_enabled = self.index_text_buffer_language_enabled;
        let head_text_buffer_language_enabled = self.head_text_buffer_language_enabled;
        let index_changed = self.index_changed;
        let head_changed = self.head_changed;
        let language_changed = self.language_changed;
        let prev_hunk_staging_operation_count = self.hunk_staging_operation_count_as_of_write;
        let index_matches_head = self.index_matches_head();

        #[allow(clippy::type_complexity)]
        let oid_diffs: Vec<(Option<git::Oid>, Entity<BufferDiff>, Entity<Buffer>, Option<Arc<str>>)> = self
            .oid_diffs
            .iter()
            .filter_map(|(oid, weak)| {
                let diff = weak.upgrade()?;
                let base_text_buffer = diff.read(cx).base_text_buffer().clone();
                let base_text = match oid {
                    Some(oid) => Some(Arc::clone(self.oid_texts.get(oid)?)),
                    None => None,
                };
                Some((*oid, diff, base_text_buffer, base_text))
            })
            .collect();

        self.oid_diffs.retain(|oid, weak| {
            let alive = weak.upgrade().is_some();
            if !alive && let Some(oid) = oid {
                self.oid_texts.remove(oid);
            }
            alive
        });
        if self.staged_diff.as_ref().is_some_and(|(weak, _)| !weak.is_upgradable()) {
            self.staged_diff = None;
        }
        self.recalculate_diff_task = Some(cx.spawn(async move |this, cx| {
            log::debug!("start recalculating diffs for buffer {}", buffer.buffer_id());

            if index_text_buffer_language_enabled && let Some(index_text_buffer) = &index_text_buffer {
                index_text_buffer.update(cx, |index_text_buffer, cx| {
                    if let Some(language_registry) = language_registry.clone() {
                        index_text_buffer.set_language_registry(language_registry);
                    }
                    index_text_buffer.set_language_async(language.clone(), cx);
                });
            }
            let enable_head_text_language = if !head_text_buffer_language_enabled
                && head_text_buffer.is_some()
                && (staged_diff.is_some() || uncommitted_diff.is_some())
            {
                let buffer_text = buffer.as_rope().clone();
                let head_text = head.clone();
                let has_staged_diff = staged_diff.is_some();
                cx.background_spawn(async move {
                    head_text_shows_deletions(&buffer_text, head_text.as_deref(), index_matches_head, has_staged_diff)
                })
                .await
            } else {
                false
            };
            if enable_head_text_language {
                this.update(cx, |this, _| this.head_text_buffer_language_enabled = true)?;
            }
            if (head_text_buffer_language_enabled || enable_head_text_language)
                && let Some(head_text_buffer) = &head_text_buffer
            {
                head_text_buffer.update(cx, |head_text_buffer, cx| {
                    if let Some(language_registry) = language_registry.clone() {
                        head_text_buffer.set_language_registry(language_registry);
                    }
                    head_text_buffer.set_language_async(language.clone(), cx);
                });
            }

            for (_, _, base_text_buffer, _) in &oid_diffs {
                base_text_buffer.update(cx, |base_text_buffer, cx| {
                    if let Some(language_registry) = language_registry.clone() {
                        base_text_buffer.set_language_registry(language_registry);
                    }
                    base_text_buffer.set_language_async(language.clone(), cx);
                });
            }

            let mut edited_index_text = None;

            let index_text_snapshot = if let Some(index_text_buffer) = &index_text_buffer
                && (unstaged_diff.is_some() || staged_diff.is_some())
            {
                let index_text_snapshot = if index_changed || language_changed {
                    let new_index_text = index.clone().unwrap_or_default();
                    let index_text_diff = index_text_buffer
                        .update(cx, |index_text_buffer, cx| index_text_buffer.diff(Arc::clone(&new_index_text), cx))
                        .await;
                    let edited = index_text_buffer
                        .update(cx, |index_text_buffer, cx| {
                            index_text_buffer.snapshot_with_edits(index_text_diff.edits, cx)
                        })
                        .await;
                    let snapshot = edited.snapshot().clone();
                    edited_index_text = Some(edited);
                    snapshot
                } else {
                    index_text_buffer.read_with(cx, |buffer, _| buffer.snapshot())
                };
                Some(index_text_snapshot)
            } else {
                None
            };

            let new_unstaged_diff = if let (Some(unstaged_diff), Some(index_text_snapshot)) =
                (unstaged_diff.as_ref(), index_text_snapshot.as_ref())
            {
                Some(
                    cx.update(|cx| {
                        unstaged_diff.read(cx).update_diff(buffer.clone(), index_text_snapshot, index.clone(), cx)
                    })
                    .await,
                )
            } else {
                None
            };

            // Dropping BufferDiff can be expensive, so yield back to the event loop
            // for a bit
            yield_now().await;

            let mut edited_head_text = None;
            let mut new_staged_diff = None;
            let mut new_uncommitted_diff = None;
            if let Some(head_text_buffer) = &head_text_buffer
                && (staged_diff.is_some() || uncommitted_diff.is_some())
            {
                let head_base_text_exists = head.is_some();
                let head_text_snapshot = if head_changed || language_changed || enable_head_text_language {
                    let new_head_text = head.clone().unwrap_or_default();
                    let head_text_diff = head_text_buffer
                        .update(cx, |head_text_buffer, cx| head_text_buffer.diff(Arc::clone(&new_head_text), cx))
                        .await;
                    let edited = head_text_buffer
                        .update(cx, |base_text_buffer, cx| {
                            base_text_buffer.snapshot_with_edits(head_text_diff.edits, cx)
                        })
                        .await;
                    let snapshot = edited.snapshot().clone();
                    edited_head_text = Some(edited);
                    snapshot
                } else {
                    head_text_buffer.read_with(cx, |buffer, _| buffer.snapshot())
                };
                if let (Some(staged_diff), Some(index_base_text_snapshot)) =
                    (staged_diff.as_ref(), index_text_snapshot.as_ref())
                {
                    new_staged_diff = Some(
                        cx.update(|cx| {
                            staged_diff.read(cx).update_diff(
                                index_base_text_snapshot.text.clone(),
                                &head_text_snapshot,
                                head.clone(),
                                cx,
                            )
                        })
                        .await,
                    );
                }

                if let Some(uncommitted_diff) = &uncommitted_diff {
                    new_uncommitted_diff = if index_matches_head {
                        new_unstaged_diff.clone().map(|mut update| {
                            update.set_base_text_snapshot(head_text_snapshot.clone(), head_base_text_exists);
                            update
                        })
                    } else {
                        None
                    };
                    if new_uncommitted_diff.is_none() {
                        new_uncommitted_diff = Some(
                            cx.update(|cx| {
                                uncommitted_diff.read(cx).update_diff(
                                    buffer.clone(),
                                    &head_text_snapshot,
                                    head.clone(),
                                    cx,
                                )
                            })
                            .await,
                        );
                    }
                }
            }

            // Dropping BufferDiff can be expensive, so yield back to the event loop
            // for a bit
            yield_now().await;

            for (oid, oid_diff, base_text_buffer, base_text) in oid_diffs {
                let base_text_snapshot = base_text_buffer.read_with(cx, |buffer, _| buffer.snapshot());
                let new_oid_diff = cx
                    .update(|cx| {
                        oid_diff.read(cx).update_diff(buffer.clone(), &base_text_snapshot, base_text.clone(), cx)
                    })
                    .await;

                oid_diff.update(cx, |diff, cx| diff.set_snapshot(new_oid_diff, cx));

                log::debug!("finished recalculating oid diff for buffer {} oid {:?}", buffer.buffer_id(), oid);

                yield_now().await;
            }

            let cancel = this.update(cx, |this, _| {
                // This checks whether all pending stage/unstage operations
                // have quiesced (i.e. both the corresponding write and the
                // read of that write have completed). If not, then we cancel
                // this recalculation attempt to avoid invalidating pending
                // state too quickly; another recalculation will come along
                // later and clear the pending state once the state of the index has settled.
                if this.hunk_staging_operation_count > prev_hunk_staging_operation_count {
                    this.recalculating.0.send(false).ok();
                    true
                } else {
                    false
                }
            })?;
            if cancel {
                log::debug!(
                    concat!("aborting recalculating diffs for buffer {}", "due to subsequent hunk operations",),
                    buffer.buffer_id()
                );
                return Ok(());
            }

            this.update(cx, |this, cx| {
                this.clear_pending_index_edits();

                if let (Some(staged_diff), Some(new_staged_diff)) = (staged_diff.as_ref(), new_staged_diff.clone()) {
                    staged_diff.update(cx, |diff, cx| {
                        if let Some(edited_base_text) = edited_index_text.take()
                            && let Some(index_text_buffer) = &index_text_buffer
                        {
                            index_text_buffer.update(cx, |index_text_buffer, cx| {
                                index_text_buffer.fast_forward(edited_base_text, cx);
                            });
                        }
                        if let Some(edited_head_text) = edited_head_text.take()
                            && let Some(head_text_buffer) = &head_text_buffer
                        {
                            head_text_buffer
                                .update(cx, |head_text_buffer, cx| head_text_buffer.fast_forward(edited_head_text, cx));
                        }
                        diff.set_snapshot_with_secondary(new_staged_diff, None, true, cx)
                    });
                }

                let unstaged_changed_range = if let (Some(unstaged_diff), Some(new_unstaged_diff)) =
                    (unstaged_diff.as_ref(), new_unstaged_diff.clone())
                {
                    Some(unstaged_diff.update(cx, |diff, cx| {
                        if let Some(edited_index_text) = edited_index_text.take()
                            && let Some(index_text_buffer) = &index_text_buffer
                        {
                            index_text_buffer.update(cx, |index_text_buffer, cx| {
                                index_text_buffer.fast_forward(edited_index_text, cx);
                            });
                        }
                        diff.set_snapshot_with_secondary(new_unstaged_diff, None, true, cx)
                    }))
                } else {
                    None
                };

                if let (Some(uncommitted_diff), Some(new_uncommitted_diff)) =
                    (uncommitted_diff.as_ref(), new_uncommitted_diff.clone())
                {
                    uncommitted_diff.update(cx, |diff, cx| {
                        if let Some(edited_base_text) = edited_head_text.take()
                            && let Some(head_text_buffer) = &head_text_buffer
                        {
                            head_text_buffer
                                .update(cx, |head_text_buffer, cx| head_text_buffer.fast_forward(edited_base_text, cx));
                        }
                        diff.set_snapshot_with_secondary(
                            new_uncommitted_diff,
                            unstaged_changed_range.flatten(),
                            true,
                            cx,
                        )
                    });
                }
            })?;

            yield_now().await;

            log::debug!("finished recalculating diffs for buffer {}", buffer.buffer_id());

            if let Some(this) = this.upgrade() {
                this.update(cx, |this, _| {
                    this.index_changed = false;
                    this.head_changed = false;
                    this.language_changed = false;
                    this.recalculating.0.send(false).ok();
                });
            }

            Ok(())
        }));
    }
}

#[cfg(test)]
mod head_text_language_tests {
    use text::Rope;

    use super::head_text_shows_deletions;

    #[test]
    fn head_text_shows_deletions_works() {
        let rope = Rope::from("fn main() {}\n");
        assert!(!head_text_shows_deletions(&rope, Some("fn main() {}\n"), true, false));
        assert!(!head_text_shows_deletions(&rope, Some("fn main() {}\n"), true, true));
        assert!(head_text_shows_deletions(&rope, Some("fn other() {}\n"), true, false));
        assert!(head_text_shows_deletions(&rope, Some("fn main() {}"), true, false));
        // A staged change against HEAD shows deletions even when the working copy matches HEAD.
        assert!(head_text_shows_deletions(&rope, Some("fn main() {}\n"), false, true));
        // Without a staged diff, index-vs-HEAD differences are never shown against HEAD.
        assert!(!head_text_shows_deletions(&rope, Some("fn main() {}\n"), false, false));
        // A file new since HEAD has no base text to highlight.
        assert!(!head_text_shows_deletions(&rope, None, true, false));
    }

    #[test]
    fn head_text_shows_deletions_across_chunks() {
        let text = "let x = 1;\n".repeat(1000);
        let rope = Rope::from(text.as_str());
        assert!(!head_text_shows_deletions(&rope, Some(&text), true, false));
        let mut changed = text;
        changed.replace_range(5000..5001, "y");
        assert!(head_text_shows_deletions(&rope, Some(&changed), true, false));
    }
}
