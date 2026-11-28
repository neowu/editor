//! The complete macOS keymap. Edit these bindings and rebuild to change shortcuts.
use std::rc::Rc;

use gpui::Action;
use gpui::KeyBinding;
use gpui::KeyBindingContextPredicate;

/// Emits bindings straight into gpui `KeyBinding`s. A binding in a deeper context wins; at the same
/// depth, the later binding wins. Only the US layout is supported, so keys are never remapped.
struct Keymap {
    bindings: Vec<KeyBinding>,
    context: Option<Rc<KeyBindingContextPredicate>>,
}

impl Keymap {
    fn group(&mut self, context: Option<&str>) {
        self.context = context.map(|context| {
            KeyBindingContextPredicate::parse(context)
                .unwrap_or_else(|error| panic!("invalid built-in context {context}: {error}"))
                .into()
        });
    }

    fn bind(&mut self, keystrokes: &str, action: impl Action) {
        let binding = KeyBinding::load(keystrokes, Box::new(action), self.context.clone())
            .unwrap_or_else(|error| panic!("invalid built-in keystrokes {keystrokes}: {error}"));
        self.bindings.push(binding);
    }
}

#[expect(clippy::too_many_lines, reason = "the whole default keymap as one flat table")]
pub(super) fn keymap() -> Vec<KeyBinding> {
    let mut k = Keymap { bindings: Vec::with_capacity(512), context: None };
    k.group(None);
    k.bind("home", actions::menu::SelectFirst);
    k.bind("shift-pageup", actions::menu::SelectFirst);
    k.bind("pageup", actions::menu::SelectFirst);
    k.bind("cmd-up", actions::menu::SelectFirst);
    k.bind("end", actions::menu::SelectLast);
    k.bind("shift-pagedown", actions::menu::SelectLast);
    k.bind("pagedown", actions::menu::SelectLast);
    k.bind("cmd-down", actions::menu::SelectLast);
    k.bind("tab", actions::menu::SelectNext);
    k.bind("ctrl-n", actions::menu::SelectNext);
    k.bind("down", actions::menu::SelectNext);
    k.bind("shift-tab", actions::menu::SelectPrevious);
    k.bind("ctrl-p", actions::menu::SelectPrevious);
    k.bind("up", actions::menu::SelectPrevious);
    k.bind("enter", actions::menu::Confirm);
    k.bind("ctrl-enter", actions::menu::SecondaryConfirm);
    k.bind("cmd-enter", actions::menu::SecondaryConfirm);
    k.bind("cmd-escape", actions::menu::Cancel);
    k.bind("ctrl-escape", actions::menu::Cancel);
    k.bind("ctrl-c", actions::menu::Cancel);
    k.bind("escape", actions::menu::Cancel);
    k.bind("alt-shift-enter", actions::menu::Restart);
    k.bind("cmd-shift-w", workspace::CloseWindow);
    k.bind("shift-escape", workspace::ToggleZoom);
    k.bind("cmd-o", workspace::Open::default());
    k.bind("cmd-[", workspace::GoBack);
    k.bind("cmd-]", workspace::GoForward);
    k.bind("cmd-=", actions::IncreaseBufferFontSize);
    k.bind("cmd-+", actions::IncreaseBufferFontSize);
    k.bind("cmd--", actions::DecreaseBufferFontSize);
    k.bind("cmd-0", actions::ResetBufferFontSize);
    k.bind("cmd-q", actions::Quit);
    k.bind("cmd-h", crate::ed::Hide);
    k.bind("alt-cmd-h", crate::ed::HideOthers);
    k.bind("cmd-m", crate::ed::Minimize);
    k.bind("ctrl-cmd-l", language_tools::lsp_button::ToggleMenu);

    k.group(Some("menu"));
    k.bind("right", actions::menu::SelectChild);
    k.bind("left", actions::menu::SelectParent);

    k.group(Some("Editor"));
    k.bind("escape", editor::actions::Cancel);
    k.bind("shift-backspace", editor::actions::Backspace);
    k.bind("ctrl-h", editor::actions::Backspace);
    k.bind("backspace", editor::actions::Backspace);
    k.bind("ctrl-d", editor::actions::Delete);
    k.bind("delete", editor::actions::Delete);
    k.bind("tab", editor::actions::Tab);
    k.bind("shift-tab", editor::actions::Backtab);
    k.bind("cmd-backspace", editor::actions::DeleteToBeginningOfLine::default());
    k.bind("cmd-delete", editor::actions::DeleteToEndOfLine);
    k.bind(
        "alt-backspace",
        editor::actions::DeleteToPreviousWordStart { ignore_newlines: false, ignore_brackets: false },
    );
    k.bind("ctrl-w", editor::actions::DeleteToPreviousWordStart { ignore_newlines: false, ignore_brackets: false });
    k.bind("alt-delete", editor::actions::DeleteToNextWordEnd { ignore_newlines: false, ignore_brackets: false });
    k.bind("cmd-x", editor::actions::Cut);
    k.bind("cmd-c", editor::actions::Copy);
    k.bind("cmd-v", editor::actions::Paste);
    k.bind("cmd-z", editor::actions::Undo);
    k.bind("cmd-shift-z", editor::actions::Redo);
    k.bind("up", actions::editor::MoveUp);
    k.bind("ctrl-up", editor::actions::MoveToStartOfParagraph);
    k.bind("pageup", editor::actions::MovePageUp::default());
    k.bind("shift-pageup", editor::actions::SelectPageUp);
    k.bind("cmd-pageup", editor::actions::PageUp);
    k.bind("ctrl-pageup", editor::actions::LineUp);
    k.bind("down", actions::editor::MoveDown);
    k.bind("ctrl-down", editor::actions::MoveToEndOfParagraph);
    k.bind("pagedown", editor::actions::MovePageDown::default());
    k.bind("shift-pagedown", editor::actions::SelectPageDown);
    k.bind("cmd-pagedown", editor::actions::PageDown);
    k.bind("ctrl-pagedown", editor::actions::LineDown);
    k.bind("ctrl-p", actions::editor::MoveUp);
    k.bind("ctrl-n", actions::editor::MoveDown);
    k.bind("ctrl-b", editor::actions::MoveLeft);
    k.bind("left", editor::actions::MoveLeft);
    k.bind("ctrl-f", editor::actions::MoveRight);
    k.bind("right", editor::actions::MoveRight);
    k.bind("ctrl-l", editor::actions::ScrollCursorCenter);
    k.bind("alt-left", editor::actions::MoveToPreviousWordStart);
    k.bind("alt-right", editor::actions::MoveToNextWordEnd);
    k.bind("cmd-left", editor::actions::MoveToBeginningOfLine { stop_at_soft_wraps: true, stop_at_indent: true });
    k.bind("ctrl-a", editor::actions::MoveToBeginningOfLine { stop_at_soft_wraps: false, stop_at_indent: true });
    k.bind("home", editor::actions::MoveToBeginningOfLine { stop_at_soft_wraps: true, stop_at_indent: true });
    k.bind("cmd-right", editor::actions::MoveToEndOfLine { stop_at_soft_wraps: true });
    k.bind("ctrl-e", editor::actions::MoveToEndOfLine { stop_at_soft_wraps: false });
    k.bind("end", editor::actions::MoveToEndOfLine { stop_at_soft_wraps: true });
    k.bind("cmd-up", editor::actions::MoveToBeginning);
    k.bind("cmd-down", editor::actions::MoveToEnd);
    k.bind("cmd-home", editor::actions::MoveToBeginning);
    k.bind("cmd-end", editor::actions::MoveToEnd);
    k.bind("shift-up", editor::actions::SelectUp);
    k.bind("ctrl-shift-p", editor::actions::SelectUp);
    k.bind("shift-down", editor::actions::SelectDown);
    k.bind("ctrl-shift-n", editor::actions::SelectDown);
    k.bind("shift-left", editor::actions::SelectLeft);
    k.bind("ctrl-shift-b", editor::actions::SelectLeft);
    k.bind("shift-right", editor::actions::SelectRight);
    k.bind("ctrl-shift-f", editor::actions::SelectRight);
    k.bind("alt-shift-left", editor::actions::SelectToPreviousWordStart);
    k.bind("alt-shift-right", editor::actions::SelectToNextWordEnd);
    k.bind("ctrl-shift-up", editor::actions::SelectToStartOfParagraph);
    k.bind("ctrl-shift-down", editor::actions::SelectToEndOfParagraph);
    k.bind("cmd-shift-up", editor::actions::SelectToBeginning);
    k.bind("cmd-shift-down", editor::actions::SelectToEnd);
    k.bind("cmd-a", editor::actions::SelectAll);
    k.bind("cmd-l", editor::actions::SelectLine);
    k.bind("cmd-shift-i", editor::actions::Format);
    k.bind("alt-shift-o", editor::actions::OrganizeImports);
    k.bind(
        "cmd-shift-left",
        editor::actions::SelectToBeginningOfLine { stop_at_soft_wraps: true, stop_at_indent: true },
    );
    k.bind("shift-home", editor::actions::SelectToBeginningOfLine { stop_at_soft_wraps: true, stop_at_indent: true });
    k.bind("ctrl-shift-a", editor::actions::SelectToBeginningOfLine { stop_at_soft_wraps: true, stop_at_indent: true });
    k.bind("cmd-shift-right", editor::actions::SelectToEndOfLine { stop_at_soft_wraps: true });
    k.bind("shift-end", editor::actions::SelectToEndOfLine { stop_at_soft_wraps: true });
    k.bind("ctrl-shift-e", editor::actions::SelectToEndOfLine { stop_at_soft_wraps: true });
    k.bind("ctrl-v", editor::actions::MovePageDown { center_cursor: true });
    k.bind("ctrl-shift-v", editor::actions::MovePageUp { center_cursor: true });
    k.bind("ctrl-cmd-space", editor::actions::ShowCharacterPalette);
    k.bind("cmd-'", editor::actions::ToggleSelectedDiffHunks);
    k.bind("cmd-\"", editor::actions::ExpandAllDiffHunks);

    k.group(Some("Editor && mode == full"));
    k.bind("shift-enter", editor::actions::Newline);
    k.bind("enter", editor::actions::Newline);
    k.bind("cmd-enter", editor::actions::NewlineBelow);
    k.bind("cmd-shift-enter", editor::actions::NewlineAbove);
    k.bind("cmd-f", actions::buffer_search::Deploy::default());
    k.bind("cmd-alt-l", actions::buffer_search::Deploy { selection_search_enabled: true, ..Default::default() });
    k.bind("cmd-e", actions::buffer_search::UseSelectionForFind);
    k.bind("cmd-alt-e", editor::actions::SelectEnclosingSymbol);
    k.bind("alt-enter", editor::actions::OpenSelectionsInMultibuffer);
    k.bind("alt-cmd-f", search::text_finder::Toggle);

    k.group(Some("Editor && multibuffer"));
    k.bind("cmd-up", editor::actions::MoveToStartOfExcerpt);
    k.bind("cmd-down", editor::actions::MoveToStartOfNextExcerpt);
    k.bind("cmd-shift-up", editor::actions::SelectToStartOfExcerpt);
    k.bind("cmd-shift-down", editor::actions::SelectToStartOfNextExcerpt);
    k.bind("shift-enter", editor::actions::ExpandExcerpts::default());

    k.group(Some("Editor && mode == auto_height"));
    k.bind("ctrl-enter", editor::actions::Newline);
    k.bind("shift-enter", editor::actions::Newline);
    k.bind("ctrl-shift-enter", editor::actions::NewlineBelow);

    k.group(Some("Markdown"));
    k.bind("cmd-c", markdown::Copy);

    k.group(Some("Editor"));
    k.bind("cmd-alt-z", git::Restore);
    k.bind("cmd-alt-y", git::ToggleStaged);
    k.bind("cmd-y", git::StageAndNext);
    k.bind("cmd-shift-y", git::UnstageAndNext);

    k.group(Some("BufferSearchBar"));
    k.bind("escape", actions::buffer_search::Dismiss);
    k.bind("tab", actions::buffer_search::FocusEditor);
    k.bind("enter", actions::search::SelectNextMatch);
    k.bind("shift-enter", actions::search::SelectPreviousMatch);
    k.bind("alt-enter", search::SelectAllMatches);
    k.bind("cmd-f", actions::search::FocusSearch);
    k.bind("cmd-alt-l", search::ToggleSelection);
    k.bind("alt-cmd-f", search::text_finder::Toggle);

    k.group(Some("BufferSearchBar > Editor"));
    k.bind("tab", search::FocusNextField);
    k.bind("shift-tab", search::FocusPreviousField);

    k.group(Some("BufferSearchBar && in_replace > Editor"));
    k.bind("enter", search::ReplaceNext);
    k.bind("cmd-enter", search::ReplaceAll);

    k.group(Some("BufferSearchBar && !in_replace > Editor"));
    k.bind("shift-enter", actions::search::SelectPreviousMatch);
    k.bind("up", search::PreviousHistoryQuery);
    k.bind("down", search::NextHistoryQuery);

    k.group(Some("BufferSearchBar || ProjectSearchBar"));
    k.bind("ctrl-enter", editor::actions::Newline);

    k.group(Some("ProjectSearchBar"));
    k.bind("escape", search::project_search::ToggleFocus);
    k.bind("cmd-shift-j", search::project_search::ToggleFilters);
    k.bind("cmd-shift-enter", search::project_search::ToggleAllSearchResults);
    k.bind("cmd-shift-f", actions::search::FocusSearch);
    k.bind("cmd-shift-h", search::ToggleReplace);
    k.bind("alt-cmd-g", search::ToggleRegex);
    k.bind("alt-cmd-x", search::ToggleRegex);
    k.bind("alt-cmd-f", search::project_search::OpenTextFinder);

    k.group(Some("ProjectSearchBar > Editor"));
    k.bind("tab", search::FocusNextField);
    k.bind("shift-tab", search::FocusPreviousField);
    k.bind("up", search::PreviousHistoryQuery);
    k.bind("down", search::NextHistoryQuery);

    k.group(Some("ProjectSearchBar && in_replace > Editor"));
    k.bind("enter", search::ReplaceNext);
    k.bind("cmd-enter", search::ReplaceAll);

    k.group(Some("ProjectSearchView"));
    k.bind("escape", search::project_search::ToggleFocus);
    k.bind("cmd-shift-j", search::project_search::ToggleFilters);
    k.bind("cmd-shift-enter", search::project_search::ToggleAllSearchResults);
    k.bind("cmd-shift-h", search::ToggleReplace);
    k.bind("alt-cmd-g", search::ToggleRegex);
    k.bind("alt-cmd-x", search::ToggleRegex);
    k.bind("alt-cmd-f", search::project_search::OpenTextFinder);

    k.group(Some("Pane"));
    k.bind("alt-cmd-left", workspace::pane::ActivatePreviousItem::default());
    k.bind("cmd-{", workspace::pane::ActivatePreviousItem::default());
    k.bind("alt-cmd-right", workspace::pane::ActivateNextItem::default());
    k.bind("cmd-}", workspace::pane::ActivateNextItem::default());
    k.bind("ctrl-shift-pageup", workspace::pane::SwapItemLeft);
    k.bind("ctrl-shift-pagedown", workspace::pane::SwapItemRight);
    k.bind("alt-cmd-t", workspace::pane::CloseOtherItems { close_pinned: false, ..Default::default() });
    k.bind("ctrl-alt-cmd-w", workspace::CloseInactiveTabsAndPanes::default());
    k.bind("cmd-f", search::project_search::ToggleFocus);
    k.bind("cmd-g", actions::search::SelectNextMatch);
    k.bind("cmd-shift-g", actions::search::SelectPreviousMatch);
    k.bind("cmd-shift-h", search::ToggleReplace);
    k.bind("cmd-alt-l", search::ToggleSelection);
    k.bind("alt-enter", search::SelectAllMatches);
    k.bind("alt-cmd-c", actions::search::ToggleCaseSensitive);
    k.bind("alt-cmd-w", search::ToggleWholeWord);
    k.bind("alt-cmd-x", search::ToggleRegex);

    k.group(Some("Editor"));
    k.bind("cmd-ctrl-p", editor::actions::AddSelectionAbove { skip_soft_wrap: false });
    k.bind("cmd-alt-up", editor::actions::AddSelectionAbove { skip_soft_wrap: true });
    k.bind("cmd-ctrl-n", editor::actions::AddSelectionBelow { skip_soft_wrap: false });
    k.bind("cmd-alt-down", editor::actions::AddSelectionBelow { skip_soft_wrap: true });
    k.bind("cmd-shift-k", editor::actions::DeleteLine);
    k.bind("alt-up", editor::actions::MoveLineUp);
    k.bind("alt-down", editor::actions::MoveLineDown);
    k.bind("alt-shift-up", editor::actions::DuplicateLineUp);
    k.bind("alt-shift-down", editor::actions::DuplicateLineDown);
    k.bind("cmd-ctrl-left", editor::actions::SelectSmallerSyntaxNode);
    k.bind("cmd-ctrl-right", editor::actions::SelectLargerSyntaxNode);
    k.bind("cmd-ctrl-up", editor::actions::SelectPreviousSyntaxNode);
    k.bind("ctrl-shift-right", editor::actions::SelectLargerSyntaxNode);
    k.bind("ctrl-shift-left", editor::actions::SelectSmallerSyntaxNode);
    k.bind("cmd-ctrl-down", editor::actions::SelectNextSyntaxNode);
    k.bind("cmd-d", editor::actions::SelectNext { replace_newest: false });
    k.bind("cmd-shift-l", editor::actions::SelectAllMatches);
    k.bind("cmd-f2", editor::actions::SelectAllMatches);
    k.bind("ctrl-cmd-d", editor::actions::SelectPrevious { replace_newest: false });
    k.bind(
        "cmd-/",
        editor::actions::ToggleComments { advance_downwards: false, comment_empty_lines: true, ..Default::default() },
    );
    k.bind("shift-alt-a", editor::actions::ToggleBlockComments);
    k.bind(
        "f8",
        editor::actions::GoToDiagnostic {
            severity: project::project_settings::GoToDiagnosticSeverityFilter::Range {
                min: project::project_settings::GoToDiagnosticSeverity::Hint,
                max: project::project_settings::GoToDiagnosticSeverity::Error,
            },
        },
    );
    k.bind(
        "shift-f8",
        editor::actions::GoToPreviousDiagnostic {
            severity: project::project_settings::GoToDiagnosticSeverityFilter::Range {
                min: project::project_settings::GoToDiagnosticSeverity::Hint,
                max: project::project_settings::GoToDiagnosticSeverity::Error,
            },
        },
    );
    k.bind("f2", editor::actions::Rename);
    k.bind("f12", editor::actions::GoToDefinition::default());
    k.bind("cmd-f12", editor::actions::GoToTypeDefinition::default());
    k.bind("shift-f12", editor::actions::GoToImplementation::default());
    k.bind("alt-shift-f12", editor::actions::FindAllReferences::default());
    k.bind("cmd-|", editor::actions::MoveToEnclosingBracket);
    k.bind("ctrl-m", editor::actions::MoveToEnclosingBracket);
    k.bind("alt-cmd-[", editor::actions::Fold);
    k.bind("alt-cmd-]", editor::actions::UnfoldLines);
    k.bind("ctrl-space", editor::actions::ShowCompletions);
    k.bind("ctrl-shift-space", editor::actions::ShowWordCompletions);
    k.bind("cmd-.", editor::actions::ToggleCodeActions::default());
    k.bind("cmd-\\", workspace::pane::SplitRight::default());

    k.group(Some("Editor"));
    k.bind("cmd-shift-v", workspace::preview::OpenPreview);

    k.group(Some("Editor && mode == full"));
    k.bind("ctrl-g", editor::actions::ToggleGoToLine);
    k.bind("cmd-shift-backspace", editor::actions::GoToPreviousChange);
    k.bind("cmd-shift-alt-backspace", editor::actions::GoToNextChange);

    k.group(Some("Pane"));
    k.bind("ctrl-1", workspace::pane::ActivateItem(0));
    k.bind("ctrl-2", workspace::pane::ActivateItem(1));
    k.bind("ctrl-3", workspace::pane::ActivateItem(2));
    k.bind("ctrl-4", workspace::pane::ActivateItem(3));
    k.bind("ctrl-5", workspace::pane::ActivateItem(4));
    k.bind("ctrl-6", workspace::pane::ActivateItem(5));
    k.bind("ctrl-7", workspace::pane::ActivateItem(6));
    k.bind("ctrl-8", workspace::pane::ActivateItem(7));
    k.bind("ctrl-9", workspace::pane::ActivateItem(8));
    k.bind("ctrl-0", workspace::pane::ActivateLastItem);

    k.group(Some("Workspace"));
    k.bind("f6", workspace::FocusNextPart);
    k.bind("shift-f6", workspace::FocusPreviousPart);
    k.bind("cmd-f6", workspace::FocusNextPart);
    k.bind("fn-f", crate::ed::ToggleFullScreen);
    k.bind("ctrl-cmd-f", crate::ed::ToggleFullScreen);
    k.bind("alt-cmd-o", actions::OpenRecent::default());
    k.bind("ctrl-r", actions::OpenRecent::default());
    k.bind("cmd-ctrl-b", git::Branch);
    k.bind("cmd-ctrl-w", git::Worktree);
    k.bind("cmd-s", workspace::Save::default());
    k.bind("alt-shift-enter", workspace::toast::RunAction);
    k.bind("cmd-shift-s", workspace::SaveAs);
    k.bind("cmd-n", workspace::NewWindow);
    k.bind("cmd-1", workspace::mode::Editor);
    k.bind("cmd-2", workspace::mode::GitHistory);
    k.bind("cmd-3", workspace::mode::Terminal);
    k.bind("cmd-b", workspace::CycleLeftDock);
    k.bind("cmd-r", workspace::CycleRightDock);
    k.bind("cmd-j", terminal_view::terminal_panel::ToggleShell);
    k.bind("ctrl-alt-0", workspace::ResetActiveDockSize);
    k.bind("ctrl-alt--", workspace::DecreaseActiveDockSize { px: 0 });
    k.bind("ctrl-alt-=", workspace::IncreaseActiveDockSize { px: 0 });
    k.bind("ctrl-alt-)", workspace::ResetOpenDocksSize);
    k.bind("ctrl-alt-_", workspace::DecreaseOpenDocksSize { px: 0 });
    k.bind("ctrl-alt-+", workspace::IncreaseOpenDocksSize { px: 0 });
    k.bind("cmd-shift-f", workspace::pane::DeploySearch::default());
    k.bind("cmd-shift-h", workspace::pane::DeploySearch { replace_enabled: true, ..Default::default() });
    k.bind("cmd-shift-t", workspace::pane::ReopenClosedItem);
    k.bind("cmd-p", workspace::ToggleFinder::default());
    k.bind("cmd-shift-p", actions::command_palette::Toggle);
    k.bind("cmd-shift-m", diagnostics::Deploy);
    k.bind("cmd-w", workspace::pane::CloseActiveItem { close_pinned: false, ..Default::default() });
    k.bind("ctrl-alt-w", workspace::CloseActiveDock);

    k.group(Some("Workspace && !Terminal"));
    k.bind("cmd-t", workspace::NewFile);

    k.group(Some("Editor"));
    k.bind("cmd-u", editor::actions::UndoSelection);
    k.bind("cmd-shift-u", editor::actions::ToggleCase);
    k.bind("f1", editor::actions::OpenDocs);
    k.bind("ctrl-j", editor::actions::JoinLines);
    k.bind("ctrl-alt-backspace", editor::actions::DeleteToPreviousSubwordStart::default());
    k.bind("ctrl-alt-h", editor::actions::DeleteToPreviousSubwordStart::default());
    k.bind("ctrl-alt-delete", editor::actions::DeleteToNextSubwordEnd::default());
    k.bind("ctrl-alt-d", editor::actions::DeleteToNextSubwordEnd::default());
    k.bind("ctrl-alt-left", editor::actions::MoveToPreviousSubwordStart);
    k.bind("ctrl-alt-b", editor::actions::MoveToPreviousSubwordStart);
    k.bind("ctrl-alt-right", editor::actions::MoveToNextSubwordEnd);
    k.bind("ctrl-alt-f", editor::actions::MoveToNextSubwordEnd);
    k.bind("ctrl-alt-shift-left", editor::actions::SelectToPreviousSubwordStart);
    k.bind("ctrl-alt-shift-b", editor::actions::SelectToPreviousSubwordStart);
    k.bind("ctrl-alt-shift-right", editor::actions::SelectToNextSubwordEnd);
    k.bind("ctrl-alt-shift-f", editor::actions::SelectToNextSubwordEnd);

    k.group(Some("Editor && renaming"));
    k.bind("enter", editor::actions::ConfirmRename);

    k.group(Some("Editor && showing_completions"));
    k.bind("enter", editor::actions::ConfirmCompletion::default());
    k.bind("shift-enter", editor::actions::ConfirmCompletionReplace);
    k.bind("tab", editor::actions::ComposeCompletion::default());

    k.group(Some("Editor && in_snippet && has_next_tabstop && !showing_completions"));
    k.bind("tab", editor::actions::NextSnippetTabstop);

    k.group(Some("Editor && in_snippet && has_previous_tabstop && !showing_completions"));
    k.bind("shift-tab", editor::actions::PreviousSnippetTabstop);

    k.group(Some("Editor && showing_code_actions"));
    k.bind("enter", editor::actions::ConfirmCodeAction::default());

    k.group(Some("Editor && (showing_code_actions || showing_completions)"));
    k.bind("up", editor::actions::ContextMenuPrevious);
    k.bind("ctrl-p", editor::actions::ContextMenuPrevious);
    k.bind("down", editor::actions::ContextMenuNext);
    k.bind("ctrl-n", editor::actions::ContextMenuNext);
    k.bind("pageup", editor::actions::ContextMenuFirst);
    k.bind("pagedown", editor::actions::ContextMenuLast);

    k.group(Some("Editor && mode == full"));
    k.bind("cmd-shift-e", workspace::pane::RevealInProjectPanel::default());
    k.bind("cmd-f8", editor::actions::GoToHunk);
    k.bind("cmd-shift-f8", editor::actions::GoToPreviousHunk);

    k.group(Some("ProjectSearchBar && !in_replace"));
    k.bind("cmd-enter", search::project_search::SearchInNew);

    k.group(Some("OutlinePanel && not_editing"));
    k.bind("escape", actions::menu::Cancel);
    k.bind("left", outline_panel::CollapseSelectedEntry);
    k.bind("right", outline_panel::ExpandSelectedEntry);
    k.bind("cmd-alt-c", workspace::CopyPath);
    k.bind("alt-cmd-shift-c", workspace::CopyRelativePath);
    k.bind("alt-cmd-r", outline_panel::RevealInFileManager);
    k.bind("space", outline_panel::OpenSelectedEntry);
    k.bind("shift-down", actions::menu::SelectNext);
    k.bind("shift-up", actions::menu::SelectPrevious);

    k.group(Some("ProjectPanel"));
    k.bind("left", project_panel::CollapseSelectedEntry);
    k.bind("cmd-left", project_panel::CollapseAllEntries);
    k.bind("right", project_panel::ExpandSelectedEntry);
    k.bind("cmd-right", project_panel::ExpandAllEntries);
    k.bind("cmd-d", project_panel::Duplicate);
    k.bind("alt-cmd-n", project_panel::NewDirectory);
    k.bind("cmd-x", project_panel::Cut);
    k.bind("cmd-c", project_panel::Copy);
    k.bind("cmd-v", project_panel::Paste);
    k.bind("cmd-alt-c", workspace::CopyPath);
    k.bind("alt-cmd-shift-c", workspace::CopyRelativePath);
    k.bind("cmd-z", project_panel::Undo);
    k.bind("cmd-shift-z", project_panel::Redo);
    k.bind("enter", project_panel::Rename);
    k.bind("f2", project_panel::Rename);
    k.bind("backspace", project_panel::Delete { skip_prompt: false });
    k.bind("delete", project_panel::Delete { skip_prompt: false });
    k.bind("cmd-backspace", project_panel::Delete { skip_prompt: true });
    k.bind("cmd-delete", project_panel::Delete { skip_prompt: true });
    k.bind("alt-cmd-r", project_panel::RevealInFileManager);
    k.bind("ctrl-shift-enter", workspace::OpenWithSystem);
    k.bind("alt-d", project_panel::CompareMarkedFiles);
    k.bind("cmd-alt-shift-f", project_panel::NewSearchInDirectory);
    k.bind("shift-f10", project_panel::OpenContextMenu);
    k.bind("shift-down", actions::menu::SelectNext);
    k.bind("shift-up", actions::menu::SelectPrevious);
    k.bind("escape", actions::menu::Cancel);

    k.group(Some("ProjectPanel && not_editing"));
    k.bind("space", project_panel::Open);

    k.group(Some("GitPanel && ChangesList && !GitBranchSelector"));
    k.bind("shift-up", actions::menu::SelectPrevious);
    k.bind("shift-down", actions::menu::SelectNext);
    k.bind("enter", actions::menu::Confirm);
    k.bind("escape", actions::menu::Cancel);
    k.bind("up", git_ui::git_panel::PreviousEntry);
    k.bind("down", git_ui::git_panel::NextEntry);
    k.bind("cmd-up", git_ui::git_panel::FirstEntry);
    k.bind("cmd-down", git_ui::git_panel::LastEntry);
    k.bind("left", git_ui::git_panel::CollapseSelectedEntry);
    k.bind("right", git_ui::git_panel::ExpandSelectedEntry);
    k.bind("cmd-alt-y", git::ToggleStaged);
    k.bind("space", git::ToggleStaged);
    k.bind("shift-space", git::StageRange);
    k.bind("cmd-y", git::StageFile);
    k.bind("cmd-shift-y", git::UnstageFile);
    k.bind("cmd-alt-c", workspace::CopyPath);
    k.bind("alt-cmd-shift-c", workspace::CopyRelativePath);
    k.bind("alt-down", git_ui::git_panel::FocusEditor);
    k.bind("tab", git_ui::git_panel::FocusEditor);
    k.bind("shift-tab", git_ui::git_panel::FocusEditor);
    k.bind("backspace", git::RestoreFile { skip_prompt: false });
    k.bind("delete", git::RestoreFile { skip_prompt: false });
    k.bind("cmd-backspace", git::RestoreFile { skip_prompt: true });
    k.bind("cmd-delete", git::RestoreFile { skip_prompt: true });

    k.group(Some("GitPanel && CommitEditor"));
    k.bind("escape", git::Cancel);

    k.group(Some("GitDiff > Editor"));
    k.bind("cmd-enter", git::Commit);
    k.bind("cmd-shift-enter", git::Amend);
    k.bind("cmd-ctrl-y", git::StageAll);
    k.bind("cmd-ctrl-shift-y", git::UnstageAll);
    k.bind("cmd-alt-z", git::RestoreAndNext);

    k.group(Some("CommitEditor > Editor && !showing_code_actions && !showing_completions"));
    k.bind("enter", editor::actions::Newline);

    k.group(Some("CommitEditor > Editor"));
    k.bind("cmd-enter", git::Commit);
    k.bind("cmd-shift-enter", git::Amend);
    k.bind("tab", git_ui::git_panel::FocusChanges);
    k.bind("shift-tab", git_ui::git_panel::FocusChanges);
    k.bind("alt-up", git_ui::git_panel::FocusChanges);
    k.bind("alt-shift-escape", git::ToggleFillCommitEditor);

    k.group(Some("GitPanel"));
    k.bind("cmd-ctrl-y", git::StageAll);
    k.bind("cmd-ctrl-shift-y", git::UnstageAll);
    k.bind("cmd-enter", git::Commit);
    k.bind("cmd-shift-enter", git::Amend);

    k.group(Some("GitCommit > Editor && mode == auto_height && !showing_code_actions && !showing_completions"));
    k.bind("enter", editor::actions::Newline);

    k.group(Some("GitCommit > Editor && mode == auto_height"));
    k.bind("escape", actions::menu::Cancel);
    k.bind("cmd-enter", git::Commit);
    k.bind("cmd-shift-enter", git::Amend);

    k.group(Some("Picker > Editor"));
    k.bind("escape", actions::menu::Cancel);
    k.bind("up", actions::menu::SelectPrevious);
    k.bind("down", actions::menu::SelectNext);
    k.bind("tab", picker::ConfirmCompletion);
    k.bind("alt-enter", picker::ConfirmInput { secondary: false });
    k.bind("cmd-alt-enter", picker::ConfirmInput { secondary: true });

    k.group(Some("LspCommandSelector > Picker > Editor"));
    k.bind("tab", lsp_command_selector::ToggleArgumentsFocus);

    k.group(Some("Finder || (Finder > Picker > Editor)"));
    k.bind("cmd-shift-i", actions::search::ToggleIncludeIgnored);
    k.bind("cmd-1", finder::ShowFiles);
    k.bind("cmd-2", finder::ShowSymbols);
    k.bind("cmd-3", finder::ShowTabs);
    k.bind("ctrl-backspace", finder::CloseSelectedTab);

    k.group(Some("RecentProjects || (RecentProjects > Picker > Editor)"));
    k.bind("cmd-shift-a", workspace::AddFolderToProject);
    k.bind("shift-backspace", recent_projects::RemoveSelected);
    k.bind("cmd-shift-enter", recent_projects::AddToWorkspace);

    k.group(Some("StashList || (StashList > Picker > Editor)"));
    k.bind("ctrl-shift-backspace", git_ui::stash_picker::DropStashItem);
    k.bind("ctrl-shift-v", git_ui::stash_picker::ShowStashItem);

    k.group(Some("Terminal"));
    k.bind("shift-enter", terminal_view::SendText("\x1b\r".into()));
    k.bind("ctrl-cmd-space", terminal::ShowCharacterPalette);
    k.bind("cmd-c", terminal::Copy);
    k.bind("cmd-v", terminal::Paste);
    k.bind("ctrl-cmd-v", terminal::PasteText);
    k.bind("cmd-f", actions::buffer_search::Deploy::default());
    k.bind("cmd-a", editor::actions::SelectAll);
    k.bind("cmd-k", terminal::Clear);
    k.bind("cmd-t", workspace::NewTerminal);
    k.bind("cmd-backspace", terminal_view::SendKeystroke("ctrl-u".into()));
    k.bind("alt-delete", terminal_view::SendText("\x1bd".into()));
    k.bind("cmd-delete", terminal_view::SendKeystroke("ctrl-k".into()));
    k.bind("cmd-right", terminal_view::SendKeystroke("ctrl-e".into()));
    k.bind("cmd-left", terminal_view::SendKeystroke("ctrl-a".into()));
    k.bind("alt-left", terminal_view::SendText("\x1bb".into()));
    k.bind("alt-right", terminal_view::SendText("\x1bf".into()));
    k.bind("alt-b", terminal_view::SendText("\x1bb".into()));
    k.bind("alt-f", terminal_view::SendText("\x1bf".into()));
    k.bind("ctrl-delete", terminal_view::SendText("\x1b[3;5~".into()));
    k.bind("up", terminal_view::SendKeystroke("up".into()));
    k.bind("pageup", terminal_view::SendKeystroke("pageup".into()));
    k.bind("down", terminal_view::SendKeystroke("down".into()));
    k.bind("pagedown", terminal_view::SendKeystroke("pagedown".into()));
    k.bind("escape", terminal_view::SendKeystroke("escape".into()));
    k.bind("enter", terminal_view::SendKeystroke("enter".into()));
    k.bind("ctrl-c", terminal_view::SendKeystroke("ctrl-c".into()));
    k.bind("ctrl-r", terminal_view::SendKeystroke("ctrl-r".into()));
    k.bind("ctrl-backspace", terminal_view::SendKeystroke("ctrl-w".into()));
    k.bind("shift-pageup", terminal::ScrollPageUp);
    k.bind("cmd-up", terminal::ScrollPageUp);
    k.bind("shift-pagedown", terminal::ScrollPageDown);
    k.bind("cmd-down", terminal::ScrollPageDown);
    k.bind("shift-up", terminal::ScrollLineUp);
    k.bind("shift-down", terminal::ScrollLineDown);
    k.bind("shift-home", terminal::ScrollToTop);
    k.bind("cmd-home", terminal::ScrollToTop);
    k.bind("shift-end", terminal::ScrollToBottom);
    k.bind("cmd-end", terminal::ScrollToBottom);
    k.bind("ctrl-shift-space", terminal::ToggleViMode);
    k.bind("ctrl-alt-up", workspace::pane::SplitUp::default());
    k.bind("ctrl-alt-down", workspace::pane::SplitDown::default());
    k.bind("ctrl-alt-left", workspace::pane::SplitLeft::default());
    k.bind("ctrl-alt-right", workspace::pane::SplitRight::default());
    k.bind("cmd-d", workspace::pane::SplitRight::default());
    k.bind("cmd-shift-d", workspace::pane::SplitDown::default());
    k.bind("cmd-alt-r", terminal_view::RerunTask);

    k.group(Some("ConfigureContextServerModal > Editor"));
    k.bind("escape", actions::menu::Cancel);
    k.bind("enter", editor::actions::Newline);
    k.bind("cmd-enter", actions::menu::Confirm);

    k.group(Some("MarkdownPreview"));
    k.bind("pageup", markdown_preview::ScrollPageUp);
    k.bind("pagedown", markdown_preview::ScrollPageDown);
    k.bind("up", markdown_preview::ScrollUp);
    k.bind("down", markdown_preview::ScrollDown);
    k.bind("alt-up", markdown_preview::ScrollUpByItem);
    k.bind("alt-down", markdown_preview::ScrollDownByItem);
    k.bind("cmd-up", markdown_preview::ScrollToTop);
    k.bind("cmd-down", markdown_preview::ScrollToBottom);
    k.bind("cmd-shift-v", markdown_preview::CloseAndReturnToEditor);
    k.bind("cmd-f", actions::buffer_search::Deploy::default());

    k.group(Some("InvalidBuffer"));
    k.bind("ctrl-shift-enter", workspace::OpenWithSystem);

    k.group(Some("StashDiff > Editor"));
    k.bind("ctrl-space", git_ui::commit_view::ApplyCurrentStash);
    k.bind("ctrl-shift-space", git_ui::commit_view::PopCurrentStash);
    k.bind("ctrl-shift-backspace", git_ui::commit_view::DropCurrentStash);

    k.group(Some("GitBranchSelector || (GitBranchSelector > Picker > Editor)"));
    k.bind("cmd-shift-backspace", git_ui::branch_picker::DeleteBranch);
    k.bind("cmd-alt-shift-backspace", git_ui::branch_picker::ForceDeleteBranch);
    k.bind("cmd-shift-i", git_ui::branch_picker::CycleBranchFilter);
    k.bind("cmd-k", git_ui::branch_picker::ToggleFilterMenu);

    k.group(Some("ImageViewer"));
    k.bind("cmd-=", image_viewer::ZoomIn);
    k.bind("cmd-+", image_viewer::ZoomIn);
    k.bind("cmd--", image_viewer::ZoomOut);
    k.bind("cmd-0", image_viewer::ResetZoom);
    k.bind("cmd-shift-0", image_viewer::FitToView);

    k.group(Some("WorktreePicker || (WorktreePicker > Picker > Editor)"));
    k.bind("cmd-shift-backspace", git_ui_core::worktree_picker::DeleteWorktree);
    k.bind("cmd-alt-shift-backspace", git_ui_core::worktree_picker::ForceDeleteWorktree);

    k.group(Some("GitGraph"));
    k.bind("tab", git_ui::git_graph::FocusNextTabStop);
    k.bind("shift-tab", git_ui::git_graph::FocusPreviousTabStop);
    k.bind("cmd-f", actions::buffer_search::Deploy::default());
    k.bind("cmd-r", git_ui::git_graph::Refresh);

    k.group(Some("GitGraphSearchBar > Editor"));
    k.bind("tab", git_ui::git_graph::FocusNextTabStop);
    k.bind("shift-tab", git_ui::git_graph::FocusPreviousTabStop);

    k.group(Some("Picker > Editor"));
    k.bind("cmd-shift-s", picker::ToggleMultiSelect);

    k.group(Some("Finder || (Finder > Picker > Editor) || (Finder > Picker > menu)"));
    k.bind("tab", picker::MultiSelectNext);

    k.group(Some("TextFinder || (TextFinder > Picker > Editor) || (TextFinder > Picker > menu)"));
    k.bind("alt-cmd-f", search::text_finder::ToProjectSearch);
    k.bind("tab", picker::MultiSelectNext);
    k.bind("alt-cmd-[", search::text_finder::Fold);
    k.bind("alt-cmd-]", search::text_finder::Unfold);
    k.bind("cmd-shift-enter", search::text_finder::ToggleFoldAll);
    k.bind("cmd-j", workspace::pane::SplitDown::default());
    k.bind("cmd-k", workspace::pane::SplitUp::default());
    k.bind("cmd-h", workspace::pane::SplitLeft::default());
    k.bind("cmd-l", workspace::pane::SplitRight::default());
    k.bindings
}

