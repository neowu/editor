use std::any::TypeId;
use std::any::type_name;
use std::borrow;
use std::cell::BorrowMutError;
use std::cell::Cell;
use std::cell::Ref;
use std::cell::RefCell;
use std::cell::RefMut;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::mem;
use std::ops;
use std::ops::Deref;
use std::ops::DerefMut;
use std::panic::Location;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::rc::Weak;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
pub use async_context::AsyncApp;
pub use async_context::AsyncWindowContext;
pub use context::Context;
use derive_more::Deref;
use derive_more::DerefMut;
pub use entity_map::AnyEntity;
pub use entity_map::AnyWeakEntity;
pub use entity_map::Entity;
pub use entity_map::EntityId;
pub(crate) use entity_map::EntityMap;
pub(crate) use entity_map::Lease;
pub(crate) use entity_map::Slot;
pub use entity_map::WeakEntity;
use futures::Future;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future;
use futures::future::LocalBoxFuture;
use gpui_util::ResultExt as _;
use gpui_util::TypeIdHashBuilder;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use slotmap::SlotMap;
use smallvec::SmallVec;

use crate::Action;
use crate::ActionRegistry;
use crate::ActivationPolicy;
use crate::Any;
use crate::AnyView;
use crate::AnyWindowHandle;
use crate::AppContext;
use crate::Arena;
use crate::ArenaBox;
use crate::Asset;
use crate::AssetSource;
use crate::BackgroundExecutor;
use crate::Bounds;
use crate::ClipboardItem;
use crate::CursorStyle;
use crate::DisplayId;
use crate::EventEmitter;
use crate::FocusHandle;
use crate::FocusMap;
use crate::ForegroundExecutor;
use crate::Global;
use crate::KeyBinding;
use crate::Keymap;
use crate::Keystroke;
use crate::LayoutId;
use crate::Menu;
use crate::MenuItem;
use crate::PathPromptOptions;
use crate::Pixels;
use crate::Platform;
use crate::PlatformDisplay;
use crate::Point;
use crate::Render;
use crate::RenderImage;
use crate::Reservation;
use crate::SingleSubscriberSet;
use crate::SubscriberSet;
use crate::Subscription;
use crate::SvgRenderer;
use crate::Task;
use crate::TextSystem;
use crate::Window;
use crate::WindowAppearance;
use crate::WindowHandle;
use crate::WindowId;
use crate::WindowInvalidator;
use crate::asset_cache::CachedLoad;
use crate::hash;
use crate::init_app_menus;

mod async_context;
mod context;
mod entity_map;

/// The duration for which native applications wait for futures returned from
/// [Context::on_app_quit] before fully quitting.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(200);

/// Temporary(?) wrapper around [`RefCell<App>`] to help us debug any double borrows.
/// Strongly consider removing after stabilization.
#[expect(clippy::print_stderr, clippy::use_debug, reason = "opt-in borrow tracing")]
fn track_thread_borrow(event: &str) {
    if option_env!("TRACK_THREAD_BORROWS").is_some() {
        eprintln!("{event} {:?}", thread::current().id());
    }
}

#[doc(hidden)]
pub struct AppCell {
    app: RefCell<App>,
}

impl AppCell {
    #[doc(hidden)]
    #[track_caller]
    pub fn borrow(&self) -> AppRef<'_> {
        track_thread_borrow("borrowed");
        AppRef(self.app.borrow())
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn borrow_mut(&self) -> AppRefMut<'_> {
        track_thread_borrow("borrowed");
        AppRefMut(self.app.borrow_mut())
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn try_borrow_mut(&self) -> Result<AppRefMut<'_>, BorrowMutError> {
        track_thread_borrow("borrowed");
        Ok(AppRefMut(self.app.try_borrow_mut()?))
    }
}

#[doc(hidden)]
#[derive(Deref, DerefMut)]
pub struct AppRef<'a>(Ref<'a, App>);

impl Drop for AppRef<'_> {
    fn drop(&mut self) {
        track_thread_borrow("dropped borrow from");
    }
}

#[doc(hidden)]
#[derive(Deref, DerefMut)]
pub struct AppRefMut<'a>(RefMut<'a, App>);

impl Drop for AppRefMut<'_> {
    fn drop(&mut self) {
        track_thread_borrow("dropped");
    }
}

/// A reference to a GPUI application, typically constructed in the `main` function of your app.
/// You won't interact with this type much outside of initial configuration and startup.
pub struct Application(Rc<AppCell>);

/// Represents an application before it is fully launched. Once your app is
/// configured, you'll start the app with `App::run`.
impl Application {
    /// Builds an app with a caller-provided platform implementation.
    pub fn with_platform(platform: Rc<dyn Platform>) -> Self {
        Self(App::new_app(platform, Arc::new(())))
    }

    /// Assigns the source of assets for the application.
    pub fn with_assets(self, asset_source: impl AssetSource) -> Self {
        let mut context_lock = self.0.borrow_mut();
        let asset_source: Arc<dyn AssetSource> = Arc::new(asset_source);
        context_lock.asset_source = Arc::clone(&asset_source);
        context_lock.svg_renderer = SvgRenderer::new(asset_source);
        drop(context_lock);
        self
    }

    /// Start the application. The provided callback will be called once the
    /// app is fully launched.
    ///
    pub fn run<F>(self, on_finish_launching: F)
    where
        F: 'static + FnOnce(&mut App),
    {
        let this = Rc::clone(&self.0);
        let platform = Rc::clone(&self.0.borrow().platform);
        platform.run(Box::new(move || {
            let cx = &mut *this.borrow_mut();
            cx.displays_changed();
            on_finish_launching(cx);
        }));
    }

    /// Register a handler to be invoked when the platform instructs the application
    /// to open one or more URLs.
    pub fn on_open_urls<F>(&self, callback: F) -> &Self
    where
        F: 'static + FnMut(Vec<String>),
    {
        self.0.borrow().platform.on_open_urls(Box::new(callback));
        self
    }

    /// Invokes a handler when an already-running application is launched.
    /// On macOS, this can occur when the application icon is double-clicked or the app is launched via the dock.
    pub fn on_reopen<F>(&self, mut callback: F) -> &Self
    where
        F: 'static + FnMut(&mut App),
    {
        let this = Rc::downgrade(&self.0);
        self.0.borrow_mut().platform.on_reopen(Box::new(move || {
            if let Some(app) = this.upgrade() {
                callback(&mut app.borrow_mut());
            }
        }));
        self
    }

    /// Returns a handle to the [`BackgroundExecutor`] associated with this app, which can be used to spawn futures in the background.
    pub fn background_executor(&self) -> BackgroundExecutor {
        self.0.borrow().background_executor.clone()
    }

    /// Returns a handle to the [`ForegroundExecutor`] associated with this app, which can be used to spawn futures in the foreground.
    pub fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.borrow().foreground_executor.clone()
    }

    /// Returns a reference to the [`TextSystem`] associated with this app.
    pub fn text_system(&self) -> Arc<TextSystem> {
        Arc::clone(&self.0.borrow().text_system)
    }
}

type Handler = Box<dyn FnMut(&mut App) -> bool + 'static>;

/// The refresh interval of each connected display.
fn read_displays(platform: &dyn Platform) -> FxHashMap<DisplayId, Option<Duration>> {
    platform.displays().into_iter().map(|display| (display.id(), display.refresh_interval())).collect()
}

