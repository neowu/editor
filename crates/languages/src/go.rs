use std::ffi::OsString;
use std::num::NonZeroU32;
use std::ops::Range;
use std::path::Path;
use std::str;
use std::sync::Arc;
use std::sync::LazyLock;

use anyhow::Result;
use async_trait::async_trait;
use gpui::App;
use gpui::AsyncApp;
use gpui::Task;
use language::BufferSnapshot;
use language::CodeLabel;
use language::ContextProvider;
use language::HighlightId;
use language::Language;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::ResolvedRunnable;
use language::RunContext;
use language::RunnableMatchCapture;
use language::RunnableResolver;
use language::language_settings::Formatter;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SemanticTokenRules;
use language::language_settings::WordsCompletionMode;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use regex::Regex;
use rustc_hash::FxHashMap;
use serde_json::json;
use task::SpawnInTerminal;

use crate::LanguageDef;

fn semantic_token_rules() -> SemanticTokenRules {
    serde_json::from_str(include_str!("go/semantic_token_rules.json"))
        .expect("failed to parse go semantic_token_rules.json")
}

fn server_binary_arguments() -> Vec<OsString> {
    vec!["-mode=stdio".into()]
}

#[derive(Copy, Clone)]
pub(crate) struct GoLspAdapter;

impl GoLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("gopls");
}

