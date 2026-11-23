use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

use crate::Action;
use crate::AnyEntity;
use crate::App;
use crate::Context;
use crate::Entity;
use crate::Window;

type Handler<V> = Box<dyn Fn(&mut V, &dyn Any, &mut Window, &mut Context<V>) + Send + Sync>;
type Condition<V> = fn(&V, &App) -> bool;

/// Action handlers for views of type `V`. A table is built once, kept in a `static`,
/// and attached to the view's dispatch node each frame (`Window::on_action_table`,
/// `InteractiveElement::action_table`), instead of registering one listener per action per frame.
/// Handlers therefore cannot hold per-view state; they get the view as an argument.
///
/// A handler added with a condition (`add_if`) is skipped while the condition is false, exactly as
/// if it were not registered: the keystroke falls through to other handlers and the action is not
/// reported as available. The table's distinct conditions are evaluated once each time the table
/// is attached, so they read the view as of its last paint. Dispatch and availability queries
/// only test what the frame recorded; they never read the view.
pub struct ActionTable<V> {
    /// Sorted; handlers for the same type keep registration order.
    types: Vec<TypeId>,
    entries: Vec<Entry<V>>,
    /// The distinct conditions of the entries, at most [`MAX_CONDITIONS`].
    conditions: Vec<Condition<V>>,
}

const MAX_CONDITIONS: usize = u64::BITS as usize;

struct Entry<V> {
    /// Index into the table's conditions.
    condition: Option<u8>,
    handler: Handler<V>,
}

/// One leaked table per view type, for [`ActionTable::shared`].
static SHARED: Mutex<Option<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>> = Mutex::new(None);

impl<V> Default for ActionTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> ActionTable<V> {
    /// An empty table. `const`, so a table can be filled in a `static` before it is frozen.
    pub const fn new() -> Self {
        Self { types: Vec::new(), entries: Vec::new(), conditions: Vec::new() }
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

    /// The table for `V`, built by `build` the first time it is asked for and leaked. For generic
    /// views, which cannot name a `static` per instantiation; look it up once per view, not per
    /// frame.
    pub fn shared(build: fn(&mut Self)) -> &'static Self {
        let mut shared = SHARED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let table = *shared.get_or_insert_default().entry(TypeId::of::<V>()).or_insert_with(|| {
            let mut table = Self::new();
            build(&mut table);
            Box::leak(Box::new(table))
        });
        table.downcast_ref().expect("tables are keyed by their view type")
    }

    /// Handle `A` only while `condition` held for the view when the table was last attached.
    /// Entries sharing a condition (the same `fn`) share one evaluation per frame.
    pub fn add_if<A: Action>(
        &mut self,
        condition: Condition<V>,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        let ix = self.conditions.iter().position(|existing| std::ptr::fn_addr_eq(*existing, condition)).unwrap_or_else(
            || {
                assert!(self.conditions.len() < MAX_CONDITIONS, "an action table holds at most 64 distinct conditions");
                self.conditions.push(condition);
                self.conditions.len() - 1
            },
        );
        self.insert(Some(ix as u8), handler);
    }

    fn insert<A: Action>(
        &mut self,
        condition: Option<u8>,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        let action_type = TypeId::of::<A>();
        let ix = self.types.partition_point(|existing| *existing <= action_type);
        self.types.insert(ix, action_type);
        self.entries.insert(
            ix,
            Entry {
                condition,
                handler: Box::new(move |view, action, window, cx| {
                    handler(view, action.downcast_ref().expect("handlers are keyed by their action type"), window, cx);
                }),
            },
        );
    }

    /// The handled action types, sorted, one per handler.
    pub fn action_types(&self) -> &[TypeId] {
        &self.types
    }

    /// Whether each handler carries a condition, in the order of [`Self::action_types`].
    pub fn conditional(&self) -> impl Iterator<Item = bool> + '_ {
        self.entries.iter().map(|entry| entry.condition.is_some())
    }
}

