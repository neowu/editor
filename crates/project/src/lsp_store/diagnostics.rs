use std::borrow::Cow;
use std::cell::LazyCell;
use std::cmp::Ordering;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::channel::mpsc;
use futures::channel::oneshot;
use futures::future::Shared;
use futures::future::join_all;
use futures::select_biased;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::SharedString;
use gpui::Task;
use language::Bias;
use language::Buffer;
use language::Diagnostic;
use language::DiagnosticEntry;
use language::DiagnosticMessage;
use language::DiagnosticSet;
use language::DiagnosticSourceKind;
use language::LocalFile as _;
use language::Patch;
use language::PointUtf16;
use language::RelatedInformation;
use language::RelatedLocation;
use language::ToPointUtf16 as _;
use language::Unclipped;
use language::range_from_lsp;
use lsp::LSP_REQUEST_TIMEOUT;
use lsp::LanguageServer;
use lsp::LanguageServerId;
use lsp_types::DiagnosticServerCapabilities;
use lsp_types::DiagnosticSeverity;
use lsp_types::DiagnosticTag;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use serde::Serialize;
use text::BufferId;
use util::ConnectionResult;
use util::ResultExt as _;
use util::post_inc;
use util::rel_path::RelPath;
use worktree::File;
use worktree::PathChange;
use worktree::UpdatedEntriesSet;
use worktree::Worktree;
use worktree::WorktreeId;

use super::CROSS_BUFFER_DIAGNOSTICS_PULL_DELAY;
use super::DOCUMENT_DIAGNOSTICS_RETRIGGER_DELAY;
use super::DOCUMENT_DIAGNOSTICS_RETRIGGER_LIMIT;
use super::LanguageServerState;
use super::LanguageServerToQuery;
use super::LocalLspStore;
use super::LspStore;
use super::LspStoreEvent;
use super::WORKSPACE_DIAGNOSTICS_REPULL_DELAY;
use super::WORKSPACE_DIAGNOSTICS_TOKEN_START;
use super::capabilities::dynamic_text_document_registration_allows_buffer;
use super::lsp_pull_cancelled_with_retrigger;
use super::should_log_lsp_response_error;
use crate::LspPullDiagnostics;
use crate::ProjectPath;
use crate::PulledDiagnostics;
use crate::lsp_store::lsp_command::GetDocumentDiagnostics;
use crate::lsp_store::lsp_command::file_path_to_lsp_url;

#[derive(Debug)]
pub(crate) struct DocumentDiagnosticsUpdate<'a, D> {
    pub(crate) diagnostics: D,
    pub(crate) result_id: Option<SharedString>,
    pub(crate) registration_id: Option<SharedString>,
    pub(crate) server_id: LanguageServerId,
    pub(crate) disk_based_sources: Cow<'a, [String]>,
}

struct DocumentDiagnostics {
    diagnostics: Vec<DiagnosticEntry<Unclipped<PointUtf16>>>,
    document_abs_path: PathBuf,
    version: Option<i32>,
}

pub(super) fn lsp_workspace_diagnostics_refresh(
    registration_id: Option<String>,
    options: DiagnosticServerCapabilities,
    server: Arc<LanguageServer>,
    cx: &Context<'_, LspStore>,
) -> Option<WorkspaceRefreshTask> {
    let identifier = workspace_diagnostic_options(&options)?.identifier.clone();
    let registration_id_shared = registration_id.as_ref().map(SharedString::from);

    let (progress_tx, mut progress_rx) = mpsc::channel(1);
    let (mut refresh_tx, mut refresh_rx) = mpsc::channel::<Option<oneshot::Sender<bool>>>(8);
    refresh_tx.try_send(None).ok();

    let timeout = LSP_REQUEST_TIMEOUT;

    let workspace_query_language_server = cx.spawn(async move |lsp_store, cx| {
        let mut attempts = 0;
        let max_attempts = 50;
        let retries_after_exhaustion = 3;
        let mut requests = 0;

        loop {
            let Some(completion_tx) = refresh_rx.next().await else {
                return;
            };
            let mut completion_txs = completion_tx.into_iter().collect::<Vec<_>>();
            let mut queued_completion_txs = Vec::<oneshot::Sender<bool>>::new();

            'request: loop {
                requests += 1;
                if attempts > max_attempts {
                    log::error!(
                        "Failed to pull workspace diagnostics {max_attempts} times, pausing until the next refresh"
                    );
                    for tx in completion_txs.drain(..).chain(queued_completion_txs.drain(..)) {
                        tx.send(false).ok();
                    }
                    attempts = max_attempts + 1 - retries_after_exhaustion;
                    break 'request;
                }
                let backoff_millis = (50 * (1 << attempts)).clamp(30, 1000);
                cx.background_executor().timer(Duration::from_millis(backoff_millis)).await;
                attempts += 1;

                // Absorb refresh requests that queued up in the meantime, so their
                // waiters are resolved by this attempt instead of waiting behind it.
                completion_txs.append(&mut queued_completion_txs);
                while let Ok(queued_refresh) = refresh_rx.try_recv() {
                    completion_txs.extend(queued_refresh);
                }

                let token = if let Some(registration_id) = &registration_id {
                    format!(
                        "workspace/diagnostic/{}/{requests}/{WORKSPACE_DIAGNOSTICS_TOKEN_START}{registration_id}",
                        server.server_id(),
                    )
                } else {
                    format!("workspace/diagnostic/{}/{requests}", server.server_id())
                };

                let Ok(previous_result_ids) = lsp_store.update(cx, |lsp_store, _| {
                    lsp_store.set_workspace_refresh_token(server.server_id(), &registration_id, Some(token.clone()));
                    lsp_store
                        .result_ids_for_workspace_refresh(server.server_id(), &registration_id_shared)
                        .into_iter()
                        .filter_map(|(abs_path, result_id)| {
                            let uri = file_path_to_lsp_url(&abs_path).ok()?;
                            Some(lsp_types::PreviousResultId { uri, value: result_id.to_string() })
                        })
                        .collect()
                }) else {
                    return;
                };

                progress_rx.try_recv().ok();
                let mut refresh_requested = false;
                let mut request = pin!(
                    server
                        .request::<lsp_types::request::WorkspaceDiagnosticRequest>(
                            lsp_types::WorkspaceDiagnosticParams {
                                previous_result_ids,
                                identifier: identifier.clone(),
                                work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                                partial_result_params: lsp_types::PartialResultParams {
                                    partial_result_token: Some(lsp_types::ProgressToken::String(token)),
                                },
                            },
                            Duration::MAX,
                        )
                        .fuse()
                );
                let response_result = 'response: loop {
                    let mut inactivity_timer = pin!(server.request_timer(timeout).fuse());
                    loop {
                        let mut progress = pin!(progress_rx.next().fuse());
                        let mut refresh = pin!(refresh_rx.next().fuse());
                        select_biased! {
                            response = request => break 'response response,
                            new_progress = progress => match new_progress {
                                Some(()) => continue 'response,
                                None => return,
                            },
                            new_refresh = refresh => match new_refresh {
                                Some(completion_tx) => {
                                    refresh_requested = true;
                                    queued_completion_txs.extend(completion_tx);
                                }
                                None => return,
                            },
                            _ = inactivity_timer => {
                                // Release everyone waiting on this refresh, including waiters
                                // that queued up while the request was in flight, so they can
                                // fall back to cached diagnostics instead of waiting through
                                // every retry.
                                for tx in completion_txs
                                    .drain(..)
                                    .chain(queued_completion_txs.drain(..))
                                {
                                    tx.send(false).ok();
                                }
                                continue 'response;
                            }
                        }
                    }
                };
                if lsp_store
                    .update(cx, |lsp_store, _| {
                        lsp_store.set_workspace_refresh_token(server.server_id(), &registration_id, None);
                    })
                    .is_err()
                {
                    return;
                }

                // https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#diagnostic_refresh
                // >  If a server closes a workspace diagnostic pull request the client should re-trigger the request.
                match response_result {
                    ConnectionResult::Timeout => {
                        log::error!("Unexpected workspace diagnostics pull timeout without a request timer");
                        continue 'request;
                    }
                    ConnectionResult::ConnectionReset => {
                        log::error!("Server closed a workspace diagnostics pull request");
                        continue 'request;
                    }
                    ConnectionResult::Result(Err(e)) => {
                        if lsp_pull_cancelled_with_retrigger(&e) {
                            attempts = 0;
                        } else {
                            if should_log_lsp_response_error(&e) {
                                log::error!("Error during workspace diagnostics pull: {e:#}");
                            } else {
                                log::debug!("Workspace diagnostics pull denied: {e:#}");
                            }
                            for tx in completion_txs.drain(..) {
                                tx.send(false).ok();
                            }
                            if refresh_requested {
                                continue 'request;
                            }
                            break 'request;
                        }
                    }
                    ConnectionResult::Result(Ok(pulled_diagnostics)) => {
                        attempts = 0;
                        if lsp_store
                            .update(cx, |lsp_store, cx| {
                                lsp_store.apply_workspace_diagnostic_report(
                                    server.server_id(),
                                    pulled_diagnostics,
                                    registration_id_shared.clone(),
                                    cx,
                                );
                            })
                            .is_err()
                        {
                            return;
                        }
                        for tx in completion_txs.drain(..) {
                            tx.send(true).ok();
                        }
                    }
                }

                if refresh_requested {
                    continue 'request;
                }
                let mut repull_timer = pin!(cx.background_executor().timer(WORKSPACE_DIAGNOSTICS_REPULL_DELAY).fuse());
                let mut refresh = pin!(refresh_rx.next().fuse());
                select_biased! {
                    new_refresh = refresh => match new_refresh {
                        Some(completion_tx) => completion_txs.extend(completion_tx),
                        None => return,
                    },
                    () = repull_timer => {}
                }
            }
        }
    });

    Some(WorkspaceRefreshTask {
        refresh_tx,
        progress_tx,
        current_request_token: None,
        _task: workspace_query_language_server,
    })
}

