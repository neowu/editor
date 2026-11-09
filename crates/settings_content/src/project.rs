use gpui::Rgba;
use serde::Deserialize;
use serde::Serialize;

/// When to scan content of linked directories.
#[derive(
    Copy, Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ScanSymlinksSetting {
    /// Always scan symlinked directories
    Always,
    /// Only scan symlinked directories when they've been expanded in the workspace
    #[default]
    Expanded,
}

/// Custom rules for rendering LSP semantic tokens.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct SemanticTokenRules {
    pub rules: Vec<SemanticTokenRule>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SemanticTokenRule {
    pub token_type: Option<String>,
    #[serde(default)]
    pub token_modifiers: Vec<String>,
    #[serde(default)]
    pub style: Vec<String>,
    pub foreground_color: Option<Rgba>,
    pub background_color: Option<Rgba>,
    pub underline: Option<SemanticTokenColorOverride>,
    pub strikethrough: Option<SemanticTokenColorOverride>,
    pub font_weight: Option<SemanticTokenFontWeight>,
    pub font_style: Option<SemanticTokenFontStyle>,
}

impl SemanticTokenRule {
    pub const fn no_style_defined(&self) -> bool {
        self.style.is_empty()
            && self.foreground_color.is_none()
            && self.background_color.is_none()
            && self.underline.is_none()
            && self.strikethrough.is_none()
            && self.font_weight.is_none()
            && self.font_style.is_none()
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SemanticTokenColorOverride {
    InheritForeground(bool),
    Replace(Rgba),
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTokenFontWeight {
    #[default]
    Normal,
    Bold,
}

#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTokenFontStyle {
    #[default]
    Normal,
    Italic,
}
