//! Tasks for tree-sitter runnables (the gutter run icon), built by the buffer's language.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::join_all;
use gpui::App;
use gpui::AppContext as _;
use gpui::Entity;
use gpui::SharedString;
use gpui::Task;
use language::Buffer;
use language::Language;
use language::RunContext;
use rustc_hash::FxHashMap;
use task::RunnableTag;
use task::SpawnInTerminal;
use text::Anchor;

use crate::Project;

impl Project {
    /// The tasks `language` offers for a runnable at `position` in `buffer`, in tag order.
    pub fn language_runnables(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        language: &Arc<Language>,
        tags: &[RunnableTag],
        captures: &FxHashMap<String, String>,
        cx: &App,
    ) -> Task<Vec<SpawnInTerminal>> {
        let Some(provider) = language.context_provider() else {
            return Task::ready(Vec::new());
        };
        let buffer = buffer.read(cx);
        let Some(file) = buffer.file().and_then(|file| file.as_local()) else {
            return Task::ready(Vec::new());
        };
        let worktree_root = self.worktree_root_dir(file.worktree_id(cx), cx);
        let symbol = buffer.snapshot().symbols_containing(position, None).pop().map(|symbol| {
            let range = symbol.name_ranges.last().cloned().unwrap_or(0..symbol.text.len());
            symbol.text[range].to_owned()
        });
        let context = RunContext {
            tag: SharedString::default(),
            captures: captures.clone(),
            file: file.abs_path(cx),
            worktree_root,
            symbol,
        };
        let tasks = tags
            .iter()
            .map(|RunnableTag(tag)| provider.runnables(RunContext { tag: tag.clone(), ..context.clone() }, cx))
            .collect::<Vec<_>>();
        cx.background_spawn(async move { join_all(tasks).await.into_iter().flatten().collect() })
    }

    /// The root of a visible, directory worktree; `None` for a single-file one.
    fn worktree_root_dir(&self, worktree_id: worktree::WorktreeId, cx: &App) -> Option<PathBuf> {
        let worktree = self.worktree_store.read(cx).worktree_for_id(worktree_id, cx)?;
        let worktree = worktree.read(cx);
        if !worktree.is_visible() {
            return None;
        }
        let root_entry = worktree.root_entry()?;
        root_entry.is_dir().then(|| worktree.absolutize(&root_entry.path))
    }
}
