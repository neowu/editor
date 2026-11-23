use std::time::Duration;

use gpui::SharedString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    Critical,
    Error,
    Warning,
}

impl ErrorSeverity {
    pub const fn auto_dismiss_delay(&self) -> Option<Duration> {
        match self {
            ErrorSeverity::Critical => None,
            ErrorSeverity::Error => Some(Duration::from_secs(30)),
            ErrorSeverity::Warning => Some(Duration::from_secs(20)),
        }
    }
}

pub trait WorkspaceError {
    fn primary_message(&self) -> SharedString;

    fn secondary_message(&self) -> Option<SharedString> {
        None
    }

    fn severity(&self) -> ErrorSeverity;
}

impl WorkspaceError for &'static str {
    fn primary_message(&self) -> SharedString {
        self.to_string().into()
    }

    fn severity(&self) -> ErrorSeverity {
        ErrorSeverity::Critical
    }
}

impl WorkspaceError for String {
    fn primary_message(&self) -> SharedString {
        self.clone().into()
    }

    fn severity(&self) -> ErrorSeverity {
        ErrorSeverity::Critical
    }
}

impl WorkspaceError for anyhow::Error {
    fn primary_message(&self) -> SharedString {
        format!("{self}").into()
    }

    fn severity(&self) -> ErrorSeverity {
        ErrorSeverity::Critical
    }
}
