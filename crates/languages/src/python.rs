use std::fmt::Write as _;
use std::iter;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::str;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::App;
use gpui::AsyncApp;
use gpui::SharedString;
use gpui::Task;
use language::ContextProvider;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::ManifestName;
use language::ManifestProvider;
use language::ManifestQuery;
use language::RunContext;
use language::Symbol;
use language::language_settings::Formatter;
use language::language_settings::LanguageServerFormatterSpecifier;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SemanticTokenRules;
use language::language_settings::WordsCompletionMode;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use lsp_types::CompletionItemKind;
use lsp_types::Uri;
use serde_json::Value;
use serde_json::json;
use task::SpawnInTerminal;
use util::rel_path::RelPath;

use crate::LanguageDef;

fn semantic_token_rules() -> SemanticTokenRules {
    serde_json::from_str(include_str!("python/semantic_token_rules.json"))
        .expect("failed to parse python semantic_token_rules.json")
}

/// The project virtual environment, always `.venv` at the worktree root.
const VENV_DIR: &str = ".venv";

/// Directory inside a virtual environment that holds its executables.
const VENV_BIN_DIR: &str = "bin";

/// Resolves a Python tool run as a language server: the project's
/// `.venv` first, then the shell `PATH`, then `uvx <tool>`, which runs the
/// latest release from uv's cache, downloading it on first use.
async fn python_tool_binary(
    delegate: &Arc<LspAdapterDelegate>,
    tool: &str,
    arguments: &[&str],
) -> Option<LanguageServerBinary> {
    let env = delegate.shell_env().await;
    let in_venv = delegate.worktree_root_path().join(VENV_DIR).join(VENV_BIN_DIR).join(tool);
    for candidate in [in_venv, PathBuf::from(tool)] {
        if let Some(path) = delegate.which(candidate.as_os_str()).await {
            return Some(LanguageServerBinary {
                path,
                env: Some(env),
                arguments: arguments.iter().map(Into::into).collect(),
            });
        }
    }

    let uvx = delegate.which("uvx".as_ref()).await?;
    Some(LanguageServerBinary {
        path: uvx,
        env: Some(env),
        arguments: iter::once(tool).chain(arguments.iter().copied()).map(Into::into).collect(),
    })
}

pub(crate) struct PyprojectTomlManifestProvider;

impl ManifestProvider for PyprojectTomlManifestProvider {
    fn name(&self) -> ManifestName {
        SharedString::new_static("pyproject.toml").into()
    }

    fn search(&self, ManifestQuery { path, depth, delegate }: ManifestQuery) -> Option<Arc<RelPath>> {
        let mut innermost_pyproject = None;
        let mut outermost_workspace_root = None;

        for path in path.ancestors().take(depth) {
            let pyproject_path = path.join(RelPath::from_unix_str("pyproject.toml").expect("valid relative path"));
            if delegate.exists(&pyproject_path, Some(false)) {
                if innermost_pyproject.is_none() {
                    innermost_pyproject = Some(Arc::from(path));
                }

                let lockfile_path = path.join(RelPath::from_unix_str("uv.lock").expect("valid relative path"));
                if delegate.exists(&lockfile_path, Some(false)) {
                    outermost_workspace_root = Some(Arc::from(path));
                }
            }
        }

        outermost_workspace_root.or(innermost_pyproject)
    }
}

fn label_for_python_symbol(symbol: &Symbol, language: &Arc<language::Language>) -> Option<language::CodeLabel> {
    let name = &symbol.name;
    let (text, filter_range, display_range) = match symbol.kind {
        language::SymbolKind::Method | language::SymbolKind::Function => {
            let text = format!("def {name}():\n");
            let filter_range = 4..4 + name.len();
            let display_range = 0..filter_range.end;
            (text, filter_range, display_range)
        }
        language::SymbolKind::Class => {
            let text = format!("class {name}:");
            let filter_range = 6..6 + name.len();
            let display_range = 0..filter_range.end;
            (text, filter_range, display_range)
        }
        language::SymbolKind::Constant => {
            let text = format!("{name} = 0");
            let filter_range = 0..name.len();
            let display_range = 0..filter_range.end;
            (text, filter_range, display_range)
        }
        // lint: expect(wildcard_match) only some variants matter here
        _ => return None,
    };
    Some(language::CodeLabel::new(
        text[display_range.clone()].to_string(),
        filter_range,
        language.highlight_text(text.as_str(), display_range).collect(),
    ))
}

/// Returns the highlight name for the given completion item kind, or `None` if the kind is not covered.
const fn highlight_name_for_completion(item_kind: CompletionItemKind) -> Option<&'static str> {
    match item_kind {
        CompletionItemKind::METHOD => Some("function.method.call"),
        CompletionItemKind::FUNCTION => Some("function.call"),
        CompletionItemKind::CLASS => Some("type"),
        CompletionItemKind::CONSTANT => Some("constant"),
        CompletionItemKind::VARIABLE => Some("variable"),
        _ => None,
    }
}

struct TyLspAdapter;

impl TyLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("ty");
}

