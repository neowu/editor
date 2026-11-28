use std::convert::TryInto as _;
use std::ops::ControlFlow;
use std::path;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

use anyhow::Context as _;
use anyhow::Result;
use fs::Fs;
use fs::PathEvent;
use fs::PathEventKind;
use futures::StreamExt as _;
use globset::Glob;
use globset::GlobBuilder;
use globset::GlobMatcher;
use globset::GlobSet;
use globset::GlobSetBuilder;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use lsp::LSP_REQUEST_TIMEOUT;
use lsp::LanguageServerId;
use lsp_types::DidChangeWatchedFilesRegistrationOptions;
use lsp_types::FileOperationFilter;
use lsp_types::FileOperationPatternKind;
use lsp_types::FileOperationRegistrationOptions;
use lsp_types::FileRename;
use lsp_types::FileSystemWatcher;
use lsp_types::RenameFilesParams;
use lsp_types::notification::DidChangeWatchedFiles;
use lsp_types::notification::DidRenameFiles;
use lsp_types::request::WillRenameFiles;
use rustc_hash::FxHashMap;
use util::ResultExt as _;
use util::paths::SanitizedPath;
use util::rel_path::RelPath;
use worktree::PathChange;
use worktree::ProjectEntryId;
use worktree::Worktree;
use worktree::WorktreeId;

use super::LanguageServerState;
use super::LocalLspStore;
use super::LspStore;
use crate::ProjectPath;
use crate::ProjectTransaction;
use crate::lsp_command::file_path_to_lsp_url;

#[derive(Default)]
pub(super) struct RenamePathsWatchedForServer {
    did_rename: Vec<RenameActionPredicate>,
    will_rename: Vec<RenameActionPredicate>,
}

impl RenamePathsWatchedForServer {
    pub(super) fn with_did_rename_patterns(mut self, did_rename: Option<&FileOperationRegistrationOptions>) -> Self {
        if let Some(did_rename) = did_rename {
            self.did_rename = did_rename.filters.iter().filter_map(|filter| filter.try_into().log_err()).collect();
        }
        self
    }
    pub(super) fn with_will_rename_patterns(mut self, will_rename: Option<&FileOperationRegistrationOptions>) -> Self {
        if let Some(will_rename) = will_rename {
            self.will_rename = will_rename.filters.iter().filter_map(|filter| filter.try_into().log_err()).collect();
        }
        self
    }

    fn should_send_did_rename(&self, path: &str, is_dir: bool) -> bool {
        self.did_rename.iter().any(|pred| pred.eval(path, is_dir))
    }
    fn should_send_will_rename(&self, path: &str, is_dir: bool) -> bool {
        self.will_rename.iter().any(|pred| pred.eval(path, is_dir))
    }
}

impl TryFrom<&FileOperationFilter> for RenameActionPredicate {
    type Error = globset::Error;
    fn try_from(ops: &FileOperationFilter) -> Result<Self, globset::Error> {
        Ok(Self {
            kind: ops.pattern.matches.clone(),
            glob: GlobBuilder::new(&ops.pattern.glob)
                .case_insensitive(ops.pattern.options.as_ref().is_some_and(|ops| ops.ignore_case.unwrap_or(false)))
                .build()?
                .compile_matcher(),
        })
    }
}

struct RenameActionPredicate {
    glob: GlobMatcher,
    kind: Option<FileOperationPatternKind>,
}

impl RenameActionPredicate {
    // Returns true if language server should be notified
    fn eval(&self, path: &str, is_dir: bool) -> bool {
        self.kind.as_ref().is_none_or(|kind| {
            let expected_kind = if is_dir { FileOperationPatternKind::Folder } else { FileOperationPatternKind::File };
            kind == &expected_kind
        }) && self.glob.is_match(path)
    }
}

#[derive(Default)]
pub(super) struct LanguageServerWatchedPaths {
    worktree_paths: FxHashMap<WorktreeId, LazyGlobSet>,
    abs_paths: FxHashMap<Arc<Path>, (LazyGlobSet, Task<()>)>,
}

#[derive(Default)]
struct LazyGlobSet {
    /// Globs keyed by registration ID.
    globs: FxHashMap<String, Vec<Glob>>,
    /// Compiled from `globs`, lazily on `is_match`. `None` when stale.
    compiled: Option<GlobSet>,
}

impl LazyGlobSet {
    fn add(&mut self, registration_id: &str, glob: Glob) {
        self.globs.entry(registration_id.to_owned()).or_default().push(glob);
        self.compiled = None;
    }

