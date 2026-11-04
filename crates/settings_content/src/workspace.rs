use serde::Deserialize;
use serde::Serialize;

use crate::DockSide;
use crate::ShowIndentGuides;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewTabsSettingsContent {
    /// Whether to show opened editors as preview tabs.
    /// Preview tabs do not stay open, are reused until explicitly set to be kept open opened (via double-click or editing) and show file names in italic.
    ///
    /// Default: true
    pub enabled: Option<bool>,
    /// Whether to open tabs in preview mode when opened from the project panel
    /// with a single click or the `project_panel::Open` action.
    ///
    /// Default: true
    pub enable_preview_from_project_panel: Option<bool>,
    /// Whether to open tabs in preview mode when selected from the file finder.
    ///
    /// Default: false
    pub enable_preview_from_file_finder: Option<bool>,
    /// Whether to open tabs in preview mode when opened from a multibuffer.
    ///
    /// Default: true
    pub enable_preview_from_multibuffer: Option<bool>,
    /// Whether to open tabs in preview mode when code navigation is used to open a multibuffer.
    ///
    /// Default: false
    pub enable_preview_multibuffer_from_code_navigation: Option<bool>,
    /// Whether to open tabs in preview mode when code navigation is used to open a single file.
    ///
    /// Default: true
    pub enable_preview_file_from_code_navigation: Option<bool>,
    /// Whether to keep tabs in preview mode when code navigation is used to navigate away from them.
    /// If `enable_preview_file_from_code_navigation` or `enable_preview_multibuffer_from_code_navigation` is also true, the new tab may replace the existing one.
    ///
    /// Default: false
    pub enable_keep_preview_on_code_navigation: Option<bool>,
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ShowDiagnostics {
    #[default]
    Off,
    Errors,
    All,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct StatusBarSettingsContent {
    /// Whether to show the status bar.
    ///
    /// Default: true
    #[serde(rename = "experimental.show")]
    pub show: Option<bool>,
    /// Whether to display the active language button in the status bar.
    ///
    /// Default: true
    pub active_language_button: Option<bool>,
    /// Whether to show the cursor position button in the status bar.
    ///
    /// Default: true
    pub cursor_position_button: Option<bool>,
    /// Whether to show the active encoding button in the status bar.
    ///
    /// Default: non_utf8
    pub active_encoding_button: Option<EncodingDisplayOptions>,
}

#[derive(
    Copy, Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize, strum::VariantNames, strum::VariantArray,
)]
#[serde(rename_all = "snake_case")]
pub enum EncodingDisplayOptions {
    Enabled,
    Disabled,
    #[default]
    NonUtf8,
}
impl EncodingDisplayOptions {
    pub const fn should_show(&self, is_utf8: bool, has_bom: bool) -> bool {
        match self {
            Self::Disabled => false,
            Self::Enabled => true,
            Self::NonUtf8 => {
                let is_standard_utf8 = is_utf8 && !has_bom;
                !is_standard_utf8
            }
        }
    }
}

#[derive(Clone, PartialEq, Default, Serialize, Deserialize, Debug)]
pub struct ProjectPanelAutoOpenSettings {
    /// Whether to automatically open newly created files in the editor.
    ///
    /// Default: true
    pub on_create: Option<bool>,
    /// Whether to automatically open files after pasting or duplicating them.
    ///
    /// Default: true
    pub on_paste: Option<bool>,
    /// Whether to automatically open files dropped from external sources.
    ///
    /// Default: true
    pub on_drop: Option<bool>,
}

#[derive(Clone, PartialEq, Default, Serialize, Deserialize, Debug)]
pub struct ProjectPanelSettingsContent {
    /// Whether to show the project panel button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
    /// Whether to hide gitignore files in the project panel.
    ///
    /// Default: false
    pub hide_gitignore: Option<bool>,
    /// Customize default width (in pixels) taken by project panel
    ///
    /// Default: 240
    pub default_width: Option<crate::PixelSetting>,
    /// The position of project panel
    ///
    /// Default: left
    pub dock: Option<DockSide>,
    // TODO
    pub title_tooltip_delay: Option<ProjectPanelTitleTooltipDelay>,
    /// Spacing between worktree entries in the project panel.
    ///
    /// Default: comfortable
    pub entry_spacing: Option<ProjectPanelEntrySpacing>,
    /// Whether to show file icons in the project panel.
    ///
    /// Default: true
    pub file_icons: Option<bool>,
    /// What to show for directories in the project panel.
    ///
    /// Default: icon
    pub folder_indicator: Option<FolderIndicator>,
    /// Whether to show the git status in the project panel.
    ///
    /// Default: true
    pub git_status: Option<bool>,
    /// Amount of indentation (in pixels) for nested items.
    ///
    /// Default: 20
    pub indent_size: Option<crate::PixelSetting>,
    /// Whether to reveal it in the project panel automatically,
    /// when a corresponding project entry becomes active.
    /// Gitignored entries are never auto revealed.
    ///
    /// Default: true
    pub auto_reveal_entries: Option<bool>,
    /// Whether to fold directories automatically
    /// when directory has only one directory inside.
    ///
    /// Default: true
    pub auto_fold_dirs: Option<bool>,
    /// Whether the project panel should open on startup.
    ///
    /// Default: true
    pub starts_open: Option<bool>,
    /// Which files containing diagnostic errors/warnings to mark in the project panel.
    ///
    /// Default: all
    pub show_diagnostics: Option<ShowDiagnostics>,
    /// Settings related to indent guides in the project panel.
    pub indent_guides: Option<ProjectPanelIndentGuidesSettings>,
    /// Whether to hide the root entry when only one folder is open in the window.
    ///
    /// Default: false
    pub hide_root: Option<bool>,
    /// Whether to hide the hidden entries in the project panel.
    ///
    /// Default: false
    pub hide_hidden: Option<bool>,
    /// Whether to stick parent directories at top of the project panel.
    ///
    /// Default: true
    pub sticky_scroll: Option<bool>,
    /// Whether to enable drag-and-drop operations in the project panel.
    ///
    /// Default: true
    pub drag_and_drop: Option<bool>,
    /// Settings for automatically opening files.
    pub auto_open: Option<ProjectPanelAutoOpenSettings>,
    /// How to order sibling entries in the project panel.
    ///
    /// Default: directories_first
    pub sort_mode: Option<ProjectPanelSortMode>,
    /// Whether to sort file and folder names case-sensitively in the project panel.
    /// This works in combination with `sort_mode`. `sort_mode` controls how files and
    /// directories are grouped, while this setting controls how names are compared.
    ///
    /// Default: default
    pub sort_order: Option<ProjectPanelSortOrder>,
    /// Whether to show error and warning count badges next to file names in the project panel.
    ///
    /// Default: false
    pub diagnostic_badges: Option<bool>,
    /// Whether to show a git status indicator next to file names in the project panel.
    ///
    /// Default: false
    pub git_status_indicator: Option<bool>,
}

