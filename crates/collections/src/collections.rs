use std::any::TypeId;
use std::collections;
pub type HashMap<K, V> = FxHashMap<K, V>;
pub type HashSet<T> = FxHashSet<T>;
pub type IndexMap<K, V> = indexmap::IndexMap<K, V, rustc_hash::FxBuildHasher>;
pub type IndexSet<T> = indexmap::IndexSet<T, rustc_hash::FxBuildHasher>;
pub type TypeIdHashMap<V> = collections::HashMap<TypeId, V, gpui_util::TypeIdHashBuilder>;
pub type TypeIdHashSet = collections::HashSet<TypeId, gpui_util::TypeIdHashBuilder>;

pub use std::collections::*;

pub use indexmap::Equivalent;
pub use rustc_hash::FxBuildHasher;
pub use rustc_hash::FxHashMap;
pub use rustc_hash::FxHashSet;
pub use rustc_hash::FxHasher;
