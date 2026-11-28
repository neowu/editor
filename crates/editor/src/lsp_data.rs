use std::time::Duration;

use futures::FutureExt as _;
use futures::future::Shared;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::Window;
use itertools::Itertools as _;
use language::Buffer;
use text::BufferId;

use super::Editor;
use super::LspDataKind;
// lint: expect(module_cycle)
use crate::inlays::inlay_hints::InlayHintRefreshReason;

impl Editor {
    pub(crate) const fn lsp_data_enabled(&self) -> bool {
        self.enable_lsp_data && self.mode().is_full()
    }

    pub(super) fn update_lsp_data(
        &mut self,
        for_buffer: Option<BufferId>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.lsp_data_enabled() {
            return;
        }

        if let Some(buffer_id) = for_buffer {
            // lint: expect(module_cycle)
            self.pull_diagnostics(buffer_id, window, cx);
        }
        // lint: expect(module_cycle)
        self.refresh_semantic_tokens(for_buffer, false, cx);
        // lint: expect(module_cycle)
        self.refresh_document_colors(for_buffer, window, cx);
        // lint: expect(module_cycle)
        self.refresh_document_links(for_buffer, cx);
        // lint: expect(module_cycle)
        self.refresh_folding_ranges(for_buffer, window, cx);
        // lint: expect(module_cycle)
        self.refresh_document_symbols(for_buffer, cx);
    }

    pub(super) fn lsp_data_buffers(&self, for_buffer: Option<BufferId>, cx: &mut Context<Self>) -> Vec<Entity<Buffer>> {
        // lint: expect(module_cycle)
        self.visible_buffers(cx)
            .into_iter()
            // lint: expect(module_cycle)
            .filter(|buffer| self.is_lsp_relevant(buffer.read(cx).file(), cx))
            .chain(for_buffer.and_then(|buffer_id| self.buffer.read(cx).buffer(buffer_id)))
            .filter(|buffer| {
                let buffer_id = buffer.read(cx).buffer_id();
                for_buffer.is_none_or(|for_buffer| for_buffer == buffer_id)
                    && self.registered_buffers.contains_key(&buffer_id)
            })
            .unique_by(|buffer| buffer.read(cx).buffer_id())
            .collect()
    }

    fn is_buffer_visible(&self, buffer_id: BufferId, cx: &mut Context<Self>) -> bool {
        self.buffer.read(cx).is_singleton()
            // lint: expect(module_cycle)
            || self.visible_buffers(cx).iter().any(|buffer| buffer.read(cx).buffer_id() == buffer_id)
    }

    /// Spawns one task per buffer. A full refresh (`for_buffer == None`) replaces every task of `kind`;
    /// a single-buffer refresh replaces only that buffer's task, keeping the others running.
    pub(super) fn spawn_lsp_data_tasks(
        &mut self,
        kind: LspDataKind,
        for_buffer: Option<BufferId>,
        buffers: Vec<Entity<Buffer>>,
        cx: &mut Context<Self>,
        spawn: impl Fn(Entity<Buffer>, &mut Context<Self>) -> Task<()>,
    ) {
        let Some(buffer_id) = for_buffer else {
            self.clear_lsp_data_tasks(kind);
            for buffer in buffers {
                let buffer_id = buffer.read(cx).buffer_id();
                let task = spawn(buffer, cx).shared();
                self.insert_lsp_data_task(kind, buffer_id, task);
            }
            return;
        };

        if let Some(tasks) = self.lsp_data_tasks.get_mut(&buffer_id) {
            tasks.remove(&kind);
        }
        if self.offscreen_lsp_data_tasks.get(&kind).is_some_and(|(task_buffer_id, _)| *task_buffer_id == buffer_id) {
            self.offscreen_lsp_data_tasks.remove(&kind);
        }
        let is_visible = self.is_buffer_visible(buffer_id, cx);
        for buffer in buffers {
            let task = spawn(buffer, cx);
            if is_visible {
                self.insert_lsp_data_task(kind, buffer_id, task.shared());
            } else {
                self.offscreen_lsp_data_tasks.insert(kind, (buffer_id, task));
            }
        }
    }

