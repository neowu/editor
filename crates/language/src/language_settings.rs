//! Per-language settings and the editor-wide language constants.
//!
//! Each language's settings are hardcoded next to its language support (in the
//! `languages` crate) and registered with [`AllLanguageSettings::register`]. There is
//! no merging: a language's [`LanguageSettings`] is used as is.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::LazyLock;

use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use gpui::App;
use gpui::Global;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
pub use settings::AutoIndentMode;
use settings::DocumentFoldingRanges;
pub use settings::DocumentSymbols;
pub use settings::FormatOnSave;
pub use settings::Formatter;
pub use settings::FormatterList;
use settings::IndentGuideBackgroundColoring;
use settings::IndentGuideColoring;
pub use settings::InlayHintKind;
pub use settings::LanguageServerFormatterSpecifier;
pub use settings::LineEndingSetting;
pub use settings::LspInsertMode;
pub use settings::RewrapBehavior;
use settings::SemanticTokenRules;
pub use settings::ShowWhitespaceSetting;
pub use settings::SoftWrap;
pub use settings::SoftWrapIndent;
pub use settings::WordsCompletionMode;
use text::ToOffset;

use crate::Buffer;
use crate::BufferSnapshot;
use crate::LanguageName;

/// How to indent soft-wrapped continuation lines.
pub const SOFT_WRAP_INDENT: SoftWrapIndent = SoftWrapIndent::Same;
pub const INDENT_GUIDES: IndentGuideSettings = IndentGuideSettings {
    enabled: true,
    line_width: 1,
    active_line_width: 1,
    coloring: IndentGuideColoring::IndentAware,
    background_coloring: IndentGuideBackgroundColoring::Disabled,
};
/// How line endings are initialized for new files and normalized during format and save.
pub const LINE_ENDING: LineEndingSetting = LineEndingSetting::EnforceLf;
/// Whether folding ranges from language servers replace tree-sitter and indent-based folding.
pub const DOCUMENT_FOLDING_RANGES: DocumentFoldingRanges = DocumentFoldingRanges::Off;
pub const SHOW_WHITESPACES: ShowWhitespaceSetting = ShowWhitespaceSetting::Selection;
pub const WHITESPACE_SPACE: &str = "•";
pub const WHITESPACE_TAB: &str = "→";
pub const INLAY_HINTS: InlayHintSettings = InlayHintSettings {
    show_type_hints: true,
    show_parameter_hints: true,
    show_other_hints: true,
    show_background: false,
    edit_debounce_ms: 700,
    scroll_debounce_ms: 50,
};
pub const AUTO_INDENT: AutoIndentMode = AutoIndentMode::SyntaxAware;
pub const COMPLETIONS: CompletionSettings = CompletionSettings {
    words_min_length: 3,
    lsp: true,
    lsp_fetch_timeout_ms: 0,
    lsp_insert_mode: LspInsertMode::ReplaceSuffix,
};

/// Returns the settings for all languages.
pub fn all_language_settings(cx: &App) -> &AllLanguageSettings {
    AllLanguageSettings::get_global(cx)
}

/// The settings registered for each language.
#[derive(Debug, Clone)]
pub struct AllLanguageSettings {
    /// Settings for buffers without a language or with an unregistered one.
    pub fallback: Arc<LanguageSettings>,
    languages: FxHashMap<LanguageName, Arc<LanguageSettings>>,
    /// Extra file globs per language name, checked before path suffixes.
    pub file_types: FxHashMap<Arc<str>, (GlobSet, Vec<String>)>,
    /// Semantic token rules applied above the defaults for each language.
    semantic_token_rules: FxHashMap<LanguageName, SemanticTokenRules>,
}

impl Global for AllLanguageSettings {}

impl Default for AllLanguageSettings {
    fn default() -> Self {
        Self {
            fallback: Arc::new(LanguageSettings::fallback()),
            languages: FxHashMap::default(),
            file_types: FxHashMap::default(),
            semantic_token_rules: FxHashMap::default(),
        }
    }
}

static FALLBACK: LazyLock<AllLanguageSettings> = LazyLock::new(AllLanguageSettings::default);

