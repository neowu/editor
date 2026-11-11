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
