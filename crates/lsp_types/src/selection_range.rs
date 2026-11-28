use serde::Deserialize;
use serde::Serialize;

use crate::StaticTextDocumentRegistrationOptions;
use crate::WorkDoneProgressOptions;
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRangeClientCapabilities {
    /// Whether implementation supports dynamic registration for selection range
    /// providers. If this is set to `true` the client supports the new
    /// `SelectionRangeRegistrationOptions` return value for the corresponding
    /// server capability as well.
    #[serde(skip_serializing_if = "Option::is_none")]
    dynamic_registration: Option<bool>,
}

#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
pub struct SelectionRangeOptions {
    #[serde(flatten)]
    work_done_progress_options: WorkDoneProgressOptions,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct SelectionRangeRegistrationOptions {
    #[serde(flatten)]
    selection_range_options: SelectionRangeOptions,

    #[serde(flatten)]
    registration_options: StaticTextDocumentRegistrationOptions,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SelectionRangeProviderCapability {
    Simple(bool),
    Options(SelectionRangeOptions),
    RegistrationOptions(SelectionRangeRegistrationOptions),
}

impl From<SelectionRangeRegistrationOptions> for SelectionRangeProviderCapability {
    fn from(from: SelectionRangeRegistrationOptions) -> Self {
        Self::RegistrationOptions(from)
    }
}

impl From<SelectionRangeOptions> for SelectionRangeProviderCapability {
    fn from(from: SelectionRangeOptions) -> Self {
        Self::Options(from)
    }
}

impl From<bool> for SelectionRangeProviderCapability {
    fn from(from: bool) -> Self {
        Self::Simple(from)
    }
}
