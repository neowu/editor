use std::num::NonZeroU32;
use std::sync::Arc;
use std::vec;

use async_trait::async_trait;
use gpui::App;
use gpui::AsyncApp;
use gpui::Task;
use language::ContextProvider;
use language::LanguageServerName;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::RunContext;
use language::language_settings::Formatter;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::WordsCompletionMode;
use lsp::LanguageServerBinary;
use rustc_hash::FxHashMap;
use task::SpawnInTerminal;

use crate::LanguageDef;
use crate::npx;

struct BashContextProvider;

impl ContextProvider for BashContextProvider {
    fn runnables(&self, cx: RunContext, _: &App) -> Task<Vec<SpawnInTerminal>> {
        Task::ready(bash_tasks(&cx))
    }
}

/// `bash-script`: runs the script file itself, so its shebang picks the interpreter.
pub fn bash_tasks(cx: &RunContext) -> Vec<SpawnInTerminal> {
    if cx.tag.as_ref() != "bash-script" {
        return Vec::new();
    }
    let file = cx.file.to_string_lossy().into_owned();
    vec![SpawnInTerminal::new(format!("run '{file}'"), file, Vec::new(), cx.worktree_root.clone())]
}

struct BashLspAdapter;

impl BashLspAdapter {
    const PACKAGE_NAME: &str = "bash-language-server";
}

#[async_trait(?Send)]
impl LspAdapter for BashLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName::new_static(Self::PACKAGE_NAME)
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        npx::node_server_binary(delegate, &[Self::PACKAGE_NAME], Self::PACKAGE_NAME, &["start"]).await
    }
}

/// Settings for Shell Script.
fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).expect("non-zero"),
        hard_tabs: false,
        soft_wrap: false,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        formatter: Formatter::Auto,
        code_actions_on_format: FxHashMap::default(),
        remove_trailing_whitespace_on_save: true,
        ensure_final_newline_on_save: true,
        lsp_document_symbols: true,
        allow_rewrap: RewrapBehavior::InComments,
        use_on_type_format: true,
        auto_indent_on_paste: true,
        word_completions: WordsCompletionMode::Fallback,
    }
}

pub(crate) fn languages() -> Vec<LanguageDef> {
    vec![LanguageDef {
        grammar: Some(tree_sitter_bash::LANGUAGE.into()),
        adapters: vec![Arc::new(BashLspAdapter)],
        context: Some(Arc::new(BashContextProvider)),
        ..LanguageDef::new("bash", settings())
    }]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use language::RunContext;

    use super::bash_tasks;

    #[test]
    fn bash_script_runs_the_file_from_the_worktree_root() {
        let cx = RunContext {
            tag: "bash-script".into(),
            file: PathBuf::from("/w/run.sh"),
            worktree_root: Some(PathBuf::from("/w")),
            ..RunContext::default()
        };
        let tasks = bash_tasks(&cx);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].full_label, "run '/w/run.sh'");
        assert_eq!(tasks[0].command.as_deref(), Some("/w/run.sh"));
        assert_eq!(tasks[0].cwd, Some(PathBuf::from("/w")));

        assert!(bash_tasks(&RunContext { tag: "other".into(), ..cx }).is_empty());
    }
}
