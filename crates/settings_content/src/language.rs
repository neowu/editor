use std::slice;

use serde::Deserialize;
use serde::Serialize;

/// The state of the modifier keys at some point in time
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModifiersContent {
    /// The control key
    #[serde(default)]
    pub control: bool,
    /// The alt key
    /// Sometimes also known as the 'meta' key
    #[serde(default)]
    pub alt: bool,
    /// The shift key
    #[serde(default)]
    pub shift: bool,
    /// The command key, on macos
    /// the windows key, on windows
    /// the super key, on linux
    #[serde(default)]
    pub platform: bool,
    /// The function key
    #[serde(default)]
    pub function: bool,
}

/// Controls the soft-wrapping behavior in the editor.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum AutoIndentMode {
    /// Adjusts indentation based on syntax context when typing.
    /// Uses tree-sitter to analyze code structure and indent accordingly.
    SyntaxAware,
    /// Preserve the indentation of the current line when creating new lines,
    /// but don't adjust based on syntax context.
    PreserveIndent,
    /// No automatic indentation. New lines start at column 0.
    None,
}

/// Controls the soft-wrapping behavior in the editor.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum SoftWrap {
    /// Prefer a single line generally, unless an overly long line is encountered.
    None,
    /// Deprecated: use None instead. Left to avoid breaking existing users' configs.
    /// Prefer a single line generally, unless an overly long line is encountered.
    PreferLine,
    /// Soft wrap lines that exceed the editor width.
    EditorWidth,
    /// Soft wrap line at the preferred line length or the editor width (whichever is smaller).
    #[serde(alias = "preferred_line_length")]
    Bounded,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    strum::EnumString,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum SoftWrapIndent {
    /// Continuation lines start at column 0.
    None,
    /// Continuation lines match the original line's indentation.
    #[default]
    Same,
    /// Continuation lines get 1 extra indent level beyond the original.
    ExtraOne,
    /// Continuation lines get 2 extra indent levels beyond the original.
    ExtraTwo,
}

/// Controls how whitespace should be displayedin the editor.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum ShowWhitespaceSetting {
    /// Draw whitespace only for the selected text.
    Selection,
    /// Do not draw any tabs or spaces.
    None,
    /// Draw all invisible symbols.
    All,
    /// Draw whitespaces at boundaries only.
    ///
    /// For a whitespace to be on a boundary, any of the following conditions need to be met:
    /// - It is a tab
    /// - It is adjacent to an edge (start or end)
    /// - It is adjacent to a whitespace (left or right)
    Boundary,
    /// Draw whitespaces only after non-whitespace characters.
    Trailing,
}

/// The behavior of `editor::Rewrap`.
#[derive(Debug, PartialEq, Clone, Copy, Default, Serialize, Deserialize, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum RewrapBehavior {
    /// Only rewrap within comments.
    #[default]
    InComments,
    /// Only rewrap within the current selection(s).
    InSelections,
    /// Allow rewrapping anywhere.
    Anywhere,
}

/// The kind of an inlay hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlayHintKind {
    /// An inlay hint for a type.
    Type,
    /// An inlay hint for a parameter.
    Parameter,
}

impl InlayHintKind {
    /// Returns the [`InlayHintKind`]fromthe given name.
    ///
    /// Returns `None` if `name` does not match any of the expected
    /// string representations.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "type" => Some(InlayHintKind::Type),
            "parameter" => Some(InlayHintKind::Parameter),
            _ => None,
        }
    }

    /// Returns the name of this [`InlayHintKind`].
    pub const fn name(&self) -> &'static str {
        match self {
            InlayHintKind::Type => "type",
            InlayHintKind::Parameter => "parameter",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum LspInsertMode {
    /// Replaces text before the cursor, using the `insert` range described in the LSP specification.
    Insert,
    /// Replaces text before and after the cursor, using the `replace` range described in the LSP specification.
    Replace,
    /// Behaves like `"replace"` if the text that would be replaced is a subsequence of the completion text,
    /// and like `"insert"` otherwise.
    ReplaceSubsequence,
    /// Behaves like `"replace"` if the text after the cursor is a suffix of the completion, and like
    /// `"insert"` otherwise.
    ReplaceSuffix,
}

/// Controls how document's words are completed.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum WordsCompletionMode {
    /// Always fetch document's words for completions along with LSP completions.
    Enabled,
    /// Only if LSP response errors or times out,
    /// use document's words to show completions.
    Fallback,
    /// Never fetch or complete document's words for completions.
    /// (Word-based completions can still be queried via a separate action)
    Disabled,
}

