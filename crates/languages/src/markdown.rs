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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use task::TaskContext;
    use task::TaskVariables;
    use task::VariableName;
    use tree_sitter::Parser;
    use tree_sitter::Query;
    use tree_sitter::QueryCursor;
    use tree_sitter::StreamingIterator as _;

    use super::markdown_task_templates;
    use super::shell_block_template;

    /// `(tag, run text, code)` for every runnable match in `markdown`.
    fn runnables(markdown: &str) -> Vec<(String, String, String)> {
        let (_, language) = grammars::native_grammars().into_iter().find(|(name, _)| *name == "markdown").unwrap();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(markdown, None).unwrap();
        let source = grammars::load_queries("markdown").runnables.unwrap();
        let query = Query::new(&language, &source).unwrap();
        let text = |node: tree_sitter::Node<'_>| markdown[node.byte_range()].to_owned();

        let mut found = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), markdown.as_bytes());
        while let Some(mat) = matches.next() {
            let capture = |name: &str| {
                let index = query.capture_index_for_name(name).unwrap();
                mat.captures().iter().find(|capture| capture.index == index).map(|capture| text(capture.node)).unwrap()
            };
            let tag = query.property_settings(mat.pattern_index)[0].value.as_deref().unwrap().to_owned();
            found.push((tag, capture("run"), capture("code")));
        }
        found
    }

    #[test]
    fn test_shell_code_blocks_are_runnable() {
        let markdown = "# t\n\n```sh\necho \"hello\"\n```\n\n```bash\nls\n```\n\n```zsh\npwd\n```\n\n```shell\nid\n```\n\n```fish\nset -l x 1\necho $x\n```\n";
        assert_eq!(
            runnables(markdown),
            [
                ("markdown-sh", "sh", "echo \"hello\"\n"),
                ("markdown-bash", "bash", "ls\n"),
                ("markdown-zsh", "zsh", "pwd\n"),
                ("markdown-sh", "shell", "id\n"),
                ("markdown-fish", "fish", "set -l x 1\necho $x\n"),
            ]
            .map(|(tag, run, code)| (tag.to_owned(), run.to_owned(), code.to_owned()))
        );
    }

    #[test]
    fn test_other_code_blocks_are_not_runnable() {
        let markdown = "```rust\nfn main() {}\n```\n\n```\necho untagged\n```\n\n```sh\n```\n\n    echo indented\n";
        assert_eq!(runnables(markdown), []);
    }

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
