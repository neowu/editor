use std::ops::Deref;
use std::sync::Arc;

use gpui::App;
use gpui::Global;
use gpui::SharedString;
use language::ManifestName;
use language::ManifestProvider;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

#[derive(Default)]
struct ManifestProvidersState {
    providers: FxHashMap<ManifestName, Arc<dyn ManifestProvider>>,
}

#[derive(Clone, Default)]
pub struct ManifestProvidersStore(Arc<RwLock<ManifestProvidersState>>);

#[derive(Default)]
struct GlobalManifestProvider(ManifestProvidersStore);

impl Deref for GlobalManifestProvider {
    type Target = ManifestProvidersStore;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Global for GlobalManifestProvider {}

impl ManifestProvidersStore {
    /// Returns the global [`ManifestStore`].
    ///
    /// Inserts a default [`ManifestStore`] if one does not yet exist.
    pub fn global(cx: &mut App) -> Self {
        cx.default_global::<GlobalManifestProvider>().0.clone()
    }

    pub fn register(&self, provider: Arc<dyn ManifestProvider>) {
        self.0.write().providers.insert(provider.name(), provider);
    }

    pub fn unregister(&self, name: &SharedString) {
        self.0.write().providers.remove(name);
    }

    pub(super) fn get(&self, name: &SharedString) -> Option<Arc<dyn ManifestProvider>> {
        self.0.read().providers.get(name).cloned()
    }
    pub(crate) fn manifest_file_names(&self) -> FxHashSet<ManifestName> {
        self.0.read().providers.keys().cloned().collect()
    }
}
