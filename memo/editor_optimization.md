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
