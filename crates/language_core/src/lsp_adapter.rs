use gpui_shared_string::SharedString;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LanguageServerStatusUpdate {
    Binary(BinaryStatus),
    Health(ServerHealth, Option<SharedString>),
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ServerHealth {
    Ok,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryStatus {
    None,
    Starting,
    Stopping,
    Stopped,
    /// The server's binary wasn't found; not an error, since servers are installed by the user.
    NotInstalled {
        message: String,
    },
    Failed {
        error: String,
    },
}
