use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;

pub use gpui_macros::Action;
use gpui_util::TypeIdHashBuilder;
pub use no_action::NoAction;
pub(crate) use no_action::Unbind;
pub(crate) use no_action::is_no_action;
pub(crate) use no_action::is_unbind;
use rustc_hash::FxHashMap;

/// Defines and registers unit structs that can be used as actions. For more complex data types, derive `Action`.
///
/// The namespace argument `editor` can also be omitted, though it is required for ed actions.
/// Each name carries its own visibility: `actions!(editor, [pub Copy, pub(crate) Fold, Private])`.
#[macro_export]
macro_rules! actions {
    ($namespace:path, [ $( $(#[$attr:meta])* $vis:vis $name:ident),* $(,)? ]) => {
        $(
            #[derive(::std::clone::Clone, PartialEq, Eq, ::std::default::Default, ::std::fmt::Debug, gpui::Action)]
            #[action(namespace = $namespace)]
            $(#[$attr])*
            $vis struct $name;
        )*
    };
    ([ $( $(#[$attr:meta])* $vis:vis $name:ident),* $(,)? ]) => {
        $(
            #[derive(::std::clone::Clone, ::std::cmp::PartialEq, ::std::default::Default, ::std::fmt::Debug, gpui::Action)]
            $(#[$attr])*
            $vis struct $name;
        )*
    };
}

/// Actions are used to implement keyboard-driven UI. When you declare an action, you can bind keys
/// to the action in the keymap and listeners for that action in the element tree.
///
/// To declare a list of simple actions, you can use the actions! macro, which defines a simple unit
/// struct action for each listed action name in the given namespace.
///
/// Registering the actions with the same name will result in a panic during  `App` creation.
///
/// # Derive Macro
///
/// The derive macro for `Action` requires that the type implement `Clone` and `PartialEq`. A type
/// with fields must also implement `Default` unless `#[action(no_build)]` is specified: the
/// default value is what the action's name builds (command palette, menus, `edctl action`).
///
/// Multiple arguments separated by commas may be specified in `#[action(...)]`:
///
/// - `namespace = some_namespace` sets the namespace. In ed this is required.
///
/// - `name = "ActionName"` overrides the action's name. This must not contain `::`.
///
/// - `no_build` marks an action that only makes sense with its fields set by the caller. It
///   cannot be built from its name, so it is never listed as available.
///
/// - `no_register` skips registering the action. This is useful for implementing the `Action` trait
///   while not supporting invocation by name.
pub trait Action: Any + Send {
    /// Clone the action into a new box.
    fn boxed_clone(&self) -> Box<dyn Action>;

    /// Do a partial equality check on this action and the other.
    fn partial_eq(&self, action: &dyn Action) -> bool;

    /// Get the name of this action, for displaying in UI.
    fn name(&self) -> &'static str;

    /// Get the name of this action type (static).
    fn name_for_type() -> &'static str
    where
        Self: Sized;
}

impl Debug for dyn Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("dyn Action").field("name", &self.name()).finish()
    }
}

impl dyn Action {
    /// Type-erase Action type.
    pub fn as_any(&self) -> &dyn Any {
        self as &dyn Any
    }
}

type ActionBuilder = fn() -> Box<dyn Action>;

pub(crate) struct ActionRegistry {
    /// `None` for an action that cannot be built from its name (`no_build`).
    by_name: FxHashMap<&'static str, Option<ActionBuilder>>,
    builders_by_type_id: HashMap<TypeId, ActionBuilder, TypeIdHashBuilder>,
    all_names: Vec<&'static str>, // So we can return a static slice.
}

impl Default for ActionRegistry {
    fn default() -> Self {
        let mut this = ActionRegistry {
            by_name: FxHashMap::default(),
            builders_by_type_id: HashMap::default(),
            all_names: Vec::default(),
        };
        for action in inventory::iter::<MacroActionData> {
            this.insert_action(action);
        }
        this
    }
}

/// This type must be public so that our macros can build it in other crates.
/// But this is an implementation detail and should not be used directly.
#[doc(hidden)]
pub struct MacroActionData {
    pub name: &'static str,
    pub type_id: fn() -> TypeId,
    pub build: Option<ActionBuilder>,
}

inventory::collect!(MacroActionData);

impl ActionRegistry {
    fn insert_action(&mut self, action: &MacroActionData) {
        let name = action.name;
        assert!(!self.by_name.contains_key(name), "Action with name `{name}` already registered");
        self.by_name.insert(name, action.build);
        if let Some(build) = action.build {
            self.builders_by_type_id.insert((action.type_id)(), build);
        }
        self.all_names.push(name);
    }

    /// Builds the action of the given type, if it is registered and can be built without input.
    pub(crate) fn build_action_type(&self, type_id: &TypeId) -> Option<Box<dyn Action>> {
        Some(self.builders_by_type_id.get(type_id)?())
    }

    /// Builds the action with the given name, if it is registered and can be built without input.
    pub(crate) fn build_action(&self, name: &str) -> Option<Box<dyn Action>> {
        Some((*self.by_name.get(name)?)?())
    }

    pub(crate) const fn all_action_names(&self) -> &[&'static str] {
        self.all_names.as_slice()
    }
}

mod no_action {
    use crate as gpui;

    actions!(
        ed,
        [
            /// Action with special handling which unbinds the keybinding this is associated with,
            /// if it is the highest precedence match.
            pub NoAction
        ]
    );

    /// Action with special handling which unbinds later bindings for the same keystrokes when they
    /// dispatch the named action, regardless of that action's context.
    ///
    #[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
    #[action(namespace = ed, no_build)]
    pub(crate) struct Unbind(pub(crate) gpui::SharedString);

    /// Returns whether or not this action represents a removed key binding.
    pub(crate) fn is_no_action(action: &dyn gpui::Action) -> bool {
        action.as_any().is::<NoAction>()
    }

    /// Returns whether or not this action represents an unbind marker.
    pub(crate) fn is_unbind(action: &dyn gpui::Action) -> bool {
        action.as_any().is::<Unbind>()
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::Action;
    use super::ActionRegistry;
    use crate as gpui;

    actions!(action_test, [Unit]);

    mod scoped {
        use crate as gpui;

        actions!(
            action_test,
            [
                /// Reachable from the parent module.
                pub(super) Visible,
                #[action(name = "Renamed")]
                Hidden
            ]
        );
    }

    #[derive(Clone, Debug, PartialEq, Eq, Action)]
    #[action(namespace = action_test)]
    struct WithDefault {
        count: usize,
    }

    impl Default for WithDefault {
        fn default() -> Self {
            Self { count: 3 }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Action)]
    #[action(namespace = action_test, no_build)]
    struct NeedsInput(usize);

    #[test]
    fn actions_take_visibility_per_name() {
        let registry = ActionRegistry::default();
        assert!(registry.build_action("action_test::Visible").unwrap().partial_eq(&scoped::Visible));
        assert!(registry.build_action("action_test::Renamed").is_some());
    }

    #[test]
    fn builds_unit_and_default_actions_by_name_and_type() {
        let registry = ActionRegistry::default();
        assert!(registry.build_action("action_test::Unit").unwrap().partial_eq(&Unit));
        assert!(registry.build_action("action_test::WithDefault").unwrap().partial_eq(&WithDefault { count: 3 }));
        assert!(registry.build_action_type(&TypeId::of::<WithDefault>()).unwrap().partial_eq(&WithDefault::default()));
        assert!(registry.build_action("action_test::Missing").is_none());
    }

    #[test]
    fn no_build_action_is_named_but_not_built() {
        let registry = ActionRegistry::default();
        assert_eq!(NeedsInput(1).name(), "action_test::NeedsInput");
        assert!(registry.all_action_names().contains(&"action_test::NeedsInput"));
        assert!(registry.build_action("action_test::NeedsInput").is_none());
        assert!(registry.build_action_type(&TypeId::of::<NeedsInput>()).is_none());
    }
}
