//! Runs a language's bundled `runnables.scm` over a fixture in `assets/`, the folder that
//! stands in for the worktree root.

use std::path::Path;
use std::path::PathBuf;

use language::RunContext;
use language::RunnableCapture;
use tree_sitter::StreamingIterator as _;

pub fn assets() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
}

/// What `dir`'s `runnables.scm` finds in the fixture at `file` (relative to `assets/`),
/// gathered the way `language` gathers ungrouped matches: the `@run` row and one
/// `RunContext` per tag, with the named captures. The outline symbol is left to the caller.
pub fn runnable_contexts(dir: &str, grammar: tree_sitter::Language, file: &str) -> Vec<(usize, RunContext)> {
    let path = assets().join(file);
    let source = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let language = languages::bundled_language(dir, grammar.clone()).expect("bundled language loads");
    let config = language.grammar().expect("language has a grammar").runnable_config.as_ref().expect("runnables query");
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar).expect("grammar loads");
    let tree = parser.parse(&source, None).expect("fixture parses");
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&config.query, tree.root_node(), source.as_bytes());
    let mut result = Vec::new();
    while let Some(query_match) = matches.next() {
        let mut row = None;
        let mut captures = rustc_hash::FxHashMap::default();
        for capture in query_match.captures() {
            match config.extra_captures.get(capture.index as usize) {
                Some(RunnableCapture::Run) => row = Some(capture.node.start_position().row),
                Some(RunnableCapture::Named(name)) => {
                    captures.insert(name.to_string(), source[capture.node.byte_range()].to_owned());
                }
                _ => {}
            }
        }
        let Some(row) = row else { continue };
        for property in config.query.property_settings(query_match.pattern_index) {
            if &*property.key == "tag"
                && let Some(tag) = &property.value
            {
                let context = RunContext {
                    tag: tag.to_string().into(),
                    captures: captures.clone(),
                    file: path.clone(),
                    worktree_root: Some(assets()),
                    symbol: None,
                };
                result.push((row, context));
            }
        }
    }
    result
}

/// `path` relative to `assets/`, `.` for `assets/` itself.
pub fn relative(path: &Path) -> String {
    match path.strip_prefix(assets()) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}
