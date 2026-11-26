use std::time::Duration;

use gpui::Context;
use lsp::LanguageServerId;
use lsp::LanguageServerName;
use util::rel_path::RelPath;
use worktree::PathChange;

use crate::LspStore;
use crate::lsp_store;

pub const RUST_ANALYZER_NAME: LanguageServerName = LanguageServerName::new_static("rust-analyzer");

/// Collapses a burst of on-disk changes (an agent editing several files) into one check.
const DISK_CHANGE_FLYCHECK_DEBOUNCE: Duration = Duration::from_secs(1);

/// Whether a worktree change is a Rust source edit rust-analyzer won't re-check on its own.
/// Open buffers are covered by their reload; ignored paths (`target/`) are flycheck's own output.
pub(crate) fn disk_change_needs_flycheck(
    path: &RelPath,
    change: PathChange,
    is_ignored: bool,
    has_open_buffer: bool,
) -> bool {
    !matches!(change, PathChange::Loaded) && !is_ignored && !has_open_buffer && path.extension() == Some("rs")
}

impl LspStore {
    /// rust-analyzer runs flycheck only on `didSave`, which a change made on disk never sends,
    /// so its cargo diagnostics would stay stale until the next manual save.
    pub(crate) fn schedule_flycheck_for_disk_change(&mut self, server_id: LanguageServerId, cx: &Context<Self>) {
        if self.language_server_for_id(server_id).is_none_or(|server| server.name() != RUST_ANALYZER_NAME) {
            return;
        }
        let task = cx.spawn(async move |lsp_store, cx| {
            cx.background_executor().timer(DISK_CHANGE_FLYCHECK_DEBOUNCE).await;
            lsp_store
                .read_with(cx, |lsp_store, _| {
                    if let Some(server) = lsp_store.language_server_for_id(server_id) {
                        server
                            .notify::<lsp_store::lsp_ext_command::LspExtRunFlycheck>(
                                lsp_store::lsp_ext_command::RunFlycheckParams { text_document: None },
                            )
                            .ok();
                    }
                })
                .ok();
        });
        self.disk_change_flycheck_tasks.insert(server_id, task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
