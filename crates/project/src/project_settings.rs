#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DiagnosticSeverity {
    // No diagnostics are shown.
    Off,
    Error,
    Warning,
    Info,
    Hint,
}

impl DiagnosticSeverity {
    pub const fn into_lsp(self) -> Option<lsp_types::DiagnosticSeverity> {
        match self {
            DiagnosticSeverity::Off => None,
            DiagnosticSeverity::Error => Some(lsp_types::DiagnosticSeverity::ERROR),
            DiagnosticSeverity::Warning => Some(lsp_types::DiagnosticSeverity::WARNING),
            DiagnosticSeverity::Info => Some(lsp_types::DiagnosticSeverity::INFORMATION),
            DiagnosticSeverity::Hint => Some(lsp_types::DiagnosticSeverity::HINT),
        }
    }
}

/// Determines the severity of the diagnostic that should be moved to.
#[derive(PartialEq, PartialOrd, Clone, Copy, Debug, Eq)]
pub enum GoToDiagnosticSeverity {
    /// Errors.
    Error = 3,
    /// Warnings.
    Warning = 2,
    /// Information.
    Information = 1,
    /// Hints.
    Hint = 0,
}

impl From<lsp_types::DiagnosticSeverity> for GoToDiagnosticSeverity {
    fn from(severity: lsp_types::DiagnosticSeverity) -> Self {
        match severity {
            lsp_types::DiagnosticSeverity::ERROR => Self::Error,
            lsp_types::DiagnosticSeverity::WARNING => Self::Warning,
            lsp_types::DiagnosticSeverity::INFORMATION => Self::Information,
            lsp_types::DiagnosticSeverity::HINT => Self::Hint,
            _ => Self::Error,
        }
    }
}

impl GoToDiagnosticSeverity {
    const fn min() -> Self {
        Self::Hint
    }

    const fn max() -> Self {
        Self::Error
    }
}

/// Allows filtering diagnostics that should be moved to.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum GoToDiagnosticSeverityFilter {
    /// Move to diagnostics of a specific severity.
    Only(GoToDiagnosticSeverity),

    /// Specify a range of severities to include.
    Range {
        /// Minimum severity to move to. Defaults no "error".
        min: GoToDiagnosticSeverity,
        /// Maximum severity to move to. Defaults to "hint".
        max: GoToDiagnosticSeverity,
    },
}

impl Default for GoToDiagnosticSeverityFilter {
    fn default() -> Self {
        Self::Range { min: GoToDiagnosticSeverity::min(), max: GoToDiagnosticSeverity::max() }
    }
}

impl GoToDiagnosticSeverityFilter {
    pub fn matches(&self, severity: lsp_types::DiagnosticSeverity) -> bool {
        let severity: GoToDiagnosticSeverity = severity.into();
        match self {
            Self::Only(target) => *target == severity,
            Self::Range { min, max } => severity >= *min && severity <= *max,
        }
    }
}