#[async_trait(?Send)]
impl LspAdapter for TyLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }

    async fn label_for_completion(
        &self,
        item: &lsp_types::CompletionItem,
        language: &Arc<language::Language>,
    ) -> Option<language::CodeLabel> {
        let label = &item.label;
        let label_len = label.len();
        let grammar = language.grammar()?;
        let highlight_id = grammar.highlight_id_for_name(highlight_name_for_completion(item.kind?)?);

        let mut text = label.clone();
        if let Some(completion_details) = item.label_details.as_ref().and_then(|details| details.detail.as_ref()) {
            write!(&mut text, " {completion_details}").ok();
        }

        Some(language::CodeLabel::filtered(
            text,
            label_len,
            item.filter_text.as_deref(),
            highlight_id.map(|id| (0..label_len, id)).into_iter().collect(),
        ))
    }

    async fn label_for_symbol(
        &self,
        symbol: &Symbol,
        language: &Arc<language::Language>,
    ) -> Option<language::CodeLabel> {
        label_for_python_symbol(symbol, language)
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &Arc<LspAdapterDelegate>,
        _: Option<Uri>,
        _: &mut AsyncApp,
    ) -> Result<Value> {
        Ok(json!({ "ty": {} }))
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        python_tool_binary(delegate, "ty", &["server"]).await
    }
}

struct RuffLspAdapter;

impl RuffLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("ruff");
}

#[async_trait(?Send)]
impl LspAdapter for RuffLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        python_tool_binary(delegate, "ruff", &["server"]).await
    }
}

struct PythonContextProvider;

impl ContextProvider for PythonContextProvider {
    fn runnables(&self, cx: RunContext, _: &App) -> Task<Vec<SpawnInTerminal>> {
        Task::ready(python_tasks(&cx).into_iter().collect())
    }
}

/// Every Python task runs through `uv run python` from the worktree root, so uv picks
/// (and syncs) the project's `.venv`.
pub fn python_tasks(cx: &RunContext) -> Option<SpawnInTerminal> {
    let worktree_root = cx.worktree_root.as_ref()?;
    let relative_file = cx.file.strip_prefix(worktree_root).ok()?.to_str()?;
    let (label, args) = match cx.tag.as_ref() {
        "python-module-main-method" => {
            let module = python_module_name_from_relative_path(relative_file)?;
            (format!("run module '{}'", cx.file.display()), vec!["-m".to_owned(), module])
        }
        "python-pytest-class" | "python-pytest-method" => {
            let target =
                pytest_target(relative_file, cx.capture("_pytest_class_name"), cx.capture("_pytest_method_name"));
            (format!("pytest {target}"), vec!["-m".to_owned(), "pytest".to_owned(), format!("\"{target}\"")])
        }
        _ => return None,
    };
    let args = ["run".to_owned(), "python".to_owned()].into_iter().chain(args).collect();
    Some(SpawnInTerminal::new(label, "uv", args, Some(worktree_root.clone())))
}

fn pytest_target(file: &str, class_name: Option<&str>, method_name: Option<&str>) -> String {
    match (class_name, method_name) {
        (Some(class_name), Some(method_name)) => format!("{file}::{class_name}::{method_name}"),
        (Some(name), None) | (None, Some(name)) => format!("{file}::{name}"),
        (None, None) => file.to_owned(),
    }
}

fn python_module_name_from_relative_path(relative_path: &str) -> Option<String> {
    let rel_path = RelPath::new(relative_path.as_ref()).ok()?;
    let path_with_dots = rel_path.display().replace('/', ".");
    Some(path_with_dots.strip_suffix(".py").map(ToOwned::to_owned).unwrap_or(path_with_dots))
}

/// Settings for Python.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).expect("non-zero"),
        hard_tabs: false,
        soft_wrap: false,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        formatter: Formatter::LanguageServer(LanguageServerFormatterSpecifier::Specific { name: "ruff".into() }),
        code_actions_on_format: iter::once(("source.organizeImports.ruff".into(), true)).collect(),
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
        grammar: Some(tree_sitter_python::LANGUAGE.into()),
        adapters: vec![Arc::new(TyLspAdapter), Arc::new(RuffLspAdapter)],
        context: Some(Arc::new(PythonContextProvider)),
        manifest_name: Some(SharedString::new_static("pyproject.toml").into()),
        semantic_token_rules: Some(semantic_token_rules()),
        ..LanguageDef::new("python", settings())
    }]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use language::RunContext;

    use super::pytest_target;
    use super::python_tasks;

    #[test]
    fn pytest_target_joins_file_class_and_method() {
        assert_eq!(pytest_target("t.py", Some("A"), Some("b")), "t.py::A::b");
        assert_eq!(pytest_target("t.py", Some("A"), None), "t.py::A");
        assert_eq!(pytest_target("t.py", None, Some("b")), "t.py::b");
        assert_eq!(pytest_target("t.py", None, None), "t.py");
    }

    #[test]
    fn python_tasks_run_through_uv_from_the_worktree_root() {
        let cx = RunContext {
            tag: "python-pytest-method".into(),
            captures: rustc_hash::FxHashMap::from_iter([("_pytest_method_name".to_owned(), "test_it".to_owned())]),
            file: PathBuf::from("/w/pkg/test_a.py"),
            worktree_root: Some(PathBuf::from("/w")),
            ..RunContext::default()
        };
        let task = python_tasks(&cx).unwrap();
        assert_eq!(task.full_label, "pytest pkg/test_a.py::test_it");
        assert_eq!(task.command_label, "uv run python -m pytest \"pkg/test_a.py::test_it\"");
        assert_eq!(task.cwd, Some(PathBuf::from("/w")));

        let task = python_tasks(&RunContext { tag: "python-module-main-method".into(), ..cx.clone() }).unwrap();
        assert_eq!(task.command_label, "uv run python -m pkg.test_a");

        assert!(python_tasks(&RunContext { worktree_root: None, ..cx.clone() }).is_none());
        assert!(python_tasks(&RunContext { tag: "other".into(), ..cx }).is_none());
    }
}
