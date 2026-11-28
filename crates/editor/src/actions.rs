//! This module contains all actions supported by [`Editor`].
use gpui::Action;
use gpui::actions;
use project::project_settings::GoToDiagnosticSeverityFilter;

use super::Debug;
use super::DisplayRow;
use super::Editor;
use super::Entity;

/// Selects the next occurrence of the current selection.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct SelectNext {
    pub replace_newest: bool,
}

/// Selects the previous occurrence of the current selection.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct SelectPrevious {
    pub replace_newest: bool,
}

/// Moves the cursor to the beginning of the current line.
#[derive(PartialEq, Eq, Clone, Action)]
#[action(namespace = editor)]
pub struct MoveToBeginningOfLine {
    pub stop_at_soft_wraps: bool,
    pub stop_at_indent: bool,
}

impl Default for MoveToBeginningOfLine {
    fn default() -> MoveToBeginningOfLine {
        MoveToBeginningOfLine { stop_at_soft_wraps: true, stop_at_indent: false }
    }
}

/// Selects from the cursor to the beginning of the current line.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct SelectToBeginningOfLine {
    pub stop_at_soft_wraps: bool,
    pub stop_at_indent: bool,
}

/// Deletes from the cursor to the beginning of the current line.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct DeleteToBeginningOfLine {
    pub(crate) stop_at_indent: bool,
}

/// Moves the cursor up by one page.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct MovePageUp {
    pub center_cursor: bool,
}

/// Moves the cursor down by one page.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct MovePageDown {
    pub center_cursor: bool,
}

/// Moves the cursor to the end of the current line.
#[derive(PartialEq, Eq, Clone, Action)]
#[action(namespace = editor)]
pub struct MoveToEndOfLine {
    pub stop_at_soft_wraps: bool,
}

impl Default for MoveToEndOfLine {
    fn default() -> MoveToEndOfLine {
        MoveToEndOfLine { stop_at_soft_wraps: true }
    }
}

/// Selects from the cursor to the end of the current line.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct SelectToEndOfLine {
    pub stop_at_soft_wraps: bool,
}

/// Toggles the display of available code actions at the cursor position.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct ToggleCodeActions {
    // Source from which the action was deployed.
    pub(crate) deployed_from: Option<CodeActionSource>,
    // Run first available task if there is only one.
    pub(crate) quick_launch: bool,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub(crate) enum CodeActionSource {
    Indicator(DisplayRow),
    RunMenu(DisplayRow),
}

/// Confirms and accepts the currently selected completion suggestion.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct ConfirmCompletion {
    pub(crate) item_ix: Option<usize>,
}

/// Composes multiple completion suggestions into a single completion.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct ComposeCompletion {
    pub(crate) item_ix: Option<usize>,
}

/// Confirms and applies the currently selected code action.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct ConfirmCodeAction {
    pub(crate) item_ix: Option<usize>,
}

/// Toggles comment markers for the selected lines.
#[derive(PartialEq, Eq, Clone, Action)]
#[action(namespace = editor)]
pub struct ToggleComments {
    pub advance_downwards: bool,
    pub ignore_indent: bool,
    /// Whether to add comment markers to blank lines inside a multi-line
    /// selection. A line of only whitespace counts as blank. Defaults to true.
    pub comment_empty_lines: bool,
}

// `Default` is written out rather than derived because `comment_empty_lines`
// defaults to true.
impl Default for ToggleComments {
    fn default() -> Self {
        Self { advance_downwards: false, ignore_indent: false, comment_empty_lines: true }
    }
}

/// Toggles block comment markers for the selected text.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct ToggleBlockComments;

/// Moves the cursor up by a specified number of lines.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct MoveUpByLines {
    pub(super) lines: u32,
}

/// Moves the cursor down by a specified number of lines.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct MoveDownByLines {
    pub(super) lines: u32,
}

/// Extends selection up by a specified number of lines.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct SelectUpByLines {
    pub(super) lines: u32,
}

/// Extends selection down by a specified number of lines.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct SelectDownByLines {
    pub(super) lines: u32,
}

/// Expands all excerpts with selections.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct ExpandExcerpts {
    pub(super) lines: u32,
}

