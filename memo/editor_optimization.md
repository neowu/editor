# Editor element: paint / prepaint / action findings

Measured 2026-11-22 while deciding whether pickers need a lighter text input
than `Editor`. Input for the action redesign.

## How it was measured

- Workload: open the command palette, type, close; repeated 6 times
  (about 120 frames), driven by `edctl`.
- Temporary instrumentation (removed): a counting global allocator and timers
  around `EditorElement::{request_layout, prepaint, paint}`, plus allocation
  backtraces for one frame.
- Build: dev profile (opt-level 3, no LTO), automation recording on.
- Caveat: the counting allocator wrapped the system allocator, not mimalloc,
  so allocation-heavy timings are pessimistic. Allocation counts are exact.
- Scope: only the editor element was instrumented. Nothing here shows the
  editor is the largest cost in a frame; other elements were not measured.
  For scale, a whole frame's draw was about 0.65-0.85 ms p50 in the same
  workload (`edctl perf`).

## Single-line editor (picker query box)

| | Value |
|---|---|
| `size_of::<Editor>()` | 8,144 bytes |
| Create (`Editor::single_line`) | 0.18-0.25 ms, 178 allocations |
| Memory held after creation | about 75 KB (172 KB allocated, 97 KB freed) |
| Minimal entity (String + focus handle), for comparison | 0.002 ms, 2 allocations, 120 bytes |
| `request_layout` p50 | 1.2 us, 1 allocation |
| `prepaint` p50 / p95 | 105 us / 227 us, 50-85 allocations |
| `paint` p50 / p95 (before the action table) | 25 us / 38 us, about 260 allocations |
| First frame after app launch | prepaint 1.4 ms, paint 0.64 ms (font and shaping warm-up) |
| First frame of later opens | prepaint about 0.15-0.18 ms, paint about 0.03 ms |

A picker's editor exists only while the picker is open. About 30 call sites
create single-line editors (search bars, project panel rename, go-to-line,
outline filter, terminal, git UI); how many are alive at once was not counted.

Conclusion at the time: not worth replacing with a simple text input for
speed or memory. The saving is tens of KB per input and about 0.1 ms per
frame, against maintaining a second text-editing implementation. gpui has no
text input element; the only `EntityInputHandler` is the editor's.

## Paint: where the allocations were

Per frame, single-line editor, before the change:

| Step in `paint` | Time p50 | Allocations |
|---|---|---|
| `register_actions` | 13.8 us | 248 |
| `register_key_listeners` | 0.1 us | 1 |
| `paint_mouse_listeners` | 0.3 us | 3 (6 listeners in the backtrace sample) |
| `paint_text` | 2.1 us | 0 |
| scrollbars and the rest | under 0.2 us | 0 |

Cause: `register_actions` called `register_action` about 250 times per frame.
Each call did `window.on_action(type, closure)`, which wraps the closure in a
new `Rc` and pushes it onto the dispatch node. All of it is dropped when the
frame is replaced. Every painted editor paid this every frame.

Other per-paint allocations seen in the backtrace sample: key context
construction (`key_context`, 3), `set_key_context` (1), `handle_input` (1),
`all_buffers` in `rust_analyzer_ext::apply_related_actions` (1), a few
`format!` calls, and UI snapshot recording (automation only).

## Prepaint: no single hot spot

About 92 us summed over these ranges of `prepaint` (line numbers in
`crates/editor/src/element.rs` at the time of measuring):

| Range | Roughly what it covers | Time p50 | Allocations |
|---|---|---|---|
| start-1349 | snapshot, font metrics, gutter dimensions, wrap width | 9.9 us | 0 |
| 1349-1425 | hitboxes, autoscroll, scroll position, visible rows | 9.0 us | 5 |
| 1425-1448 | row infos, start/end anchors | 2.2 us | 2 |
| 1448-1627 | highlighted rows and ranges, selections layout | 12.9 us | 17 |
| 1627-1658 | selection head, run indicators, gutter | 8.9 us | 5 |
| 1658-1700 | line numbers, expand/crease toggles, diff hunks | 6.4 us | 2 |
| 1700-1745 | background segments, line layout | 11.4 us | 4 |
| 1745-1780 | longest line, scrollbar info, sticky header | 6.5 us | 4 |
| 1780-1931 | indent guides, blocks, sticky buffer header | 4.6 us | 5 |
| 1931-2067 | inline diagnostics, blame, prepaint lines, cursors, overlays | 3.3 us | 3 |
| 2067-2114 | scrollbars, cursor popovers, gutter menu | 0.8 us | 0 |
| 2114-2178 | run indicators, hover popovers, context menu, toggles, wrap guides, minimap | 13.7 us | 3 |
| 2178-end | invisible glyph shaping, position map | 2.7 us | 2 |

