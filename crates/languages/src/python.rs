use std::borrow::Cow;
use std::fmt::Write;
use std::iter;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::str;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::App;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::SharedString;
use gpui::Task;
use language::Buffer;
use language::ContextLocation;
use language::ContextProvider;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::ManifestName;
use language::ManifestProvider;
use language::ManifestQuery;
use language::Symbol;
use language::language_settings::DocumentSymbols;
use language::language_settings::FormatOnSave;
use language::language_settings::Formatter;
use language::language_settings::FormatterList;
use language::language_settings::LanguageServerFormatterSpecifier;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SoftWrap;
use language::language_settings::WordsCompletionMode;
use lsp::CompletionItemKind;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use lsp::Uri;
use rustc_hash::FxHashMap;
use serde_json::Value;
use serde_json::json;
use settings::SemanticTokenRules;
use task::TaskTemplate;
use task::TaskTemplates;
use task::VariableName;
use util::rel_path::RelPath;

use crate::LanguageDef;

fn semantic_token_rules() -> SemanticTokenRules {
    serde_json::from_str(include_str!("python/semantic_token_rules.json"))
        .expect("failed to parse python semantic_token_rules.json")
}

/// The project virtual environment, always `.venv` at the worktree root.
const VENV_DIR: &str = ".venv";

/// Directory inside a virtual environment that holds its executables.
const VENV_BIN_DIR: &str = if cfg!(windows) { "Scripts" } else { "bin" };

/// Resolves a Python tool run as a language server: the project's
/// `.venv` first, then the shell `PATH`, then `uvx <tool>`, which runs the
/// latest release from uv's cache, downloading it on first use.
async fn python_tool_binary(
    delegate: &Arc<dyn LspAdapterDelegate>,
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
            let pyproject_path = path.join(RelPath::from_unix_str("pyproject.toml").unwrap());
            if delegate.exists(&pyproject_path, Some(false)) {
                if innermost_pyproject.is_none() {
                    innermost_pyproject = Some(Arc::from(path));
                }

                let lockfile_path = path.join(RelPath::from_unix_str("uv.lock").unwrap());
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
            let text = format!("def {}():\n", name);
            let filter_range = 4..4 + name.len();
            let display_range = 0..filter_range.end;
            (text, filter_range, display_range)
        }
        language::SymbolKind::Class => {
            let text = format!("class {}:", name);
            let filter_range = 6..6 + name.len();
            let display_range = 0..filter_range.end;
            (text, filter_range, display_range)
        }
        language::SymbolKind::Constant => {
            let text = format!("{} = 0", name);
            let filter_range = 0..name.len();
            let display_range = 0..filter_range.end;
            (text, filter_range, display_range)
        }
        _ => return None,
    };
    Some(language::CodeLabel::new(
        text[display_range.clone()].to_string(),
        filter_range,
        language.highlight_text(text.as_str(), display_range).collect(),
    ))
}

/// Returns the highlight ID for the given completion item kind, if it is supported.
///
/// The outer `Option` is `None` if the item kind returned by the language server is not covered.
/// The inner `Option` is `None` if the item kind is covered, but the highlight name is not present in the grammar.
fn highlight_id_for_completion(
    item_kind: CompletionItemKind,
    grammar: &Arc<language::Grammar>,
) -> Option<Option<language::HighlightId>> {
    match item_kind {
        CompletionItemKind::METHOD => Some(grammar.highlight_id_for_name("function.method.call")),
        CompletionItemKind::FUNCTION => Some(grammar.highlight_id_for_name("function.call")),
        CompletionItemKind::CLASS => Some(grammar.highlight_id_for_name("type")),
        CompletionItemKind::CONSTANT => Some(grammar.highlight_id_for_name("constant")),
        CompletionItemKind::VARIABLE => Some(grammar.highlight_id_for_name("variable")),
        _ => None,
    }
}

pub struct TyLspAdapter;

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
        item: &lsp::CompletionItem,
        language: &Arc<language::Language>,
    ) -> Option<language::CodeLabel> {
        let label = &item.label;
        let label_len = label.len();
        let grammar = language.grammar()?;
        let highlight_id = highlight_id_for_completion(item.kind?, grammar)?;

        let mut text = label.clone();
        if let Some(completion_details) = item.label_details.as_ref().and_then(|details| details.detail.as_ref()) {
            write!(&mut text, " {}", completion_details).ok();
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
        symbol: &language::Symbol,
        language: &Arc<language::Language>,
    ) -> Option<language::CodeLabel> {
        label_for_python_symbol(symbol, language)
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &Arc<dyn LspAdapterDelegate>,
        _: Option<Uri>,
        _: &mut AsyncApp,
    ) -> Result<Value> {
        Ok(json!({ "ty": {} }))
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        python_tool_binary(delegate, "ty", &["server"]).await
    }
}

pub(crate) struct RuffLspAdapter;

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
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        python_tool_binary(delegate, "ruff", &["server"]).await
    }
}