type Listener = Box<dyn FnMut(&dyn Any, &mut App) -> bool + 'static>;
type QuitHandler = (Box<dyn FnOnce(&mut App) -> LocalBoxFuture<'static, ()> + 'static>, &'static Location<'static>);
type ReleaseListener = Box<dyn FnOnce(&mut dyn Any, &mut App) + 'static>;
type NewEntityListener = Box<dyn FnMut(AnyEntity, &mut Option<&mut Window>, &mut App) + 'static>;

/// Contains the state of the full application, and passed as a reference to a variety of callbacks.
/// Other [Context] derefs to this type.
/// You need a reference to an `App` to access the state of a [Entity].
pub struct App {
    pub(crate) this: Weak<AppCell>,
    pub(crate) platform: Rc<dyn Platform>,
    text_system: Arc<TextSystem>,

    pub(crate) actions: Rc<ActionRegistry>,
    pub(crate) active_drag: Option<AnyDrag>,
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) entities: EntityMap,
    pub(crate) new_entity_observers: SubscriberSet<TypeId, NewEntityListener>,
    pub(crate) windows: SlotMap<WindowId, Option<Box<Window>>>,
    pub(crate) window_handles: FxHashMap<WindowId, AnyWindowHandle>,
    pub(crate) focus_handles: Arc<RwLock<FocusMap>>,
    pub(crate) keymap: Rc<RefCell<Keymap>>,
    /// One listener per action type; registering again replaces the previous one.
    pub(crate) global_action_listeners: HashMap<TypeId, Rc<dyn Fn(&dyn Any, &mut Self)>, TypeIdHashBuilder>,
    pending_effects: VecDeque<Effect>,

    pub(crate) observers: SubscriberSet<EntityId, Handler>,
    pub(crate) event_listeners: SubscriberSet<EntityId, (TypeId, Listener)>,
    /// The connected displays, as of the platform's last display change
    /// notification.
    displays: FxHashMap<DisplayId, Option<Duration>>,
    pub(crate) system_sleep_observers: SingleSubscriberSet<Handler>,
    pub(crate) system_wake_observers: SingleSubscriberSet<Handler>,
    pub(crate) release_listeners: SubscriberSet<EntityId, ReleaseListener>,
    pub(crate) global_observers: SubscriberSet<TypeId, Handler>,
    pub(crate) quit_observers: SingleSubscriberSet<QuitHandler>,

    /// Per-App element arena. This isolates element allocations between different
    /// App instances.
    ///
    /// Kept in its own allocation because draws reach it through a raw pointer
    /// (`CURRENT_ELEMENT_ARENA`) while a `&mut App` is live. If it were stored
    /// inline, those accesses would alias memory the `&mut App` claims
    /// exclusively, which is undefined behavior.
    pub(crate) element_arena: Rc<RefCell<Arena>>,
    /// Per-App event arena.
    pub(crate) event_arena: Arena,

    // Drop globals last, after all tasks owned by entities and callbacks are
    // marked cancelled.
    pub(crate) globals_by_type: HashMap<TypeId, Box<dyn Any>, TypeIdHashBuilder>,

    // assets
    pub(crate) loading_assets: FxHashMap<(TypeId, u64), Box<dyn Any>>,
    asset_source: Arc<dyn AssetSource>,
    pub(crate) svg_renderer: SvgRenderer,

    // below is plain data, the drop order is insignificant here
    pub(crate) pending_notifications: FxHashSet<EntityId>,
    pub(crate) pending_global_notifications: HashSet<TypeId, TypeIdHashBuilder>,
    pub(crate) layout_id_buffer: Vec<LayoutId>, // We recycle this memory across layout requests.
    pub(crate) propagate_event: bool,
    pub(crate) ui_snapshots: bool,
    pub(crate) window_invalidators_by_entity: FxHashMap<EntityId, FxHashMap<WindowId, WindowInvalidator>>,
    pub(crate) tracked_entities: FxHashMap<WindowId, FxHashSet<EntityId>>,
    pub(crate) current_window_by_entity: FxHashMap<EntityId, WindowId>,

    pub(crate) window_update_stack: Vec<WindowId>,
    /// Origin of the shared clock that phase-locks synced repeating animations.
    pub(crate) synced_animation_epoch: Instant,
    pending_updates: usize,
    quitting: bool,
}

impl App {
    pub(crate) fn new_app(platform: Rc<dyn Platform>, asset_source: Arc<dyn AssetSource>) -> Rc<AppCell> {
        let background_executor = platform.background_executor();
        let foreground_executor = platform.foreground_executor();
        assert!(background_executor.is_main_thread(), "must construct App on main thread");
        let synced_animation_epoch = background_executor.now();

        let text_system = Arc::new(TextSystem::new(platform.text_system()));
        let entities = EntityMap::new();

        let app = Rc::new_cyclic(|this| AppCell {
            app: RefCell::new(App {
                this: Weak::clone(this),
                platform: Rc::clone(&platform),
                text_system,
                actions: Rc::new(ActionRegistry::default()),
                pending_updates: 0,
                active_drag: None,
                background_executor,
                foreground_executor,
                svg_renderer: SvgRenderer::new(Arc::clone(&asset_source)),
                loading_assets: FxHashMap::default(),
                asset_source,
                globals_by_type: HashMap::default(),
                entities,
                new_entity_observers: SubscriberSet::new(),
                windows: SlotMap::with_key(),
                window_update_stack: Vec::new(),
                window_handles: FxHashMap::default(),
                focus_handles: Arc::new(RwLock::new(FocusMap::default())),
                keymap: Rc::new(RefCell::new(Keymap::default())),
                global_action_listeners: HashMap::default(),
                pending_effects: VecDeque::new(),
                pending_notifications: FxHashSet::default(),
                pending_global_notifications: HashSet::default(),
                observers: SubscriberSet::new(),
                tracked_entities: FxHashMap::default(),
                window_invalidators_by_entity: FxHashMap::default(),
                current_window_by_entity: FxHashMap::default(),
                event_listeners: SubscriberSet::new(),
                release_listeners: SubscriberSet::new(),
                // Read in `run`, and again by a window that finds its display unknown.
                displays: FxHashMap::default(),
                system_sleep_observers: SingleSubscriberSet::new(),
                system_wake_observers: SingleSubscriberSet::new(),
                global_observers: SubscriberSet::new(),
                quit_observers: SingleSubscriberSet::new(),
                layout_id_buffer: Vec::default(),
                propagate_event: true,
                ui_snapshots: false,
                quitting: false,
                synced_animation_epoch,

                element_arena: Rc::new(RefCell::new(Arena::new(1024 * 1024))),
                event_arena: Arena::new(1024 * 1024),
            }),
        });

        init_app_menus(platform.as_ref(), &app.borrow());

        platform.on_displays_changed(Box::new({
            let app = Rc::downgrade(&app);
            move || {
                if let Some(app) = app.upgrade() {
                    app.borrow_mut().displays_changed();
                }
            }
        }));

        platform.on_system_sleep(Box::new({
            let app = Rc::downgrade(&app);
            move || {
                if let Some(app) = app.upgrade() {
                    let cx = &mut app.borrow_mut();
                    cx.system_sleep_observers.clone().retain(move |callback| (callback)(cx));
                }
            }
        }));

        platform.on_system_wake(Box::new({
            let app = Rc::downgrade(&app);
            move || {
                if let Some(app) = app.upgrade() {
                    let cx = &mut app.borrow_mut();
                    cx.system_wake_observers.clone().retain(move |callback| (callback)(cx));
                }
            }
        }));

        platform.on_quit(Box::new({
            let cx = Rc::downgrade(&app);
            move || {
                let Some(cx) = cx.upgrade() else {
                    return true;
                };
                match cx.try_borrow_mut() {
                    Ok(mut cx) => {
                        cx.shutdown();
                        true
                    }
                    Err(_) => {
                        // Quit was requested while the AppCell was borrowed, so we can't shut down synchronously.
                        // The platform decides how to proceed.
                        false
                    }
                }
            }
        }));

        app
    }

