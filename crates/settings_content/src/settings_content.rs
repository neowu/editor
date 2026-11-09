mod editor;
mod language;
mod project;
mod serde_helper;
mod terminal;
mod workspace;

#[allow(clippy::wildcard_imports)]
pub use editor::*;
#[allow(clippy::wildcard_imports)]
pub use language::*;
#[allow(clippy::wildcard_imports)]
pub use project::*;
use serde::Deserialize;
use serde::Serialize;
pub use serde_helper::serialize_f32_with_two_decimal_places;
#[allow(clippy::wildcard_imports)]
pub use terminal::*;
#[allow(clippy::wildcard_imports)]
pub use workspace::*;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SshConnection {
    pub host: String,
    pub username: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub projects: std::collections::BTreeSet<RemoteProject>,
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
