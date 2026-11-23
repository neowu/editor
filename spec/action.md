# Actions

How a view declares the actions it handles, how an action reaches a handler,
and how the app knows which actions are available. Read the code for details
(`gpui::ActionTable`, `gpui::key_dispatch`, `workspace::WorkspaceActions`,
`editor::element::actions`).

## Decision

GPUI is immediate mode, and upstream registers every action handler as a
closure on every frame. This fork does not. Handlers are static; the only
thing decided per frame is which conditional handlers apply.

- Handlers live in a `gpui::ActionTable<V>` held in a `static`, built once
  per app. Not per view, not per frame.
- Each frame a view attaches its table to a dispatch node together with its
  entity. That is one push however many actions the table holds.
- A handler may carry a condition. Conditions are evaluated when the table is
  attached, and the result is recorded on the dispatch node as a bitmask.
- Dispatch and availability queries only read what the frame recorded. They
  never read a view.

Why: an editor registered about 250 closures per paint (one `Rc` each), and
the workspace rebuilt all of its handlers on every frame.

## Tables

- A view handles actions one way: through a table. There is no per-frame
  listener API: no `Window::on_action`, no `on_action` / `capture_action` /
  `on_boxed_action` on elements.
- Attach with `.action_table(&entity, &TABLE)` on an element, or
  `Window::on_action_table` in a custom element's paint.
- Handlers are `Send + Sync` and hold no per-view or per-render state. They
  get the view as an argument and find what they need through it. State a
  handler needs from the last frame is kept on the view by paint (the markdown
  element stores the text it painted, for copy).
- A table handles a fixed set of action types. There is no wildcard or
  fallback handler: a context menu does not handle the actions of its
  entries, so with a menu open a key bound to one of them goes to the views
  above the menu.
- A table private to one view is a `static` in its module (`Pane`, the
  panels, pickers, modals).
- A view that handles different actions at different nodes has one table per
  node (the project search bar's query and filter inputs, a terminal's tab
  and its rename editor). A node outside the view's own render (an item's
  tab, drawn by the pane) attaches the table with the view's entity.
- A generic view cannot name a `static` per instantiation. `Picker<D>` keeps a
  private map of one leaked table per delegate type, looks its table up once
  when it is created, and keeps the reference. gpui has no shared registry.

## Dispatch

- Actions dispatch along the focus tree in one pass: from the focused node up
  to the window root, then the app-level handler for the action type. There
  is no capture phase.
- A handler stops propagation unless it calls `cx.propagate()`.
- The focused view always gets an action first. A view embedding an editor
  cannot take an action before that editor does. It binds the key, in its own
  key context (`BufferSearchBar > Editor`), to an action the editor does not
  handle (Tab in the search bars is `search::FocusNextField`), or relies on
  the editor propagating (a single-line editor propagates Tab, which the
  go-to-line input uses).
- An editor action dispatched while a search bar input has focus acts on that
  input, not on the searched editor (fold all, toggle soft wrap). Toggling
  soft wrap in a split diff changes the focused side only.
- On a node, tables run in attach order and handlers for the same action in
  registration order.
- Handlers run inside the view's update. They use the view they are given and
  defer anything that reads it through its handle (go-to-line opening its
  modal).

## Conditions and availability

- `add_if(condition, handler)`: while the condition is false the handler
  behaves as if it were not registered. The keystroke falls through to other
  handlers and bindings, and the action is not reported as available.
- This is the only gate. Render does not enable or disable individual
  actions.
- Attaching a table evaluates each distinct condition once. Entries passing
  the same `fn` share one evaluation; a table holds at most 64 distinct
  conditions. Use a named `fn` or one bound closure for entries that share a
  condition.
- Availability queries (command palette, macOS menu validation,
  `edctl actions --available`, `Window::is_action_available`) can be made
  anywhere, including inside an update.
- Availability is as of the view's last paint. Key dispatch redraws a dirty
  window first, so that is current state as long as whatever a condition
  reads notifies the view. A cached view (panes, panels) is not repainted for
  changes it is not notified of.
- Conditions read the view at paint, when no view is being updated. A table
  must not be attached from inside an update of its view.
- Conditions run every frame the view paints, so they must be cheap reads.
  Where the exact check is costly, the condition is coarser and the handler
  propagates when it has nothing to do. Such an action can be listed as
  available without doing anything:
  - blame actions: blame is on, not the entry under the cursor;
  - wrap in tag, format selections: the editor is editable;
  - file history, file diff (workspace): there is an active repository, not
    the active file's status;
  - LSP command selector: the active item is an editor, not whether its
    servers offer commands.
- A condition may read a flag the view's render keeps, when the check cannot
  be made at the point of query (the pane's preview actions: the editor
  context menu asks for them while the editor is being updated).

## Workspace actions

Actions that work wherever focus is in a window.

- Collected once at startup. `main` creates a `WorkspaceActions`, passes it to
  each crate's `init`, which adds handlers with `register_action` /
  `register_action_if`, then calls `install`, which appends the workspace's
  own handlers and stores the table for every workspace. Nothing is
  registered after that or per window.
- The crate that owns the behaviour registers the action. All workspaces
  share the handlers, so a handler reaches per-window state through its
  `workspace` argument (a panel, the status bar item, the title bar).
- Handlers for the same action run in registration order, i.e. the order of
  the `init` calls, and before the workspace's own.
- Workspace conditions never ask where focus is. An action that acts on a
  panel's selection (file history of the selected entry) is handled on that
  panel's node, which gets it first while the panel holds focus; the
  workspace entry acts on the active editor.
- The search actions of a pane outside the workspace's active-pane handling
  (terminal panel, terminal and Git History modes) are one `ActionTable<Pane>`
  attached with that pane.

## Editor actions

- One table for all editors, collected at startup like workspace actions:
  `main` creates an `ActionTable<Editor>`, passes it to the crates that add
  editor actions (`go_to_line`, `lsp_locations`), and calls
  `editor::install_actions`, which appends the editor's own.
- The editor has no per-instance action list. What an action depends on
  (read-only, multibuffer, blame, minimap, LSP, a Rust buffer, full mode) is
  a condition on its entry; every edit action shares the one "editable"
  condition.
- A registered handler for an action the editor also handles runs first and
  propagates to reach the editor's.

## App-level actions

- Actions that must work with no window open (quit, about, open, new window
  from the menu) are `cx.on_action` handlers. They get no window or
  workspace, run after everything in the focus tree, and are always reported
  as available.
- One handler per action type (a second registration replaces the first).

## Action values and names

- An action is a plain Rust value. Nothing builds one from JSON: actions do not
  implement `Deserialize`, and the keymap binds values
  (`k.bind("cmd-1", pane::ActivateItem(0))`), so a wrong action or field is a
  compile error instead of a startup panic.
- Each action also registers its name. A name builds the action's argument-free
  form: the unit struct, or `Default` for an action with fields. That is what
  availability queries, the command palette and `edctl action` produce.
- An action that only makes sense with its fields set by the caller is marked
  `#[action(no_build)]` (`pane::ActivateItem`, `terminal::SendText`,
  `ed::OpenBrowser`). Its name is listed by `edctl actions`, but it is never
  reported as available and can only be dispatched from code or a key binding.
- Trade-off: `edctl action` takes no arguments. Automation reaches an action
  that needs input through a key bound to it (`edctl keys`).

## Where action types are declared

See "Crate layering" in `model.md`: a type is declared in the crate that owns
its behaviour when every crate naming it can depend on that crate, otherwise
in the `actions` crate.