#[cfg(test)]
mod tests {
    use gpui::KeyBinding;
    use rustc_hash::FxHashMap;

    use super::keymap;

    fn keystrokes(binding: &KeyBinding) -> String {
        binding.keystrokes().iter().map(|keystroke| keystroke.inner().unparse()).collect::<Vec<_>>().join(" ")
    }

    fn context(binding: &KeyBinding) -> Option<String> {
        binding.predicate().map(|predicate| predicate.to_string())
    }

    #[test]
    fn bindings_carry_their_action_input() {
        let keymap = keymap();
        let action = |context_name: &str, keys: &str| {
            keymap
                .iter()
                .find(|binding| context(binding).as_deref() == Some(context_name) && keystrokes(binding) == keys)
                .unwrap_or_else(|| panic!("{context_name} binds {keys}"))
                .action()
        };
        assert!(action("Terminal", "ctrl-c").partial_eq(&terminal_view::SendKeystroke("ctrl-c".into())));
        assert!(action("Pane", "ctrl-1").partial_eq(&workspace::pane::ActivateItem(0)));
        assert!(action("Pane", "ctrl-9").partial_eq(&workspace::pane::ActivateItem(8)));
    }

    #[test]
    fn each_key_is_bound_once_per_context() {
        let mut seen = FxHashMap::default();
        for binding in keymap() {
            let key = (context(&binding), keystrokes(&binding));
            if let Some(previous) = seen.insert(key.clone(), binding.action().name()) {
                panic!("{key:?} bound to both {previous} and {}", binding.action().name());
            }
        }
    }

