# Actions: further simplification options

Written 2026-11-23, after commit `e8901aa7` (conditions evaluated at paint,
context menu fallback removed). Current design is in `spec/action.md`.
Nothing here is implemented or decided.

Three things could still go, in order of how much they remove.

## 1. Drop conditions entirely

Every handler is always registered. A handler that does not apply calls
`cx.propagate()`.

What goes:

- `add_if`, `register_action_if`, the table's condition list.
- The per-node condition mask and its evaluation at paint.
- The 64-condition limit and the "don't attach a table inside an update of
  its view" rule.
- A dispatch node becomes the entity and the table. Availability becomes "is
  the type in a table on the focus path". Per-frame cost is the one push.

What it takes:

- The 48 conditional registrations become a check at the top of the handler.
  Many handlers already have one.
- The editor's edit actions share one wrapper (`Editable` in
  `editor/src/element/actions.rs`), so "read-only, propagate" is written
  once.

UX cost:

- The command palette lists actions that do nothing in the current state:
  edit actions in a read-only editor, expand excerpts in a single file,
  branch actions on the stash tab, preview on a Rust file.
- macOS menu items are never greyed out by state.
- The editor context menu's "Open Preview" entry uses
  `is_action_available`. It would need the pane's `has_preview` flag exposed
  to it, or it always shows.

Risk:

- A handler that returns without propagating swallows the key instead of
  letting it fall through to other bindings. Has to be checked per handler.

## 2. Drop the capture phase

Done 2026-11-23. Tab in the search bars moved to the keymap; the fold-all and
soft-wrap relays and the split diff's soft-wrap sync were dropped, not
replaced. The rest of this section is the plan as written.

`add_capture` has 13 uses:

- buffer search bar: tab, backtab, toggle/fold/unfold all, toggle soft wrap;
- project search bar: tab, backtab, toggle/fold/unfold all;
- go-to-line: accept placeholder;
- split editor: toggle soft wrap.

They let a view take a key before the editor it embeds.

Alternative: in the view's key context, bind those keys to an action the
embedded editor does not handle, so it bubbles to the view. That removes the
capture pass and the capture flag from dispatch.

No UX cost expected if the keymap contexts work out. The 13 sites have not
been checked one by one.

## 3. Drop `ActionTable::shared`

One user, `Picker<D>`. It costs a global mutex-guarded map of leaked tables,
because a generic view cannot name a `static` per instantiation.

If the picker's handlers did not depend on `D`, it could use a plain `static`
like every other view. Feasibility not looked at.

## Recommendation

Do 1 if less precise palette and menus are acceptable: it removes the last
per-frame work and the last piece of machinery that needs explaining. 2 is
worth doing after that. 3 is tidy-up.

## Considered and rejected

- Render enables each conditional action (owner depends on, or knows the
  types of, the crates that register on it). Every conditional action then
  lives in two crates, and shared conditions cost one push per action per
  frame instead of one bit. The crate dependency itself would be a cycle.
- Gate bits defined by the owner and passed as a mask at attach. Keeps one
  bit per shared condition and lets render compute it, but only removes the
  function-pointer dedup and the paint-time view read, at the cost of a
  closed set of gates per owner. Moot if option 1 is taken.