    fn insert_lsp_data_task(&mut self, kind: LspDataKind, buffer_id: BufferId, task: Shared<Task<()>>) {
        self.lsp_data_tasks.entry(buffer_id).or_default().insert(kind, task);
    }

    fn clear_lsp_data_tasks(&mut self, kind: LspDataKind) {
        for tasks in self.lsp_data_tasks.values_mut() {
            tasks.remove(&kind);
        }
        self.offscreen_lsp_data_tasks.remove(&kind);
    }

    /// Drops LSP data and pending requests that no longer match the multibuffer's excerpts:
    /// all of it for `buffers_removed`, otherwise only what fell outside the remaining excerpts.
    pub(super) fn remove_invalid_lsp_data(
        &mut self,
        buffer_ids: &[BufferId],
        buffers_removed: bool,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.buffer.read(cx).snapshot(cx);
        for buffer_id in buffer_ids {
            if snapshot.buffer_for_id(*buffer_id).is_none() {
                self.lsp_data_tasks.remove(buffer_id);
                self.offscreen_lsp_data_tasks.retain(|_, (task_buffer_id, _)| task_buffer_id != buffer_id);
                self.refresh_document_symbols_tasks.remove(buffer_id);
            }
        }
        // lint: expect(module_cycle)
        self.remove_invalid_inline_diagnostics(buffer_ids, buffers_removed, &snapshot, cx);
        // lint: expect(module_cycle)
        self.remove_invalid_document_colors(buffer_ids, buffers_removed, &snapshot, cx);
    }

    pub(super) fn register_visible_buffers(&mut self, cx: &mut Context<Self>) {
        if !self.lsp_data_enabled() {
            return;
        }
        let visible_buffers: Vec<_> = self
            // lint: expect(module_cycle)
            .visible_buffers(cx)
            .into_iter()
            // lint: expect(module_cycle)
            .filter(|buffer| self.is_lsp_relevant(buffer.read(cx).file(), cx))
            .collect();
        for visible_buffer in visible_buffers {
            self.register_buffer(visible_buffer.read(cx).buffer_id(), cx);
        }
    }

    pub(super) fn register_buffer(&mut self, buffer_id: BufferId, cx: &mut Context<Self>) {
        if !self.lsp_data_enabled() {
            return;
        }

        if !self.registered_buffers.contains_key(&buffer_id)
            && let Some(project) = self.project.as_ref()
        {
            if let Some(buffer) = self.buffer.read(cx).buffer(buffer_id) {
                project.update(cx, |project, cx| {
                    self.registered_buffers
                        .insert(buffer_id, project.register_buffer_with_language_servers(&buffer, cx));
                });
            } else {
                self.registered_buffers.remove(&buffer_id);
            }
        }
    }

    pub(super) const fn disable_lsp_data(&mut self) {
        self.enable_lsp_data = false;
    }

    pub(super) const fn disable_runnables(&mut self) {
        self.enable_runnables = false;
    }

    pub(super) fn update_data_on_scroll(&mut self, debounce: bool, window: &Window, cx: &mut Context<'_, Self>) {
        if debounce {
            self.post_scroll_update = cx.spawn_in(window, async move |editor, cx| {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                editor
                    .update_in(cx, |editor, window, cx| {
                        editor.do_update_data_on_scroll(window, cx);
                    })
                    .ok();
            });
        } else {
            self.post_scroll_update = Task::ready(());
            self.do_update_data_on_scroll(window, cx);
        }
    }

    fn do_update_data_on_scroll(&mut self, window: &Window, cx: &mut Context<'_, Self>) {
        self.register_visible_buffers(cx);
        // lint: expect(module_cycle)
        self.refresh_spell_check(cx);
        // lint: expect(module_cycle)
        self.refresh_inlay_hints(InlayHintRefreshReason::NewLinesShown, cx);

        if !self.buffer().read(cx).is_singleton() || self.needs_initial_data_update {
            self.needs_initial_data_update = false;
            self.update_lsp_data(None, window, cx);
        }
        // Syntax runnables are only known around the viewport.
        // lint: expect(module_cycle)
        self.refresh_runnables(None, window, cx);
    }
}
