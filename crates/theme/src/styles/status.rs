use gpui::Hsla;

#[derive(Clone, Debug, PartialEq)]
pub struct StatusColors {
    /// Indicates some kind of conflict, like a file changed on disk while it was open, or
    /// merge conflicts in a Git repository.
    pub conflict: Hsla,

    /// Indicates something new, like a new file added to a Git repository.
    pub created: Hsla,
    pub created_background: Hsla,

    /// Indicates that something no longer exists, like a deleted file.
    pub deleted: Hsla,
    pub deleted_background: Hsla,

    /// Indicates a system error, a failed operation or a diagnostic error.
    pub error: Hsla,
    pub error_background: Hsla,
    pub error_border: Hsla,

    /// Represents a hidden status, such as a file being hidden in a file tree.
    pub hidden: Hsla,

    /// Indicates a hint or some kind of additional information.
    pub hint: Hsla,
    pub hint_background: Hsla,
    pub hint_border: Hsla,

    /// Indicates that something is deliberately ignored, such as a file or operation ignored by Git.
    pub ignored: Hsla,
    pub ignored_background: Hsla,
    pub ignored_border: Hsla,

    /// Represents informational status updates or messages.
    pub info: Hsla,
    pub info_background: Hsla,
    pub info_border: Hsla,

    /// Indicates a changed or altered status, like a file that has been edited.
    pub modified: Hsla,

    /// Represents a renamed status, such as a file that has been renamed.
    pub renamed: Hsla,

    /// Indicates a successful operation or task completion.
    pub success: Hsla,
    pub success_background: Hsla,
    pub success_border: Hsla,

    /// Indicates some kind of unreachable status, like a block of code that can never be reached.
    pub unreachable: Hsla,

    /// Represents a warning status, like an operation that is about to fail.
    pub warning: Hsla,
    pub warning_background: Hsla,
    pub warning_border: Hsla,
}

pub struct DiagnosticColors {
    pub error: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
}

impl StatusColors {
    pub const fn diagnostic(&self) -> DiagnosticColors {
        DiagnosticColors { error: self.error, warning: self.warning, info: self.info }
    }
}
