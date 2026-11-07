mod highlighted_label;
#[allow(clippy::module_inception)]
mod label;
mod label_like;
mod loading_label;
mod spinner_label;

#[allow(clippy::wildcard_imports)]
pub use highlighted_label::*;
#[allow(clippy::wildcard_imports)]
pub use label::*;
#[allow(clippy::wildcard_imports)]
pub use label_like::*;
#[allow(clippy::wildcard_imports)]
pub use loading_label::*;
#[allow(clippy::wildcard_imports)]
pub use spinner_label::*;
