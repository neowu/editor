use std::num::NonZeroU32;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::SharedString;
use gpui::Task;
use language::ContextProvider;
use language::LanguageName;
use language::LanguageRegistry;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::RunContext;
use language::language_settings::AllLanguageSettings;
use language::language_settings::Formatter;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::WordsCompletionMode;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use lsp_types::Uri;
use project::Fs;
use rustc_hash::FxHashMap;
use serde_json::Value;
use serde_json::json;
use task::SpawnInTerminal;

use crate::LanguageDef;
use crate::PackageJsonData;
use crate::npx;

pub(crate) struct JsonTaskProvider {
    fs: Arc<Fs>,
}

impl ContextProvider for JsonTaskProvider {
    fn runnables(&self, cx: RunContext, app: &App) -> Task<Vec<SpawnInTerminal>> {
        let Some(script) = cx.capture("script").map(ToOwned::to_owned) else {
            return Task::ready(Vec::new());
        };
        let file_name = cx.file.file_name().and_then(|name| name.to_str());
        match (cx.tag.as_ref(), file_name) {
            ("package-script", Some("package.json")) => {
                let fs = Arc::clone(&self.fs);
                app.background_spawn(async move {
                    let package_json = fs
                        .load(&cx.file)
                        .await
                        .ok()
                        .and_then(|contents| serde_json::from_str::<FxHashMap<String, Value>>(&contents).ok());
                    let package_manager = package_json.and_then(|package_json| {
                        PackageJsonData::new(cx.file.as_path().into(), package_json).package_manager
                    });
                    vec![package_script_task(&cx, package_manager.unwrap_or("npm"), script)]
                })
            }
            ("composer-script", Some("composer.json")) => Task::ready(vec![composer_script_task(&cx, script)]),
            _ => Task::ready(Vec::new()),
        }
    }
}

pub fn package_script_task(cx: &RunContext, package_manager: &str, script: String) -> SpawnInTerminal {
    SpawnInTerminal::new(format!("package script {script}"), package_manager, vec!["run".into(), script], cx.dir())
}

pub fn composer_script_task(cx: &RunContext, script: String) -> SpawnInTerminal {
    let dir = cx.dir().unwrap_or_default().to_string_lossy().into_owned();
    SpawnInTerminal::new(
        format!("composer script {script}"),
        "composer",
        vec!["-d".into(), dir, script],
        cx.worktree_root.clone(),
    )
}

pub(crate) struct JsonLspAdapter {
    languages: Arc<LanguageRegistry>,
}

impl JsonLspAdapter {
    pub(crate) const fn new(languages: Arc<LanguageRegistry>) -> Self {
        Self { languages }
    }
}

#[async_trait(?Send)]
impl LspAdapter for JsonLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName("json-language-server".into())
    }

    fn binary_name(&self) -> String {
        "vscode-json-language-server".into()
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        npx::node_server_binary(
            delegate,
            &["vscode-langservers-extracted"],
            "vscode-json-language-server",
            &["--stdio"],
        )
        .await
    }

    async fn initialization_options(
        self: Arc<Self>,
        _: &Arc<LspAdapterDelegate>,
        _: &mut AsyncApp,
    ) -> Result<Option<Value>> {
        Ok(Some(json!({
            "provideFormatter": true
        })))
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &Arc<LspAdapterDelegate>,
        _: Option<Uri>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        Ok(cx.update(|cx| {
            let schemas = schema_associations(&jsonc_globs(&self.languages, cx));

            // This can be viewed via `dev: open language server logs` -> `json-language-server` ->
            // `Server Info`
            serde_json::json!({
                "json": {
                    "format": {
                        "enable": true,
                    },
                    "validate": {
                        "enable": true,
                    },
                    "schemas": schemas
                }
            })
        }))
    }

    fn language_ids(&self) -> FxHashMap<LanguageName, String> {
        [(LanguageName::new_static("JSON"), "json".into()), (LanguageName::new_static("JSONC"), "jsonc".into())]
            .into_iter()
            .collect()
    }

    async fn process_completions(&self, items: &mut [lsp_types::CompletionItem]) {
        unquote_filter_texts(items);
    }
}

