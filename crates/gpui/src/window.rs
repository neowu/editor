use std::any;
use std::any::Any;
use std::any::TypeId;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp;
use std::fmt::Debug;
use std::hash::Hash;
use std::mem;
use std::ops::Range;
use std::path;
use std::ptr;
use std::rc::Rc;
use std::result;
use std::sync::Arc;
use std::thread;
use std::thread::AccessError;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use futures::channel::oneshot;
use gpui_util::ResultExt as _;
use gpui_util::post_inc;
use itertools::FoldWhile::Continue;
use itertools::FoldWhile::Done;
use itertools::Itertools as _;
use raw_window_handle::HandleError;
use raw_window_handle::HasDisplayHandle;
use raw_window_handle::HasWindowHandle;
use refineable::Refineable as _;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

use crate::Action;
use crate::ActionTable;
use crate::AnyElement;
use crate::AnyImageCache;
use crate::AnyTooltip;
use crate::AnyView;
use crate::App;
use crate::AppContext as _;
use crate::Arena;
use crate::Asset;
use crate::AsyncWindowContext;
use crate::AvailableSpace;
use crate::Bounds;
use crate::Capslock;
use crate::Context;
use crate::CursorStyle;
use crate::DispatchActionTable;
use crate::DispatchNodeId;
use crate::DispatchTree;
use crate::DisplayId;
use crate::Effect;
use crate::Entity;
use crate::EntityId;
use crate::EventEmitter;
use crate::FrameStats;
use crate::Global;
use crate::GlobalElementId;
use crate::InputHandler;
use crate::IsZero as _;
use crate::KeyContext;
use crate::KeyEvent;
use crate::LayoutId;
use crate::LineLayoutIndex;
use crate::Modifiers;
use crate::ModifiersChangedEvent;
use crate::MouseEvent;
use crate::Pixels;
use crate::PlatformAtlas;
use crate::PlatformDisplay;
use crate::PlatformInputHandler;
use crate::PlatformWindow;
use crate::Point;
use crate::PromptButton;
use crate::PromptLevel;
use crate::Render;
use crate::ScaledPixels;
use crate::Scene;
use crate::SingleSubscriberSet;
use crate::Size;
use crate::Style;
use crate::Subscription;
use crate::TabStopMap;
use crate::TaffyLayoutEngine;
use crate::Task;
use crate::TextStyle;
use crate::TextStyleRefinement;
use crate::UiElement;
use crate::UiText;
use crate::WindowAppearance;
use crate::WindowBackgroundAppearance;
use crate::WindowBounds;
use crate::WindowOptions;
use crate::WindowParams;
use crate::WindowTextSystem;
use crate::WindowVisibility;
use crate::point;
use crate::prelude::IntoElement as _;
use crate::px;
use crate::size;

mod dispatch;
mod element_id;
mod focus;
mod handle;
mod hitbox;
mod paint;
mod prompts;

pub use dispatch::DispatchEventResult;
use dispatch::PendingInput;
pub use element_id::ElementId;
pub(crate) use element_id::ElementIdStack;
pub use focus::DismissEvent;
pub use focus::FocusHandle;
pub use focus::FocusId;
pub(crate) use focus::FocusMap;
pub use focus::Focusable;
pub use focus::ManagedView;
pub use focus::WeakFocusHandle;
pub use handle::AnyWindowHandle;
pub use handle::WindowHandle;
pub use handle::WindowId;
pub(crate) use hitbox::CursorStyleRequest;
pub(crate) use hitbox::HitTest;
pub use hitbox::Hitbox;
pub use hitbox::HitboxBehavior;
pub use hitbox::HitboxId;
pub use hitbox::WindowControlArea;
pub use paint::PaintQuad;
pub use paint::fill;
pub use paint::outline;
pub use paint::quad;
#[allow(clippy::wildcard_imports)]
pub use prompts::*;

/// Default window size used when no explicit size is provided.
/// How often a window presents again to advance its scene's animated sprites
/// (rotating spinners) when nothing else draws. A spinner can show for minutes
/// while a language server indexes, so this is well below the display rate.
const SPRITE_ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(100);

pub const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(1095.));

/// Represents the two different phases when dispatching events.
#[derive(Default, Copy, Clone, Debug, Eq, PartialEq)]
pub enum DispatchPhase {
    /// After the capture phase comes the bubble phase, in which mouse event listeners are
    /// invoked front to back and keyboard event listeners are invoked from the focused element
    /// to the root of the element tree. This is the phase you'll most commonly want to use when
    /// registering event listeners.
    #[default]
    Bubble,
    /// During the initial capture phase, mouse event listeners are invoked back to front, and keyboard
    /// listeners are invoked from the root of the tree downward toward the focused element. This phase
    /// is used for special purposes such as clearing the "pressed" state for click events. If
    /// you stop event propagation during this phase, you need to know what you're doing. Handlers
    /// outside of the immediate region may rely on detecting non-local events during this phase.
    Capture,
}

impl DispatchPhase {
    /// Returns true if this represents the "bubble" phase.
    #[inline]
    pub fn bubble(self) -> bool {
        self == DispatchPhase::Bubble
    }

    /// Returns true if this represents the "capture" phase.
    #[inline]
    pub fn capture(self) -> bool {
        self == DispatchPhase::Capture
    }
}

struct WindowInvalidatorInner {
    pub dirty: bool,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
    pub update_count: usize,
    pub platform_waker: Option<Rc<dyn Fn()>>,
}

#[derive(Clone)]
pub(crate) struct WindowInvalidator {
    inner: Rc<RefCell<WindowInvalidatorInner>>,
}

impl WindowInvalidator {
    pub(crate) fn new() -> Self {
        WindowInvalidator {
            inner: Rc::new(RefCell::new(WindowInvalidatorInner {
                dirty: true,
                draw_phase: DrawPhase::None,
                dirty_views: FxHashSet::default(),
                update_count: 0,
                platform_waker: None,
            })),
        }
    }