const fn diagnostics_inter_file_dependencies(options: &DiagnosticServerCapabilities) -> bool {
    match options {
        DiagnosticServerCapabilities::Options(diagnostic_options) => diagnostic_options.inter_file_dependencies,
        DiagnosticServerCapabilities::RegistrationOptions(registration_options) => {
            registration_options.diagnostic_options.inter_file_dependencies
        }
    }
}

fn buffer_diagnostic_identifier(options: &DiagnosticServerCapabilities) -> Option<SharedString> {
    match &options {
        DiagnosticServerCapabilities::Options(diagnostic_options) => {
            diagnostic_options.identifier.as_deref().map(SharedString::new)
        }
        DiagnosticServerCapabilities::RegistrationOptions(registration_options) => {
            let diagnostic_options = &registration_options.diagnostic_options;
            diagnostic_options.identifier.as_deref().map(SharedString::new)
        }
    }
}

fn workspace_diagnostic_options(options: &DiagnosticServerCapabilities) -> Option<&lsp_types::DiagnosticOptions> {
    let diagnostic_options = match options {
        DiagnosticServerCapabilities::Options(diagnostic_options) => diagnostic_options,
        DiagnosticServerCapabilities::RegistrationOptions(registration_options) => {
            &registration_options.diagnostic_options
        }
    };
    diagnostic_options.workspace_diagnostics.then_some(diagnostic_options)
}

pub(crate) struct WorkspaceRefreshTask {
    refresh_tx: mpsc::Sender<Option<oneshot::Sender<bool>>>,
    pub(super) progress_tx: mpsc::Sender<()>,
    pub(super) current_request_token: Option<String>,
    _task: Task<()>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DiagnosticSummary {
    pub error_count: usize,
    pub warning_count: usize,
}

impl DiagnosticSummary {
    fn new<'a, T: 'a>(diagnostics: impl IntoIterator<Item = &'a DiagnosticEntry<T>>) -> Self {
        let mut this = Self { error_count: 0, warning_count: 0 };

        for entry in diagnostics {
            if entry.diagnostic.is_primary {
                match entry.diagnostic.severity {
                    DiagnosticSeverity::ERROR => this.error_count += 1,
                    DiagnosticSeverity::WARNING => this.warning_count += 1,
                    _ => {}
                }
            }
        }

        this
    }

    const fn is_empty(&self) -> bool {
        self.error_count == 0 && self.warning_count == 0
    }
}

/// Converts the related information of a published diagnostic, putting the locations
/// of the document it was published for into that document's coordinates so that they
/// are converted and followed like the diagnostic's own range.
fn related_information_from_lsp(
    diagnostic: &lsp_types::Diagnostic,
    document_uri: &lsp_types::Uri,
) -> Option<Arc<[RelatedInformation<Unclipped<PointUtf16>>]>> {
    let infos = diagnostic.related_information.as_ref()?;
    if infos.is_empty() {
        return None;
    }
    Some(
        infos
            .iter()
            .map(|info| RelatedInformation {
                location: if &info.location.uri == document_uri {
                    RelatedLocation::InBuffer(range_from_lsp(info.location.range))
                } else {
                    RelatedLocation::InAnotherFile(info.location.clone())
                },
                message: info.message.clone(),
            })
            .collect(),
    )
}

impl LocalLspStore {
    pub(super) fn server_pulls_workspace_diagnostics(&self, server_id: LanguageServerId) -> bool {
        match self.language_servers.get(&server_id) {
            Some(LanguageServerState::Running { workspace_diagnostics_refresh_tasks, .. }) => {
                !workspace_diagnostics_refresh_tasks.is_empty()
            }
            _ => false,
        }
    }

