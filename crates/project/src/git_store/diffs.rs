use std::ops::Range;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use buffer_diff::BufferDiff;
use buffer_diff::DiffHunkSecondaryStatus;
use buffer_diff::PendingSense;
use futures::FutureExt;
use futures::future;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use language::Anchor;
use language::Buffer;
use text::BufferId;
use text::OffsetRangeExt as _;
use text::ToOffset as _;

use super::DiffKind;
use super::GitStore;
use super::GitStoreEvent;
use super::Repository;
use super::buffer_git_state::BufferGitState;
use super::buffer_git_state::DiffBasesChange;
use super::buffer_git_state::GitDiffOperations;
use super::buffer_git_state::IndexTextFile;
use super::buffer_git_state::pending_hunks;
use super::conflict_set::ConflictSet;

impl GitStore {
    pub fn open_unstaged_diff(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<BufferDiff>>> {
        let buffer_id = buffer.read(cx).buffer_id();
        if let Some(diff_state) = self.diffs.get(&buffer_id)
            && let Some(unstaged_diff) = diff_state.read(cx).unstaged_diff.as_ref().and_then(WeakEntity::upgrade)
        {
            // If this unstaged diff was first opened as the uncommitted diff's
            // secondary, its index text wasn't highlighted. Enable it now and
            // recalc so the language gets applied to the deleted (base) side.
            diff_state.update(cx, |diff_state, cx| {
                if !diff_state.index_text_buffer_language_enabled {
                    diff_state.index_text_buffer_language_enabled = true;
                    let buffer_snapshot = buffer.read(cx).text_snapshot();
                    diff_state.recalculate_diffs(buffer_snapshot, cx);
                }
            });
            if let Some(task) = diff_state.update(cx, |diff_state, _| diff_state.wait_for_recalculation()) {
                return cx.background_executor().spawn(async move {
                    task.await;
                    Ok(unstaged_diff)
                });
            }
            return Task::ready(Ok(unstaged_diff));
        }

        let Some((repo, repo_path)) = self.repository_and_path_for_buffer_id(buffer.read(cx).buffer_id(), cx) else {
            return Task::ready(Err(anyhow!("failed to find git repository for buffer")));
        };

        let is_symlink = Self::buffer_is_symlink(&buffer, cx);
        let task = self
            .loading_diffs
            .entry((buffer_id, DiffKind::Unstaged))
            .or_insert_with(|| {
                let staged_text = if is_symlink {
                    Task::ready(Ok(None))
                } else {
                    repo.update(cx, |repo, cx| repo.load_staged_text(repo_path, cx))
                };
                cx.spawn(async move |this, cx| {
                    Self::open_diff_internal(
                        this,
                        DiffKind::Unstaged,
                        staged_text.await.map(DiffBasesChange::Index),
                        buffer,
                        cx,
                    )
                    .await
                    .map_err(Arc::new)
                })
                .shared()
            })
            .clone();

        cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) })
    }

    /// Opens the staged (HEAD-vs-index) diff for the given buffer, along with
    /// the index text buffer that is the diff's main buffer.
    pub fn open_staged_diff(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Entity<BufferDiff>, Entity<Buffer>)>> {
        let buffer_id = buffer.read(cx).buffer_id();

        if let Some(diff_state) = self.diffs.get(&buffer_id)
            && let Some(staged_diff) = diff_state.read(cx).staged_diff_and_index_text_buffer()
        {
            if let Some(task) = diff_state.update(cx, |diff_state, _| diff_state.wait_for_recalculation()) {
                return cx.background_executor().spawn(async move {
                    task.await;
                    Ok(staged_diff)
                });
            }
            return Task::ready(Ok(staged_diff));
        }

        let Some((repo, repo_path)) = self.repository_and_path_for_buffer_id(buffer.read(cx).buffer_id(), cx) else {
            return Task::ready(Err(anyhow!("failed to find git repository for buffer")));
        };

        let task = self
            .loading_diffs
            .entry((buffer_id, DiffKind::Staged))
            .or_insert_with(|| {
                let changes = repo.update(cx, |repo, cx| repo.load_committed_text(repo_path, cx));

                cx.spawn(async move |this, cx| {
                    Self::open_diff_internal(this, DiffKind::Staged, changes.await, buffer, cx).await.map_err(Arc::new)
                })
                .shared()
            })
            .clone();

        cx.spawn(async move |this, cx| {
            let diff = task.await.map_err(|e| anyhow!("{e}"))?;
            this.update(cx, |this, cx| {
                let index_text_buffer = this
                    .diffs
                    .get(&buffer_id)
                    .and_then(|diff_state| {
                        let (_, index_text_buffer) = diff_state.read(cx).staged_diff.as_ref()?;
                        Some(index_text_buffer.clone())
                    })
                    .context("index text buffer missing after opening staged diff")?;
                Ok((diff, index_text_buffer))
            })?
        })
    }

    /// Stages the worktree changes covered by `worktree_ranges`, acting on the
    /// given unstaged (index-vs-worktree) diff. Used by both the unstaged-changes
    /// view and the uncommitted (gutter) controls: "stage" means the same index
    /// change regardless of which view it was invoked from, so callers holding an
    /// uncommitted diff pass its unstaged secondary.
    ///
    /// Decomposes the worktree region into the unstaged hunks it covers, so no
    /// worktree->index projection is needed. Optimistically suppresses the staged
    /// hunks from the unstaged diff and, if the uncommitted diff happens to be
    /// open, marks the corresponding uncommitted hunks as staging.
    pub fn stage_hunks(
        &mut self,
        buffer: Entity<Buffer>,
        unstaged_diff: Entity<BufferDiff>,
        worktree_ranges: Vec<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if worktree_ranges.is_empty() {
            return Ok(());
        }
        let buffer_snapshot = buffer.read(cx).snapshot();
        let buffer_id = buffer_snapshot.buffer_id();
        let file_exists = buffer_snapshot.file().is_some_and(|file| file.disk_state().exists());

        let unstaged_snapshot = unstaged_diff.read(cx).snapshot(cx);

        // Decompose: the unstaged hunks the worktree region covers carry the
        // index range directly. Sorting by buffer offset also sorts by index
        // offset, since the hunks are non-overlapping. We read the raw hunks
        // (ignoring optimistic suppression) so that re-staging a hunk that was
        // optimistically staged and then unstaged still finds it. The footprints
        // let the patch drop earlier edits the region covers even where the raw
        // diff is empty (re-staging a hunk that is staged on disk but
        // optimistically unstaged).
        let mut unstaged_hunks = Vec::new();
        let mut index_footprints = Vec::new();
        for range in &worktree_ranges {
            unstaged_hunks.extend(unstaged_snapshot.raw_hunks_intersecting_range(range.clone(), &buffer_snapshot));
            index_footprints.push(unstaged_snapshot.base_text_range_for_buffer_range(range.clone(), &buffer_snapshot));
        }
        unstaged_hunks.sort_by_key(|hunk| hunk.buffer_range.start.to_offset(&buffer_snapshot));
        unstaged_hunks.dedup_by(|a, b| a.buffer_range.start == b.buffer_range.start);

        // The uncommitted hunks the region covers get the optimistic
        // "staging" secondary status (free cross-view update).
        let uncommitted_diff = self.get_uncommitted_diff(buffer_id, cx);
        let mut uncommitted_hunks = Vec::new();
        if let Some(uncommitted_diff) = &uncommitted_diff {
            let uncommitted_snapshot = uncommitted_diff.read(cx).snapshot(cx);
            for range in &worktree_ranges {
                uncommitted_hunks.extend(
                    uncommitted_snapshot
                        .hunks_intersecting_range(range.clone(), &buffer_snapshot)
                        .filter(|hunk| hunk.secondary_status != DiffHunkSecondaryStatus::NoSecondaryHunk),
                );
            }
            uncommitted_hunks.sort_by_key(|hunk| hunk.buffer_range.start.to_offset(&buffer_snapshot));
            uncommitted_hunks.dedup_by(|a, b| a.buffer_range.start == b.buffer_range.start);
        }

        let index_edits = file_exists.then(|| {
            unstaged_hunks
                .iter()
                .map(|hunk| {
                    let worktree_range = hunk.buffer_range.to_offset(&buffer_snapshot);
                    let replacement: Arc<str> =
                        Arc::from(buffer_snapshot.text_for_range(worktree_range).collect::<String>());
                    (hunk.diff_base_byte_range.clone(), replacement)
                })
                .collect::<Vec<_>>()
        });

        let version = buffer_snapshot.version().clone();
        let unstaged_pending = pending_hunks(&unstaged_hunks, &version, PendingSense::Suppress);
        let uncommitted_pending =
            pending_hunks(&uncommitted_hunks, &version, PendingSense::SetSecondaryStatus { stage: true });
        drop(unstaged_snapshot);

        let diff_state = self.diffs.get(&buffer_id).cloned().context("failed to find git state for buffer")?;
        diff_state.update(cx, |diff_state, _| {
            diff_state.remove_overlapping_pending_index_edits(&index_footprints);
            diff_state.insert_pending_index_edits(index_edits);
        });

        if let Some(uncommitted_diff) = uncommitted_diff {
            uncommitted_diff.update(cx, |diff, cx| {
                diff.set_pending_hunks(&uncommitted_pending, &buffer_snapshot, cx);
            });
        }
        unstaged_diff.update(cx, |diff, cx| {
            diff.set_pending_hunks(&unstaged_pending, &buffer_snapshot, cx);
        });

        self.write_optimistic_index(buffer_id, cx);
        Ok(())
    }

    /// Unstages the worktree changes covered by `worktree_ranges`, acting on the
    /// given uncommitted (HEAD-vs-worktree) diff, invoked from the uncommitted
    /// (gutter) controls. Uses the worktree->index projection (the hard part)
    /// because the acted-on hunks are HEAD-vs-worktree.
    pub fn unstage_uncommitted_hunks(
        &mut self,
        buffer: Entity<Buffer>,
        uncommitted_diff: Entity<BufferDiff>,
        worktree_ranges: Vec<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if worktree_ranges.is_empty() {
            return Ok(());
        }
        let buffer_snapshot = buffer.read(cx).snapshot();
        let buffer_id = buffer_snapshot.buffer_id();
        let file_exists = buffer_snapshot.file().is_some_and(|file| file.disk_state().exists());

        let uncommitted_snapshot = uncommitted_diff.read(cx).snapshot(cx);
        let unstaged_snapshot = uncommitted_snapshot.secondary_diff().context("diff has no unstaged secondary")?;

        let mut hunks = Vec::new();
        for range in &worktree_ranges {
            hunks.extend(uncommitted_snapshot.hunks_intersecting_range(range.clone(), &buffer_snapshot));
        }
        hunks.sort_by_key(|hunk| hunk.buffer_range.start.to_offset(&buffer_snapshot));
        hunks.dedup_by(|a, b| a.buffer_range.start == b.buffer_range.start);

        let (index_edits, pending) = uncommitted_snapshot.compute_uncommitted_index_edits(
            unstaged_snapshot,
            false,
            &hunks,
            &buffer_snapshot,
            file_exists,
        );
        drop(uncommitted_snapshot);

        let diff_state = self.diffs.get(&buffer_id).cloned().context("failed to find git state for buffer")?;
        diff_state.update(cx, |diff_state, _| diff_state.insert_pending_index_edits(index_edits));

        uncommitted_diff.update(cx, |diff, cx| {
            diff.set_pending_hunks(&pending, &buffer_snapshot, cx);
        });

        self.write_optimistic_index(buffer_id, cx);
        Ok(())
    }

    /// Unstages staged (HEAD-vs-index) hunks covered by `index_ranges` (in the
    /// index text buffer's coordinates), acting on the given staged diff,
    /// invoked from the staged-changes view. The acted-on hunks already carry
    /// an index range, so no projection is needed; optimistically suppresses
    /// them from the staged diff.
    pub fn unstage_staged_hunks(
        &mut self,
        staged_diff: Entity<BufferDiff>,
        index_ranges: Vec<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if index_ranges.is_empty() {
            return Ok(());
        }
        let index_buffer_id = staged_diff.read(cx).buffer_id;
        let buffer_id = *self
            .buffer_ids_by_index_text_buffer_id
            .get(&index_buffer_id)
            .context("failed to find git state for index text buffer")?;
        let diff_state = self.diffs.get(&buffer_id).cloned().context("failed to find git state for buffer")?;
        let index_buffer = diff_state.read(cx).index_text_buffer().context("index text is not loaded")?;
        let index_snapshot = index_buffer.read(cx).text_snapshot();
        let staged_snapshot = staged_diff.read(cx).snapshot(cx);

        let mut hunks = Vec::new();
        for range in &index_ranges {
            hunks.extend(staged_snapshot.hunks_intersecting_range(range.clone(), &index_snapshot));
        }
        hunks.sort_by_key(|hunk| hunk.buffer_range.start.to_offset(&index_snapshot));
        hunks.dedup_by(|a, b| a.buffer_range.start == b.buffer_range.start);

        let index_edits = staged_diff.read(cx).unstage_staged_hunks(&hunks, &index_snapshot);
        let version = index_snapshot.version().clone();
        let pending = pending_hunks(&hunks, &version, PendingSense::Suppress);
        drop(staged_snapshot);

        diff_state.update(cx, |diff_state, _| diff_state.insert_pending_index_edits(index_edits));
        staged_diff.update(cx, |diff, cx| {
            diff.set_pending_hunks(&pending, &index_snapshot, cx);
        });

        self.write_optimistic_index(buffer_id, cx);
        Ok(())
    }

    /// Derives the desired index text from the buffer's optimistic patch and
    /// schedules the write.
    fn write_optimistic_index(&self, buffer_id: BufferId, cx: &mut Context<Self>) {
        let Some(diff_state) = self.diffs.get(&buffer_id) else {
            return;
        };
        let new_index_text = diff_state.read(cx).pending_index_text(cx).map(|rope| rope.to_string());
        self.write_index_text_for_buffer_id(buffer_id, new_index_text, cx);
    }

    pub fn open_diff_since(
        &mut self,
        oid: Option<git::Oid>,
        buffer: Entity<Buffer>,
        repo: Entity<Repository>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<BufferDiff>>> {
        let buffer_id = buffer.read(cx).buffer_id();

        if let Some(diff_state) = self.diffs.get(&buffer_id)
            && let Some(oid_diff) = diff_state.read(cx).oid_diff(oid)
        {
            if let Some(task) = diff_state.update(cx, |diff_state, _| diff_state.wait_for_recalculation()) {
                return cx.background_executor().spawn(async move {
                    task.await;
                    Ok(oid_diff)
                });
            }
            return Task::ready(Ok(oid_diff));
        }

        let diff_kind = DiffKind::SinceOid(oid);
        if let Some(task) = self.loading_diffs.get(&(buffer_id, diff_kind)) {
            let task = task.clone();
            return cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) });
        }

        let task = cx
            .spawn(async move |this, cx| {
                let result: Result<Entity<BufferDiff>> = async {
                    let content: Option<Arc<str>> = match oid {
                        None => None,
                        Some(oid) => Some({
                            let mut content = repo.update(cx, |repo, cx| repo.load_blob_content(oid, cx)).await?;
                            text::LineEnding::normalize(&mut content);
                            content.into()
                        }),
                    };
                    let (buffer_snapshot, language_registry) =
                        buffer.read_with(cx, |buffer, _| (buffer.snapshot(), buffer.language_registry()));
                    let buffer_version = buffer_snapshot.version.clone();
                    let buffer_diff = cx.new(|cx| {
                        BufferDiff::new(&buffer_snapshot, buffer_snapshot.language().cloned(), language_registry, cx)
                    });

                    buffer_diff
                        .update(cx, |buffer_diff, cx| {
                            buffer_diff.set_base_text(content.clone(), buffer_snapshot.text, cx)
                        })
                        .await;
                    let unstaged_diff =
                        this.update(cx, |this, cx| this.open_unstaged_diff(buffer.clone(), cx))?.await?;
                    buffer_diff.update(cx, |buffer_diff, _| {
                        buffer_diff.set_secondary_diff(unstaged_diff);
                    });

                    let recalculation = this.update(cx, |this, cx| {
                        let git_store = cx.weak_entity();
                        let diff_state = this
                            .diffs
                            .entry(buffer_id)
                            .or_insert_with(|| cx.new(|cx| BufferGitState::new(git_store, cx)));

                        diff_state.update(cx, |state, cx| {
                            if let Some(oid) = oid
                                && let Some(content) = content
                            {
                                state.oid_texts.insert(oid, content);
                            }
                            state.oid_diffs.insert(oid, buffer_diff.downgrade());
                            let buffer = buffer.read(cx);
                            if buffer.version() != buffer_version {
                                state.recalculate_diffs(buffer.text_snapshot(), cx);
                            }
                            state.wait_for_recalculation()
                        })
                    })?;
                    if let Some(recalculation) = recalculation {
                        recalculation.await;
                    }

                    Ok(buffer_diff)
                }
                .await;
                // Remove the entry on failure too, so the next call retries the load.
                this.update(cx, |this, _| {
                    this.loading_diffs.remove(&(buffer_id, diff_kind));
                })
                .ok();
                result.map_err(Arc::new)
            })
            .shared();

        self.loading_diffs.insert((buffer_id, diff_kind), task.clone());
        cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) })
    }

    pub fn open_uncommitted_diff(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<BufferDiff>>> {
        let buffer_id = buffer.read(cx).buffer_id();

        if let Some(diff_state) = self.diffs.get(&buffer_id)
            && let Some(uncommitted_diff) = diff_state.read(cx).uncommitted_diff.as_ref().and_then(WeakEntity::upgrade)
        {
            if let Some(task) = diff_state.update(cx, |diff_state, _| diff_state.wait_for_recalculation()) {
                return cx.background_executor().spawn(async move {
                    task.await;
                    Ok(uncommitted_diff)
                });
            }
            return Task::ready(Ok(uncommitted_diff));
        }

        let Some((repo, repo_path)) = self.repository_and_path_for_buffer_id(buffer.read(cx).buffer_id(), cx) else {
            return Task::ready(Err(anyhow!("failed to find git repository for buffer")));
        };

        let is_symlink = Self::buffer_is_symlink(&buffer, cx);
        let task = self
            .loading_diffs
            .entry((buffer_id, DiffKind::Uncommitted))
            .or_insert_with(|| {
                let changes = if is_symlink {
                    Task::ready(Ok(DiffBasesChange::Both(None)))
                } else {
                    repo.update(cx, |repo, cx| repo.load_committed_text(repo_path, cx))
                };

                // todo(lw): hot foreground spawn
                cx.spawn(async move |this, cx| {
                    Self::open_diff_internal(this, DiffKind::Uncommitted, changes.await, buffer, cx)
                        .await
                        .map_err(Arc::new)
                })
                .shared()
            })
            .clone();

        cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) })
    }

    async fn open_diff_internal(
        this: WeakEntity<Self>,
        kind: DiffKind,
        texts: Result<DiffBasesChange>,
        buffer_entity: Entity<Buffer>,
        cx: &mut AsyncApp,
    ) -> Result<Entity<BufferDiff>> {
        let diff_bases_change = match texts {
            Err(e) => {
                this.update(cx, |this, cx| {
                    let buffer = buffer_entity.read(cx);
                    let buffer_id = buffer.buffer_id();
                    this.loading_diffs.remove(&(buffer_id, kind));
                })?;
                return Err(e);
            }
            Ok(change) => change,
        };

        this.update(cx, |this, cx| {
            let buffer = buffer_entity.read(cx);
            let buffer_id = buffer.buffer_id();
            let language = buffer.language().cloned();
            let language_registry = buffer.language_registry();
            let index_text_file =
                buffer.file().map(|file| Arc::new(IndexTextFile::new(file.as_ref(), cx)) as Arc<dyn language::File>);
            let text_snapshot = buffer.text_snapshot();
            this.loading_diffs.remove(&(buffer_id, kind));

            let git_store = cx.weak_entity();
            let project = this.project.clone();
            let diff_state =
                this.diffs.entry(buffer_id).or_insert_with(|| cx.new(|cx| BufferGitState::new(git_store.clone(), cx)));

            let existing_unstaged_diff = diff_state.read(cx).unstaged_diff();

            let mut staged_index_text_buffer = None;
            let diff = if kind == DiffKind::Unstaged
                && let Some(existing_unstaged_diff) = existing_unstaged_diff.clone()
            {
                existing_unstaged_diff
            } else {
                match kind {
                    DiffKind::Unstaged => {
                        let base_text_buffer = diff_state.update(cx, |diff_state, cx| {
                            diff_state.get_or_create_index_text_buffer(index_text_file.clone(), cx)
                        });
                        cx.new(|cx| {
                            let mut diff = BufferDiff::new_with_base_text_buffer(&text_snapshot, base_text_buffer, cx);
                            if let Some(project) = project.clone() {
                                diff.set_operations(Arc::new(GitDiffOperations { project, kind }));
                            }
                            diff
                        })
                    }
                    DiffKind::Staged => {
                        let (index_text_buffer, base_text_buffer) = diff_state.update(cx, |diff_state, cx| {
                            (
                                diff_state.get_or_create_index_text_buffer(index_text_file.clone(), cx),
                                diff_state.get_or_create_head_text_buffer(cx),
                            )
                        });
                        index_text_buffer.update(cx, |index_text_buffer, cx| {
                            if let Some(language_registry) = language_registry.clone() {
                                index_text_buffer.set_language_registry(language_registry);
                            }
                            index_text_buffer.set_language_async(language.clone(), cx);
                        });
                        let index_text_snapshot = index_text_buffer.read(cx).text_snapshot();
                        staged_index_text_buffer = Some(index_text_buffer);
                        cx.new(|cx| {
                            let mut diff =
                                BufferDiff::new_with_base_text_buffer(&index_text_snapshot, base_text_buffer, cx);
                            if let Some(project) = project.clone() {
                                diff.set_operations(Arc::new(GitDiffOperations { project, kind }));
                            }
                            diff
                        })
                    }
                    DiffKind::Uncommitted => {
                        let base_text_buffer =
                            diff_state.update(cx, |state, cx| state.get_or_create_head_text_buffer(cx));
                        cx.new(|cx| {
                            let mut diff = BufferDiff::new_with_base_text_buffer(&text_snapshot, base_text_buffer, cx);
                            if let Some(project) = project.clone() {
                                diff.set_operations(Arc::new(GitDiffOperations { project, kind }));
                            }
                            diff
                        })
                    }
                    DiffKind::SinceOid(_) => {
                        unreachable!("open_diff_internal is not used for OID diffs")
                    }
                }
            };
            let rx = diff_state.update(cx, |diff_state, cx| {
                diff_state.language = language;
                diff_state.language_registry = language_registry;

                match kind {
                    DiffKind::Unstaged => {
                        diff_state.unstaged_diff = Some(diff.downgrade());
                        // The deleted (base) side of a standalone unstaged diff
                        // is the index, so highlight it. The recalc kicked off by
                        // `diff_bases_changed` below applies the language.
                        diff_state.index_text_buffer_language_enabled = true;
                    }
                    DiffKind::Staged => {
                        diff_state.index_text_buffer_language_enabled = true;
                        let index_text_buffer = staged_index_text_buffer
                            .take()
                            .context("index text buffer was not created for staged diff")?;
                        diff_state.staged_diff = Some((diff.downgrade(), index_text_buffer));
                    }
                    DiffKind::Uncommitted => {
                        let unstaged_diff = if let Some(diff) = existing_unstaged_diff {
                            diff
                        } else {
                            let base_text_buffer = diff_state.get_or_create_index_text_buffer(index_text_file, cx);
                            let unstaged_diff = cx.new(|cx| {
                                let mut diff =
                                    BufferDiff::new_with_base_text_buffer(&text_snapshot, base_text_buffer, cx);
                                if let Some(project) = project.clone() {
                                    diff.set_operations(Arc::new(GitDiffOperations {
                                        project,
                                        kind: DiffKind::Unstaged,
                                    }));
                                }
                                diff
                            });
                            diff_state.unstaged_diff = Some(unstaged_diff.downgrade());
                            unstaged_diff
                        };

                        diff.update(cx, |diff, _| diff.set_secondary_diff(unstaged_diff));
                        diff_state.uncommitted_diff = Some(diff.downgrade());
                    }
                    DiffKind::SinceOid(_) => {
                        unreachable!("open_diff_internal is not used for OID diffs")
                    }
                }

                diff_state.diff_bases_changed(text_snapshot, Some(diff_bases_change), cx);
                let rx = diff_state.wait_for_recalculation();

                anyhow::Ok(async move {
                    if let Some(rx) = rx {
                        rx.await;
                    }
                    Ok(diff)
                })
            });
            let diff_state = this.diffs.get(&buffer_id).cloned();
            if let Some(index_text_buffer) = diff_state.and_then(|diff_state| diff_state.read(cx).index_text_buffer()) {
                this.buffer_ids_by_index_text_buffer_id.insert(index_text_buffer.read(cx).buffer_id(), buffer_id);
            }
            rx
        })??
        .await
    }

    pub fn get_uncommitted_diff(&self, buffer_id: BufferId, cx: &App) -> Option<Entity<BufferDiff>> {
        let diff_state = self.diffs.get(&buffer_id)?;
        diff_state.read(cx).uncommitted_diff.as_ref()?.upgrade()
    }

    pub fn open_conflict_set(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) -> Task<Entity<ConflictSet>> {
        let buffer_id = buffer.read(cx).buffer_id();

        if let Some(git_state) = self.diffs.get(&buffer_id)
            && let Some(conflict_set) = git_state.read(cx).conflict_set.as_ref().and_then(WeakEntity::upgrade)
        {
            let conflict_set = conflict_set;
            let buffer_snapshot = buffer.read(cx).text_snapshot();

            let rx = git_state.update(cx, |state, cx| state.reparse_conflict_markers(buffer_snapshot, cx));

            return cx.spawn(async move |_, _| {
                rx.await.ok();
                conflict_set
            });
        }

        let is_unmerged = self
            .repository_and_path_for_buffer_id(buffer_id, cx)
            .is_some_and(|(repo, path)| repo.read(cx).snapshot.has_conflict(&path));
        let git_store = cx.weak_entity();
        let buffer_git_state =
            self.diffs.entry(buffer_id).or_insert_with(|| cx.new(|cx| BufferGitState::new(git_store, cx)));
        let conflict_set = cx.new(|cx| ConflictSet::new(buffer_id, is_unmerged, cx));

        self._subscriptions.push(cx.subscribe(&conflict_set, |_, _, _, cx| {
            cx.emit(GitStoreEvent::ConflictsUpdated);
        }));

        let rx = buffer_git_state.update(cx, |state, cx| {
            state.conflict_set = Some(conflict_set.downgrade());
            let buffer_snapshot = buffer.read(cx).text_snapshot();
            state.reparse_conflict_markers(buffer_snapshot, cx)
        });

        cx.spawn(async move |_, _| {
            rx.await.ok();
            conflict_set
        })
    }

    pub fn recalculate_buffer_diffs(
        &mut self,
        buffers: Vec<Entity<Buffer>>,
        cx: &mut Context<Self>,
    ) -> impl Future<Output = ()> + use<> {
        let mut futures = Vec::new();
        for buffer in buffers {
            if let Some(diff_state) = self.diffs.get_mut(&buffer.read(cx).buffer_id()) {
                let buffer = buffer.read(cx).text_snapshot();
                diff_state.update(cx, |diff_state, cx| {
                    diff_state.recalculate_diffs(buffer.clone(), cx);
                    futures.extend(diff_state.wait_for_recalculation().map(FutureExt::boxed));
                });
                futures.push(
                    diff_state.update(cx, |diff_state, cx| {
                        diff_state.reparse_conflict_markers(buffer, cx).map(|_| {}).boxed()
                    }),
                );
            }
        }
        async move {
            future::join_all(futures).await;
        }
    }

    fn write_index_text_for_buffer_id(
        &self,
        buffer_id: BufferId,
        new_index_text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(diff_state) = self.diffs.get(&buffer_id) else {
            return;
        };
        let hunk_staging_operation_count = diff_state.update(cx, |diff_state, _| {
            diff_state.hunk_staging_operation_count += 1;
            diff_state.hunk_staging_operation_count
        });
        let Some((repo, path)) = self.repository_and_path_for_buffer_id(buffer_id, cx) else {
            return;
        };
        let (encoding, has_bom) =
            self.buffer_store.read(cx).get(buffer_id).map_or((encoding_rs::UTF_8, false), |buffer| {
                let buffer = buffer.read(cx);
                (buffer.encoding(), buffer.has_bom())
            });
        let recv = repo.update(cx, |repo, cx| {
            log::debug!("hunks changed for {}", path.as_unix_str());
            repo.spawn_set_index_text_job(
                path,
                new_index_text,
                encoding,
                has_bom,
                Some(hunk_staging_operation_count),
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(error)) = cx.background_spawn(recv).await {
                this.update(cx, |this, cx| {
                    if let Some(diff_state) = this.diffs.get(&buffer_id).cloned() {
                        diff_state.update(cx, |diff_state, cx| {
                            diff_state.clear_pending_index_edits_and_hunks(cx);
                        });
                    }
                    cx.emit(GitStoreEvent::IndexWriteError(error));
                })
                .ok();
            }
        })
        .detach();
    }
}
