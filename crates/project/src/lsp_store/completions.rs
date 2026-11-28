use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ops::Range;
use std::rc::Rc;
use std::slice;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

use anyhow::Context as _;
use anyhow::Result;
use futures::FutureExt as _;
use futures::future::join_all;
use futures::select_biased;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::SharedString;
use gpui::Task;
use language::Buffer;
use language::BufferSnapshot;
use language::CachedLspAdapter;
use language::CodeLabel;
use language::CodeLabelExt as _;
use language::Language;
use language::LanguageAwareStyling;
use language::PointUtf16;
use language::ToOffset as _;
use language::Transaction;
use language::language_settings::COMPLETIONS;
use lsp::LSP_REQUEST_TIMEOUT;
use lsp_types::CompletionContext;
use lsp_types::CompletionTriggerKind;
use lsp_types::request::ResolveCompletionItem;
use rustc_hash::FxHashSet;
use text::LineEnding;
use util::ResultExt as _;

use super::LanguageServerToQuery;
use super::LspStore;
use super::capabilities::completion_trigger_characters_for_buffer;
use super::capabilities::lsp_command_allowed_for_buffer;
use crate::Completion;
use crate::CompletionDisplayOptions;
use crate::CompletionResponse;
use crate::CompletionSource;
use crate::CoreCompletion;
use crate::lsp_command::GetCompletions;

fn resolve_word_completion(snapshot: &BufferSnapshot, completion: &mut Completion) {
    let CompletionSource::BufferWord { word_range, resolved } = &mut completion.source else {
        return;
    };
    if *resolved {
        return;
    }

    if completion.new_text != snapshot.text_for_range(word_range.clone()).collect::<String>() {
        return;
    }

    let mut offset = 0;
    for chunk in snapshot.chunks(word_range.clone(), LanguageAwareStyling { tree_sitter: true, diagnostics: true }) {
        let end_offset = offset + chunk.text.len();
        if let Some(highlight_id) = chunk.syntax_highlight_id {
            completion.label.runs.push((offset..end_offset, highlight_id));
        }
        offset = end_offset;
    }
    *resolved = true;
}

async fn populate_labels_for_completions(
    new_completions: Vec<CoreCompletion>,
    language: Option<Arc<Language>>,
    lsp_adapter: Option<Arc<CachedLspAdapter>>,
) -> Vec<Completion> {
    let lsp_completions = new_completions
        .iter()
        .filter_map(|new_completion| new_completion.source.lsp_completion(true).map(|x| x.into_owned()))
        .collect::<Vec<_>>();

    let mut labels = if let Some((language, lsp_adapter)) = language.as_ref().zip(lsp_adapter) {
        lsp_adapter.labels_for_completions(&lsp_completions, language).await.log_err().unwrap_or_default()
    } else {
        Vec::new()
    }
    .into_iter()
    .fuse();

    let mut completions = Vec::new();
    for completion in new_completions {
        if let Some(lsp_completion) = completion.source.lsp_completion(true) {
            let documentation = lsp_completion.documentation.clone().map(Into::into);

            let mut label = labels
                .next()
                .flatten()
                .unwrap_or_else(|| CodeLabel::fallback_for_completion(&lsp_completion, language.as_deref()));
            ensure_uniform_list_compatible_label(&mut label);
            completions.push(Completion {
                label,
                documentation,
                replace_range: completion.replace_range,
                new_text: completion.new_text,
                insert_text_mode: lsp_completion.insert_text_mode,
                source: completion.source,
                icon_path: None,
                icon_color: None,
                confirm: None,
                match_start: None,
                snippet_deduplication_key: None,
                group: None,
            });
        } else {
            let mut label = CodeLabel::plain(completion.new_text.clone(), None);
            ensure_uniform_list_compatible_label(&mut label);
            completions.push(Completion {
                label,
                documentation: None,
                replace_range: completion.replace_range,
                new_text: completion.new_text,
                source: completion.source,
                insert_text_mode: None,
                icon_path: None,
                icon_color: None,
                confirm: None,
                match_start: None,
                snippet_deduplication_key: None,
                group: None,
            });
        }
    }
    completions
}

