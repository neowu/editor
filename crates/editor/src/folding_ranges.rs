use language::language_settings::DOCUMENT_FOLDING_RANGES;
use text::BufferId;
use ui::Context;
use ui::Window;

use crate::Editor;
use crate::LSP_REQUEST_DEBOUNCE_TIMEOUT;
use crate::LspDataKind;

impl Editor {
    pub(super) fn refresh_folding_ranges(
        &mut self,
        for_buffer: Option<BufferId>,
        _window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.lsp_data_enabled() || !self.use_document_folding_ranges {
            return;
        }
        let Some(project) = self.project.as_ref().map(|p| p.downgrade()) else {
            return;
        };

        let buffers_to_query =
            self.lsp_data_buffers(for_buffer, cx).into_iter().filter(|_| DOCUMENT_FOLDING_RANGES.enabled()).collect();
        self.spawn_lsp_data_tasks(LspDataKind::FoldingRanges, for_buffer, buffers_to_query, cx, |buffer, cx| {
            let project = project.clone();
            let buffer_id = buffer.read(cx).buffer_id();
            cx.spawn(async move |editor, cx| {
                cx.background_executor().timer(LSP_REQUEST_DEBOUNCE_TIMEOUT).await;

                let Ok(task) = project.update(cx, |project, cx| {
                    project.lsp_store().update(cx, |lsp_store, cx| lsp_store.fetch_folding_ranges(&buffer, cx))
                }) else {
                    return;
                };
                let ranges = task.await;

                editor
                    .update(cx, |editor, cx| {
                        editor.display_map.update(cx, |display_map, cx| {
                            display_map.set_lsp_folding_ranges(buffer_id, ranges, cx);
                        });
                        cx.notify();
                    })
                    .ok();
            })
        });
    }
}
