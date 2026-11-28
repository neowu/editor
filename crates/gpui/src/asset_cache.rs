use std::cell::RefCell;
use std::fmt::Debug;
use std::hash::BuildHasher as _;
use std::hash::Hash;
use std::marker::PhantomData;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use futures::TryFutureExt as _;
use rustc_hash::FxHashSet;

// lint: expect(module_cycle)
use crate::App;
// lint: expect(module_cycle)
use crate::EntityId;
use crate::SharedString;
use crate::Task;

pub(crate) struct CachedLoad<T> {
    state: Rc<RefCell<CachedLoadState<T>>>,
    // Keep the owner outside the state so publishing a result cannot cancel its own task.
    _task: Task<()>,
}

enum CachedLoadState<T> {
    // lint: expect(module_cycle)
    Loading(FxHashSet<EntityId>),
    Loaded(T),
}

impl<T: Clone + Send + 'static> CachedLoad<T> {
    // lint: expect(module_cycle)
    pub(crate) fn new(future: impl Future<Output = T> + Send + 'static, cx: &App) -> Self {
        let state = Rc::new(RefCell::new(CachedLoadState::Loading(FxHashSet::default())));
        // lint: expect(module_cycle)
        let task = cx.background_executor().spawn(future);
        // lint: expect(module_cycle)
        let task = cx.spawn({
            let state = Rc::downgrade(&state);
            async move |cx| {
                let result = task.await;
                let Some(state) = state.upgrade() else {
                    return;
                };
                let previous = mem::replace(&mut *state.borrow_mut(), CachedLoadState::Loaded(result));
                let CachedLoadState::Loading(views) = previous else {
                    unreachable!("a cached load completes only once");
                };
                // lint: expect(module_cycle)
                cx.update(|cx| {
                    for view in views {
                        // lint: expect(module_cycle)
                        cx.notify(view);
                    }
                });
            }
        });
        Self { state, _task: task }
    }

    pub(crate) fn get(&self) -> Option<T> {
        match &*self.state.borrow() {
            CachedLoadState::Loading(_) => None,
            CachedLoadState::Loaded(result) => Some(result.clone()),
        }
    }

    // lint: expect(module_cycle)
    pub(crate) fn use_by(&self, view: EntityId) -> Option<T> {
        match &mut *self.state.borrow_mut() {
            CachedLoadState::Loading(views) => {
                views.insert(view);
                None
            }
            CachedLoadState::Loaded(result) => Some(result.clone()),
        }
    }
}

/// An enum representing.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum Resource {
    /// This resource is at a given path in the file system.
    Path(Arc<Path>),
    /// This resource is embedded in the application binary.
    Embedded(SharedString),
}

impl From<PathBuf> for Resource {
    fn from(value: PathBuf) -> Self {
        Self::Path(value.into())
    }
}

impl From<Arc<Path>> for Resource {
    fn from(value: Arc<Path>) -> Self {
        Self::Path(value)
    }
}

/// A trait for asynchronous asset loading.
pub trait Asset: 'static {
    /// The source of the asset.
    type Source: Clone + Hash + Send;

    /// The loaded asset.
    type Output: Clone + Send;

    /// Load the asset asynchronously.
    // lint: expect(module_cycle)
    fn load(source: Self::Source, cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static;
}

/// An asset Loader which logs the [`Err`] variant of a [`Result`] during loading.
pub struct AssetLogger<T>(PhantomData<T>);

impl<T, R, E> Asset for AssetLogger<T>
where
    T: Asset<Output = Result<R, E>>,
    R: Clone + Send,
    E: Clone + Send + Debug,
{
    type Source = T::Source;

    type Output = T::Output;

    // lint: expect(module_cycle)
    fn load(source: Self::Source, cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        let load = T::load(source, cx);
        load.inspect_err(|e| log::error!("Failed to load asset: {e:?}"))
    }
}

/// Use a quick, non-cryptographically secure hash function to get an identifier from data.
pub fn hash<T: Hash>(data: &T) -> u64 {
    rustc_hash::FxBuildHasher.hash_one(data)
}