pub(crate) struct PythonContextProvider;

const PYTHON_TEST_TARGET_TASK_VARIABLE: VariableName = VariableName::Custom(Cow::Borrowed("PYTHON_TEST_TARGET"));

const PYTHON_MODULE_NAME_TASK_VARIABLE: VariableName = VariableName::Custom(Cow::Borrowed("PYTHON_MODULE_NAME"));

/// Every Python task runs through `uv run python`, so uv picks (and syncs) the
/// project's `.venv`.
fn uv_python_task(label: String, args: impl IntoIterator<Item = String>) -> TaskTemplate {
    TaskTemplate {
        label,
        command: "uv".to_owned(),
        args: ["run".to_owned(), "python".to_owned()].into_iter().chain(args).collect(),
        cwd: Some(VariableName::WorktreeRoot.template_value()),
        ..TaskTemplate::default()
    }
}

impl ContextProvider for PythonContextProvider {
    fn build_context(
        &self,
        variables: &task::TaskVariables,
        _: ContextLocation<'_>,
        _: Option<FxHashMap<String, String>>,
        _: &mut gpui::App,
    ) -> Task<Result<task::TaskVariables>> {
        let test_target = self.build_pytest_target(variables);
        let module_target = self.build_module_target(variables);

        Task::ready(Ok(task::TaskVariables::from_iter(test_target.into_iter().chain([module_target]))))
    }

    fn associated_tasks(&self, _: Option<Entity<Buffer>>, _: &App) -> Task<Option<TaskTemplates>> {
        let file = VariableName::File.template_value_with_whitespace();
        let test_target = PYTHON_TEST_TARGET_TASK_VARIABLE.template_value_with_whitespace();

        let mut tasks = vec![
            uv_python_task(
                "execute selection".to_owned(),
                ["-c".to_owned(), VariableName::SelectedText.template_value_with_whitespace()],
            ),
            uv_python_task(format!("run '{}'", VariableName::File.template_value()), [file.clone()]),
            TaskTemplate {
                tags: vec!["python-module-main-method".to_owned()],
                ..uv_python_task(
                    format!("run module '{}'", VariableName::File.template_value()),
                    ["-m".to_owned(), PYTHON_MODULE_NAME_TASK_VARIABLE.template_value()],
                )
            },
        ];

        let runner = "pytest";
        let target_tags = ["python-pytest-class", "python-pytest-method"];
        tasks.extend([
            // Run tests for an entire file
            uv_python_task(
                format!("{runner} '{}'", VariableName::File.template_value()),
                ["-m".to_owned(), runner.to_owned(), file],
            ),
            // Run test(s) for a specific target within a file
            TaskTemplate {
                tags: target_tags.map(ToOwned::to_owned).to_vec(),
                ..uv_python_task(
                    format!("{runner} $ED_CUSTOM_PYTHON_TEST_TARGET"),
                    ["-m".to_owned(), runner.to_owned(), test_target],
                )
            },
        ]);

        Task::ready(Some(TaskTemplates(tasks)))
    }
}

impl PythonContextProvider {
    fn build_pytest_target(&self, variables: &task::TaskVariables) -> Option<(VariableName, String)> {
        let file_path = variables.get(&VariableName::RelativeFile)?;

        let pytest_class_name = variables.get(&VariableName::Custom(Cow::Borrowed("_pytest_class_name")));

        let pytest_method_name = variables.get(&VariableName::Custom(Cow::Borrowed("_pytest_method_name")));

        let pytest_target_str = match (pytest_class_name, pytest_method_name) {
            (Some(class_name), Some(method_name)) => {
                format!("{file_path}::{class_name}::{method_name}")
            }
            (Some(class_name), None) => {
                format!("{file_path}::{class_name}")
            }
            (None, Some(method_name)) => {
                format!("{file_path}::{method_name}")
            }
            (None, None) => file_path.to_string(),
        };

        Some((PYTHON_TEST_TARGET_TASK_VARIABLE.clone(), pytest_target_str))
    }

    fn build_module_target(&self, variables: &task::TaskVariables) -> (VariableName, String) {
        let python_module_name = variables
            .get(&VariableName::RelativeFile)
            .and_then(python_module_name_from_relative_path)
            .unwrap_or_default();

        (PYTHON_MODULE_NAME_TASK_VARIABLE.clone(), python_module_name)
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
        tab_size: NonZeroU32::new(4).unwrap(),
        hard_tabs: false,
        soft_wrap: SoftWrap::None,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        format_on_save: FormatOnSave::On,
        formatter: FormatterList::Single(Formatter::LanguageServer(LanguageServerFormatterSpecifier::Specific {
            name: "ruff".into(),
        })),
        code_actions_on_format: [("source.organizeImports.ruff".into(), true)].into_iter().collect(),
        remove_trailing_whitespace_on_save: true,
        ensure_final_newline_on_save: true,
        document_symbols: DocumentSymbols::On,
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