    pub(crate) fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.update_count += 1;
        inner.dirty_views.insert(entity);
        if inner.draw_phase == DrawPhase::None {
            let became_dirty = !inner.dirty;
            inner.dirty = true;
            let waker = became_dirty.then(|| inner.platform_waker.clone()).flatten();
            drop(inner);
            cx.push_effect(Effect::Notify { emitter: entity });
            if let Some(waker) = waker {
                waker();
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    pub(crate) fn set_dirty(&self, dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        let became_dirty = dirty && !inner.dirty;
        inner.dirty = dirty;
        if dirty {
            inner.update_count += 1;
        }
        let waker = became_dirty.then(|| inner.platform_waker.clone()).flatten();
        drop(inner);
        if let Some(waker) = waker {
            waker();
        }
    }

    pub(crate) fn set_platform_waker(&self, waker: Option<Rc<dyn Fn()>>) {
        let mut inner = self.inner.borrow_mut();
        inner.platform_waker = waker;
        let waker = inner.dirty.then(|| inner.platform_waker.clone()).flatten();
        drop(inner);
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Wakes the platform's frame-request source so a frame request is
    /// delivered even if the platform stops requesting frames for idle
    /// windows. No-op on platforms without a frame waker.
    pub(crate) fn wake_platform(&self) {
        let waker = self.inner.borrow().platform_waker.clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    pub(crate) fn set_phase(&self, phase: DrawPhase) {
        self.inner.borrow_mut().draw_phase = phase;
    }

    pub(crate) fn update_count(&self) -> usize {
        self.inner.borrow().update_count
    }

    pub(crate) fn take_views(&self) -> FxHashSet<EntityId> {
        mem::take(&mut self.inner.borrow_mut().dirty_views)
    }

    pub(crate) fn replace_views(&self, views: FxHashSet<EntityId>) {
        self.inner.borrow_mut().dirty_views = views;
    }

    pub(crate) fn not_drawing(&self) -> bool {
        self.inner.borrow().draw_phase == DrawPhase::None
    }
}

type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type AnyWindowFocusListener = Box<dyn FnMut(&WindowFocusEvent, &mut Window, &mut App) -> bool + 'static>;

pub(crate) struct WindowFocusEvent {
    pub(crate) previous_focus_path: SmallVec<[FocusId; 8]>,
    pub(crate) current_focus_path: SmallVec<[FocusId; 8]>,
}

impl WindowFocusEvent {
    pub(crate) fn is_focus_in(&self, focus_id: FocusId) -> bool {
        !self.previous_focus_path.contains(&focus_id) && self.current_focus_path.contains(&focus_id)
    }

    pub(crate) fn is_focus_out(&self, focus_id: FocusId) -> bool {
        self.previous_focus_path.contains(&focus_id) && !self.current_focus_path.contains(&focus_id)
    }
}

/// This is provided when subscribing for `Context::on_focus_out` events.
pub struct FocusOutEvent {
    /// A weak focus handle representing what was blurred.
    pub blurred: WeakFocusHandle,
}

thread_local! {
    /// Fallback arena used when no app-specific arena is active.
    /// In production, each window draw sets CURRENT_ELEMENT_ARENA to the app's arena.
    pub(crate) static ELEMENT_ARENA: RefCell<Arena> = RefCell::new(Arena::new(1024 * 1024));

    /// Points to the current App's element arena during draw operations.
    /// This allows multiple test Apps to have isolated arenas, preventing
    /// cross-session corruption when the scheduler interleaves their tasks.
    static CURRENT_ELEMENT_ARENA: Cell<Option<*const RefCell<Arena>>> = const { Cell::new(None) };
}

/// Whether a window draw is currently in progress on this thread.
///
/// This holds exactly while an `ElementArenaScope` is active: nested scopes
/// restore the previous (still set) arena pointer, so `CURRENT_ELEMENT_ARENA`
/// is `Some` from the outermost draw's start to its end.
///
/// The `on_request_frame` callback uses this to defer draw requests that
/// arrive re-entrantly while a draw is already on the stack (e.g. via nested
/// message pumping in the Windows window procedure), instead of running a
/// nested draw or panicking on the already-borrowed App.
pub(crate) fn draw_in_progress() -> bool {
    CURRENT_ELEMENT_ARENA.with(|current| current.get().is_some())
}

/// Allocates an element in the current arena. Uses the app-specific arena if one
/// is active (during draw), otherwise falls back to the thread-local ELEMENT_ARENA.
#[inline(always)]
pub(crate) fn with_element_arena<R>(callback: impl FnOnce(&mut Arena) -> R) -> R {
    let mut callback = Some(callback);
    let mut result = None;
    let access = with_element_arena_erased(&mut |arena| {
        result = Some(callback.take().expect("arena callback runs once")(arena));
    });
    drop(callback);
    access.expect("cannot access a Thread Local Storage value during or after destruction");
    result.expect("arena callback produces a result")
}

/// Scope guard that sets CURRENT_ELEMENT_ARENA for the duration of a draw
/// operation and tracks the arena's scope depth, so that a nested draw's
/// `ArenaClearNeeded::clear` is deferred rather than freeing memory the outer
/// draw still references (see `Arena::clear`).
///
/// Call [`ElementArenaScope::exit`] with the same arena that was entered to
/// obtain the [`ArenaClearNeeded`] token the draw now owes; requiring `exit`
/// makes it impossible to request a clear before the scope has ended. The
/// scope's teardown — restoring the thread-local and balancing `begin_scope`
/// with `end_scope` — happens in `Drop`, so the arena's scope depth stays
/// balanced on every path, including when a panic unwinds a draw before `exit`
/// is reached. (If teardown lived only in `exit`, such a panic would leave the
/// scope depth permanently elevated and defer every future clear, leaking
/// memory unboundedly.)
pub(crate) struct ElementArenaScope {
    /// The entered arena: compared against the argument in `exit`, and
    /// dereferenced in `Drop` to end its scope (see the SAFETY note there).
    entered: *const RefCell<Arena>,
    previous: Option<*const RefCell<Arena>>,
    exited: bool,
}

impl ElementArenaScope {
    /// Enter a scope where element allocations use the given arena.
    pub(crate) fn enter(arena: &RefCell<Arena>) -> Self {
        arena.borrow_mut().begin_scope();
        let previous = CURRENT_ELEMENT_ARENA.with(|current| {
            let prev = current.get();
            current.set(Some(std::ptr::from_ref::<RefCell<Arena>>(arena)));
            prev
        });
        Self { entered: std::ptr::from_ref::<RefCell<Arena>>(arena), previous, exited: false }
    }

    /// End the scope: restores the previously-current arena and ends the
    /// arena's clear-deferral scope. Returns the token for the arena clear the
    /// draw now owes; producing it here makes it impossible to request a clear
    /// before the scope has ended (which would be silently deferred forever).
    ///
    /// Panics if passed a different arena than was entered: ending the scope
    /// of the wrong arena would unbalance two arenas' scope depths, allowing
    /// one of them to clear while a draw still references its memory.
    pub(crate) fn exit(mut self, arena: &RefCell<Arena>) -> ArenaClearNeeded {
        assert!(ptr::eq(self.entered, arena), "ElementArenaScope::exit called with a different arena than was entered");
        self.exited = true;
        // Teardown (restoring the thread-local and ending the arena's
        // clear-deferral scope) runs in `Drop`, which fires both here — `self`
        // is dropped as `exit` returns, before the token reaches the caller —
        // and when a panic unwinds the draw before `exit` is reached.
        ArenaClearNeeded::new(arena)
    }
}

impl Drop for ElementArenaScope {
    fn drop(&mut self) {
        // Teardown lives here (rather than in `exit`) so it runs exactly once on
        // every path: `exit` consumes and drops the guard on the normal path,
        // and unwinding drops it on the panic path. Balancing `begin_scope` here
        // keeps the arena's scope depth correct even when a draw panics; if this
        // only happened in `exit`, a panic between `enter` and `exit` would leave
        // the depth elevated and defer every future clear.
        CURRENT_ELEMENT_ARENA.with(|current| {
            current.set(self.previous);
        });
        // SAFETY: `entered` came from a `&RefCell<Arena>` in `enter`, and the
        // arena (owned by the `App` being drawn) outlives this guard on both the
        // normal and unwinding paths, since the guard is a local of the draw.
        unsafe { &*self.entered }.borrow_mut().end_scope();
        if !self.exited && !thread::panicking() {
            log::error!(
                "ElementArenaScope dropped without calling exit(); \
                 the arena clear for this draw was never requested"
            );
        }
    }
}

/// Returned when the element arena has been used and so must be cleared before the next draw.
#[must_use]
pub struct ArenaClearNeeded {
    /// Identity of the arena that was drawn into. Only ever compared against
    /// another pointer in `clear`; never dereferenced.
    arena: *const RefCell<Arena>,
}

impl ArenaClearNeeded {
    /// Create a new ArenaClearNeeded token for the App whose arena was drawn
    /// into. Private: the only way to obtain one is [`ElementArenaScope::exit`].
    const fn new(arena: &RefCell<Arena>) -> Self {
        Self { arena: std::ptr::from_ref::<RefCell<Arena>>(arena) }
    }

    /// Clear the element arena of the App the draw ran against. If an enclosing
    /// draw is still in progress (this draw was nested inside it), the clear is
    /// deferred to the enclosing draw's own `ArenaClearNeeded` so that its live
    /// allocations aren't freed.
    ///
    /// Panics if passed a different App than the draw ran against, since
    /// clearing another App's arena could free memory its draws still
    /// reference.
    pub fn clear(self, cx: &mut App) {
        assert!(
            ptr::eq(self.arena, &raw const *cx.element_arena),
            "ArenaClearNeeded::clear called with a different App than the draw ran against"
        );
        RefCell::borrow_mut(&cx.element_arena).clear();
    }
}

type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;

pub(crate) type AnyMouseListener = Box<dyn FnMut(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static>;

/// An identifier for a tooltip.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct TooltipId(usize);

impl TooltipId {
    /// Checks if the tooltip is currently hovered.
    pub fn is_hovered(&self, window: &Window) -> bool {
        window.tooltip_bounds.as_ref().is_some_and(|tooltip_bounds| {
            tooltip_bounds.id == *self && tooltip_bounds.bounds.contains(&window.mouse_position())
        })
    }
}

pub(crate) struct TooltipBounds {
    id: TooltipId,
    bounds: Bounds<Pixels>,
}

#[derive(Clone)]
pub(crate) struct TooltipRequest {
    id: TooltipId,
    tooltip: AnyTooltip,
}

pub(crate) struct DeferredDraw {
    current_view: EntityId,
    priority: usize,
    parent_node: DispatchNodeId,
    element_id_stack: ElementIdStack,
    text_style_stack: Vec<TextStyleRefinement>,
    content_mask: Option<ContentMask<Pixels>>,
    rem_size: Pixels,
    element: Option<AnyElement>,
    absolute_offset: Point<Pixels>,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    /// The entities the deferred element accessed while it was last drawn, in
    /// any phase, restored when its prepaint is reused.
    accessed_entities: FxHashSet<EntityId>,
}

pub(crate) struct Frame {
    pub(crate) focus: Option<FocusId>,
    pub(crate) window_active: bool,
    pub(crate) element_states: FxHashMap<(GlobalElementId, TypeId), ElementStateBox>,
    accessed_element_states: Vec<(GlobalElementId, TypeId)>,
    pub(crate) mouse_listeners: Vec<Option<AnyMouseListener>>,
    pub(crate) dispatch_tree: DispatchTree,
    pub(crate) scene: Scene,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(crate) window_control_hitboxes: Vec<(WindowControlArea, Hitbox)>,
    pub(crate) deferred_draws: Vec<DeferredDraw>,
    pub(crate) input_handlers: Vec<Option<PlatformInputHandler>>,
    pub(crate) tooltip_requests: Vec<Option<TooltipRequest>>,
    pub(crate) cursor_styles: Vec<CursorStyleRequest>,
    pub(crate) tab_stops: TabStopMap,
    pub(crate) ui_elements: Vec<UiElement>,
    pub(crate) ui_texts: Vec<UiText>,
}

#[derive(Clone, Default)]
pub(crate) struct PrepaintStateIndex {
    hitboxes_index: usize,
    tooltips_index: usize,
    deferred_draws_index: usize,
    dispatch_tree_index: usize,
    accessed_element_states_index: usize,
    line_layout_index: LineLayoutIndex,
}

#[derive(Clone, Default)]
pub(crate) struct PaintIndex {
    scene_index: usize,
    mouse_listeners_index: usize,
    input_handlers_index: usize,
    cursor_styles_index: usize,
    accessed_element_states_index: usize,
    tab_handle_index: usize,
    ui_elements_index: usize,
    ui_texts_index: usize,
    line_layout_index: LineLayoutIndex,
}

impl Frame {
    pub(crate) fn new(dispatch_tree: DispatchTree) -> Self {
        Frame {
            focus: None,
            window_active: false,
            element_states: FxHashMap::default(),
            accessed_element_states: Vec::new(),
            mouse_listeners: Vec::new(),
            dispatch_tree,
            scene: Scene::default(),
            hitboxes: Vec::new(),
            window_control_hitboxes: Vec::new(),
            deferred_draws: Vec::new(),
            input_handlers: Vec::new(),
            tooltip_requests: Vec::new(),
            cursor_styles: Vec::new(),

            tab_stops: TabStopMap::default(),
            ui_elements: Vec::new(),
            ui_texts: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.element_states.clear();
        self.accessed_element_states.clear();
        self.mouse_listeners.clear();
        self.dispatch_tree.clear();
        self.scene.clear();
        self.input_handlers.clear();
        self.tooltip_requests.clear();
        self.cursor_styles.clear();
        self.hitboxes.clear();
        self.window_control_hitboxes.clear();
        self.deferred_draws.clear();
        self.tab_stops.clear();
        self.ui_elements.clear();
        self.ui_texts.clear();
        self.focus = None;
    }

    pub(crate) fn cursor_style(&self, window: &Window) -> Option<CursorStyle> {
        self.cursor_styles
            .iter()
            .rev()
            .fold_while(None, |style, request| match request.hitbox_id {
                None => Done(Some(request.style)),
                Some(hitbox_id) => Continue(
                    style.or_else(|| hitbox_id.is_hovered_ignoring_last_input(window).then_some(request.style)),
                ),
            })
            .into_inner()
    }

    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> HitTest {
        let mut set_hover_hitbox_count = false;
        let mut hit_test = HitTest::default();
        for hitbox in self.hitboxes.iter().rev() {
            let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            if bounds.contains(&position) {
                hit_test.ids.push(hitbox.id);
                if !set_hover_hitbox_count && hitbox.behavior == HitboxBehavior::BlockMouseExceptScroll {
                    hit_test.hover_hitbox_count = hit_test.ids.len();
                    set_hover_hitbox_count = true;
                }
                if hitbox.behavior == HitboxBehavior::BlockMouse {
                    break;
                }
            }
        }
        if !set_hover_hitbox_count {
            hit_test.hover_hitbox_count = hit_test.ids.len();
        }
        hit_test
    }

    pub(crate) fn focus_path(&self) -> SmallVec<[FocusId; 8]> {
        self.focus.map_or_default(|focus_id| self.dispatch_tree.focus_path(focus_id))
    }

    pub(crate) fn finish(&mut self, prev_frame: &mut Self) {
        for element_state_key in &self.accessed_element_states {
            if let Some((element_state_key, element_state)) = prev_frame.element_states.remove_entry(element_state_key)
            {
                self.element_states.insert(element_state_key, element_state);
            }
        }

        self.scene.finish();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
enum InputModality {
    Mouse,
    Keyboard,
}

/// Holds the state for a specific window.
pub struct Window {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) invalidator: WindowInvalidator,
    pub(crate) removed: bool,
    pub(crate) platform_window: Box<dyn PlatformWindow>,
    pub(crate) display_id: Option<DisplayId>,
    is_resizable: bool,
    is_minimizable: bool,
    sprite_atlas: Arc<dyn PlatformAtlas>,
    text_system: Rc<WindowTextSystem>,
    rem_size: Pixels,
    /// The stack of override values for the window's rem size.
    ///
    /// This is used by `with_rem_size` to allow rendering an element tree with
    /// a given rem size.
    rem_size_override_stack: SmallVec<[Pixels; 8]>,
    pub(crate) viewport_size: Size<Pixels>,
    layout_engine: Option<TaffyLayoutEngine>,
    pub(crate) root: Option<AnyView>,
    pub(crate) element_id_stack: ElementIdStack,
    pub(crate) text_style_stack: Vec<TextStyleRefinement>,
    pub(crate) rendered_entity_stack: Vec<EntityId>,
    pub(crate) element_offset_stack: Vec<Point<Pixels>>,
    pub(crate) element_opacity: f32,
    pub(crate) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) requested_autoscroll: Option<Bounds<Pixels>>,
    pub(crate) image_cache_stack: Vec<AnyImageCache>,
    pub(crate) rendered_frame: Frame,
    pub(crate) next_frame: Frame,
    next_hitbox_id: HitboxId,
    pub(crate) next_tooltip_id: TooltipId,
    pub(crate) tooltip_bounds: Option<TooltipBounds>,
    pub(crate) next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>>,
    pub(crate) dirty_views: FxHashSet<EntityId>,
    focus_listeners: SingleSubscriberSet<AnyWindowFocusListener>,
    pub(crate) focus_lost_listeners: SingleSubscriberSet<AnyObserver>,
    focus_lost_path: SmallVec<[FocusId; 8]>,
    default_prevented: bool,
    mouse_position: Point<Pixels>,
    mouse_hit_test: HitTest,
    modifiers: Modifiers,
    capslock: Capslock,
    scale_factor: f32,
    pub(crate) bounds_observers: SingleSubscriberSet<AnyObserver>,
    appearance: WindowAppearance,
    pub(crate) appearance_observers: SingleSubscriberSet<AnyObserver>,
    active: Rc<Cell<bool>>,
    visibility: WindowVisibility,
    pub(crate) needs_present: Rc<Cell<bool>>,
    /// Whether the timer for the next present of the scene's animated sprites is running.
    animation_frame_pending: Rc<Cell<bool>>,
    /// Collected only after `Window::reset_frame_stats`.
    pub(crate) frame_stats: Option<Box<FrameStats>>,
    /// Tracks recent input event timestamps to determine if input is arriving at a high rate.
    /// Used to selectively enable VRR optimization only when input rate exceeds 60fps.
    pub(crate) input_rate_tracker: Rc<RefCell<InputRateTracker>>,
    last_input_modality: InputModality,
    pub(crate) refreshing: bool,
    /// The number of the frame being drawn, or of the last one drawn.
    pub(crate) frame_number: u64,
    pub(crate) activation_observers: SingleSubscriberSet<AnyObserver>,
    pub(crate) focus: Option<FocusId>,
    /// Incremented every time focus moves. Used to invalidate a
    /// pending keyboard activation state when focus changes.
    pub(crate) focus_generation: u64,
    pending_input: Option<PendingInput>,
    pending_modifier: ModifierState,
    pub(crate) pending_input_observers: SingleSubscriberSet<AnyObserver>,
    prompt: Option<RenderablePromptHandle>,
}

#[derive(Clone, Debug, Default)]
struct ModifierState {
    modifiers: Modifiers,
    saw_other_input: bool,
}

/// Tracks input event timestamps to determine if input is arriving at a high rate.
/// Used for selective VRR (Variable Refresh Rate) optimization.
#[derive(Clone, Debug)]
pub(crate) struct InputRateTracker {
    timestamps: Vec<Instant>,
    window: Duration,
    inputs_per_second: u32,
    sustain_until: Instant,
    sustain_duration: Duration,
}

impl Default for InputRateTracker {
    fn default() -> Self {
        Self {
            timestamps: Vec::new(),
            window: Duration::from_millis(100),
            inputs_per_second: 60,
            sustain_until: Instant::now(),
            sustain_duration: Duration::from_secs(1),
        }
    }
}

impl InputRateTracker {
    pub(crate) fn record_input(&mut self) {
        let now = Instant::now();
        self.timestamps.push(now);
        self.prune_old_timestamps(now);

        let min_events = self.inputs_per_second as u128 * self.window.as_millis() / 1000;
        if self.timestamps.len() as u128 >= min_events {
            self.sustain_until = now + self.sustain_duration;
        }
    }

    pub(crate) fn is_high_rate(&self) -> bool {
        Instant::now() < self.sustain_until
    }

    fn prune_old_timestamps(&mut self, now: Instant) {
        self.timestamps.retain(|&t| now.duration_since(t) <= self.window);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawPhase {
    None,
    Prepaint,
    Paint,
    Focus,
}

pub(crate) struct ElementStateBox {
    pub(crate) inner: Box<dyn Any>,
}

fn default_bounds(display_id: Option<DisplayId>, cx: &mut App) -> WindowBounds {
    // TODO, BUG: if you open a window with the currently active window
    // on the stack, this will erroneously fallback to `None`
    //
    // TODO these should be the initial window bounds not considering maximized/fullscreen
    let active_window_bounds =
        cx.active_window().and_then(|w| w.update(cx, |_, window, _| window.window_bounds()).ok());

    const CASCADE_OFFSET: f32 = 25.0;

    let display = display_id.map_or_else(|| cx.main_display(), |id| cx.find_display(id));

    let default_placement = || Bounds::new(point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);

    // Use visible_bounds to exclude taskbar/dock areas
    let display_bounds = display.as_ref().map_or_else(default_placement, |d| d.visible_bounds());

    let (Bounds { origin: base_origin, size: base_size }, window_bounds_ctor): (_, fn(Bounds<Pixels>) -> WindowBounds) =
        match active_window_bounds {
            Some(bounds) => match bounds {
                WindowBounds::Windowed(bounds) => (bounds, WindowBounds::Windowed),
                WindowBounds::Maximized(bounds) => (bounds, WindowBounds::Maximized),
                WindowBounds::Fullscreen(bounds) => (bounds, WindowBounds::Fullscreen),
            },
            None => (display.as_ref().map_or_else(default_placement, |d| d.default_bounds()), WindowBounds::Windowed),
        };

    let cascade_offset = point(px(CASCADE_OFFSET), px(CASCADE_OFFSET));
    let proposed_origin = base_origin + cascade_offset;
    let proposed_bounds = Bounds::new(proposed_origin, base_size);

    let display_right = display_bounds.origin.x + display_bounds.size.width;
    let display_bottom = display_bounds.origin.y + display_bounds.size.height;
    let window_right = proposed_bounds.origin.x + proposed_bounds.size.width;
    let window_bottom = proposed_bounds.origin.y + proposed_bounds.size.height;

    let fits_horizontally = window_right <= display_right;
    let fits_vertically = window_bottom <= display_bottom;

    let final_origin = match (fits_horizontally, fits_vertically) {
        (true, true) => proposed_origin,
        (false, true) => point(display_bounds.origin.x, base_origin.y),
        (true, false) => point(base_origin.x, display_bounds.origin.y),
        (false, false) => display_bounds.origin,
    };
    window_bounds_ctor(Bounds::new(final_origin, base_size))
}

impl Window {
    pub(crate) fn new(handle: AnyWindowHandle, options: WindowOptions, cx: &mut App) -> Result<Self> {
        let WindowOptions {
            window_bounds,
            titlebar,
            focus,
            show,
            kind,
            is_movable,
            app_owns_titlebar_drag,
            inactive_frame_interval,
            is_resizable,
            is_minimizable,
            display_id,
            window_background,
            window_min_size,
        } = options;

        let window_bounds = window_bounds.unwrap_or_else(|| default_bounds(display_id, cx));
        let platform_window = cx.platform.open_window(
            handle,
            WindowParams {
                bounds: window_bounds.get_bounds(),
                titlebar,
                kind,
                is_movable,
                app_owns_titlebar_drag,
                is_resizable,
                is_minimizable,
                focus,
                show,
                display_id,
                window_min_size,
            },
        )?;

        let display_id = platform_window.display().map(|display| display.id());
        let sprite_atlas = platform_window.sprite_atlas();
        let mouse_position = platform_window.mouse_position();
        let modifiers = platform_window.modifiers();
        let capslock = platform_window.capslock();
        let content_size = platform_window.content_size();
        let scale_factor = platform_window.scale_factor();
        let appearance = platform_window.appearance();
        let text_system = Rc::new(WindowTextSystem::new(Arc::clone(cx.text_system())));
        let invalidator = WindowInvalidator::new();
        let active = Rc::new(Cell::new(platform_window.is_active()));
        let visibility = platform_window.visibility();
        let needs_present = Rc::new(Cell::new(false));
        let next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>> = Rc::default();
        let input_rate_tracker = Rc::new(RefCell::new(InputRateTracker::default()));
        let last_frame_time = Rc::new(Cell::new(None));

        platform_window.set_background_appearance(window_background);

        match window_bounds {
            WindowBounds::Fullscreen(_) => platform_window.toggle_fullscreen(),
            WindowBounds::Maximized(_) => platform_window.zoom(),
            WindowBounds::Windowed(_) => {}
        }

        platform_window.on_close(Box::new({
            let window_id = handle.window_id();
            let cx = cx.to_async();
            move || {
                // Native destruction can re-enter while the app is borrowed, so don't borrow it here.
                cx.spawn(async move |cx| {
                    cx.update(|cx| {
                        // App-driven closure may already have removed the window.
                        if cx.windows.contains_key(window_id) {
                            handle.update(cx, |_, window, _| window.remove_window()).log_err();
                        }
                    });
                })
                .detach();
            }
        }));
        platform_window.on_request_frame(Box::new({
            let mut cx = cx.to_async();
            let invalidator = invalidator.clone();
            let active = Rc::clone(&active);
            let needs_present = Rc::clone(&needs_present);
            let next_frame_callbacks = Rc::clone(&next_frame_callbacks);
            let input_rate_tracker = Rc::clone(&input_rate_tracker);
            let mut deferred_force_render = false;
            move |request_frame_options| {
                // This must be checked before accessing App: if this request
                // arrived re-entrantly while a draw is on this thread's stack,
                // drawing would nest draws, and even touching the App would
                // panic on its already-mutable borrow. Skip instead;
                // the platform leaves the window invalidated (or re-invalidates
                // it), so a fresh request arrives once the in-progress draw
                // unwinds. Remember force_render so the deferred frame still
                // bypasses the view cache.
                if draw_in_progress() {
                    log::debug!("deferring re-entrant window draw request");
                    deferred_force_render |= request_frame_options.force_render;
                    return;
                }
                // Take the deferred flag first: `||` short-circuits, and leaving
                // the flag set when this request already forces a render would
                // force a second, redundant render on the next frame.
                let force_render = mem::take(&mut deferred_force_render) || request_frame_options.force_render;

                // Cap an inactive (unfocused) window to ~30fps to save energy.
                let min_frame_interval = if request_frame_options.require_presentation
                    || (!request_frame_options.force_render && next_frame_callbacks.borrow().is_empty())
                {
                    None
                } else if !active.get() && !input_rate_tracker.borrow_mut().is_high_rate() {
                    inactive_frame_interval
                } else {
                    None
                };

                let now = Instant::now();
                if let Some(min_interval) = min_frame_interval
                    && let Some(last_frame) = last_frame_time.get()
                    && now.duration_since(last_frame) < min_interval
                {
                    // Don't lose a pending forced render to throttling.
                    deferred_force_render |= force_render;
                    // Deferred by throttling: ask demand-driven platforms to retry.
                    handle
                        .update(&mut cx, |_, window, _| {
                            window.platform_window.schedule_frame();
                        })
                        .log_err();
                    // The demand that entered this branch (a deferred forced
                    // render or pending next-frame callbacks) is still
                    // unserved; platforms that stop requesting frames for
                    // idle windows need a wakeup to deliver the retry.
                    invalidator.wake_platform();
                    return;
                }
                last_frame_time.set(Some(now));

                let pending_next_frame_callbacks = next_frame_callbacks.take();
                if !pending_next_frame_callbacks.is_empty() {
                    handle
                        .update(&mut cx, |_, window, cx| {
                            for callback in pending_next_frame_callbacks {
                                callback(window, cx);
                            }
                        })
                        .log_err();
                }

                // Keep presenting if input was recently arriving at a high rate (>= 60fps).
                // Once high-rate input is detected, we sustain presentation for 1 second
                // to prevent display underclocking during active input.
                let needs_present = request_frame_options.require_presentation
                    || needs_present.get()
                    || input_rate_tracker.borrow_mut().is_high_rate();

                let should_draw = invalidator.is_dirty() || force_render;
                if should_draw {
                    handle
                        .update(&mut cx, |_, window, cx| {
                            if force_render {
                                // Bypass cached view reuse so we don't replay stale
                                // atlas tile references after a GPU device recovery.
                                window.refresh();
                            }
                            let arena_clear_needed = window.draw(cx);
                            window.animate_scene(cx);
                            window.present();
                            arena_clear_needed.clear(cx);
                        })
                        .log_err();
                } else if needs_present {
                    handle
                        .update(&mut cx, |_, window, cx| {
                            window.animate_scene(cx);
                            window.present();
                        })
                        .log_err();
                }

                handle
                    .update(&mut cx, |_, window, _| {
                        if window.invalidator.is_dirty() || !window.next_frame_callbacks.borrow().is_empty() {
                            window.platform_window.schedule_frame();
                        }
                    })
                    .log_err();

                // Platforms that stop requesting frames for idle windows only
                // deliver another request after a wakeup. If demand remains
                // after this frame (the window was re-invalidated mid-draw, or
                // animations scheduled next-frame callbacks), re-arm the frame
                // source explicitly.
                if invalidator.is_dirty() || !next_frame_callbacks.borrow().is_empty() {
                    invalidator.wake_platform();
                }
            }
        }));
        invalidator.set_platform_waker(platform_window.frame_waker());
        platform_window.on_resize(Box::new({
            let mut cx = cx.to_async();
            move |_, _| {
                handle.update(&mut cx, |_, window, cx| window.bounds_changed(cx)).log_err();
            }
        }));
        platform_window.on_moved(Box::new({
            let mut cx = cx.to_async();
            move || {
                handle.update(&mut cx, |_, window, cx| window.bounds_changed(cx)).log_err();
            }
        }));
        platform_window.on_display_changed(Box::new({
            let cx = cx.to_async();
            move || cx.update(|cx| Window::display_changed(handle, cx))
        }));
        platform_window.on_appearance_changed(Box::new({
            let cx = cx.to_async();
            let foreground_executor = cx.foreground_executor().clone();
            move || {
                let mut cx = cx.clone();
                // Defer the update because changing the AppKit appearance may
                // synchronously invoke this callback while App is already borrowed.
                foreground_executor
                    .spawn(async move {
                        handle.update(&mut cx, |_, window, cx| window.appearance_changed(cx)).log_err();
                    })
                    .detach();
            }
        }));
        platform_window.on_active_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                handle
                    .update(&mut cx, |_, window, cx| {
                        window.active.set(active);
                        window.modifiers = window.platform_window.modifiers();
                        window.capslock = window.platform_window.capslock();
                        window.activation_observers.clone().retain(|callback| callback(window, cx));

                        window.bounds_changed(cx);
                        window.refresh();
                    })
                    .log_err();
            }
        }));
        platform_window.on_visibility_change(Box::new({
            let mut cx = cx.to_async();
            move |_| {
                handle.update(&mut cx, |_, window, _| window.refresh_visibility()).log_err();
            }
        }));
        platform_window.on_input({
            let mut cx = cx.to_async();
            Box::new(move |event| {
                handle
                    .update(&mut cx, |_, window, cx| window.dispatch_event(event, cx))
                    .log_err()
                    .unwrap_or_else(DispatchEventResult::default)
            })
        });
        platform_window.on_hit_test_window_control({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, window, _cx| {
                        for (area, hitbox) in &window.rendered_frame.window_control_hitboxes {
                            if window.mouse_hit_test.ids.contains(&hitbox.id) {
                                return Some(*area);
                            }
                        }
                        None
                    })
                    .log_err()
                    .unwrap_or(None)
            })
        });

        Ok(Window {
            handle,
            invalidator,
            removed: false,
            platform_window,
            display_id,
            is_resizable,
            is_minimizable,
            sprite_atlas,
            text_system,
            rem_size: px(16.),
            rem_size_override_stack: SmallVec::new(),
            viewport_size: content_size,
            layout_engine: Some(TaffyLayoutEngine::new()),
            root: None,
            element_id_stack: ElementIdStack::default(),
            text_style_stack: Vec::new(),
            rendered_entity_stack: Vec::new(),
            element_offset_stack: Vec::new(),
            content_mask_stack: Vec::new(),
            element_opacity: 1.0,
            requested_autoscroll: None,
            rendered_frame: Frame::new(DispatchTree::new(Rc::clone(&cx.keymap), Rc::clone(&cx.actions))),
            next_frame: Frame::new(DispatchTree::new(Rc::clone(&cx.keymap), Rc::clone(&cx.actions))),
            next_frame_callbacks,
            next_hitbox_id: HitboxId(0),
            next_tooltip_id: TooltipId::default(),
            tooltip_bounds: None,
            dirty_views: FxHashSet::default(),
            focus_listeners: SingleSubscriberSet::new(),
            focus_lost_listeners: SingleSubscriberSet::new(),
            focus_lost_path: SmallVec::new(),
            default_prevented: true,
            mouse_position,
            mouse_hit_test: HitTest::default(),
            modifiers,
            capslock,
            scale_factor,
            bounds_observers: SingleSubscriberSet::new(),
            appearance,
            appearance_observers: SingleSubscriberSet::new(),
            active,
            visibility,
            needs_present,
            animation_frame_pending: Rc::default(),
            frame_stats: None,
            input_rate_tracker,
            last_input_modality: InputModality::Mouse,
            refreshing: false,
            frame_number: 0,
            activation_observers: SingleSubscriberSet::new(),
            focus: None,
            focus_generation: 0,
            pending_input: None,
            pending_modifier: ModifierState::default(),
            pending_input_observers: SingleSubscriberSet::new(),
            prompt: None,
            image_cache_stack: Vec::new(),
        })
    }

    pub(crate) fn new_focus_listener(&self, value: AnyWindowFocusListener) -> (Subscription, impl FnOnce() + use<>) {
        self.focus_listeners.insert_pending(value)
    }

    fn mark_view_dirty(&mut self, view_id: EntityId) {
        // Mark ancestor views as dirty. If already in the `dirty_views` set, then all its ancestors
        // should already be dirty.
        for view_id in self.rendered_frame.dispatch_tree.view_path_reversed(view_id) {
            if !self.dirty_views.insert(view_id) {
                break;
            }
        }
    }

    pub(crate) fn refresh_visibility(&mut self) {
        let visibility = self.platform_window.visibility();
        if self.visibility != visibility {
            self.visibility = visibility;
        }
    }

    /// Whether the platform is presenting this window's frames (see
    /// [`WindowVisibility`]).
    pub const fn visibility(&self) -> WindowVisibility {
        self.visibility
    }

    /// Whether frames drawn for this window will be shown.
    ///
    /// This is not the window's shown/hidden state: a shown window that is
    /// fully behind another window, minimized, or on a sleeping display is not
    /// visible here.
    pub fn is_visible(&self) -> bool {
        self.visibility.is_visible()
    }

    /// Replaces the root entity of the window with a new one.
    pub fn replace_root<E>(
        &mut self,
        cx: &mut App,
        build_view: impl FnOnce(&mut Window, &mut Context<E>) -> E,
    ) -> Entity<E>
    where
        E: 'static + Render,
    {
        let view = cx.new(|cx| build_view(self, cx));
        self.root = Some(view.clone().into());
        self.refresh();
        view
    }

    /// Returns the root entity of the window, if it has one.
    pub fn root<E>(&self) -> Option<Option<Entity<E>>>
    where
        E: 'static + Render,
    {
        self.root.as_ref().map(|view| view.clone().downcast::<E>().ok())
    }

    /// Obtain a handle to the window that belongs to this context.
    pub const fn window_handle(&self) -> AnyWindowHandle {
        self.handle
    }

    /// Mark the window as dirty, scheduling it to be redrawn on the next frame.
    pub fn refresh(&mut self) {
        if self.invalidator.not_drawing() {
            self.refreshing = true;
            self.invalidator.set_dirty(true);
        }
    }

    /// Close this window.
    pub const fn remove_window(&mut self) {
        self.removed = true;
    }

    /// Obtain the currently focused [`FocusHandle`]. If no elements are focused, returns `None`.
    pub fn focused(&self, cx: &App) -> Option<FocusHandle> {
        let id = self.focus?;
        FocusHandle::for_id(id, &cx.focus_handles)
    }

    /// While focus-lost listeners are being dispatched, returns the closest ancestor of the
    /// previously focused element that can still receive focus, making it a suitable target
    /// for focus restoration. Returns `None` at all other times, or when no such ancestor exists.
    pub fn focus_lost_restore_target(&self, cx: &App) -> Option<FocusHandle> {
        let (_leaf, ancestors) = self.focus_lost_path.split_last()?;
        ancestors.iter().rev().find_map(|id| {
            self.rendered_frame.dispatch_tree.focusable_node_id(*id)?;
            FocusHandle::for_id(*id, &cx.focus_handles)
        })
    }

    /// Move focus to the element associated with the given [`FocusHandle`].
    pub fn focus(&mut self, handle: &FocusHandle, cx: &mut App) {
        if self.focus == Some(handle.id) {
            return;
        }

        self.focus = Some(handle.id);
        self.focus_generation = self.focus_generation.wrapping_add(1);
        self.clear_pending_keystrokes(cx);

        self.refresh();
    }

    /// Remove focus from all elements within this context's window.
    pub fn blur(&mut self, cx: &mut App) {
        self.clear_pending_keystrokes(cx);

        if self.focus.is_some() {
            self.focus_generation = self.focus_generation.wrapping_add(1);
        }
        self.focus = None;
        self.refresh();
    }

    /// Move focus to next tab stop.
    pub fn focus_next(&mut self, cx: &mut App) {
        if let Some(handle) = self.rendered_frame.tab_stops.next(self.focus.as_ref()) {
            self.focus(&handle, cx);
        }
    }

    /// Move focus to previous tab stop.
    pub fn focus_prev(&mut self, cx: &mut App) {
        if let Some(handle) = self.rendered_frame.tab_stops.prev(self.focus.as_ref()) {
            self.focus(&handle, cx);
        }
    }

    /// Accessor for the text system.
    pub const fn text_system(&self) -> &Rc<WindowTextSystem> {
        &self.text_system
    }

    /// The current text style. Which is composed of all the style refinements provided to `with_text_style`.
    pub fn text_style(&self) -> TextStyle {
        let mut style = TextStyle::default();
        for refinement in &self.text_style_stack {
            style.refine(refinement);
        }
        style
    }

    /// Check if the platform window is maximized.
    pub fn is_maximized(&self) -> bool {
        self.platform_window.is_maximized()
    }

    /// Return the `WindowBounds` to indicate that how a window should be opened
    /// after it has been closed.
    pub fn window_bounds(&self) -> WindowBounds {
        self.platform_window.window_bounds()
    }

    /// Encode the window's native restorable state into an opaque blob.
    /// Returns `None` on platforms without native state restoration or if encoding fails.
    pub fn native_window_state(&self) -> Option<Vec<u8>> {
        self.platform_window.native_window_state()
    }

    /// Restore the window's native state from a blob previously produced
    /// by [`Window::native_window_state`]. A no-op on platforms without native state restoration.
    pub fn restore_native_window_state(&self, state: &[u8]) {
        self.platform_window.restore_native_window_state(state);
    }

    /// Dispatch the given action on the currently focused element.
    pub fn dispatch_action(&mut self, action: Box<dyn Action>, cx: &mut App) {
        let focus_id = self.focused(cx).map(|handle| handle.id);

        let window = self.handle;
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    let node_id = window.focus_node_id_in_rendered_frame(focus_id);
                    window.dispatch_action_on_node(node_id, action.as_ref(), cx);
                })
                .log_err();
        });
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer(&self, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        let handle = self.handle;
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| f(window, cx)).ok();
        });
    }

    /// Subscribe to events emitted by a entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked a handle to the emitting entity, the event, and a window context for the current window.
    pub fn observe<T: 'static>(
        &mut self,
        observed: &Entity<T>,
        cx: &mut App,
        mut on_notify: impl FnMut(Entity<T>, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let entity_id = observed.entity_id();
        let observed = observed.downgrade();
        let window_handle = self.handle;
        cx.new_observer(
            entity_id,
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| {
                        if let Some(handle) = observed.upgrade() {
                            on_notify(handle, window, cx);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false)
            }),
        )
    }

    /// Subscribe to events emitted by a entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked a handle to the emitting entity, the event, and a window context for the current window.
    pub fn subscribe<Emitter, Evt>(
        &mut self,
        entity: &Entity<Emitter>,
        cx: &mut App,
        mut on_event: impl FnMut(Entity<Emitter>, &Evt, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        Emitter: EventEmitter<Evt>,
        Evt: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        let window_handle = self.handle;
        cx.new_subscription(
            entity_id,
            (
                TypeId::of::<Evt>(),
                Box::new(move |event, cx| {
                    window_handle
                        .update(cx, |_, window, cx| {
                            if let Some(entity) = handle.upgrade() {
                                let event = event.downcast_ref().expect("invalid event type");
                                on_event(entity, event, window, cx);
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                }),
            ),
        )
    }

    /// Register a callback to be invoked when the given `Entity` is released.
    pub fn observe_release<T>(
        &self,
        entity: &Entity<T>,
        cx: &mut App,
        on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let entity_id = entity.entity_id();
        let window_handle = self.handle;
        cx.release_listeners.insert(
            entity_id,
            Box::new(move |entity, cx| {
                let entity = entity.downcast_mut().expect("invalid entity type");
                window_handle.update(cx, |_, window, cx| on_release(entity, window, cx)).ok();
            }),
        )
    }

    /// Creates an [`AsyncWindowContext`], which has a static lifetime and can be held across
    /// await points in async code.
    pub fn to_async(&self, cx: &App) -> AsyncWindowContext {
        AsyncWindowContext::new_context(cx.to_async(), self.handle)
    }

    /// Schedule the given closure to be run directly after the current frame is rendered.
    pub fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(callback));
        self.platform_window.schedule_frame();
        // Next-frame callbacks create frame demand without dirtying the
        // window, so the platform's frame source must be woken explicitly.
        self.invalidator.wake_platform();
    }

    /// Schedule a frame to be drawn on the next animation frame.
    ///
    /// This is useful for elements that need to animate continuously, such as a video player or an animated GIF.
    /// It will cause the window to redraw on the next frame, even if no other changes have occurred.
    ///
    /// If called from within a view, it will notify that view on the next frame. Otherwise, it will refresh the entire window.
    pub fn request_animation_frame(&self) {
        let entity = self.current_view();
        self.on_next_frame(move |_, cx| cx.notify(entity));
    }

    /// Spawn the future returned by the given closure on the application thread pool.
    /// The closure is provided a handle to the current window and an `AsyncWindowContext` for
    /// use within your future.
    #[track_caller]
    pub fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn(async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }

    /// Notify the window that its bounds have changed.
    ///
    /// This updates internal state like `viewport_size` and `scale_factor` from
    /// the platform window, then notifies observers. Normally called automatically
    /// by the platform's resize callback, but exposed publicly for test infrastructure.
    pub fn bounds_changed(&mut self, cx: &mut App) {
        self.scale_factor = self.platform_window.scale_factor();
        self.viewport_size = self.platform_window.content_size();
        self.mouse_position = self.platform_window.mouse_position();

        self.refresh();

        self.bounds_observers.clone().retain(|callback| callback(self, cx));
    }

    fn display_changed(handle: AnyWindowHandle, cx: &mut App) {
        let moved_to = handle
            .update(cx, |_, window, _| {
                let display_id = window.platform_window.display().map(|display| display.id());
                let moved = display_id != window.display_id;
                window.display_id = display_id;
                moved.then_some(display_id)
            })
            .log_err()
            .flatten();
        let Some(display_id) = moved_to else {
            return;
        };
        // The platform's notifications about the window and about displays
        // arrive in no guaranteed order, so make sure App knows the display.
        if let Some(display_id) = display_id
            && !cx.knows_display(display_id)
        {
            cx.displays_changed();
        }
    }

    /// The time between refreshes of the display the window is on, as of the
    /// platform's last display change notification. Variable refresh rate
    /// displays report their maximum rate. Returns `None` when the platform
    /// doesn't report it.
    pub fn refresh_interval(&self, cx: &App) -> Option<Duration> {
        cx.display_refresh_interval(self.display_id?)
    }

    /// Returns the bounds of the current window in the global coordinate space, which could span across multiple displays.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.platform_window.bounds()
    }

    /// Set the content size of the window.
    pub fn resize(&mut self, size: Size<Pixels>) {
        self.platform_window.resize(size);
    }

    /// Returns whether or not the window is currently fullscreen.
    pub fn is_fullscreen(&self) -> bool {
        self.platform_window.is_fullscreen()
    }

    /// Returns whether the window is currently in simple (borderless) fullscreen,
    /// where it covers the entire screen including the menu bar and notch area.
    /// Always `false` on platforms other than macOS.
    pub fn is_simple_fullscreen(&self) -> bool {
        self.platform_window.is_simple_fullscreen()
    }

    pub(crate) fn appearance_changed(&mut self, cx: &mut App) {
        let appearance = self.platform_window.appearance();
        if appearance != self.appearance {
            self.appearance = appearance;
            self.appearance_observers.clone().retain(|callback| callback(self, cx));
        }
    }

    /// Registers a callback to be invoked when the window's light/dark appearance changes.
    pub fn observe_appearance(&self, mut callback: impl FnMut(&mut Window, &mut App) + 'static) -> Subscription {
        self.appearance_observers.insert(Box::new(move |window, cx| {
            callback(window, cx);
            true
        }))
    }

    /// Returns the appearance of the current window.
    pub const fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    /// Returns the size of the drawable area within the window.
    pub const fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size
    }

    /// Returns whether this window is focused by the operating system (receiving key events).
    pub fn is_window_active(&self) -> bool {
        self.active.get()
    }

    /// Toggle zoom on the window.
    pub fn zoom_window(&self) {
        self.platform_window.zoom();
    }

    /// Hands window movement to the system for a drag that started in this window.
    ///
    /// Events may not be received during a move operation.
    pub fn start_window_move(&self) {
        self.platform_window.start_window_move();
    }

    /// Returns whether this window is resizable.
    pub const fn is_resizable(&self) -> bool {
        self.is_resizable
    }

    /// Returns whether this window is minimizable.
    pub const fn is_minimizable(&self) -> bool {
        self.is_minimizable
    }

    /// Updates the window's title at the platform level.
    pub fn set_window_title(&mut self, title: &str) {
        self.platform_window.set_title(title);
    }

    /// Sets the position of the macOS traffic light buttons.
    pub fn set_traffic_light_position(&self, position: Point<Pixels>) {
        self.platform_window.set_traffic_light_position(position);
    }

    /// Sets the window background appearance.
    pub fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.platform_window.set_background_appearance(background_appearance);
    }

    /// Mark the window as dirty at the platform level.
    pub fn set_window_edited(&mut self, edited: bool) {
        self.platform_window.set_edited(edited);
    }

    /// Set the path of the file this window represents.
    /// On macOS, this sets the window's accessibility document property (AXDocument).
    pub fn set_document_path(&self, path: Option<&path::Path>) {
        self.platform_window.set_document_path(path);
    }

    /// Determine the display on which the window is visible.
    pub fn display(&self, cx: &App) -> Option<Rc<dyn PlatformDisplay>> {
        cx.platform.displays().into_iter().find(|display| Some(display.id()) == self.display_id)
    }

    /// Show the platform character palette.
    pub fn show_character_palette(&self) {
        self.platform_window.show_character_palette();
    }

    /// The scale factor of the display associated with the window. For example, it could
    /// return 2.0 for a "retina" display, indicating that each logical pixel should actually
    /// be rendered as two pixels on screen.
    pub const fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// The size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn rem_size(&self) -> Pixels {
        self.rem_size_override_stack.last().copied().unwrap_or(self.rem_size)
    }

    /// Sets the size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn set_rem_size(&mut self, rem_size: impl Into<Pixels>) {
        self.rem_size = rem_size.into();
    }

    /// Acquire a globally unique identifier for the given ElementId.
    /// Only valid for the duration of the provided closure.
    pub fn with_global_id<R>(&mut self, element_id: ElementId, f: impl FnOnce(&GlobalElementId, &mut Self) -> R) -> R {
        self.with_id(element_id, |this| {
            let global_id = this.element_id_stack.global_id();

            f(&global_id, this)
        })
    }

    /// Calls the provided closure with the element ID pushed on the stack.
    #[inline]
    pub fn with_id<R>(&mut self, element_id: impl Into<ElementId>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.element_id_stack.push(element_id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    /// Executes the provided function with the specified rem size.
    ///
    /// This method must only be called as part of element drawing.
    // This function is called in a highly recursive manner in editor
    // prepainting, make sure its inlined to reduce the stack burden
    #[inline]
    pub fn with_rem_size<F, R>(&mut self, rem_size: Option<impl Into<Pixels>>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        if let Some(rem_size) = rem_size {
            self.rem_size_override_stack.push(rem_size.into());
            let result = f(self);
            self.rem_size_override_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// The line height associated with the current text style.
    pub fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels(self.rem_size())
    }

    /// Call to prevent the default action of an event. Currently only used to prevent
    /// parent elements from becoming focused on mouse down.
    pub const fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    /// Obtain whether default has been prevented for the event currently being dispatched.
    pub const fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    /// Determine whether the given action is available along the dispatch path to the currently focused element.
    pub fn is_action_available(&self, action: &dyn Action, cx: &App) -> bool {
        let node_id = self.focus_node_id_in_rendered_frame(self.focused(cx).map(|handle| handle.id));
        self.rendered_frame.dispatch_tree.is_action_available(action, node_id)
    }

    /// The position of the mouse relative to the window.
    pub const fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position
    }

    /// The current state of the keyboard's modifiers.
    pub const fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Returns true if the last input event was keyboard-based (key press, tab navigation, etc.)
    /// This is used for focus-visible styling to show focus indicators only for keyboard navigation.
    pub fn last_input_was_keyboard(&self) -> bool {
        self.last_input_modality == InputModality::Keyboard
    }

    /// The current state of the keyboard's capslock.
    pub const fn capslock(&self) -> Capslock {
        self.capslock
    }

    /// Produces a new frame and assigns it to `rendered_frame`. To actually show
    /// the contents of the new [`Scene`], the window must present it.
    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {
        // Set up the per-App arena for element allocation during this draw.
        // This ensures that multiple test Apps have isolated arenas.
        let arena_scope = ElementArenaScope::enter(&cx.element_arena);

        self.invalidate_entities();
        cx.entities.clear_accessed();
        self.frame_number += 1;
        self.invalidator.set_dirty(false);
        self.requested_autoscroll = None;

        // Restore the previously-used input handler.
        // Place it back into a None slot (left by a previous .take()) so that
        // cached paint_range indices in reuse_paint find the handler at the
        // expected position.
        if let Some(input_handler) = self.platform_window.take_input_handler() {
            if let Some(slot) = self.rendered_frame.input_handlers.iter_mut().rev().find(|h| h.is_none()) {
                *slot = Some(input_handler);
            } else {
                self.rendered_frame.input_handlers.push(Some(input_handler));
            }
        }
        let draw_start = self.frame_stats.is_some().then(Instant::now);
        let shaped_before = self.frame_stats.is_some().then(|| self.text_system().shaped_line_count());
        let phase_times = self.draw_roots(cx);
        self.dirty_views.clear();
        self.next_frame.window_active = self.active.get();

        // Register requested input handler with the platform window.
        // Use .take() instead of .pop() to preserve Vec length, so that cached
        // paint_range indices remain valid for reuse_paint on the next frame.
        // Search backwards to find the last Some entry, since reuse_paint may
        // have copied None slots from the previous frame. (Fixes #50456)
        if let Some(input_handler) = self.next_frame.input_handlers.iter_mut().rev().find_map(Option::take) {
            self.platform_window.set_input_handler(input_handler);
        }

        self.layout_engine.as_mut().expect("layout engine is present outside compute_layout").clear();
        self.text_system().finish_frame();
        self.next_frame.finish(&mut self.rendered_frame);

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        self.next_frame.clear();
        let current_focus_path = self.rendered_frame.focus_path();
        let current_window_active = self.rendered_frame.window_active;
        let mut focus_before_listeners = self.focus;

        if previous_focus_path != current_focus_path || previous_window_active != current_window_active {
            if !previous_focus_path.is_empty() && current_focus_path.is_empty() {
                self.focus_lost_path.clone_from(&previous_focus_path);
                self.focus_lost_listeners.clone().retain(|listener| listener(self, cx));
                self.focus_lost_path = SmallVec::new();
                // The focus-lost fallback (e.g. a workspace refocusing itself) may target
                // an element that isn't part of the element tree, in which case scheduling
                // a redraw below would dispatch focus-lost again, looping forever. Only
                // track focus movement caused by the focus listeners.
                focus_before_listeners = self.focus;
            }

            let event = WindowFocusEvent {
                previous_focus_path: if previous_window_active { previous_focus_path } else { SmallVec::default() },
                current_focus_path: if current_window_active { current_focus_path } else { SmallVec::default() },
            };
            self.focus_listeners.clone().retain(|listener| listener(&event, self, cx));
        }

        self.record_entities_accessed(cx);
        self.reset_cursor_style(cx);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        // Focus listeners may move focus (e.g. a dock forwarding focus to its active
        // panel). `Window::focus` suppresses `refresh` while a draw is in progress, so
        // schedule another frame here to render the new focus state and dispatch the
        // resulting focus events.
        if self.focus != focus_before_listeners {
            self.refresh();
        }
        self.needs_present.set(true);
        if let (Some(stats), Some(draw_start)) = (self.frame_stats.as_mut(), draw_start) {
            let (build, paint) = phase_times.unwrap_or_default();
            let shaped_lines = self.text_system.shaped_line_count() - shaped_before.unwrap_or_default();
            stats.record_draw(build, paint, draw_start.elapsed(), shaped_lines);
        }

        // Exit the scope to obtain the arena-clear token this draw owes; the
        // scope's teardown itself happens in `ElementArenaScope::drop`.
        arena_scope.exit(&cx.element_arena)
    }

    fn record_entities_accessed(&self, cx: &mut App) {
        let entities = cx.entities.take_accessed();
        let handle = self.handle;
        cx.record_entities_accessed(
            handle,
            // Try moving window invalidator into the Window
            self.invalidator.clone(),
            &entities,
        );
        cx.entities.restore_accessed(entities);
    }

    fn invalidate_entities(&mut self) {
        let mut views = self.invalidator.take_views();
        for entity in views.drain() {
            self.mark_view_dirty(entity);
        }
        self.invalidator.replace_views(views);
    }

    /// Turns the rendered scene's animated sprites to the current time and, while it has any,
    /// schedules the next present. Nothing is redrawn and no view is notified.
    fn animate_scene(&mut self, cx: &App) {
        let scene = &mut self.rendered_frame.scene;
        if !scene.has_animations() {
            return;
        }
        scene.advance_animations(cx.background_executor().now() - cx.synced_animation_epoch);
        if self.animation_frame_pending.replace(true) {
            return;
        }
        let pending = Rc::clone(&self.animation_frame_pending);
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(SPRITE_ANIMATION_FRAME_INTERVAL).await;
            pending.set(false);
            cx.update(|window, _| {
                if window.rendered_frame.scene.has_animations() {
                    window.needs_present.set(true);
                    window.platform_window.schedule_frame();
                }
            })
            .ok();
        })
        .detach();
    }

    fn present(&mut self) {
        let start = self.frame_stats.is_some().then(Instant::now);
        self.platform_window.draw(&self.rendered_frame.scene);
        self.needs_present.set(false);
        if let (Some(stats), Some(start)) = (self.frame_stats.as_mut(), start) {
            stats.record_present(start.elapsed());
        }
    }

    /// Returns the build (render, layout, prepaint) and paint times when frame stats are on.
    fn draw_roots(&mut self, cx: &mut App) -> Option<(Duration, Duration)> {
        let build_start = self.frame_stats.is_some().then(Instant::now);
        self.invalidator.set_phase(DrawPhase::Prepaint);
        self.tooltip_bounds.take();

        // Layout all root elements. Like the root element on the web, which
        // stretches to fill the viewport unless explicitly sized, window roots
        // fill the window when their size is `auto`.
        let scale_factor = self.scale_factor();
        let mut root_element = self.root.as_ref().expect("window has a root view").clone().into_any_element();
        let root_layout_id = root_element.request_layout(self, cx);
        self.layout_engine
            .as_mut()
            .expect("layout engine is present outside compute_layout")
            .stretch_auto_size_to_fill(root_layout_id, self.viewport_size, scale_factor);
        root_element.prepaint_as_root(Point::default(), self.viewport_size.into(), self, cx);

        self.prepaint_deferred_draws(cx);

        let mut prompt_element = None;
        let mut active_drag_element = None;
        let mut tooltip_element = None;
        if let Some(prompt) = self.prompt.take() {
            let mut element = prompt.view.any_view().into_any_element();
            let prompt_layout_id = element.request_layout(self, cx);
            self.layout_engine
                .as_mut()
                .expect("layout engine is present outside compute_layout")
                .stretch_auto_size_to_fill(prompt_layout_id, self.viewport_size, scale_factor);
            element.prepaint_as_root(Point::default(), self.viewport_size.into(), self, cx);
            prompt_element = Some(element);
            self.prompt = Some(prompt);
        } else if let Some(active_drag) = cx.active_drag.take() {
            let mut element = active_drag.view.clone().into_any_element();
            let offset = self.mouse_position() - active_drag.cursor_offset;
            element.prepaint_as_root(offset, AvailableSpace::min_size(), self, cx);
            active_drag_element = Some(element);
            cx.active_drag = Some(active_drag);
        } else {
            tooltip_element = self.prepaint_tooltip(cx);
        }

        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);

        // Now actually paint the elements.
        let paint_start = build_start.map(|build_start| (build_start.elapsed(), Instant::now()));
        self.invalidator.set_phase(DrawPhase::Paint);
        root_element.paint(self, cx);

        self.paint_deferred_draws(cx);

        if let Some(mut prompt_element) = prompt_element {
            prompt_element.paint(self, cx);
        } else if let Some(mut drag_element) = active_drag_element {
            drag_element.paint(self, cx);
        } else if let Some(mut tooltip_element) = tooltip_element {
            tooltip_element.paint(self, cx);
        }

        paint_start.map(|(build, paint_start)| (build, paint_start.elapsed()))
    }

    fn prepaint_tooltip(&mut self, cx: &mut App) -> Option<AnyElement> {
        // Use indexing instead of iteration to avoid borrowing self for the duration of the loop.
        for tooltip_request_index in (0..self.next_frame.tooltip_requests.len()).rev() {
            let Some(Some(tooltip_request)) = self.next_frame.tooltip_requests.get(tooltip_request_index).cloned()
            else {
                log::error!("Unexpectedly absent TooltipRequest");
                continue;
            };
            let mut element = tooltip_request.tooltip.view.clone().into_any_element();
            let mouse_position = tooltip_request.tooltip.mouse_position;
            let tooltip_size = element.layout_as_root(AvailableSpace::min_size(), self, cx);

            let mut tooltip_bounds = Bounds::new(mouse_position + point(px(1.), px(1.)), tooltip_size);
            let window_bounds = Bounds { origin: Point::default(), size: self.viewport_size() };

            if tooltip_bounds.right() > window_bounds.right() {
                let new_x = mouse_position.x - tooltip_bounds.size.width - px(1.);
                if new_x >= Pixels::ZERO {
                    tooltip_bounds.origin.x = new_x;
                } else {
                    tooltip_bounds.origin.x = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.x - tooltip_bounds.right() - window_bounds.right(),
                    );
                }
            }

            if tooltip_bounds.bottom() > window_bounds.bottom() {
                let new_y = mouse_position.y - tooltip_bounds.size.height - px(1.);
                if new_y >= Pixels::ZERO {
                    tooltip_bounds.origin.y = new_y;
                } else {
                    tooltip_bounds.origin.y = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.y - tooltip_bounds.bottom() - window_bounds.bottom(),
                    );
                }
            }

            // It's possible for an element to have an active tooltip while not being painted (e.g.
            // via the `visible_on_hover` method). Since mouse listeners are not active in this
            // case, instead update the tooltip's visibility here.
            let is_visible = (tooltip_request.tooltip.check_visible_and_update)(tooltip_bounds, self, cx);
            if !is_visible {
                continue;
            }

            self.with_absolute_element_offset(tooltip_bounds.origin, |window| element.prepaint(window, cx));

            self.tooltip_bounds = Some(TooltipBounds { id: tooltip_request.id, bounds: tooltip_bounds });
            return Some(element);
        }
        None
    }

    fn prepaint_deferred_draws(&mut self, cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        // Process deferred draws in multiple rounds to support nesting.
        // Each round processes all current deferred draws, which may push new ones.
        //
        // The draws are processed in place rather than being moved out of
        // `next_frame.deferred_draws`: `prepaint_index` snapshots that vector's
        // length, so any prepaint range recorded during a round (view caches,
        // nested deferred draws) must index the same vector `reuse_prepaint`
        // slices on the next frame. Moving the draws out and re-appending them
        // shifts the indices of nested draws, causing reused subtrees to graft
        // the wrong deferred draws and panic in the dispatch tree.
        let mut round_start = 0;
        let mut depth = 0;
        loop {
            let round_end = self.next_frame.deferred_draws.len();
            if round_start == round_end {
                break;
            }
            // Limit maximum nesting depth to prevent infinite loops.
            assert!(depth < 10, "Exceeded maximum (10) deferred depth");
            depth += 1;

            // Sort this round by priority.
            let mut traversal_order = (round_start..round_end).collect::<SmallVec<[usize; 8]>>();
            traversal_order.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);

            for deferred_draw_ix in traversal_order {
                let (element, parent_node, current_view, rem_size, absolute_offset, prepaint_range) = {
                    let deferred_draw = &mut self.next_frame.deferred_draws[deferred_draw_ix];
                    self.element_id_stack.clone_from(&deferred_draw.element_id_stack);
                    self.text_style_stack.clone_from(&deferred_draw.text_style_stack);
                    (
                        deferred_draw.element.take(),
                        deferred_draw.parent_node,
                        deferred_draw.current_view,
                        deferred_draw.rem_size,
                        deferred_draw.absolute_offset,
                        deferred_draw.prepaint_range.clone(),
                    )
                };
                self.next_frame.dispatch_tree.set_active_node(parent_node);

                let prepaint_start = self.prepaint_index();
                if let Some(mut element) = element {
                    let ((), accessed_entities) = cx.detect_accessed_entities(|cx| {
                        self.with_rendered_view(current_view, |window| {
                            window.with_rem_size(Some(rem_size), |window| {
                                window.with_absolute_element_offset(absolute_offset, |window| {
                                    element.prepaint(window, cx);
                                });
                            });
                        });
                    });
                    let deferred_draw = &mut self.next_frame.deferred_draws[deferred_draw_ix];
                    deferred_draw.accessed_entities = accessed_entities;
                    deferred_draw.element = Some(element);
                } else {
                    cx.entities.extend_accessed(
                        self.next_frame.deferred_draws[deferred_draw_ix].accessed_entities.iter().copied(),
                    );
                    self.reuse_prepaint(prepaint_range);
                }
                let prepaint_end = self.prepaint_index();
                self.next_frame.deferred_draws[deferred_draw_ix].prepaint_range = prepaint_start..prepaint_end;
            }

            self.element_id_stack.clear();
            self.text_style_stack.clear();
            round_start = round_end;
        }
    }

    fn paint_deferred_draws(&mut self, cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        // Paint all deferred draws in priority order.
        // Since prepaint has already processed nested deferreds, we just paint them all.
        if self.next_frame.deferred_draws.is_empty() {
            return;
        }

        let traversal_order = self.deferred_draw_traversal_order();
        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in traversal_order {
            let deferred_draw = &mut deferred_draws[deferred_draw_ix];
            self.element_id_stack.clone_from(&deferred_draw.element_id_stack);
            self.next_frame.dispatch_tree.set_active_node(deferred_draw.parent_node);

            let paint_start = self.paint_index();
            let content_mask = deferred_draw.content_mask;
            if let Some(element) = deferred_draw.element.as_mut() {
                let ((), accessed_entities) = cx.detect_accessed_entities(|cx| {
                    self.with_rendered_view(deferred_draw.current_view, |window| {
                        window.with_content_mask(content_mask, |window| {
                            window.with_rem_size(Some(deferred_draw.rem_size), |window| {
                                element.paint(window, cx);
                            });
                        });
                    });
                });
                deferred_draw.accessed_entities.extend(accessed_entities);
            } else {
                self.reuse_paint(deferred_draw.paint_range.clone());
            }
            let paint_end = self.paint_index();
            deferred_draw.paint_range = paint_start..paint_end;
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
    }

    fn deferred_draw_traversal_order(&self) -> SmallVec<[usize; 8]> {
        let deferred_count = self.next_frame.deferred_draws.len();
        let mut sorted_indices = (0..deferred_count).collect::<SmallVec<[_; 8]>>();
        sorted_indices.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        sorted_indices
    }

    pub(crate) fn prepaint_index(&self) -> PrepaintStateIndex {
        PrepaintStateIndex {
            hitboxes_index: self.next_frame.hitboxes.len(),
            tooltips_index: self.next_frame.tooltip_requests.len(),
            deferred_draws_index: self.next_frame.deferred_draws.len(),
            dispatch_tree_index: self.next_frame.dispatch_tree.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn reuse_prepaint(&mut self, range: Range<PrepaintStateIndex>) {
        self.next_frame
            .hitboxes
            .extend(self.rendered_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index].iter().cloned());
        self.next_frame.tooltip_requests.extend(
            self.rendered_frame.tooltip_requests[range.start.tooltips_index..range.end.tooltips_index]
                .iter_mut()
                .map(Option::take),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states
                [range.start.accessed_element_states_index..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (id.clone(), *type_id)),
        );
        self.text_system.reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);

        let reused_subtree = self.next_frame.dispatch_tree.reuse_subtree(
            range.start.dispatch_tree_index..range.end.dispatch_tree_index,
            &mut self.rendered_frame.dispatch_tree,
            self.focus,
        );

        if reused_subtree.contains_focus() {
            self.next_frame.focus = self.focus;
        }

        self.next_frame.deferred_draws.extend(
            self.rendered_frame.deferred_draws[range.start.deferred_draws_index..range.end.deferred_draws_index]
                .iter()
                .map(|deferred_draw| DeferredDraw {
                    current_view: deferred_draw.current_view,
                    parent_node: reused_subtree.refresh_node_id(deferred_draw.parent_node),
                    element_id_stack: deferred_draw.element_id_stack.clone(),
                    text_style_stack: deferred_draw.text_style_stack.clone(),
                    content_mask: deferred_draw.content_mask,
                    rem_size: deferred_draw.rem_size,
                    priority: deferred_draw.priority,
                    element: None,
                    absolute_offset: deferred_draw.absolute_offset,
                    prepaint_range: deferred_draw.prepaint_range.clone(),
                    paint_range: deferred_draw.paint_range.clone(),
                    accessed_entities: deferred_draw.accessed_entities.clone(),
                }),
        );
    }

    pub(crate) fn paint_index(&self) -> PaintIndex {
        PaintIndex {
            scene_index: self.next_frame.scene.len(),
            mouse_listeners_index: self.next_frame.mouse_listeners.len(),
            input_handlers_index: self.next_frame.input_handlers.len(),
            cursor_styles_index: self.next_frame.cursor_styles.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            tab_handle_index: self.next_frame.tab_stops.paint_index(),
            ui_elements_index: self.next_frame.ui_elements.len(),
            ui_texts_index: self.next_frame.ui_texts.len(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn reuse_paint(&mut self, range: Range<PaintIndex>) {
        self.next_frame.cursor_styles.extend(
            self.rendered_frame.cursor_styles[range.start.cursor_styles_index..range.end.cursor_styles_index]
                .iter()
                .cloned(),
        );
        self.next_frame.input_handlers.extend(
            self.rendered_frame.input_handlers[range.start.input_handlers_index..range.end.input_handlers_index]
                .iter_mut()
                .map(Option::take),
        );
        self.next_frame.mouse_listeners.extend(
            self.rendered_frame.mouse_listeners[range.start.mouse_listeners_index..range.end.mouse_listeners_index]
                .iter_mut()
                .map(Option::take),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states
                [range.start.accessed_element_states_index..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (id.clone(), *type_id)),
        );
        self.next_frame.tab_stops.replay(
            &self.rendered_frame.tab_stops.insertion_history[range.start.tab_handle_index..range.end.tab_handle_index],
        );

        self.next_frame.ui_elements.extend(
            self.rendered_frame.ui_elements[range.start.ui_elements_index..range.end.ui_elements_index].iter().cloned(),
        );
        self.next_frame
            .ui_texts
            .extend(self.rendered_frame.ui_texts[range.start.ui_texts_index..range.end.ui_texts_index].iter().cloned());

        self.text_system.reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);
        self.next_frame.scene.replay(range.start.scene_index..range.end.scene_index, &self.rendered_frame.scene);
    }

    /// Push a text style onto the stack, and call a function with that style active.
    /// Use [`Window::text_style`] to get the current, combined text style. This method
    /// should only be called as part of element drawing.
    pub fn with_text_style<F, R>(&mut self, style: Option<TextStyleRefinement>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        if let Some(style) = style {
            self.text_style_stack.push(style);
            let result = f(self);
            self.text_style_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the cursor style at the platform level. This method should only be called
    /// during the paint phase of element drawing.
    pub fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.next_frame.cursor_styles.push(CursorStyleRequest { hitbox_id: Some(hitbox.id), style });
    }

    /// Updates the cursor style for the entire window at the platform level. A cursor
    /// style using this method will have precedence over any cursor style set using
    /// `set_cursor_style`. This method should only be called during the paint
    /// phase of element drawing.
    pub fn set_window_cursor_style(&mut self, style: CursorStyle) {
        self.next_frame.cursor_styles.push(CursorStyleRequest { hitbox_id: None, style });
    }

    /// Sets a tooltip to be rendered for the upcoming frame. This method should only be called
    /// during the paint phase of element drawing.
    pub fn set_tooltip(&mut self, tooltip: AnyTooltip) -> TooltipId {
        let id = TooltipId(post_inc(&mut self.next_tooltip_id.0));
        self.next_frame.tooltip_requests.push(Some(TooltipRequest { id, tooltip }));
        id
    }

    /// Invoke the given function with the given content mask after intersecting it
    /// with the current mask. This method should only be called during element drawing.
    // This function is called in a highly recursive manner in editor
    // prepainting, make sure its inlined to reduce the stack burden
    #[inline]
    pub fn with_content_mask<R>(&mut self, mask: Option<ContentMask<Pixels>>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(mask) = mask {
            let mask = mask.intersect(&self.content_mask());
            self.content_mask_stack.push(mask);
            let result = f(self);
            self.content_mask_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the global element offset relative to the current offset. This is used to implement
    /// scrolling. This method should only be called during the prepaint phase of element drawing.
    pub fn with_element_offset<R>(&mut self, offset: Point<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        if offset.is_zero() {
            return f(self);
        }

        let abs_offset = self.element_offset() + offset;
        self.with_absolute_element_offset(abs_offset, f)
    }

    /// Updates the global element offset based on the given offset. This is used to implement
    /// drag handles and other manual painting of elements. This method should only be called during
    /// the prepaint phase of element drawing.
    pub fn with_absolute_element_offset<R>(&mut self, offset: Point<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.element_offset_stack.push(offset);
        let result = f(self);
        self.element_offset_stack.pop();
        result
    }

    pub(crate) fn with_element_opacity<R>(&mut self, opacity: Option<f32>, f: impl FnOnce(&mut Self) -> R) -> R {
        let Some(opacity) = opacity else {
            return f(self);
        };

        let previous_opacity = self.element_opacity;
        self.element_opacity = previous_opacity * opacity;
        let result = f(self);
        self.element_opacity = previous_opacity;
        result
    }

    /// Perform prepaint on child elements in a "retryable" manner, so that any side effects
    /// of prepaints can be discarded before prepainting again. This is used to support autoscroll
    /// where we need to prepaint children to detect the autoscroll bounds, then adjust the
    /// element offset and prepaint again. See [`crate::List`] for an example. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn transact<T, U>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, U>) -> Result<T, U> {
        let index = self.prepaint_index();
        let result = f(self);
        if result.is_err() {
            self.next_frame.hitboxes.truncate(index.hitboxes_index);
            self.next_frame.tooltip_requests.truncate(index.tooltips_index);
            self.next_frame.deferred_draws.truncate(index.deferred_draws_index);
            self.next_frame.dispatch_tree.truncate(index.dispatch_tree_index);
            self.next_frame.accessed_element_states.truncate(index.accessed_element_states_index);
            self.text_system.truncate_layouts(index.line_layout_index);
        }
        result
    }

    /// When you call this method during [`Element::prepaint`], containing elements will attempt to
    /// scroll to cause the specified bounds to become visible. When they decide to autoscroll, they will call
    /// [`Element::prepaint`] again with a new set of bounds. See [`crate::List`] for an example of an element
    /// that supports this method being called on the elements it contains. This method should only be
    /// called during the prepaint phase of element drawing.
    pub const fn request_autoscroll(&mut self, bounds: Bounds<Pixels>) {
        self.requested_autoscroll = Some(bounds);
    }

    /// This method can be called from a containing element such as [`crate::List`] to support the autoscroll behavior
    /// described in [`Self::request_autoscroll`].
    pub const fn take_autoscroll(&mut self) -> Option<Bounds<Pixels>> {
        self.requested_autoscroll.take()
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    /// Your view will be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn use_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        cx.asset_entry::<A>(source).use_by(self.current_view())
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading or doesn't exist this will return None.
    /// Your view will not be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn get_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        cx.fetch_asset::<A>(source)
    }
    /// Obtain the current element offset. This method should only be called during the
    /// prepaint phase of element drawing.
    pub fn element_offset(&self) -> Point<Pixels> {
        self.element_offset_stack.last().copied().unwrap_or_default()
    }

    /// Obtain the current element opacity. This method should only be called during the
    /// prepaint phase of element drawing.
    #[inline]
    pub(crate) const fn element_opacity(&self) -> f32 {
        self.element_opacity
    }

    /// Obtain the current content mask. This method should only be called during element drawing.
    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.content_mask_stack
            .last()
            .copied()
            .unwrap_or_else(|| ContentMask { bounds: Bounds { origin: Point::default(), size: self.viewport_size } })
    }

    /// Provide elements in the called function with a new namespace in which their identifiers must be unique.
    /// This can be used within a custom element to distinguish multiple sets of child elements.
    pub fn with_element_namespace<R>(&mut self, element_id: impl Into<ElementId>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.element_id_stack.push(element_id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames.
    pub fn use_keyed_state<S: 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        let current_view = self.current_view();
        self.with_global_id(key.into(), |global_id, window| {
            window.with_element_state(global_id, |state: Option<Entity<S>>, window| {
                if let Some(state) = state {
                    (state.clone(), state)
                } else {
                    let new_state = cx.new(|cx| init(window, cx));
                    Self::observe_keyed_state(&new_state, current_view, cx);
                    (new_state.clone(), new_state)
                }
            })
        })
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames, without needing to specify a key.
    ///
    /// NOTE: This method uses the location of the caller to generate an ID for this state.
    ///       If this is not sufficient to identify your state (e.g. you're rendering a list item),
    ///       you can provide a custom ElementID using the `use_keyed_state` method.
    #[track_caller]
    pub fn use_state<S: 'static>(
        &mut self,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        self.use_keyed_state(ElementId::CodeLocation(*core::panic::Location::caller()), cx, init)
    }

    /// Updates or initializes state for an element with the given id that lives across multiple
    /// frames. If an element with this ID existed in the rendered frame, its state will be passed
    /// to the given closure. The state returned by the closure will be stored so it can be referenced
    /// when drawing the next frame. This method should only be called as part of element drawing.
    #[inline(always)]
    pub fn with_element_state<S, R>(
        &mut self,
        global_id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R
    where
        S: 'static,
    {
        let (key, state) = self.take_element_state(global_id, TypeId::of::<S>());

        if let Some(any) = state {
            let ElementStateBox { inner } = any;
            // Using the extra inner option to avoid needing to reallocate a new box.
            let Ok(mut state_box) = inner.downcast::<Option<S>>() else {
                panic!("invalid element state type for id, requested {}", any::type_name::<S>());
            };

            let state =
                state_box.take().expect("reentrant call to with_element_state for the same state type and element id");
            let (result, state) = f(Some(state), self);
            state_box.replace(state);
            self.insert_element_state(key, ElementStateBox { inner: state_box });
            result
        } else {
            let (result, state) = f(None, self);
            self.insert_element_state(key, ElementStateBox { inner: Box::new(Some(state)) });
            result
        }
    }

    /// A variant of `with_element_state` that allows the element's id to be optional. This is a convenience
    /// method for elements where the element id may or may not be assigned. Prefer using `with_element_state`
    /// when the element is guaranteed to have an id.
    ///
    /// The first option means 'no ID provided'
    /// The second option means 'not yet initialized'.
    pub fn with_optional_element_state<S, R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        f: impl FnOnce(Option<Option<S>>, &mut Self) -> (R, Option<S>),
    ) -> R
    where
        S: 'static,
    {
        if let Some(global_id) = global_id {
            self.with_element_state(global_id, |state, cx| {
                let (result, state) = f(Some(state), cx);
                let state = state.expect("you must return some state when you pass some element id");
                (result, state)
            })
        } else {
            f(None, self).0
        }
    }

    /// Executes the given closure within the context of a tab group.
    #[inline]
    pub fn with_tab_group<R>(&mut self, index: Option<isize>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(index) = index {
            self.next_frame.tab_stops.begin_group(index);
            let result = f(self);
            self.next_frame.tab_stops.end_group();
            result
        } else {
            f(self)
        }
    }

    /// Defers the drawing of the given element, scheduling it to be painted on top of the currently-drawn tree
    /// at a later time. The `priority` parameter determines the drawing order relative to other deferred elements,
    /// with higher values being drawn on top.
    ///
    /// When `content_mask` is provided, the deferred element will be clipped to that region during
    /// both prepaint and paint. When `None`, no additional clipping is applied.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn defer_draw(
        &mut self,
        element: AnyElement,
        absolute_offset: Point<Pixels>,
        priority: usize,
        content_mask: Option<ContentMask<Pixels>>,
    ) {
        let parent_node = self.next_frame.dispatch_tree.active_node_id().expect("dispatch tree has an active node");
        self.next_frame.deferred_draws.push(DeferredDraw {
            current_view: self.current_view(),
            parent_node,
            element_id_stack: self.element_id_stack.clone(),
            text_style_stack: self.text_style_stack.clone(),
            content_mask,
            rem_size: self.rem_size(),
            priority,
            element: Some(element),
            absolute_offset,
            prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
            paint_range: PaintIndex::default()..PaintIndex::default(),
            accessed_entities: FxHashSet::default(),
        });
    }

    /// Add a node to the layout tree for the current frame. Takes the `Style` of the element for which
    /// layout is being requested, along with the layout ids of any children. This method is called during
    /// calls to the [`Element::request_layout`] trait method and enables any element to participate in layout.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    #[must_use]
    pub fn request_layout(
        &mut self,
        style: Style,
        children: impl IntoIterator<Item = LayoutId>,
        cx: &mut App,
    ) -> LayoutId {
        cx.layout_id_buffer.clear();
        cx.layout_id_buffer.extend(children);
        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();

        self.layout_engine.as_mut().expect("layout engine is present outside compute_layout").request_layout(
            style,
            rem_size,
            scale_factor,
            &cx.layout_id_buffer,
        )
    }

    /// Add a node to the layout tree for the current frame. Instead of taking a `Style` and children,
    /// this variant takes a function that is invoked during layout so you can use arbitrary logic to
    /// determine the element's size. One place this is used internally is when measuring text.
    ///
    /// The given closure is invoked at layout time with the known dimensions and available space and
    /// returns a `Size`.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    pub fn request_measured_layout<F>(&mut self, style: Style, measure: F) -> LayoutId
    where
        F: Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels> + 'static,
    {
        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        self.layout_engine.as_mut().expect("layout engine is present outside compute_layout").request_measured_layout(
            style,
            rem_size,
            scale_factor,
            measure,
        )
    }

    /// Compute the layout for the given id within the given available space.
    /// This method is called for its side effect, typically by the framework prior to painting.
    /// After calling it, you can request the bounds of the given layout node id or any descendant.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn compute_layout(&mut self, layout_id: LayoutId, available_space: Size<AvailableSpace>, cx: &mut App) {
        let mut layout_engine = self.layout_engine.take().expect("compute_layout is not reentrant");
        layout_engine.compute_layout(layout_id, available_space, self, cx);
        self.layout_engine = Some(layout_engine);
    }

    /// Obtain the bounds computed for the given LayoutId relative to the window. This method will usually be invoked by
    /// GPUI itself automatically in order to pass your element its `Bounds` automatically.
    ///
    /// This method should only be called as part of element drawing.
    pub fn layout_bounds(&mut self, layout_id: LayoutId) -> Bounds<Pixels> {
        let scale_factor = self.scale_factor();
        let mut bounds = self
            .layout_engine
            .as_mut()
            .expect("layout engine is present outside compute_layout")
            .layout_bounds(layout_id, scale_factor)
            .map(Into::into);
        let snapped_offset = self.pixel_snap_point(self.element_offset());
        bounds.origin += snapped_offset;
        bounds
    }

    /// This method should be called during `prepaint`. You can use
    /// the returned [Hitbox] during `paint` or in an event handler
    /// to determine whether the inserted hitbox was the topmost.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn insert_hitbox(&mut self, bounds: Bounds<Pixels>, behavior: HitboxBehavior) -> Hitbox {
        let content_mask = self.content_mask();
        let id = self.next_hitbox_id;
        self.next_hitbox_id = self.next_hitbox_id.next();
        let hitbox = Hitbox { id, bounds, content_mask, behavior };
        self.next_frame.hitboxes.push(hitbox.clone());
        hitbox
    }

    /// Set a hitbox which will act as a control area of the platform window.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn insert_window_control_hitbox(&mut self, area: WindowControlArea, hitbox: Hitbox) {
        self.next_frame.window_control_hitboxes.push((area, hitbox));
    }

    /// Sets the key context for the current element. This context will be used to translate
    /// keybindings into actions.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn set_key_context(&mut self, context: KeyContext) {
        self.next_frame.dispatch_tree.set_key_context(context);
    }

    /// Sets the focus handle for the current element. This handle will be used to manage focus state
    /// and keyboard event dispatch for the element.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn set_focus_handle(&mut self, focus_handle: &FocusHandle, _: &App) {
        if focus_handle.is_focused(self) {
            self.next_frame.focus = Some(focus_handle.id);
        }
        self.next_frame.dispatch_tree.set_focus_id(focus_handle.id);
    }

    /// Sets the view id for the current element, which will be used to manage view caching.
    ///
    /// This method should only be called as part of element prepaint. We plan on removing this
    /// method eventually when we solve some issues that require us to construct editor elements
    /// directly instead of always using editors via views.
    pub fn set_view_id(&mut self, view_id: EntityId) {
        self.next_frame.dispatch_tree.set_view_id(view_id);
    }

    /// Get the entity ID for the currently rendering view.
    pub fn current_view(&self) -> EntityId {
        self.rendered_entity_stack.last().copied().expect("called while rendering a view")
    }

    #[inline]
    pub(crate) fn with_rendered_view<R>(&mut self, id: EntityId, f: impl FnOnce(&mut Self) -> R) -> R {
        self.rendered_entity_stack.push(id);
        let result = f(self);
        self.rendered_entity_stack.pop();
        result
    }

    /// Executes the provided function with the specified image cache.
    pub fn with_image_cache<F, R>(&mut self, image_cache: Option<AnyImageCache>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        if let Some(image_cache) = image_cache {
            self.image_cache_stack.push(image_cache);
            let result = f(self);
            self.image_cache_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Sets an input handler, such as [`ElementInputHandler`][element_input_handler], which interfaces with the
    /// platform to receive textual input with proper integration with concerns such
    /// as IME interactions. This handler will be active for the upcoming frame until the following frame is
    /// rendered.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// [element_input_handler]: crate::ElementInputHandler
    pub fn handle_input(&mut self, focus_handle: &FocusHandle, input_handler: impl InputHandler, cx: &App) {
        if focus_handle.is_focused(self) {
            let cx = self.to_async(cx);
            self.next_frame.input_handlers.push(Some(PlatformInputHandler::new(cx, Box::new(input_handler))));
        }
    }

    /// Register a mouse event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_mouse_event<Event: MouseEvent>(
        &mut self,
        mut listener: impl FnMut(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.next_frame.mouse_listeners.push(Some(Box::new(
            move |event: &dyn Any, phase: DispatchPhase, window: &mut Window, cx: &mut App| {
                if let Some(event) = event.downcast_ref() {
                    listener(event, phase, window, cx);
                }
            },
        )));
    }

    /// Register a key event listener on this node for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a listener yourself.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_key_event<Event: KeyEvent>(
        &mut self,
        listener: impl Fn(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.next_frame.dispatch_tree.on_key_event(Rc::new(
            move |event: &dyn Any, phase, window: &mut Window, cx: &mut App| {
                if let Some(event) = event.downcast_ref::<Event>() {
                    listener(event, phase, window, cx);
                }
            },
        ));
    }

    /// Register a modifiers changed event listener on the window for the next frame.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_modifiers_changed(&mut self, listener: impl Fn(&ModifiersChangedEvent, &mut Window, &mut App) + 'static) {
        self.next_frame.dispatch_tree.on_modifiers_changed(Rc::new(
            move |event: &ModifiersChangedEvent, window: &mut Window, cx: &mut App| listener(event, window, cx),
        ));
    }

    /// Register a listener to be called when the given focus handle or one of its descendants receives focus.
    /// This does not fire if the given focus handle - or one of its descendants - was previously focused.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_in(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) = self.new_focus_listener(Box::new(move |event, window, cx| {
            if event.is_focus_in(focus_id) {
                listener(window, cx);
            }
            true
        }));
        cx.defer(move |_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle or one of its descendants loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_out(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(FocusOutEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) = self.new_focus_listener(Box::new(move |event, window, cx| {
            if let Some(blurred_id) = event.previous_focus_path.last().copied()
                && event.is_focus_out(focus_id)
            {
                let event = FocusOutEvent {
                    blurred: WeakFocusHandle { id: blurred_id, handles: Arc::downgrade(&cx.focus_handles) },
                };
                listener(event, window, cx);
            }
            true
        }));
        cx.defer(move |_| activate());
        subscription
    }

    fn reset_cursor_style(&self, cx: &App) {
        // Set the cursor only if we're the active window.
        if self.is_window_active() {
            // A drag can start before the next paint records its cursor in the frame.
            let style = cx
                .active_drag_cursor_style()
                .or_else(|| self.rendered_frame.cursor_style(self))
                .unwrap_or(CursorStyle::Arrow);
            cx.platform.set_cursor_style(style);
        }
    }

    /// Register the given handler to be invoked whenever the global of the given type
    /// is updated.
    pub fn observe_global<G: Global>(
        &mut self,
        cx: &mut App,
        f: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let window_handle = self.handle;
        let (subscription, activate) = cx.global_observers.insert_pending(
            TypeId::of::<G>(),
            Box::new(move |cx| window_handle.update(cx, |_, window, cx| f(window, cx)).is_ok()),
        );
        cx.defer(move |_| activate());
        subscription
    }

    /// Focus the current window and bring it to the foreground at the platform level.
    pub fn activate_window(&self) {
        self.platform_window.activate();
    }

    /// Requests that the operating system draw attention to this window.
    pub fn request_attention(&self) {
        self.platform_window.request_attention();
    }

    /// Minimize the current window at the platform level.
    pub fn minimize_window(&self) {
        self.platform_window.minimize();
    }

    /// Toggle full screen status on the current window at the platform level.
    pub fn toggle_fullscreen(&self) {
        self.platform_window.toggle_fullscreen();
    }

    /// Toggle simple (borderless) fullscreen, where the window covers the entire
    /// screen including the menu bar and, on notched displays, the area around the
    /// notch. Unlike [`Window::toggle_fullscreen`], this does not move the window
    /// into its own Mission Control space. Only has an effect on macOS.
    pub fn toggle_simple_fullscreen(&self) {
        self.platform_window.toggle_simple_fullscreen();
    }

    /// Updates the IME panel position suggestions for languages like japanese, chinese.
    pub fn invalidate_character_coordinates(&self) {
        self.on_next_frame(|window, cx| {
            if let Some(mut input_handler) = window.platform_window.take_input_handler() {
                if let Some(bounds) = input_handler.selected_bounds(window, cx) {
                    window.platform_window.update_ime_position(bounds);
                }
                window.platform_window.set_input_handler(input_handler);
            }
        });
    }

    /// Present a platform dialog.
    /// The provided message will be presented, along with buttons for each answer.
    /// When a button is clicked, the returned Receiver will receive the index of the clicked button.
    pub fn prompt<T>(
        &mut self,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[T],
        cx: &mut App,
    ) -> oneshot::Receiver<usize>
    where
        T: Clone + Into<PromptButton>,
    {
        let answers = answers.iter().map(|answer| answer.clone().into()).collect::<Vec<_>>();
        self.platform_window.prompt(level, message, detail, &answers).unwrap_or_else(|| {
            let (sender, receiver) = oneshot::channel();
            let handle = PromptHandle::new(sender);
            self.prompt = Some(fallback_prompt_renderer(level, message, detail, &answers, handle, self, cx));
            receiver
        })
    }

    /// Returns whether a prompt rendered by GPUI is currently active in this window.
    ///
    /// This is only true for prompts rendered in the window, not for platform-native prompt dialogs.
    pub const fn has_active_prompt(&self) -> bool {
        self.prompt.is_some()
    }

    /// Returns a generic event listener that invokes the given listener with the view and context associated with the given view handle.
    pub fn listener_for<T: 'static, E>(
        &self,
        view: &Entity<T>,
        f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = view.downgrade();
        move |e: &E, window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| f(view, e, window, cx)).ok();
        }
    }

    /// Returns a generic handler that invokes the given handler with the view and context associated with the given view handle.
    pub fn handler_for<E: 'static, Callback: Fn(&mut E, &mut Window, &mut Context<E>) + 'static>(
        &self,
        entity: &Entity<E>,
        f: Callback,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let entity = entity.downgrade();
        move |window: &mut Window, cx: &mut App| {
            entity.update(cx, |entity, cx| f(entity, window, cx)).ok();
        }
    }

    /// Register a callback that can interrupt the closing of the current window based the returned boolean.
    /// If the callback returns false, the window won't be closed.
    pub fn on_window_should_close(&self, cx: &App, f: impl Fn(&mut Window, &mut App) -> bool + 'static) {
        let mut cx = self.to_async(cx);
        self.platform_window.on_should_close(Box::new(move || cx.update(|window, cx| f(window, cx)).unwrap_or(true)));
    }

    /// Attach `table` to the current dispatch node for the next frame, with `entity` as the view
    /// its handlers run on. Costs one push however many actions the table handles, plus one call
    /// per distinct condition of the table; those read `entity`, which must not be being updated.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_action_table<V: 'static>(&mut self, entity: &Entity<V>, table: &'static ActionTable<V>, cx: &App) {
        let mut table = DispatchActionTable::new(entity, table);
        table.evaluate_conditions(cx);
        self.next_frame.dispatch_tree.on_action_table(table);
    }

    /// Perform titlebar double-click action.
    /// This is macOS specific.
    pub fn titlebar_double_click(&self) {
        self.platform_window.titlebar_double_click(self.is_resizable, self.is_minimizable);
    }

    /// Gets the window's title at the platform level.
    /// This is macOS specific.
    pub fn window_title(&self) -> String {
        self.platform_window.get_title()
    }

    /// Request the OS to play an alert sound. On some platforms this is associated
    /// with the window, for others it's just a simple global function call.
    pub fn play_system_bell(&self) {
        self.platform_window.play_system_bell();
    }

    #[inline(never)]
    fn take_element_state(
        &mut self,
        global_id: &GlobalElementId,
        state_type: TypeId,
    ) -> ((GlobalElementId, TypeId), Option<ElementStateBox>) {
        let key = (global_id.clone(), state_type);
        self.next_frame.accessed_element_states.push(key.clone());
        let state =
            self.next_frame.element_states.remove(&key).or_else(|| self.rendered_frame.element_states.remove(&key));
        (key, state)
    }

    #[inline(never)]
    fn insert_element_state(
        &mut self,
        key: (GlobalElementId, TypeId),
        state: ElementStateBox,
    ) -> Option<ElementStateBox> {
        self.next_frame.element_states.insert(key, state)
    }

    #[inline(never)]
    fn observe_keyed_state<S: 'static>(state: &Entity<S>, current_view: EntityId, cx: &mut App) {
        cx.observe(state, move |_, cx| {
            cx.notify(current_view);
        })
        .detach();
    }
}

