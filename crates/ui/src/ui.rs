//! # UI – ed UI Primitives & Components
//!
//! This crate provides a set of UI primitives and components that are used to build all of the elements in ed's UI.
//!
//! ## Related Crates:
//!
//! - `ui_input` - the single line input component

mod components;
pub mod font;
pub mod prelude;
mod styles;
mod traits;
pub mod utils;

#[allow(clippy::wildcard_imports)]
pub use components::*;
#[allow(clippy::wildcard_imports)]
pub use prelude::*;
#[allow(clippy::wildcard_imports)]
pub use styles::*;
#[allow(clippy::wildcard_imports)]
pub use traits::animation_ext::*;
