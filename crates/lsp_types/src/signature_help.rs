use serde::Deserialize;
use serde::Serialize;

use crate::MarkupKind;
use crate::WorkDoneProgressOptions;

#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignatureInformationSettings {
    /// Client supports the follow content formats for the documentation
    /// property. The order describes the preferred format of the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    documentation_format: Option<Vec<MarkupKind>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    parameter_information: Option<ParameterInformationSettings>,

    /// The client support the `activeParameter` property on `SignatureInformation`
    /// literal.
    ///
    /// @since 3.16.0.
    #[serde(skip_serializing_if = "Option::is_none")]
    active_parameter_support: Option<bool>,
}

#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ParameterInformationSettings {
    /// The client supports processing label offsets instead of a
    /// simple label string.
    ///
    /// @since 3.14.0.
    #[serde(skip_serializing_if = "Option::is_none")]
    label_offset_support: Option<bool>,
}

#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpClientCapabilities {
    /// Whether completion supports dynamic registration.
    #[serde(skip_serializing_if = "Option::is_none")]
    dynamic_registration: Option<bool>,

    /// The client supports the following `SignatureInformation`
    /// specific properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    signature_information: Option<SignatureInformationSettings>,

    /// The client supports to send additional context information for a
    /// `textDocument/signatureHelp` request. A client that opts into
    /// contextSupport will also support the `retriggerCharacters` on
    /// `SignatureHelpOptions`.
    ///
    /// @since 3.15.0.
    #[serde(skip_serializing_if = "Option::is_none")]
    context_support: Option<bool>,
}

/// Signature help options.
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpOptions {
    /// The characters that trigger signature help automatically.
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger_characters: Option<Vec<String>>,

    /// List of characters that re-trigger signature help.
    /// These trigger characters are only active when signature help is already showing. All trigger characters
    /// are also counted as re-trigger characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    retrigger_characters: Option<Vec<String>>,

    #[serde(flatten)]
    work_done_progress_options: WorkDoneProgressOptions,
}
