use std::any::Any;
use std::any::TypeId;
use std::ops::Range;

use crate::Action;
use crate::AnyEntity;
use crate::App;
use crate::Context;
use crate::Entity;
use crate::Window;

type Handler<V> = Box<dyn Fn(&mut V, &dyn Any, &mut Window, &mut Context<V>) + Send + Sync>;
type Condition<V> = fn(&V, &App) -> bool;

/// Bubble-phase action handlers for views of type `V`. A table is built once, kept in a `static`,
/// and attached to the view's dispatch node each frame (`Window::on_action_table`,
/// `InteractiveElement::action_table`), instead of registering one listener per action per frame.
/// Handlers therefore cannot hold per-view state; they get the view as an argument.
///
/// A handler added with a condition is skipped while the condition is false, exactly as if it
/// were not registered: the keystroke falls through to other handlers and the action is not
/// reported as available. Conditions run when an action is dispatched or availability is queried,
/// never per frame. They read the view, so availability must not be queried while a view on the
/// focus path is being updated.
pub struct ActionTable<V> {
    /// Sorted; handlers for the same type keep registration order.
    types: Vec<TypeId>,
    entries: Vec<(Option<Condition<V>>, Handler<V>)>,
}

impl<V> Default for ActionTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> ActionTable<V> {
    /// An empty table. `const`, so a table can be filled in a `static` before it is frozen.
    pub const fn new() -> Self {
        Self { types: Vec::new(), entries: Vec::new() }
    }
}

impl<V: 'static> ActionTable<V> {
    /// Handle `A` on every view the table is attached to.
    pub fn add<A: Action>(
        &mut self,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        self.insert(None, handler);
    }

    /// Handle `A` only while `condition` holds for the view.
    pub fn add_if<A: Action>(
        &mut self,
        condition: Condition<V>,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        self.insert(Some(condition), handler);
    }

    fn insert<A: Action>(
        &mut self,
        condition: Option<Condition<V>>,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        let action_type = TypeId::of::<A>();
        let ix = self.types.partition_point(|existing| *existing <= action_type);
        self.types.insert(ix, action_type);
        self.entries.insert(
            ix,
            (
                condition,
                Box::new(move |view, action, window, cx| {
                    handler(view, action.downcast_ref().expect("handlers are keyed by their action type"), window, cx);
                }),
            ),
        );
    }

    /// The handled action types, sorted, one per handler.
    pub fn action_types(&self) -> &[TypeId] {
        &self.types
    }

    /// Whether each handler carries a condition, in the order of [`Self::action_types`].
    pub fn conditional(&self) -> impl Iterator<Item = bool> + '_ {
        self.entries.iter().map(|(condition, _)| condition.is_some())
    }
}

trait AnyActionTable {
    fn action_types(&self) -> &[TypeId];
    fn applies(&self, ix: usize, entity: &AnyEntity, cx: &App) -> bool;
    fn dispatch(&self, ix: usize, entity: &AnyEntity, action: &dyn Any, window: &mut Window, cx: &mut App);
}

impl<V: 'static> AnyActionTable for ActionTable<V> {
    fn action_types(&self) -> &[TypeId] {
        &self.types
    }

    fn applies(&self, ix: usize, entity: &AnyEntity, cx: &App) -> bool {
        self.entries[ix].0.is_none_or(|condition| condition(view::<V>(entity).read(cx), cx))
    }

    fn dispatch(&self, ix: usize, entity: &AnyEntity, action: &dyn Any, window: &mut Window, cx: &mut App) {
        let handler = &self.entries[ix].1;
        view::<V>(entity).update(cx, |view, cx| handler(view, action, window, cx));
    }
}

fn view<V: 'static>(entity: &AnyEntity) -> Entity<V> {
    entity.clone().downcast().expect("a table is attached with a view of its own type")
}

/// An [`ActionTable`] attached to a dispatch node, with the view its handlers run on.
#[derive(Clone)]
pub(crate) struct DispatchActionTable {
    entity: AnyEntity,
    table: &'static dyn AnyActionTable,
}

impl DispatchActionTable {
    pub(crate) fn new<V: 'static>(entity: &Entity<V>, table: &'static ActionTable<V>) -> Self {
        Self { entity: entity.clone().into_any(), table }
    }

    pub(crate) fn action_types(&self) -> &[TypeId] {
        self.table.action_types()
    }

    /// Indices of the handlers for `action_type`, in registration order.
    pub(crate) fn handlers_for(&self, action_type: TypeId) -> Range<usize> {
        let types = self.table.action_types();
        let start = types.partition_point(|existing| *existing < action_type);
        let len = types[start..].iter().take_while(|existing| **existing == action_type).count();
        start..start + len
    }

    /// Whether handler `ix` currently applies to the view. Reads the view.
    pub(crate) fn applies(&self, ix: usize, cx: &App) -> bool {
        self.table.applies(ix, &self.entity, cx)
    }

    pub(crate) fn dispatch(&self, ix: usize, action: &dyn Any, window: &mut Window, cx: &mut App) {
        self.table.dispatch(ix, &self.entity, action, window, cx);
    }
}

#[cfg(test)]
#[allow(unreachable_pub, reason = "actions! emits pub structs")]
mod tests {
    use super::*;
    use crate::app::EntityMap;
    actions!(action_table_test, [First, Second, Third]);

    fn noop<A: Action>(_: &mut (), _: &A, _: &mut Window, _: &mut Context<()>) {}

    fn attach(table: ActionTable<()>) -> DispatchActionTable {
        let mut entities = EntityMap::new();
        let slot = entities.reserve::<()>();
        DispatchActionTable::new(&entities.insert(slot, ()), Box::leak(Box::new(table)))
    }

    #[test]
    fn add_keeps_types_sorted() {
        let mut table = ActionTable::default();
        table.add(noop::<First>);
        table.add(noop::<Second>);
        table.add(noop::<Third>);

        assert_eq!(table.action_types().len(), 3);
        assert!(table.action_types().is_sorted());
        assert!(table.action_types().contains(&TypeId::of::<Second>()));
    }

    #[test]
    fn handlers_for_the_same_type_keep_registration_order() {
        let mut table = ActionTable::default();
        table.add(noop::<First>);
        table.add_if(|(), _| false, noop::<First>);
        table.add(noop::<Second>);
        table.add(noop::<First>);
        let conditional: Vec<bool> = table.conditional().collect();
        let attached = attach(table);

        let handlers = attached.handlers_for(TypeId::of::<First>());
        assert_eq!(handlers.len(), 3);
        assert_eq!(conditional[handlers], [false, true, false]);
        assert_eq!(attached.handlers_for(TypeId::of::<Second>()).len(), 1);
    }

    #[test]
    fn handlers_for_an_unhandled_type_is_empty() {
        let mut table = ActionTable::default();
        table.add(noop::<First>);
        table.add(noop::<Third>);
        let attached = attach(table);

        assert!(attached.handlers_for(TypeId::of::<Second>()).is_empty());
        assert!(attach(ActionTable::default()).handlers_for(TypeId::of::<First>()).is_empty());
    }

    #[test]
    fn attached_table_reports_the_tables_types() {
        let mut table = ActionTable::default();
        table.add(noop::<First>);
        table.add(noop::<Second>);
        let types = table.action_types().to_vec();

        assert_eq!(attach(table).action_types(), types);
    }

    #[test]
    fn new_table_is_empty() {
        static TABLE: ActionTable<()> = ActionTable::new();

        assert!(TABLE.action_types().is_empty());
        assert!(ActionTable::<()>::default().action_types().is_empty());
    }
}
