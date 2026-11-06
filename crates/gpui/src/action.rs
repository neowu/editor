use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::result;

use anyhow::Context as _;
use anyhow::Result;
pub use gpui_macros::Action;
use gpui_util::TypeIdHashBuilder;
pub use no_action::NoAction;
pub use no_action::Unbind;
pub use no_action::is_no_action;
pub use no_action::is_unbind;
use rustc_hash::FxHashMap;
use serde_json::json;

/// Defines and registers unit structs that can be used as actions. For more complex data types, derive `Action`.
///
/// The namespace argument `editor` can also be omitted, though it is required for ed actions.
#[macro_export]
macro_rules! actions {
    ($namespace:path, [ $( $(#[$attr:meta])* $name:ident),* $(,)? ]) => {
        $(
            #[derive(::std::clone::Clone, ::std::cmp::PartialEq, ::std::default::Default, ::std::fmt::Debug, gpui::Action)]
            #[action(namespace = $namespace)]
            $(#[$attr])*
            pub struct $name;
        )*
    };
    ([ $( $(#[$attr:meta])* $name:ident),* $(,)? ]) => {
        $(
            #[derive(::std::clone::Clone, ::std::cmp::PartialEq, ::std::default::Default, ::std::fmt::Debug, gpui::Action)]
            $(#[$attr])*
            pub struct $name;
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
/// The derive macro for `Action` requires that the type implement `Clone` and `PartialEq`. It also
/// requires `serde::Deserialize` unless `#[action(no_json)]` is
/// specified. In ed these trait impls are used to load keymaps from JSON.
///
/// Multiple arguments separated by commas may be specified in `#[action(...)]`:
///
/// - `namespace = some_namespace` sets the namespace. In ed this is required.
///
/// - `name = "ActionName"` overrides the action's name. This must not contain `::`.
///
/// - `no_json` causes the `build` method to always error, and allows actions not implement
///   `serde::Deserialize`.
///
/// - `no_register` skips registering the action. This is useful for implementing the `Action` trait
///   while not supporting invocation by name or JSON deserialization.
///
/// # Manual Implementation
///
/// If you want to control the behavior of the action trait manually, you can use the lower-level
/// `#[register_action]` macro, which only generates the code needed to register your action before
/// `main`.
pub trait Action: Any + Send {
    /// Clone the action into a new box
    fn boxed_clone(&self) -> Box<dyn Action>;

    /// Do a partial equality check on this action and the other
    fn partial_eq(&self, action: &dyn Action) -> bool;

    /// Get the name of this action, for displaying in UI
    fn name(&self) -> &'static str;

    /// Get the name of this action type (static)
    fn name_for_type() -> &'static str
    where
        Self: Sized;

    /// Build this action from a JSON value. This is used to construct actions from the keymap.
    /// A value of `{}` will be passed for actions that don't have any parameters.
    fn build(value: serde_json::Value) -> Result<Box<dyn Action>>
    where
        Self: Sized;

    /// The documentation for this action, if any. When using the derive macro for actions
    /// this will be automatically generated from the doc comments on the action struct.
    fn documentation() -> Option<&'static str>
    where
        Self: Sized,
    {
        None
    }
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

/// Error type for `Keystroke::parse`. This is used instead of `anyhow::Error` so that ed can use
/// markdown to display it.
#[derive(Debug)]
pub enum ActionBuildError {
    /// Indicates that an action with this name has not been registered.
    NotFound {
        /// Name of the action that was not found.
        name: Box<str>,
    },
    /// Indicates that an error occurred while building the action, typically a JSON deserialization
    /// error.
    BuildError {
        /// Name of the action that was attempting to be built.
        name: Box<str>,
        /// Error that occurred while building the action.
        error: anyhow::Error,
    },
}

impl error::Error for ActionBuildError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ActionBuildError::NotFound { .. } => None,
            ActionBuildError::BuildError { error, .. } => error.source(),
        }
    }
}

impl Display for ActionBuildError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ActionBuildError::NotFound { name } => {
                write!(f, "Didn't find an action named \"{name}\"")
            }
            ActionBuildError::BuildError { name, error } => {
                write!(f, "Error while building action \"{name}\": {error}")
            }
        }
    }
}

type ActionBuilder = fn(json: serde_json::Value) -> anyhow::Result<Box<dyn Action>>;

pub(crate) struct ActionRegistry {
    by_name: FxHashMap<&'static str, ActionData>,
    names_by_type_id: HashMap<TypeId, &'static str, TypeIdHashBuilder>,
    all_names: Vec<&'static str>,                         // So we can return a static slice.
    documentation: FxHashMap<&'static str, &'static str>, // action name -> documentation
}

impl Default for ActionRegistry {
    fn default() -> Self {
        let mut this = ActionRegistry {
            by_name: FxHashMap::default(),
            names_by_type_id: HashMap::default(),
            documentation: FxHashMap::default(),
            all_names: Vec::default(),
        };

        this.load_actions();

        this
    }
}

struct ActionData {
    pub build: ActionBuilder,
}

/// This type must be public so that our macros can build it in other crates.
/// But this is an implementation detail and should not be used directly.
#[doc(hidden)]
pub struct MacroActionBuilder(pub fn() -> MacroActionData);

/// This type must be public so that our macros can build it in other crates.
/// But this is an implementation detail and should not be used directly.
#[doc(hidden)]
pub struct MacroActionData {
    pub name: &'static str,
    pub type_id: TypeId,
    pub build: ActionBuilder,
    pub documentation: Option<&'static str>,
}

inventory::collect!(MacroActionBuilder);

impl ActionRegistry {
    /// Load all registered actions into the registry.
    pub(crate) fn load_actions(&mut self) {
        for builder in inventory::iter::<MacroActionBuilder> {
            let action = builder.0();
            self.insert_action(action);
        }
    }

    fn insert_action(&mut self, action: MacroActionData) {
        let name = action.name;
        if self.by_name.contains_key(name) {
            panic!("Action with name `{name}` already registered");
        }
        self.by_name.insert(name, ActionData { build: action.build });
        self.names_by_type_id.insert(action.type_id, name);
        self.all_names.push(name);
        if let Some(documentation) = action.documentation {
            self.documentation.insert(name, documentation);
        }
    }

    /// Construct an action based on its name and optional JSON parameters sourced from the keymap.
    pub fn build_action_type(&self, type_id: &TypeId) -> Result<Box<dyn Action>> {
        let name =
            self.names_by_type_id.get(type_id).with_context(|| format!("no action type registered for {type_id:?}"))?;

        Ok(self.build_action(name, None)?)
    }

    /// Construct an action based on its name and optional JSON parameters sourced from the keymap.
    pub fn build_action(
        &self,
        name: &str,
        params: Option<serde_json::Value>,
    ) -> result::Result<Box<dyn Action>, ActionBuildError> {
        let build_action =
            self.by_name.get(name).ok_or_else(|| ActionBuildError::NotFound { name: name.into() })?.build;
        (build_action)(params.unwrap_or_else(|| json!({})))
            .map_err(|e| ActionBuildError::BuildError { name: name.into(), error: e })
    }

    pub const fn all_action_names(&self) -> &[&'static str] {
        self.all_names.as_slice()
    }

    pub const fn documentation(&self) -> &FxHashMap<&'static str, &'static str> {
        &self.documentation
    }
}

/// Generate a list of all the registered actions.
/// Useful for transforming the list of available actions into a
/// format suited for static analysis such as in validating keymaps, or
/// generating documentation.
pub fn generate_list_of_all_registered_actions() -> impl Iterator<Item = MacroActionData> {
    inventory::iter::<MacroActionBuilder>.into_iter().map(|builder| builder.0())
}

mod no_action {
    use serde::Deserialize;

    use crate as gpui;

    actions!(
        ed,
        [
            /// Action with special handling which unbinds the keybinding this is associated with,
            /// if it is the highest precedence match.
            NoAction
        ]
    );

    /// Action with special handling which unbinds later bindings for the same keystrokes when they
    /// dispatch the named action, regardless of that action's context.
    ///
    /// In keymap JSON this is written as:
    ///
    /// `["ed::Unbind", "editor::NewLine"]`
    #[derive(Clone, Debug, PartialEq, Deserialize, gpui::Action)]
    #[action(namespace = ed)]
    pub struct Unbind(pub gpui::SharedString);

    /// Returns whether or not this action represents a removed key binding.
    pub fn is_no_action(action: &dyn gpui::Action) -> bool {
        action.as_any().is::<NoAction>()
    }

    /// Returns whether or not this action represents an unbind marker.
    pub fn is_unbind(action: &dyn gpui::Action) -> bool {
        action.as_any().is::<Unbind>()
    }
}