/// Indicates which region of the window is visible. Content falling outside of this mask will not be
/// rendered. Currently, only rectangular content masks are supported, but we give the mask its own type
/// to leave room to support more complex shapes in the future.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ContentMask<P: Clone + Debug + Default + PartialEq> {
    /// The bounds.
    pub bounds: Bounds<P>,
}

impl ContentMask<Pixels> {
    /// Scale the content mask's pixel units by the given scaling factor.
    pub fn scale(&self, factor: f32) -> ContentMask<ScaledPixels> {
        ContentMask { bounds: self.bounds.scale(factor) }
    }

    /// Intersect the content mask with the given content mask.
    pub fn intersect(&self, other: &Self) -> Self {
        let bounds = self.bounds.intersect(&other.bounds);
        ContentMask { bounds }
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, HandleError> {
        self.platform_window.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> result::Result<raw_window_handle::DisplayHandle<'_>, HandleError> {
        self.platform_window.display_handle()
    }
}

#[inline(never)]
fn with_element_arena_erased(callback: &mut dyn FnMut(&mut Arena)) -> Result<(), AccessError> {
    if let Some(arena_pointer) = CURRENT_ELEMENT_ARENA.try_with(Cell::get)? {
        // SAFETY: The pointer is valid for the duration of the draw operation
        // that set it, and we're being called during that same draw.
        let arena_cell = unsafe { &*arena_pointer };
        callback(&mut arena_cell.borrow_mut());
        Ok(())
    } else {
        ELEMENT_ARENA.try_with(|arena| callback(&mut arena.borrow_mut()))
    }
}
