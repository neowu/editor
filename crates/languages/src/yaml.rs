use std::num::NonZeroU32;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use gpui::AsyncApp;
use language::LanguageName;
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
use language::language_settings::all_language_settings;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use lsp::Uri;
use rustc_hash::FxHashMap;
use serde_json::Value;

use crate::npx;

pub struct YamlLspAdapter;

impl YamlLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("yaml-language-server");
}

#[async_trait(?Send)]
impl LspAdapter for YamlLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let bin = Self::SERVER_NAME;
        npx::node_server_binary(delegate, &[bin.as_ref()], bin.as_ref(), &["--stdio"]).await
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &Arc<dyn LspAdapterDelegate>,
        _: Option<Uri>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let tab_size =
            cx.update(|cx| all_language_settings(cx).language(Some(&LanguageName::new_static("YAML"))).tab_size);
        Ok(serde_json::json!({
            "[yaml]": {"editor.tabSize": tab_size},
            "yaml": {"format": {"enable": true}}
        }))
    }
}

/// Settings for YAML.
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
        auto_indent_on_paste: false,
        word_completions: WordsCompletionMode::Fallback,
    }
}