/// Globs matching every JSONC file: the language's path suffixes plus `file_types` overrides.
fn jsonc_globs(languages: &Arc<LanguageRegistry>, cx: &App) -> Vec<SharedString> {
    let extension_globs = languages
        .available_language_for_name("JSONC")
        .map(|language| language.matcher().path_suffixes.clone())
        .into_iter()
        .flatten()
        // Path suffixes can be entire file names or just their extensions.
        .flat_map(|path_suffix| [format!("*.{path_suffix}").into(), path_suffix]);
    let override_globs = AllLanguageSettings::get_global(cx)
        .file_types
        .get("JSONC")
        .into_iter()
        .flat_map(|(_, glob_strings)| glob_strings.iter().map(SharedString::from));
    extension_globs.chain(override_globs).collect()
}

/// The server downloads the SchemaStore schemas itself; the JSONC schema is inline.
fn schema_associations(jsonc_globs: &[SharedString]) -> Value {
    json!([
        {
            "fileMatch": ["tsconfig*.json"],
            "url": "https://json.schemastore.org/tsconfig.json"
        },
        {
            "fileMatch": ["package.json"],
            "url": "https://json.schemastore.org/package.json"
        },
        {
            "fileMatch": jsonc_globs,
            "schema": { "allowTrailingCommas": true }
        },
    ])
}

/// json-language-server quotes `filterText` (`"status_bar"`), or the label of items
/// without one (`"dark"`), but the completion query never includes the opening
/// quote, which skews fuzzy match scores.
fn unquote_filter_texts(items: &mut [lsp_types::CompletionItem]) {
    for item in items {
        let filter_text = item.filter_text.as_deref().unwrap_or(&item.label);
        if filter_text.starts_with('"')
            && let Ok(unquoted) = serde_json::from_str::<String>(filter_text)
        {
            item.filter_text = Some(unquoted);
        }
    }
}

/// Settings for JSON and JSONC.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(2).expect("non-zero"),
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

pub(crate) fn languages(registry: Arc<LanguageRegistry>, biome: Arc<dyn LspAdapter>, fs: Arc<Fs>) -> Vec<LanguageDef> {
    let adapter: Arc<dyn LspAdapter> = Arc::new(JsonLspAdapter::new(registry));
    let context: Arc<dyn ContextProvider> = Arc::new(JsonTaskProvider { fs });
    vec![
        LanguageDef {
            grammar: Some(tree_sitter_json::LANGUAGE.into()),
            adapters: vec![Arc::clone(&adapter), Arc::clone(&biome)],
            context: Some(Arc::clone(&context)),
            ..LanguageDef::new("json", settings())
        },
        LanguageDef {
            grammar: Some(tree_sitter_json::LANGUAGE.into()),
            adapters: vec![adapter, biome],
            context: Some(context),
            file_types: &["tsconfig*.json"],
            ..LanguageDef::new("jsonc", settings())
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn script_tasks_run_from_the_manifest_folder() {
        let cx = RunContext {
            file: PathBuf::from("/w/pkg/package.json"),
            worktree_root: Some(PathBuf::from("/w")),
            ..RunContext::default()
        };
        let task = package_script_task(&cx, "pnpm", "build".to_owned());
        assert_eq!(task.full_label, "package script build");
        assert_eq!(task.command_label, "pnpm run build");
        assert_eq!(task.cwd, Some(PathBuf::from("/w/pkg")));

        let task = composer_script_task(&cx, "test".to_owned());
        assert_eq!(task.full_label, "composer script test");
        assert_eq!(task.command_label, "composer -d /w/pkg test");
        assert_eq!(task.cwd, Some(PathBuf::from("/w")));
    }

    #[test]
    fn schema_associations_use_schemastore_and_inline_jsonc() {
        let schemas = schema_associations(&["*.jsonc".into(), "jsonc".into()]);
        assert_eq!(
            schemas,
            json!([
                { "fileMatch": ["tsconfig*.json"], "url": "https://json.schemastore.org/tsconfig.json" },
                { "fileMatch": ["package.json"], "url": "https://json.schemastore.org/package.json" },
                { "fileMatch": ["*.jsonc", "jsonc"], "schema": { "allowTrailingCommas": true } },
            ])
        );
    }
}