#[derive(Clone, Debug)]
pub enum CompletionDocumentation {
    /// There is no documentation for this completion.
    Undocumented,
    /// A single line of documentation.
    SingleLine(SharedString),
    /// Multiple lines of plain text documentation.
    MultiLinePlainText(SharedString),
    /// Markdown documentation.
    MultiLineMarkdown(SharedString),
    /// Both single line and multiple lines of plain text documentation.
    SingleLineAndMultiLinePlainText { single_line: SharedString, plain_text: Option<SharedString> },
}

impl From<lsp_types::Documentation> for CompletionDocumentation {
    fn from(docs: lsp_types::Documentation) -> Self {
        match docs {
            lsp_types::Documentation::String(text) => {
                if text.lines().count() <= 1 {
                    CompletionDocumentation::SingleLine(text.trim().to_owned().into())
                } else {
                    CompletionDocumentation::MultiLinePlainText(text.into())
                }
            }

            lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent { kind, value }) => match kind {
                lsp_types::MarkupKind::PlainText => {
                    if value.lines().count() <= 1 {
                        CompletionDocumentation::SingleLine(value.into())
                    } else {
                        CompletionDocumentation::MultiLinePlainText(value.into())
                    }
                }

                lsp_types::MarkupKind::Markdown => CompletionDocumentation::MultiLineMarkdown(value.into()),
            },
        }
    }
}

/// Completion items are displayed in a `UniformList`.
/// Usually, those items are single-line strings, but in LSP responses,
/// completion items `label`, `detail` and `label_details.description` may contain newlines or long spaces.
/// Many language plugins construct these items by joining these parts together, and we may use `CodeLabel::fallback_for_completion` that uses `label` at least.
/// All that may lead to a newline being inserted into resulting `CodeLabel.text`, which will force `UniformList` to bloat each entry to occupy more space,
/// breaking the completions menu presentation.
///
/// Sanitize the text to ensure there are no newlines, or, if there are some, remove them and also remove long space sequences if there were newlines.
fn ensure_uniform_list_compatible_label(label: &mut CodeLabel) {
    let mut new_text = String::with_capacity(label.text.len());
    let mut offset_map = vec![0; label.text.len() + 1];
    let mut last_char_was_space = false;
    let mut new_idx = 0;
    let chars = label.text.char_indices().fuse();
    let mut newlines_removed = false;

    for (idx, c) in chars {
        offset_map[idx] = new_idx;

        match c {
            '\r' | '\n' if last_char_was_space => {
                newlines_removed = true;
            }
            '\t' | ' ' if last_char_was_space => {}
            '\r' | '\n' if !last_char_was_space => {
                new_text.push(' ');
                new_idx += 1;
                last_char_was_space = true;
                newlines_removed = true;
            }
            ' ' | '\t' => {
                new_text.push(' ');
                new_idx += 1;
                last_char_was_space = true;
            }
            _ => {
                new_text.push(c);
                new_idx += c.len_utf8();
                last_char_was_space = false;
            }
        }
    }
    offset_map[label.text.len()] = new_idx;

    // Only modify the label if newlines were removed.
    if !newlines_removed {
        return;
    }

    let last_index = new_idx;
    let mut run_ranges_errors = Vec::new();
    label.runs.retain_mut(|(range, _)| {
        if let Some(&start) = offset_map.get(range.start) {
            range.start = start;
        } else {
            run_ranges_errors.push(range.clone());
            return false;
        }

        if let Some(&end) = offset_map.get(range.end) {
            range.end = end;
        } else {
            run_ranges_errors.push(range.clone());
            range.end = last_index;
        }
        true
    });
    if !run_ranges_errors.is_empty() {
        log::error!("Completion label has errors in its run ranges: {run_ranges_errors:?}, label text: {}", label.text);
    }

    let mut wrong_filter_range = None;
    if label.filter_range == (0..label.text.len()) {
        label.filter_range = 0..new_text.len();
    } else {
        let mut original_filter_range = Some(label.filter_range.clone());
        if let Some(&start) = offset_map.get(label.filter_range.start) {
            label.filter_range.start = start;
        } else {
            wrong_filter_range = original_filter_range.take();
            label.filter_range.start = last_index;
        }

        if let Some(&end) = offset_map.get(label.filter_range.end) {
            label.filter_range.end = end;
        } else {
            wrong_filter_range = original_filter_range.take();
            label.filter_range.end = last_index;
        }
    }
    if let Some(wrong_filter_range) = wrong_filter_range {
        log::error!("Completion label has an invalid filter range: {wrong_filter_range:?}, label text: {}", label.text);
    }

    label.text = new_text;
}

