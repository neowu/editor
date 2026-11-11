use std::fmt::Display;
mod built_in_keymap;
mod keymap;

use std::fmt;

#[allow(clippy::wildcard_imports)]
pub use ::settings_content::*;
pub use built_in_keymap::keymap as built_in_keymap;
pub use keymap::KeyBindingSpec;
pub use keymap::built_in_key_bindings;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash, PartialOrd, Ord, serde::Serialize)]
pub struct WorktreeId(usize);

impl From<WorktreeId> for usize {
    fn from(value: WorktreeId) -> Self {
        value.0
    }
}

impl WorktreeId {
    pub const fn from_usize(handle_id: usize) -> Self {
        Self(handle_id)
    }

    pub const fn to_usize(self) -> usize {
        self.0
    }
}

impl fmt::Display for WorktreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}