/// Expands excerpts above the current position.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct ExpandExcerptsUp {
    pub(super) lines: u32,
}

/// Expands excerpts below the current position.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct ExpandExcerptsDown {
    pub(super) lines: u32,
}

/// Handles text input in the editor.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor, no_build)]
pub(crate) struct HandleInput(pub(crate) String);

/// Deletes from the cursor to the end of the next word.
/// Stops before the end of the next word, if whitespace sequences of length >= 2 are encountered.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct DeleteToNextWordEnd {
    pub ignore_newlines: bool,
    // Whether to stop before the end of the next word, if language-defined bracket is encountered.
    pub ignore_brackets: bool,
}

/// Deletes from the cursor to the start of the previous word.
/// Stops before the start of the previous word, if whitespace sequences of length >= 2 are encountered.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct DeleteToPreviousWordStart {
    pub ignore_newlines: bool,
    // Whether to stop before the start of the previous word, if language-defined bracket is encountered.
    pub ignore_brackets: bool,
}

/// Deletes from the cursor to the end of the next subword.
/// Stops before the end of the next subword, if whitespace sequences of length >= 2 are encountered.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct DeleteToNextSubwordEnd {
    pub(crate) ignore_newlines: bool,
    // Whether to stop before the start of the previous word, if language-defined bracket is encountered.
    pub(crate) ignore_brackets: bool,
}

/// Deletes from the cursor to the start of the previous subword.
/// Stops before the start of the previous subword, if whitespace sequences of length >= 2 are encountered.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct DeleteToPreviousSubwordStart {
    pub(crate) ignore_newlines: bool,
    // Whether to stop before the start of the previous word, if language-defined bracket is encountered.
    pub(crate) ignore_brackets: bool,
}

/// Cuts from cursor to end of line.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct CutToEndOfLine {
    pub(crate) stop_at_newlines: bool,
}

/// Folds all code blocks at the specified indentation level.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor, no_build)]
pub(crate) struct FoldAtLevel(pub(crate) u32);

#[derive(Clone, PartialEq, Eq, Action)]
#[action(no_build, no_register)]
pub struct DiffClipboardWithSelectionData {
    pub clipboard_text: String,
    pub editor: Entity<Editor>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub(crate) enum UuidVersion {
    #[default]
    V4,
    V7,
}

/// Splits selection into individual lines.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub(crate) struct SplitSelectionIntoLines {
    /// Keep the text selected after splitting instead of collapsing to cursors.
    pub(crate) keep_selections: bool,
}

/// Expands the diagnostic under the cursor, if any, in case diagnostics are not
/// yet active. Otherwise, goes to the next diagnostic in the file.
#[derive(PartialEq, Eq, Clone, Default, Debug, Action)]
#[action(namespace = editor)]
pub struct GoToDiagnostic {
    pub severity: GoToDiagnosticSeverityFilter,
}

/// Expands the diagnostic under the cursor, if any, in case diagnostics are not
/// yet active. Otherwise, goes to the previous diagnostic in the file.
#[derive(PartialEq, Eq, Clone, Default, Debug, Action)]
#[action(namespace = editor)]
pub struct GoToPreviousDiagnostic {
    pub severity: GoToDiagnosticSeverityFilter,
}

/// Adds a cursor above the current selection.
#[derive(PartialEq, Eq, Clone, Debug, Action)]
#[action(namespace = editor)]
pub struct AddSelectionAbove {
    pub skip_soft_wrap: bool,
}

impl Default for AddSelectionAbove {
    fn default() -> AddSelectionAbove {
        AddSelectionAbove { skip_soft_wrap: true }
    }
}

/// Adds a cursor below the current selection.
#[derive(PartialEq, Eq, Clone, Debug, Action)]
#[action(namespace = editor)]
pub struct AddSelectionBelow {
    pub skip_soft_wrap: bool,
}

impl Default for AddSelectionBelow {
    fn default() -> AddSelectionBelow {
        AddSelectionBelow { skip_soft_wrap: true }
    }
}