    #[test]
    fn new_tab_and_new_window_bindings() {
        let keymap = keymap();
        for (keys, expected) in [
            ("cmd-n", vec![(Some("Workspace"), "workspace::NewWindow")]),
            (
                "cmd-t",
                vec![
                    (Some("Workspace && !Terminal"), "workspace::NewFile"),
                    (Some("Terminal"), "workspace::NewTerminal"),
                ],
            ),
            ("cmd-shift-n", vec![]),
        ] {
            let bound: Vec<_> = keymap
                .iter()
                .filter(|binding| keystrokes(binding) == keys)
                .map(|binding| (context(binding), binding.action().name()))
                .collect();
            let expected: Vec<_> =
                expected.into_iter().map(|(context, action)| (context.map(str::to_owned), action)).collect();
            assert_eq!(bound, expected, "{keys}");
        }
    }

    #[test]
    fn expand_excerpts_is_multibuffer_only() {
        let keymap = keymap();
        let contexts: Vec<_> = keymap
            .iter()
            .filter(|binding| binding.action().name() == "editor::ExpandExcerpts")
            .filter_map(context)
            .collect();
        assert_eq!(contexts, ["Editor && multibuffer"]);
    }

    #[test]
    fn terminal_split_down_binding() {
        let keymap = keymap();
        let split_down = keymap
            .iter()
            .find(|binding| context(binding).as_deref() == Some("Terminal") && keystrokes(binding) == "cmd-shift-d")
            .expect("terminal binds cmd-shift-d");
        assert_eq!(split_down.action().name(), "pane::SplitDown");
    }

