use std::num::NonZeroU32;
use std::sync::Arc;

use gpui::App;
use gpui::Task;
use language::ContextProvider;
use language::RunContext;
use language::language_settings::Formatter;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::WordsCompletionMode;
use rustc_hash::FxHashMap;
use task::SpawnInTerminal;

use crate::LanguageDef;

/// Environment variable carrying a code block's text into the shell that runs it.
/// Passing it through the environment keeps the user's shell from re-parsing the code.
const CODE_BLOCK_ENV: &str = "CODE_BLOCK";

/// Runnable tag (from `markdown/runnables.scm`) and the shell that runs that block.
const SHELL_BLOCKS: [(&str, &str); 4] =
    [("markdown-sh", "sh"), ("markdown-bash", "bash"), ("markdown-zsh", "zsh"), ("markdown-fish", "fish")];

pub(crate) struct MarkdownContextProvider;

impl ContextProvider for MarkdownContextProvider {
    fn runnables(&self, cx: RunContext, _: &App) -> Task<Vec<SpawnInTerminal>> {
        Task::ready(shell_block_task(&cx).into_iter().collect())
    }
}

/// Runs the block's code (the `@code` capture) with `shell -c`, from the markdown file's folder.
pub fn shell_block_task(cx: &RunContext) -> Option<SpawnInTerminal> {
    let (_, shell) = SHELL_BLOCKS.iter().find(|(tag, _)| *tag == cx.tag.as_ref())?;
    let code = cx.capture("code")?;
    let task = SpawnInTerminal::new(
        format!("run {shell} code block"),
        *shell,
        vec!["-c".to_owned(), format!("\"${CODE_BLOCK_ENV}\"")],
        cx.dir(),
    );
    Some(SpawnInTerminal { env: FxHashMap::from_iter([(CODE_BLOCK_ENV.to_owned(), code.to_owned())]), ..task })
}

/// Settings for Markdown and Markdown-Inline.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(2).expect("non-zero"),
        hard_tabs: false,
        soft_wrap: true,
        preferred_line_length: 300,
        wrap_guides: vec![120],
        formatter: Formatter::Auto,
        code_actions_on_format: FxHashMap::default(),
        remove_trailing_whitespace_on_save: false,
        ensure_final_newline_on_save: true,
        // No markdown language server; outline and breadcrumbs come from tree-sitter.
        lsp_document_symbols: false,
        allow_rewrap: RewrapBehavior::Anywhere,
        use_on_type_format: false,
        auto_indent_on_paste: false,
        word_completions: WordsCompletionMode::Disabled,
    }
}

pub(crate) fn languages() -> Vec<LanguageDef> {
    vec![
        LanguageDef {
            grammar: Some(tree_sitter_md::LANGUAGE.into()),
            context: Some(Arc::new(MarkdownContextProvider)),
            file_types: &[".rules", ".cursorrules", ".windsurfrules", ".clinerules"],
            ..LanguageDef::new("markdown", settings())
        },
        LanguageDef {
            grammar: Some(tree_sitter_md::INLINE_LANGUAGE.into()),
            ..LanguageDef::new("markdown-inline", settings())
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use language::RunContext;

    use super::shell_block_task;

    #[test]
    fn shell_block_task_runs_code_from_env() {
        let cx = RunContext {
            tag: "markdown-fish".into(),
            captures: rustc_hash::FxHashMap::from_iter([("code".to_owned(), "echo 'a' \"$HOME\"\n".to_owned())]),
            file: PathBuf::from("/notes/a.md"),
            ..RunContext::default()
        };
        let task = shell_block_task(&cx).unwrap();
        assert_eq!(task.full_label, "run fish code block");
        assert_eq!(task.command.as_deref(), Some("fish"));
        assert_eq!(task.args, ["-c", "\"$CODE_BLOCK\""]);
        assert_eq!(task.env.get("CODE_BLOCK").map(String::as_str), Some("echo 'a' \"$HOME\"\n"));
        assert_eq!(task.cwd, Some(PathBuf::from("/notes")));

        for tag in ["markdown-sh", "markdown-bash", "markdown-zsh"] {
            let shell = tag.trim_start_matches("markdown-");
            let task = shell_block_task(&RunContext { tag: tag.into(), ..cx.clone() }).unwrap();
            assert_eq!(task.command.as_deref(), Some(shell));
        }
        assert!(shell_block_task(&RunContext { tag: "markdown-python".into(), ..cx.clone() }).is_none());
        assert!(shell_block_task(&RunContext { captures: rustc_hash::FxHashMap::default(), ..cx }).is_none());
    }
}
