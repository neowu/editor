use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

use actions::menu;
use actions::menu::SelectChild;
use actions::menu::SelectFirst;
use actions::menu::SelectLast;
use actions::menu::SelectNext;
use actions::menu::SelectParent;
use actions::menu::SelectPrevious;
use gpui::Action;
use gpui::Anchor;
use gpui::AnchoredPositionMode;
use gpui::AnyElement;
use gpui::App;
use gpui::AppContext as _;
use gpui::Bounds;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::MouseUpEvent;
use gpui::Pixels;
use gpui::Point;
use gpui::Size;
use gpui::Subscription;
use gpui::anchored;
use gpui::canvas;
use gpui::deferred;
use gpui::point;
use gpui::prelude::Context;
use gpui::prelude::FluentBuilder;
use gpui::prelude::InteractiveElement as _;
use gpui::prelude::IntoElement;
use gpui::prelude::ParentElement as _;
use gpui::prelude::Render;
use gpui::prelude::StatefulInteractiveElement as _;
use gpui::prelude::Styled as _;
use gpui::px;
use gpui::relative;
use theme::built_in::BUFFER_LINE_HEIGHT;

use crate::KeyBinding;
use crate::List;
use crate::ListItem;
use crate::ListItemSpacing;
use crate::ListSeparator;
use crate::ListSubHeader;
use crate::font;
use crate::prelude::Color;
use crate::prelude::DefiniteLength;
use crate::prelude::Disableable as _;
use crate::prelude::Icon;
use crate::prelude::IconName;
use crate::prelude::IconPosition;
use crate::prelude::IconSize;
use crate::prelude::Label;
use crate::prelude::LabelCommon as _;
use crate::prelude::SharedString;
use crate::prelude::StyledExt as _;
use crate::prelude::Toggleable as _;
use crate::prelude::Window;
use crate::prelude::div;
use crate::prelude::h_flex;
use crate::prelude::rems_from_px;
use crate::prelude::v_flex;
use crate::prelude::vh;
use crate::utils::WithRemSize;

const CONTEXT_MENU_MIN_WIDTH: Pixels = px(200.);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SubmenuOpenTrigger {
    Pointer,
    Keyboard,
}

struct OpenSubmenu {
    item_index: usize,
    entity: Entity<ContextMenu>,
    _dismiss_subscription: Subscription,
}

enum SubmenuState {
    Closed,
    Open(OpenSubmenu),
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum HoverTarget {
    #[default]
    None,
    MainMenu,
    Submenu,
}

pub enum ContextMenuItem {
    Separator,
    Header(SharedString),
    Label(SharedString),
    Entry(ContextMenuEntry),
    CustomEntry {
        #[allow(clippy::type_complexity)]
        entry_render: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
        #[allow(clippy::type_complexity)]
        handler: Rc<dyn Fn(Option<&FocusHandle>, &mut Window, &mut App)>,
        selectable: bool,
    },
    Submenu {
        label: SharedString,
        icon: Option<IconName>,
        icon_color: Option<Color>,
        #[allow(clippy::type_complexity)]
        builder: Rc<dyn Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu>,
    },
}

impl ContextMenuItem {
    fn is_selectable(&self) -> bool {
        match self {
            ContextMenuItem::Header(_) | ContextMenuItem::Separator | ContextMenuItem::Label { .. } => false,
            ContextMenuItem::Entry(ContextMenuEntry { disabled, .. }) => !disabled,
            ContextMenuItem::CustomEntry { selectable, .. } => *selectable,
            ContextMenuItem::Submenu { .. } => true,
        }
    }
}

pub struct ContextMenuEntry {
    toggle: Option<(IconPosition, bool)>,
    label: SharedString,
    icon: Option<IconName>,
    custom_icon_path: Option<SharedString>,
    custom_icon_svg: Option<SharedString>,
    icon_position: IconPosition,
    icon_size: IconSize,
    icon_color: Option<Color>,
    #[allow(clippy::type_complexity)]
    handler: Rc<dyn Fn(Option<&FocusHandle>, &mut Window, &mut App)>,
    #[allow(clippy::type_complexity)]
    secondary_handler: Option<Rc<dyn Fn(Option<&FocusHandle>, &mut Window, &mut App)>>,
    action: Option<Box<dyn Action>>,
    disabled: bool,
    documentation_aside: Option<DocumentationAside>,
}

impl ContextMenuEntry {
    pub fn new(label: impl Into<SharedString>) -> Self {
        ContextMenuEntry {
            toggle: None,
            label: label.into(),
            icon: None,
            custom_icon_path: None,
            custom_icon_svg: None,
            icon_position: IconPosition::Start,
            icon_size: IconSize::Small,
            icon_color: None,
            handler: Rc::new(|_, _, _| {}),
            secondary_handler: None,
            action: None,
            disabled: false,
            documentation_aside: None,
        }
    }

    pub const fn toggleable(mut self, toggle_position: IconPosition, toggled: bool) -> Self {
        self.toggle = Some((toggle_position, toggled));
        self
    }

    pub const fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub const fn icon_size(mut self, icon_size: IconSize) -> Self {
        self.icon_size = icon_size;
        self
    }

    pub const fn icon_color(mut self, icon_color: Color) -> Self {
        self.icon_color = Some(icon_color);
        self
    }

