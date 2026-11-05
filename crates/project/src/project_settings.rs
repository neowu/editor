use std::time::Duration;

use collections::HashMap;
use lsp::DEFAULT_LSP_REQUEST_TIMEOUT_SECS;
use lsp::LanguageServerName;
use serde::Deserialize;
use serde::Serialize;
pub use settings::BinarySettings;
pub use settings::LspSettings;
use settings::SemanticTokenRules;
use settings::Settings;

#[derive(Debug, Clone)]
pub struct ProjectSettings {
    /// Configuration for language servers.
    ///
    /// The following settings can be overridden for specific language servers:
    /// - initialization_options
    ///
    /// To override settings for a language, add an entry for that language server's
    /// name to the lsp value.
    /// Default: null
    // todo(settings-follow-up)
    // We should change to use a non content type (settings::LspSettings is a content type)
    // Note: Will either require merging with defaults, which also requires deciding where the defaults come from,
    //       or case by case deciding which fields are optional and which are actually required.
    pub lsp: HashMap<LanguageServerName, settings::LspSettings>,

    /// Common language server settings.
    pub global_lsp_settings: GlobalLspSettings,
}

settings::register_setting!(ProjectSettings);

/// Common language server settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalLspSettings {
    /// Whether to show the LSP servers button in the status bar.
    ///
    /// Default: `true`
    pub button: bool,
    /// The maximum amount of time to wait for responses from language servers, in seconds.
    /// A value of `0` will result in no timeout being applied (causing all LSP responses to wait
    /// indefinitely until completed).
    /// This should not be used outside of serialization/de-serialization in favor of get_request_timeout.
    ///
    /// Default: `120`
    pub request_timeout: u64,
    /// The maximum line length a buffer may contain before language server features are disabled for the entire buffer.
    ///
    /// Default: `20000`
    pub max_buffer_line_length: u32,
    pub notifications: LspNotificationSettings,

    /// Rules for highlighting semantic tokens.
    pub semantic_token_rules: SemanticTokenRules,
}

impl Default for GlobalLspSettings {
    fn default() -> Self {
        Self {
            button: true,
            request_timeout: DEFAULT_LSP_REQUEST_TIMEOUT_SECS,
            max_buffer_line_length: 20_000,
            notifications: LspNotificationSettings::default(),
            semantic_token_rules: SemanticTokenRules::default(),
        }
    }
}

impl GlobalLspSettings {
    /// Returns the timeout duration for LSP-related interactions, or Duration::ZERO if no timeout should be applied.
    /// Zero durations are treated as no timeout by language servers, so code using this in an async context can
    /// simply call unwrap_or_default.
    pub const fn get_request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout)
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "source", rename_all = "snake_case")]
pub struct LspNotificationSettings {
    /// Timeout in milliseconds for dismissing language server notifications.
    /// Zero disables automatic dismissal.
    pub dismiss_timeout_ms: Option<u64>,
}

impl Default for LspNotificationSettings {
    fn default() -> Self {
        Self { dismiss_timeout_ms: Some(5000) }
    }
}

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
    pub const fn into_lsp(self) -> Option<lsp::DiagnosticSeverity> {
        match self {
            DiagnosticSeverity::Off => None,
            DiagnosticSeverity::Error => Some(lsp::DiagnosticSeverity::ERROR),
            DiagnosticSeverity::Warning => Some(lsp::DiagnosticSeverity::WARNING),
            DiagnosticSeverity::Info => Some(lsp::DiagnosticSeverity::INFORMATION),
            DiagnosticSeverity::Hint => Some(lsp::DiagnosticSeverity::HINT),
        }
    }
}

/// Determines the severity of the diagnostic that should be moved to.
#[derive(PartialEq, PartialOrd, Clone, Copy, Debug, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoToDiagnosticSeverity {
    /// Errors
    Error = 3,
    /// Warnings
    Warning = 2,
    /// Information
    Information = 1,
    /// Hints
    Hint = 0,
}

impl From<lsp::DiagnosticSeverity> for GoToDiagnosticSeverity {
    fn from(severity: lsp::DiagnosticSeverity) -> Self {
        match severity {
            lsp::DiagnosticSeverity::ERROR => Self::Error,
            lsp::DiagnosticSeverity::WARNING => Self::Warning,
            lsp::DiagnosticSeverity::INFORMATION => Self::Information,
            lsp::DiagnosticSeverity::HINT => Self::Hint,
            _ => Self::Error,
        }
    }
}

impl GoToDiagnosticSeverity {
    pub const fn min() -> Self {
        Self::Hint
    }

    pub const fn max() -> Self {
        Self::Error
    }
}

/// Allows filtering diagnostics that should be moved to.
#[derive(PartialEq, Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
pub enum GoToDiagnosticSeverityFilter {
    /// Move to diagnostics of a specific severity.
    Only(GoToDiagnosticSeverity),

    /// Specify a range of severities to include.
    Range {
        /// Minimum severity to move to. Defaults no "error".
        #[serde(default = "GoToDiagnosticSeverity::min")]
        min: GoToDiagnosticSeverity,
        /// Maximum severity to move to. Defaults to "hint".
        #[serde(default = "GoToDiagnosticSeverity::max")]
        max: GoToDiagnosticSeverity,
    },
}

impl Default for GoToDiagnosticSeverityFilter {
    fn default() -> Self {
        Self::Range { min: GoToDiagnosticSeverity::min(), max: GoToDiagnosticSeverity::max() }
    }
}

impl GoToDiagnosticSeverityFilter {
    pub fn matches(&self, severity: lsp::DiagnosticSeverity) -> bool {
        let severity: GoToDiagnosticSeverity = severity.into();
        match self {
            Self::Only(target) => *target == severity,
            Self::Range { min, max } => severity >= *min && severity <= *max,
        }
    }
}

impl Settings for ProjectSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let project = &content.project.clone();

        Self {
            lsp: project.lsp.clone().into_iter().map(|(key, value)| (LanguageServerName(key.into()), value)).collect(),
            global_lsp_settings: GlobalLspSettings {
                button: content.global_lsp_settings.as_ref().unwrap().button.unwrap(),
                request_timeout: content.global_lsp_settings.as_ref().unwrap().request_timeout.unwrap(),
                max_buffer_line_length: content.global_lsp_settings.as_ref().unwrap().max_buffer_line_length.unwrap(),
                notifications: LspNotificationSettings {
                    dismiss_timeout_ms: content
                        .global_lsp_settings
                        .as_ref()
                        .unwrap()
                        .notifications
                        .as_ref()
                        .unwrap()
                        .dismiss_timeout_ms,
                },
                semantic_token_rules: content
                    .global_lsp_settings
                    .as_ref()
                    .unwrap()
                    .semantic_token_rules
                    .as_ref()
                    .unwrap()
                    .clone(),
            },
        }
    }
}
