use std::time::Duration;

use gpui::Context;
use lsp::LanguageServerId;
use lsp::LanguageServerName;

use crate::LspStore;
use crate::lsp_store;

pub const RUST_ANALYZER_NAME: LanguageServerName = LanguageServerName::new_static("rust-analyzer");

/// Collapses a burst of on-disk changes (an agent editing several files) into one check.
const DISK_CHANGE_FLYCHECK_DEBOUNCE: Duration = Duration::from_secs(1);

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
