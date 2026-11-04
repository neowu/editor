mod editor;
mod font;
mod language;
mod project;
mod serde_helper;
mod terminal;
mod workspace;

use anyhow::Context;
use collections::HashMap;
pub use editor::*;
pub use font::*;
pub use language::*;
pub use project::*;
use serde::Deserialize;
use serde::Serialize;
pub use serde_helper::serialize_f32_with_two_decimal_places;
pub use serde_helper::serialize_optional_f32_with_two_decimal_places;
pub use terminal::*;
pub use workspace::*;

/// A non-negative size in pixels.
///
/// Valid range: 0.0 and up
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    PartialOrd,
    derive_more::FromStr,
    derive_more::Deref,
    derive_more::From,
)]
#[serde(transparent)]
pub struct PixelSetting(#[serde(serialize_with = "crate::serialize_f32_with_two_decimal_places")] pub f32);

impl std::fmt::Display for PixelSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rounded = (self.0 * 100.0).round() / 100.0;
        write!(f, "{rounded}")
    }
}

use std::hash::Hash;

#[derive(Debug, PartialEq, Default, Clone, Serialize, Deserialize)]
pub struct SettingsContent {
    #[serde(flatten)]
    pub project: ProjectSettingsContent,

    /// Settings related to the file finder.
    pub file_finder: Option<FileFinderSettingsContent>,

    pub tab_switcher: Option<TabSwitcherSettingsContent>,

    /// Configuration for Diagnostics-related features.
    pub diagnostics: Option<DiagnosticsSettingsContent>,

    /// Configuration for Git-related features
    pub git: Option<GitSettings>,

    /// Common language server settings.
    pub global_lsp_settings: Option<GlobalLspSettingsContent>,

    /// The settings for the image viewer.
    pub image_viewer: Option<ImageViewerSettingsContent>,

    /// Configuration of the terminal in ed.
    pub terminal: Option<TerminalSettingsContent>,
}

impl SettingsContent {
    pub const fn languages_mut(&mut self) -> &mut HashMap<String, LanguageSettingsContent> {
        &mut self.project.all_languages.languages.0
    }
}

pub struct LanguageDefaultsContent {
    pub all_languages: AllLanguageSettingsContent,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct FileFinderSettingsContent {
    /// Whether to show file icons in the file finder.
    ///
    /// Default: true
    pub file_icons: Option<bool>,
    /// Determines how much space the file finder can take up in relation to the available window width.
    ///
    /// Default: small
    pub modal_max_width: Option<ModalWidthContent>,
    /// Determines whether the file finder should skip focus for the active file in search results.
    ///
    /// Default: true
    pub skip_focus_for_active_in_search: Option<bool>,
    /// Whether to use gitignored files when searching.
    /// Only the file ed had indexed will be used, not necessary all the gitignored files.
    ///
    /// Default: Smart
    pub include_ignored: Option<IncludeIgnoredContent>,
    /// Whether to pre-fill the file finder's query with the text selected in the
    /// focused item (e.g. an editor or a terminal) when it is opened.
    ///
    /// Default: true
    pub prefill_query_from_selection: Option<bool>,
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, Default, Serialize, Deserialize, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum IncludeIgnoredContent {
    /// Use all gitignored files
    All,
    /// Use only the files ed had indexed
    Indexed,
    /// Be smart and search for ignored when called from a gitignored worktree
    #[default]
    Smart,
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, Default, Serialize, Deserialize, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "lowercase")]
pub enum ModalWidthContent {
    #[default]
    Small,
    Medium,
    Large,
    XLarge,
    Full,
}

impl ModalWidthContent {
    pub fn to_pixels(self, base_width: gpui::Pixels, window_width: gpui::Pixels) -> gpui::Pixels {
        match self {
            ModalWidthContent::Small => base_width,
            ModalWidthContent::Full => window_width,
            ModalWidthContent::XLarge => (window_width - gpui::px(512.)).max(base_width),
            ModalWidthContent::Large => (window_width - gpui::px(768.)).max(base_width),
            ModalWidthContent::Medium => (window_width - gpui::px(1024.)).max(base_width),
        }
    }
}

/// The settings for the image viewer.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ImageViewerSettingsContent {
    /// The unit to use for displaying image file sizes.
    ///
    /// Default: "binary"
    pub unit: Option<ImageFileSizeUnit>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum ImageFileSizeUnit {
    /// Displays file size in binary units (e.g., KiB, MiB).
    #[default]
    Binary,
    /// Displays file size in decimal units (e.g., KB, MB).
    Decimal,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SshConnection {
    pub host: String,
    pub username: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub projects: collections::BTreeSet<RemoteProject>,
    /// Name to use for this server in UI.
    pub nickname: Option<String>,
    pub port_forwards: Option<Vec<SshPortForwardOption>>,
    /// Timeout in seconds for the SSH connection.
    /// Defaults to 10 seconds if not specified.
    pub connection_timeout: Option<u16>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub struct RemoteProject {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct SshPortForwardOption {
    pub local_host: Option<String>,
    pub local_port: u16,
    pub remote_host: Option<String>,
    pub remote_port: u16,
}

/// Settings for configuring the tab switcher behaviour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TabSwitcherSettingsContent {
    /// When false, files are not shown while cycling through the tab switcher.
    /// Only the confirmed file (when the modifier key is released) is shown and
    /// recorded in the MRU history.
    ///
    /// Default: true
    pub show_file_preview: Option<bool>,
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DelayMs(pub u64);

impl From<u64> for DelayMs {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

impl std::fmt::Display for DelayMs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

impl std::str::FromStr for DelayMs {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .strip_suffix("ms")
            .unwrap_or(s.trim())
            .parse::<u64>()
            .map(DelayMs)
            .with_context(|| format!("failed to parse delay duration: {s}"))
    }
}
