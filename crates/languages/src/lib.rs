use std::sync::Arc;

use futures::FutureExt as _;
use gpui::App;
use gpui::SharedString;
use language::language_settings::AllLanguageSettings;
use language::language_settings::LanguageSettings;
#[allow(clippy::wildcard_imports)]
pub use language::*;
use project::Fs;
use project::library::LibraryProviders;
use python::PyprojectTomlManifestProvider;
use rust::CargoLibraryProvider;
use rust::CargoManifestProvider;
use settings::SemanticTokenRules;

use crate::json::JsonTaskProvider;

mod bash;
mod biome;
mod css;
mod diff;
mod env;
mod git_commit;
mod go;
mod html;
mod json;
mod markdown;
mod npx;
mod package_json;
mod plain_text;
mod python;
mod rust;
mod tombi;
mod tsgo;
mod typescript;
mod yaml;

pub(crate) use package_json::PackageJson;
pub(crate) use package_json::PackageJsonData;

pub fn init(languages: Arc<LanguageRegistry>, fs: Arc<dyn Fs>, cx: &mut App) {
    languages.register_native_grammars(grammars::native_grammars());

    let bash_lsp_adapter = Arc::new(bash::BashLspAdapter);
    let css_lsp_adapter = Arc::new(css::CssLspAdapter);
    let biome_lsp_adapter = Arc::new(biome::BiomeLspAdapter::new(fs.clone()));
    let go_context_provider = Arc::new(go::GoContextProvider);
    let go_lsp_adapter = Arc::new(go::GoLspAdapter);
    let html_lsp_adapter = Arc::new(html::HtmlLspAdapter);
    let json_context_provider = Arc::new(JsonTaskProvider);
    let json_lsp_adapter = Arc::new(json::JsonLspAdapter::new(languages.clone()));
    let python_context_provider = Arc::new(python::PythonContextProvider);
    let rust_context_provider = Arc::new(rust::RustContextProvider);
    let rust_lsp_adapter = Arc::new(rust::RustLspAdapter);
    let typescript_context = Arc::new(typescript::TypeScriptContextProvider::new(fs.clone()));
    let tombi_lsp_adapter = Arc::new(tombi::TombiLspAdapter);
    let tsgo_lsp_adapter = Arc::new(tsgo::TsgoLspAdapter::new(fs));
    let yaml_lsp_adapter = Arc::new(yaml::YamlLspAdapter);

    let built_in_languages = [
        LanguageInfo {
            name: "bash",
            adapters: vec![bash_lsp_adapter],
            context: Some(Arc::new(bash::bash_task_context())),
            manifest_name: None,
            semantic_token_rules: None,
            settings: bash::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "css",
            adapters: vec![css_lsp_adapter, biome_lsp_adapter.clone()],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: css::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "diff",
            adapters: vec![],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: diff::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "env",
            adapters: vec![],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: env::settings(),
            file_types: &[".env.*"],
        },
        LanguageInfo {
            name: "go",
            adapters: vec![go_lsp_adapter.clone()],
            context: Some(go_context_provider.clone()),
            manifest_name: None,
            semantic_token_rules: Some(go::semantic_token_rules()),
            settings: go::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "gomod",
            adapters: vec![go_lsp_adapter.clone()],
            context: Some(go_context_provider.clone()),
            manifest_name: None,
            semantic_token_rules: None,
            settings: go::mod_settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "gowork",
            adapters: vec![go_lsp_adapter],
            context: Some(go_context_provider),
            manifest_name: None,
            semantic_token_rules: None,
            settings: go::mod_settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "html",
            adapters: vec![html_lsp_adapter],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: html::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "json",
            adapters: vec![json_lsp_adapter.clone(), biome_lsp_adapter.clone()],
            context: Some(json_context_provider.clone()),
            manifest_name: None,
            semantic_token_rules: None,
            settings: json::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "jsonc",
            adapters: vec![json_lsp_adapter, biome_lsp_adapter.clone()],
            context: Some(json_context_provider),
            manifest_name: None,
            semantic_token_rules: None,
            settings: json::settings(),
            file_types: &["tsconfig*.json"],
        },
        LanguageInfo {
            name: "markdown",
            adapters: vec![],
            context: Some(Arc::new(markdown::markdown_task_context())),
            manifest_name: None,
            semantic_token_rules: None,
            settings: markdown::settings(),
            file_types: &[".rules", ".cursorrules", ".windsurfrules", ".clinerules"],
        },
        LanguageInfo {
            name: "markdown-inline",
            adapters: vec![],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: markdown::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "python",
            adapters: vec![Arc::new(python::TyLspAdapter), Arc::new(python::RuffLspAdapter)],
            context: Some(python_context_provider),
            manifest_name: Some(SharedString::new_static("pyproject.toml").into()),
            semantic_token_rules: Some(python::semantic_token_rules()),
            settings: python::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "rust",
            adapters: vec![rust_lsp_adapter],
            context: Some(rust_context_provider),
            manifest_name: Some(SharedString::new_static("Cargo.toml").into()),
            semantic_token_rules: Some(rust::semantic_token_rules()),
            settings: rust::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "toml",
            adapters: vec![tombi_lsp_adapter],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: tombi::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "tsx",
            adapters: vec![tsgo_lsp_adapter.clone(), biome_lsp_adapter.clone()],
            context: Some(typescript_context.clone()),
            manifest_name: None,
            semantic_token_rules: None,
            settings: typescript::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "typescript",
            adapters: vec![tsgo_lsp_adapter.clone(), biome_lsp_adapter.clone()],
            context: Some(typescript_context.clone()),
            manifest_name: None,
            semantic_token_rules: None,
            settings: typescript::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "javascript",
            adapters: vec![tsgo_lsp_adapter, biome_lsp_adapter],
            context: Some(typescript_context),
            manifest_name: None,
            semantic_token_rules: None,
            settings: typescript::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "yaml",
            adapters: vec![yaml_lsp_adapter],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: yaml::settings(),
            file_types: &[],
        },
        LanguageInfo {
            name: "gitcommit",
            adapters: vec![],
            context: None,
            manifest_name: None,
            semantic_token_rules: None,
            settings: git_commit::settings(),
            file_types: &[],
        },
    ];

    for registration in built_in_languages {
        register_language(
            &languages,
            registration.name,
            registration.adapters,
            registration.context,
            registration.manifest_name,
            registration.semantic_token_rules,
            registration.settings,
            registration.file_types,
            cx,
        );
    }

    AllLanguageSettings::register(PLAIN_TEXT.name(), plain_text::settings(), &[], None, cx);

    let manifest_providers: [Arc<dyn ManifestProvider>; 2] =
        [Arc::from(CargoManifestProvider), Arc::from(PyprojectTomlManifestProvider)];
    for provider in manifest_providers {
        project::ManifestProvidersStore::global(cx).register(provider);
    }
    LibraryProviders::register(Arc::new(CargoLibraryProvider), cx);
}