/// Inserts a snippet at the cursor.
#[derive(PartialEq, Eq, Clone, Default, Debug, Action)]
#[action(namespace = editor)]
pub(crate) struct InsertSnippet {
    /// Language name if using a named snippet, or `None` for a global snippet.
    ///
    /// This is typically lowercase and matches the filename containing the snippet, without the `.json` extension.
    pub(crate) language: Option<String>,
    /// Name if using a named snippet.
    pub(crate) name: Option<String>,

    /// Snippet body, if not using a named snippet.
    // todo(andrew): use `ListOrDirect` or similar for multiline snippet body
    pub(crate) snippet: Option<String>,
}

actions!(
    go_to_line,
    [
        /// Toggles the go to line dialog.
        #[action(name = "Toggle")]
        pub ToggleGoToLine
    ]
);

actions!(
    markdown,
    [
        /// Toggles a block quote (`> `) prefix on the selected lines (or the
        /// current line) while in Markdown files.
        pub(crate) ToggleBlockQuote,
    ]
);

// Handled by rust-analyzer, so only available in Rust buffers.
actions!(
    rust,
    [
        /// Expands macros recursively at cursor position.
        pub(crate) ExpandMacroRecursively,
        /// Goes to the tests that exercise the item at cursor position.
        pub(crate) GoToRelatedTests,
        /// Opens the documentation of the symbol at cursor position.
        pub OpenDocs,
        /// Shows the HIR of the function at cursor position.
        pub(crate) ViewHir,
        /// Shows the size, alignment and field offsets of the type at cursor position.
        pub(crate) ViewMemoryLayout,
        /// Shows the MIR of the function at cursor position.
        pub(crate) ViewMir,
    ]
);