    fn remove(&mut self, registration_id: &str) {
        if self.globs.remove(registration_id).is_some() {
            self.compiled = None;
        }
    }

    fn is_empty(&self) -> bool {
        self.globs.is_empty()
    }

    fn is_match<P: AsRef<Path>>(&mut self, path: P) -> bool {
        let compiled = self.compiled.get_or_insert_with(|| {
            let mut builder = GlobSetBuilder::new();
            for glob in self.globs.values().flatten() {
                builder.add(glob.clone());
            }
            builder.build().log_err().unwrap_or_default()
        });
        compiled.is_match(path)
    }
}

impl LanguageServerWatchedPaths {
    fn spawn_abs_path_watcher(
        abs_path: Arc<Path>,
        fs: Arc<Fs>,
        language_server_id: LanguageServerId,
        cx: &Context<LspStore>,
    ) -> Task<()> {
        let lsp_store = cx.weak_entity();
        const LSP_ABS_PATH_OBSERVE: Duration = Duration::from_millis(100);

        cx.spawn({
            async move |_, cx| {
                async move {
                    let mut push_updates = cx
                        .background_spawn({
                            let abs_path = Arc::clone(&abs_path);
                            async move { fs.watch(&abs_path, LSP_ABS_PATH_OBSERVE).await }
                        })
                        .await;
                    while let Some(update) = push_updates.0.next().await {
                        let action = lsp_store
                            .update(cx, |this, _| {
                                let local = &mut this.local;
                                let Some(watcher) = local.language_server_watched_paths.get_mut(&language_server_id)
                                else {
                                    return ControlFlow::Break(());
                                };
                                let Some((globs, _)) = watcher.abs_paths.get_mut(&abs_path) else {
                                    return ControlFlow::Break(());
                                };
                                let matching_entries =
                                    update.into_iter().filter(|event| globs.is_match(&event.path)).collect::<Vec<_>>();
                                this.lsp_notify_abs_paths_changed(language_server_id, matching_entries);
                                ControlFlow::Continue(())
                            })
                            .ok()?;

                        if action.is_break() {
                            break;
                        }
                    }
                    Some(())
                }
                .await;
            }
        })
    }
}

/// Whether a worktree change is a Rust source edit rust-analyzer won't re-check on its own.
/// Open buffers are covered by their reload; ignored paths (`target/`) are flycheck's own output.
fn disk_change_needs_flycheck(path: &RelPath, change: PathChange, is_ignored: bool, has_open_buffer: bool) -> bool {
    !matches!(change, PathChange::Loaded) && !is_ignored && !has_open_buffer && path.extension() == Some("rs")
}

fn glob_literal_prefix(glob: &Path) -> PathBuf {
    glob.components()
        .take_while(|component| match component {
            path::Component::Normal(part) => !part.to_string_lossy().contains(['*', '?', '{', '}']),
            _ => true,
        })
        .collect()
}

impl LocalLspStore {
    fn register_watcher(
        &mut self,
        worktrees: &[Entity<Worktree>],
        watcher: &FileSystemWatcher,
        registration_id: &str,
        language_server_id: LanguageServerId,
        cx: &mut Context<LspStore>,
    ) {
        let watched = self.language_server_watched_paths.entry(language_server_id).or_default();

        if let Some((worktree, literal_prefix, pattern)) =
            Self::worktree_and_path_for_file_watcher(worktrees, watcher, cx)
        {
            if let Some(glob) = Glob::new(&pattern).log_err() {
                let worktree_id = worktree.read(cx).id();
                watched.worktree_paths.entry(worktree_id).or_default().add(registration_id, glob);
                worktree.update(cx, |worktree, _| {
                    drop(worktree.add_path_prefix_to_scan(literal_prefix));
                });
            }

            return;
        }

        let (path, pattern) = match &watcher.glob_pattern {
            lsp_types::GlobPattern::String(s) => {
                let watcher_path = SanitizedPath::new(s);
                let path = glob_literal_prefix(watcher_path.as_path());
                let pattern = watcher_path.as_path().strip_prefix(&path).map_or_else(
                    |e| {
                        log::error!(
                            "Failed to strip prefix for string pattern: {}, with prefix: {}, with error: {}",
                            s,
                            path.display(),
                            e
                        );
                        watcher_path.as_path().to_string_lossy().into_owned()
                    },
                    |p| p.to_string_lossy().into_owned(),
                );
                (path, pattern)
            }
            lsp_types::GlobPattern::Relative(rp) => {
                let Ok(mut base_uri) = match &rp.base_uri {
                    lsp_types::OneOf::Left(workspace_folder) => &workspace_folder.uri,
                    lsp_types::OneOf::Right(base_uri) => base_uri,
                }
                .to_file_path() else {
                    return;
                };

                let path = glob_literal_prefix(Path::new(&rp.pattern));
                let pattern = Path::new(&rp.pattern).strip_prefix(&path).map_or_else(
                    |e| {
                        log::error!(
                            "Failed to strip prefix for relative pattern: {}, with prefix: {}, with error: {}",
                            rp.pattern,
                            path.display(),
                            e
                        );
                        rp.pattern.clone()
                    },
                    |p| p.to_string_lossy().into_owned(),
                );
                base_uri.push(path);
                (base_uri, pattern)
            }
        };

        if let Some(glob) = Glob::new(&pattern).log_err() {
            if path.components().any(|c| matches!(c, path::Component::Normal(_))) {
                let abs_path: Arc<Path> = path.into();
                let fs = Arc::clone(&self.fs);
                let entry = watched.abs_paths.entry(Arc::clone(&abs_path)).or_insert_with(|| {
                    let task = LanguageServerWatchedPaths::spawn_abs_path_watcher(abs_path, fs, language_server_id, cx);
                    (LazyGlobSet::default(), task)
                });
                entry.0.add(registration_id, glob);
            } else {
                // For an unrooted glob like `**/Cargo.toml`, watch it within each worktree,
                // rather than adding a new watcher for `/`.
                for worktree in worktrees {
                    watched
                        .worktree_paths
                        .entry(worktree.read(cx).id())
                        .or_default()
                        .add(registration_id, glob.clone());
                }
            }
        }
    }

