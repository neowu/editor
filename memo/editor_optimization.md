# Frame cost: where a frame goes, and what layout retention could save

Remeasured 2026-11-24 at `daafc2bf` to answer one question: upstream is
keeping Taffy nodes between frames (#67222, `spec/upstream.md`); is that the
right direction, and is it worth implementing here instead of waiting?

## Answer

- Right direction, small prize. Rebuilding and recomputing the layout tree
  every frame is real waste, and retention is the way to remove it.
- In this fork it can save at most about 6-9% of draw time on editor frames
  (roughly 130-160 us of a 1.7-2.5 ms frame) and about 17% on picker frames
  (about 250 us of 1.45 ms). That is the ceiling: everything Taffy computes
  plus the per-frame clear, assuming every node hits its cache.
- The ceiling is low because cached views already prune the tree. An editor
  frame lays out about 90 nodes; clean panels, panes, the status bar and the
  tab bar are single leaf nodes.
- The measured-node follow-up upstream is waiting on matters even less here:
  measure callbacks are 0.4-2.4% of draw. The full editor is not a measured
  node; its cost is prepaint, which retention does not touch.
- Not worth an own implementation. It is a change to core layout with a
  stale-layout failure mode, upstream is writing it anyway, and an own
  version would have to be reconciled with their stack later. Keep the
  decision to wait and port the stack once it settles.
- Larger levers found in the same measurement, all in owned code: the
  minimap re-render (about 40% of draw time while typing, 20% while
  scrolling) and the editor element as a whole (45-65% of draw on editor
  frames).

## How it was measured

- Temporary probes (removed): timers around `Window::draw`, view `render`
  calls, the layout engine (`request_layout`, `compute_layout`, measure
  callbacks, `layout_bounds`, `clear`) and
  `EditorElement::{request_layout, prepaint, paint}` by editor mode, written
  out per frame. The per-node timers (`request_layout`, `layout_bounds`) ran
  in separate passes so they don't inflate the others.
- Build: dev profile (opt-level 3, no LTO), `--features mimalloc`, automation
  on. MacBook Pro M5 Max, built-in display, 1200x700 window, project panel
  open, one pane. No release build was measured.
- Project: 38 Rust files, no `Cargo.toml` (rust-analyzer starts but has
  nothing to index). `hunks.rs` is 545 lines with 61 modified hunks; `big.rs`
  is 3.4k lines, unmodified.
- Workloads (`edctl`, one event per call), each after a warm-up:
  - palette: open the command palette, type `toggle`, close; 25 times.
  - cursor: down, right, up, 40 each, in `hunks.rs`.
  - typing: 30 `x`, 30 backspace, twice, in `big.rs`.
  - scroll: 40 wheel events down, 40 page-ups, in `big.rs`.
- Two probed passes plus two with per-node timers; the pairs agree within
  about 3%. Two more passes without probes gave the `edctl perf` table; the
  probes cost nothing measurable.
- The project's `.ed/workspace.json` restores tabs even with
  `launch --fresh`, which made early runs differ; it is deleted before each
  run.

## Whole frame (`edctl perf`, no probes, two runs)

| | palette | cursor | typing | scroll |
|---|---|---|---|---|
| frames drawn | 458 / 455 | 1342 / 1338 | 949 / 965 | 112 / 116 |
| instructions per frame | 13.4M | 10.9M / 10.8M | 56.7M / 57.3M | 20.3M / 19.8M |
| draw p50 (ms) | 1.48 / 1.46 | 1.78 / 1.71 | 1.52 / 1.53 | 2.79 / 2.71 |
| draw p95 (ms) | 4.20 / 4.17 | 2.81 / 2.66 | 3.85 / 3.84 | 4.59 / 4.97 |
| build p50 (ms) | 0.95 / 0.94 | 1.16 / 1.12 | 1.06 / 1.05 | 1.82 / 1.76 |
| paint p50 (ms) | 0.35 / 0.34 | 0.43 / 0.41 | 0.28 / 0.27 | 0.60 / 0.56 |

- Typing draws about 8 frames per key and its frames are bimodal (edit
  frames against animation frames), so its p50 moves between runs; the
  breakdown below uses means for it.
- Typing's instructions per frame are process-wide and mostly outside
  drawing. Not looked into here; see `memo/research/performance.md`.
- The cursor row is the workload of the 2026-11-23 table at the end, but on
  a different file, so the two are not a before/after.

## Where a frame goes

Per frame in microseconds: p50 for palette, cursor and scroll, mean for
typing. Average of the two probed passes.

| | palette | cursor | typing | scroll |
|---|---|---|---|---|
| draw | 1440 | 1660 | 1730 | 2540 |
| build (render, layout, prepaint) | 944 | 1098 | 1342 | 1692 |
| paint | 329 | 391 | 277 | 564 |
| view and component `render` | 203 | 175 | 102 | 170 |
| layout: `request_layout` in the engine | 46 | 27 | 24 | 29 |
| layout: `compute_layout` without measure | 242 | 129 | 113 | 137 |
| layout: measure callbacks | 38 | 11 | 11 | 11 |
| layout: `layout_bounds` | 18 | 13 | 11 | 13 |
| layout: `clear` | 25 | 18 | 14 | 19 |
| full editor prepaint | 0 | 549 | 942 | 1031 |
| full editor paint | 0 | 220 | 137 | 386 |
| minimap prepaint + paint (inside the two rows above) | 0 | 0 | 712 | 573 |
| single-line editor prepaint / paint | 76 / 14 | - | - | - |

Counts per frame (p50): layout nodes 211 / 90 / 91 / 91, `compute_layout`
calls 15 / 4 / 5 / 5 (one per root and per re-rendered cached view), measure
calls 89 / 16 / 16 / 16, `render` calls 103 / 43 / 44 / 44.

Share of total draw time:

| | palette | cursor | typing | scroll |
|---|---|---|---|---|
| compute without measure | 15.8% | 8.1% | 6.6% | 5.3% |
| measure callbacks | 2.4% | 1.9% | 0.6% | 0.4% |
| engine `request_layout` | 3.0% | 1.8% | 1.4% | 1.1% |
| `layout_bounds` | 1.2% | 0.8% | 0.6% | 0.5% |
| `clear` | 1.7% | 1.1% | 0.8% | 0.7% |
| all of the layout engine | 24% | 14% | 10% | 8% |
| `render` | 13% | 10% | 6% | 7% |
| full editor (layout, prepaint, paint) | 12% | 45% | 61% | 64% |

## Reading for layout retention

- What retention removes is "compute without measure" and `clear`, for the
  nodes whose style and children did not change. `request_layout` stays (it
  gains a lookup and a style comparison per node) and so does
  `layout_bounds`. The follow-up adds the measure row.
- Editor frames (cursor, typing, scroll) should get close to the ceiling,
  since moving the cursor, editing and scrolling change no styles outside
  the editor element. Picker frames get less than theirs: the result list
  changes on every keystroke.
- Upstream reports 2-8% fewer instructions on synthetic full-refresh trees,
  which matches a ceiling of this size.
- The numbers are for one pane at 1200x700. More dirty views in a frame
  (split panes editing the same buffer, a larger window with more visible
  rows in a re-rendered list) raise the node count and the saving with it;
  not measured.
- Compute costs about 1.4 us per node here (129 us for 90 nodes in 4 calls).
  Why it is that high per node was not looked into; if layout ever needs to
  be cheaper before the port, that is the place to look first.
- Not measured: a release build, and the actual saving. Only the ceiling was
  measured; nothing was prototyped.

## Reading for the editor

- The minimap is cached, but an edit or a scroll re-renders it. Typing
  re-rendered it on about half the frames at about 1.4 ms each; scrolling on
  nearly every frame at about 0.6 ms. Cursor moves leave it alone (0.1
  re-renders per frame).
- Full editor prepaint is 0.55 ms on a cursor move in a 61-hunk file, and
  roughly 0.5 ms on a scroll frame once the minimap is taken out. Paint is
  0.22 ms and 0.39 ms.
- Single-line editor (picker query box): prepaint 76 us p50, paint 14 us.
  On 2026-11-23 prepaint was 86-94 us, and before the action tables paint
  was 25 us. Still not worth a second text input implementation.
- `render` is 6-13% of draw, about 4 us per call. It counts stateless
  components as well as views.

## Earlier work, kept for reference

Details are in the code and `spec/`; only what was measured or left open is
kept here.

- Single-line editor (2026-11-22): `size_of::<Editor>()` 8,144 bytes, create
  0.18-0.25 ms and 178 allocations, about 75 KB held. A minimal entity
  (String + focus handle) is 0.002 ms, 2 allocations, 120 bytes. Not
  remeasured.
- Action registration (2026-11-22): `register_actions` cost 248 allocations
  and 13.8 us per editor per frame, about 250 `on_action` calls each making
  an `Rc`. Now static tables attached per dispatch node, with conditions
  evaluated when the table is attached at paint (`spec/action.md`,
  `spec/model.md`). Whole-frame difference was below the 2% run-to-run noise.
  - Blame revision actions are conditional only on "blame is on", so they
    are listed as available more often than they do anything.
  - Dispatch with real conditions has no unit test.
  - Still registered per frame, on cached views: project panel and git panel
    `.on_action` lists, and the pane search actions.
- Prepaint (2026-11-23): one display snapshot per pass, early returns for
  the context menu and hover popovers, selection resolution without
  `itertools::tee`, `em`/`ch` measures cached in `FontMetrics`. Single-line
  prepaint 115 -> 86-94 us (60 -> 21 allocations); full editor in a 10-line
  file 940-1040 -> 800-860 us (219 -> 144 allocations).
- Gutter and minimap (2026-11-23): early returns in the gutter (expand
  toggles, crease toggles, word diffs, run indicators), no longest-line,
  crease trailer or diff hunk work in the minimap, and the minimap as a
  cached view. Cursor moves in a 545-line file with 60 hunks: instructions
  per frame 18.06M -> 10.10M, draw p50 1.879 -> 1.407 ms, build p50
  1.517 -> 0.963 ms.
  - Not done: `active_run_indicators` by row lookup. Runnables are kept for
    about three screens, and row keys go stale for the 50 ms debounce after
    an edit while anchors don't.
  - Risk, not checked: a hunk that changes only between staged and unstaged
    relies on the multibuffer bumping `edit_count` to refresh the minimap.
