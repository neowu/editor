#![allow(clippy::type_complexity)] // Not useful, GPUI makes heavy use of callbacks

use std::borrow;
#[cfg(not(target_os = "macos"))]
compile_error!("macOS is the only supported target.");

extern crate self as gpui;

#[macro_use]
mod action;
mod action_table;
mod app;

mod arena;
mod asset_cache;
mod assets;
mod bounds_tree;
mod color;
/// The default colors used by GPUI.
mod element;
mod elements;
mod executor;
mod frame_stats;
mod geometry;
mod global;
mod input;
mod interactive;
mod key_dispatch;
mod keymap;
mod path_builder;
mod platform;
pub mod prelude;
mod scene;
mod scroll_axis_lock;
mod spring;
mod style;
mod styled;
mod subscription;
mod svg_renderer;
mod tab_stop;
mod taffy;
mod text_system;
mod ui_snapshot;
mod util;
mod view;
mod window;

/// Do not touch, here be dragons for use by gpui_macros and such.
#[doc(hidden)]
pub mod private {
    pub use inventory;
}

mod seal {
    /// A mechanism for restricting implementations of a trait to only those in GPUI.
    /// See: <https://predr.ag/blog/definitive-guide-to-sealed-traits-in-rust/>.
    pub trait Sealed {}
}

use std::any::Any;
use std::future::Future;