    fn worktree_and_path_for_file_watcher(
        worktrees: &[Entity<Worktree>],
        watcher: &FileSystemWatcher,
        cx: &App,
    ) -> Option<(Entity<Worktree>, Arc<RelPath>, String)> {
        worktrees.iter().find_map(|worktree| {
            let tree = worktree.read(cx);
            let worktree_root_path = tree.abs_path();
            match &watcher.glob_pattern {
                lsp_types::GlobPattern::String(s) => {
                    let watcher_path = SanitizedPath::new(s);
                    let relative = watcher_path.as_path().strip_prefix(&worktree_root_path).ok()?;
                    let literal_prefix = glob_literal_prefix(relative);
                    Some((
                        worktree.clone(),
                        RelPath::new(&literal_prefix).ok()?.into_arc(),
                        relative.to_string_lossy().into_owned(),
                    ))
                }
                lsp_types::GlobPattern::Relative(rp) => {
                    let base_uri = match &rp.base_uri {
                        lsp_types::OneOf::Left(workspace_folder) => &workspace_folder.uri,
                        lsp_types::OneOf::Right(base_uri) => base_uri,
                    }
                    .to_file_path()
                    .ok()?;
                    let relative = base_uri.strip_prefix(&worktree_root_path).ok()?;
                    let literal_prefix = relative.join(glob_literal_prefix(Path::new(&rp.pattern)));
                    Some((worktree.clone(), RelPath::new(&literal_prefix).ok()?.into_arc(), rp.pattern.clone()))
                }
            }
        })
    }

    pub(super) fn on_lsp_did_change_watched_files(
        &mut self,
        language_server_id: LanguageServerId,
        registration_id: &str,
        params: DidChangeWatchedFilesRegistrationOptions,
        cx: &mut Context<LspStore>,
    ) {
        log::trace!("Processing new watcher paths for language server with id {language_server_id}");

        let worktrees: Vec<Entity<Worktree>> = self
            .worktree_store
            .read(cx)
            .worktrees()
            .filter_map(|worktree| {
                self.language_servers_for_worktree(worktree.read(cx).id())
                    .find(|server| server.server_id() == language_server_id)
                    .map(|_| worktree)
            })
            .collect();

        for watcher in &params.watchers {
            self.register_watcher(&worktrees, watcher, registration_id, language_server_id, cx);
        }

        let registrations = self.language_server_dynamic_registrations.entry(language_server_id).or_default();
        registrations.did_change_watched_files.insert(registration_id.to_owned());

        cx.notify();
    }