/// Falls back to an invoked completion when the server did not register the trigger character.
fn completion_context_for_triggers(mut context: CompletionContext, triggers: &BTreeSet<String>) -> CompletionContext {
    if context.trigger_kind == CompletionTriggerKind::TRIGGER_CHARACTER
        && !context.trigger_character.as_ref().is_some_and(|trigger| triggers.contains(trigger))
    {
        context.trigger_kind = CompletionTriggerKind::INVOKED;
        context.trigger_character = None;
    }
    context
}

impl LspStore {
    #[inline(never)]
    pub(crate) fn completions(
        &self,
        buffer: &Entity<Buffer>,
        position: PointUtf16,
        context: CompletionContext,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<CompletionResponse>>> {
        {
            let local = &self.local;
            let snapshot = buffer.read(cx).snapshot();
            let language = snapshot.language().cloned();

            let request = GetCompletions { position, context: context.clone() };
            let server_ids: Vec<_> = buffer.update(cx, |buffer, cx| {
                local
                    .language_servers_for_buffer(buffer, cx)
                    .filter(|(adapter, server)| {
                        lsp_command_allowed_for_buffer(local, &request, buffer, adapter, server)
                    })
                    .map(|(_, server)| server.server_id())
                    .collect()
            });

            let buffer = buffer.clone();
            let lsp_timeout = COMPLETIONS.lsp_fetch_timeout_ms;
            let lsp_timeout = (lsp_timeout > 0).then(|| Duration::from_millis(lsp_timeout));
            cx.spawn(async move |this,  cx| {
                let mut tasks = Vec::with_capacity(server_ids.len());
                this.update(cx, |lsp_store, cx| {
                    for server_id in server_ids {
                        let lsp_adapter = lsp_store.language_server_adapter_for_id(server_id);
                        // Servers register different trigger characters, so each one only sees
                        // a trigger character it registered itself.
                        let context = match &lsp_adapter {
                            Some(adapter) => completion_context_for_triggers(
                                context.clone(),
                                &completion_trigger_characters_for_buffer(
                                    &lsp_store.local,
                                    server_id,
                                    adapter,
                                    buffer.read(cx),
                                ),
                            ),
                            None => context.clone(),
                        };
                        let lsp_timeout = lsp_timeout
                            .map(|lsp_timeout| cx.background_executor().timer(lsp_timeout));
                        let mut timeout = cx.background_spawn(async move {
                            match lsp_timeout {
                                Some(lsp_timeout) => {
                                    lsp_timeout.await;
                                    true
                                },
                                None => false,
                            }
                        }).fuse();
                        let mut lsp_request = lsp_store.request_lsp(
                            buffer.clone(),
                            LanguageServerToQuery::Other(server_id),
                            GetCompletions {
                                position,
                                context,
                            },
                            cx,
                        ).fuse();
                        let new_task = cx.background_spawn(async move {
                            select_biased! {
                                response = lsp_request => anyhow::Ok(Some(response?)),
                                timeout_happened = timeout => {
                                    if timeout_happened {
                                        log::warn!("Fetching completions from server {server_id} timed out, timeout ms: {}", COMPLETIONS.lsp_fetch_timeout_ms);
                                        Ok(None)
                                    } else {
                                        let completions = lsp_request.await?;
                                        Ok(Some(completions))
                                    }
                                },
                            }
                        });
                        tasks.push((lsp_adapter, new_task));
                    }
                })?;

                let futures = tasks.into_iter().map(async |(lsp_adapter, task)| {
                    let completion_response = task.await.ok()??;
                    let completions = populate_labels_for_completions(
                            completion_response.completions,
                            language.clone(),
                            lsp_adapter,
                        )
                        .await;
                    Some(CompletionResponse {
                        completions,
                        display_options: CompletionDisplayOptions::default(),
                        is_incomplete: completion_response.is_incomplete,
                    })
                });

                let responses: Vec<Option<CompletionResponse>> = join_all(futures).await;

                Ok(responses.into_iter().flatten().collect())
            })
        }
    }

    pub fn resolve_completions(
        &self,
        buffer: Entity<Buffer>,
        completion_indices: Vec<usize>,
        completions: Rc<RefCell<Box<[Completion]>>>,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        let buffer_snapshot = buffer.read(cx).snapshot();
        let resolvable_servers = completion_indices
            .iter()
            .filter_map(|&completion_index| completions.borrow()[completion_index].source.server_id())
            .filter(|&server_id| {
                self.text_document_capability_matches_for_server(
                    &buffer,
                    server_id,
                    "textDocument/completion",
                    |capabilities| GetCompletions::can_resolve_completions(capabilities.server_capabilities),
                    cx,
                )
            })
            .collect::<FxHashSet<_>>();

        cx.spawn(async move |lsp_store, cx| {
            let request_timeout = LSP_REQUEST_TIMEOUT;

            let mut did_resolve = false;
            for completion_index in completion_indices {
                let server_id = {
                    let completion = &completions.borrow()[completion_index];
                    completion.source.server_id()
                };
                if let Some(server_id) = server_id {
                    if !resolvable_servers.contains(&server_id) {
                        continue;
                    }
                    let server_and_adapter = lsp_store
                        .read_with(cx, |lsp_store, _| {
                            let server = lsp_store.language_server_for_id(server_id)?;
                            let adapter = lsp_store.language_server_adapter_for_id(server.server_id())?;
                            Some((server, adapter))
                        })
                        .ok()
                        .flatten();
                    let Some((server, adapter)) = server_and_adapter else {
                        continue;
                    };

                    let resolved = Self::resolve_completion_local(
                        server,
                        Rc::clone(&completions),
                        completion_index,
                        request_timeout,
                        true,
                    )
                    .await
                    .log_err()
                    .is_some();
                    if resolved {
                        Self::regenerate_completion_labels(
                            adapter,
                            &buffer_snapshot,
                            Rc::clone(&completions),
                            completion_index,
                        )
                        .await
                        .log_err();
                        did_resolve = true;
                    }
                } else {
                    resolve_word_completion(&buffer_snapshot, &mut completions.borrow_mut()[completion_index]);
                }
            }

            Ok(did_resolve)
        })
    }

    async fn resolve_completion_local(
        server: Arc<lsp::LanguageServer>,
        completions: Rc<RefCell<Box<[Completion]>>>,
        completion_index: usize,
        request_timeout: Duration,
        can_resolve: bool,
    ) -> Result<()> {
        let server_id = server.server_id();
        if !can_resolve {
            return Ok(());
        }

        let request = {
            let completion = &completions.borrow()[completion_index];
            match &completion.source {
                CompletionSource::Lsp { lsp_completion, resolved, server_id: completion_server_id, .. } => {
                    if *resolved {
                        return Ok(());
                    }
                    anyhow::ensure!(
                        server_id == *completion_server_id,
                        "server_id mismatch, querying completion resolve for {server_id} but completion server id is {completion_server_id}"
                    );
                    server.request::<ResolveCompletionItem>(*lsp_completion.clone(), request_timeout)
                }
                CompletionSource::BufferWord { .. } | CompletionSource::Custom => {
                    return Ok(());
                }
            }
        };
        let resolved_completion = request.await.into_response().context("resolve completion")?;

        let mut completions = completions.borrow_mut();
        let completion = &mut completions[completion_index];
        if let CompletionSource::Lsp { lsp_completion, resolved, server_id: completion_server_id, .. } =
            &mut completion.source
        {
            if *resolved {
                return Ok(());
            }
            anyhow::ensure!(
                server_id == *completion_server_id,
                "server_id mismatch, applying completion resolve for {server_id} but completion server id is {completion_server_id}"
            );
            **lsp_completion = resolved_completion;
            *resolved = true;

            // We must not use any data such as sortText, filterText, insertText and textEdit to edit `Completion` since they are not supposed to change during resolve.
            // Refer: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion
            //
            // We still re-derive new_text here as a workaround for the specific
            // VS Code TypeScript completion resolve flow that vtsls wraps:
            // https://github.com/microsoft/vscode/blob/838b48504cd9a2338e2ca9e854da9cec990c4d57/extensions/typescript-language-features/src/languageFeatures/completions.ts#L218
            //
            // Some servers (e.g. vtsls with completeFunctionCalls) update
            // insertText/textEdit during resolve to add snippet content like
            // function call parentheses.
            //
            // vtsls resolve flow:
            //   https://github.com/yioneko/vtsls/blob/fecf52324a30e72dfab1537047556076720c1a5f/packages/service/src/service/completion.ts#L228-L244
            // vtsls converter (isSnippet / insertTextFormat):
            //   https://github.com/yioneko/vtsls/blob/28e075105d7711d635ebf8aefc971bb8e1d2fe65/packages/service/src/utils/converter.ts#L149-L200
            //
            // NB: We only update the text content here, NOT the replace/insert
            // ranges on `Completion`. Those ranges were converted to anchors from
            // the original response and stay valid across buffer edits. The LSP
            // ranges in the resolved text_edit are stale when completions are
            // cached across keystrokes (see #34094).
            let resolved_new_text = lsp_completion
                .text_edit
                .as_ref()
                .map(|edit| match edit {
                    lsp_types::CompletionTextEdit::Edit(e) => e.new_text.clone(),
                    lsp_types::CompletionTextEdit::InsertAndReplace(e) => e.new_text.clone(),
                })
                .or_else(|| lsp_completion.insert_text.clone());
            if let Some(mut resolved_new_text) = resolved_new_text {
                LineEnding::normalize(&mut resolved_new_text);
                completion.new_text = resolved_new_text;
            }
        }
        Ok(())
    }

    async fn regenerate_completion_labels(
        adapter: Arc<CachedLspAdapter>,
        snapshot: &BufferSnapshot,
        completions: Rc<RefCell<Box<[Completion]>>>,
        completion_index: usize,
    ) -> Result<()> {
        let completion_item = completions.borrow()[completion_index].source.lsp_completion(true).map(Cow::into_owned);
        if let Some(lsp_documentation) =
            completion_item.as_ref().and_then(|completion_item| completion_item.documentation.clone())
        {
            let mut completions = completions.borrow_mut();
            let completion = &mut completions[completion_index];
            completion.documentation = Some(lsp_documentation.into());
        } else {
            let mut completions = completions.borrow_mut();
            let completion = &mut completions[completion_index];
            completion.documentation = Some(CompletionDocumentation::Undocumented);
        }

        let mut new_label = match completion_item {
            Some(completion_item) => {
                // Some language servers always return `detail` lazily via resolve, regardless of
                // the resolvable properties ed advertises. Regenerate labels here to handle this.
                // See: https://github.com/yioneko/vtsls/issues/213
                let language = snapshot.language();
                match language {
                    Some(language) => {
                        adapter.labels_for_completions(slice::from_ref(&completion_item), language).await?
                    }
                    None => Vec::new(),
                }
                .pop()
                .flatten()
                .unwrap_or_else(|| CodeLabel::fallback_for_completion(&completion_item, language.map(AsRef::as_ref)))
            }
            None => CodeLabel::plain(completions.borrow()[completion_index].new_text.clone(), None),
        };
        ensure_uniform_list_compatible_label(&mut new_label);

        let mut completions = completions.borrow_mut();
        let completion = &mut completions[completion_index];
        completion.label = new_label;

        Ok(())
    }

    pub fn apply_additional_edits_for_completion(
        &self,
        buffer_handle: Entity<Buffer>,
        completions: Rc<RefCell<Box<[Completion]>>>,
        completion_index: usize,
        push_to_history: bool,
        all_commit_ranges: Vec<Range<language::Anchor>>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<Transaction>>> {
        let request_timeout = LSP_REQUEST_TIMEOUT;

        let Some(server_id) = completions.borrow()[completion_index].source.server_id() else {
            return Task::ready(Ok(None));
        };
        let can_resolve = self.text_document_capability_matches_for_server(
            &buffer_handle,
            server_id,
            "textDocument/completion",
            |capabilities| GetCompletions::can_resolve_completions(capabilities.server_capabilities),
            cx,
        );
        let Some(server) = buffer_handle.update(cx, |buffer, cx| {
            self.language_server_for_local_buffer(buffer, server_id, cx).map(|(_, server)| Arc::clone(server))
        }) else {
            return Task::ready(Ok(None));
        };

        cx.spawn(async move |this, cx| {
            Self::resolve_completion_local(
                Arc::clone(&server),
                Rc::clone(&completions),
                completion_index,
                request_timeout,
                can_resolve,
            )
            .await
            .context("resolving completion")?;
            let completion = completions.borrow()[completion_index].clone();
            let additional_text_edits = completion
                .source
                .lsp_completion(true)
                .as_ref()
                .and_then(|lsp_completion| lsp_completion.additional_text_edits.clone());
            if let Some(edits) = additional_text_edits {
                let edits = this
                    .update(cx, |this, cx| {
                        this.local.edits_from_lsp(&buffer_handle, edits, server.server_id(), None, cx)
                    })?
                    .await?;

                buffer_handle.update(cx, |buffer, cx| {
                    buffer.finalize_last_transaction();
                    buffer.start_transaction();

                    for (range, text) in edits {
                        // Zero-width additional edits (e.g. auto-imports at file start, or
                        // rust-analyzer's ref-match `&` insertions) only overlap the primary
                        // edit when they fall strictly inside it. Touching its boundary is fine.
                        //
                        // Ref: https://github.com/zed-industries/zed/issues/26136
                        // Ref: https://github.com/zed-industries/zed/issues/56973
                        let is_insertion = range.start.cmp(&range.end, buffer).is_eq();
                        let has_overlap = if is_insertion {
                            let insert_offset = range.start.to_offset(buffer);
                            all_commit_ranges.iter().any(|commit_range| {
                                commit_range.start.to_offset(buffer) < insert_offset
                                    && insert_offset < commit_range.end.to_offset(buffer)
                            })
                        } else {
                            all_commit_ranges.iter().any(|commit_range| {
                                let start_within = commit_range.start.cmp(&range.start, buffer).is_le()
                                    && commit_range.end.cmp(&range.start, buffer).is_ge();
                                let end_within = range.start.cmp(&commit_range.end, buffer).is_le()
                                    && range.end.cmp(&commit_range.end, buffer).is_ge();
                                start_within || end_within
                            })
                        };

                        // Skip additional edits which overlap with the primary completion edit
                        // https://github.com/zed-industries/zed/pull/1871
                        if !has_overlap {
                            buffer.edit([(range, text)], None, cx);
                        }
                    }

                    let transaction = buffer.end_transaction(cx).is_some().then(|| {
                        let transaction =
                            buffer.finalize_last_transaction().expect("transaction was just ended").clone();
                        if !push_to_history {
                            buffer.forget_transaction(transaction.id);
                        }
                        transaction
                    });
                    Ok(transaction)
                })
            } else {
                Ok(None)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_context_keeps_only_registered_trigger_characters() {
        let triggers = BTreeSet::from([".".to_owned(), ":".to_owned()]);
        let triggered = |character: &str| CompletionContext {
            trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
            trigger_character: Some(character.to_owned()),
        };
        let invoked = CompletionContext { trigger_kind: CompletionTriggerKind::INVOKED, trigger_character: None };

        assert_eq!(completion_context_for_triggers(triggered("."), &triggers), triggered("."));
        // Another server's trigger character (e.g. a letter registered by Emmet).
        assert_eq!(completion_context_for_triggers(triggered("a"), &triggers), invoked);
        assert_eq!(completion_context_for_triggers(triggered("."), &BTreeSet::new()), invoked);
        assert_eq!(completion_context_for_triggers(invoked.clone(), &triggers), invoked);
    }
}
