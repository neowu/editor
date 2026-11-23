use gpui::AnyView;
use gpui::App;
use gpui::Context;
use gpui::Div;
use gpui::Entity;
use gpui::EntityId;
use gpui::EventEmitter;
use gpui::Global;
use gpui::ParentElement as _;
use gpui::Render;
use gpui::Styled as _;
use gpui::Window;
use ui::h_flex;
use ui::prelude::ActiveTheme as _;
use ui::prelude::DynamicSpacing;
use ui::prelude::FluentBuilder as _;
use ui::prelude::InteractiveElement as _;
use ui::prelude::IntoElement;
use ui::prelude::div;
use ui::v_flex;

use crate::ItemHandle;

pub struct PaneSearchBarCallbacks {
    pub setup_search_bar: fn(&Entity<Toolbar>, &mut Window, &mut App),
    pub wrap_div_with_search_actions: fn(Div, Entity<crate::Pane>) -> Div,
}

impl Global for PaneSearchBarCallbacks {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarItemEvent {
    ChangeLocation(ToolbarItemLocation),
}

pub trait ToolbarItemView: Render + EventEmitter<ToolbarItemEvent> {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn crate::ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation;
}

trait ToolbarItemViewHandle: Send {
    fn id(&self) -> EntityId;
    fn to_any(&self) -> AnyView;
    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> ToolbarItemLocation;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarItemLocation {
    Hidden,
    PrimaryLeft,
    PrimaryRight,
    Secondary,
}

pub struct Toolbar {
    active_item: Option<Box<dyn ItemHandle>>,
    hidden: bool,
    items: Vec<(Box<dyn ToolbarItemViewHandle>, ToolbarItemLocation)>,
}

impl Toolbar {
    fn has_any_visible_items(&self) -> bool {
        self.items.iter().any(|(_item, location)| *location != ToolbarItemLocation::Hidden)
    }

    fn left_items(&self) -> impl Iterator<Item = &dyn ToolbarItemViewHandle> {
        self.items
            .iter()
            .filter(|(_, location)| *location == ToolbarItemLocation::PrimaryLeft)
            .map(|(item, _)| item.as_ref())
    }

    fn right_items(&self) -> impl Iterator<Item = &dyn ToolbarItemViewHandle> {
        self.items
            .iter()
            .filter(|(_, location)| *location == ToolbarItemLocation::PrimaryRight)
            .map(|(item, _)| item.as_ref())
    }

    fn secondary_items(&self) -> impl Iterator<Item = &dyn ToolbarItemViewHandle> {
        self.items
            .iter()
            .rev()
            .filter(|(_, location)| *location == ToolbarItemLocation::Secondary)
            .map(|(item, _)| item.as_ref())
    }

    pub fn new() -> Self {
        Self { active_item: None, items: Vec::default(), hidden: false }
    }

    pub fn add_item<T>(&mut self, item: Entity<T>, window: &mut Window, cx: &mut Context<Self>)
    where
        T: 'static + ToolbarItemView,
    {
        let location = item.set_active_pane_item(self.active_item.as_deref(), window, cx);
        cx.subscribe(&item, |this, item, event, cx| {
            if let Some((_, current_location)) = this.items.iter_mut().find(|(i, _)| i.id() == item.entity_id()) {
                match event {
                    ToolbarItemEvent::ChangeLocation(new_location) => {
                        if new_location != current_location {
                            *current_location = *new_location;
                            cx.notify();
                        }
                    }
                }
            }
        })
        .detach();
        self.items.push((Box::new(item), location));
        cx.notify();
    }

    pub fn set_active_item(&mut self, item: Option<&dyn ItemHandle>, window: &mut Window, cx: &mut Context<Self>) {
        self.active_item = item.map(|x| x.boxed_clone());
        self.hidden = self.active_item.as_ref().is_some_and(|item| !item.show_toolbar(cx));

        for (toolbar_item, current_location) in &mut self.items {
            let new_location = toolbar_item.set_active_pane_item(item, window, cx);
            if new_location != *current_location {
                *current_location = new_location;
                cx.notify();
            }
        }
    }

    pub fn item_of_type<T: ToolbarItemView>(&self) -> Option<Entity<T>> {
        self.items.iter().find_map(|(item, _)| item.to_any().downcast().ok())
    }

    pub const fn hidden(&self) -> bool {
        self.hidden
    }
}

impl Render for Toolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.has_any_visible_items() {
            return div();
        }

        let secondary_items = self.secondary_items().map(ToolbarItemViewHandle::to_any);

        let has_left_items = self.left_items().count() > 0;
        let has_right_items = self.right_items().count() > 0;

        v_flex()
            .group("toolbar")
            .relative()
            .py(DynamicSpacing::Base06.rems())
            .px(DynamicSpacing::Base08.rems())
            .when(has_left_items || has_right_items, |this| this.gap(DynamicSpacing::Base06.rems()))
            .bg(cx.theme().colors().toolbar_background)
            .when(has_left_items || has_right_items, |this| {
                this.child(
                    h_flex()
                        .items_start()
                        // `justify_between` would put a lone right group on the left.
                        .map(|this| if has_left_items { this.justify_between() } else { this.justify_end() })
                        .gap(DynamicSpacing::Base08.rems())
                        .when(has_left_items, |this| {
                            this.child(
                                h_flex()
                                    .min_h_8()
                                    .flex_auto()
                                    .justify_start()
                                    .overflow_x_hidden()
                                    .children(self.left_items().map(ToolbarItemViewHandle::to_any)),
                            )
                        })
                        .when(has_right_items, |this| {
                            this.child(
                                h_flex()
                                    .h_8()
                                    .flex_row_reverse()
                                    .when(has_left_items, gpui::Styled::flex_none)
                                    .justify_end()
                                    .children(self.right_items().map(ToolbarItemViewHandle::to_any)),
                            )
                        }),
                )
            })
            .children(secondary_items)
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ToolbarItemView> ToolbarItemViewHandle for Entity<T> {
    fn id(&self) -> EntityId {
        self.entity_id()
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> ToolbarItemLocation {
        self.update(cx, |this, cx| this.set_active_pane_item(active_pane_item, window, cx))
    }
}