    /// Quit the application gracefully.
    ///
    /// Native applications give handlers registered with [`Context::on_app_quit`]
    /// [`SHUTDOWN_TIMEOUT`] to complete.
    pub fn shutdown(&mut self) {
        // Requested first so the final hang poll overlaps with the quit
        // handlers. It's awaited alongside them, within `SHUTDOWN_TIMEOUT`.

        let mut futures: Vec<LocalBoxFuture<'static, ()>> = Vec::new();
        let mut pending = Vec::new();

        for (observer, location) in self.quit_observers.remove() {
            let future = observer(self);
            let done = Rc::new(Cell::new(false));
            pending.push((location, Rc::clone(&done)));
            futures.push(
                async move {
                    future.await;
                    done.set(true);
                }
                .boxed_local(),
            );
        }

        self.windows.clear();
        self.window_handles.clear();
        self.flush_effects();
        self.quitting = true;

        let futures = future::join_all(futures);

        if self.foreground_executor.block_with_timeout(SHUTDOWN_TIMEOUT, futures).is_err() {
            let stuck = pending
                .iter()
                .filter(|(_, done)| !done.get())
                .map(|(location, _)| location.to_string())
                .collect::<Vec<_>>();
            log::error!("timed out after {SHUTDOWN_TIMEOUT:?} waiting on app_will_quit handlers: {}", stuck.join(", "));
        }

        self.quitting = false;
    }

    /// Gracefully quit the application via the platform's standard routine.
    pub fn quit(&self) {
        self.platform.quit();
    }

    /// Marks the cursor visible again, as a mouse event from the platform
    /// does. For mouse input injected past the platform.
    pub fn mark_cursor_visible(&self) {
        self.platform.mark_cursor_visible();
    }

    /// Returns whether the cursor is currently visible according to the
    /// platform. This will report `false` after a keyboard input has hidden
    /// the cursor and the user has not yet moved the mouse to restore it.
    pub fn is_cursor_visible(&self) -> bool {
        self.platform.is_cursor_visible()
    }

    /// Schedules all windows in the application to be redrawn. This can be called
    /// multiple times in an update cycle and still result in a single redraw.
    pub fn refresh_windows(&mut self) {
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

    #[inline(always)]
    pub(crate) fn update<R>(&mut self, update: impl FnOnce(&mut Self) -> R) -> R {
        self.start_update();
        let result = update(self);
        self.finish_update();
        result
    }

    pub(crate) const fn start_update(&mut self) {
        self.pending_updates += 1;
    }

    #[inline(never)]
    pub(crate) fn finish_update(&mut self) {
        if self.pending_updates == 1 {
            self.flush_effects();
        }
        self.pending_updates -= 1;
    }

    /// Arrange a callback to be invoked when the given entity calls `notify` on its respective context.
    pub fn observe<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(Entity<W>, &mut App) + 'static,
    ) -> Subscription
    where
        W: 'static,
    {
        self.observe_internal(entity, move |e, cx| {
            on_notify(e, cx);
            true
        })
    }

    pub(crate) fn detect_accessed_entities<R>(
        &mut self,
        callback: impl FnOnce(&mut App) -> R,
    ) -> (R, FxHashSet<EntityId>) {
        self.entities.push_access_scope();
        let result = callback(self);
        (result, self.entities.pop_access_scope())
    }

    pub(crate) fn record_entities_accessed(
        &mut self,
        window_handle: AnyWindowHandle,
        invalidator: WindowInvalidator,
        entities: &FxHashSet<EntityId>,
    ) {
        let mut tracked_entities = mem::take(self.tracked_entities.entry(window_handle.id).or_default());
        for entity in &tracked_entities {
            self.window_invalidators_by_entity.entry(*entity).and_modify(|windows| {
                windows.remove(&window_handle.id);
            });
        }
        for entity in entities {
            self.window_invalidators_by_entity
                .entry(*entity)
                .or_default()
                .insert(window_handle.id, invalidator.clone());
            self.current_window_by_entity.insert(*entity, window_handle.id);
        }
        tracked_entities.clear();
        tracked_entities.extend(entities.iter().copied());
        self.tracked_entities.insert(window_handle.id, tracked_entities);
    }

    pub(crate) fn new_observer(&mut self, key: EntityId, value: Handler) -> Subscription {
        let (subscription, activate) = self.observers.insert_pending(key, value);
        self.defer(move |_| activate());
        subscription
    }