    pub(super) fn update_buffer_diagnostics(
        &mut self,
        buffer: &Entity<Buffer>,
        server_id: LanguageServerId,
        version: Option<i32>,
        new_diagnostics: Vec<DiagnosticEntry<Unclipped<PointUtf16>>>,
        reused_diagnostics: Vec<DiagnosticEntry<Unclipped<PointUtf16>>>,
        cx: &mut Context<LspStore>,
    ) -> Result<()> {
        fn compare_diagnostics(a: &Diagnostic, b: &Diagnostic) -> Ordering {
            Ordering::Equal
                .then_with(|| b.is_primary.cmp(&a.is_primary))
                .then_with(|| a.is_disk_based.cmp(&b.is_disk_based))
                .then_with(|| a.severity.cmp(&b.severity))
                .then_with(|| a.message.as_str().cmp(b.message.as_str()))
        }

        let mut diagnostics = Vec::with_capacity(new_diagnostics.len() + reused_diagnostics.len());
        diagnostics.extend(new_diagnostics.into_iter().map(|d| (true, d)));
        diagnostics.extend(reused_diagnostics.into_iter().map(|d| (false, d)));

        diagnostics.sort_unstable_by(|(_, a), (_, b)| {
            Ordering::Equal
                .then_with(|| a.range.start.cmp(&b.range.start))
                .then_with(|| b.range.end.cmp(&a.range.end))
                .then_with(|| compare_diagnostics(&a.diagnostic, &b.diagnostic))
        });

        let snapshot = self.buffer_snapshot_for_lsp_version(buffer, server_id, version, cx)?;

        let edits_since_save = LazyCell::new(|| {
            let saved_version = buffer.read(cx).saved_version();
            Patch::new(snapshot.edits_since::<PointUtf16>(saved_version).collect())
        });

        let mut sanitized_diagnostics = Vec::with_capacity(diagnostics.len());

        for (new_diagnostic, entry) in diagnostics {
            // Some diagnostics are based on files on disk instead of buffers'
            // current contents. Adjust these diagnostics' ranges to reflect
            // any unsaved edits.
            // Do not alter the reused ones though, as their coordinates were stored as anchors
            // and were properly adjusted on reuse.
            let is_disk_based = new_diagnostic && entry.diagnostic.is_disk_based;
            let mut entry = entry.map_coordinates(|range| {
                let range = if is_disk_based {
                    Unclipped((*edits_since_save).old_to_new(range.start.0))
                        ..Unclipped((*edits_since_save).old_to_new(range.end.0))
                } else {
                    range.clone()
                };
                snapshot.clip_point_utf16(range.start, Bias::Left)..snapshot.clip_point_utf16(range.end, Bias::Right)
            });

            // Expand empty ranges by one codepoint. Only the diagnostic's own range is
            // widened: the related locations are reported back as the server framed them.
            if entry.range.start == entry.range.end {
                // This will be go to the next boundary when being clipped
                entry.range.end.column += 1;
                entry.range.end = snapshot.clip_point_utf16(Unclipped(entry.range.end), Bias::Right);
                if entry.range.start == entry.range.end && entry.range.end.column > 0 {
                    entry.range.start.column -= 1;
                    entry.range.start = snapshot.clip_point_utf16(Unclipped(entry.range.start), Bias::Left);
                }
            } else if entry.range.end == PointUtf16::new(entry.range.start.row + 1, 0) {
                // Some language servers (e.g. Python parsers reporting a missing `:`)
                // report a diagnostic whose range starts at the end of a line and ends
                // at the start of the next one. That range contains nothing but the
                // line terminator itself, so there is no glyph left to underline and
                // the diagnostic renders invisibly even though it's real. Only collapse
                // ranges that are exactly the terminator (checked via the precise end
                // position above) so genuinely multi-line diagnostics that merely start
                // at an end-of-line are left untouched.
                let line_end =
                    snapshot.clip_point_utf16(Unclipped(PointUtf16::new(entry.range.start.row, u32::MAX)), Bias::Left);
                if entry.range.start == line_end {
                    entry.range.end = entry.range.start;
                    if entry.range.start.column > 0 {
                        entry.range.start.column -= 1;
                        entry.range.start = snapshot.clip_point_utf16(Unclipped(entry.range.start), Bias::Left);
                    }
                }
            }

            sanitized_diagnostics.push(entry);
        }
        drop(edits_since_save);

        let set = DiagnosticSet::new(sanitized_diagnostics, &snapshot);
        buffer.update(cx, |buffer, cx| buffer.update_diagnostics(server_id, set, cx));

        Ok(())
    }
}

impl LspStore {
    fn refresh_background_diagnostics_for_buffers(
        &mut self,
        buffers: FxHashSet<BufferId>,
        cx: &Context<Self>,
    ) -> Shared<Task<()>> {
        let local = &mut self.local;
        for buffer in buffers {
            if local.buffers_to_refresh_hash_set.insert(buffer) {
                local.buffers_to_refresh_queue.push_back(buffer);
                if local.buffers_to_refresh_queue.len() == 1 {
                    local._background_diagnostics_worker = Self::background_diagnostics_worker(cx).shared();
                }
            }
        }

        local._background_diagnostics_worker.clone()
    }

    fn refresh_next_buffer(&mut self, cx: &mut Context<Self>) -> Option<Task<Result<()>>> {
        let buffer_store = self.buffer_store.clone();
        let local = &mut self.local;
        while let Some(buffer_id) = local.buffers_to_refresh_queue.pop_front() {
            local.buffers_to_refresh_hash_set.remove(&buffer_id);
            if let Some(buffer) = buffer_store.read(cx).get(buffer_id) {
                return Some(self.pull_diagnostics_for_buffer(buffer, cx));
            }
        }
        None
    }