static GO_ESCAPE_SUBTEST_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[.*+?^${}()|\[\]\\"']"#).expect("Failed to create GO_ESCAPE_SUBTEST_NAME_REGEX"));

#[async_trait(?Send)]
impl LspAdapter for GoLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate.which(Self::SERVER_NAME.as_ref()).await?;
        Some(LanguageServerBinary { path, arguments: server_binary_arguments(), env: None })
    }

    async fn initialization_options(
        self: Arc<Self>,
        _: &Arc<LspAdapterDelegate>,
        _: &mut AsyncApp,
    ) -> Result<Option<serde_json::Value>> {
        Ok(Some(json!({
            "usePlaceholders": false,
            "hints": {
                "assignVariableTypes": true,
                "compositeLiteralFields": true,
                "compositeLiteralTypes": true,
                "constantValues": true,
                "functionTypeParameters": true,
                "parameterNames": true,
                "rangeVariableTypes": true
            },
            "semanticTokens": true
        })))
    }

    async fn label_for_completion(
        &self,
        completion: &lsp_types::CompletionItem,
        language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        let label = &completion.label;

        // Gopls returns nested fields and methods as completions.
        // To syntax highlight these, combine their final component
        // with their detail.
        let name_offset = label.rfind('.').unwrap_or(0);

        match completion.kind.zip(completion.detail.as_ref()) {
            Some((lsp_types::CompletionItemKind::MODULE, detail)) => {
                let text = format!("{label} {detail}");
                let source = format!("import {text}");
                let runs = language.highlight_text(source.as_str(), 7..7 + text[name_offset..].len()).collect();
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| text.find(filter_text).map(|start| start..start + filter_text.len()))
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp_types::CompletionItemKind::CONSTANT | lsp_types::CompletionItemKind::VARIABLE, detail)) => {
                let text = format!("{label} {detail}");
                let source = format!("var {} {}", &text[name_offset..], detail);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 4..4 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| text.find(filter_text).map(|start| start..start + filter_text.len()))
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp_types::CompletionItemKind::STRUCT, _)) => {
                let text = format!("{label} struct {{}}");
                let source = format!("type {}", &text[name_offset..]);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 5..5 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| text.find(filter_text).map(|start| start..start + filter_text.len()))
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp_types::CompletionItemKind::INTERFACE, _)) => {
                let text = format!("{label} interface {{}}");
                let source = format!("type {}", &text[name_offset..]);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 5..5 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| text.find(filter_text).map(|start| start..start + filter_text.len()))
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp_types::CompletionItemKind::FIELD, detail)) => {
                let text = format!("{label} {detail}");
                let source = format!("type T struct {{ {} }}", &text[name_offset..]);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 16..16 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| text.find(filter_text).map(|start| start..start + filter_text.len()))
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp_types::CompletionItemKind::FUNCTION | lsp_types::CompletionItemKind::METHOD, detail)) => {
                if let Some(signature) = detail.strip_prefix("func") {
                    let text = format!("{label}{signature}");
                    let source = format!("func {} {{}}", &text[name_offset..]);
                    let runs = adjust_runs(
                        name_offset,
                        language.highlight_text(source.as_str(), 5..5 + text[name_offset..].len()),
                    );
                    let filter_range = completion
                        .filter_text
                        .as_deref()
                        .and_then(|filter_text| text.find(filter_text).map(|start| start..start + filter_text.len()))
                        .unwrap_or(0..label.len());
                    return Some(CodeLabel::new(text, filter_range, runs));
                }
            }
            _ => {}
        }
        None
    }

    async fn label_for_symbol(&self, symbol: &language::Symbol, language: &Arc<Language>) -> Option<CodeLabel> {
        let name = &symbol.name;
        let (text, filter_range, display_range) = match symbol.kind {
            language::SymbolKind::Method | language::SymbolKind::Function => {
                let text = format!("func {name} () {{}}");
                let filter_range = 5..5 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Struct => {
                let text = format!("type {name} struct {{}}");
                let filter_range = 5..5 + name.len();
                let display_range = 0..text.len();
                (text, filter_range, display_range)
            }
            language::SymbolKind::Interface => {
                let text = format!("type {name} interface {{}}");
                let filter_range = 5..5 + name.len();
                let display_range = 0..text.len();
                (text, filter_range, display_range)
            }
            language::SymbolKind::Class => {
                let text = format!("type {name} T");
                let filter_range = 5..5 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Constant => {
                let text = format!("const {name} = nil");
                let filter_range = 6..6 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Variable => {
                let text = format!("var {name} = nil");
                let filter_range = 4..4 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Module => {
                let text = format!("package {name}");
                let filter_range = 8..8 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            _ => return None,
        };

        Some(CodeLabel::new(
            text[display_range.clone()].to_string(),
            filter_range,
            language.highlight_text(text.as_str(), display_range).collect(),
        ))
    }

    fn diagnostic_message_to_markdown(&self, message: &str) -> Option<String> {
        static REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)\n\s*").expect("Failed to create REGEX"));
        Some(REGEX.replace_all(message, "\n\n").to_string())
    }
}

fn adjust_runs(
    delta: usize,
    runs: impl Iterator<Item = (Range<usize>, HighlightId)>,
) -> Vec<(Range<usize>, HighlightId)> {
    runs.map(|(range, highlight_id)| (range.start + delta..range.end + delta, highlight_id)).collect()
}

pub(crate) struct GoContextProvider;

pub(crate) struct GoRunnableResolver;

impl RunnableResolver for GoRunnableResolver {
    fn resolve(
        &self,
        local_captures: &[RunnableMatchCapture],
        shared_captures: &[RunnableMatchCapture],
        buffer: &BufferSnapshot,
    ) -> Option<ResolvedRunnable> {
        const FIELD_CHECK: &str = "_field_check";
        const FIELD_NAME: &str = "_field_name";
        const TABLE_TEST_CASE_NAME: &str = "_table_test_case_name";

        // A row may declare several string fields (e.g. `{ name: "x", label: "y" }`), so
        // the query emits one `@_field_name` + `@run` pair per field, in source order.
        //
        // When the loop body calls `t.Run(tc.<field>, ...)`, `@_field_check` names that
        // field; pick the pair whose `@_field_name` matches it. Without a `@_field_check`
        // (e.g. map-keyed tables) the first pair wins.
        let reference_text = shared_captures
            .iter()
            .find(|capture| capture.name() == Some(FIELD_CHECK))
            .map(|capture| buffer.text_for_range(capture.range()).collect::<String>());
        let pair_index = match &reference_text {
            Some(reference) => local_captures
                .iter()
                .filter(|capture| capture.name() == Some(FIELD_NAME))
                .position(|capture| buffer.text_for_range(capture.range()).equals_str(reference))?,
            None => 0,
        };

        // `@run` and `@_table_test_case_name` tag the same string literal, so the chosen
        // run's text is the case name.
        let run_capture = local_captures.iter().filter(|capture| capture.is_run()).nth(pair_index)?;
        Some(ResolvedRunnable {
            run_range: run_capture.range(),
            extra_captures: smallvec::smallvec![(
                TABLE_TEST_CASE_NAME.to_owned(),
                buffer.text_for_range(run_capture.range()).collect(),
            )],
        })
    }
}

impl ContextProvider for GoContextProvider {
    fn runnables(&self, cx: RunContext, _: &App) -> Task<Vec<SpawnInTerminal>> {
        Task::ready(go_tasks(&cx).into_iter().collect())
    }

    fn runnable_resolver(&self) -> Option<Arc<dyn RunnableResolver>> {
        Some(Arc::new(GoRunnableResolver))
    }
}

/// Runs from the file's folder, so `go` acts on that package.
pub fn go_tasks(cx: &RunContext) -> Option<SpawnInTerminal> {
    let dir = cx.dir()?;
    let package = go_package_label(&dir, cx.worktree_root.as_deref());
    let symbol = || cx.symbol.as_deref();
    let capture = |name| cx.capture(name).map(extract_subtest_name);
    let (label, args): (String, Vec<String>) = match cx.tag.as_ref() {
        "go-testify-suite" => {
            let (suite, symbol) = (capture("_suite_name")?, symbol()?);
            (
                format!("go test {package} -v -run Test{suite}/{symbol}"),
                vec!["test".into(), "-v".into(), "-run".into(), format!("\\^Test{suite}\\$/\\^{symbol}\\$")],
            )
        }
        "go-table-test-case" | "go-table-test-case-without-explicit-variable" => {
            let (case, symbol) = (capture("_table_test_case_name")?, symbol()?);
            (
                format!("go test {package} -v -run {symbol}/{case}"),
                vec!["test".into(), "-v".into(), "-run".into(), format!("\\^{symbol}\\$/\\^{case}\\$")],
            )
        }
        "go-subtest" => {
            let (subtest, symbol) = (capture("_subtest_name").unwrap_or_default(), symbol()?);
            (
                format!("go test {package} -v -run {symbol}/{subtest}"),
                vec!["test".into(), "-v".into(), "-run".into(), format!("\\^{symbol}\\$/\\^{subtest}\\$")],
            )
        }
        "go-test" | "go-example" => {
            let symbol = symbol()?;
            (format!("go test {package} -run {symbol}"), vec!["test".into(), "-run".into(), format!("\\^{symbol}\\$")])
        }
        "go-benchmark" => {
            let symbol = symbol()?;
            (
                format!("go test {package} -bench {symbol}"),
                vec!["test".into(), "-benchmem".into(), "-run='^$'".into(), "-bench".into(), format!("\\^{symbol}\\$")],
            )
        }
        "go-fuzz" => {
            let symbol = symbol()?;
            (
                format!("go test {package} -fuzz=Fuzz -run {symbol}"),
                vec!["test".into(), "-fuzz=Fuzz".into(), "-run".into(), format!("\\^{symbol}\\$")],
            )
        }
        "go-package-test" => (format!("go test {package}"), vec!["test".into()]),
        "go-main" => (format!("go run {package}"), vec!["run".into(), ".".into()]),
        "go-generate" => (format!("go generate {package}"), vec!["generate".into()]),
        _ => return None,
    };
    Some(SpawnInTerminal::new(label, "go", args, Some(dir)))
}

/// `./nested/pkg` (or `.`) relative to the worktree root, else the absolute folder.
fn go_package_label(dir: &Path, worktree_root: Option<&Path>) -> String {
    match worktree_root.and_then(|root| dir.strip_prefix(root).ok()) {
        Some(relative) if relative.as_os_str().is_empty() => ".".to_owned(),
        Some(relative) => format!("./{}", relative.display()),
        None => dir.display().to_string(),
    }
}

fn extract_subtest_name(input: &str) -> String {
    let content =
        if input.starts_with('`') && input.ends_with('`') { input.trim_matches('`') } else { input.trim_matches('"') };

    let processed = content.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect::<String>();

    GO_ESCAPE_SUBTEST_NAME_REGEX.replace_all(&processed, |caps: &regex::Captures| format!("\\{}", &caps[0])).to_string()
}

/// Settings for Go.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).expect("non-zero"),
        hard_tabs: true,
        soft_wrap: false,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        formatter: Formatter::Auto,
        code_actions_on_format: std::iter::once(("source.organizeImports".into(), true)).collect(),
        remove_trailing_whitespace_on_save: true,
        ensure_final_newline_on_save: true,
        lsp_document_symbols: true,
        allow_rewrap: RewrapBehavior::InComments,
        use_on_type_format: true,
        auto_indent_on_paste: true,
        word_completions: WordsCompletionMode::Fallback,
    }
}

/// Settings for Go Mod and Go Work.
pub(crate) fn mod_settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).expect("non-zero"),
        hard_tabs: true,
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
    let adapter: Arc<dyn LspAdapter> = Arc::new(GoLspAdapter);
    let context: Arc<dyn ContextProvider> = Arc::new(GoContextProvider);
    vec![
        LanguageDef {
            grammar: Some(tree_sitter_go::LANGUAGE.into()),
            adapters: vec![Arc::clone(&adapter)],
            context: Some(Arc::clone(&context)),
            semantic_token_rules: Some(semantic_token_rules()),
            ..LanguageDef::new("go", settings())
        },
        LanguageDef {
            adapters: vec![Arc::clone(&adapter)],
            context: Some(Arc::clone(&context)),
            ..LanguageDef::new("gomod", mod_settings())
        },
        LanguageDef { adapters: vec![adapter], context: Some(context), ..LanguageDef::new("gowork", mod_settings()) },
    ]
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use language::RunContext;

    use super::go_package_label;
    use super::go_tasks;

    #[test]
    fn go_package_label_is_relative_to_the_worktree() {
        assert_eq!(go_package_label(Path::new("/w"), Some(Path::new("/w"))), ".");
        assert_eq!(go_package_label(Path::new("/w/a/b"), Some(Path::new("/w"))), "./a/b");
        assert_eq!(go_package_label(Path::new("/x/a"), Some(Path::new("/w"))), "/x/a");
        assert_eq!(go_package_label(Path::new("/x/a"), None), "/x/a");
    }

    #[test]
    fn go_tasks_build_per_tag() {
        let cx = RunContext {
            tag: "go-test".into(),
            file: PathBuf::from("/w/pkg/a_test.go"),
            worktree_root: Some(PathBuf::from("/w")),
            symbol: Some("TestA".to_owned()),
            ..RunContext::default()
        };
        let task = go_tasks(&cx).unwrap();
        assert_eq!(task.full_label, "go test ./pkg -run TestA");
        assert_eq!(task.command_label, "go test -run \\^TestA\\$");
        assert_eq!(task.cwd, Some(PathBuf::from("/w/pkg")));

        let subtest = RunContext {
            tag: "go-subtest".into(),
            captures: rustc_hash::FxHashMap::from_iter([("_subtest_name".to_owned(), "\"with space\"".to_owned())]),
            ..cx.clone()
        };
        assert_eq!(go_tasks(&subtest).unwrap().full_label, "go test ./pkg -v -run TestA/with_space");

        let main = RunContext { tag: "go-main".into(), symbol: None, ..cx.clone() };
        assert_eq!(go_tasks(&main).unwrap().command_label, "go run .");

        // Tags that name a test need its symbol.
        assert!(go_tasks(&RunContext { symbol: None, ..cx.clone() }).is_none());
        assert!(go_tasks(&RunContext { tag: "go-testify-suite".into(), ..cx.clone() }).is_none());
        assert!(go_tasks(&RunContext { tag: "other".into(), ..cx }).is_none());
    }
}