    pub(crate) fn observe_internal<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(Entity<W>, &mut App) -> bool + 'static,
    ) -> Subscription
    where
        W: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        self.new_observer(
            entity_id,
            Box::new(move |cx| if let Some(entity) = handle.upgrade() { on_notify(entity, cx) } else { false }),
        )
    }

    /// Arrange for the given callback to be invoked whenever the given entity emits an event of a given type.
    /// The callback is provided a handle to the emitting entity and a reference to the emitted event.
    pub fn subscribe<T, Event>(
        &mut self,
        entity: &Entity<T>,
        mut on_event: impl FnMut(Entity<T>, &Event, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static + EventEmitter<Event>,
        Event: 'static,
    {
        self.subscribe_internal(entity, move |entity, event, cx| {
            on_event(entity, event, cx);
            true
        })
    }

    pub(crate) fn new_subscription(&mut self, key: EntityId, value: (TypeId, Listener)) -> Subscription {
        let (subscription, activate) = self.event_listeners.insert_pending(key, value);
        self.defer(move |_| activate());
        subscription
    }
    pub(crate) fn subscribe_internal<T, Evt>(
        &mut self,
        entity: &Entity<T>,
        mut on_event: impl FnMut(Entity<T>, &Evt, &mut App) -> bool + 'static,
    ) -> Subscription
    where
        T: 'static + EventEmitter<Evt>,
        Evt: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        self.new_subscription(
            entity_id,
            (
                TypeId::of::<Evt>(),
                Box::new(move |event, cx| {
                    let event: &Evt = event.downcast_ref().expect("invalid event type");
                    if let Some(entity) = handle.upgrade() { on_event(entity, event, cx) } else { false }
                }),
            ),
        )
    }

    /// Returns handles to all open windows in the application.
    /// Each handle could be downcast to a handle typed for the root view of that window.
    /// To find all windows of a given type, you could filter on.
    pub fn windows(&self) -> Vec<AnyWindowHandle> {
        self.windows.keys().filter_map(|window_id| self.window_handles.get(&window_id).copied()).collect()
    }

    /// Returns the window handles ordered by their appearance on screen, front to back.
    ///
    /// The first window in the returned list is the active/topmost window of the application.
    ///
    /// This method returns None if the platform doesn't implement the method yet.
    pub fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        self.platform.window_stack()
    }

    /// Returns a handle to the window that is currently focused at the platform level, if one exists.
    pub fn active_window(&self) -> Option<AnyWindowHandle> {
        self.platform.active_window()
    }

    /// Opens a new window with the given option and the root view returned by the given function.
    /// The function is invoked with a `Window`, which can be used to interact with window-specific
    /// functionality.
    pub fn open_window<V: 'static + Render>(
        &mut self,
        options: crate::WindowOptions,
        build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> anyhow::Result<WindowHandle<V>> {
        self.update(|cx| {
            let id = cx.windows.insert(None);
            let handle = WindowHandle::new(id);
            match Window::new(handle.into(), options, cx) {
                Ok(mut window) => {
                    cx.window_update_stack.push(id);
                    let root_view = build_root_view(&mut window, cx);
                    cx.window_update_stack.pop();
                    window.root.replace(root_view.into());
                    window.defer(cx, |window: &mut Window, cx| window.appearance_changed(cx));

                    // allow a window to draw at least once before returning
                    // this didn't cause any issues on non windows platforms as it seems we always won the race to on_request_frame
                    // on windows we quite frequently lose the race and return a window that has never rendered, which leads to a crash
                    // where DispatchTree::root_node_id asserts on empty nodes
                    let clear = window.draw(cx);
                    clear.clear(cx);

                    cx.window_handles.insert(id, window.handle);
                    cx.windows.get_mut(id).expect("window slot was reserved above").replace(Box::new(window));
                    Ok(handle)
                }
                Err(e) => {
                    cx.windows.remove(id);
                    Err(e)
                }
            }
        })
    }

    /// Instructs the platform to activate the application by bringing it to the foreground.
    pub fn activate(&self, ignoring_other_apps: bool) {
        self.platform.activate(ignoring_other_apps);
    }

    /// Hide the application at the platform level.
    pub fn hide(&self) {
        self.platform.hide();
    }

    /// Hide other applications at the platform level.
    pub fn hide_other_apps(&self) {
        self.platform.hide_other_apps();
    }

    /// Unhide other applications at the platform level.
    pub fn unhide_other_apps(&self) {
        self.platform.unhide_other_apps();
    }

    /// Returns the list of currently active displays.
    pub fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.platform.displays()
    }

    /// Returns the display the user is working on, where new windows open
    /// (see [`Platform::main_display`]).
    pub fn main_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.platform.main_display()
    }

    /// Returns the display with the given ID, if one exists.
    pub fn find_display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        self.displays().iter().find(|display| display.id() == id).cloned()
    }

    /// Invokes a handler when the system is about to sleep.
    ///
    /// The platform gives the process only a short time before suspending, so
    /// handlers should record state or cancel work rather than start it.
    pub fn on_system_sleep<F>(&self, mut callback: F) -> Subscription
    where
        F: 'static + FnMut(&mut App),
    {
        self.system_sleep_observers.insert(Box::new(move |cx| {
            callback(cx);
            true
        }))
    }

    pub(crate) fn displays_changed(&mut self) {
        self.displays = read_displays(self.platform.as_ref());
    }

    pub(crate) fn knows_display(&self, id: DisplayId) -> bool {
        self.displays.contains_key(&id)
    }

    /// The refresh interval of a connected display, as of the platform's last
    /// display change notification.
    pub(crate) fn display_refresh_interval(&self, id: DisplayId) -> Option<Duration> {
        *self.displays.get(&id)?
    }

    /// Invokes a handler when the system wakes from sleep.
    pub fn on_system_wake<F>(&self, mut callback: F) -> Subscription
    where
        F: 'static + FnMut(&mut App),
    {
        self.system_wake_observers.insert(Box::new(move |cx| {
            callback(cx);
            true
        }))
    }

    /// Returns the appearance of the application's windows.
    pub fn window_appearance(&self) -> WindowAppearance {
        self.platform.window_appearance()
    }

    /// Overrides the appearance (light/dark) applied to the app's windows, independent of
    /// the OS-wide setting. Pass `None` to clear the override and follow the system again.
    /// The current value is reported by [`App::window_appearance`].
    ///
    /// On macOS this sets the underlying `NSApplication.appearance`, which controls the
    /// native window chrome (the window border and titlebar) of every window. Use this
    /// when the app uses a dark theme while the system is in light mode (or vice versa)
    /// so the window edges render to match the theme. While an appearance is forced,
    /// windows stop tracking system light/dark changes; pass `None` to resume following
    /// the system. On other platforms this is a no-op.
    pub fn set_window_appearance(&self, appearance: Option<WindowAppearance>) {
        self.platform.set_window_appearance(appearance);
    }

    /// Reads data from the platform clipboard.
    pub fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.platform.read_from_clipboard()
    }

    /// Writes data to the platform clipboard.
    pub fn write_to_clipboard(&self, item: ClipboardItem) {
        self.platform.write_to_clipboard(item);
    }

    /// Reads data from macOS's "Find" pasteboard.
    ///
    /// Used to share the current search string between apps.
    ///
    /// <https://developer.apple.com/documentation/appkit/nspasteboard/name-swift.struct/find>.
    pub fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.platform.read_from_find_pasteboard()
    }

    /// Writes data to macOS's "Find" pasteboard.
    ///
    /// Used to share the current search string between apps.
    ///
    /// <https://developer.apple.com/documentation/appkit/nspasteboard/name-swift.struct/find>.
    pub fn write_to_find_pasteboard(&self, item: ClipboardItem) {
        self.platform.write_to_find_pasteboard(item);
    }

    /// Directs the platform's default browser to open the given URL.
    pub fn open_url(&self, url: &str) {
        self.platform.open_url(url);
    }

    /// Returns the full pathname of the current app bundle.
    ///
    /// Returns an error if the app is not being run from a bundle.
    pub fn app_path(&self) -> Result<PathBuf> {
        self.platform.app_path()
    }

    /// Displays a platform modal for selecting paths.
    ///
    /// When one or more paths are selected, they'll be relayed asynchronously via the returned oneshot channel.
    /// If cancelled, a `None` will be relayed instead.
    pub fn prompt_for_paths(&self, options: PathPromptOptions) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.platform.prompt_for_paths(options)
    }

    /// Displays a platform modal for selecting a new path where a file can be saved.
    ///
    /// The provided directory will be used to set the initial location.
    /// When a path is selected, it is relayed asynchronously via the returned oneshot channel.
    /// If cancelled, a `None` will be relayed instead.
    pub fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.platform.prompt_for_new_path(directory, suggested_name)
    }

    /// Reveals the specified path at the platform level, such as in Finder on macOS.
    pub fn reveal_path(&self, path: &Path) {
        self.platform.reveal_path(path);
    }

    /// Opens the specified path with the system's default application.
    pub fn open_with_system(&self, path: &Path) {
        self.platform.open_with_system(path);
    }

    /// Returns whether the user has configured scrollbars to auto-hide at the platform level.
    pub fn should_auto_hide_scrollbars(&self) -> bool {
        self.platform.should_auto_hide_scrollbars()
    }

    /// Sets whether the application participates in the system's foreground UI.
    ///
    /// Use this for an accessory app that shows windows, such as a menu bar utility. After switching to [`ActivationPolicy::Regular`], activate the app yourself
    /// with [`Self::activate`]; otherwise its menu bar may not appear until the app is
    /// reactivated.
    pub fn set_activation_policy(&mut self, policy: ActivationPolicy) {
        self.platform.set_activation_policy(policy);
    }

    /// Returns the SVG renderer used by the application.
    pub fn svg_renderer(&self) -> SvgRenderer {
        self.svg_renderer.clone()
    }

    pub(crate) fn push_effect(&mut self, effect: Effect) {
        match &effect {
            Effect::Notify { emitter } => {
                if !self.pending_notifications.insert(*emitter) {
                    return;
                }
            }
            Effect::NotifyGlobalObservers { global_type }
                if !self.pending_global_notifications.insert(*global_type) =>
            {
                return;
            }
            _ => {}
        }

        self.pending_effects.push_back(effect);
    }

    /// Called at the end of [`App::update`] to complete any side effects
    /// such as notifying observers, emitting events, etc. Effects can themselves
    /// cause effects, so we continue looping until all effects are processed.
    fn flush_effects(&mut self) {
        loop {
            self.release_dropped_entities();
            self.release_dropped_focus_handles();
            if let Some(effect) = self.pending_effects.pop_front() {
                match effect {
                    Effect::Notify { emitter } => {
                        self.apply_notify_effect(emitter);
                    }

                    Effect::Emit { emitter, event_type, event } => self.apply_emit_effect(emitter, event_type, &*event),

                    Effect::RefreshWindows => {
                        self.apply_refresh_effect();
                    }

                    Effect::NotifyGlobalObservers { global_type } => {
                        self.apply_notify_global_observers_effect(global_type);
                    }

                    Effect::Defer { callback } => {
                        self.apply_defer_effect(callback);
                    }
                    Effect::EntityCreated { entity, tid, window } => {
                        self.apply_entity_created_effect(entity, tid, window);
                    }
                }
            } else if self.pending_effects.is_empty() {
                for window in self.windows.values().filter_map(|window| window.as_deref()) {
                    if window.invalidator.is_dirty()
                        || window.needs_present.get()
                        || !window.next_frame_callbacks.borrow().is_empty()
                    {
                        window.platform_window.schedule_frame();
                    }
                }

                self.event_arena.clear();
                break;
            }
        }
    }

    /// Repeatedly called during `flush_effects` to release any entities whose
    /// reference count has become zero. We invoke any release observers before dropping
    /// each entity.
    fn release_dropped_entities(&mut self) {
        loop {
            let dropped = self.entities.take_dropped();
            if dropped.is_empty() {
                break;
            }

            for (entity_id, mut entity) in dropped {
                self.observers.remove(&entity_id);
                self.event_listeners.remove(&entity_id);
                self.window_invalidators_by_entity.remove(&entity_id);
                self.current_window_by_entity.remove(&entity_id);
                for release_callback in self.release_listeners.remove(&entity_id) {
                    release_callback(entity.as_mut(), self);
                }
            }
        }
    }

    /// Repeatedly called during `flush_effects` to handle a focused handle being dropped.
    fn release_dropped_focus_handles(&mut self) {
        let focus_handles = Arc::clone(&self.focus_handles);
        let mut focus_handles = focus_handles.write();
        let FocusMap { handles, dropped_handles } = &mut *focus_handles;
        if dropped_handles.is_empty() {
            return;
        }

        let windows = self
            .windows
            .values()
            .filter_map(|window| {
                let window = window.as_ref()?;
                dropped_handles.contains(&window.focus?).then_some(window.handle)
            })
            .collect::<SmallVec<[_; 1]>>();
        for handle_id in dropped_handles.drain(..) {
            handles.remove(handle_id);
        }
        drop(focus_handles);

        for window_handle in windows {
            window_handle.update(self, |_, window, cx| window.blur(cx)).ok();
        }
    }

    fn apply_notify_effect(&mut self, emitter: EntityId) {
        self.pending_notifications.remove(&emitter);

        self.observers.clone().retain(&emitter, |handler| handler(self));
    }

    fn apply_emit_effect(&mut self, emitter: EntityId, event_type: TypeId, event: &dyn Any) {
        self.event_listeners.clone().retain(&emitter, |(stored_type, handler)| {
            if *stored_type == event_type { handler(event, self) } else { true }
        });
    }

    fn apply_refresh_effect(&mut self) {
        for window in self.windows.values_mut() {
            if let Some(window) = window.as_deref_mut() {
                window.refreshing = true;
                window.invalidator.set_dirty(true);
            }
        }
    }

    fn apply_notify_global_observers_effect(&mut self, type_id: TypeId) {
        self.pending_global_notifications.remove(&type_id);
        self.global_observers.clone().retain(&type_id, |observer| observer(self));
    }

    fn apply_defer_effect(&mut self, callback: Box<dyn FnOnce(&mut Self) + 'static>) {
        callback(self);
    }

    fn apply_entity_created_effect(&mut self, entity: AnyEntity, tid: TypeId, window: Option<WindowId>) {
        // Seed the entity's current window from its creation context so
        // `with_window` resolves correctly before the entity has ever been
        // rendered.
        if let Some(id) = window {
            self.current_window_by_entity.insert(entity.entity_id(), id);
        }

        self.new_entity_observers.clone().retain(&tid, |observer| {
            if let Some(id) = window {
                self.update_window_id(id, {
                    let entity = entity.clone();
                    |_, window, cx| (observer)(entity, &mut Some(window), cx)
                })
                .expect("All windows should be off the stack when flushing effects");
            } else {
                (observer)(entity.clone(), &mut None, self);
            }
            true
        });
    }

    /// Run `f` against the entity's *current* window — the most recently
    /// rendered window that referenced the entity, or its creation window if
    /// it has yet to be rendered. Returns `None` if the entity has no
    /// current window, or if that window has been closed, or if it is
    /// already on the update stack.
    #[inline(always)]
    pub fn with_window<R>(&mut self, entity_id: EntityId, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R> {
        let window_id = *self.current_window_by_entity.get(&entity_id)?;
        self.update_window_id(window_id, |_, window, cx| f(window, cx)).ok()
    }

    fn ensure_window(&mut self, entity_id: EntityId, window: WindowId) {
        self.current_window_by_entity.entry(entity_id).or_insert(window);
    }

    #[inline(always)]
    pub(crate) fn update_window_id<T, F>(&mut self, id: WindowId, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        let mut update = Some(update);
        let mut result = None;
        self.update_window_erased(id, &mut |arguments| {
            if let Some((root_view, window, cx)) = arguments {
                result = Some(update.take().expect("update is called once")(root_view, window, cx));
            } else {
                drop(update.take());
                drop(result.take());
            }
        });
        result.context("window not found")
    }

    /// Creates an `AsyncApp`, which can be cloned and has a static lifetime
    /// so it can be held across `await` points.
    pub fn to_async(&self) -> AsyncApp {
        AsyncApp {
            app: Weak::clone(&self.this),
            background_executor: self.background_executor.clone(),
            foreground_executor: self.foreground_executor.clone(),
        }
    }

    /// Obtains a reference to the executor, which can be used to spawn futures.
    pub const fn background_executor(&self) -> &BackgroundExecutor {
        &self.background_executor
    }

    /// Obtains a reference to the executor, which can be used to spawn futures.
    pub fn foreground_executor(&self) -> &ForegroundExecutor {
        assert!(!self.quitting, "Can't spawn on main thread after on_app_quit");
        &self.foreground_executor
    }

    /// Spawns the future returned by the given function on the main thread. The closure will be invoked
    /// with [AsyncApp], which allows the application state to be accessed across await points.
    #[track_caller]
    #[inline(always)]
    pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        let mut cx = self.prepare_spawn();

        self.foreground_executor.spawn(async move { f(&mut cx).await })
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.push_effect(Effect::Defer { callback: Box::new(f) });
    }

    /// Accessor for the application's asset source, which is provided when constructing the `App`.
    pub fn asset_source(&self) -> &Arc<dyn AssetSource> {
        &self.asset_source
    }

    /// Accessor for the text system.
    pub const fn text_system(&self) -> &Arc<TextSystem> {
        &self.text_system
    }

    /// Check whether a global of the given type has been assigned.
    pub fn has_global<G: Global>(&self) -> bool {
        self.globals_by_type.contains_key(&TypeId::of::<G>())
    }

    /// Access the global of the given type. Panics if a global for that type has not been assigned.
    #[track_caller]
    pub fn global<G: Global>(&self) -> &G {
        self.globals_by_type.get(&TypeId::of::<G>()).map_or_else(
            || panic!("no state of type {} exists", type_name::<G>()),
            |any_state| any_state.downcast_ref::<G>().expect("globals are keyed by their type id"),
        )
    }

    /// Access the global of the given type if a value has been assigned.
    pub fn try_global<G: Global>(&self) -> Option<&G> {
        self.globals_by_type
            .get(&TypeId::of::<G>())
            .map(|any_state| any_state.downcast_ref::<G>().expect("globals are keyed by their type id"))
    }

    /// Access the global of the given type mutably. A default value is assigned if a global of this type has not
    /// yet been assigned.
    pub fn default_global<G: Global + Default>(&mut self) -> &mut G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type
            .entry(global_type)
            .or_insert_with(|| Box::<G>::default())
            .downcast_mut::<G>()
            .expect("globals are keyed by their type id")
    }

    /// Sets the value of the global of the given type.
    pub fn set_global<G: Global>(&mut self, global: G) {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type.insert(global_type, Box::new(global));
    }

    /// Remove the global of the given type from the app context. Does not notify global observers.
    pub fn remove_global<G: Global>(&mut self) -> G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        *self
            .globals_by_type
            .remove(&global_type)
            .unwrap_or_else(|| panic!("no global added for {}", type_name::<G>()))
            .downcast()
            .expect("globals are keyed by their type id")
    }

    /// Register a callback to be invoked when a global of the given type is updated.
    pub fn observe_global<G: Global>(&mut self, mut f: impl FnMut(&mut Self) + 'static) -> Subscription {
        let (subscription, activate) = self.global_observers.insert_pending(
            TypeId::of::<G>(),
            Box::new(move |cx| {
                f(cx);
                true
            }),
        );
        self.defer(move |_| activate());
        subscription
    }

    /// Move the global of the given type to the stack.
    #[track_caller]
    pub(crate) fn lease_global<G: Global>(&mut self) -> GlobalLease<G> {
        GlobalLease::new(
            self.globals_by_type
                .remove(&TypeId::of::<G>())
                .unwrap_or_else(|| panic!("no global registered of type {}", type_name::<G>())),
        )
    }

    /// Restore the global of the given type after it is moved to the stack.
    pub(crate) fn end_global_lease<G: Global>(&mut self, lease: GlobalLease<G>) {
        let global_type = TypeId::of::<G>();

        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type.insert(global_type, lease.global);
    }

    pub(crate) fn new_entity_observer(&self, key: TypeId, value: NewEntityListener) -> Subscription {
        self.new_entity_observers.insert(key, value)
    }

    /// Arrange for the given function to be invoked whenever a view of the specified type is created.
    /// The function will be passed a mutable reference to the view along with an appropriate context.
    pub fn observe_new<T: 'static>(
        &self,
        on_new: impl 'static + Fn(&mut T, Option<&mut Window>, &mut Context<T>),
    ) -> Subscription {
        self.new_entity_observer(
            TypeId::of::<T>(),
            Box::new(move |any_entity: AnyEntity, window: &mut Option<&mut Window>, cx: &mut App| {
                any_entity
                    .downcast::<T>()
                    .expect("observer is registered for this entity type")
                    .update(cx, |entity_state, cx| on_new(entity_state, window.as_deref_mut(), cx));
            }),
        )
    }

    /// Observe the release of a entity. The callback is invoked after the entity
    /// has no more strong references but before it has been dropped.
    pub fn observe_release<T>(
        &self,
        handle: &Entity<T>,
        on_release: impl FnOnce(&mut T, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        self.release_listeners.insert(
            handle.entity_id(),
            Box::new(move |entity, cx| {
                let entity = entity.downcast_mut().expect("invalid entity type");
                on_release(entity, cx);
            }),
        )
    }

    /// Observe the release of a entity. The callback is invoked after the entity
    /// has no more strong references but before it has been dropped.
    pub fn observe_release_in<T>(
        &self,
        handle: &Entity<T>,
        window: &Window,
        on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let window_handle = window.handle;
        self.observe_release(handle, move |entity, cx| {
            window_handle.update(cx, |_, window, cx| on_release(entity, window, cx)).ok();
        })
    }

    /// Register key bindings.
    pub fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        self.keymap.borrow_mut().add_bindings(bindings);
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

    /// Register the global handler for an action type, replacing any previous one. It runs after
    /// the window's bubble phase, so only if there are no other handlers or they called
    /// `cx.propagate()`.
    pub fn on_action<A: Action>(&mut self, listener: impl Fn(&A, &mut Self) + 'static) -> &mut Self {
        self.global_action_listeners.insert(
            TypeId::of::<A>(),
            Rc::new(move |action, cx| listener(action.downcast_ref().expect("actions are keyed by their type id"), cx)),
        );
        self
    }

    /// Event handlers propagate events by default. Call this method to stop dispatching to
    /// event handlers with a lower z-index (mouse) or higher in the tree (keyboard). This is
    /// the opposite of [`Self::propagate`]. It's also possible to cancel a call to [`Self::propagate`] by
    /// calling this method before effects are flushed.
    pub const fn stop_propagation(&mut self) {
        self.propagate_event = false;
    }

    /// Action handlers stop propagation by default during the bubble phase of action dispatch
    /// dispatching to action handlers higher in the element tree. This is the opposite of
    /// [`Self::stop_propagation`]. It's also possible to cancel a call to [`Self::stop_propagation`] by calling
    /// this method before effects are flushed.
    pub const fn propagate(&mut self) {
        self.propagate_event = true;
    }

    /// Builds the action registered under `name`, unless it cannot be built without input.
    pub fn build_action(&self, name: &str) -> Option<Box<dyn Action>> {
        self.actions.build_action(name)
    }

    /// Get all action names that have been registered. Note that registration only allows for
    /// actions to be built dynamically, and is unrelated to binding actions in the element tree.
    pub fn all_action_names(&self) -> &[&'static str] {
        self.actions.all_action_names()
    }

    /// Returns key bindings that invoke the given action on the currently focused element, without
    /// checking context. Bindings are returned in the order they were added. For display, the last
    /// binding should take precedence.
    pub fn all_bindings_for_input(&self, input: &[Keystroke]) -> Vec<KeyBinding> {
        RefCell::borrow(&self.keymap).all_bindings_for_input(input)
    }

    /// Register a callback to be invoked when the application is about to quit.
    /// It is not possible to cancel the quit event at this point.
    #[track_caller]
    pub fn on_app_quit<Fut>(&self, mut on_quit: impl FnMut(&mut App) -> Fut + 'static) -> Subscription
    where
        Fut: 'static + Future<Output = ()>,
    {
        self.quit_observers.insert((
            Box::new(move |cx| {
                let future = on_quit(cx);
                future.boxed_local()
            }),
            std::panic::Location::caller(),
        ))
    }

    /// Checks if the given action is bound in the current context, as defined by the app's current focus,
    /// the bindings in the element tree, and any global action listeners.
    pub fn is_action_available(&mut self, action: &dyn Action) -> bool {
        let action_available = self
            .active_window()
            .and_then(|window| window.update(self, |_, window, cx| window.is_action_available(action, cx)).ok())
            .unwrap_or(false);

        action_available || self.global_action_listeners.contains_key(&action.as_any().type_id())
    }

    /// Sets the menu bar for this application. This will replace any existing menu bar.
    pub fn set_menus(&self, menus: impl IntoIterator<Item = Menu>) {
        let menus: Vec<Menu> = menus.into_iter().collect();
        self.platform.set_menus(menus, &self.keymap.borrow());
    }

    /// Sets the right click menu for the app icon in the dock.
    pub fn set_dock_menu(&self, menus: Vec<MenuItem>) {
        self.platform.set_dock_menu(menus, &self.keymap.borrow());
    }

    /// Dispatch an action to the currently active window or global action handler
    /// See [`crate::Action`] for more information on how actions work.
    pub fn dispatch_action(&mut self, action: &dyn Action) {
        if let Some(active_window) = self.active_window() {
            active_window.update(self, |_, window, cx| window.dispatch_action(action.boxed_clone(), cx)).log_err();
        } else {
            self.dispatch_global_action(action);
        }
    }

    fn dispatch_global_action(&mut self, action: &dyn Action) {
        self.propagate_event = true;
        if let Some(listener) = self.global_action_listeners.get(&action.as_any().type_id()).cloned() {
            listener(action.as_any(), self);
        }
    }

    /// Is there currently something being dragged?
    pub const fn has_active_drag(&self) -> bool {
        self.active_drag.is_some()
    }

    /// Gets the cursor style of the currently active drag operation.
    pub fn active_drag_cursor_style(&self) -> Option<CursorStyle> {
        let drag = self.active_drag.as_ref()?;
        drag.cursor_style
    }

    /// Stops active drag and clears any related effects.
    pub fn stop_active_drag(&mut self, window: &mut Window) -> bool {
        if self.active_drag.is_some() {
            self.active_drag = None;
            window.refresh();
            true
        } else {
            false
        }
    }

    /// Sets the cursor style for the currently active drag operation.
    pub fn set_active_drag_cursor_style(&mut self, cursor_style: CursorStyle, window: &mut Window) -> bool {
        if let Some(ref mut drag) = self.active_drag {
            drag.cursor_style = Some(cursor_style);
            window.refresh();
            true
        } else {
            false
        }
    }

    /// Remove an asset from GPUI's cache.
    pub fn remove_asset<A: Asset>(&mut self, source: &A::Source) {
        let asset_id = (TypeId::of::<A>(), hash(source));
        self.loading_assets.remove(&asset_id);
    }

    /// Starts loading an uncached asset and returns its result once available.
    ///
    /// Pending loads and completed results are cached until [`Self::remove_asset`].
    /// This method does not subscribe a view to completion notifications.
    pub fn fetch_asset<A: Asset>(&mut self, source: &A::Source) -> Option<A::Output> {
        self.asset_entry::<A>(source).get()
    }

    pub(crate) fn asset_entry<A: Asset>(&mut self, source: &A::Source) -> &CachedLoad<A::Output> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        if !self.loading_assets.contains_key(&asset_id) {
            let future = A::load(source.clone(), self);
            let entry = CachedLoad::new(future, self);
            self.loading_assets.insert(asset_id, Box::new(entry));
        }
        self.loading_assets
            .get(&asset_id)
            .and_then(|entry| entry.downcast_ref())
            .expect("asset cache entries are keyed by their asset type")
    }

    /// Obtain a new [`FocusHandle`], which allows you to track and manipulate the keyboard focus
    /// for elements rendered within this window.
    #[track_caller]
    pub fn focus_handle(&self) -> FocusHandle {
        FocusHandle::new(&self.focus_handles)
    }

    /// Tell GPUI that an entity has changed and observers of it should be notified.
    pub fn notify(&mut self, entity_id: EntityId) {
        let window_invalidators = mem::take(self.window_invalidators_by_entity.entry(entity_id).or_default());

        // `window_invalidators_by_entity` is monotonic, so an entry alone
        // doesn't mean the window is currently rendering the entity. Filter
        // through `tracked_entities` to keep invalidation tight to windows
        // that actually display this entity right now.
        let live_invalidators: SmallVec<[WindowInvalidator; 2]> = window_invalidators
            .iter()
            .filter(|(window_id, _)| self.tracked_entities.get(window_id).is_some_and(|set| set.contains(&entity_id)))
            .map(|(_, invalidator)| invalidator.clone())
            .collect();

        if live_invalidators.is_empty() {
            if self.pending_notifications.insert(entity_id) {
                self.pending_effects.push_back(Effect::Notify { emitter: entity_id });
            }
        } else {
            for invalidator in &live_invalidators {
                invalidator.invalidate_view(entity_id, self);
            }
        }

        self.window_invalidators_by_entity.insert(entity_id, window_invalidators);
    }

    /// Removes an image from the sprite atlas on all windows.
    ///
    /// If the current window is being updated, it will be removed from `App.windows`, you can use `current_window` to specify the current window.
    /// This is a no-op if the image is not in the sprite atlas.
    pub fn drop_image(&mut self, image: Arc<RenderImage>, current_window: Option<&mut Window>) {
        // remove the texture from all other windows
        for window in self.windows.values_mut().flatten() {
            _ = window.drop_image(Arc::clone(&image));
        }

        // remove the texture from the current window
        if let Some(window) = current_window {
            _ = window.drop_image(image);
        }
    }

    #[inline(never)]
    fn update_window_erased(
        &mut self,
        window_id: WindowId,
        update: &mut dyn FnMut(Option<(AnyView, &mut Window, &mut App)>),
    ) {
        self.update(|cx| {
            let Some(mut window) = cx.windows.get_mut(window_id).and_then(Option::take) else {
                update(None);
                return;
            };

            let root_view = window.root.clone().expect("window has a root view");

            cx.window_update_stack.push(window.handle.id);
            update(Some((root_view, &mut window, cx)));
            fn trail(window_id: WindowId, window: Box<Window>, cx: &mut App) -> Option<()> {
                cx.window_update_stack.pop();

                if window.removed {
                    cx.window_handles.remove(&window_id);
                    cx.windows.remove(window_id);
                    if let Some(tracked) = cx.tracked_entities.remove(&window_id) {
                        for entity_id in tracked {
                            if let Some(windows) = cx.window_invalidators_by_entity.get_mut(&entity_id) {
                                windows.remove(&window_id);
                            }
                            if cx.current_window_by_entity.get(&entity_id) == Some(&window_id) {
                                cx.current_window_by_entity.remove(&entity_id);
                            }
                        }
                    }

                    if cx.windows.is_empty() {
                        cx.quit();
                    }
                } else {
                    cx.windows.get_mut(window_id)?.replace(window);
                }
                Some(())
            }
            if trail(window_id, window, cx).is_none() {
                update(None);
            }
        });
    }

    #[inline(never)]
    fn update_entity_erased(
        &mut self,
        handle: &AnyEntity,
        entity_type: &str,
        update: &mut dyn FnMut(&mut dyn Any, &mut App),
    ) {
        self.update(|cx| {
            let mut lease = cx.entities.lease_erased(handle, entity_type);
            update(lease.entity.as_deref_mut().expect("entity is leased"), cx);
            cx.entities.end_lease_erased(handle.entity_id, lease);
        });
    }

    #[inline(never)]
    #[track_caller]
    fn prepare_spawn(&self) -> AsyncApp {
        if self.quitting {
            log::error!("Can't spawn on main thread after on_app_quit");
        }
        self.to_async()
    }
}