    pub(super) fn on_lsp_unregister_did_change_watched_files(
        &mut self,
        language_server_id: LanguageServerId,
        registration_id: &str,
        cx: &mut Context<LspStore>,
    ) {
        let Some(registrations) = self.language_server_dynamic_registrations.get_mut(&language_server_id) else {
            return;
        };

        if registrations.did_change_watched_files.remove(registration_id) {
            log::info!(
                "language server {language_server_id}: unregistered workspace/DidChangeWatchedFiles capability with id {registration_id}"
            );
        } else {
            log::warn!(
                "language server {language_server_id}: failed to unregister workspace/DidChangeWatchedFiles capability with id {registration_id}. not registered."
            );
            return;
        }

        if let Some(watched) = self.language_server_watched_paths.get_mut(&language_server_id) {
            watched.worktree_paths.retain(|_, glob_set| {
                glob_set.remove(registration_id);
                !glob_set.is_empty()
            });
            watched.abs_paths.retain(|_, (glob_set, _)| {
                glob_set.remove(registration_id);
                !glob_set.is_empty()
            });
        }

        cx.notify();
    }
}

impl LspStore {
    pub(crate) fn did_rename_entry(&self, worktree_id: WorktreeId, old_path: &Path, new_path: &Path, is_dir: bool) {
        (|| {
            let local_store = &self.local;

            let old_uri = lsp_types::Uri::from_file_path(old_path).ok().map(|uri| uri.to_string())?;
            let new_uri = lsp_types::Uri::from_file_path(new_path).ok().map(|uri| uri.to_string())?;

            for language_server in local_store.language_servers_for_worktree(worktree_id) {
                let Some(filter) =
                    local_store.language_server_paths_watched_for_rename.get(&language_server.server_id())
                else {
                    continue;
                };

                if filter.should_send_did_rename(&old_uri, is_dir) {
                    language_server
                        .notify::<DidRenameFiles>(RenameFilesParams {
                            files: vec![FileRename { old_uri: old_uri.clone(), new_uri: new_uri.clone() }],
                        })
                        .ok();
                }
            }
            Some(())
        })();
    }

    pub(crate) fn will_rename_entry(
        this: WeakEntity<Self>,
        worktree_id: WorktreeId,
        old_path: &Path,
        new_path: &Path,
        is_dir: bool,
        cx: AsyncApp,
    ) -> Task<ProjectTransaction> {
        let old_uri = lsp_types::Uri::from_file_path(old_path).ok().map(|uri| uri.to_string());
        let new_uri = lsp_types::Uri::from_file_path(new_path).ok().map(|uri| uri.to_string());
        cx.spawn(async move |cx| {
            let mut tasks = vec![];
            this.update(cx, |this, cx| {
                let local_store = &this.local;
                let old_uri = old_uri?;
                let new_uri = new_uri?;
                for language_server in local_store.language_servers_for_worktree(worktree_id) {
                    let Some(filter) =
                        local_store.language_server_paths_watched_for_rename.get(&language_server.server_id())
                    else {
                        continue;
                    };

                    if !filter.should_send_will_rename(&old_uri, is_dir) {
                        continue;
                    }
                    let request_timeout = LSP_REQUEST_TIMEOUT;

                    let apply_edit = cx.spawn({
                        let old_uri = old_uri.clone();
                        let new_uri = new_uri.clone();
                        let language_server = Arc::clone(language_server);
                        async move |this, cx| {
                            let edit = language_server
                                .request::<WillRenameFiles>(
                                    RenameFilesParams { files: vec![FileRename { old_uri, new_uri }] },
                                    request_timeout,
                                )
                                .await
                                .into_response()
                                .context("will rename files")
                                .log_err()
                                .flatten()?;

                            LocalLspStore::deserialize_workspace_edit(
                                this.upgrade()?,
                                edit,
                                false,
                                Arc::clone(&language_server),
                                cx,
                            )
                            .await
                            .ok()
                        }
                    });
                    tasks.push(apply_edit);
                }
                Some(())
            })
            .ok()
            .flatten();
            let mut merged_transaction = ProjectTransaction::default();
            for task in tasks {
                // Await on tasks sequentially so that the order of application of edits is deterministic
                // (at least with regards to the order of registration of language servers)
                if let Some(transaction) = task.await {
                    for (buffer, buffer_transaction) in transaction.0 {
                        merged_transaction.0.insert(buffer, buffer_transaction);
                    }
                }
            }
            merged_transaction
        })
    }

