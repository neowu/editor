use std::num::NonZeroU32;

use language::language_settings::Formatter;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::WordsCompletionMode;
use rustc_hash::FxHashMap;

use crate::LanguageDef;

/// Settings for Env.
fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).expect("non-zero"),
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

pub(crate) fn languages() -> Vec<LanguageDef> {
    vec![LanguageDef {
        grammar: Some(tree_sitter_bash::LANGUAGE.into()),
        file_types: &[".env.*"],
        ..LanguageDef::new("env", settings())
    }]
}
