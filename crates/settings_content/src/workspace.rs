use serde::Deserialize;
use serde::Serialize;

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
