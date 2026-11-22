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
type FallbackMatches<V> = fn(&V, &dyn Action, &App) -> bool;
type FallbackHandler<V> = fn(&mut V, &dyn Action, &mut Window, &mut Context<V>);

/// Action handlers for views of type `V`. A table is built once, kept in a `static`,
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
    entries: Vec<Entry<V>>,
    fallback: Option<(FallbackMatches<V>, FallbackHandler<V>)>,
}

struct Entry<V> {
    condition: Option<Condition<V>>,
    /// Runs in the capture phase, before any node below it, and propagates unless it stops.
    capture: bool,
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
        Self { types: Vec::new(), entries: Vec::new(), fallback: None }
    }
}

impl<V: 'static> ActionTable<V> {
    /// Handle `A` on every view the table is attached to.
    pub fn add<A: Action>(
        &mut self,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        self.insert(None, false, handler);
    }

    /// Handle `A` in the capture phase: before the nodes below the one the table is attached to.
    /// The action keeps propagating unless the handler calls `cx.stop_propagation()`.
    pub fn add_capture<A: Action>(
        &mut self,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        self.insert(None, true, handler);
    }

    /// Handle, in the bubble phase, any action for which `matches` holds for the view and that
    /// no typed handler of this table stopped. For a view whose actions are data (a menu handling
    /// the actions of its items). Such actions are available but, having no fixed set, are not
    /// listed among the available actions.
    pub fn set_fallback(&mut self, matches: FallbackMatches<V>, handler: FallbackHandler<V>) {
        self.fallback = Some((matches, handler));
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

    /// Handle `A` only while `condition` holds for the view.
    pub fn add_if<A: Action>(
        &mut self,
        condition: Condition<V>,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        self.insert(Some(condition), false, handler);
    }

    fn insert<A: Action>(
        &mut self,
        condition: Option<Condition<V>>,
        capture: bool,
        handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + Send + Sync + 'static,
    ) {
        let action_type = TypeId::of::<A>();
        let ix = self.types.partition_point(|existing| *existing <= action_type);
        self.types.insert(ix, action_type);
        self.entries.insert(
            ix,
            Entry {
                condition,
                capture,
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
    fn is_capture(&self, ix: usize) -> bool;
    fn applies(&self, ix: usize, entity: &AnyEntity, cx: &App) -> bool;
    fn dispatch(&self, ix: usize, entity: &AnyEntity, action: &dyn Any, window: &mut Window, cx: &mut App);
    fn fallback_matches(&self, entity: &AnyEntity, action: &dyn Action, cx: &App) -> bool;
    fn dispatch_fallback(&self, entity: &AnyEntity, action: &dyn Action, window: &mut Window, cx: &mut App);
}

impl<V: 'static> AnyActionTable for ActionTable<V> {
    fn action_types(&self) -> &[TypeId] {
        &self.types
    }

    fn is_capture(&self, ix: usize) -> bool {
        self.entries[ix].capture
    }

    fn applies(&self, ix: usize, entity: &AnyEntity, cx: &App) -> bool {
        self.entries[ix].condition.is_none_or(|condition| condition(view::<V>(entity).read(cx), cx))
    }

    fn dispatch(&self, ix: usize, entity: &AnyEntity, action: &dyn Any, window: &mut Window, cx: &mut App) {
        let handler = &self.entries[ix].handler;
        view::<V>(entity).update(cx, |view, cx| handler(view, action, window, cx));
    }

    fn fallback_matches(&self, entity: &AnyEntity, action: &dyn Action, cx: &App) -> bool {
        self.fallback.is_some_and(|(matches, _)| matches(view::<V>(entity).read(cx), action, cx))
    }

    fn dispatch_fallback(&self, entity: &AnyEntity, action: &dyn Action, window: &mut Window, cx: &mut App) {
        if let Some((_, handler)) = self.fallback {
            view::<V>(entity).update(cx, |view, cx| handler(view, action, window, cx));
        }
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

    pub(crate) fn is_capture(&self, ix: usize) -> bool {
        self.table.is_capture(ix)
    }

    /// Whether handler `ix` currently applies to the view. Reads the view.
    pub(crate) fn applies(&self, ix: usize, cx: &App) -> bool {
        self.table.applies(ix, &self.entity, cx)
    }

    pub(crate) fn dispatch(&self, ix: usize, action: &dyn Any, window: &mut Window, cx: &mut App) {
        self.table.dispatch(ix, &self.entity, action, window, cx);
    }

    /// Whether the table's fallback currently handles `action`. Reads the view.
    pub(crate) fn fallback_matches(&self, action: &dyn Action, cx: &App) -> bool {
        self.table.fallback_matches(&self.entity, action, cx)
    }

    pub(crate) fn dispatch_fallback(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        self.table.dispatch_fallback(&self.entity, action, window, cx);
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
    fn add_capture_marks_only_its_handler() {
        let mut table = ActionTable::default();
        table.add(noop::<First>);
        table.add_capture(noop::<First>);
        table.add(noop::<Second>);
        let attached = attach(table);

        let first = attached.handlers_for(TypeId::of::<First>());
        let capture: Vec<bool> = first.map(|ix| attached.is_capture(ix)).collect();
        assert_eq!(capture, [false, true]);
        assert!(attached.handlers_for(TypeId::of::<Second>()).all(|ix| !attached.is_capture(ix)));
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
    fn set_fallback_adds_no_typed_handler() {
        // Matching and dispatch read the view through an `App`, which a unit test cannot build.
        let mut table = ActionTable::<()>::default();
        assert!(table.fallback.is_none());
        table.add(noop::<First>);
        table.set_fallback(|(), action, _| action.partial_eq(&Second), |(), _, _, _| {});

        assert!(table.fallback.is_some());
        assert_eq!(table.action_types(), [TypeId::of::<First>()]);
    }

    #[test]
    fn new_table_is_empty() {
        static TABLE: ActionTable<()> = ActionTable::new();

        assert!(TABLE.action_types().is_empty());
        assert!(ActionTable::<()>::default().action_types().is_empty());
    }
}
