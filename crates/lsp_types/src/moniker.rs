use serde::Deserialize;
use serde::Serialize;

use crate::DynamicRegistrationClientCapabilities;
use crate::TextDocumentRegistrationOptions;
use crate::WorkDoneProgressOptions;

pub type MonikerClientCapabilities = DynamicRegistrationClientCapabilities;

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MonikerServerCapabilities {
    Options(MonikerOptions),
    RegistrationOptions(MonikerRegistrationOptions),
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
pub struct MonikerOptions {
    #[serde(flatten)]
    work_done_progress_options: WorkDoneProgressOptions,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonikerRegistrationOptions {
    #[serde(flatten)]
    text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    moniker_options: MonikerOptions,
}
