use serde::Deserialize;
use serde::Serialize;

use crate::DocumentSelector;
use crate::DynamicRegistrationClientCapabilities;
use crate::PartialResultParams;
use crate::Range;
use crate::TextDocumentIdentifier;
use crate::WorkDoneProgressParams;

pub type DocumentColorClientCapabilities = DynamicRegistrationClientCapabilities;

// Braces matter: a unit struct (de)serializes as `null`, not the `{}` LSP requires.
#[allow(clippy::empty_structs_with_brackets)]
#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorProviderOptions {}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticTextDocumentColorProviderOptions {
    /// A document selector to identify the scope of the registration. If set to null
    /// the document selector provided on the client side will be used.
    document_selector: Option<DocumentSelector>,

    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ColorProviderCapability {
    Simple(bool),
    ColorProvider(ColorProviderOptions),
    Options(StaticTextDocumentColorProviderOptions),
}

impl From<ColorProviderOptions> for ColorProviderCapability {
    fn from(from: ColorProviderOptions) -> Self {
        Self::ColorProvider(from)
    }
}

impl From<StaticTextDocumentColorProviderOptions> for ColorProviderCapability {
    fn from(from: StaticTextDocumentColorProviderOptions) -> Self {
        Self::Options(from)
    }
}

impl From<bool> for ColorProviderCapability {
    fn from(from: bool) -> Self {
        Self::Simple(from)
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentColorParams {
    /// The text document.
    pub text_document: TextDocumentIdentifier,

    #[serde(flatten)]
    pub work_done_progress_params: WorkDoneProgressParams,

    #[serde(flatten)]
    pub partial_result_params: PartialResultParams,
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorInformation {
    /// The range in the document where this color appears.
    pub range: Range,
    /// The actual color value for this color range.
    pub color: Color,
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize, Copy)]
#[serde(rename_all = "camelCase")]
pub struct Color {
    /// The red component of this color in the range [0-1].
    pub red: f32,
    /// The green component of this color in the range [0-1].
    pub green: f32,
    /// The blue component of this color in the range [0-1].
    pub blue: f32,
    /// The alpha component of this color in the range [0-1].
    pub alpha: f32,
}
