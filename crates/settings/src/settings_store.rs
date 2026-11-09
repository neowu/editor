use std::any::Any;
use std::any::TypeId;
use std::any::type_name;
use std::collections::HashMap;
use std::collections::hash_map;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::str;

use gpui::App;
use gpui::AsyncApp;
use gpui::BorrowAppContext;
use gpui::Global;
use gpui::SharedString;
use gpui::UpdateGlobal;
use gpui_util::TypeIdHashBuilder;
use rustc_hash::FxHashMap;

use crate::SemanticTokenRules;
use crate::built_in_semantic_token_rules;

/// A typed view of the built-in settings.
pub trait Settings: 'static + Send + Sync + Sized {
    /// The hardcoded value of this setting.
    fn built_in() -> Self;

    #[track_caller]
    fn register(cx: &mut App)
    where
        Self: Sized,
    {
        SettingsStore::update_global(cx, |store, _| {
            store.register_setting::<Self>();
        });
    }

    #[track_caller]
    fn get_global(cx: &App) -> &Self
    where
        Self: Sized,
    {
        cx.global::<SettingsStore>().get()
    }

    #[track_caller]
    fn try_get(cx: &App) -> Option<&Self>
    where
        Self: Sized,
    {
        if cx.has_global::<SettingsStore>() { cx.global::<SettingsStore>().try_get() } else { None }
    }

    #[track_caller]
    fn try_read_global<R>(cx: &AsyncApp, f: impl FnOnce(&Self) -> R) -> Option<R>
    where
        Self: Sized,
    {
        cx.try_read_global(|s: &SettingsStore, _| f(s.get()))
    }

    #[track_caller]
    fn override_global(settings: Self, cx: &mut App)
    where
        Self: Sized,
    {
        cx.global_mut::<SettingsStore>().override_global(settings)
    }
}

pub struct RegisteredSetting {
    pub id: fn() -> TypeId,
    pub setting_value: fn() -> Box<dyn AnySettingValue>,
}

pub fn setting_value<T: Settings>() -> Box<dyn AnySettingValue> {
    Box::new(SettingValue::<T> { global_value: None })
}

inventory::collect!(RegisteredSetting);

/// Hardcoded application-wide settings, built once per registered type.
pub struct SettingsStore {
    setting_values: HashMap<TypeId, Box<dyn AnySettingValue>, TypeIdHashBuilder>,
    language_semantic_token_rules: FxHashMap<SharedString, SemanticTokenRules>,
}

impl Global for SettingsStore {}

#[derive(Default)]
pub struct DefaultSemanticTokenRules(pub SemanticTokenRules);

impl gpui::Global for DefaultSemanticTokenRules {}

#[derive(Debug)]
struct SettingValue<T> {
    global_value: Option<T>,
}

#[doc(hidden)]
pub trait AnySettingValue: 'static + Send + Sync {
    fn setting_type_name(&self) -> &'static str;
    fn built_in_value(&self) -> Box<dyn Any>;
    fn global_value(&self) -> &dyn Any;
    fn set_global_value(&mut self, value: Box<dyn Any>);
}

impl SettingsStore {
    pub fn new(cx: &mut App) -> Self {
        if !cx.has_global::<DefaultSemanticTokenRules>() {
            cx.set_global(DefaultSemanticTokenRules(built_in_semantic_token_rules::rules()));
        }
        let mut this = Self { setting_values: HashMap::default(), language_semantic_token_rules: FxHashMap::default() };
        this.load_settings_types();
        this
    }

    pub fn update<C, R>(cx: &mut C, f: impl FnOnce(&mut Self, &mut C) -> R) -> R
    where
        C: BorrowAppContext,
    {
        cx.update_global(f)
    }

    /// Add a new type of setting to the store.
    pub fn register_setting<T: Settings>(&mut self) {
        self.register_setting_internal(&RegisteredSetting {
            id: || TypeId::of::<T>(),
            setting_value: setting_value::<T>,
        });
    }

    fn load_settings_types(&mut self) {
        for registered_setting in inventory::iter::<RegisteredSetting>() {
            self.register_setting_internal(registered_setting);
        }
    }

    fn register_setting_internal(&mut self, registered_setting: &RegisteredSetting) {
        let entry = self.setting_values.entry((registered_setting.id)());

        if matches!(entry, hash_map::Entry::Occupied(_)) {
            return;
        }

        let setting_value = entry.or_insert((registered_setting.setting_value)());
        let value = setting_value.built_in_value();
        setting_value.set_global_value(value);
    }

    /// Get the value of a setting.
    ///
    /// Panics if the given setting type has not been registered, or if there is no
    /// value for this setting.
    pub fn get<T: Settings>(&self) -> &T {
        self.setting_values
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("unregistered setting type {}", type_name::<T>()))
            .global_value()
            .downcast_ref::<T>()
            .expect("no default value for setting type")
    }

    /// Get the value of a setting.
    ///
    /// Does not panic
    pub fn try_get<T: Settings>(&self) -> Option<&T> {
        self.setting_values
            .get(&TypeId::of::<T>())
            .map(|value| value.global_value())
            .and_then(|value| value.downcast_ref::<T>())
    }

    /// Override a typed setting until the next settings recomputation.
    pub fn override_global<T: Settings>(&mut self, value: T) {
        self.setting_values
            .get_mut(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("unregistered setting type {}", type_name::<T>()))
            .set_global_value(Box::new(value))
    }

    /// Sets language-specific semantic token rules.
    ///
    /// These rules are registered by built-in language modules.
    /// They are stored separately from the global rules and are only
    /// applied to buffers of the matching language by the `SemanticTokenStylizer`.
    pub fn set_language_semantic_token_rules(&mut self, language: SharedString, rules: SemanticTokenRules) {
        self.language_semantic_token_rules.insert(language, rules);
    }

    /// Returns the language-specific semantic token rules for the given language,
    /// if any have been registered.
    pub fn language_semantic_token_rules(&self, language: &str) -> Option<&SemanticTokenRules> {
        self.language_semantic_token_rules.get(language)
    }
}

impl Debug for SettingsStore {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsStore").finish_non_exhaustive()
    }
}

impl<T: Settings> AnySettingValue for SettingValue<T> {
    fn built_in_value(&self) -> Box<dyn Any> {
        Box::new(T::built_in())
    }

    fn setting_type_name(&self) -> &'static str {
        type_name::<T>()
    }

    fn global_value(&self) -> &dyn Any {
        self.global_value
            .as_ref()
            .unwrap_or_else(|| panic!("no default value for setting {}", self.setting_type_name()))
    }

    fn set_global_value(&mut self, value: Box<dyn Any>) {
        self.global_value = Some(*value.downcast().unwrap());
    }
}