pub use action::Action;
pub(crate) use action::ActionRegistry;
pub use action::MacroActionData;
pub use action::NoAction;
pub use action::Unbind;
pub use action::is_no_action;
pub use action::is_unbind;
pub use action_table::ActionTable;
pub(crate) use action_table::DispatchActionTable;
pub use anyhow::Result;
pub use app::AnyDrag;
pub use app::AnyEntity;
pub use app::AnyTooltip;
pub use app::AnyWeakEntity;
pub use app::App;
pub use app::AppCell;
pub use app::Application;
pub use app::AsyncApp;
pub use app::AsyncWindowContext;
pub use app::Context;
pub(crate) use app::Effect;
pub use app::Entity;
pub use app::EntityId;
#[cfg(test)]
pub(crate) use app::EntityMap;
pub use app::GpuiBorrow;
pub use app::InputPreference;
pub use app::SHUTDOWN_TIMEOUT;
pub(crate) use app::Slot;
pub use app::WeakEntity;
pub(crate) use arena::Arena;
pub(crate) use arena::ArenaBox;
pub use asset_cache::Asset;
pub use asset_cache::AssetLogger;
pub use asset_cache::Resource;
pub use asset_cache::hash;
pub use assets::AssetSource;
pub use assets::ImageId;
pub use assets::RenderImage;
pub use assets::RenderImageParams;
pub use color::Background;
pub(crate) use color::BackgroundTag;
pub use color::Hsla;
pub use color::Rgba;
pub use color::black;
pub use color::blue;
pub use color::checkerboard;
pub use color::green;
pub use color::hsla;
pub use color::linear_color_stop;
pub use color::linear_gradient;
pub use color::pattern_slash;
pub use color::red;
pub use color::rgb;
pub use color::rgba;
pub use color::solid_background;
pub use color::swap_rgba_pa_to_bgra;
pub use color::transparent_black;
pub use color::transparent_white;
pub use color::white;
pub use color::yellow;
pub use element::AnyElement;
pub(crate) use element::EMPTY_PATH_HASH;
pub use element::Element;
pub use element::Empty;
pub use element::GlobalElementId;
pub use element::IntoElement;
pub use element::ParentElement;
pub use element::Render;
pub use element::RenderOnce;
pub(crate) use element::extend_path_hash;
pub use elements::Anchored;
pub use elements::AnchoredPositionMode;
pub use elements::Animation;
pub use elements::AnimationElement;
pub use elements::AnimationExt;
pub use elements::AnyImageCache;
pub use elements::Deferred;
pub use elements::Div;
pub use elements::DivFrameState;
pub use elements::DragMoveEvent;
pub use elements::ImageAssetLoader;
pub use elements::ImageCache;
pub use elements::ImageCacheElement;
pub use elements::ImageCacheError;
pub use elements::ImageCacheProvider;
pub use elements::ImageSource;
pub use elements::Img;
pub use elements::ImgResourceLoader;
pub use elements::InteractiveElement;
pub use elements::Interactivity;
pub use elements::List;
pub use elements::ListAlignment;
pub use elements::ListHorizontalSizingBehavior;
pub use elements::ListOffset;
pub use elements::ListSizingBehavior;
pub use elements::ListState;
pub use elements::RetainAllImageCache;
pub use elements::ScrollHandle;
pub use elements::ScrollStrategy;
pub use elements::Stateful;
pub use elements::StatefulInteractiveElement;
pub use elements::StyledImage;
pub use elements::StyledText;
pub use elements::Svg;
pub use elements::TextLayout;
pub use elements::Transformation;
pub use elements::UniformList;
pub use elements::UniformListDecoration;
pub use elements::UniformListScrollHandle;
pub use elements::anchored;
pub use elements::canvas;
pub use elements::deferred;
pub use elements::div;
pub use elements::ease_in_out;
pub use elements::ease_out_quint;
pub use elements::image_cache;
pub use elements::img;
pub use elements::linear;
pub use elements::list;
pub use elements::pulsating_between;
pub use elements::svg;
pub use elements::uniform_list;
pub use executor::BackgroundExecutor;
pub use executor::ForegroundExecutor;
pub use executor::Priority;
pub use executor::RunnableMeta;
pub use executor::Scope;
pub use executor::Task;
pub use executor::TaskExt;
pub use frame_stats::DurationSummary;
pub use frame_stats::FrameStats;
pub use frame_stats::FrameStatsSummary;
pub use geometry::AbsoluteLength;
pub use geometry::Along;
pub use geometry::Anchor;
pub use geometry::Axis;
pub use geometry::Bounds;
pub use geometry::Corners;
pub use geometry::CornersRefinement;
pub use geometry::DefiniteLength;
pub use geometry::DevicePixels;
pub use geometry::Edges;
pub use geometry::EdgesRefinement;
pub use geometry::GridLocation;
pub use geometry::GridPlacement;
pub use geometry::Half;
pub use geometry::IsZero;
pub use geometry::Length;
pub use geometry::Percentage;
pub use geometry::Pixels;
pub use geometry::Point;
pub use geometry::PointRefinement;
pub use geometry::Radians;
pub use geometry::Rems;
pub use geometry::ScaledPixels;
pub use geometry::Size;
pub use geometry::SizeRefinement;
pub use geometry::auto;
pub use geometry::bounds;
pub use geometry::percentage;
pub use geometry::phi;
pub use geometry::point;
pub use geometry::px;
pub use geometry::radians;
pub use geometry::relative;
pub use geometry::rems;
pub use geometry::size;
pub use global::Global;
pub use global::ReadGlobal;
pub use global::UpdateGlobal;
pub use gpui_macros::AppContext;
pub use gpui_macros::IntoElement;
pub use gpui_macros::Render;
pub use gpui_macros::VisualContext;
pub use gpui_shared_string::SharedString;
pub use input::ElementInputHandler;
pub use input::EntityInputHandler;
pub use interactive::ClickEvent;
pub use interactive::ExternalPaths;
pub use interactive::KeyDownEvent;
pub use interactive::KeyEvent;
pub use interactive::KeyUpEvent;
pub use interactive::KeyboardButton;
pub use interactive::KeyboardClickEvent;
pub use interactive::ModifiersChangedEvent;
pub use interactive::MouseButton;
pub use interactive::MouseClickEvent;
pub use interactive::MouseDownEvent;
pub use interactive::MouseEvent;
pub use interactive::MouseExitEvent;
pub use interactive::MouseMoveEvent;
pub use interactive::MousePressureEvent;
pub use interactive::MouseUpEvent;
pub use interactive::NavigationDirection;
pub use interactive::PinchEvent;
pub use interactive::PlatformInput;
pub use interactive::PressureStage;
pub use interactive::ScrollDelta;
pub use interactive::ScrollWheelEvent;
pub use interactive::TouchPhase;
use key_dispatch::DispatchNodeId;
use key_dispatch::DispatchTree;
use key_dispatch::Replay;
pub use keymap::KeyBinding;
pub use keymap::KeyBindingContextPredicate;
pub use keymap::KeyContext;
pub use keymap::Keymap;
pub use path_builder::PathBuilder;
pub use path_builder::Transform;
pub use platform::ActivationPolicy;
pub use platform::AsKeystroke;
pub use platform::AtlasBackend;
pub use platform::AtlasKey;
pub use platform::AtlasState;
pub use platform::AtlasTextureId;
pub use platform::AtlasTextureKind;
pub use platform::AtlasTextureList;
pub use platform::AtlasTile;
pub use platform::Capslock;
pub use platform::ClipboardEntry;
pub use platform::ClipboardItem;
pub use platform::ClipboardString;
pub use platform::CursorStyle;
pub use platform::DisplayId;
pub use platform::Image;
pub use platform::ImageFormat;
pub use platform::InputHandler;
pub use platform::InvalidKeystrokeError;
pub use platform::KeybindingKeystroke;
pub use platform::Keystroke;
pub use platform::Menu;
pub use platform::MenuItem;
pub use platform::Modifiers;
pub use platform::NoopTextSystem;
pub use platform::OsAction;
pub use platform::OsMenu;
pub use platform::PathPromptOptions;
pub use platform::Platform;
pub use platform::PlatformAtlas;
pub use platform::PlatformDispatcher;
pub use platform::PlatformDisplay;
pub use platform::PlatformInputHandler;
pub use platform::PlatformTextSystem;
pub use platform::PlatformWindow;
pub use platform::PromptButton;
pub use platform::PromptLevel;
pub use platform::RequestFrameOptions;
pub use platform::RunnableVariant;
pub use platform::SystemMenuType;
pub use platform::TileId;
pub use platform::TitlebarOptions;
pub use platform::UTF16Selection;
pub use platform::WindowAppearance;
pub use platform::WindowBackgroundAppearance;
pub use platform::WindowBounds;
pub use platform::WindowKind;
pub use platform::WindowOptions;
pub use platform::WindowParams;
pub use platform::WindowVisibility;
pub(crate) use platform::decode_gif_image;
pub(crate) use platform::decode_static_image;
pub(crate) use platform::decode_webp_image;
pub(crate) use platform::init_app_menus;
pub use platform::refresh_interval_from_hz;
pub use refineable::IsEmpty;
pub use refineable::Refineable;
pub use scene::BorderStyle;
pub use scene::MonochromeSprite;
pub use scene::Path;
pub use scene::PolychromeSprite;
pub use scene::PrimitiveBatch;
pub use scene::Quad;
pub use scene::Scene;
pub use scene::Shadow;
pub(crate) use scene::SpriteAnimation;
pub use scene::TransformationMatrix;
pub use scene::Underline;
pub use scroll_axis_lock::OngoingScroll;
pub use spring::Decay;
pub use spring::SpringConfig;
pub use spring::SpringState;
pub use spring::motion;
pub use style::AlignContent;
pub use style::AlignItems;
pub use style::AlignSelf;
pub use style::BoxShadow;
pub use style::Display;
pub use style::Fill;
pub use style::FlexDirection;
pub use style::FlexWrap;
pub use style::GridTemplate;
pub use style::GridTemplateMinSize;
pub use style::HighlightStyle;
pub use style::JustifyContent;
pub use style::ObjectFit;
pub use style::Overflow;
pub use style::Position;
pub use style::StrikethroughStyle;
pub use style::Style;
pub use style::StyleRefinement;
pub use style::TextAlign;
pub use style::TextOverflow;
pub use style::TextStyle;
pub use style::TextStyleRefinement;
pub use style::UnderlineStyle;
pub use style::Visibility;
pub use style::WhiteSpace;
pub use style::combine_highlights;
pub use styled::Styled;
pub use styled::margin_style_methods;
pub use styled::padding_style_methods;
pub(crate) use subscription::SingleSubscriberSet;
pub(crate) use subscription::SubscriberSet;
pub use subscription::Subscription;
pub use svg_renderer::RenderSvgParams;
pub use svg_renderer::SMOOTH_SVG_SCALE_FACTOR;
pub use svg_renderer::SvgRenderer;
pub(crate) use tab_stop::TabStopMap;
pub use taffy::AvailableSpace;
pub use taffy::LayoutId;
pub use taffy::MeasureState;
use taffy::TaffyLayoutEngine;
pub use text_system::Boundary;
pub use text_system::DecorationRun;
pub use text_system::Font;
pub use text_system::FontFeatures;
pub use text_system::FontId;
pub use text_system::FontMetrics;
pub use text_system::FontRun;
pub use text_system::FontStyle;
pub use text_system::FontWeight;
pub use text_system::GlyphId;
pub use text_system::IndentAdjustment;
pub use text_system::LineFragment;
pub use text_system::LineLayout;
pub(crate) use text_system::LineLayoutIndex;
pub use text_system::LineWrapper;
pub use text_system::RenderGlyphParams;
pub use text_system::SUBPIXEL_VARIANTS_X;
pub use text_system::SUBPIXEL_VARIANTS_Y;
pub use text_system::ShapedGlyph;
pub use text_system::ShapedLine;
pub use text_system::ShapedRun;
pub use text_system::TextRun;
pub use text_system::TextSystem;
pub use text_system::TruncateFrom;
pub use text_system::WindowTextSystem;
pub use text_system::WrapBoundary;
pub use text_system::WrappedLine;
pub use text_system::WrappedLineLayout;
pub use text_system::font;
pub use text_system::font_name_with_fallbacks;
pub use text_system::underline_y_offset;
pub use ui_snapshot::UiElement;
pub use ui_snapshot::UiText;
pub use util::FutureExt;
pub use util::Timeout;
pub use util::block_on;
pub use view::AnyView;
pub use view::AnyWeakView;
pub use view::View;
pub use view::ViewElement;
pub use window::AnyWindowHandle;
pub use window::ContentMask;
pub use window::DEFAULT_WINDOW_SIZE;
pub use window::DismissEvent;
pub use window::DispatchEventResult;
pub use window::DispatchPhase;
pub use window::ElementId;
pub use window::FocusHandle;
pub use window::FocusId;
pub(crate) use window::FocusMap;
pub use window::FocusOutEvent;
pub use window::Focusable;
pub use window::Hitbox;
pub use window::HitboxBehavior;
pub use window::HitboxId;
pub use window::ManagedView;
pub(crate) use window::PaintIndex;
pub use window::PaintQuad;
pub(crate) use window::PrepaintStateIndex;
pub use window::TooltipId;
pub use window::WeakFocusHandle;
pub use window::Window;
pub use window::WindowControlArea;
pub use window::WindowHandle;
pub use window::WindowId;
pub(crate) use window::WindowInvalidator;
pub use window::fill;
pub use window::outline;
pub use window::quad;