Allocation call sites from one sampled frame (85 allocations):

- 35 directly in the `prepaint` closure body (not attributed further).
- 17 through a `fmt` call inside `prepaint` (a `format!`-style site, not
  pinned down).
- About 15 in selection resolution: `itertools::tee` in
  `selections_collection::resolve_selections_*` and
  `summaries_for_anchors`.
- A few each in `layout_selections`, `layout_lines`, `layout_visible_cursors`,
  `layout_line_numbers`, `parse_drawn_buffers`.

A single-line editor still runs every step, including ones that cannot apply
to it (gutter, minimap, blame, indent guides, blocks, sticky headers). Most
return quickly, but the walk itself is the 92 us.

## What was changed (action tables)

- gpui: `Window::on_action_table(action_types, listener)` registers one
  bubble-phase listener for a static, sorted `&'static [TypeId]`; the listener
  gets the index of the matching type. Stored per dispatch node as
  `DispatchActionTable` (`crates/gpui/src/key_dispatch.rs`). Dispatch,
  `available_actions` and `is_action_available` consult tables after the
  node's individual listeners.
- editor: unconditional actions live in two `LazyLock` tables in
  `crates/editor/src/element/actions.rs`: `ACTIONS` (all editors) and
  `EDIT_ACTIONS` (not read-only). `register_actions` registers each table with
  one call.
- Still registered one by one, every frame:
  - dynamic `editor.editor_actions` (added by other crates through
    `Editor::register_action`)
  - rust-analyzer actions (Rust buffers only)
  - multibuffer excerpt expansion (3)
  - `accept_inlay_hint`, `toggle_diagnostics` (LSP data enabled)
  - `toggle_minimap`
  - blame: `open_git_blame_commit`, `blame_revision`,
    `blame_previous_revision`
  - `wrap_selections_in_tag`, `format_selections` (not read-only)

Result for `register_actions`:

| | Before | After |
|---|---|---|
| Single-line editor | 248 allocations, 13.8 us | 6 allocations, 3.0 us |
| Main editor | not measured | 16 allocations, 2.9 us |

Whole-frame A/B (`edctl perf`, 512 frames, two runs each) showed no
measurable difference: draw p50 0.644 / 0.843 ms old against 0.682 / 0.823 ms
new, instructions per frame 12.65M / 12.96M old against 12.78M / 12.77M new.
Run-to-run noise is about 2%, larger than the change. With mimalloc the real
saving is probably a few microseconds per editor per frame.

## Notes for an action redesign

- Per-frame registration is what costs: anything built in `paint` and thrown
  away each frame. Tables remove the allocation but the editor still re-runs
  the conditions every frame (`buffer_kind`, `lsp_data_enabled`,
  `can_accept_inlay_hint`, `supports_minimap`, `blame()`,
  `blame_revision_target`, `blame_previous_revision_target`, `read_only`,
  `enable_wrap_selections_in_tag`, `can_format_selections`). These were not
  timed individually; all of `register_actions` is now about 3 us.
- Conditional registration does two jobs:
  1. Key fall-through. An unregistered action lets the keystroke continue to
     parent nodes. A static handler that does not apply must call
     `cx.propagate()` to stay equivalent.
  2. Availability. `available_actions` / `is_action_available` drive the
     command palette and anything that shows whether an action can run. A
     fully static list makes inapplicable actions appear available.
- Tables are bubble-phase only. The old per-action listeners were also invoked
  in the capture phase and ignored it.
- Within a node, individual listeners run before tables, and tables in
  registration order. Table builders reject duplicate action types; `ACTIONS`
  and `EDIT_ACTIONS` are tested to be disjoint. Nothing checks duplicates
  between a table and the individually registered actions.
- Dispatch clones a node's whole `action_listeners` Vec for each phase
  (`crates/gpui/src/window/dispatch.rs`); tables are walked by index without
  cloning.