actions!(
    editor,
    [
        /// Amends the editor text with hint's edit (if sent).
        pub(crate) AcceptInlayHint,
        /// Deletes the character before the cursor.
        pub Backspace,
        /// Cancels the current operation.
        pub Cancel,
        /// Cancels pending language server work.
        pub(crate) CancelLanguageServerWork,
        /// Confirms the rename operation.
        pub ConfirmRename,
        /// Confirms completion by inserting at cursor.
        pub(crate) ConfirmCompletionInsert,
        /// Confirms completion by replacing existing text.
        pub ConfirmCompletionReplace,
        /// Navigates to the first item in the context menu.
        pub ContextMenuFirst,
        /// Navigates to the last item in the context menu.
        pub ContextMenuLast,
        /// Navigates to the next item in the context menu.
        pub ContextMenuNext,
        /// Navigates to the previous item in the context menu.
        pub ContextMenuPrevious,
        /// Converts indentation from tabs to spaces.
        pub(crate) ConvertIndentationToSpaces,
        /// Converts indentation from spaces to tabs.
        pub(crate) ConvertIndentationToTabs,
        /// Converts selected text to kebab-case.
        pub(crate) ConvertToKebabCase,
        /// Converts selected text to lowerCamelCase.
        pub(crate) ConvertToLowerCamelCase,
        /// Converts selected text to lowercase.
        pub(crate) ConvertToLowerCase,
        /// Toggles the case of selected text.
        pub(crate) ConvertToOppositeCase,
        /// Converts selected text to sentence case.
        pub(crate) ConvertToSentenceCase,
        /// Converts selected text to snake_case.
        pub(crate) ConvertToSnakeCase,
        /// Converts selected text to Title Case.
        pub(crate) ConvertToTitleCase,
        /// Converts selected text to UpperCamelCase.
        pub(crate) ConvertToUpperCamelCase,
        /// Converts selected text to UPPERCASE.
        pub(crate) ConvertToUpperCase,
        /// Applies ROT13 cipher to selected text.
        pub(crate) ConvertToRot13,
        /// Applies ROT47 cipher to selected text.
        pub(crate) ConvertToRot47,
        /// Base64-encodes the selected text or word under cursor.
        pub(crate) ConvertToBase64,
        /// Base64-decodes the selected text or word under cursor.
        pub(crate) ConvertFromBase64,
        /// Copies selected text to the clipboard.
        pub Copy,
        /// Copies the current file location to the clipboard.
        pub(crate) CopyFileLocation,
        /// Copies the highlighted text as JSON.
        pub(crate) CopyHighlightJson,
        /// Copies the current file name to the clipboard.
        pub(crate) CopyFileName,
        /// Copies the file name without extension to the clipboard.
        pub(crate) CopyFileNameWithoutExtension,
        /// Cuts selected text to the clipboard.
        pub Cut,
        /// Deletes the character after the cursor.
        pub Delete,
        /// Deletes the current line.
        pub DeleteLine,
        /// Deletes from cursor to end of line.
        pub DeleteToEndOfLine,
        /// Diffs the text stored in the clipboard against the current selection.
        pub(crate) DiffClipboardWithSelection,
        /// Displays names of all active cursors.
        /// Duplicates the current line below.
        pub DuplicateLineDown,
        /// Duplicates the current line above.
        pub DuplicateLineUp,
        /// Duplicates the current selection.
        pub(crate) DuplicateSelection,
        /// Expands all diff hunks in the editor.
        pub ExpandAllDiffHunks,
        /// Collapses all diff hunks in the editor.
        pub(crate) CollapseAllDiffHunks,
        /// Toggles all diff hunks in the editor. Collapses all hunks if any are
        /// currently expanded, otherwise expands all hunks.
        pub(crate) ToggleAllDiffHunks,
        /// Finds the next match in the search.
        pub(crate) FindNextMatch,
        /// Finds the previous match in the search.
        pub(crate) FindPreviousMatch,
        /// Folds the current code block.
        pub Fold,
        /// Folds all foldable regions in the editor.
        pub FoldAll,
        /// Folds all code blocks at indentation level 1.
        #[action(name = "FoldAtLevel_1")]
        pub(crate) FoldAtLevel1,
        /// Folds all code blocks at indentation level 2.
        #[action(name = "FoldAtLevel_2")]
        pub(crate) FoldAtLevel2,
        /// Folds all code blocks at indentation level 3.
        #[action(name = "FoldAtLevel_3")]
        pub(crate) FoldAtLevel3,
        /// Folds all code blocks at indentation level 4.
        #[action(name = "FoldAtLevel_4")]
        pub(crate) FoldAtLevel4,
        /// Folds all code blocks at indentation level 5.
        #[action(name = "FoldAtLevel_5")]
        pub(crate) FoldAtLevel5,
        /// Folds all code blocks at indentation level 6.
        #[action(name = "FoldAtLevel_6")]
        pub(crate) FoldAtLevel6,
        /// Folds all code blocks at indentation level 7.
        #[action(name = "FoldAtLevel_7")]
        pub(crate) FoldAtLevel7,
        /// Folds all code blocks at indentation level 8.
        #[action(name = "FoldAtLevel_8")]
        pub(crate) FoldAtLevel8,
        /// Folds all code blocks at indentation level 9.
        #[action(name = "FoldAtLevel_9")]
        pub(crate) FoldAtLevel9,
        /// Folds all function bodies in the editor.
        pub(crate) FoldFunctionBodies,
        /// Folds the current code block and all its children.
        pub(crate) FoldRecursive,
        /// Folds the selected ranges.
        pub(crate) FoldSelectedRanges,
        /// Toggles focus back to the last active buffer.
        pub(crate) ToggleFocus,
        /// Toggles folding at the current position.
        pub(crate) ToggleFold,
        /// Toggles recursive folding at the current position.
        pub(crate) ToggleFoldRecursive,
        /// Toggles all folds in a buffer or all excerpts in multibuffer.
        pub(crate) ToggleFoldAll,
        /// Hides or shows the references in test code in find-references results.
        pub(crate) ToggleTestReferences,
        /// Formats the entire document.
        pub Format,
        /// Formats only the selected text.
        ///
        /// This action is only available when the active formatter can format ranges.
        /// When using a language server, this sends an LSP range formatting request for each
        /// selection, and is hidden when the selected buffer's configured language server does
        /// not advertise range-formatting support. When using Biome, the whole buffer is
        /// formatted and resulting edits outside the selected ranges are discarded. External command formatters do not
        /// support range formatting and are skipped.
        pub(crate) FormatSelections,
        /// Goes to the next diff hunk.
        pub GoToHunk,
        /// Goes to the previous diff hunk.
        pub GoToPreviousHunk,
        /// Goes to the next change in the file.
        pub GoToNextChange,
        /// Goes to the previous change in the file.
        pub GoToPreviousChange,
        /// Goes to the next symbol.
        pub(crate) GoToNextSymbol,
        /// Goes to the previous symbol.
        pub(crate) GoToPreviousSymbol,
        /// Goes to the next reference to the symbol under the cursor.
        pub(crate) GoToNextReference,
        /// Goes to the previous reference to the symbol under the cursor.
        pub(crate) GoToPreviousReference,
        /// Goes to the next document highlight.
        pub(crate) GoToNextDocumentHighlight,
        /// Goes to the previous document highlight.
        pub(crate) GoToPreviousDocumentHighlight,
        /// Scrolls down by half a page.
        pub(crate) HalfPageDown,
        /// Scrolls up by half a page.
        pub(crate) HalfPageUp,
        /// Shows hover information for the symbol at cursor.
        pub(crate) Hover,
        /// Increases indentation of selected lines.
        pub(crate) Indent,
        /// Inserts a UUID v4 at cursor position.
        pub(crate) InsertUuidV4,
        /// Inserts a UUID v7 at cursor position.
        pub(crate) InsertUuidV7,
        /// Joins the current line with the next line.
        pub JoinLines,
        /// Cuts to kill ring (Emacs-style).
        pub(crate) KillRingCut,
        /// Yanks from kill ring (Emacs-style).
        pub(crate) KillRingYank,
        /// Moves cursor down one line.
        pub LineDown,
        /// Moves cursor up one line.
        pub LineUp,
        /// Moves cursor left.
        pub MoveLeft,
        /// Moves the current line down.
        pub MoveLineDown,
        /// Moves the current line up.
        pub MoveLineUp,
        /// Moves cursor right.
        pub MoveRight,
        /// Moves cursor to the beginning of the document.
        pub MoveToBeginning,
        /// Moves cursor to the enclosing bracket.
        pub MoveToEnclosingBracket,
        /// Selects the content within the nearest enclosing delimiters
        /// (brackets, braces, parentheses, or quotes), excluding the
        /// delimiters.
        /// Repeating the action expands the selection to the next enclosing
        /// pair.
        pub(crate) SelectInsideDelimiters,
        /// Selects the nearest enclosing delimiters (brackets, braces,
        /// parentheses, or quotes) together with the content between them.
        /// Repeating the action expands the selection to the next enclosing
        /// pair.
        pub(crate) SelectAroundDelimiters,
        /// Moves cursor to the end of the document.
        pub MoveToEnd,
        /// Moves cursor to the end of the paragraph.
        pub MoveToEndOfParagraph,
        /// Moves cursor to the start of the next comment paragraph.
        pub(crate) MoveToNextCommentParagraph,
        /// Moves cursor to the end of the next subword.
        pub MoveToNextSubwordEnd,
        /// Moves cursor to the end of the next word.
        pub MoveToNextWordEnd,
        /// Moves cursor to the start of the previous comment paragraph.
        pub(crate) MoveToPreviousCommentParagraph,
        /// Moves cursor to the start of the previous subword.
        pub MoveToPreviousSubwordStart,
        /// Moves cursor to the start of the previous word.
        pub MoveToPreviousWordStart,
        /// Moves cursor to the start of the paragraph.
        pub MoveToStartOfParagraph,
        /// Moves cursor to the start of the current excerpt.
        pub MoveToStartOfExcerpt,
        /// Moves cursor to the start of the next excerpt.
        pub MoveToStartOfNextExcerpt,
        /// Moves cursor to the end of the current excerpt.
        pub(crate) MoveToEndOfExcerpt,
        /// Moves cursor to the end of the previous excerpt.
        pub(crate) MoveToEndOfPreviousExcerpt,
        /// Moves cursor to the start of the next larger syntax node.
        pub(crate) MoveToStartOfLargerSyntaxNode,
        /// Moves cursor to the end of the next larger syntax node.
        pub(crate) MoveToEndOfLargerSyntaxNode,
        /// Inserts a new line and moves cursor to it.
        pub Newline,
        /// Inserts a new line above the current line.
        pub NewlineAbove,
        /// Inserts a new line below the current line.
        pub NewlineBelow,
        /// Scrolls to the next screen.
        pub(crate) NextScreen,
        /// Goes to the next snippet tabstop if one exists.
        pub NextSnippetTabstop,
        /// Opens the context menu at cursor position.
        pub(crate) OpenContextMenu,
        /// Opens the file whose name is selected in the editor.
        pub(crate) OpenSelectedFilename,
        /// Opens all selections in a multibuffer.
        pub OpenSelectionsInMultibuffer,
        /// Opens the URL at cursor position.
        pub(crate) OpenUrl,
        /// Organizes import statements.
        pub OrganizeImports,
        /// Decreases indentation of selected lines.
        pub(crate) Outdent,
        /// Automatically adjusts indentation based on context.
        pub(crate) AutoIndent,
        /// Scrolls down by one page.
        pub PageDown,
        /// Scrolls up by one page.
        pub PageUp,
        /// Pastes from clipboard.
        pub Paste,
        /// Goes to the previous snippet tabstop if one exists.
        pub PreviousSnippetTabstop,
        /// Redoes the last undone edit.
        pub Redo,
        /// Redoes the last selection change.
        pub(crate) RedoSelection,
        /// Renames the symbol at cursor.
        pub Rename,
        /// Restarts the language server for the current file.
        pub(crate) RestartLanguageServer,
        /// Reloads the file from disk.
        pub(crate) ReloadFile,
        /// Rewraps text to fit within the preferred line length.
        pub(crate) Rewrap,
        /// Scrolls the cursor to the bottom of the viewport.
        pub(crate) ScrollCursorBottom,
        /// Scrolls the cursor to the center of the viewport.
        pub ScrollCursorCenter,
        /// Cycles cursor position between center, top, and bottom.
        pub(crate) ScrollCursorCenterTopBottom,
        /// Scrolls the cursor to the top of the viewport.
        pub(crate) ScrollCursorTop,
        /// Selects all text in the editor.
        pub SelectAll,
        /// Selects all matches of the current selection.
        pub SelectAllMatches,
        /// Selects to the start of the current excerpt.
        pub SelectToStartOfExcerpt,
        /// Selects to the start of the next excerpt.
        pub SelectToStartOfNextExcerpt,
        /// Selects to the end of the current excerpt.
        pub(crate) SelectToEndOfExcerpt,
        /// Selects to the end of the previous excerpt.
        pub(crate) SelectToEndOfPreviousExcerpt,
        /// Extends selection down.
        pub SelectDown,
        /// Selects the enclosing symbol.
        pub SelectEnclosingSymbol,
        /// Selects to the start of the next larger syntax node.
        pub(crate) SelectToStartOfLargerSyntaxNode,
        /// Selects to the end of the next larger syntax node.
        pub(crate) SelectToEndOfLargerSyntaxNode,
        /// Selects the next larger syntax node.
        pub SelectLargerSyntaxNode,
        /// Selects the next syntax node sibling.
        pub SelectNextSyntaxNode,
        /// Selects the previous syntax node sibling.
        pub SelectPreviousSyntaxNode,
        /// Extends selection left.
        pub SelectLeft,
        /// Selects the current line.
        pub SelectLine,
        /// Extends selection down by one page.
        pub SelectPageDown,
        /// Extends selection up by one page.
        pub SelectPageUp,
        /// Extends selection right.
        pub SelectRight,
        /// Selects the next smaller syntax node.
        pub SelectSmallerSyntaxNode,
        /// Selects to the beginning of the document.
        pub SelectToBeginning,
        /// Selects to the end of the document.
        pub SelectToEnd,
        /// Selects to the end of the paragraph.
        pub SelectToEndOfParagraph,
        /// Selects to the end of the next subword.
        pub SelectToNextSubwordEnd,
        /// Selects to the end of the next word.
        pub SelectToNextWordEnd,
        /// Selects to the start of the previous subword.
        pub SelectToPreviousSubwordStart,
        /// Selects to the start of the previous word.
        pub SelectToPreviousWordStart,
        /// Selects to the start of the paragraph.
        pub SelectToStartOfParagraph,
        /// Extends selection up.
        pub SelectUp,
        /// Shows code completion suggestions at the cursor position.
        pub ShowCompletions,
        /// Shows the system character palette.
        pub ShowCharacterPalette,
        /// Shows word completions.
        pub ShowWordCompletions,
        /// Sorts selected lines by length.
        pub(crate) SortLinesByLength,
        /// Sorts selected lines case-insensitively.
        pub(crate) SortLinesCaseInsensitive,
        /// Sorts selected lines case-sensitively.
        pub(crate) SortLinesCaseSensitive,
        /// Stops the language server for the current file.
        pub(crate) StopLanguageServer,
        /// Inserts a tab character or indents.
        pub Tab,
        /// Removes a tab character or outdents.
        pub Backtab,
        /// Toggles the case of selected text.
        pub ToggleCase,
        /// Opens the git commit for the blame at cursor.
        pub(crate) OpenGitBlameCommit,
        /// Opens a blame of the file at the revision of the blame entry at cursor.
        pub(crate) BlameRevision,
        /// Opens a blame of the file at the revision preceding the blame entry at cursor.
        pub(crate) BlamePreviousRevision,
        /// Toggles all diagnostics in this editor: underlines, inline messages, and navigation.
        pub(crate) ToggleDiagnostics,
        /// Toggles indent guides display.
        pub(crate) ToggleIndentGuides,
        /// Toggles the minimap display.
        pub(crate) ToggleMinimap,
        /// Swaps the start and end of the current selection.
        pub(crate) SwapSelectionEnds,
        /// Sets a mark at the current position.
        pub(crate) SetMark,
        /// Toggles diff display for selected hunks.
        pub ToggleSelectedDiffHunks,
        /// Toggles soft wrap mode.
        pub(crate) ToggleSoftWrap,
        /// Transposes characters around cursor.
        pub(crate) Transpose,
        /// Undoes the last edit.
        pub Undo,
        /// Undoes the last selection change.
        pub UndoSelection,
        /// Unfolds all folded regions.
        pub UnfoldAll,
        /// Unfolds lines at cursor.
        pub UnfoldLines,
        /// Unfolds recursively at cursor.
        pub(crate) UnfoldRecursive,
        /// Removes duplicate lines (case-insensitive).
        pub(crate) UniqueLinesCaseInsensitive,
        /// Removes duplicate lines (case-sensitive).
        pub(crate) UniqueLinesCaseSensitive,
        /// Removes the surrounding syntax node (for example brackets, or closures)
        /// from the current selections.
        pub(crate) UnwrapSyntaxNode,
        /// Wraps selections in tag specified by language.
        pub(crate) WrapSelectionsInTag,
        /// Aligns selections from different rows into the same column
        pub(crate) AlignSelections,
        /// Saves the current location to navigation history.
        pub(crate) SaveLocation,
    ]
);