impl AppContext for App {
    /// Builds an entity that is owned by the application.
    ///
    /// The given function will be invoked with a [`Context`] and must return an object representing the entity. An
    /// [`Entity`] handle will be returned, which can be used to access the entity in a context.
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        self.update(|cx| {
            let slot = cx.entities.reserve();
            let handle = slot.clone();
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));

            cx.push_effect(Effect::EntityCreated {
                entity: handle.into_any(),
                tid: TypeId::of::<T>(),
                window: cx.window_update_stack.last().copied(),
            });

            cx.entities.insert(slot, entity)
        })
    }

    fn reserve_entity<T: 'static>(&mut self) -> Reservation<T> {
        Reservation(self.entities.reserve())
    }

    fn insert_entity<T: 'static>(
        &mut self,
        reservation: Reservation<T>,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T> {
        self.update(|cx| {
            let slot = reservation.0;
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));
            cx.entities.insert(slot, entity)
        })
    }

    /// Updates the entity referenced by the given handle. The function is passed a mutable reference to the
    /// entity along with a `Context` for the entity.
    #[inline(always)]
    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let mut update = Some(update);
        let mut result = None;
        self.update_entity_erased(handle, type_name::<T>(), &mut |entity, cx| {
            let value = update.take().expect("update is called once")(
                entity.downcast_mut::<T>().expect("entity type matches its handle"),
                &mut Context::new_context(cx, handle.downgrade()),
            );
            result = Some(value);
        });
        result.expect("update was called")
    }

    fn as_mut<'a, T>(&'a mut self, handle: &Entity<T>) -> GpuiBorrow<'a, T>
    where
        T: 'static,
    {
        GpuiBorrow::new(handle.clone(), self)
    }

    #[inline(always)]
    fn read_entity<T, R>(&self, handle: &Entity<T>, read: impl FnOnce(&T, &App) -> R) -> R
    where
        T: 'static,
    {
        let entity = self.entities.read(handle);
        read(entity, self)
    }

    fn update_window<T, F>(&mut self, handle: AnyWindowHandle, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.update_window_id(handle.id, update)
    }

    fn with_window<R>(&mut self, entity_id: EntityId, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R> {
        App::with_window(self, entity_id, f)
    }

    fn read_window<T, R>(&self, window: &WindowHandle<T>, read: impl FnOnce(Entity<T>, &App) -> R) -> Result<R>
    where
        T: 'static,
    {
        let window = self
            .windows
            .get(window.id)
            .context("window not found")?
            .as_deref()
            .expect("attempted to read a window that is already on the stack");

        let root_view = window.root.clone().expect("window has a root view");
        #[expect(clippy::map_err_ignore, reason = "the error is the unmatched view itself")]
        let view = root_view.downcast::<T>().map_err(|_| anyhow!("root view's type has changed"))?;

        Ok(read(view, self))
    }

    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_executor.spawn(future)
    }

    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> R
    where
        G: Global,
    {
        let g = self.global::<G>();
        callback(g, self)
    }
}