/// The settings for a particular language. Settings that never vary by language are
/// the constants in this module.
#[derive(Debug, Clone, PartialEq)]
pub struct LanguageSettings {
    /// How many columns a tab should occupy.
    pub tab_size: NonZeroU32,
    /// Whether to indent lines using tab characters, as opposed to multiple spaces.
    pub hard_tabs: bool,
    pub soft_wrap: SoftWrap,
    /// The column at which to soft-wrap lines, for buffers where soft-wrap is enabled.
    pub preferred_line_length: u32,
    /// Columns at which to show wrap guides (vertical rulers).
    pub wrap_guides: Vec<usize>,
    pub format_on_save: FormatOnSave,
    pub formatter: FormatterList,
    /// Which code actions to run on format.
    pub code_actions_on_format: FxHashMap<String, bool>,
    pub remove_trailing_whitespace_on_save: bool,
    pub ensure_final_newline_on_save: bool,
    /// The source of document symbols used for outlines and breadcrumbs.
    pub document_symbols: DocumentSymbols,
    /// Where the `editor::Rewrap` action is allowed.
    pub allow_rewrap: RewrapBehavior,
    /// Whether to format on LSP on-type-format trigger characters.
    pub use_on_type_format: bool,
    /// Whether indentation of pasted content is adjusted to the context.
    pub auto_indent_on_paste: bool,
    /// How buffer words are offered as completions.
    pub word_completions: WordsCompletionMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionSettings {
    /// How many characters the query needs before word completions show automatically.
    pub words_min_length: usize,
    /// Whether to fetch LSP completions.
    pub lsp: bool,
    /// How long to wait for each language server's completions; 0 waits indefinitely.
    pub lsp_fetch_timeout_ms: u64,
    pub lsp_insert_mode: LspInsertMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndentGuideSettings {
    pub enabled: bool,
    /// Width of the indent guides in pixels, between 1 and 10.
    pub line_width: u32,
    /// Width of the active indent guide in pixels, between 1 and 10.
    pub active_line_width: u32,
    pub coloring: IndentGuideColoring,
    pub background_coloring: IndentGuideBackgroundColoring,
}

impl IndentGuideSettings {
    /// Returns the clamped line width in pixels for an indent guide based on
    /// whether it is active, or `None` when line coloring is disabled.
    pub fn visible_line_width(&self, active: bool) -> Option<u32> {
        if self.coloring == IndentGuideColoring::Disabled {
            return None;
        }
        let width = if active { self.active_line_width } else { self.line_width };
        Some(width.clamp(1, 10))
    }
}

impl LanguageSettings {
    pub fn for_buffer(buffer: &Buffer, cx: &App) -> Arc<LanguageSettings> {
        Self::resolve(Some(buffer), None, cx)
    }

    pub fn for_buffer_at<D: ToOffset>(buffer: &Buffer, position: D, cx: &App) -> Arc<LanguageSettings> {
        let language = buffer.language_at(position);
        Self::resolve(Some(buffer), language.map(|l| l.name()).as_ref(), cx)
    }

    pub fn for_buffer_snapshot(buffer: &BufferSnapshot, offset: Option<usize>, cx: &App) -> Arc<LanguageSettings> {
        let language = if let Some(offset) = offset { buffer.language_at(offset) } else { buffer.language() };

        if let Some(resolved) = buffer.resolved_settings()
            && language == buffer.language()
        {
            return Arc::clone(resolved);
        }

        AllLanguageSettings::get_global(cx).language(language.map(|l| l.name()).as_ref())
    }

    pub fn resolve(
        buffer: Option<&Buffer>,
        override_language: Option<&LanguageName>,
        cx: &App,
    ) -> Arc<LanguageSettings> {
        let Some(buffer) = buffer else {
            return AllLanguageSettings::get_global(cx).language(override_language);
        };

        if let Some(resolved) = buffer.resolved_settings()
            && override_language.is_none_or(|language| Some(language) == buffer.language().map(|l| l.name()).as_ref())
        {
            return resolved.clone();
        }

        Self::resolve_uncached(buffer, override_language, cx)
    }

    pub(crate) fn resolve_uncached(
        buffer: &Buffer,
        override_language: Option<&LanguageName>,
        cx: &App,
    ) -> Arc<LanguageSettings> {
        let buffer_language = buffer.language().map(|l| l.name());
        let language_name = override_language.or(buffer_language.as_ref());
        AllLanguageSettings::get_global(cx).language(language_name)
    }
}

/// The settings for inlay hints.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InlayHintSettings {
    /// Whether type hints should be shown.
    pub show_type_hints: bool,
    /// Whether parameter hints should be shown.
    pub show_parameter_hints: bool,
    /// Whether other hints should be shown.
    pub show_other_hints: bool,
    /// Whether to show a background for inlay hints.
    ///
    /// If set to `true`, the background will use the `hint.background` color
    /// from the current theme.
    pub show_background: bool,
    /// Whether or not to debounce inlay hints updates after buffer edits.
    /// 0 disables debouncing.
    pub edit_debounce_ms: u64,
    /// Whether or not to debounce inlay hints updates after buffer scrolls.
    /// 0 disables debouncing.
    pub scroll_debounce_ms: u64,
}

impl InlayHintSettings {
    /// Returns the kinds of inlay hints that are enabled based on the settings.
    pub fn enabled_inlay_hint_kinds(&self) -> FxHashSet<Option<InlayHintKind>> {
        let mut kinds = FxHashSet::default();
        if self.show_type_hints {
            kinds.insert(Some(InlayHintKind::Type));
        }
        if self.show_parameter_hints {
            kinds.insert(Some(InlayHintKind::Parameter));
        }
        if self.show_other_hints {
            kinds.insert(None);
        }
        kinds
    }
}

impl AllLanguageSettings {
    pub fn get_global(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or(&FALLBACK)
    }

