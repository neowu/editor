use std::num::NonZeroU32;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::AsyncApp;
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
use serde_json::json;

use crate::npx;

pub struct CssLspAdapter;

#[async_trait(?Send)]
impl LspAdapter for CssLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName("vscode-css-language-server".into())
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        npx::node_server_binary(delegate, &["vscode-langservers-extracted"], "vscode-css-language-server", &["--stdio"])
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
        _: &mut AsyncApp,
    ) -> Result<serde_json::Value> {
        Ok(json!({
            "css": {
                "lint": {}
            },
            "less": {
                "lint": {}
            },
            "scss": {
                "lint": {}
            }
        }))
    }
}

/// Settings for CSS.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).unwrap(),
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