struct LanguageInfo {
    name: &'static str,
    adapters: Vec<Arc<dyn LspAdapter>>,
    context: Option<Arc<dyn ContextProvider>>,
    manifest_name: Option<ManifestName>,
    semantic_token_rules: Option<SemanticTokenRules>,
    settings: LanguageSettings,
    /// File globs that select the language before path suffixes do.
    file_types: &'static [&'static str],
}

fn register_language(
    languages: &LanguageRegistry,
    name: &'static str,
    adapters: Vec<Arc<dyn LspAdapter>>,
    context: Option<Arc<dyn ContextProvider>>,
    manifest_name: Option<ManifestName>,
    semantic_token_rules: Option<SemanticTokenRules>,
    settings: LanguageSettings,
    file_types: &[&str],
    cx: &mut App,
) {
    let config = grammars::load_config(name);
    AllLanguageSettings::register(config.name.clone(), settings, file_types, semantic_token_rules, cx);
    for adapter in adapters {
        languages.register_lsp_adapter(config.name.clone(), adapter);
    }
    languages.register_language(
        config.name.clone(),
        config.grammar.clone(),
        config.matcher.clone(),
        config.hidden,
        manifest_name.clone(),
        Arc::new(move || {
            let config = config.clone();
            let context = context.clone();
            let manifest_name = manifest_name.clone();
            async move {
                Ok(LoadedLanguage {
                    config,
                    queries: grammars::load_queries(name),
                    context_provider: context,
                    manifest_name,
                })
            }
            .boxed()
        }),
    );
}

#[cfg(test)]
mod tests {
    use language::language_settings::Formatter;
    use language::language_settings::FormatterList;
    use language::language_settings::RewrapBehavior;
    use language::language_settings::SoftWrap;
    use language::language_settings::WordsCompletionMode;

    use super::*;

    #[test]
    fn test_language_settings_differ_where_languages_need_it() {
        let fallback = LanguageSettings::fallback();
        assert_eq!(rust::settings(), fallback);

        assert!(go::settings().hard_tabs && go::mod_settings().hard_tabs);
        assert!(go::settings().code_actions_on_format.contains_key("source.organizeImports"));
        assert!(go::mod_settings().code_actions_on_format.is_empty());

        for settings in [json::settings(), typescript::settings(), yaml::settings(), markdown::settings()] {
            assert_eq!(settings.tab_size.get(), 2);
        }
        assert!(!yaml::settings().auto_indent_on_paste);

        let diff = diff::settings();
        assert!(!diff.remove_trailing_whitespace_on_save && !diff.ensure_final_newline_on_save);

        let markdown = markdown::settings();
        assert_eq!(markdown.soft_wrap, SoftWrap::EditorWidth);
        assert_eq!(markdown.word_completions, WordsCompletionMode::Disabled);
        assert_eq!(markdown.allow_rewrap, RewrapBehavior::Anywhere);

        for settings in [plain_text::settings(), git_commit::settings()] {
            assert_eq!(settings.soft_wrap, SoftWrap::EditorWidth);
            assert_eq!(settings.allow_rewrap, RewrapBehavior::Anywhere);
        }

        assert!(matches!(python::settings().formatter, FormatterList::Single(Formatter::LanguageServer(_))));
    }
}