    pub const fn toggle(mut self, toggle_position: IconPosition, toggled: bool) -> Self {
        self.toggle = Some((toggle_position, toggled));
        self
    }

    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    pub fn handler(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.handler = Rc::new(move |_, window, cx| handler(window, cx));
        self
    }

    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn documentation_aside(
        mut self,
        side: DocumentationSide,
        render: impl Fn(&mut App) -> AnyElement + 'static,
    ) -> Self {
        self.documentation_aside = Some(DocumentationAside { side, render: Rc::new(render) });

        self
    }
}

impl FluentBuilder for ContextMenuEntry {}

impl From<ContextMenuEntry> for ContextMenuItem {
    fn from(entry: ContextMenuEntry) -> Self {
        ContextMenuItem::Entry(entry)
    }
}

pub struct ContextMenu {
    #[allow(clippy::type_complexity)]
    builder: Option<Rc<dyn Fn(Self, &mut Window, &mut Context<Self>) -> Self>>,
    items: Vec<ContextMenuItem>,
    focus_handle: FocusHandle,
    action_context: Option<FocusHandle>,
    selected_index: Option<usize>,
    clicked: bool,
    key_context: SharedString,
    _on_blur_subscription: Subscription,
    keep_open_on_confirm: bool,
    fixed_width: Option<DefiniteLength>,
    spacing: ListItemSpacing,
    main_menu: Option<Entity<ContextMenu>>,
    main_menu_observed_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    // Docs aide-related fields
    documentation_aside: Option<(usize, DocumentationAside)>,
    aside_trigger_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    // Submenu-related fields
    submenu_state: SubmenuState,
    hover_target: HoverTarget,
    submenu_trigger_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    submenu_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    submenu_depth: usize,
    submenu_trigger_mouse_down: bool,
    ignore_blur_until: Option<Instant>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum DocumentationSide {
    Left,
    Right,
}

#[derive(Clone)]
pub struct DocumentationAside {
    pub side: DocumentationSide,
    pub render: Rc<dyn Fn(&mut App) -> AnyElement>,
}

impl DocumentationAside {
    pub fn new(side: DocumentationSide, render: Rc<dyn Fn(&mut App) -> AnyElement>) -> Self {
        Self { side, render }
    }
}

impl Focusable for ContextMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ContextMenu {}

impl FluentBuilder for ContextMenu {}

impl ContextMenu {
    #[inline(always)]
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        build_menu: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Self {
        let menu = Self::new_inner(window, cx);
        build_menu(menu, window, cx)
    }

