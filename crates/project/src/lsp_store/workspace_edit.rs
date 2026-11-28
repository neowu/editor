use std::ops::Range;
use std::sync::Arc;
use std::vec;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use language::Bias;
use language::Buffer;
use language::CachedLspAdapter;
use language::PointUtf16;
use language::ToOffset as _;
use language::Transaction;
use language::Unclipped;
use language::range_from_lsp;
use lsp::LanguageServer;
use lsp::LanguageServerId;
use lsp_types::Edit;
use lsp_types::TextEdit;
use snippet::Snippet;
use text::Anchor;
use text::LineEnding;
use util::ResultExt as _;

use super::LocalLspStore;
use super::LspStore;
use super::LspStoreEvent;
use crate::ProjectItem as _;
use crate::ProjectTransaction;

impl LocalLspStore {
    pub(crate) async fn deserialize_text_edits(
        this: Entity<LspStore>,
        buffer_to_edit: Entity<Buffer>,
        edits: Vec<TextEdit>,
        push_to_history: bool,
        _: Arc<CachedLspAdapter>,
        language_server: Arc<LanguageServer>,
        cx: &mut AsyncApp,
    ) -> Result<Option<Transaction>> {
        let edits = this
            .update(cx, |this, cx| {
                this.local.edits_from_lsp(&buffer_to_edit, edits, language_server.server_id(), None, cx)
            })
            .await?;

        let transaction = buffer_to_edit.update(cx, |buffer, cx| {
            buffer.finalize_last_transaction();
            buffer.start_transaction();
            for (range, text) in edits {
                buffer.edit([(range, text)], None, cx);
            }

            buffer.end_transaction(cx).is_some().then(|| {
                let transaction = buffer.finalize_last_transaction().expect("transaction was just ended").clone();
                if !push_to_history {
                    buffer.forget_transaction(transaction.id);
                }
                transaction
            })
        });

        Ok(transaction)
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn edits_from_lsp(
        &mut self,
        buffer: &Entity<Buffer>,
        lsp_edits: impl 'static + Send + IntoIterator<Item = TextEdit>,
        server_id: LanguageServerId,
        version: Option<i32>,
        cx: &Context<LspStore>,
    ) -> Task<Result<Vec<(Range<Anchor>, Arc<str>)>>> {
        let snapshot = self.buffer_snapshot_for_lsp_version(buffer, server_id, version, cx);
        cx.background_spawn(async move {
            let snapshot = snapshot?;
            let mut lsp_edits = lsp_edits
                .into_iter()
                .map(|mut edit| {
                    LineEnding::normalize(&mut edit.new_text);
                    (range_from_lsp(edit.range), edit.new_text)
                })
                .collect::<Vec<_>>();

            lsp_edits.sort_by_key(|(range, _)| (range.start, range.end));

            let mut lsp_edits = lsp_edits.into_iter().peekable();
            let mut edits = Vec::new();
            while let Some((range, mut new_text)) = lsp_edits.next() {
                // Clip invalid ranges provided by the language server.
                let mut range = snapshot.clip_point_utf16(range.start, Bias::Left)
                    ..snapshot.clip_point_utf16(range.end, Bias::Left);

                // Combine any LSP edits that are adjacent.
                //
                // Also, combine LSP edits that are separated from each other by only
                // a newline. This is important because for some code actions,
                // Rust-analyzer rewrites the entire buffer via a series of edits that
                // are separated by unchanged newline characters.
                //
                // In order for the diffing logic below to work properly, any edits that
                // cancel each other out must be combined into one.
                while let Some((next_range, next_text)) = lsp_edits.peek() {
                    if next_range.start.0 > range.end {
                        if next_range.start.0.row > range.end.row + 1
                            || next_range.start.0.column > 0
                            || snapshot
                                .clip_point_utf16(Unclipped(PointUtf16::new(range.end.row, u32::MAX)), Bias::Left)
                                > range.end
                        {
                            break;
                        }
                        new_text.push('\n');
                    }
                    range.end = snapshot.clip_point_utf16(next_range.end, Bias::Left);
                    new_text.push_str(next_text);
                    lsp_edits.next();
                }

                // For multiline edits, perform a diff of the old and new text so that
                // we can identify the changes more precisely, preserving the locations
                // of any anchors positioned in the unchanged regions.
                if range.end.row > range.start.row {
                    let offset = range.start.to_offset(&snapshot);
                    let old_text = snapshot.text_for_range(range).collect::<String>();
                    let range_edits = language::text_diff(old_text.as_str(), &new_text);
                    edits.extend(range_edits.into_iter().map(|(range, replacement)| {
                        (
                            snapshot.anchor_after(offset + range.start)..snapshot.anchor_before(offset + range.end),
                            replacement,
                        )
                    }));
                } else if range.end == range.start {
                    let anchor = snapshot.anchor_after(range.start);
                    edits.push((anchor..anchor, new_text.into()));
                } else {
                    let edit_start = snapshot.anchor_after(range.start);
                    let edit_end = snapshot.anchor_before(range.end);
                    edits.push((edit_start..edit_end, new_text.into()));
                }
            }

            Ok(edits)
        })
    }

    pub(crate) async fn deserialize_workspace_edit(
        this: Entity<LspStore>,
        edit: lsp_types::WorkspaceEdit,
        push_to_history: bool,
        language_server: Arc<LanguageServer>,
        cx: &mut AsyncApp,
    ) -> Result<ProjectTransaction> {
        let fs = this.read_with(cx, |this, _| Arc::clone(&this.local.fs));

        let mut operations = Vec::new();
        if let Some(document_changes) = edit.document_changes {
            match document_changes {
                lsp_types::DocumentChanges::Edits(edits) => {
                    operations.extend(edits.into_iter().map(lsp_types::DocumentChangeOperation::Edit));
                }
                lsp_types::DocumentChanges::Operations(ops) => operations = ops,
            }
        } else if let Some(changes) = edit.changes {
            operations.extend(changes.into_iter().map(|(uri, edits)| {
                lsp_types::DocumentChangeOperation::Edit(lsp_types::TextDocumentEdit {
                    text_document: lsp_types::OptionalVersionedTextDocumentIdentifier { uri, version: None },
                    edits: edits.into_iter().map(Edit::Plain).collect(),
                })
            }));
        }

        let mut project_transaction = ProjectTransaction::default();
        for operation in operations {
            match operation {
                lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Create(op)) => {
                    let abs_path = op.uri.to_file_path().map_err(|()| anyhow!("can't convert URI to path"))?;

                    if let Some(parent_path) = abs_path.parent() {
                        fs.create_dir(parent_path).await?;
                    }
                    if abs_path.ends_with("/") {
                        fs.create_dir(&abs_path).await?;
                    } else {
                        fs.create_file(
                            &abs_path,
                            op.options.map_or_default(|options| fs::CreateOptions {
                                overwrite: options.overwrite.unwrap_or(false),
                                ignore_if_exists: options.ignore_if_exists.unwrap_or(false),
                            }),
                        )
                        .await?;
                    }
                }

                lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Rename(op)) => {
                    let source_abs_path =
                        op.old_uri.to_file_path().map_err(|()| anyhow!("can't convert URI to path"))?;
                    let target_abs_path =
                        op.new_uri.to_file_path().map_err(|()| anyhow!("can't convert URI to path"))?;

                    // An LSP "rename symbol" can also rename the file, with the text edit
                    // applied only to the in-memory buffer. Persist it before renaming, or
                    // fs.rename moves the stale on-disk content and the files' contents swap.
                    let dirty_buffer = this.update(cx, |this, cx| {
                        let project_path =
                            this.worktree_store().read(cx).project_path_for_absolute_path(&source_abs_path, cx)?;
                        let buffer = this.buffer_store().read(cx).get_by_path(&project_path)?;
                        buffer.read(cx).is_dirty().then_some(buffer)
                    });
                    if let Some(buffer) = dirty_buffer {
                        this.update(cx, |this, cx| {
                            this.buffer_store().update(cx, |buffer_store, cx| buffer_store.save_buffer(buffer, cx))
                        })
                        .await?;
                    }

                    let options = fs::RenameOptions {
                        overwrite: op.options.as_ref().and_then(|options| options.overwrite).unwrap_or(false),
                        ignore_if_exists: op
                            .options
                            .as_ref()
                            .and_then(|options| options.ignore_if_exists)
                            .unwrap_or(false),
                        create_parents: true,
                    };

                    fs.rename(&source_abs_path, &target_abs_path, options).await?;

                    // Preserve the entry id across the rename so an open buffer follows it
                    // to the new path, instead of being stranded at the old path when the
                    // filesystem watcher reports the deletion before the creation. Only a
                    // rename within one worktree can do this; anything else falls back to
                    // the watcher.
                    let refresh = this.update(cx, |this, cx| {
                        let (source_worktree, source_rel_path) =
                            this.worktree_store().read(cx).find_worktree(&source_abs_path, cx)?;
                        let (target_worktree, target_rel_path) =
                            this.worktree_store().read(cx).find_worktree(&target_abs_path, cx)?;
                        if source_worktree != target_worktree {
                            return None;
                        }
                        target_worktree.update(cx, |worktree, cx| {
                            Some(worktree.refresh_entry(target_rel_path, Some(source_rel_path), cx))
                        })
                    });
                    if let Some(refresh) = refresh {
                        refresh.await?;
                    }
                }

                lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Delete(op)) => {
                    let abs_path = op.uri.to_file_path().map_err(|()| anyhow!("can't convert URI to path"))?;
                    let options = op.options.map_or_default(|options| fs::RemoveOptions {
                        recursive: options.recursive.unwrap_or(false),
                        ignore_if_not_exists: options.ignore_if_not_exists.unwrap_or(false),
                    });
                    if abs_path.ends_with("/") {
                        fs.remove_dir(&abs_path, options).await?;
                    } else {
                        fs.remove_file(&abs_path, options).await?;
                    }
                }

                lsp_types::DocumentChangeOperation::Edit(op) => {
                    let buffer_to_edit = this
                        .update(cx, |this, cx| {
                            this.open_local_buffer_via_lsp(
                                op.text_document.uri.clone(),
                                language_server.server_id(),
                                cx,
                            )
                        })
                        .await?;

                    let edits = this
                        .update(cx, |this, cx| {
                            let path = buffer_to_edit.read(cx).project_path(cx);
                            let active_entry = this.active_entry;
                            let is_active_entry = path.is_some_and(|project_path| {
                                this.worktree_store
                                    .read(cx)
                                    .entry_for_path(&project_path, cx)
                                    .is_some_and(|entry| Some(entry.id) == active_entry)
                            });
                            let local = &mut this.local;

                            let (mut edits, mut snippet_edits) = (vec![], vec![]);
                            for edit in op.edits {
                                match edit {
                                    Edit::Plain(edit) => {
                                        if !edits.contains(&edit) {
                                            edits.push(edit);
                                        }
                                    }
                                    Edit::Annotated(edit) => {
                                        if !edits.contains(&edit.text_edit) {
                                            edits.push(edit.text_edit);
                                        }
                                    }
                                    Edit::Snippet(edit) => {
                                        let Ok(snippet) = Snippet::parse(&edit.snippet.value) else {
                                            continue;
                                        };

                                        if is_active_entry {
                                            snippet_edits.push((edit.range, snippet));
                                        } else {
                                            // Since this buffer is not focused, apply a normal edit.
                                            let new_edit = TextEdit { range: edit.range, new_text: snippet.text };
                                            if !edits.contains(&new_edit) {
                                                edits.push(new_edit);
                                            }
                                        }
                                    }
                                }
                            }
                            if !snippet_edits.is_empty() {
                                let buffer_id = buffer_to_edit.read(cx).buffer_id();
                                cx.emit(LspStoreEvent::SnippetEdit { buffer_id, edits: snippet_edits });
                            }

                            local.edits_from_lsp(
                                &buffer_to_edit,
                                edits,
                                language_server.server_id(),
                                op.text_document.version,
                                cx,
                            )
                        })
                        .await?;

                    let transaction = buffer_to_edit.update(cx, |buffer, cx| {
                        buffer.finalize_last_transaction();
                        buffer.start_transaction();
                        for (range, text) in edits {
                            buffer.edit([(range, text)], None, cx);
                        }

                        let transaction_id = buffer.end_transaction(cx)?;
                        if push_to_history {
                            buffer.finalize_last_transaction();
                            buffer.get_transaction(transaction_id).cloned()
                        } else {
                            buffer.forget_transaction(transaction_id)
                        }
                    });
                    if let Some(transaction) = transaction {
                        project_transaction.0.insert(buffer_to_edit, transaction);
                    }
                }
            }
        }

        Ok(project_transaction)
    }

    pub(super) async fn on_lsp_workspace_edit(
        this: WeakEntity<LspStore>,
        params: lsp_types::ApplyWorkspaceEditParams,
        server_id: LanguageServerId,
        cx: &mut AsyncApp,
    ) -> Result<lsp_types::ApplyWorkspaceEditResponse> {
        let this = this.upgrade().context("project project closed")?;
        let language_server = this
            .read_with(cx, |this, _| this.language_server_for_id(server_id))
            .context("language server not found")?;
        let transaction =
            Self::deserialize_workspace_edit(this.clone(), params.edit, true, Arc::clone(&language_server), cx)
                .await
                .log_err();
        this.update(cx, |this, cx| {
            if let Some(transaction) = transaction {
                cx.emit(LspStoreEvent::WorkspaceEditApplied(transaction.clone()));

                this.local.last_workspace_edits_by_language_server.insert(server_id, transaction);
            }
        });
        Ok(lsp_types::ApplyWorkspaceEditResponse { applied: true, failed_change: None, failure_reason: None })
    }
}
