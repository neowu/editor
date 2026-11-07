use std::fmt::Debug;

#[allow(clippy::wildcard_imports)]
pub use coordinates::*;
mod coordinates;
#[allow(clippy::wildcard_imports)]
pub use table_cell::*;
mod table_cell;
#[allow(clippy::wildcard_imports)]
pub use table_like_content::*;
mod table_like_content;

/// Line number information for table rows
#[derive(Debug, Clone, Copy)]
pub enum LineNumber {
    /// Single line row
    Line(usize),
    /// Multi-line row spanning from start to end line. Incluisive
    LineRange(usize, usize),
}