/// Where to show LSP results that can contain multiple locations.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum OpenResultsIn {
    #[default]
    MultiBuffer,
    Picker,
}

/// Goes to the definition of the symbol at cursor.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct GoToDefinition {
    /// Where to show the definitions. Defaults to a multibuffer
    /// when omitted. A single result is always opened directly.
    pub open_results_in: Option<OpenResultsIn>,
}

/// Goes to the implementation of the symbol at cursor.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct GoToImplementation {
    /// Where to show the implementations. Defaults to a multibuffer
    /// when omitted. A single result is always opened directly.
    pub open_results_in: Option<OpenResultsIn>,
}

/// Goes to the type definition of the symbol at cursor.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = editor)]
pub struct GoToTypeDefinition {
    /// Where to show the definitions. Defaults to a multibuffer
    /// when omitted. A single result is always opened directly.
    pub open_results_in: Option<OpenResultsIn>,
}

/// Finds all references to the symbol at cursor.
#[derive(PartialEq, Eq, Clone, Action)]
#[action(namespace = editor)]
pub struct FindAllReferences {
    pub(crate) always_open_multibuffer: bool,
    /// Where to show the references. Defaults to a multibuffer
    /// when omitted. A single result is always opened directly.
    pub open_results_in: Option<OpenResultsIn>,
}

impl Default for FindAllReferences {
    fn default() -> Self {
        Self { always_open_multibuffer: true, open_results_in: None }
    }
}
