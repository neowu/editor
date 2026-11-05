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

    pub diagnostics: DiagnosticsSettings,
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

impl From<settings::DiagnosticSeverityContent> for DiagnosticSeverity {
    fn from(severity: settings::DiagnosticSeverityContent) -> Self {
        match severity {
            settings::DiagnosticSeverityContent::Off => DiagnosticSeverity::Off,
            settings::DiagnosticSeverityContent::Error => DiagnosticSeverity::Error,
            settings::DiagnosticSeverityContent::Warning => DiagnosticSeverity::Warning,
            settings::DiagnosticSeverityContent::Info => DiagnosticSeverity::Info,
            settings::DiagnosticSeverityContent::Hint => DiagnosticSeverity::Hint,
            settings::DiagnosticSeverityContent::All => DiagnosticSeverity::Hint,
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

#[derive(Clone, Debug)]
pub struct DiagnosticsSettings {
    /// Whether to show the project diagnostics button in the status bar.
    pub button: bool,

    /// Settings for using LSP pull diagnostics mechanism in ed.
    pub lsp_pull_diagnostics: LspPullDiagnosticsSettings,

    /// Settings for showing inline diagnostics.
    pub inline: InlineDiagnosticsSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InlineDiagnosticsSettings {
    /// Whether to only show the inline diagnostics after a delay after the
    /// last editor event.
    ///
    /// Default: 150
    pub update_debounce_ms: u64,
    /// The amount of padding between the end of the source line and the start
    /// of the inline diagnostic in units of columns.
    ///
    /// Default: 4
    pub padding: u32,
    /// The minimum column to display inline diagnostics. This setting can be
    /// used to horizontally align inline diagnostics at some position. Lines
    /// longer than this value will still push diagnostics further to the right.
    ///
    /// Default: 0
    pub min_column: u32,

    pub max_severity: Option<DiagnosticSeverity>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspPullDiagnosticsSettings {
    /// Whether to pull for diagnostics or not.
    ///
    /// Default: true
    pub enabled: bool,
    /// Minimum time to wait before pulling diagnostics from the language server(s).
    /// 0 turns the debounce off.
    ///
    /// Default: 50
    pub debounce_ms: u64,
}

impl Settings for ProjectSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let project = &content.project.clone();
        let diagnostics = content.diagnostics.as_ref().unwrap();
        let lsp_pull_diagnostics = diagnostics.lsp_pull_diagnostics.as_ref().unwrap();
        let inline_diagnostics = diagnostics.inline.as_ref().unwrap();

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
            diagnostics: DiagnosticsSettings {
                button: diagnostics.button.unwrap(),
                lsp_pull_diagnostics: LspPullDiagnosticsSettings {
                    enabled: lsp_pull_diagnostics.enabled.unwrap(),
                    debounce_ms: lsp_pull_diagnostics.debounce_ms.unwrap().0,
                },
                inline: InlineDiagnosticsSettings {
                    update_debounce_ms: inline_diagnostics.update_debounce_ms.unwrap().0,
                    padding: inline_diagnostics.padding.unwrap(),
                    min_column: inline_diagnostics.min_column.unwrap(),
                    max_severity: inline_diagnostics.max_severity.map(Into::into),
                },
            },
        }
    }
}