- `editor_actions` is a `BTreeMap` of boxed closures that each call
  `window.on_action` per frame, so every externally registered editor action
  still costs one `Rc` per frame per editor.
- The dispatch tree is rebuilt every frame (`next_frame.dispatch_tree`), so
  any design that keeps registrations per frame pays at least a push per
  entry. Keeping the action set on the entity or the type, with the node only
  pointing at it, avoids that.

## Redesign step 1: static tables for workspace, pane and editor actions (2026-11-22)

Supersedes the gpui side of "What was changed" above, and the notes there on
table-only bubble order, duplicate checks and `editor_actions`.

- `gpui::ActionTable<V>` replaces the `&'static [TypeId]` + listener form.
  Tables live in `static`s; a dispatch node holds the view's entity and a
  `&'static` table, so attaching allocates nothing beyond the node's Vec.
  Handlers are `Send + Sync` boxes with no per-view state. Duplicate action
  types are allowed and run in registration order.
- `add_if(condition, handler)`: the condition is checked at dispatch (false
  means skipped, as if unregistered) and by `available_actions` /
  `is_action_available`. Nothing is evaluated per frame.
- Workspace: `main` builds one `WorkspaceActions`, each crate's `init` adds
  its handlers to it, and `install` appends the workspace's own and stores
  the table in a `static`. Nothing is registered per window or after
  startup; `Workspace::register_action` and `register_action_renderer` are
  gone. Before, each render rebuilt every handler as a `Box` plus an `Rc`,
  on a root view that renders every frame. Not measured.
- Workspace conditions (`register_action_if`): the 3 save actions (active
  item is editable), `git::FileHistory`, `git::OpenFileDiff`, the git graph
  `Open` / `OpenAtCommit` (active repository), and the LSP command selector
  toggle. The former renderer callbacks became these.
- Pane: one static table attached by `Pane::render`. Only `OpenPreview` /
  `OpenPreviewToTheSide` are conditional, on `has_preview`, a flag cached at
  the pane's last render so the editor context menu can ask for them while
  the editor is updated.
- Editor: one table for all editors, installed at startup
  (`editor::install_actions`). `go_to_line` and `lsp_locations` add their
  handlers to it first, so they run before the editor's own and propagate
  to them. `Editor::register_action`, `register_action_renderer` and the
  per-editor `editor_actions` map are gone. Edit actions carry a "not
  read-only" condition; multibuffer, inlay hint, diagnostics, minimap,
  blame, Rust and full-mode actions carry their own.
- Blame revision actions: the exact target needs `&mut Editor` and the
  window, so the condition is only "blame is on" and the handlers propagate
  when the cursor is not on a blamed line. They are listed as available
  more often than they do anything.
- Handlers that captured per-window state now look it up from the
  workspace: the title bar's project menu (`Workspace::titlebar_item`) and
  the language server button's menu (status bar item, registered in
  `language_tools`). The app crate's duplicate panel `ToggleFocus` handlers
  are gone; the panel crates register them. `git::FileHistory` for a panel
  selection is handled by the git panel and project panel themselves, no
  longer by a workspace handler that checked focus.
- Superseded: conditions are no longer lazy. They are evaluated when the
  table is attached at paint and recorded on the dispatch node; see
  `spec/model.md`. The double-lease constraint below and the palette
  deferral are gone, and the fallback (context menu entry actions) was
  dropped.
- Cost of lazy conditions: an availability query panics (double lease)
  when made while any entity a condition reads is being updated. That is
  the views on the focus path, and for workspace conditions also the
  active item, its buffer and the project. Current call sites are safe:
  the command palette defers its query, the editor context menu computes
  `FormatSelections` directly, and the pane preview condition reads the
  cached flag. A new `is_action_available` call inside a render or update
  needs the same care.
- Other costs: the rendered frame holds the workspace and pane strongly
  (was weak), so a closed pane lives until the next frame. Both tables are
  install-once statics; `Workspace::render` panics without `install`, while
  the editor falls back to its built-in table. Dispatch with real
  conditions has no unit test (tests substitute the `applies` check); it
  was checked with `edctl` in a read-only editor (edit and save actions
  unavailable and ignored, navigation still handled) and with
  `GoToDefinition` falling through to the editor's own handler.
- Still per frame: project panel and git panel `.on_action` lists and the
  pane search actions (`register_pane_search_actions`), all on cached
  views, so only paid when dirty.