/// These effects are processed at the end of each application update cycle.
pub(crate) enum Effect {
    Notify { emitter: EntityId },
    Emit { emitter: EntityId, event_type: TypeId, event: ArenaBox<dyn Any> },
    RefreshWindows,
    NotifyGlobalObservers { global_type: TypeId },
    Defer { callback: Box<dyn FnOnce(&mut App) + 'static> },
    EntityCreated { entity: AnyEntity, tid: TypeId, window: Option<WindowId> },
}

impl Debug for Effect {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Notify { emitter } => write!(f, "Notify({emitter})"),
            Effect::Emit { emitter, .. } => write!(f, "Emit({emitter:?})"),
            Effect::RefreshWindows => write!(f, "RefreshWindows"),
            Effect::NotifyGlobalObservers { global_type } => {
                write!(f, "NotifyGlobalObservers({global_type:?})")
            }
            Effect::Defer { .. } => write!(f, "Defer(..)"),
            Effect::EntityCreated { entity, .. } => write!(f, "EntityCreated({entity:?})"),
        }
    }
}

/// Wraps a global variable value during `update_global` while the value has been moved to the stack.
pub(crate) struct GlobalLease<G: Global> {
    global: Box<dyn Any>,
    global_type: PhantomData<G>,
}

impl<G: Global> GlobalLease<G> {
    fn new(global: Box<dyn Any>) -> Self {
        GlobalLease { global, global_type: PhantomData }
    }
}

