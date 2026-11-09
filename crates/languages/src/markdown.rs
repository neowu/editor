use std::num::NonZeroU32;

use language::language_settings::DocumentSymbols;
use language::language_settings::FormatOnSave;
use language::language_settings::Formatter;
use language::language_settings::FormatterList;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SoftWrap;
use language::language_settings::WordsCompletionMode;
use project::ContextProviderWithTasks;
use rustc_hash::FxHashMap;
use task::TaskTemplate;
use task::TaskTemplates;
use task::VariableName;

/// Environment variable carrying a code block's text into the shell that runs it.
/// Passing it through the environment keeps the user's shell from re-parsing the code.
const CODE_BLOCK_ENV: &str = "CODE_BLOCK";

/// Runnable tag (from `markdown/runnables.scm`) and the shell that runs that block.
const SHELL_BLOCKS: [(&str, &str); 4] =
    [("markdown-sh", "sh"), ("markdown-bash", "bash"), ("markdown-zsh", "zsh"), ("markdown-fish", "fish")];

pub(super) fn markdown_task_context() -> ContextProviderWithTasks {
    ContextProviderWithTasks::new(markdown_task_templates())
}

fn markdown_task_templates() -> TaskTemplates {
    TaskTemplates(SHELL_BLOCKS.iter().map(|&(tag, shell)| shell_block_template(tag, shell)).collect())
}

/// Runs the block's code (the `@code` capture) with `shell -c`, from the markdown file's folder.
fn shell_block_template(tag: &str, shell: &str) -> TaskTemplate {
    TaskTemplate {
        label: format!("run {shell} code block"),
        command: shell.to_owned(),
        args: vec!["-c".to_owned(), format!("\"${CODE_BLOCK_ENV}\"")],
        env: FxHashMap::from_iter([(CODE_BLOCK_ENV.to_owned(), VariableName::Custom("code".into()).template_value())]),
        cwd: Some(VariableName::Dirname.template_value()),
        tags: vec![tag.to_owned()],
    }
}

/// Settings for Markdown and Markdown-Inline.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(2).unwrap(),
        hard_tabs: false,
        soft_wrap: SoftWrap::EditorWidth,
        preferred_line_length: 300,
        wrap_guides: vec![120],
        format_on_save: FormatOnSave::On,
        formatter: FormatterList::Single(Formatter::Auto),
        code_actions_on_format: FxHashMap::default(),
        remove_trailing_whitespace_on_save: false,
        ensure_final_newline_on_save: true,
        // No markdown language server; outline and breadcrumbs come from tree-sitter.
        document_symbols: DocumentSymbols::Off,
        allow_rewrap: RewrapBehavior::Anywhere,
        use_on_type_format: false,
        auto_indent_on_paste: false,
        word_completions: WordsCompletionMode::Disabled,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use task::TaskContext;
    use task::TaskVariables;
    use task::VariableName;

    use super::markdown_task_templates;
    use super::shell_block_template;

    #[test]
    fn test_shell_block_template_runs_code_from_env() {
        let mut variables = TaskVariables::default();
        variables.insert(VariableName::Custom("code".into()), "echo 'a' \"$HOME\"\n".to_owned());
        variables.insert(VariableName::Dirname, "/notes".to_owned());
        let context = TaskContext { task_variables: variables, ..TaskContext::default() };

        let resolved = shell_block_template("markdown-fish", "fish").resolve_task("test", &context).unwrap().resolved;
        assert_eq!(resolved.command.as_deref(), Some("fish"));
        assert_eq!(resolved.args, ["-c", "\"$CODE_BLOCK\""]);
        assert_eq!(resolved.env.get("CODE_BLOCK").map(String::as_str), Some("echo 'a' \"$HOME\"\n"));
        assert_eq!(resolved.cwd, Some(PathBuf::from("/notes")));
    }

    #[test]
    fn test_markdown_task_templates_have_one_per_shell_tag() {
        let templates = markdown_task_templates();
        let tags: Vec<_> =
            templates.0.iter().map(|template| (template.tags[0].as_str(), template.command.as_str())).collect();
        assert_eq!(
            tags,
            [("markdown-sh", "sh"), ("markdown-bash", "bash"), ("markdown-zsh", "zsh"), ("markdown-fish", "fish")]
        );
    }
}