    /// Registers a language's settings, plus file globs that select the language
    /// before path suffixes do.
    pub fn register(
        name: LanguageName,
        settings: LanguageSettings,
        file_types: &[&str],
        semantic_token_rules: Option<SemanticTokenRules>,
        cx: &mut App,
    ) {
        cx.default_global::<Self>().insert(name, settings, file_types, semantic_token_rules);
    }

    fn insert(
        &mut self,
        name: LanguageName,
        settings: LanguageSettings,
        file_types: &[&str],
        semantic_token_rules: Option<SemanticTokenRules>,
    ) {
        if let Some(rules) = semantic_token_rules {
            self.semantic_token_rules.insert(name.clone(), rules);
        }
        if !file_types.is_empty() {
            let mut builder = GlobSetBuilder::new();
            for pattern in file_types {
                builder.add(Glob::new(pattern).expect("valid built-in file type glob"));
            }
            let globs = builder.build().expect("valid built-in file type globs");
            self.file_types.insert(name.0.as_ref().into(), (globs, file_types.iter().map(|s| s.to_string()).collect()));
        }
        self.languages.insert(name, Arc::new(settings));
    }

    pub fn semantic_token_rules(&self, language_name: &LanguageName) -> Option<&SemanticTokenRules> {
        self.semantic_token_rules.get(language_name)
    }

    /// Returns the [`LanguageSettings`] for the language with the specified name.
    pub fn language(&self, language_name: Option<&LanguageName>) -> Arc<LanguageSettings> {
        let settings = language_name.and_then(|name| self.languages.get(name)).unwrap_or(&self.fallback);
        Arc::clone(settings)
    }
}

impl LanguageSettings {
    /// Settings for buffers without a language or with an unregistered one.
    pub fn fallback() -> Self {
        Self {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_falls_back_for_missing_or_unregistered_language() {
        let mut all = AllLanguageSettings::default();
        let rust = LanguageSettings { tab_size: NonZeroU32::new(2).unwrap(), ..LanguageSettings::fallback() };
        all.insert(LanguageName::new_static("Rust"), rust.clone(), &[], None);

        assert_eq!(*all.language(Some(&LanguageName::new_static("Rust"))), rust);
        assert_eq!(*all.language(Some(&LanguageName::new_static("Go"))), LanguageSettings::fallback());
        assert_eq!(*all.language(None), LanguageSettings::fallback());
    }

    #[test]
    fn test_insert_records_file_types_only_when_given() {
        let mut all = AllLanguageSettings::default();
        all.insert(LanguageName::new_static("JSONC"), LanguageSettings::fallback(), &["tsconfig*.json"], None);
        all.insert(LanguageName::new_static("JSON"), LanguageSettings::fallback(), &[], None);

        let (globs, patterns) = &all.file_types["JSONC"];
        assert!(globs.is_match("tsconfig.base.json"));
        assert!(!globs.is_match("package.json"));
        assert_eq!(patterns, &vec!["tsconfig*.json".to_string()]);
        assert!(!all.file_types.contains_key("JSON"));
    }

    #[test]
    fn test_insert_records_semantic_token_rules_only_when_given() {
        let mut all = AllLanguageSettings::default();
        let rust = LanguageName::new_static("Rust");
        let go = LanguageName::new_static("Go");
        all.insert(rust.clone(), LanguageSettings::fallback(), &[], Some(SemanticTokenRules::default()));
        all.insert(go.clone(), LanguageSettings::fallback(), &[], None);

        assert_eq!(all.semantic_token_rules(&rust), Some(&SemanticTokenRules::default()));
        assert_eq!(all.semantic_token_rules(&go), None);
    }

    #[test]
    fn test_indent_guide_visible_line_width() {
        assert_eq!(INDENT_GUIDES.visible_line_width(false), Some(1));
        let disabled = IndentGuideSettings { coloring: IndentGuideColoring::Disabled, ..INDENT_GUIDES };
        assert_eq!(disabled.visible_line_width(true), None);
    }
}