impl<G: Global> Deref for GlobalLease<G> {
    type Target = G;

    fn deref(&self) -> &Self::Target {
        self.global.downcast_ref().expect("globals are keyed by their type id")
    }
}

impl<G: Global> DerefMut for GlobalLease<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.global.downcast_mut().expect("globals are keyed by their type id")
    }
}

/// Contains state associated with an active drag operation, started by dragging an element
/// within the window.
pub struct AnyDrag {
    /// The view used to render this drag.
    pub view: AnyView,

    /// The value of the dragged item, to be dropped.
    pub value: Arc<dyn Any>,

    /// This is used to render the dragged item in the same place
    /// on the original element that the drag was initiated.
    pub cursor_offset: Point<Pixels>,

    /// The cursor style to use while dragging.
    pub cursor_style: Option<CursorStyle>,
}

/// Contains state associated with a tooltip. You'll only need this struct if you're implementing
/// tooltip behavior on a custom element. Otherwise, use [Div::tooltip](crate::Interactivity::tooltip).
#[derive(Clone)]
pub struct AnyTooltip {
    /// The view used to display the tooltip.
    pub view: AnyView,

    /// The absolute position of the mouse when the tooltip was deployed.
    pub mouse_position: Point<Pixels>,

    /// Given the bounds of the tooltip, checks whether the tooltip should still be visible and
    /// updates its state accordingly. This is needed atop the hovered element's mouse move handler
    /// to handle the case where the element is not painted (e.g. via use of `visible_on_hover`).
    pub check_visible_and_update: Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App) -> bool>,
}

