use language::language_settings::LanguageSettings;
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

        let buffers_to_query = self
            .lsp_data_buffers(for_buffer, cx)
            .into_iter()
            .filter(|buffer| LanguageSettings::for_buffer(buffer.read(cx), cx).document_folding_ranges.enabled())
            .collect();
        self.spawn_lsp_data_tasks(LspDataKind::FoldingRanges, for_buffer, buffers_to_query, cx, |buffer, cx| {
            let project = project.clone();
            let buffer_id = buffer.read(cx).remote_id();
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

    /// Removes LSP folding creases for buffers whose `lsp_folding_ranges`
    /// setting has been turned off, and triggers a refresh so newly-enabled
    /// buffers get their ranges fetched.
    pub(super) fn clear_disabled_lsp_folding_ranges(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.use_document_folding_ranges {
            return;
        }

        let buffers_to_clear = self
            .buffer
            .read(cx)
            .all_buffers()
            .into_iter()
            .filter(|buffer| {
                let buffer = buffer.read(cx);
                !LanguageSettings::for_buffer(buffer, cx).document_folding_ranges.enabled()
            })
            .map(|buffer| buffer.read(cx).remote_id())
            .collect::<Vec<_>>();

        if !buffers_to_clear.is_empty() {
            self.display_map.update(cx, |display_map, cx| {
                for buffer_id in buffers_to_clear {
                    display_map.clear_lsp_folding_ranges(buffer_id, cx);
                }
            });
            cx.notify();
        }

        self.refresh_folding_ranges(None, window, cx);
    }
}
