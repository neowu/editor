use serde::Deserialize;
use serde::Serialize;

use crate::DynamicRegistrationClientCapabilities;
use crate::StaticRegistrationOptions;
use crate::TextDocumentRegistrationOptions;
use crate::WorkDoneProgressOptions;

pub type InlineValueClientCapabilities = DynamicRegistrationClientCapabilities;

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum InlineValueServerCapabilities {
    Options(InlineValueOptions),
    RegistrationOptions(InlineValueRegistrationOptions),
}

/// Inline value options used during static registration.
///
/// @since 3.17.0.
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
pub struct InlineValueOptions {
    #[serde(flatten)]
    work_done_progress_options: WorkDoneProgressOptions,
}

/// Inline value options used during static or dynamic registration.
///
/// @since 3.17.0.
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
pub struct InlineValueRegistrationOptions {
    #[serde(flatten)]
    inline_value_options: InlineValueOptions,

    #[serde(flatten)]
    text_document_registration_options: TextDocumentRegistrationOptions,

    #[serde(flatten)]
    static_registration_options: StaticRegistrationOptions,
}

/// Client workspace capabilities specific to inline values.
#[derive(Debug, Eq, PartialEq, Clone, Default, Deserialize, Serialize)]
///
/// @since 3.17.0.
#[serde(rename_all = "camelCase")]
pub struct InlineValueWorkspaceClientCapabilities {
    /// Whether the client implementation supports a refresh request sent from
    /// the server to the client.
    ///
    /// Note that this event is global and will force the client to refresh all
    /// inline values currently shown. It should be used with absolute care and
    /// is useful for situation where a server for example detect a project wide
    /// change that requires such a calculation.
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_support: Option<bool>,
}
