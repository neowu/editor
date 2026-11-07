pub use proto;
pub use proto::Receipt;
pub use proto::TypedEnvelope;
#[allow(clippy::wildcard_imports)]
pub use proto::error::*;

#[cfg(feature = "gpui")]
mod proto_client;
#[cfg(feature = "gpui")]
#[allow(clippy::wildcard_imports)]
pub use proto_client::*;

/// Identifies a project in the remote editor protocol.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct ProjectId(pub u64);
impl ProjectId {
    pub const fn to_proto(self) -> u64 {
        self.0
    }
}