/// Whether a keystroke should prefer character input or key bindings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputPreference {
    /// Prefer typing text over triggering key bindings.
    CharacterInput,
    /// Dispatch key bindings normally, if any match.
    KeyBindings,
}

/// A mutable reference to an entity owned by GPUI.
pub struct GpuiBorrow<'a, T> {
    inner: Option<Lease<T>>,
    app: &'a mut App,
}

impl<'a, T: 'static> GpuiBorrow<'a, T> {
    fn new(inner: Entity<T>, app: &'a mut App) -> Self {
        app.start_update();
        let lease = app.entities.lease(&inner);
        Self { inner: Some(lease), app }
    }
}

impl<T: 'static> borrow::Borrow<T> for GpuiBorrow<'_, T> {
    fn borrow(&self) -> &T {
        self.inner.as_ref().expect("borrow is live until drop").borrow()
    }
}

impl<T: 'static> borrow::BorrowMut<T> for GpuiBorrow<'_, T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.inner.as_mut().expect("borrow is live until drop").borrow_mut()
    }
}

impl<T: 'static> ops::Deref for GpuiBorrow<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("borrow is live until drop")
    }
}

impl<T: 'static> ops::DerefMut for GpuiBorrow<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner.as_mut().expect("borrow is live until drop")
    }
}

impl<T> Drop for GpuiBorrow<'_, T> {
    fn drop(&mut self) {
        let lease = self.inner.take().expect("borrow is live until drop");
        self.app.notify(lease.id);
        self.app.entities.end_lease(lease);
        self.app.finish_update();
    }
}