    pub fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx, f))
    }

    /// Builds a [`ContextMenu`] that will stay open when making changes instead of closing after each confirmation.
    ///
    /// The main difference from [`ContextMenu::build`] is the type of the `builder`, as we need to be able to hold onto
    /// it to call it again.
    pub fn build_persistent(
        window: &mut Window,
        cx: &mut App,
        builder: impl Fn(Self, &mut Window, &mut Context<Self>) -> Self + 'static,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let builder = Rc::new(builder);

            let focus_handle = cx.focus_handle();
            let _on_blur_subscription = cx.on_blur(&focus_handle, window, |this: &mut ContextMenu, window, cx| {
                if let Some(ignore_until) = this.ignore_blur_until {
                    if Instant::now() < ignore_until {
                        return;
                    }
                    this.ignore_blur_until = None;
                }

                if this.main_menu.is_none()
                    && let SubmenuState::Open(open_submenu) = &this.submenu_state
                {
                    let submenu_focus = open_submenu.entity.read(cx).focus_handle.clone();
                    if submenu_focus.contains_focused(window, cx) {
                        return;
                    }
                }

                this.cancel(&menu::Cancel, window, cx);
            });
            window.refresh();

            Rc::clone(&builder)(
                Self {
                    builder: Some(builder),
                    items: Vec::default(),
                    focus_handle,
                    action_context: None,
                    selected_index: None,
                    clicked: false,
                    key_context: "menu".into(),
                    _on_blur_subscription,
                    keep_open_on_confirm: true,
                    fixed_width: None,
                    spacing: ListItemSpacing::default(),
                    main_menu: None,
                    main_menu_observed_bounds: Rc::new(Cell::new(None)),
                    documentation_aside: None,
                    aside_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
                    submenu_state: SubmenuState::Closed,
                    hover_target: HoverTarget::MainMenu,
                    submenu_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
                    submenu_bounds: Rc::new(Cell::new(None)),
                    submenu_depth: 0,
                    submenu_trigger_mouse_down: false,
                    ignore_blur_until: None,
                },
                window,
                cx,
            )
        })
    }

    /// Rebuilds the menu.
    ///
    /// This is used to refresh the menu entries when entries are toggled when the menu is configured with
    /// `keep_open_on_confirm = true`.
    ///
    /// This only works if the [`ContextMenu`] was constructed using [`ContextMenu::build_persistent`]. Otherwise it is
    /// a no-op.
    pub fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(builder) = self.builder.clone() else {
            return;
        };

        // The way we rebuild the menu is a bit of a hack.
        let focus_handle = cx.focus_handle();
        let new_menu = Rc::clone(&builder)(
            Self {
                builder: Some(builder),
                items: Vec::default(),
                focus_handle: focus_handle.clone(),
                action_context: None,
                selected_index: None,
                clicked: false,
                key_context: "menu".into(),
                _on_blur_subscription: cx.on_blur(&focus_handle, window, |this: &mut ContextMenu, window, cx| {
                    if let Some(ignore_until) = this.ignore_blur_until {
                        if Instant::now() < ignore_until {
                            return;
                        }
                        this.ignore_blur_until = None;
                    }

                    if this.main_menu.is_none()
                        && let SubmenuState::Open(open_submenu) = &this.submenu_state
                    {
                        let submenu_focus = open_submenu.entity.read(cx).focus_handle.clone();
                        if submenu_focus.contains_focused(window, cx) {
                            return;
                        }
                    }

                    this.cancel(&menu::Cancel, window, cx);
                }),
                keep_open_on_confirm: false,
                fixed_width: None,
                spacing: self.spacing,
                main_menu: None,
                main_menu_observed_bounds: Rc::new(Cell::new(None)),
                documentation_aside: None,
                aside_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
                submenu_state: SubmenuState::Closed,
                hover_target: HoverTarget::MainMenu,
                submenu_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
                submenu_bounds: Rc::new(Cell::new(None)),
                submenu_depth: 0,
                submenu_trigger_mouse_down: false,
                ignore_blur_until: None,
            },
            window,
            cx,
        );

        self.items = new_menu.items;
        self.spacing = new_menu.spacing;
        self.submenu_trigger_bounds.borrow_mut().clear();

        cx.notify();
    }

    pub fn context(mut self, focus: FocusHandle) -> Self {
        self.action_context = Some(focus);
        self
    }

    pub fn header(mut self, title: impl Into<SharedString>) -> Self {
        self.items.push(ContextMenuItem::Header(title.into()));
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(ContextMenuItem::Separator);
        self
    }

    pub fn extend<I: Into<ContextMenuItem>>(mut self, items: impl IntoIterator<Item = I>) -> Self {
        self.items.extend(items.into_iter().map(Into::into));
        self
    }

    pub fn item(mut self, item: impl Into<ContextMenuItem>) -> Self {
        self.items.push(item.into());
        self
    }

    pub fn push_item(&mut self, item: impl Into<ContextMenuItem>) {
        self.items.push(item.into());
    }

    pub fn entry(
        mut self,
        label: impl Into<SharedString>,
        action: Option<Box<dyn Action>>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            toggle: None,
            label: label.into(),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            secondary_handler: None,
            icon: None,
            custom_icon_path: None,
            custom_icon_svg: None,
            icon_position: IconPosition::End,
            icon_size: IconSize::Small,
            icon_color: None,
            action,
            disabled: false,
            documentation_aside: None,
        }));
        self
    }

    pub fn toggleable_entry(
        self,
        label: impl Into<SharedString>,
        toggled: bool,
        position: IconPosition,
        action: Option<Box<dyn Action>>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.toggleable_entry_disabled_when(label, toggled, false, position, action, handler)
    }

    /// Like [`Self::toggleable_entry`], but the entry is rendered disabled (and its handler is not
    /// invoked) when `disabled` is `true`.
    pub fn toggleable_entry_disabled_when(
        mut self,
        label: impl Into<SharedString>,
        toggled: bool,
        disabled: bool,
        position: IconPosition,
        action: Option<Box<dyn Action>>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            toggle: Some((position, toggled)),
            label: label.into(),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            secondary_handler: None,
            icon: None,
            custom_icon_path: None,
            custom_icon_svg: None,
            icon_position: position,
            icon_size: IconSize::Small,
            icon_color: None,
            action,
            disabled,
            documentation_aside: None,
        }));
        self
    }

    pub fn custom_row(mut self, entry_render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        self.items.push(ContextMenuItem::CustomEntry {
            entry_render: Box::new(entry_render),
            handler: Rc::new(|_, _, _| {}),
            selectable: false,
        });
        self
    }

    pub fn custom_entry(
        mut self,
        entry_render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::CustomEntry {
            entry_render: Box::new(entry_render),
            handler: Rc::new(move |_, window, cx| handler(window, cx)),
            selectable: true,
        });
        self
    }

    pub fn selectable(mut self, selectable: bool) -> Self {
        if let Some(ContextMenuItem::CustomEntry { selectable: entry_selectable, .. }) = self.items.last_mut() {
            *entry_selectable = selectable;
        }
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(ContextMenuItem::Label(label.into()));
        self
    }

    pub fn action(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.action_checked(label, action, false)
    }

    pub fn action_checked(self, label: impl Into<SharedString>, action: Box<dyn Action>, checked: bool) -> Self {
        self.action_checked_with_disabled(label, action, checked, false)
    }

    pub fn action_checked_with_disabled(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        checked: bool,
        disabled: bool,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            toggle: checked.then_some((IconPosition::Start, true)),
            label: label.into(),
            action: Some(action.boxed_clone()),
            handler: Rc::new(move |context, window, cx| {
                if let Some(context) = &context {
                    window.focus(context, cx);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            }),
            secondary_handler: None,
            icon: None,
            custom_icon_path: None,
            custom_icon_svg: None,
            icon_position: IconPosition::End,
            icon_size: IconSize::Small,
            icon_color: None,
            disabled,
            documentation_aside: None,
        }));
        self
    }

    pub fn action_disabled_when(
        mut self,
        disabled: bool,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            toggle: None,
            label: label.into(),
            action: Some(action.boxed_clone()),
            handler: Rc::new(move |context, window, cx| {
                if let Some(context) = &context {
                    window.focus(context, cx);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            }),
            secondary_handler: None,
            icon: None,
            custom_icon_path: None,
            custom_icon_svg: None,
            icon_size: IconSize::Small,
            icon_position: IconPosition::End,
            icon_color: None,
            disabled,
            documentation_aside: None,
        }));
        self
    }

    pub fn link(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.link_with_handler(label, action, |_, _| {})
    }

    pub fn link_with_handler(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Entry(ContextMenuEntry {
            toggle: None,
            label: label.into(),
            action: Some(action.boxed_clone()),
            handler: Rc::new(move |_, window, cx| {
                handler(window, cx);
                window.dispatch_action(action.boxed_clone(), cx);
            }),
            secondary_handler: None,
            icon: Some(IconName::ArrowUpRight),
            custom_icon_path: None,
            custom_icon_svg: None,
            icon_size: IconSize::XSmall,
            icon_position: IconPosition::End,
            icon_color: None,
            disabled: false,
            documentation_aside: None,
        }));
        self
    }

    pub fn submenu(
        mut self,
        label: impl Into<SharedString>,
        builder: impl Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Submenu {
            label: label.into(),
            icon: None,
            icon_color: None,
            builder: Rc::new(builder),
        });
        self
    }

    pub fn submenu_with_colored_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        icon_color: Color,
        builder: impl Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem::Submenu {
            label: label.into(),
            icon: Some(icon),
            icon_color: Some(icon_color),
            builder: Rc::new(builder),
        });
        self
    }

    pub const fn keep_open_on_confirm(mut self, keep_open: bool) -> Self {
        self.keep_open_on_confirm = keep_open;
        self
    }

    pub const fn fixed_width(mut self, width: DefiniteLength) -> Self {
        self.fixed_width = Some(width);
        self
    }

    pub const fn spacing(mut self, spacing: ListItemSpacing) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = context.into();
        self
    }

    pub const fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index else {
            return cx.emit(DismissEvent);
        };

        if let Some(ContextMenuItem::Submenu { builder, .. }) = self.items.get(ix) {
            self.open_submenu(ix, Rc::clone(builder), SubmenuOpenTrigger::Keyboard, window, cx);

            if let SubmenuState::Open(open_submenu) = &self.submenu_state {
                let focus_handle = open_submenu.entity.read(cx).focus_handle.clone();
                window.focus(&focus_handle, cx);
                open_submenu.entity.update(cx, |submenu, cx| {
                    submenu.select_first(&SelectFirst, window, cx);
                });
            }

            cx.notify();
            return;
        }

        let context = self.action_context.as_ref();

        if let Some(
            ContextMenuItem::Entry(ContextMenuEntry { handler, disabled: false, .. })
            | ContextMenuItem::CustomEntry { handler, .. },
        ) = self.items.get(ix)
        {
            (handler)(context, window, cx);
        }

        if self.main_menu.is_some() && !self.keep_open_on_confirm {
            self.clicked = true;
        }

        if self.keep_open_on_confirm {
            self.rebuild(window, cx);
        } else {
            cx.emit(DismissEvent);
        }
    }

    pub fn secondary_confirm(&mut self, _: &menu::SecondaryConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index else {
            return;
        };

        if let Some(ContextMenuItem::Submenu { builder, .. }) = self.items.get(ix) {
            self.open_submenu(ix, Rc::clone(builder), SubmenuOpenTrigger::Keyboard, window, cx);

            if let SubmenuState::Open(open_submenu) = &self.submenu_state {
                let focus_handle = open_submenu.entity.read(cx).focus_handle.clone();
                window.focus(&focus_handle, cx);
                open_submenu.entity.update(cx, |submenu, cx| {
                    submenu.select_first(&SelectFirst, window, cx);
                });
            }

            cx.notify();
            return;
        }

        let context = self.action_context.as_ref();

        if let Some(ContextMenuItem::Entry(ContextMenuEntry { handler, secondary_handler, disabled: false, .. })) =
            self.items.get(ix)
        {
            if let Some(secondary) = secondary_handler {
                (secondary)(context, window, cx);
            } else {
                (handler)(context, window, cx);
            }
        } else if let Some(ContextMenuItem::CustomEntry { handler, .. }) = self.items.get(ix) {
            (handler)(context, window, cx);
        }

        if self.main_menu.is_some() && !self.keep_open_on_confirm {
            self.clicked = true;
        }

        if self.keep_open_on_confirm {
            self.rebuild(window, cx);
        } else {
            cx.emit(DismissEvent);
        }
    }

    pub fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.main_menu.is_some() {
            cx.emit(DismissEvent);

            // Restore keyboard focus to the parent menu so arrow keys / Escape / Enter work again.
            if let Some(parent) = &self.main_menu {
                let parent_focus = parent.read(cx).focus_handle.clone();

                parent.update(cx, |parent, _cx| {
                    parent.ignore_blur_until = Some(Instant::now() + Duration::from_millis(200));
                });

                window.focus(&parent_focus, cx);
            }

            return;
        }

        cx.emit(DismissEvent);
    }

    pub const fn clear_selected(&mut self) {
        self.selected_index = None;
    }

    pub fn select_first(&mut self, _: &SelectFirst, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.items.iter().position(|item| item.is_selectable()) {
            self.select_index(ix, window, cx);
        }
        cx.notify();
    }

    pub fn select_last(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        for (ix, item) in self.items.iter().enumerate().rev() {
            if item.is_selectable() {
                return self.select_index(ix, window, cx);
            }
        }
        None
    }

    fn handle_select_last(&mut self, _: &SelectLast, window: &mut Window, cx: &mut Context<Self>) {
        if self.select_last(window, cx).is_some() {
            cx.notify();
        }
    }

    pub fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected_index {
            let next_index = ix + 1;
            if self.items.len() <= next_index {
                self.select_first(&SelectFirst, window, cx);
                return;
            }
            for (ix, item) in self.items.iter().enumerate().skip(next_index) {
                if item.is_selectable() {
                    self.select_index(ix, window, cx);
                    cx.notify();
                    return;
                }
            }
        }
        self.select_first(&SelectFirst, window, cx);
    }

    pub fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected_index {
            for (ix, item) in self.items.iter().enumerate().take(ix).rev() {
                if item.is_selectable() {
                    self.select_index(ix, window, cx);
                    cx.notify();
                    return;
                }
            }
        }
        self.handle_select_last(&SelectLast, window, cx);
    }

    pub fn select_submenu_child(&mut self, _: &SelectChild, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index else {
            return;
        };

        let Some(ContextMenuItem::Submenu { builder, .. }) = self.items.get(ix) else {
            return;
        };

        self.open_submenu(ix, Rc::clone(builder), SubmenuOpenTrigger::Keyboard, window, cx);

        if let SubmenuState::Open(open_submenu) = &self.submenu_state {
            let focus_handle = open_submenu.entity.read(cx).focus_handle.clone();
            window.focus(&focus_handle, cx);
            open_submenu.entity.update(cx, |submenu, cx| {
                submenu.select_first(&SelectFirst, window, cx);
            });
        }

        cx.notify();
    }

    pub fn select_submenu_parent(&mut self, _: &SelectParent, window: &mut Window, cx: &mut Context<Self>) {
        if self.main_menu.is_none() {
            return;
        }

        if let Some(parent) = &self.main_menu {
            let parent_clone = parent.clone();

            let parent_focus = parent.read(cx).focus_handle.clone();
            window.focus(&parent_focus, cx);

            cx.emit(DismissEvent);

            parent_clone.update(cx, |parent, cx| {
                if let SubmenuState::Open(open_submenu) = &parent.submenu_state {
                    let trigger_index = open_submenu.item_index;
                    parent.close_submenu(false, cx);
                    parent.select_index(trigger_index, window, cx);
                    cx.notify();
                }
            });

            return;
        }

        cx.emit(DismissEvent);
    }

    fn select_index(&mut self, ix: usize, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        self.documentation_aside = None;
        let item = self.items.get(ix)?;
        if item.is_selectable() {
            self.selected_index = Some(ix);
            match item {
                ContextMenuItem::Entry(entry) => {
                    if let Some(callback) = &entry.documentation_aside {
                        self.documentation_aside = Some((ix, callback.clone()));
                    }
                }
                ContextMenuItem::Submenu { .. } => {}
                _ => (),
            }
        }
        Some(ix)
    }

    #[allow(clippy::type_complexity)]
    fn create_submenu(
        builder: Rc<dyn Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu>,
        parent_entity: Entity<ContextMenu>,
        parent_depth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<ContextMenu>, Subscription) {
        let submenu = Self::build_submenu(builder, parent_entity, parent_depth, window, cx);

        let dismiss_subscription = cx.subscribe(&submenu, |this, submenu, _: &DismissEvent, cx| {
            let should_dismiss_parent = submenu.read(cx).clicked;

            this.close_submenu(false, cx);

            if should_dismiss_parent {
                cx.emit(DismissEvent);
            }
        });

        (submenu, dismiss_subscription)
    }

    #[allow(clippy::type_complexity)]
    fn build_submenu(
        builder: Rc<dyn Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu>,
        parent_entity: Entity<ContextMenu>,
        parent_depth: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        cx.new(|cx| {
            let focus_handle = cx.focus_handle();

            let _on_blur_subscription = cx.on_blur(&focus_handle, window, |_this: &mut ContextMenu, _window, _cx| {});

            let mut menu = ContextMenu {
                builder: None,
                items: Vec::default(),
                focus_handle,
                action_context: None,
                selected_index: None,
                clicked: false,
                key_context: "menu".into(),
                _on_blur_subscription,
                keep_open_on_confirm: false,
                fixed_width: None,
                spacing: ListItemSpacing::default(),
                documentation_aside: None,
                aside_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
                main_menu: Some(parent_entity),
                main_menu_observed_bounds: Rc::new(Cell::new(None)),
                submenu_state: SubmenuState::Closed,
                hover_target: HoverTarget::MainMenu,
                submenu_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
                submenu_bounds: Rc::new(Cell::new(None)),
                submenu_depth: parent_depth + 1,
                submenu_trigger_mouse_down: false,
                ignore_blur_until: None,
            };

            menu = (builder)(menu, window, cx);
            menu
        })
    }

    fn close_submenu(&mut self, clear_selection: bool, cx: &mut Context<Self>) {
        self.submenu_state = SubmenuState::Closed;
        self.hover_target = HoverTarget::MainMenu;
        self.submenu_bounds.set(None);

        if clear_selection {
            self.selected_index = None;
        }

        cx.notify();
    }

    #[allow(clippy::type_complexity)]
    fn open_submenu(
        &mut self,
        item_index: usize,
        builder: Rc<dyn Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu>,
        reason: SubmenuOpenTrigger,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If the submenu is already open for this item, don't recreate it.
        if matches!(
            &self.submenu_state,
            SubmenuState::Open(open_submenu) if open_submenu.item_index == item_index
        ) {
            return;
        }

        let (submenu, dismiss_subscription) =
            Self::create_submenu(builder, cx.entity(), self.submenu_depth, window, cx);

        self.submenu_bounds.set(None);
        self.hover_target = HoverTarget::MainMenu;

        // When opening a submenu via keyboard, there is a brief moment where focus/hover can
        // transition in a way that triggers the parent menu's `on_blur` dismissal.
        if matches!(reason, SubmenuOpenTrigger::Keyboard) {
            self.ignore_blur_until = Some(Instant::now() + Duration::from_millis(150));
        }

        self.submenu_state = SubmenuState::Open(OpenSubmenu {
            item_index,
            entity: submenu,
            _dismiss_subscription: dismiss_subscription,
        });

        cx.notify();
    }

    /// The first enabled entry that runs `action`.
    pub fn on_blur_subscription(mut self, new_subscription: Subscription) -> Self {
        self._on_blur_subscription = new_subscription;
        self
    }

    fn render_menu_item(
        &self,
        ix: usize,
        item: &ContextMenuItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        match item {
            ContextMenuItem::Separator => ListSeparator.into_any_element(),
            ContextMenuItem::Header(header) => ListSubHeader::new(header.clone()).inset(true).into_any_element(),
            ContextMenuItem::Label(label) => ListItem::new(ix)
                .inset(true)
                .spacing(self.spacing)
                .disabled(true)
                .child(Label::new(label.clone()))
                .into_any_element(),
            ContextMenuItem::Entry(entry) => self.render_menu_entry(ix, entry, cx).into_any_element(),
            ContextMenuItem::CustomEntry { entry_render, handler, selectable } => {
                let handler = Rc::clone(handler);
                let menu = cx.entity().downgrade();
                let selectable = *selectable;

                div()
                    .id(("context-menu-child", ix))
                    .child(
                        ListItem::new(ix)
                            .inset(true)
                            .spacing(self.spacing)
                            .toggle_state(Some(ix) == self.selected_index)
                            .selectable(selectable)
                            .when(selectable, |item| {
                                item.on_click({
                                    let context = self.action_context.clone();
                                    let keep_open_on_confirm = self.keep_open_on_confirm;
                                    move |_, window, cx| {
                                        handler(context.as_ref(), window, cx);
                                        menu.update(cx, |menu, cx| {
                                            menu.clicked = true;

                                            if keep_open_on_confirm {
                                                menu.rebuild(window, cx);
                                            } else {
                                                cx.emit(DismissEvent);
                                            }
                                        })
                                        .ok();
                                    }
                                })
                            })
                            .child(entry_render(window, cx)),
                    )
                    .into_any_element()
            }
            ContextMenuItem::Submenu { label, icon, icon_color, .. } => {
                self.render_submenu_item_trigger(ix, label.clone(), *icon, *icon_color, window, cx).into_any_element()
            }
        }
    }

    fn render_submenu_item_trigger(
        &self,
        ix: usize,
        label: SharedString,
        icon: Option<IconName>,
        icon_color: Option<Color>,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let toggle_state = Some(ix) == self.selected_index
            || matches!(
                &self.submenu_state,
                SubmenuState::Open(open_submenu) if open_submenu.item_index == ix
            );
        let submenu = match &self.submenu_state {
            SubmenuState::Open(open_submenu) if open_submenu.item_index == ix => self
                .submenu_trigger_bounds
                .borrow()
                .get(&ix)
                .copied()
                .map(|bounds| (open_submenu.entity.clone(), bounds)),
            _ => None,
        };

        div()
            .id(("context-menu-submenu-trigger", ix))
            .relative()
            .capture_any_mouse_down(cx.listener(move |this, event: &MouseDownEvent, _, _| {
                // This prevents on_hover(false) from closing the submenu during a click.
                if event.button == MouseButton::Left {
                    this.submenu_trigger_mouse_down = true;
                }
            }))
            .capture_any_mouse_up(cx.listener(move |this, event: &MouseUpEvent, _, _| {
                if event.button == MouseButton::Left {
                    this.submenu_trigger_mouse_down = false;
                }
            }))
            .child(
                canvas(
                    {
                        let trigger_bounds = Rc::clone(&self.submenu_trigger_bounds);
                        move |bounds, _window, _cx| {
                            trigger_bounds.borrow_mut().insert(ix, bounds);
                        }
                    },
                    |_bounds, _state, _window, _cx| {},
                )
                .size_full()
                .absolute()
                .top_0()
                .left_0(),
            )
            .child(
                ListItem::new(ix)
                    .inset(true)
                    .spacing(self.spacing)
                    .toggle_state(toggle_state)
                    .on_hover(cx.listener(move |this, hovered, window, cx| {
                        if *hovered {
                            this.clear_selected();
                            window.focus(&this.focus_handle, cx);
                            this.hover_target = HoverTarget::MainMenu;

                            if let Some(ContextMenuItem::Submenu { builder, .. }) = this.items.get(ix) {
                                this.open_submenu(ix, Rc::clone(builder), SubmenuOpenTrigger::Pointer, window, cx);
                            }

                            cx.notify();
                        } else {
                            if this.submenu_trigger_mouse_down {
                                return;
                            }

                            let is_open_for_this_item = matches!(
                                &this.submenu_state,
                                SubmenuState::Open(open_submenu) if open_submenu.item_index == ix
                            );

                            let mouse_in_submenu_zone = this
                                .padded_submenu_bounds()
                                .is_some_and(|bounds| bounds.contains(&window.mouse_position()));

                            if is_open_for_this_item
                                && this.hover_target != HoverTarget::Submenu
                                && !mouse_in_submenu_zone
                            {
                                this.close_submenu(false, cx);
                                this.clear_selected();
                                window.focus(&this.focus_handle, cx);
                                cx.notify();
                            }
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if matches!(
                            &this.submenu_state,
                            SubmenuState::Open(open_submenu) if open_submenu.item_index == ix
                        ) {
                            return;
                        }

                        if let Some(ContextMenuItem::Submenu { builder, .. }) = this.items.get(ix) {
                            this.open_submenu(ix, Rc::clone(builder), SubmenuOpenTrigger::Pointer, window, cx);
                        }
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .when_some(icon, |this, icon_name| {
                                        this.child(
                                            Icon::new(icon_name)
                                                .size(IconSize::Small)
                                                .color(icon_color.unwrap_or(Color::Muted)),
                                        )
                                    })
                                    .child(Label::new(label).color(Color::Default)),
                            )
                            .child(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(Color::Muted)),
                    ),
            )
            .when_some(submenu, |this, (submenu, trigger_bounds)| {
                this.child(self.render_submenu_container(ix, submenu, trigger_bounds, window, cx))
            })
    }

    fn padded_submenu_bounds(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.submenu_bounds.get()?;
        Some(Bounds {
            origin: Point { x: bounds.origin.x - px(50.0), y: bounds.origin.y - px(50.0) },
            size: Size { width: bounds.size.width + px(100.0), height: bounds.size.height + px(100.0) },
        })
    }

    fn render_submenu_container(
        &self,
        ix: usize,
        submenu: Entity<ContextMenu>,
        trigger_bounds: Bounds<Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let space_on_right = window.viewport_size().width - trigger_bounds.right();
        let space_on_left = trigger_bounds.left();
        let flip_left = space_on_right < CONTEXT_MENU_MIN_WIDTH && space_on_left > space_on_right;
        let anchor_position = point(if flip_left { px(2.) } else { trigger_bounds.size.width - px(2.) }, px(0.));
        let bounds_cell = Rc::clone(&self.submenu_bounds);
        let canvas = canvas(
            {
                move |bounds, _window, _cx| {
                    bounds_cell.set(Some(bounds));
                }
            },
            |_bounds, _state, _window, _cx| {},
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0();

        // Anchored uses its own layout origin; this zero-sized child pins that
        // origin to the trigger's top-left instead of below the row's contents.
        div().absolute().top_0().left_0().w(px(0.)).h(px(0.)).child(
            deferred(
                anchored()
                    .position(anchor_position)
                    .position_mode(AnchoredPositionMode::Local)
                    .anchor(if flip_left { Anchor::TopRight } else { Anchor::TopLeft })
                    .snap_to_window_with_margin(px(8.0))
                    .child(
                        div()
                            .id(("submenu-container", ix))
                            .on_hover(cx.listener(|this, hovered, _, _| {
                                if *hovered {
                                    this.hover_target = HoverTarget::Submenu;
                                }
                            }))
                            .occlude()
                            .child(canvas)
                            .child(submenu),
                    ),
            )
            .with_priority(self.submenu_depth + 2),
        )
    }

    fn render_menu_entry(&self, ix: usize, entry: &ContextMenuEntry, cx: &Context<Self>) -> impl IntoElement {
        let ContextMenuEntry {
            toggle,
            label,
            handler,
            icon,
            custom_icon_path,
            custom_icon_svg,
            icon_position,
            icon_size,
            icon_color,
            action,
            disabled,
            documentation_aside,
            secondary_handler: _,
        } = entry;
        let handler = Rc::clone(handler);
        let menu = cx.entity().downgrade();

        let icon_color = if *disabled {
            Color::Muted
        } else if toggle.is_some() {
            icon_color.unwrap_or(Color::Accent)
        } else {
            icon_color.unwrap_or(Color::Default)
        };

        let label_color = if *disabled { Color::Disabled } else { Color::Default };

        let label_element = if let Some(custom_path) = custom_icon_path {
            h_flex()
                .gap_1p5()
                .when(*icon_position == IconPosition::Start && toggle.is_none(), |flex| {
                    flex.child(Icon::from_path(custom_path.clone()).size(*icon_size).color(icon_color))
                })
                .child(Label::new(label.clone()).color(label_color).truncate())
                .when(*icon_position == IconPosition::End, |flex| {
                    flex.child(Icon::from_path(custom_path.clone()).size(*icon_size).color(icon_color))
                })
                .into_any_element()
        } else if let Some(custom_icon_svg) = custom_icon_svg {
            h_flex()
                .gap_1p5()
                .when(*icon_position == IconPosition::Start && toggle.is_none(), |flex| {
                    flex.child(Icon::from_external_svg(custom_icon_svg.clone()).size(*icon_size).color(icon_color))
                })
                .child(Label::new(label.clone()).color(label_color).truncate())
                .when(*icon_position == IconPosition::End, |flex| {
                    flex.child(Icon::from_external_svg(custom_icon_svg.clone()).size(*icon_size).color(icon_color))
                })
                .into_any_element()
        } else if let Some(icon_name) = icon {
            h_flex()
                .gap_1p5()
                .when(*icon_position == IconPosition::Start && toggle.is_none(), |flex| {
                    flex.child(Icon::new(*icon_name).size(*icon_size).color(icon_color))
                })
                .child(Label::new(label.clone()).color(label_color).truncate())
                .when(*icon_position == IconPosition::End, |flex| {
                    flex.child(Icon::new(*icon_name).size(*icon_size).color(icon_color))
                })
                .into_any_element()
        } else {
            Label::new(label.clone()).color(label_color).truncate().into_any_element()
        };

        let aside_trigger_bounds = Rc::clone(&self.aside_trigger_bounds);

        div()
            .id(("context-menu-child", ix))
            .when_some(documentation_aside.clone(), |this, documentation_aside| {
                this.occlude().on_hover(cx.listener(move |menu, hovered, _, cx| {
                    if *hovered {
                        menu.documentation_aside = Some((ix, documentation_aside.clone()));
                    } else if matches!(menu.documentation_aside, Some((id, _)) if id == ix) {
                        menu.documentation_aside = None;
                    }
                    cx.notify();
                }))
            })
            .when(documentation_aside.is_some(), |this| {
                this.child(
                    canvas(
                        {
                            let aside_trigger_bounds = Rc::clone(&aside_trigger_bounds);
                            move |bounds, _window, _cx| {
                                aside_trigger_bounds.borrow_mut().insert(ix, bounds);
                            }
                        },
                        |_bounds, _state, _window, _cx| {},
                    )
                    .size_full()
                    .absolute()
                    .top_0()
                    .left_0(),
                )
            })
            .child(
                ListItem::new(ix)
                    .group_name("label_container")
                    .inset(true)
                    .spacing(self.spacing)
                    .disabled(*disabled)
                    .toggle_state(Some(ix) == self.selected_index)
                    .when(self.main_menu.is_none() && !*disabled, |item| {
                        item.on_hover(cx.listener(move |this, hovered, window, cx| {
                            if *hovered {
                                this.clear_selected();
                                window.focus(&this.focus_handle, cx);

                                if let SubmenuState::Open(open_submenu) = &this.submenu_state
                                    && open_submenu.item_index != ix
                                {
                                    this.close_submenu(false, cx);
                                }
                            }
                        }))
                    })
                    .when(self.main_menu.is_some(), |item| {
                        item.on_click(cx.listener(move |this, _, window, cx| {
                            if matches!(
                                &this.submenu_state,
                                SubmenuState::Open(open_submenu) if open_submenu.item_index == ix
                            ) {
                                return;
                            }

                            if let Some(ContextMenuItem::Submenu { builder, .. }) = this.items.get(ix) {
                                this.open_submenu(ix, Rc::clone(builder), SubmenuOpenTrigger::Pointer, window, cx);
                            }
                        }))
                        .on_hover(cx.listener(move |this, hovered, window, cx| {
                            if *hovered {
                                this.clear_selected();
                                cx.notify();
                            }

                            if let Some(parent) = &this.main_menu {
                                let mouse_pos = window.mouse_position();
                                let parent_clone = parent.clone();

                                if *hovered {
                                    parent.update(cx, |parent, _| {
                                        parent.clear_selected();
                                        parent.hover_target = HoverTarget::Submenu;
                                    });
                                } else {
                                    parent_clone.update(cx, |parent, cx| {
                                        if matches!(&parent.submenu_state, SubmenuState::Open(_))
                                            && !parent
                                                .padded_submenu_bounds()
                                                .is_some_and(|bounds| bounds.contains(&mouse_pos))
                                        {
                                            parent.close_submenu(true, cx);
                                        }
                                    });
                                }
                            }
                        }))
                    })
                    .when_some(*toggle, |list_item, (position, toggled)| {
                        let contents = div()
                            .flex_none()
                            .child(Icon::new(icon.unwrap_or(IconName::Check)).color(icon_color).size(*icon_size))
                            .when(!toggled, |contents| contents.invisible());

                        match position {
                            IconPosition::Start => list_item.start_slot(contents),
                            IconPosition::End => list_item.end_slot(contents),
                        }
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(label_element)
                            .children(action.as_ref().map(|action| {
                                let binding = self.action_context.as_ref().map_or_else(
                                    || KeyBinding::for_action(&**action, cx),
                                    |focus| KeyBinding::for_action_in(&**action, focus, cx),
                                );

                                div()
                                    .ml_4()
                                    .child(binding.disabled(*disabled))
                                    .when(*disabled && documentation_aside.is_some(), |parent| parent.invisible())
                            }))
                            .when(*disabled && documentation_aside.is_some(), |parent| {
                                parent.child(Icon::new(IconName::Info).size(IconSize::XSmall).color(Color::Muted))
                            }),
                    )
                    .on_click({
                        let context = self.action_context.clone();
                        let keep_open_on_confirm = self.keep_open_on_confirm;
                        move |_, window, cx| {
                            handler(context.as_ref(), window, cx);
                            menu.update(cx, |menu, cx| {
                                menu.clicked = true;
                                if keep_open_on_confirm {
                                    menu.rebuild(window, cx);
                                } else {
                                    cx.emit(DismissEvent);
                                }
                            })
                            .ok();
                        }
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn new_inner(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let _on_blur_subscription = cx.on_blur(&focus_handle, window, |context_menu: &mut ContextMenu, window, cx| {
            if let Some(ignore_until) = context_menu.ignore_blur_until {
                if Instant::now() < ignore_until {
                    return;
                }
                context_menu.ignore_blur_until = None;
            }

            if context_menu.main_menu.is_none()
                && let SubmenuState::Open(open_submenu) = &context_menu.submenu_state
            {
                let submenu_focus = open_submenu.entity.read(cx).focus_handle.clone();
                if submenu_focus.contains_focused(window, cx) {
                    return;
                }
            }

            context_menu.cancel(&menu::Cancel, window, cx);
        });
        window.refresh();

        Self {
            builder: None,
            items: Vec::new(),
            focus_handle,
            action_context: None,
            selected_index: None,
            clicked: false,
            key_context: SharedString::from("menu"),
            _on_blur_subscription,
            keep_open_on_confirm: false,
            fixed_width: None,
            spacing: ListItemSpacing::default(),
            main_menu: None,
            main_menu_observed_bounds: Rc::new(Cell::new(None)),
            documentation_aside: None,
            aside_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
            submenu_state: SubmenuState::Closed,
            hover_target: HoverTarget::MainMenu,
            submenu_trigger_bounds: Rc::new(RefCell::new(HashMap::default())),
            submenu_bounds: Rc::new(Cell::new(None)),
            submenu_depth: 0,
            submenu_trigger_mouse_down: false,
            ignore_blur_until: None,
        }
    }
}

static CONTEXT_MENU_ACTIONS: std::sync::LazyLock<gpui::ActionTable<ContextMenu>> = std::sync::LazyLock::new(|| {
    let mut t = gpui::ActionTable::<ContextMenu>::default();
    t.add(ContextMenu::select_first);
    t.add(ContextMenu::handle_select_last);
    t.add(ContextMenu::select_next);
    t.add(ContextMenu::select_previous);
    t.add(ContextMenu::select_submenu_child);
    t.add(ContextMenu::select_submenu_parent);
    t.add(ContextMenu::confirm);
    t.add(ContextMenu::secondary_confirm);
    t.add(ContextMenu::cancel);
    t
});

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font_size = font::ui_font_size(cx);
        let ui_font_family = font::ui_font().family.clone();
        // Menus can be deferred from inside elements that override the text
        // style (e.g. the editor with a custom `buffer_line_height`), so always
        // apply the default line height to render the same everywhere.
        let line_height = relative(BUFFER_LINE_HEIGHT);
        let window_size = window.viewport_size();
        let rem_size = window.rem_size();
        let is_wide_window = window_size.width / rem_size > rems_from_px(800_f32).0;

        let focus_submenu = match &self.submenu_state {
            SubmenuState::Open(open_submenu)
                if self.submenu_trigger_bounds.borrow().contains_key(&open_submenu.item_index) =>
            {
                Some(open_submenu.entity.read(cx).focus_handle.clone())
            }
            _ => None,
        };

        let aside = self.documentation_aside.clone();
        let render_aside = |aside: DocumentationAside, cx: &mut Context<Self>| {
            WithRemSize::new(ui_font_size)
                .occlude()
                .font_family(ui_font_family.clone())
                .line_height(line_height)
                .elevation_2(cx)
                .w_full()
                .p_2()
                .overflow_hidden()
                .when(is_wide_window, |this| this.max_w_96())
                .when(!is_wide_window, |this| this.max_w_48())
                .child((aside.render)(cx))
        };

        let render_menu = |cx: &mut Context<Self>, window: &mut Window| {
            let bounds_cell = Rc::clone(&self.main_menu_observed_bounds);
            let menu_bounds_measure = canvas(
                {
                    move |bounds, _window, _cx| {
                        bounds_cell.set(Some(bounds));
                    }
                },
                |_bounds, _state, _window, _cx| {},
            )
            .size_full()
            .absolute()
            .top_0()
            .left_0();

            WithRemSize::new(ui_font_size)
                .occlude()
                .font_family(ui_font_family.clone())
                .line_height(line_height)
                .elevation_2(cx)
                .flex()
                .flex_row()
                .flex_shrink_0()
                .child(
                    v_flex()
                        .id("context-menu")
                        .max_h(vh(0.75, window))
                        .flex_shrink_0()
                        .child(menu_bounds_measure)
                        .when_some(self.fixed_width, |this, width| this.w(width).overflow_x_hidden())
                        .when(self.fixed_width.is_none(), |this| this.min_w(CONTEXT_MENU_MIN_WIDTH).flex_1())
                        .overflow_y_scroll()
                        .track_focus(&self.focus_handle(cx))
                        .key_context(self.key_context.as_ref())
                        .action_table(&cx.entity(), &CONTEXT_MENU_ACTIONS)
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.hover_target = HoverTarget::MainMenu;
                                if let Some(parent) = &this.main_menu {
                                    parent.update(cx, |parent, _| {
                                        parent.hover_target = HoverTarget::Submenu;
                                    });
                                }
                            }
                        }))
                        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if matches!(&this.submenu_state, SubmenuState::Open(_))
                                && let Some(padded_bounds) = this.padded_submenu_bounds()
                                && padded_bounds.contains(&event.position)
                            {
                                return;
                            }

                            if let Some(parent) = &this.main_menu {
                                let parent = parent.read(cx);
                                let overridden_by_parent_trigger = match &parent.submenu_state {
                                    SubmenuState::Open(open_submenu) => parent
                                        .submenu_trigger_bounds
                                        .borrow()
                                        .get(&open_submenu.item_index)
                                        .is_some_and(|bounds| bounds.contains(&event.position)),
                                    SubmenuState::Closed => false,
                                };
                                if overridden_by_parent_trigger {
                                    return;
                                }
                            }

                            this.cancel(&menu::Cancel, window, cx);
                        }))
                        .child(List::new().children(
                            self.items.iter().enumerate().map(|(ix, item)| self.render_menu_item(ix, item, window, cx)),
                        )),
                )
        };

        if let Some(focus_handle) = focus_submenu.as_ref() {
            window.focus(focus_handle, cx);
        }

        if is_wide_window {
            let menu_bounds = self.main_menu_observed_bounds.get();
            let trigger_bounds = self
                .documentation_aside
                .as_ref()
                .and_then(|(ix, _)| self.aside_trigger_bounds.borrow().get(ix).copied());

            let trigger_position = match (menu_bounds, trigger_bounds) {
                (Some(menu_bounds), Some(trigger_bounds)) => {
                    let relative_top = trigger_bounds.origin.y - menu_bounds.origin.y;
                    let height = trigger_bounds.size.height;
                    Some((relative_top, height))
                }
                _ => None,
            };

            div()
                .relative()
                .child(render_menu(cx, window))
                // Only render the aside once we have trigger bounds to avoid flicker.
                .when_some(trigger_position, |this, (top, height)| {
                    this.children(aside.map(|(_, aside)| {
                        h_flex()
                            .absolute()
                            .when(aside.side == DocumentationSide::Left, |el| el.right_full().mr_1())
                            .when(aside.side == DocumentationSide::Right, |el| el.left_full().ml_1())
                            .top(top)
                            .h(height)
                            .child(render_aside(aside, cx))
                    }))
                })
        } else {
            v_flex()
                .w_full()
                .relative()
                .gap_1()
                .justify_end()
                .children(aside.map(|(_, aside)| render_aside(aside, cx)))
                .child(render_menu(cx, window))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;

    #[test]
    fn actions_handle_navigation_once() {
        let types = CONTEXT_MENU_ACTIONS.action_types();

        assert_eq!(types.len(), 9);
        assert!(types.is_sorted_by(|a, b| a < b), "action handled twice");
        assert!(types.contains(&TypeId::of::<menu::Confirm>()));
        assert!(CONTEXT_MENU_ACTIONS.conditional().all(|conditional| !conditional));
    }
}