    fn background_diagnostics_worker(cx: &Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CROSS_BUFFER_DIAGNOSTICS_PULL_DELAY).await;
                let Ok(Some(task)) = this.update(cx, |this, cx| this.refresh_next_buffer(cx)) else {
                    return;
                };
                task.await.log_err();
            }
        })
    }

    fn pull_diagnostics(
        &self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<Vec<LspPullDiagnostics>>>> {
        let buffer_id = buffer.read(cx).buffer_id();

        let servers = buffer.update(cx, |buffer, cx| {
            self.running_language_servers_for_local_buffer(buffer, cx)
                .map(|(adapter, server)| (Arc::clone(adapter), Arc::clone(server)))
                .collect::<Vec<_>>()
        });

        let pull_diagnostics = servers
            .into_iter()
            .flat_map(|(adapter, server)| {
                let local = &self.local;
                let server_id = server.server_id();
                let mut providers_with_identifiers = Vec::new();
                if let Some(provider) = local
                    .initial_server_capabilities
                    .get(&server_id)
                    .and_then(|capabilities| capabilities.diagnostic_provider.clone())
                {
                    providers_with_identifiers.push((None, provider));
                }
                if let Some(diagnostic_registrations) = local
                    .language_server_dynamic_registrations
                    .get(&server_id)
                    .and_then(|registrations| registrations.text_documents.get("textDocument/diagnostic"))
                {
                    for (registration_id, registration) in diagnostic_registrations {
                        if !dynamic_text_document_registration_allows_buffer(registration, buffer.read(cx), &adapter) {
                            continue;
                        }
                        let Some(provider) = registration.server_capabilities.diagnostic_provider.clone() else {
                            continue;
                        };
                        providers_with_identifiers.push((Some(registration_id.clone()), provider));
                    }
                }
                providers_with_identifiers
                    .into_iter()
                    .map(|(registration_id, dynamic_caps)| {
                        let identifier = buffer_diagnostic_identifier(&dynamic_caps);
                        let registration_id = registration_id.map(SharedString::from);
                        let result_id = self.result_id_for_buffer_pull(server_id, buffer_id, &registration_id, cx);
                        self.request_lsp(
                            buffer.clone(),
                            LanguageServerToQuery::Other(server_id),
                            GetDocumentDiagnostics { previous_result_id: result_id, registration_id, identifier },
                            cx,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        cx.background_spawn(async move {
            let mut responses = Vec::new();
            for diagnostics in join_all(pull_diagnostics).await {
                responses.extend(diagnostics?);
            }
            Ok(Some(responses))
        })
    }

    fn diagnostic_registration_exists(
        &self,
        server_id: LanguageServerId,
        registration_id: Option<&SharedString>,
    ) -> bool {
        let local = &self.local;
        match registration_id {
            Some(registration_id) => local
                .language_server_dynamic_registrations
                .get(&server_id)
                .and_then(|registrations| registrations.text_documents.get("textDocument/diagnostic"))
                .is_some_and(|registrations| registrations.contains_key(registration_id.as_ref())),
            None => local
                .initial_server_capabilities
                .get(&server_id)
                .is_some_and(|capabilities| capabilities.diagnostic_provider.is_some()),
        }
    }

    pub fn pull_diagnostics_for_buffer(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) -> Task<Result<()>> {
        let mut diagnostics_task = self.pull_diagnostics(buffer.clone(), cx);
        cx.spawn(async move |lsp_store, cx| {
            let mut retriggers_left = DOCUMENT_DIAGNOSTICS_RETRIGGER_LIMIT;
            let diagnostics = loop {
                match diagnostics_task.await {
                    Ok(Some(diagnostics)) => break diagnostics,
                    Ok(None) => return Ok(()),
                    Err(error) if retriggers_left > 0 && lsp_pull_cancelled_with_retrigger(&error) => {
                        retriggers_left -= 1;
                        cx.background_executor().timer(DOCUMENT_DIAGNOSTICS_RETRIGGER_DELAY).await;
                        diagnostics_task =
                            lsp_store.update(cx, |lsp_store, cx| lsp_store.pull_diagnostics(buffer.clone(), cx))?;
                    }
                    Err(error) if should_log_lsp_response_error(&error) => {
                        return Err(error).context("pulling diagnostics");
                    }
                    Err(_) => return Ok(()),
                }
            };
            lsp_store.update(cx, |lsp_store, cx| {
                let mut unchanged_buffers = FxHashMap::default();
                let server_diagnostics_updates = diagnostics
                    .into_iter()
                    .filter_map(|diagnostics_set| match diagnostics_set {
                        LspPullDiagnostics::Response { server_id, uri, diagnostics, registration_id } => {
                            Some((server_id, uri, diagnostics, registration_id))
                        }
                        LspPullDiagnostics::Default => None,
                    })
                    .filter(|(server_id, _, _, registration_id)| {
                        lsp_store.diagnostic_registration_exists(*server_id, registration_id.as_ref())
                    })
                    .fold(FxHashMap::default(), |mut acc, (server_id, uri, diagnostics, new_registration_id)| {
                        let (result_id, diagnostics) = match diagnostics {
                            PulledDiagnostics::Unchanged { result_id } => {
                                unchanged_buffers
                                    .entry(new_registration_id.clone())
                                    .or_insert_with(FxHashSet::default)
                                    .insert(uri.clone());
                                (Some(result_id), Vec::new())
                            }
                            PulledDiagnostics::Changed { result_id, diagnostics } => (result_id, diagnostics),
                        };
                        let disk_based_sources = Cow::Owned(
                            lsp_store
                                .language_server_adapter_for_id(server_id)
                                .as_ref()
                                .map_or(&[][..], |adapter| adapter.disk_based_diagnostic_sources.as_slice())
                                .to_vec(),
                        );
                        acc.entry(server_id)
                            .or_insert_with(FxHashMap::default)
                            .entry(new_registration_id.clone())
                            .or_insert_with(Vec::new)
                            .push(DocumentDiagnosticsUpdate {
                                server_id,
                                diagnostics: lsp_types::PublishDiagnosticsParams { uri, diagnostics, version: None },
                                result_id: result_id.map(SharedString::new),
                                disk_based_sources,
                                registration_id: new_registration_id,
                            });
                        acc
                    });

                for diagnostic_updates in server_diagnostics_updates.into_values() {
                    for (registration_id, diagnostic_updates) in diagnostic_updates {
                        lsp_store
                            .merge_lsp_diagnostics(
                                DiagnosticSourceKind::Pulled,
                                diagnostic_updates,
                                |document_uri, old_diagnostic, _| match old_diagnostic.source_kind {
                                    DiagnosticSourceKind::Pulled => {
                                        old_diagnostic.registration_id != registration_id
                                            || unchanged_buffers.get(&old_diagnostic.registration_id).is_some_and(
                                                |unchanged_buffers| unchanged_buffers.contains(document_uri),
                                            )
                                    }
                                    DiagnosticSourceKind::Other | DiagnosticSourceKind::Pushed => true,
                                },
                                cx,
                            )
                            .log_err();
                    }
                }
            })
        })
    }

    pub(crate) fn diagnostic_summary(&self, include_ignored: bool, cx: &App) -> DiagnosticSummary {
        let mut summary = DiagnosticSummary::default();
        for (_, _, path_summary) in self.diagnostic_summaries(include_ignored, cx) {
            summary.error_count += path_summary.error_count;
            summary.warning_count += path_summary.warning_count;
        }
        summary
    }

    /// Returns the diagnostic summary for a specific project path.
    pub(crate) fn diagnostic_summary_for_path(&self, project_path: &ProjectPath, _: &App) -> DiagnosticSummary {
        if let Some(summaries) =
            self.diagnostic_summaries.get(&project_path.worktree_id).and_then(|map| map.get(&project_path.path))
        {
            let (error_count, warning_count) =
                summaries.iter().fold((0, 0), |(error_count, warning_count), (_language_server_id, summary)| {
                    (error_count + summary.error_count, warning_count + summary.warning_count)
                });

            DiagnosticSummary { error_count, warning_count }
        } else {
            DiagnosticSummary::default()
        }
    }

    pub(crate) fn diagnostic_summaries<'a>(
        &'a self,
        include_ignored: bool,
        cx: &'a App,
    ) -> impl Iterator<Item = (ProjectPath, LanguageServerId, DiagnosticSummary)> + 'a {
        self.worktree_store
            .read(cx)
            .visible_worktrees(cx)
            .filter_map(|worktree| {
                let worktree = worktree.read(cx);
                Some((worktree, self.diagnostic_summaries.get(&worktree.id())?))
            })
            .flat_map(move |(worktree, summaries)| {
                let worktree_id = worktree.id();
                summaries
                    .iter()
                    .filter(move |(path, _)| {
                        include_ignored || worktree.entry_for_path(path.as_ref()).is_some_and(|entry| !entry.is_ignored)
                    })
                    .flat_map(move |(path, summaries)| {
                        summaries.iter().map(move |(server_id, summary)| {
                            (ProjectPath { worktree_id, path: Arc::clone(path) }, *server_id, *summary)
                        })
                    })
            })
    }

    pub(super) fn invalidate_diagnostic_summaries_for_removed_entries(
        &mut self,
        worktree_id: WorktreeId,
        changes: &UpdatedEntriesSet,
        cx: &mut Context<Self>,
    ) {
        // A removed path takes its stored diagnostics with it, or reopening a file
        // recreated at the same path replays them from the worktree store via
        // initialize_buffer, showing verdicts about content that no longer exists.
        if let Some(diagnostics_for_tree) = self.local.diagnostics.get_mut(&worktree_id) {
            for (path, _, _) in changes.iter().filter(|(_, _, change)| *change == PathChange::Removed) {
                diagnostics_for_tree.remove(path);
            }
        }

        let Some(summaries_for_tree) = self.diagnostic_summaries.get_mut(&worktree_id) else {
            return;
        };

        let mut cleared_paths: Vec<ProjectPath> = Vec::new();
        let mut cleared_server_ids: FxHashSet<LanguageServerId> = FxHashSet::default();

        for (path, _, _) in changes.iter().filter(|(_, _, change)| *change == PathChange::Removed) {
            if let Some(summaries_by_server_id) = summaries_for_tree.remove(path) {
                for server_id in summaries_by_server_id.keys() {
                    cleared_server_ids.insert(*server_id);
                }
                cleared_paths.push(ProjectPath { worktree_id, path: Arc::clone(path) });
            }
        }

        if !cleared_paths.is_empty() {
            for server_id in cleared_server_ids {
                cx.emit(LspStoreEvent::DiagnosticsUpdated { server_id, paths: cleared_paths.clone() });
            }
        }
    }

    // A path's diagnostics are dropped per server, through the same merge machinery that
    // applies them: buffer sets, worktree diagnostics, summaries and the DiagnosticsUpdated
    // event all follow from one call.
    pub(super) fn clear_path_diagnostics_for_servers(
        &mut self,
        abs_path: PathBuf,
        server_ids: Vec<LanguageServerId>,
        retain_diagnostic: impl Fn(&lsp_types::Uri, &Diagnostic, &App) -> bool + Clone,
        cx: &mut Context<Self>,
    ) {
        if server_ids.is_empty() {
            return;
        }
        let diagnostic_updates = server_ids
            .into_iter()
            .map(|server_id| DocumentDiagnosticsUpdate {
                diagnostics: DocumentDiagnostics {
                    document_abs_path: abs_path.clone(),
                    version: None,
                    diagnostics: Vec::new(),
                },
                result_id: None,
                registration_id: None,
                server_id,
                disk_based_sources: Cow::Borrowed(&[]),
            })
            .collect::<Vec<_>>();
        self.merge_diagnostic_entries(diagnostic_updates, retain_diagnostic, cx)
            .context("clearing path diagnostics")
            .log_err();
    }

    fn merge_diagnostic_entries(
        &mut self,
        diagnostic_updates: Vec<DocumentDiagnosticsUpdate<'_, DocumentDiagnostics>>,
        merge: impl Fn(&lsp_types::Uri, &Diagnostic, &App) -> bool + Clone,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let mut updated_diagnostics_paths = FxHashMap::default();
        for mut update in diagnostic_updates {
            let abs_path = &update.diagnostics.document_abs_path;
            let server_id = update.server_id;
            let Some((worktree, relative_path)) = self.worktree_store.read(cx).find_worktree(abs_path, cx) else {
                log::warn!("skipping diagnostics update, no worktree found for path {abs_path:?}");
                continue;
            };

            let worktree_id = worktree.read(cx).id();
            let project_path = ProjectPath { worktree_id, path: relative_path };

            let document_uri = lsp_types::Uri::from_file_path(abs_path)
                .map_err(|()| anyhow!("Failed to convert buffer path {abs_path:?} to lsp Uri"))?;
            if let Some(buffer_handle) = self.buffer_store.read(cx).get_by_path(&project_path) {
                let snapshot = buffer_handle.read(cx).snapshot();
                let buffer = buffer_handle.read(cx);
                let reused_diagnostics = buffer
                    .buffer_diagnostics(Some(server_id))
                    .iter()
                    .filter(|v| merge(&document_uri, &v.diagnostic, cx))
                    .map(|v| {
                        (*v).clone().map_coordinates(|range| {
                            Unclipped(range.start.to_point_utf16(&snapshot))
                                ..Unclipped(range.end.to_point_utf16(&snapshot))
                        })
                    })
                    .collect::<Vec<_>>();

                self.local.update_buffer_diagnostics(
                    &buffer_handle,
                    server_id,
                    update.diagnostics.version,
                    update.diagnostics.diagnostics.clone(),
                    reused_diagnostics.clone(),
                    cx,
                )?;
                if let Some(abs_path) = File::from_dyn(buffer_handle.read(cx).file()).map(|f| f.abs_path(cx)) {
                    self.local
                        .buffer_pull_diagnostics_result_ids
                        .entry(server_id)
                        .or_default()
                        .entry(update.registration_id)
                        .or_default()
                        .insert(abs_path, update.result_id);
                }

                update.diagnostics.diagnostics.extend(reused_diagnostics);
            } else {
                let local = &self.local;
                let reused_diagnostics = local
                    .diagnostics
                    .get(&worktree_id)
                    .and_then(|diagnostics_for_tree| diagnostics_for_tree.get(&project_path.path))
                    .and_then(|diagnostics_by_server_id| {
                        diagnostics_by_server_id
                            .binary_search_by_key(&server_id, |e| e.0)
                            .ok()
                            .map(|ix| &diagnostics_by_server_id[ix].1)
                    })
                    .into_iter()
                    .flatten()
                    .filter(|v| merge(&document_uri, &v.diagnostic, cx));

                update.diagnostics.diagnostics.extend(reused_diagnostics.cloned());
            }

            let updated = worktree.update(cx, |worktree, cx| {
                self.update_worktree_diagnostics(
                    worktree.id(),
                    server_id,
                    Arc::clone(&project_path.path),
                    update.diagnostics.diagnostics,
                    cx,
                )
            });
            if updated {
                updated_diagnostics_paths.entry(server_id).or_insert_with(Vec::new).push(project_path);
            }
        }

        for (server_id, paths) in updated_diagnostics_paths {
            cx.emit(LspStoreEvent::DiagnosticsUpdated { server_id, paths });
        }
        Ok(())
    }

    fn update_worktree_diagnostics(
        &mut self,
        worktree_id: WorktreeId,
        server_id: LanguageServerId,
        path_in_worktree: Arc<RelPath>,
        diagnostics: Vec<DiagnosticEntry<Unclipped<PointUtf16>>>,
        _: &mut Context<Worktree>,
    ) -> bool {
        let local = &mut self.local;

        let summaries_for_tree = self.diagnostic_summaries.entry(worktree_id).or_default();
        let diagnostics_for_tree = local.diagnostics.entry(worktree_id).or_default();
        let summaries_by_server_id = summaries_for_tree.entry(Arc::clone(&path_in_worktree)).or_default();

        let old_summary = summaries_by_server_id.remove(&server_id).unwrap_or_default();

        let new_summary = DiagnosticSummary::new(&diagnostics);
        if diagnostics.is_empty() {
            if let Some(diagnostics_by_server_id) = diagnostics_for_tree.get_mut(&path_in_worktree) {
                if let Ok(ix) = diagnostics_by_server_id.binary_search_by_key(&server_id, |e| e.0) {
                    diagnostics_by_server_id.remove(ix);
                }
                if diagnostics_by_server_id.is_empty() {
                    diagnostics_for_tree.remove(&path_in_worktree);
                }
            }
        } else {
            summaries_by_server_id.insert(server_id, new_summary);
            let diagnostics_by_server_id = diagnostics_for_tree.entry(Arc::clone(&path_in_worktree)).or_default();
            match diagnostics_by_server_id.binary_search_by_key(&server_id, |e| e.0) {
                Ok(ix) => {
                    diagnostics_by_server_id[ix] = (server_id, diagnostics);
                }
                Err(ix) => {
                    diagnostics_by_server_id.insert(ix, (server_id, diagnostics));
                }
            }
        }

        !old_summary.is_empty() || !new_summary.is_empty()
    }

    pub(crate) fn disk_based_diagnostics_started(
        &mut self,
        language_server_id: LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        if let Some(language_server_status) = self.language_server_statuses.get_mut(&language_server_id) {
            language_server_status.has_pending_diagnostic_updates = true;
        }

        cx.emit(LspStoreEvent::DiskBasedDiagnosticsStarted { language_server_id });
    }

    pub(crate) fn disk_based_diagnostics_finished(
        &mut self,
        language_server_id: LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        if let Some(language_server_status) = self.language_server_statuses.get_mut(&language_server_id) {
            language_server_status.has_pending_diagnostic_updates = false;
        }

        cx.emit(LspStoreEvent::DiskBasedDiagnosticsFinished { language_server_id });
    }

    // After saving a buffer using a language server that doesn't provide a disk-based progress token,
    // kick off a timer that will reset every time the buffer is saved. If the timer eventually fires,
    // simulate disk-based diagnostics being finished so that other pieces of UI (e.g., project
    // diagnostics view, diagnostic status bar) can update. We don't emit an event right away because
    // the language server might take some time to publish diagnostics.
    pub(super) fn simulate_disk_based_diagnostics_events_if_needed(
        &mut self,
        language_server_id: LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        const DISK_BASED_DIAGNOSTICS_DEBOUNCE: Duration = Duration::from_secs(1);

        let Some(LanguageServerState::Running { simulate_disk_based_diagnostics_completion, adapter, .. }) =
            self.local.language_servers.get_mut(&language_server_id)
        else {
            return;
        };

        if adapter.disk_based_diagnostics_progress_token.is_some() {
            return;
        }

        let prev_task = simulate_disk_based_diagnostics_completion.replace(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DISK_BASED_DIAGNOSTICS_DEBOUNCE).await;

            this.update(cx, |this, cx| {
                this.disk_based_diagnostics_finished(language_server_id, cx);

                if let Some(LanguageServerState::Running { simulate_disk_based_diagnostics_completion, .. }) =
                    this.local.language_servers.get_mut(&language_server_id)
                {
                    *simulate_disk_based_diagnostics_completion = None;
                }
            })
            .ok();
        }));

        if prev_task.is_none() {
            self.disk_based_diagnostics_started(language_server_id, cx);
        }
    }

    pub(crate) fn merge_lsp_diagnostics(
        &mut self,
        source_kind: DiagnosticSourceKind,
        lsp_diagnostics: Vec<DocumentDiagnosticsUpdate<lsp_types::PublishDiagnosticsParams>>,
        merge: impl Fn(&lsp_types::Uri, &Diagnostic, &App) -> bool + Clone,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let updates = lsp_diagnostics
            .into_iter()
            .filter_map(|update| {
                let abs_path = update.diagnostics.uri.to_file_path().ok()?;
                Some(DocumentDiagnosticsUpdate {
                    diagnostics: self.lsp_to_document_diagnostics(
                        abs_path,
                        source_kind,
                        update.server_id,
                        update.diagnostics,
                        &update.disk_based_sources,
                        update.registration_id.clone(),
                    ),
                    result_id: update.result_id,
                    server_id: update.server_id,
                    disk_based_sources: update.disk_based_sources,
                    registration_id: update.registration_id,
                })
            })
            .collect();
        self.merge_diagnostic_entries(updates, merge, cx)?;
        Ok(())
    }

    fn lsp_to_document_diagnostics(
        &mut self,
        document_abs_path: PathBuf,
        source_kind: DiagnosticSourceKind,
        server_id: LanguageServerId,
        mut lsp_diagnostics: lsp_types::PublishDiagnosticsParams,
        disk_based_sources: &[String],
        registration_id: Option<SharedString>,
    ) -> DocumentDiagnostics {
        let mut diagnostics = Vec::default();
        let mut primary_diagnostic_group_ids = FxHashMap::default();
        let mut sources_by_group_id = FxHashMap::default();
        let mut supporting_diagnostics = FxHashMap::default();

        let adapter = self.language_server_adapter_for_id(server_id);

        // Ensure that primary diagnostics are always the most severe
        lsp_diagnostics.diagnostics.sort_by_key(|item| item.severity);

        for diagnostic in &lsp_diagnostics.diagnostics {
            let source = diagnostic.source.as_ref();
            let range = range_from_lsp(diagnostic.range);
            let is_supporting = diagnostic.related_information.as_ref().is_some_and(|infos| {
                infos.iter().any(|info| {
                    primary_diagnostic_group_ids.contains_key(&(
                        source,
                        diagnostic.code.clone(),
                        range_from_lsp(info.location.range),
                    ))
                })
            });

            let is_unnecessary =
                diagnostic.tags.as_ref().is_some_and(|tags| tags.contains(&DiagnosticTag::UNNECESSARY));

            if is_supporting {
                supporting_diagnostics.insert(
                    (source, diagnostic.code.clone(), range),
                    (
                        diagnostic.severity,
                        is_unnecessary,
                        related_information_from_lsp(diagnostic, &lsp_diagnostics.uri),
                    ),
                );
            } else {
                let group_id = post_inc(&mut self.local.next_diagnostic_group_id);
                let is_disk_based = source.is_some_and(|source| disk_based_sources.contains(source));

                sources_by_group_id.insert(group_id, source);
                primary_diagnostic_group_ids.insert((source, diagnostic.code.clone(), range.clone()), group_id);

                let message = match &diagnostic.message {
                    lsp_types::DiagnosticMessage::String(message) => DiagnosticMessage::plain_with_adapter_markdown(
                        message.trim(),
                        adapter
                            .as_ref()
                            .and_then(|adapter| adapter.diagnostic_message_to_markdown(message))
                            .map(SharedString::from),
                    ),
                    lsp_types::DiagnosticMessage::MarkupContent(markup) => DiagnosticMessage::from_lsp_markup(markup),
                };
                diagnostics.push(DiagnosticEntry {
                    range,
                    related_information: related_information_from_lsp(diagnostic, &lsp_diagnostics.uri),
                    diagnostic: Diagnostic {
                        source: diagnostic.source.clone(),
                        source_kind,
                        code: diagnostic.code.clone(),
                        code_description: diagnostic.code_description.as_ref().and_then(|d| d.href.clone()),
                        severity: diagnostic.severity.unwrap_or(DiagnosticSeverity::ERROR),
                        message,
                        group_id,
                        is_primary: true,
                        is_disk_based,
                        is_unnecessary,
                        underline: true,
                        data: diagnostic.data.clone(),
                        registration_id: registration_id.clone(),
                    },
                });
                if let Some(infos) = &diagnostic.related_information {
                    for info in infos {
                        if info.location.uri == lsp_diagnostics.uri && !info.message.is_empty() {
                            let range = range_from_lsp(info.location.range);
                            diagnostics.push(DiagnosticEntry {
                                range,
                                related_information: None,
                                diagnostic: Diagnostic {
                                    source: diagnostic.source.clone(),
                                    source_kind,
                                    code: diagnostic.code.clone(),
                                    code_description: diagnostic.code_description.as_ref().and_then(|d| d.href.clone()),
                                    severity: DiagnosticSeverity::INFORMATION,
                                    message: DiagnosticMessage::plain_with_adapter_markdown(
                                        info.message.trim(),
                                        adapter
                                            .as_ref()
                                            .and_then(|adapter| adapter.diagnostic_message_to_markdown(&info.message))
                                            .map(SharedString::from),
                                    ),
                                    group_id,
                                    is_primary: false,
                                    is_disk_based,
                                    is_unnecessary: false,
                                    underline: true,
                                    data: diagnostic.data.clone(),
                                    registration_id: registration_id.clone(),
                                },
                            });
                        }
                    }
                }
            }
        }

        for entry in &mut diagnostics {
            if !entry.diagnostic.is_primary {
                let source = sources_by_group_id[&entry.diagnostic.group_id];
                if let Some((severity, is_unnecessary, related_information)) =
                    supporting_diagnostics.get(&(source, entry.diagnostic.code.clone(), entry.range.clone()))
                {
                    if let Some(severity) = severity {
                        entry.diagnostic.severity = *severity;
                    }
                    entry.diagnostic.is_unnecessary = *is_unnecessary;
                    entry.related_information = related_information.clone();
                }
            }
        }

        DocumentDiagnostics { diagnostics, document_abs_path, version: lsp_diagnostics.version }
    }

    pub(crate) fn language_servers_running_disk_based_diagnostics(
        &self,
    ) -> impl Iterator<Item = LanguageServerId> + '_ {
        self.language_server_statuses
            .iter()
            .filter_map(|(id, status)| status.has_pending_diagnostic_updates.then_some(*id))
    }

    // The map is keyed by `Option<SharedString>`; taking `Option<&SharedString>` would force a clone for the lookup.
    #[allow(clippy::ref_option)]
    fn result_id_for_buffer_pull(
        &self,
        server_id: LanguageServerId,
        buffer_id: BufferId,
        registration_id: &Option<SharedString>,
        cx: &App,
    ) -> Option<SharedString> {
        let abs_path = self
            .buffer_store
            .read(cx)
            .get(buffer_id)
            .and_then(|b| File::from_dyn(b.read(cx).file()))
            .map(|f| f.abs_path(cx))?;
        self.local.buffer_pull_diagnostics_result_ids.get(&server_id)?.get(registration_id)?.get(&abs_path)?.clone()
    }

    /// Gets all result_ids for a workspace diagnostics pull request.
    /// First, it tries to find buffer's result_id retrieved via the diagnostics pull; if it fails, it falls back to the workspace disagnostics pull result_id.
    /// The latter is supposed to be of lower priority as we keep on pulling diagnostics for open buffers eagerly.
    // The map is keyed by `Option<String>`; taking `Option<&String>` would force a clone for the lookup.
    #[allow(clippy::ref_option)]
    fn set_workspace_refresh_token(
        &mut self,
        server_id: LanguageServerId,
        registration_id: &Option<String>,
        token: Option<String>,
    ) {
        if let Some(workspace_diagnostics) =
            self.local.language_servers.get_mut(&server_id).and_then(|state| match state {
                LanguageServerState::Running { workspace_diagnostics_refresh_tasks, .. } => {
                    workspace_diagnostics_refresh_tasks.get_mut(registration_id)
                }
                LanguageServerState::Starting { .. } => None,
            })
        {
            workspace_diagnostics.current_request_token = token;
        }
    }

    // The map is keyed by `Option<SharedString>`; taking `Option<&SharedString>` would force a clone for the lookup.
    #[allow(clippy::ref_option)]
    fn result_ids_for_workspace_refresh(
        &self,
        server_id: LanguageServerId,
        registration_id: &Option<SharedString>,
    ) -> FxHashMap<PathBuf, SharedString> {
        let local = &self.local;
        local
            .workspace_pull_diagnostics_result_ids
            .get(&server_id)
            .into_iter()
            .filter_map(|diagnostics| diagnostics.get(registration_id))
            .flatten()
            .filter_map(|(abs_path, result_id)| {
                let result_id = local
                    .buffer_pull_diagnostics_result_ids
                    .get(&server_id)
                    .and_then(|buffer_ids_result_ids| buffer_ids_result_ids.get(registration_id)?.get(abs_path))
                    .cloned()
                    .flatten()
                    .or_else(|| result_id.clone())?;
                Some((abs_path.clone(), result_id))
            })
            .collect()
    }

    pub(crate) fn pull_workspace_diagnostics(&mut self, server_id: LanguageServerId) {
        if let Some(LanguageServerState::Running { workspace_diagnostics_refresh_tasks, .. }) =
            self.local.language_servers.get_mut(&server_id)
        {
            for diagnostics in workspace_diagnostics_refresh_tasks.values_mut() {
                diagnostics.refresh_tx.try_send(None).ok();
            }
        }
    }

    /// Refreshes `textDocument/diagnostic` for all open buffers associated with the given server.
    /// This is called in response to `workspace/diagnostic/refresh` to comply with the LSP spec,
    /// which requires refreshing both workspace and document diagnostics.
    pub(crate) fn pull_document_diagnostics_for_server(
        &mut self,
        server_id: LanguageServerId,
        source_buffer_id: Option<BufferId>,
        cx: &Context<Self>,
    ) -> Shared<Task<()>> {
        let local = &mut self.local;
        let mut buffers_to_refresh = FxHashSet::default();
        for (buffer_id, server_ids) in &local.buffers_opened_in_servers {
            if server_ids.contains(&server_id) && Some(buffer_id) != source_buffer_id.as_ref() {
                buffers_to_refresh.insert(*buffer_id);
            }
        }

        self.refresh_background_diagnostics_for_buffers(buffers_to_refresh, cx)
    }

    pub fn pull_document_diagnostics_for_buffer_edit(&mut self, buffer_id: BufferId, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffer_store.read(cx).get(buffer_id) else {
            return;
        };
        let Some(language_servers) = self.language_server_ids_for_opened_buffer(buffer_id).cloned() else {
            return;
        };
        for server_id in language_servers {
            let inter_file_dependencies = self.text_document_capability_matches_for_server(
                &buffer,
                server_id,
                "textDocument/diagnostic",
                |capabilities| {
                    capabilities
                        .server_capabilities
                        .diagnostic_provider
                        .as_ref()
                        .is_some_and(diagnostics_inter_file_dependencies)
                },
                cx,
            );
            if inter_file_dependencies {
                drop(self.pull_document_diagnostics_for_server(server_id, Some(buffer_id), cx));
            }
        }
    }

    pub(super) fn apply_workspace_diagnostic_report(
        &mut self,
        server_id: LanguageServerId,
        report: lsp_types::WorkspaceDiagnosticReportResult,
        registration_id: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let mut workspace_diagnostics =
            GetDocumentDiagnostics::deserialize_workspace_diagnostics_report(report, server_id, registration_id);
        workspace_diagnostics.retain(|d| match &d.diagnostics {
            LspPullDiagnostics::Response { server_id, registration_id, .. } => {
                self.diagnostic_registration_exists(*server_id, registration_id.as_ref())
            }
            LspPullDiagnostics::Default => false,
        });
        let mut unchanged_buffers = FxHashMap::default();
        let workspace_diagnostics_updates = workspace_diagnostics
            .into_iter()
            .filter_map(|workspace_diagnostics| match workspace_diagnostics.diagnostics {
                LspPullDiagnostics::Response { server_id, uri, diagnostics, registration_id } => {
                    Some((server_id, uri, diagnostics, workspace_diagnostics.version, registration_id))
                }
                LspPullDiagnostics::Default => None,
            })
            .fold(FxHashMap::default(), |mut acc, (server_id, uri, diagnostics, version, new_registration_id)| {
                let (result_id, diagnostics) = match diagnostics {
                    PulledDiagnostics::Unchanged { result_id } => {
                        unchanged_buffers
                            .entry(new_registration_id.clone())
                            .or_insert_with(FxHashSet::default)
                            .insert(uri.clone());
                        (Some(result_id), Vec::new())
                    }
                    PulledDiagnostics::Changed { result_id, diagnostics } => (result_id, diagnostics),
                };
                let disk_based_sources = Cow::Owned(
                    self.language_server_adapter_for_id(server_id)
                        .as_ref()
                        .map_or(&[][..], |adapter| adapter.disk_based_diagnostic_sources.as_slice())
                        .to_vec(),
                );

                let Some(abs_path) = uri.to_file_path().ok() else {
                    return acc;
                };
                let Some((worktree, relative_path)) = self.worktree_store.read(cx).find_worktree(abs_path.clone(), cx)
                else {
                    log::warn!("skipping workspace diagnostics update, no worktree found for path {abs_path:?}");
                    return acc;
                };
                let worktree_id = worktree.read(cx).id();
                let project_path = ProjectPath { worktree_id, path: relative_path };
                {
                    let local_lsp_store = &mut self.local;
                    local_lsp_store
                        .workspace_pull_diagnostics_result_ids
                        .entry(server_id)
                        .or_default()
                        .entry(new_registration_id.clone())
                        .or_default()
                        .insert(abs_path, result_id.clone());
                }
                // The LSP spec recommends that "diagnostics from a document pull should win over diagnostics from a workspace pull."
                // Since we actively pull diagnostics for documents with open buffers, we ignore contents of workspace pulls for these documents.
                if self.buffer_store.read(cx).get_by_path(&project_path).is_none() {
                    acc.entry(server_id)
                        .or_insert_with(FxHashMap::default)
                        .entry(new_registration_id.clone())
                        .or_insert_with(Vec::new)
                        .push(DocumentDiagnosticsUpdate {
                            server_id,
                            diagnostics: lsp_types::PublishDiagnosticsParams { uri, diagnostics, version },
                            result_id: result_id.map(SharedString::new),
                            disk_based_sources,
                            registration_id: new_registration_id,
                        });
                }
                acc
            });

        for diagnostic_updates in workspace_diagnostics_updates.into_values() {
            for (registration_id, diagnostic_updates) in diagnostic_updates {
                self.merge_lsp_diagnostics(
                    DiagnosticSourceKind::Pulled,
                    diagnostic_updates,
                    |document_uri, old_diagnostic, _| match old_diagnostic.source_kind {
                        DiagnosticSourceKind::Pulled => {
                            old_diagnostic.registration_id != registration_id
                                || unchanged_buffers
                                    .get(&old_diagnostic.registration_id)
                                    .is_some_and(|unchanged_buffers| unchanged_buffers.contains(document_uri))
                        }
                        DiagnosticSourceKind::Other | DiagnosticSourceKind::Pushed => true,
                    },
                    cx,
                )
                .log_err();
            }
        }
    }

    pub(super) fn clear_unregistered_diagnostics(
        &mut self,
        server_id: LanguageServerId,
        cleared_registration_id: SharedString,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let mut affected_abs_paths: FxHashSet<PathBuf> = FxHashSet::default();

        self.buffer_store.update(cx, |buffer_store, cx| {
            for buffer_handle in buffer_store.buffers() {
                let buffer = buffer_handle.read(cx);
                let abs_path = File::from_dyn(buffer.file()).map(|f| f.abs_path(cx));
                let Some(abs_path) = abs_path else {
                    continue;
                };
                affected_abs_paths.insert(abs_path);
            }
        });

        let local = &self.local;
        for (worktree_id, diagnostics_for_tree) in &local.diagnostics {
            let Some(worktree) = self.worktree_store.read(cx).worktree_for_id(*worktree_id, cx) else {
                continue;
            };

            for (rel_path, diagnostics_by_server_id) in diagnostics_for_tree {
                if let Ok(ix) = diagnostics_by_server_id.binary_search_by_key(&server_id, |e| e.0) {
                    let has_matching_registration = diagnostics_by_server_id[ix]
                        .1
                        .iter()
                        .any(|entry| entry.diagnostic.registration_id.as_ref() == Some(&cleared_registration_id));
                    if has_matching_registration {
                        let abs_path = worktree.read(cx).absolutize(rel_path);
                        affected_abs_paths.insert(abs_path);
                    }
                }
            }
        }

        if affected_abs_paths.is_empty() {
            return Ok(());
        }

        // Send a fake diagnostic update which clears the state for the registration ID
        let clears: Vec<DocumentDiagnosticsUpdate<'static, DocumentDiagnostics>> = affected_abs_paths
            .into_iter()
            .map(|abs_path| DocumentDiagnosticsUpdate {
                diagnostics: DocumentDiagnostics {
                    diagnostics: Vec::new(),
                    document_abs_path: abs_path,
                    version: None,
                },
                result_id: None,
                registration_id: Some(cleared_registration_id.clone()),
                server_id,
                disk_based_sources: Cow::Borrowed(&[]),
            })
            .collect();

        let merge_registration_id = cleared_registration_id.clone();
        self.merge_diagnostic_entries(
            clears,
            move |_, diagnostic, _| {
                if diagnostic.source_kind == DiagnosticSourceKind::Pulled {
                    diagnostic.registration_id != Some(merge_registration_id.clone())
                } else {
                    true
                }
            },
            cx,
        )?;

        Ok(())
    }
}