trait AnyActionTable {
    fn action_types(&self) -> &[TypeId];
    fn condition(&self, ix: usize) -> Option<u8>;
    /// One bit per condition that holds for the view. Reads the view if there are conditions.
    fn conditions(&self, entity: &AnyEntity, cx: &App) -> u64;
    fn dispatch(&self, ix: usize, entity: &AnyEntity, action: &dyn Any, window: &mut Window, cx: &mut App);
}

impl<V: 'static> AnyActionTable for ActionTable<V> {
    fn action_types(&self) -> &[TypeId] {
        &self.types
    }

    fn condition(&self, ix: usize) -> Option<u8> {
        self.entries[ix].condition
    }

    fn conditions(&self, entity: &AnyEntity, cx: &App) -> u64 {
        if self.conditions.is_empty() {
            return 0;
        }
        let view = view::<V>(entity);
        let view = view.read(cx);
        self.conditions
            .iter()
            .enumerate()
            .fold(0, |mask, (ix, condition)| mask | (u64::from(condition(view, cx)) << ix))
    }

    fn dispatch(&self, ix: usize, entity: &AnyEntity, action: &dyn Any, window: &mut Window, cx: &mut App) {
        let handler = &self.entries[ix].handler;
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
    /// One bit per condition of the table that held when it was attached.
    pub(crate) conditions: u64,
}

impl DispatchActionTable {
    pub(crate) fn new<V: 'static>(entity: &Entity<V>, table: &'static ActionTable<V>) -> Self {
        Self { entity: entity.clone().into_any(), table, conditions: 0 }
    }

    /// Evaluate the table's conditions on the view, which must not be being updated.
    pub(crate) fn evaluate_conditions(&mut self, cx: &App) {
        self.conditions = self.table.conditions(&self.entity, cx);
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

    /// Whether handler `ix` applied to the view when the table was attached.
    pub(crate) fn applies(&self, ix: usize) -> bool {
        self.table.condition(ix).is_none_or(|condition| self.conditions & (1 << condition) != 0)
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
    fn shared_builds_one_table_per_view_type() {
        struct A;
        struct B;
        fn first<V: 'static>(table: &mut ActionTable<V>) {
            table.add(|_: &mut V, _: &First, _: &mut Window, _: &mut Context<V>| {});
        }
        fn none<V>(_: &mut ActionTable<V>) {}

        let a = ActionTable::<A>::shared(first);
        assert_eq!(a.action_types(), [TypeId::of::<First>()]);
        // The second builder is ignored: the table already exists.
        assert!(std::ptr::eq(a, ActionTable::<A>::shared(none)));
        assert!(ActionTable::<B>::shared(none).action_types().is_empty());
    }

    #[test]
    fn add_if_shares_one_condition_per_fn() {
        let yes: Condition<()> = |(), _| true;
        let no: Condition<()> = |(), _| false;
        let mut table = ActionTable::default();
        table.add_if(yes, noop::<First>);
        table.add_if(no, noop::<Second>);
        table.add_if(yes, noop::<Third>);

        assert_eq!(table.conditions.len(), 2);
    }

    #[test]
    fn applies_follows_each_handlers_condition() {
        // Different bodies, so the two cannot be folded into one function.
        let first: Condition<()> = |(), _| true;
        let second: Condition<()> = |(), _| false;
        let mut table = ActionTable::default();
        table.add(noop::<First>);
        table.add_if(first, noop::<First>);
        table.add_if(second, noop::<First>);
        let mut attached = attach(table);
        let applies = |attached: &DispatchActionTable| {
            attached.handlers_for(TypeId::of::<First>()).map(|ix| attached.applies(ix)).collect::<Vec<_>>()
        };

        // Nothing evaluated: only the unconditional handler applies.
        assert_eq!(applies(&attached), [true, false, false]);
        // Stands in for `evaluate_conditions`, which needs an `App`: only `second` holds.
        attached.conditions = 0b10;
        assert_eq!(applies(&attached), [true, false, true]);
    }

    #[test]
    fn new_table_is_empty() {
        static TABLE: ActionTable<()> = ActionTable::new();

        assert!(TABLE.action_types().is_empty());
        assert!(ActionTable::<()>::default().action_types().is_empty());
    }
}