/// The context trait, allows the different contexts in GPUI to be used
/// interchangeably for certain operations.
pub trait AppContext {
    /// Create a new entity in the app context.
    #[expect(
        clippy::wrong_self_convention,
        clippy::new_ret_no_self,
        reason = "`App::new` is an ubiquitous function for creating entities"
    )]
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T>;

    /// Reserve a slot for a entity to be inserted later.
    /// The returned [Reservation] allows you to obtain the [EntityId] for the future entity.
    fn reserve_entity<T: 'static>(&mut self) -> Reservation<T>;

    /// Insert a new entity in the app context based on a [Reservation] previously obtained from [`reserve_entity`].
    ///
    /// [`reserve_entity`]: Self::reserve_entity
    fn insert_entity<T: 'static>(
        &mut self,
        reservation: Reservation<T>,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T>;

    /// Update a entity in the app context.
    fn update_entity<T, R>(&mut self, handle: &Entity<T>, update: impl FnOnce(&mut T, &mut Context<T>) -> R) -> R
    where
        T: 'static;

    /// Update a entity in the app context.
    fn as_mut<'a, T>(&'a mut self, handle: &Entity<T>) -> GpuiBorrow<'a, T>
    where
        T: 'static;

    /// Read a entity from the app context.
    fn read_entity<T, R>(&self, handle: &Entity<T>, read: impl FnOnce(&T, &App) -> R) -> R
    where
        T: 'static;

    /// Update a window for the given handle.
    fn update_window<T, F>(&mut self, window: AnyWindowHandle, f: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T;

    /// Run `f` against the entity's *current* window — the most recently
    /// rendered window that referenced the entity. Returns `None` if the
    /// entity has no current window or that window is unavailable. See
    /// [`App::with_window`] for the underlying lookup.
    fn with_window<R>(&mut self, entity_id: EntityId, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R>;

    /// Read a window off of the application context.
    fn read_window<T, R>(&self, window: &WindowHandle<T>, read: impl FnOnce(Entity<T>, &App) -> R) -> Result<R>
    where
        T: 'static;

    /// Spawn a future on a background thread.
    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static;

    /// Read a global from this app context.
    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> R
    where
        G: Global;
}

/// Returned by [Context::reserve_entity] to later be passed to [Context::insert_entity].
/// Allows you to obtain the [EntityId] for a entity before it is created.
pub struct Reservation<T>(pub(crate) Slot<T>);

impl<T: 'static> Reservation<T> {
    /// Returns the [EntityId] that will be associated with the entity once it is inserted.
    pub fn entity_id(&self) -> EntityId {
        self.0.entity_id()
    }
}