    #[test]
    fn nav_history_has_one_binding_per_direction() {
        let keymap = keymap();
        for (action, expected) in [("workspace::GoBack", "cmd-["), ("workspace::GoForward", "cmd-]")] {
            let bound: Vec<_> =
                keymap.iter().filter(|binding| binding.action().name() == action).map(keystrokes).collect();
            assert_eq!(bound, [expected], "{action}");
        }
    }

    #[test]
    fn commit_editor_enter_yields_to_menus() {
        let keymap = keymap();
        let newlines: Vec<_> = keymap
            .iter()
            .filter(|binding| keystrokes(binding) == "enter" && binding.action().name() == "editor::Newline")
            .filter_map(context)
            .filter(|context| context.contains("CommitEditor") || context.contains("GitCommit"))
            .collect();
        assert_eq!(newlines.len(), 2);
        for context in newlines {
            assert!(context.contains("!showing_code_actions") && context.contains("!showing_completions"), "{context}");
        }
    }

    #[test]
    fn search_bar_tab_cycles_fields() {
        let keymap = keymap();
        for bar in ["BufferSearchBar > Editor", "ProjectSearchBar > Editor"] {
            for (keys, action) in [("tab", "search::FocusNextField"), ("shift-tab", "search::FocusPreviousField")] {
                let bound: Vec<_> = keymap
                    .iter()
                    .filter(|binding| context(binding).as_deref() == Some(bar) && keystrokes(binding) == keys)
                    .map(|binding| binding.action().name())
                    .collect();
                assert_eq!(bound, [action], "{bar} {keys}");
            }
        }
    }

    #[test]
    fn personal_bindings_live_in_their_context_groups() {
        let keymap = keymap();
        for (context_name, keys, action) in [
            (Some("Editor"), "cmd-shift-u", "editor::ToggleCase"),
            (Some("Editor"), "f1", "rust::OpenDocs"),
            (Some("Terminal"), "shift-enter", "terminal::SendText"),
        ] {
            let bound: Vec<_> = keymap
                .iter()
                .filter(|binding| context(binding).as_deref() == context_name && keystrokes(binding) == keys)
                .map(|binding| binding.action().name())
                .collect();
            assert_eq!(bound, [action], "{keys}");
        }
    }
}
