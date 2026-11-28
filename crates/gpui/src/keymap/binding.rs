use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::rc::Rc;

use smallvec::SmallVec;

use crate::Action;
use crate::AsKeystroke;
use crate::InvalidKeystrokeError;
use crate::KeyBindingContextPredicate;
use crate::KeybindingKeystroke;
use crate::Keystroke;

/// A keybinding and its associated metadata, from the keymap.
pub struct KeyBinding {
    pub(crate) action: Box<dyn Action>,
    pub(crate) keystrokes: SmallVec<[KeybindingKeystroke; 2]>,
    pub(crate) context_predicate: Option<Rc<KeyBindingContextPredicate>>,
    pub(crate) meta: Option<KeyBindingMetaIndex>,
}

impl Clone for KeyBinding {
    fn clone(&self) -> Self {
        KeyBinding {
            action: self.action.boxed_clone(),
            keystrokes: self.keystrokes.clone(),
            context_predicate: self.context_predicate.clone(),
            meta: self.meta,
        }
    }
}

impl KeyBinding {
    /// Construct a new keybinding from the given data. Panics on parse error.
    pub fn new<A: Action>(keystrokes: &str, action: A, context: Option<&str>) -> Self {
        let context_predicate = context
            .map(|context| KeyBindingContextPredicate::parse(context).expect("invalid keybinding context").into());
        Self::load(keystrokes, Box::new(action), context_predicate).expect("invalid keybinding keystrokes")
    }

    /// Load a keybinding from the given raw data.
    pub fn load(
        keystrokes: &str,
        action: Box<dyn Action>,
        context_predicate: Option<Rc<KeyBindingContextPredicate>>,
    ) -> Result<Self, InvalidKeystrokeError> {
        let keystrokes: SmallVec<[KeybindingKeystroke; 2]> = keystrokes
            .split_whitespace()
            .map(|source| Ok(KeybindingKeystroke::from_keystroke(Keystroke::parse(source)?)))
            .collect::<Result<_, _>>()?;

        Ok(Self { keystrokes, action, context_predicate, meta: None })
    }

    /// Set the metadata for this binding.
    pub const fn with_meta(mut self, meta: KeyBindingMetaIndex) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Check if the given keystrokes match this binding.
    pub(crate) fn match_keystrokes(&self, typed: &[impl AsKeystroke]) -> Option<bool> {
        if self.keystrokes.len() < typed.len() {
            return None;
        }

        for (target, typed) in self.keystrokes.iter().zip(typed.iter()) {
            if !typed.as_keystroke().should_match(target) {
                return None;
            }
        }

        Some(self.keystrokes.len() > typed.len())
    }

    /// Get the keystrokes associated with this binding.
    pub fn keystrokes(&self) -> &[KeybindingKeystroke] {
        self.keystrokes.as_slice()
    }

    /// Get the action associated with this binding.
    pub fn action(&self) -> &dyn Action {
        self.action.as_ref()
    }

    /// Get the predicate used to match this binding.
    pub fn predicate(&self) -> Option<Rc<KeyBindingContextPredicate>> {
        self.context_predicate.as_ref().map(Clone::clone)
    }

    /// Get the metadata for this binding.
    pub const fn meta(&self) -> Option<KeyBindingMetaIndex> {
        self.meta
    }
}

impl Debug for KeyBinding {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyBinding")
            .field("keystrokes", &self.keystrokes)
            .field("context_predicate", &self.context_predicate)
            .field("action", &self.action.name())
            .finish()
    }
}

/// A unique identifier for retrieval of metadata associated with a key binding.
/// Intended to be used as an index or key into a user-defined store of metadata
/// associated with the binding, such as the source of the binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBindingMetaIndex(pub(crate) u32);

#[cfg(test)]
mod tests {
    use super::*;

    actions!(binding_test, [First]);

    #[test]
    fn load_parses_each_keystroke_unmapped() {
        let binding = KeyBinding::load("cmd-k  ctrl-]", Box::new(First), None).unwrap();
        let keystrokes: Vec<_> = binding.keystrokes().iter().map(|k| k.inner().clone()).collect();
        assert_eq!(keystrokes, [Keystroke::parse("cmd-k").unwrap(), Keystroke::parse("ctrl-]").unwrap()]);
        assert!(binding.action().partial_eq(&First));
    }

    #[test]
    fn load_rejects_invalid_keystrokes() {
        KeyBinding::load("cmd-k a-b", Box::new(First), None).unwrap_err();
    }
}
