use gpui::Hsla;

/// Sets a color that has a consistent meaning across all themes.
#[derive(Debug, Default, Eq, PartialEq, Copy, Clone)]
pub enum Color {
    #[default]
    /// The default text color. Might be known as "foreground" or "primary" in
    /// some theme systems.
    ///
    /// For less emphasis, consider using [`Color::Muted`] or [`Color::Hidden`].
    Default,
    /// A text color used for accents, such as links or highlights.
    Accent,
    /// A color used to indicate a conflict, such as a version control merge conflict, or a conflict between a file in the editor and the file system.
    Conflict,
    /// A color used to indicate a newly created item, such as a new file in
    /// version control, or a new file on disk.
    Created,
    /// It is highly, HIGHLY recommended not to use this! Using this color
    /// means detaching it from any semantic meaning across themes.
    ///
    /// A custom color specified by an HSLA value.
    Custom(Hsla),
    /// A color used to indicate a deleted item, such as a file removed from version control.
    Deleted,
    /// A color used for disabled UI elements or text, like a disabled button or menu item.
    Disabled,
    /// A color used to indicate an error condition, or something the user
    /// cannot do. In very rare cases, it might be used to indicate dangerous or
    /// destructive action.
    Error,
    /// A color used for elements that represent something that is hidden, like
    /// a hidden file, or an element that should be visually de-emphasized.
    Hidden,
    /// A color used for hint or suggestion text, often a blue color. Use this
    /// color to represent helpful, or semantically neutral information.
    Hint,
    /// A color used for items that are intentionally ignored, such as files ignored by version control.
    Ignored,
    /// A color used for informational messages or status indicators, often a blue color.
    Info,
    /// A color used to indicate a modified item, such as an edited file, or a modified entry in version control.
    Modified,
    /// A color used for text or UI elements that should be visually muted or de-emphasized.
    ///
    /// For more emphasis, consider using [`Color::Default`].
    ///
    /// For less emphasis, consider using [`Color::Hidden`].
    Muted,
    /// A color used for placeholder text in input fields.
    Placeholder,
    /// A color used to indicate selected text or UI elements.
    Selected,
    /// A color used to indicate a successful operation or status.
    Success,
    /// A version control color used to indicate a newly added file or content in version control.
    VersionControlAdded,
    /// A version control color used to indicate conflicting changes that need resolution.
    VersionControlConflict,
    /// A version control color used to indicate a file or content that has been deleted in version control.
    VersionControlDeleted,
    /// A version control color used to indicate files or content that is being ignored by version control.
    VersionControlIgnored,
    /// A version control color used to indicate modified files or content in version control.
    VersionControlModified,
    /// A color used to indicate a warning condition.
    Warning,
}

impl Color {
    /// Returns the Color's HSLA value.
    pub fn color(&self) -> Hsla {
        match self {
            Color::Default => theme::theme().colors().text,
            Color::Muted => theme::theme().colors().text_muted,
            Color::Created => theme::theme().status().created,
            Color::Modified => theme::theme().status().modified,
            Color::Conflict => theme::theme().status().conflict,
            Color::Ignored => theme::theme().status().ignored,
            Color::Deleted => theme::theme().status().deleted,
            Color::Disabled => theme::theme().colors().text_disabled,
            Color::Hidden => theme::theme().status().hidden,
            Color::Hint => theme::theme().status().hint,
            Color::Info => theme::theme().status().info,
            Color::Placeholder => theme::theme().colors().text_placeholder,
            Color::Accent => theme::theme().colors().text_accent,
            Color::Error => theme::theme().status().error,
            Color::Selected => theme::theme().colors().text_accent,
            Color::Success => theme::theme().status().success,
            Color::VersionControlAdded => theme::theme().colors().version_control_added,
            Color::VersionControlConflict => theme::theme().colors().version_control_conflict,
            Color::VersionControlDeleted => theme::theme().colors().version_control_deleted,
            Color::VersionControlIgnored => theme::theme().colors().version_control_ignored,
            Color::VersionControlModified => theme::theme().colors().version_control_modified,
            Color::Warning => theme::theme().status().warning,
            Color::Custom(color) => *color,
        }
    }
}

impl From<Hsla> for Color {
    fn from(color: Hsla) -> Self {
        Color::Custom(color)
    }
}
