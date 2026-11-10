use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::App;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::Task;
use language::Buffer;
use language::ContextProvider;
use language::LanguageName;
use language::LanguageRegistry;
use language::LocalFile as _;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::language_settings::DocumentSymbols;
use language::language_settings::FormatOnSave;
use language::language_settings::Formatter;
use language::language_settings::FormatterList;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SoftWrap;
use language::language_settings::WordsCompletionMode;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use lsp::Uri;
use rustc_hash::FxHashMap;
use serde_json::Value;
use serde_json::json;
use task::TaskTemplate;
use task::TaskTemplates;
use task::VariableName;
use util::rel_path::RelPath;

use crate::LanguageDef;
use crate::PackageJsonData;
use crate::npx;

pub(crate) struct JsonTaskProvider;

impl ContextProvider for JsonTaskProvider {
    fn associated_tasks(&self, buffer: Option<Entity<Buffer>>, cx: &App) -> gpui::Task<Option<TaskTemplates>> {
        let file = buffer.as_ref().and_then(|buf| buf.read(cx).file());
        let Some(file) = project::File::from_dyn(file).cloned() else {
            return Task::ready(None);
        };
        let is_package_json = file.path.ends_with(RelPath::from_unix_str("package.json").unwrap());
        let is_composer_json = file.path.ends_with(RelPath::from_unix_str("composer.json").unwrap());
        if !is_package_json && !is_composer_json {
            return Task::ready(None);
        }

        cx.spawn(async move |cx| {
            let contents = file.worktree.update(cx, |this, cx| this.load_file(&file.path, cx)).await.ok()?;
            let path = cx.update(|cx| file.abs_path(cx)).as_path().into();
            let contents_text = contents.text.to_string();

            let task_templates = if is_package_json {
                let package_json = serde_json::from_str::<FxHashMap<String, serde_json::Value>>(&contents_text).ok()?;
                let package_json = PackageJsonData::new(path, package_json);
                let command = package_json.package_manager.unwrap_or("npm").to_owned();
                package_json
                    .scripts
                    .into_iter()
                    .map(|(_, key)| TaskTemplate {
                        label: format!("run {key}"),
                        command: command.clone(),
                        args: vec!["run".into(), key],
                        cwd: Some(VariableName::Dirname.template_value()),
                        ..TaskTemplate::default()
                    })
                    .chain([TaskTemplate {
                        label: "package script $ED_CUSTOM_script".to_owned(),
                        command: command.clone(),
                        args: vec!["run".into(), VariableName::Custom("script".into()).template_value()],
                        cwd: Some(VariableName::Dirname.template_value()),
                        tags: vec!["package-script".into()],
                        ..TaskTemplate::default()
                    }])
                    .collect()
            } else if is_composer_json {
                serde_json::Value::from_str(&contents_text)
                    .ok()?
                    .get("scripts")?
                    .as_object()?
                    .keys()
                    .map(|key| TaskTemplate {
                        label: format!("run {key}"),
                        command: "composer".to_owned(),
                        args: vec!["-d".into(), "$ED_DIRNAME".into(), key.into()],
                        ..TaskTemplate::default()
                    })
                    .chain([TaskTemplate {
                        label: "composer script $ED_CUSTOM_script".to_owned(),
                        command: "composer".to_owned(),
                        args: vec![
                            "-d".into(),
                            "$ED_DIRNAME".into(),
                            VariableName::Custom("script".into()).template_value(),
                        ],
                        tags: vec!["composer-script".into()],
                        ..TaskTemplate::default()
                    }])
                    .collect()
            } else {
                vec![]
            };

            Some(TaskTemplates(task_templates))
        })
    }
}

pub struct JsonLspAdapter {
    languages: Arc<LanguageRegistry>,
}

impl JsonLspAdapter {
    pub const fn new(languages: Arc<LanguageRegistry>) -> Self {
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
        delegate: &Arc<dyn LspAdapterDelegate>,
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
        _: &Arc<dyn LspAdapterDelegate>,
        _: &mut AsyncApp,
    ) -> Result<Option<serde_json::Value>> {
        Ok(Some(json!({
            "provideFormatter": true
        })))
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &Arc<dyn LspAdapterDelegate>,
        _: Option<Uri>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        Ok(cx.update(|cx| {
            let schemas = json_schema_store::all_schema_file_associations(&self.languages, cx);

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

    async fn process_completions(&self, items: &mut [lsp::CompletionItem]) {
        unquote_filter_texts(items);
    }
}

/// json-language-server quotes `filterText` (`"status_bar"`), or the label of items
/// without one (`"dark"`), but the completion query never includes the opening
/// quote, which skews fuzzy match scores.
fn unquote_filter_texts(items: &mut [lsp::CompletionItem]) {
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
        tab_size: NonZeroU32::new(2).unwrap(),
        hard_tabs: false,
        soft_wrap: SoftWrap::None,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        format_on_save: FormatOnSave::On,
        formatter: FormatterList::Single(Formatter::Auto),
        code_actions_on_format: FxHashMap::default(),
        remove_trailing_whitespace_on_save: true,
        ensure_final_newline_on_save: true,
        document_symbols: DocumentSymbols::On,
        allow_rewrap: RewrapBehavior::InComments,
        use_on_type_format: true,
        auto_indent_on_paste: true,
        word_completions: WordsCompletionMode::Fallback,
    }
}

pub(crate) fn languages(registry: Arc<LanguageRegistry>, biome: Arc<dyn LspAdapter>) -> Vec<LanguageDef> {
    let adapter: Arc<dyn LspAdapter> = Arc::new(JsonLspAdapter::new(registry));
    let context: Arc<dyn ContextProvider> = Arc::new(JsonTaskProvider);
    vec![
        LanguageDef {
            grammar: Some(tree_sitter_json::LANGUAGE.into()),
            adapters: vec![adapter.clone(), biome.clone()],
            context: Some(context.clone()),
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
