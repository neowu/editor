use std::path::PathBuf;
use std::sync::Arc;

use gpui::App;
use gpui::SharedString;
use gpui::Task;
use lsp::LanguageServerName;
use rustc_hash::FxHashMap;
use task::SpawnInTerminal;

// lint: expect(module_cycle)
use crate::RunnableResolver;

/// Where a tree-sitter runnable is, for its language to build the tasks it offers.
#[derive(Clone, Debug, Default)]
pub struct RunContext {
    /// The runnable's tag (`#set! tag` in `runnables.scm`).
    pub tag: SharedString,
    /// Named captures of the runnable's match, `_`-prefixed ones included.
    pub captures: FxHashMap<String, String>,
    /// Absolute path of the buffer's file.
    pub file: PathBuf,
    /// Root of the file's worktree; `None` for a single-file worktree.
    pub worktree_root: Option<PathBuf>,
    /// Name of the innermost outline symbol containing the runnable.
    pub symbol: Option<String>,
}

impl RunContext {
    pub fn capture(&self, name: &str) -> Option<&str> {
        self.captures.get(name).map(String::as_str)
    }

    /// The directory containing the file.
    pub fn dir(&self) -> Option<PathBuf> {
        self.file.parent().map(PathBuf::from)
    }
}

/// What a language contributes to the gutter run icon.
pub trait ContextProvider: Send + Sync {
    /// The tasks for a tree-sitter runnable, built when its run icon is clicked.
    /// The first one is what a left click launches.
    fn runnables(&self, _: RunContext, _: &App) -> Task<Vec<SpawnInTerminal>> {
        Task::ready(Vec::new())
    }

    /// A language server name, that can return tasks using LSP (ext) for this language.
    fn lsp_task_source(&self) -> Option<LanguageServerName> {
        None
    }

    /// A resolver for runnable queries that group captures with `@run_item`.
    /// For each group, the resolver picks which `@run` range and extra captures
    /// to surface. Without a resolver, grouped matches emit no runnables.
    // lint: expect(module_cycle)
    fn runnable_resolver(&self) -> Option<Arc<dyn RunnableResolver>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::RunContext;

    #[test]
    fn run_context_reads_captures_and_dir() {
        let cx = RunContext {
            captures: rustc_hash::FxHashMap::from_iter([("_test_name".to_owned(), "it_works".to_owned())]),
            file: PathBuf::from("/a/b/c.rs"),
            ..RunContext::default()
        };
        assert_eq!(cx.capture("_test_name"), Some("it_works"));
        assert_eq!(cx.capture("missing"), None);
        assert_eq!(cx.dir(), Some(PathBuf::from("/a/b")));
    }
}