/// This trait is used for the different visual contexts in GPUI that
/// require a window to be present.
pub trait VisualContext: AppContext {
    /// The result type for window operations.
    type Result<T>;

    /// Returns the handle of the window associated with this context.
    fn window_handle(&self) -> AnyWindowHandle;

    /// Update a view with the given callback.
    fn update_window_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R,
    ) -> Self::Result<R>;

    /// Create a new entity, with access to `Window`.
    fn new_window_entity<T: 'static>(
        &mut self,
        build_entity: impl FnOnce(&mut Window, &mut Context<T>) -> T,
    ) -> Self::Result<Entity<T>>;

    /// Replace the root view of a window with a new view.
    fn replace_root_view<V>(
        &mut self,
        build_view: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) -> Self::Result<Entity<V>>
    where
        V: 'static + Render;

    /// Focus a entity in the window, if it implements the [`Focusable`] trait.
    fn focus<V>(&mut self, entity: &Entity<V>) -> Self::Result<()>
    where
        V: Focusable;
}

/// A trait for tying together the types of a GPUI entity and the events it can
/// emit.
pub trait EventEmitter<E: Any>: 'static {}

/// A helper trait for auto-implementing certain methods on contexts that
/// can be used interchangeably.
pub trait BorrowAppContext {
    /// Set a global value on the context.
    fn set_global<T: Global>(&mut self, global: T);
    /// Updates the global state of the given type.
    fn update_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global;
    /// Updates the global state of the given type, creating a default if it didn't exist before.
    fn update_default_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global + Default;
}

impl<C> BorrowAppContext for C
where
    C: borrow::BorrowMut<App>,
{
    fn set_global<G: Global>(&mut self, global: G) {
        self.borrow_mut().set_global(global);
    }

    #[track_caller]
    fn update_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global,
    {
        let mut global = self.borrow_mut().lease_global::<G>();
        let result = f(&mut global, self);
        self.borrow_mut().end_global_lease(global);
        result
    }

    fn update_default_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R
    where
        G: Global + Default,
    {
        self.borrow_mut().default_global::<G>();
        self.update_global(f)
    }
}