/// Controls the width of the git diff hunk indicators in the gutter.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, strum::EnumDiscriminants)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelTitleTooltipDelay {
    /// Default is 1500ms.
    #[default]
    Default,
    /// A custom offset in milliseconds for the tooltip show delay.
    Custom(crate::DelayMs),
    /// Disables the tooltip
    Disabled,
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelEntrySpacing {
    /// Comfortable spacing of entries.
    #[default]
    Comfortable,
    /// The standard spacing of entries.
    Standard,
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum FolderIndicator {
    /// Show a folder icon.
    #[default]
    Icon,
    /// Show a disclosure chevron.
    Chevron,
    /// Show a disclosure chevron followed by a folder icon.
    Both,
}

impl FolderIndicator {
    pub const fn shows_chevron(self) -> bool {
        matches!(self, Self::Chevron | Self::Both)
    }

    pub const fn shows_icon(self) -> bool {
        matches!(self, Self::Icon | Self::Both)
    }
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelSortMode {
    /// Show directories first, then files
    #[default]
    DirectoriesFirst,
    /// Mix directories and files together
    Mixed,
    /// Show files first, then directories
    FilesFirst,
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelSortOrder {
    /// Case-insensitive natural sort with lowercase preferred in ties.
    /// Numbers in file names are compared by value (e.g., `file2` before `file10`).
    #[default]
    Default,
    /// Uppercase names are grouped before lowercase names, with case-insensitive
    /// natural sort within each group. Dot-prefixed names sort before both groups.
    Upper,
    /// Lowercase names are grouped before uppercase names, with case-insensitive
    /// natural sort within each group. Dot-prefixed names sort before both groups.
    Lower,
    /// Pure Unicode codepoint comparison. No case folding, no natural number sorting.
    /// Uppercase ASCII sorts before lowercase. Accented characters sort after ASCII.
    Unicode,
}

impl From<ProjectPanelSortMode> for util::paths::SortMode {
    fn from(mode: ProjectPanelSortMode) -> Self {
        match mode {
            ProjectPanelSortMode::DirectoriesFirst => Self::DirectoriesFirst,
            ProjectPanelSortMode::Mixed => Self::Mixed,
            ProjectPanelSortMode::FilesFirst => Self::FilesFirst,
        }
    }
}

impl From<ProjectPanelSortOrder> for util::paths::SortOrder {
    fn from(order: ProjectPanelSortOrder) -> Self {
        match order {
            ProjectPanelSortOrder::Default => Self::Default,
            ProjectPanelSortOrder::Upper => Self::Upper,
            ProjectPanelSortOrder::Lower => Self::Lower,
            ProjectPanelSortOrder::Unicode => Self::Unicode,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectPanelIndentGuidesSettings {
    pub show: Option<ShowIndentGuides>,
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, Default, Serialize, Deserialize, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFoldingRanges {
    /// Do not request folding ranges from language servers; use tree-sitter and indent-based folding.
    #[default]
    Off,
    /// Use LSP folding wherever possible, falling back to tree-sitter and indent-based folding when no results were returned by the server.
    On,
}

impl DocumentFoldingRanges {
    /// Returns true if LSP folding ranges should be requested from language servers.
    pub fn enabled(&self) -> bool {
        self != &Self::Off
    }
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, Default, Serialize, Deserialize, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSymbols {
    /// Use tree-sitter queries to compute document symbols for outlines and breadcrumbs (default).
    #[default]
    #[serde(alias = "tree_sitter")]
    Off,
    /// Use the language server's `textDocument/documentSymbol` LSP response for outlines and
    /// breadcrumbs. When enabled, tree-sitter is not used for document symbols.
    #[serde(alias = "language_server")]
    On,
}

impl DocumentSymbols {
    /// Returns true if LSP document symbols should be used instead of tree-sitter.
    pub fn lsp_enabled(&self) -> bool {
        self == &Self::On
    }
}
