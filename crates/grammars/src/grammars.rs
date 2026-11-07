use std::borrow::Cow;
use std::str;

use anyhow::Context as _;
use language_core::LanguageConfig;
use language_core::LanguageQueries;
use language_core::QueryFile;
use language_core::QueryFileContents;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "src/"]
#[exclude = "*.rs"]
struct GrammarDir;

/// Built-in tree-sitter grammars; register them before loading language configs/queries.
pub fn native_grammars() -> Vec<(&'static str, tree_sitter::Language)> {
    vec![
        ("bash", tree_sitter_bash::LANGUAGE.into()),
        ("css", tree_sitter_css::LANGUAGE.into()),
        ("go", tree_sitter_go::LANGUAGE.into()),
        ("html", tree_sitter_html::LANGUAGE.into()),
        ("json", tree_sitter_json::LANGUAGE.into()),
        ("jsonc", tree_sitter_json::LANGUAGE.into()),
        ("markdown", tree_sitter_md::LANGUAGE.into()),
        ("markdown-inline", tree_sitter_md::INLINE_LANGUAGE.into()),
        ("python", tree_sitter_python::LANGUAGE.into()),
        ("rust", tree_sitter_rust::LANGUAGE.into()),
        ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        ("yaml", tree_sitter_yaml::LANGUAGE.into()),
    ]
}

/// Load and parse the `config.toml` for a given language name.
pub fn load_config(name: &str) -> LanguageConfig {
    let file = GrammarDir::get(&format!("{name}/config.toml"))
        .unwrap_or_else(|| panic!("missing config for language {name:?}"));
    LanguageConfig::from_toml(str::from_utf8(&file.data).unwrap())
        .with_context(|| format!("failed to load config.toml for language {name:?}"))
        .unwrap()
}

/// Get a raw embedded file by path (relative to `src/`).
///
/// Returns the file data as bytes, or `None` if the file does not exist.
pub fn get_file(path: &str) -> Option<rust_embed::EmbeddedFile> {
    GrammarDir::get(path)
}

/// Load all Tree-sitter query files for a given language name.
pub fn load_queries(name: &str) -> LanguageQueries {
    LanguageQueries::from_files(GrammarDir::iter().filter_map(|path| {
        let file_name = path.strip_prefix(name)?.strip_prefix('/')?;
        let query_file = file_name.parse::<QueryFile>().ok()?;
        let contents = match GrammarDir::get(path.as_ref())?.data {
            Cow::Borrowed(bytes) => Cow::Borrowed(str::from_utf8(bytes).ok()?),
            Cow::Owned(bytes) => Cow::Owned(String::from_utf8(bytes).ok()?),
        };
        Some(QueryFileContents::new(query_file, contents))
    }))
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;
    use tree_sitter::Query;
    use tree_sitter::QueryCursor;
    use tree_sitter::StreamingIterator as _;

    use super::GrammarDir;
    use super::load_config;
    use super::load_queries;
    use super::native_grammars;

    /// `(tag, run text, code)` for every match of `markdown/runnables.scm` in `markdown`.
    fn markdown_runnables(markdown: &str) -> Vec<(String, String, String)> {
        let language = tree_sitter_md::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(markdown, None).unwrap();
        let query = Query::new(&language, &load_queries("markdown").runnables.unwrap()).unwrap();
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
    fn test_markdown_shell_code_blocks_are_runnable() {
        let markdown = "# t\n\n```sh\necho \"hello\"\n```\n\n```bash\nls\n```\n\n```zsh\npwd\n```\n\n```shell\nid\n```\n\n```fish\nset -l x 1\necho $x\n```\n";
        assert_eq!(
            markdown_runnables(markdown),
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
    fn test_markdown_other_code_blocks_are_not_runnable() {
        let markdown = "```rust\nfn main() {}\n```\n\n```\necho untagged\n```\n\n```sh\n```\n\n    echo indented\n";
        assert_eq!(markdown_runnables(markdown), []);
    }

    #[test]
    fn test_every_config_loads_and_names_a_native_grammar() {
        let grammars: Vec<_> = native_grammars().into_iter().map(|(name, _)| name).collect();
        let names: Vec<_> =
            GrammarDir::iter().filter_map(|path| Some(path.strip_suffix("/config.toml")?.to_owned())).collect();
        assert!(names.len() > grammars.len());
        for name in names {
            if let Some(grammar) = load_config(&name).grammar {
                assert!(grammars.contains(&grammar.as_ref()), "{name}: unknown grammar {grammar:?}");
            }
        }
    }
}
