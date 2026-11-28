use gpui::Action;
use gpui::actions;

actions!(
    ed,
    [
        /// Quits the application.
        pub Quit,
        /// Shows information about ed.
        pub About,
    ]
);

actions!(
    ed,
    [
        /// Decreases the font size in the editor buffer.
        pub DecreaseBufferFontSize,
        /// Increases the font size in the editor buffer.
        pub IncreaseBufferFontSize,
        /// Resets the buffer font size to the default value.
        pub ResetBufferFontSize,
        /// Decreases the font size of the user interface.
        pub DecreaseUiFontSize,
        /// Increases the font size of the user interface.
        pub IncreaseUiFontSize,
        /// Resets the UI font size to the default value.
        pub ResetUiFontSize,
        /// Resets all zoom levels (UI and buffer font sizes) to their default values.
        pub ResetAllZoom,
    ]
);

pub mod editor {
    use gpui::actions;
    actions!(
        editor,
        [
            /// Moves cursor up.
            pub MoveUp,
            /// Moves cursor down.
            pub MoveDown,
            /// Reveals the current file in the system file manager.
            pub RevealInFileManager,
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
            pub Open
        ]
    );
}

pub mod command_palette {
    use gpui::actions;

    actions!(
        command_palette,
        [
            /// Toggles the command palette.
            pub Toggle,
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
    #[action(namespace = search, no_build, no_register)]
    pub struct NewSearchInDirectory {
        pub directory: String,
    }

    actions!(
        search,
        [
            /// Focuses on the search input field.
            pub FocusSearch,
            /// Selects the next search match.
            pub SelectNextMatch,
            /// Selects the previous search match.
            pub SelectPreviousMatch,
            /// Toggles case-sensitive search.
            pub ToggleCaseSensitive,
            /// Toggles searching in ignored files.
            pub ToggleIncludeIgnored
        ]
    );
}
pub mod buffer_search {
    use gpui::Action;
    use gpui::actions;

    /// Opens the buffer search interface with the specified configuration.
    #[derive(PartialEq, Eq, Clone, Action)]
    #[action(namespace = buffer_search)]
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
            pub DeployReplace,
            /// Dismisses the search bar.
            pub Dismiss,
            /// Focuses back on the editor.
            pub FocusEditor,
            /// Sets the search query from the selection or word under cursor.
            pub UseSelectionForFind,
        ]
    );
}
/// Opens the recent projects interface.
#[derive(PartialEq, Eq, Clone, Default, Action)]
#[action(namespace = projects)]
pub struct OpenRecent {
    pub create_new_window: Option<bool>,
}

pub mod menu {
    use gpui::actions;

    actions!(
        menu,
        [
            /// Cancels the current menu operation.
            pub Cancel,
            /// Confirms the selected menu item.
            pub Confirm,
            /// Performs secondary confirmation action.
            pub SecondaryConfirm,
            /// Selects the previous item in the menu.
            pub SelectPrevious,
            /// Selects the next item in the menu.
            pub SelectNext,
            /// Selects the first item in the menu.
            pub SelectFirst,
            /// Selects the last item in the menu.
            pub SelectLast,
            /// Enters a submenu (navigates to child menu).
            pub SelectChild,
            /// Exits a submenu (navigates to parent menu).
            pub SelectParent,
            /// Restarts the menu from the beginning.
            pub Restart,
        ]
    );
}