/// TODO: this should just be a bool
/// Controls the behavior of formatting files when they are saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum FormatOnSave {
    /// Files should be formatted on save.
    On,
    /// Files should not be formatted on save.
    Off,
    /// Only lines with unstaged changes are formatted on save.
    /// Requires source control and LSP range formatting support.
    /// If no git diff is available or if the LSP doesn't support
    /// range formatting, formatting is skipped.
    Modifications,
    /// Only lines with unstaged changes are formatted on save.
    /// If no git diff is available (e.g., when source control is
    /// unavailable) or if the LSP doesn't support range formatting,
    /// falls back to formatting the whole file.
    ModificationsIfAvailable,
}

/// Controls how line endings are normalized when a buffer is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum LineEndingSetting {
    /// Preserve the existing line endings of the file. New files use the
    /// platform default line ending.
    #[strum(serialize = "Detect")]
    Detect,
    /// Use LF for new files and files with no existing line-ending
    /// convention, while preserving existing LF or CRLF files.
    #[strum(serialize = "Prefer LF")]
    PreferLf,
    /// Use CRLF for new files and files with no existing line-ending
    /// convention, while preserving existing LF or CRLF files.
    #[strum(serialize = "Prefer CRLF")]
    PreferCrlf,
    /// Normalize line endings to LF (`\n`) during format and save.
    #[strum(serialize = "Enforce LF")]
    EnforceLf,
    /// Normalize line endings to CRLF (`\r\n`) during format and save.
    #[strum(serialize = "Enforce CRLF")]
    EnforceCrlf,
}

/// Controls which formatters should be used when formatting code.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FormatterList {
    Single(Formatter),
    Vec(Vec<Formatter>),
}

impl Default for FormatterList {
    fn default() -> Self {
        Self::Single(Formatter::default())
    }
}

impl AsRef<[Formatter]> for FormatterList {
    fn as_ref(&self) -> &[Formatter] {
        match &self {
            Self::Single(single) => slice::from_ref(single),
            Self::Vec(v) => v,
        }
    }
}

/// Controls which formatter should be used when formatting code. If there are multiple formatters, they are executed in the order of declaration.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Formatter {
    /// Format files using Biome for languages it supports,
    /// or falling back to formatting via language server.
    #[default]
    Auto,
    /// Do not format code.
    None,
    /// Format code using an external command.
    External {
        /// The external program to run.
        command: String,
        /// The arguments to pass to the program.
        arguments: Option<Vec<String>>,
    },
    /// Files should be formatted using a code action executed by language servers.
    CodeAction(String),
    /// Format code using a language server.
    #[serde(untagged)]
    LanguageServer(LanguageServerFormatterSpecifier),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    rename_all = "snake_case",
    // allow specifying language servers as "language_server" or {"language_server": {"name": ...}}
    from = "LanguageServerVariantContent",
    into = "LanguageServerVariantContent"
)]
pub enum LanguageServerFormatterSpecifier {
    Specific {
        name: String,
    },
    #[default]
    Current,
}

impl From<LanguageServerVariantContent> for LanguageServerFormatterSpecifier {
    fn from(value: LanguageServerVariantContent) -> Self {
        match value {
            LanguageServerVariantContent::Specific {
                language_server: LanguageServerSpecifierContent { name: Some(name) },
            } => Self::Specific { name },
            _ => Self::Current,
        }
    }
}

impl From<LanguageServerFormatterSpecifier> for LanguageServerVariantContent {
    fn from(value: LanguageServerFormatterSpecifier) -> Self {
        match value {
            LanguageServerFormatterSpecifier::Specific { name } => {
                Self::Specific { language_server: LanguageServerSpecifierContent { name: Some(name) } }
            }
            LanguageServerFormatterSpecifier::Current => Self::Current(CurrentLanguageServerContent::LanguageServer),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", untagged)]
enum LanguageServerVariantContent {
    /// Format code using a specific language server.
    Specific { language_server: LanguageServerSpecifierContent },
    /// Format code using the current language server.
    Current(CurrentLanguageServerContent),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CurrentLanguageServerContent {
    #[default]
    LanguageServer,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
struct LanguageServerSpecifierContent {
    /// The name of the language server to format with
    name: Option<String>,
}

/// Determines how indent guides are colored.
#[derive(
    Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum IndentGuideColoring {
    /// Do not render any lines for indent guides.
    Disabled,
    /// Use the same color for all indentation levels.
    #[default]
    Fixed,
    /// Use a different color for each indentation level.
    IndentAware,
}

/// Determines how indent guide backgrounds are colored.
#[derive(
    Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum IndentGuideBackgroundColoring {
    /// Do not render any background for indent guides.
    #[default]
    Disabled,
    /// Use a different color for each indentation level.
    IndentAware,
}