    fn lsp_notify_abs_paths_changed(&self, server_id: LanguageServerId, changes: Vec<PathEvent>) {
        (|| {
            let server = self.language_server_for_id(server_id)?;
            let changes = changes
                .into_iter()
                .filter_map(|event| {
                    let typ = match event.kind? {
                        PathEventKind::Created => lsp_types::FileChangeType::CREATED,
                        PathEventKind::Removed => lsp_types::FileChangeType::DELETED,
                        PathEventKind::Changed | PathEventKind::Rescan => lsp_types::FileChangeType::CHANGED,
                    };
                    Some(lsp_types::FileEvent { uri: file_path_to_lsp_url(&event.path).log_err()?, typ })
                })
                .collect::<Vec<_>>();
            if !changes.is_empty() {
                server.notify::<DidChangeWatchedFiles>(lsp_types::DidChangeWatchedFilesParams { changes }).ok();
            }
            Some(())
        })();
    }

    pub(crate) fn update_local_worktree_language_servers(
        &mut self,
        worktree_handle: &Entity<Worktree>,
        changes: &[(Arc<RelPath>, ProjectEntryId, PathChange)],
        cx: &Context<Self>,
    ) {
        if changes.is_empty() {
            return;
        }

        let local = &mut self.local;

        let worktree_id = worktree_handle.read(cx).id();
        let mut language_server_ids = local
            .language_server_ids
            .iter()
            .filter_map(|(seed, v)| seed.worktree_id.eq(&worktree_id).then_some(v.id))
            .collect::<Vec<_>>();
        language_server_ids.sort_unstable();
        language_server_ids.dedup();

        // let abs_path = worktree_handle.read(cx).abs_path();
        for server_id in &language_server_ids {
            if let Some(LanguageServerState::Running { server, .. }) = local.language_servers.get(server_id)
                && let Some(watched_paths) = local
                    .language_server_watched_paths
                    .get_mut(server_id)
                    .and_then(|paths| paths.worktree_paths.get_mut(&worktree_id))
            {
                let params = lsp_types::DidChangeWatchedFilesParams {
                    changes: changes
                        .iter()
                        .filter_map(|(path, _, change)| {
                            let typ = match change {
                                PathChange::Loaded => return None,
                                PathChange::Added => lsp_types::FileChangeType::CREATED,
                                PathChange::Removed => lsp_types::FileChangeType::DELETED,
                                PathChange::Updated => lsp_types::FileChangeType::CHANGED,
                                PathChange::AddedOrUpdated => lsp_types::FileChangeType::CHANGED,
                            };
                            if !watched_paths.is_match(path.as_std_path()) {
                                return None;
                            }
                            let uri = lsp_types::Uri::from_file_path(worktree_handle.read(cx).absolutize(path)).ok()?;
                            Some(lsp_types::FileEvent { uri, typ })
                        })
                        .collect(),
                };
                if !params.changes.is_empty() {
                    server.notify::<DidChangeWatchedFiles>(params).ok();
                }
            }
        }
        let needs_flycheck = {
            let worktree = worktree_handle.read(cx);
            let buffer_store = self.buffer_store.read(cx);
            changes.iter().any(|(path, _, change)| {
                disk_change_needs_flycheck(
                    path,
                    *change,
                    worktree.entry_for_path(path).is_some_and(|entry| entry.is_ignored),
                    buffer_store.get_by_path(&ProjectPath { worktree_id, path: Arc::clone(path) }).is_some(),
                )
            })
        };
        if needs_flycheck {
            for server_id in language_server_ids {
                self.schedule_flycheck_for_disk_change(server_id, cx);
            }
        }
        let local = &self.local;
        for (path, _, _) in changes {
            if let Some(file_name) = path.file_name()
                && local.watched_manifest_filenames.contains(file_name)
            {
                self.request_workspace_config_refresh();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use util::rel_path::RelPath;
    use worktree::PathChange;

    use super::disk_change_needs_flycheck;

    #[test]
    fn disk_change_needs_flycheck_only_for_unopened_tracked_rust_files() {
        let rust = RelPath::new_test("crates/a/src/lib.rs");
        for change in [PathChange::Added, PathChange::Removed, PathChange::Updated, PathChange::AddedOrUpdated] {
            assert!(disk_change_needs_flycheck(&rust, change, false, false));
        }
        // The initial scan is not a change.
        assert!(!disk_change_needs_flycheck(&rust, PathChange::Loaded, false, false));
        // Flycheck's own output must not re-trigger it.
        assert!(!disk_change_needs_flycheck(&rust, PathChange::Updated, true, false));
        // An open buffer triggers through its reload, and a manual save through `didSave`.
        assert!(!disk_change_needs_flycheck(&rust, PathChange::Updated, false, true));
        for other in ["Cargo.toml", "README.md", "src/rs", "Makefile"] {
            assert!(!disk_change_needs_flycheck(&RelPath::new_test(other), PathChange::Updated, false, false));
        }
    }
}