## Prepaint follow-up (2026-11-23)

From reading the four most expensive ranges above. Measured below.

- One snapshot per pass: `layout_selections`, the diff hunk cursor row and
  `active_run_indicators` each re-synced the display map; they now read the
  prepaint snapshot.
- `layout_mouse_context_menu` laid out two rows and `layout_hover_popovers`
  cloned the `EditorStyle` before checking that anything was shown; both
  check first.
- Selection resolution: `itertools::tee` replaced by `SmallVec` collects,
  `pending()` returns early without a pending selection, and the newest
  selection is matched by id instead of being resolved again.
- `MultiBuffer::language_settings` no longer clones the multibuffer snapshot
  (called from `soft_wrap_mode` and `wrap_guides` every frame).
- `bg_segments_per_row` returns early with no highlights and only empty
  selections; header selection data is skipped when headers are hidden.
- Wrap guides use one space's shaped width times the column instead of
  shaping a run of spaces per guide.
- The diff hunk cursor row reuses the newest selection head from
  `layout_selections`. Recomputing it late in prepaint cost about 20 us in a
  full editor (a second `SelectionLayout::new`), against about nothing when it
  ran right after the first.
- Font metrics: `FontMetrics` carries the width and advance of `m` and `0`
  in font units (0 when the glyph is missing), so `em_width`, `em_advance`,
  `ch_width` and `ch_advance` scale a cached value instead of calling
  CoreText. Font size changes need no invalidation.

### Measured

- Temporary instrumentation (removed): timers and a per-thread allocation
  count at the range boundaries of `prepaint`, with a counting wrapper around
  mimalloc. Dev profile, `--features mimalloc`, 1200x700 window.
- Workload (`edctl`): open the command palette, type `toggle`, close, 25
  times (about 300 single-line frames); then down / right / up 40 times in a
  10-line Rust file with one diff hunk (460 full-editor frames, minimap on;
  the minimap's nested prepaint is counted in its parent).
- Each build ran twice; both p50 values are given. The first run of a pair
  is usually the slower one, and runs differ by up to about 10%.

Single-line editor, p50 us (allocations):

| Range | Before | Without font cache | Final |
|---|---|---|---|
| whole prepaint | 114.7 / 115.2 (60) | 96.8 / 99.7 (21) | 86.3 / 94.4 (21) |
| snapshot, font metrics, wrap width | 14.2 / 17.8 (0) | 16.0 / 15.4 (0) | 7.0 / 6.6 (0) |
| highlights, selections | 18.5 / 18.9 (22) | 9.6 / 9.3 (4) | 9.5 / 8.9 (4) |
| background segments, line layout | 10.7 / 11.1 (4) | 11.7 / 11.0 (4) | 12.0 / 11.2 (4) |
| run indicators .. minimap | 12.2 / 12.2 (3) | 1.5 / 1.3 (0) | 1.4 / 1.2 (0) |

Full editor, p50 us (allocations):

| Range | Before | Without font cache | Final |
|---|---|---|---|
| whole prepaint | 1039.8 / 937.8 (219) | 923.0 / 844.1 (144) | 858.4 / 798.0 (144) |
| snapshot, font metrics, wrap width | 59.2 / 47.2 (1) | 58.2 / 47.2 (1) | 23.4 / 20.1 (1) |
| highlights, selections | 102.5 / 88.8 (22) | 66.7 / 55.9 (4) | 64.5 / 57.4 (4) |
| background segments, line layout | 153.2 / 142.6 (40) | 152.7 / 144.6 (39) | 144.5 (39, second run) |
| run indicators .. minimap | 353.7 / 333.8 (87) | 261.6 / 246.3 (51) | 250.1 / 241.8 (51) |

- Largest single savings: the context menu check (about 11 us single-line,
  about 90 us full, which includes the minimap editor's own prepaint), the
  font cache (about 8 us single-line, about 27 us full, where the gutter
  also asks for the `0` measures), and selection resolution (about 9 us
  single-line, about 30 us full).
- Line layout did not change, as expected.
- Removing the extra display-map syncs from the gutter range changed nothing
  measurable (173 us before and after): syncing an unchanged map is cheap.
- What is left in a full editor: gutter and diff hunks about 175 us, line
  layout about 145 us, longest line / blocks / cursors about 110 us, and the
  minimap range about 240 us.
