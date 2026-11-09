use std::num::NonZeroU32;
use std::sync::Arc;

use async_trait::async_trait;
use gpui::AsyncApp;
use language::LanguageServerName;
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
use rustc_hash::FxHashMap;

use crate::npx;

pub struct TombiLspAdapter;

impl TombiLspAdapter {
    const PACKAGE_NAME: &str = "tombi";
}

#[async_trait(?Send)]
impl LspAdapter for TombiLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName::new_static(Self::PACKAGE_NAME)
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        npx::node_server_binary(delegate, &[Self::PACKAGE_NAME], Self::PACKAGE_NAME, &["lsp"]).await
    }
}

/// Settings for TOML.
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
