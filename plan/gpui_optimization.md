# GPUI CPU optimization: follow-ups

Snapshot: Nov 2026. Context for two follow-ups found while profiling scrolling
and idle CPU. Done so far: `edctl perf` tooling (`spec/automation.md`), minimap
drawn as blocks and minimap rows cached across frames (`spec/editor.md`).

## How to measure

- Build release: `cargo build --release -p ed -p control`.
- Drive with `target/release/edctl`. `launch --fresh --app target/release/ed <file>`,
  then `wait --id editor`, `wait-lsp`, `perf-reset`, the workload, then `perf`.
  Compare **instructions per frame**: they vary <2% between runs, while ms per
  frame shifts with CPU clocks. Keep the window visible.
- `perf` also reports `lines shaped` (line layout cache misses per frame).
- Find where time goes with `sample <pid> 30 -file out.txt` while a workload
  loops in the background. Symbols are v0-mangled. `rustfilt` isn't
  installed; a crude parser that aggregates the call tree under a root frame
  was enough (inclusive and self samples under `gpui::window::Window::draw`).

Baseline after the minimap work (`crates/editor/src/element.rs` open,
60 × 200px scrolls; rust-analyzer idle):

| Workload | Instr/frame | Draw mean | Lines shaped/frame |
|---|--:|--:|--:|
| Scroll, minimap on | 47.6 M | 4.0 ms | ~9.5 |
| Scroll, minimap off | 31 M | 2.35 ms | ~8.5 |
| Idle (cursor blink, 2 fps) | 29 M | 5.3 ms | ~0 |

## 1. Redraws while a language server reports progress

### Finding

While rust-analyzer reports work ("Fetching…", "Indexing: 395/469"), the window
draws at display rate: 240–320 frames per 3s, ~11% of a core on the main
thread. It drops to the cursor blink (~2 fps) the moment the work list is
empty. On a big project, indexing lasts minutes, and `cargo check` progress
runs on every save, so this is steady-state cost, not just startup.

Repro: `edctl launch --fresh` on a Rust file in this repo, then every 3s
`perf-reset` / sleep / `perf` and `state` (its `language_servers[].work`).
Frames per window track `work` being non-empty exactly.

### What isn't known yet

Which entity notifies. A 3s startup sample showed the frames were cheap
(~1 ms draw), but they included the main editor's prepaint and the minimap,
plus `Scene::replay` and `BoundsTree::insert` for the cached pane. So
it's not only the status bar redrawing. Candidates:

- `activity_indicator` (`crates/activity_indicator/src/activity_indicator.rs`):
  it calls `cx.notify()` on every language server status update (around
  line 188) and on `fs` job events. Every `$/progress` notification
  (rust-analyzer sends many per second while indexing) becomes a redraw.
- The LSP status button (`lsp-tool` in the status bar).
- Something in the editor that refreshes per progress event (inlay hints,
  diagnostics pulls, semantic tokens), which would explain the editor
  prepaint in those frames.

First step: find who notifies. Options: count `cx.notify()` per entity type for
one second, or sample during indexing and check which views' `render` show up
(not just `prepaint`).

### Fix direction

- Coalesce: on a status event, store the new status and schedule at most one
  `cx.notify()` per interval (e.g. 250ms) with a pending flag and a
  `cx.spawn` timer. The text "Indexing: 395/469" doesn't need 120 updates/s.
- If the editor is the one notifying, make that path skip the notify when
  nothing it renders changed.
- Check any spinner/animation shown while work is in flight. A rotating icon
  would ask for a frame every vsync by design.

Done when `perf` during indexing shows a few frames per second, not ~100.

## 2. Keep shaped lines longer than one frame

### Finding

`LineLayoutCache` (`crates/gpui/src/text_system/line_layout.rs`) keeps two
generations: `current_frame` and `previous_frame`. `finish_frame` swaps
them and clears the new current one. A line shaped two frames ago is gone. So:

- Scrolling down then back up reshapes every line that left the viewport.
- In a scroll sample, CoreText shaping (`shape_line` →
  `CTLineCreateWithAttributedString`) was ~35% of draw time. Much of it is
  `OTL::GlyphLookups` setup per created line (font feature tables), so cost
  is per line, not per glyph. The minimap no longer shapes; what's left is
  the main editor: ~8.5 lines shaped per frame on a 200px-per-step scroll.
- Line numbers in the gutter are shaped the same way (`shape_line_number`).

### Fix direction

Add a bounded third tier: lines evicted from `previous_frame` go into an LRU
(keyed by the same `CacheKey`: text, font size, font runs, wrap width, force
width), capped by count and/or total glyphs. A miss in both frame maps checks
the LRU before calling `platform_text_system.layout_line`. Same for
`wrapped_lines`.

Constraints:

- Keep the frame maps as they are: `layout_index` / `reuse_layouts` /
  `truncate_layouts` rely on `used_lines` ordering for cached views. The LRU
  is only a fallback behind them.
- `clear_if_font_generation_changed` must clear the LRU too.
- The cache is per window (`WindowTextSystem`, `RefCell`, single-threaded).
- Bound the memory. A `LineLayout` holds one `ShapedGlyph` (~32 bytes) per
  glyph; 5–10k lines is a few MB.
- Unit tests next to the existing `LineLayoutCache` tests
  (`shaped_line_count_counts_only_cache_misses` shows how to count misses).

Measure with `lines shaped` from `edctl perf`: scroll down N steps then back up
N steps. The way back should shape ~0 lines. Also check instructions per frame
for a plain scroll; the LRU lookup must not cost more than it saves.
