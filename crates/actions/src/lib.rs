use std::sync::Arc;

use gpui::Action;
use gpui::actions;
use serde::Deserialize;

// If the ed binary doesn't use anything in this crate, it will be optimized away
// and the actions won't initialize. So we just provide an empty initialization function
// to be called from main.
//
// These may provide relevant context:
// https://github.com/rust-lang/rust/issues/47384
// https://github.com/mmastrac/rust-ctor/issues/280
pub const fn init() {}

/// Opens a URL in the system's default web browser.
#[derive(Clone, PartialEq, Eq, Deserialize, Action)]
#[action(namespace = ed)]
#[serde(deny_unknown_fields)]
pub struct OpenBrowser {
    pub url: Arc<str>,
}

actions!(
    ed,
    [
        /// Quits the application.
        Quit,
        /// Shows information about ed.
        About,
    ]
);

actions!(
    ed,
    [
        /// Decreases the font size in the editor buffer.
        DecreaseBufferFontSize,
        /// Increases the font size in the editor buffer.
        IncreaseBufferFontSize,
        /// Resets the buffer font size to the default value.
        ResetBufferFontSize,
        /// Decreases the font size of the user interface.
        DecreaseUiFontSize,
        /// Increases the font size of the user interface.
        IncreaseUiFontSize,
        /// Resets the UI font size to the default value.
        ResetUiFontSize,
        /// Resets all zoom levels (UI and buffer font sizes) to their default values.
        ResetAllZoom,
    ]
);

pub mod editor {
    use gpui::actions;
    actions!(
        editor,
        [
            /// Moves cursor up.
            MoveUp,
            /// Moves cursor down.
            MoveDown,
            /// Reveals the current file in the system file manager.
            RevealInFileManager,
        ]
    );
}

pub mod git_graph {
    use gpui::actions;

    actions!(
        git_graph,
        [
            /// Switches the window to Git History mode and brings the active
            /// repository's graph to the front, even over its own commit tabs.
            Open
        ]
    );
}

pub mod command_palette {
    use gpui::actions;

    actions!(
        command_palette,
        [
            /// Toggles the command palette.
            Toggle,
        ]
    );
}

pub mod search {
    use gpui::Action;
    use gpui::actions;

    /// Opens a new project search filtered down to the given directory.
    ///
    /// An internal forwarding action: the user-facing, keybindable entry
    /// point is `project_panel::NewSearchInDirectory`, which resolves the
    /// selected directory and dispatches this action with it.
    #[derive(Clone, Debug, Default, PartialEq, Eq, Action)]
    #[action(namespace = search, no_json, no_register)]
    pub struct NewSearchInDirectory {
        pub directory: String,
    }

    actions!(
        search,
        [
            /// Focuses on the search input field.
            FocusSearch,
            /// Selects the next search match.
            SelectNextMatch,
            /// Selects the previous search match.
            SelectPreviousMatch,
            /// Toggles case-sensitive search.
            ToggleCaseSensitive,
            /// Toggles searching in ignored files.
            ToggleIncludeIgnored
        ]
    );
}
pub mod buffer_search {
    use gpui::Action;
    use gpui::actions;
    use serde::Deserialize;

    /// Opens the buffer search interface with the specified configuration.
    #[derive(PartialEq, Eq, Clone, Deserialize, Action)]
    #[action(namespace = buffer_search)]
    #[serde(deny_unknown_fields, default)]
    pub struct Deploy {
        pub focus: bool,
        pub replace_enabled: bool,
        pub selection_search_enabled: bool,
    }

    impl Default for Deploy {
        fn default() -> Deploy {
            Deploy::find()
        }
    }

    impl Deploy {
        pub const fn find() -> Self {
            Self { focus: true, replace_enabled: false, selection_search_enabled: false }
        }

        pub const fn replace() -> Self {
            Self { focus: true, replace_enabled: true, selection_search_enabled: false }
        }
    }

    actions!(
        buffer_search,
        [
            /// Deploys the search and replace interface.
            DeployReplace,
            /// Dismisses the search bar.
            Dismiss,
            /// Focuses back on the editor.
            FocusEditor,
            /// Sets the search query from the selection or word under cursor.
            UseSelectionForFind,
        ]
    );
}
/// Opens the recent projects interface.
#[derive(PartialEq, Eq, Clone, Deserialize, Default, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenRecent {
    #[serde(default)]
    pub create_new_window: Option<bool>,
}

pub mod menu {
    use gpui::actions;

    actions!(
        menu,
        [
            /// Cancels the current menu operation.
            Cancel,
            /// Confirms the selected menu item.
            Confirm,
            /// Performs secondary confirmation action.
            SecondaryConfirm,
            /// Selects the previous item in the menu.
            SelectPrevious,
            /// Selects the next item in the menu.
            SelectNext,
            /// Selects the first item in the menu.
            SelectFirst,
            /// Selects the last item in the menu.
            SelectLast,
            /// Enters a submenu (navigates to child menu).
            SelectChild,
            /// Exits a submenu (navigates to parent menu).
            SelectParent,
            /// Restarts the menu from the beginning.
            Restart,
            EndSlot,
        ]
    );
}
