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

## Results (both done)

Same workload, HEAD vs this change (`crates/editor/src/element.rs`, 60 ×
200px scrolls per pass, rust-analyzer idle for scrolls):

| Workload | Before | After |
|---|--:|--:|
| Indexing, first 6s: frames | 573 | 141 |
| Indexing, first 6s: main thread | 514 ms | 270 ms |
| Scroll down, first pass: instr/frame (shaped) | 68.8 M (1235) | 59.4 M (1174) |
| Scroll back up: instr/frame (shaped) | 63.5 M (1199) | 35.1 M (16) |
| Scroll down again: instr/frame (shaped) | 65.2 M (1177) | 34.7 M (0) |

The first scroll pass is noisy (LSP requests for newly visible rows); don't
read the 14% there as a win.

### 1. Redraws while a language server reports progress

Not notifies. Counting `cx.notify()` per entity type during indexing:
`ActivityIndicator` 2–18/s, `LogStore` hundreds/s but not rendered, while the
window drew ~115 fps. The frames came from the status bar's rotating
`LoadCircle` spinner (`Button::loading`), an animation that asked for a frame
every vsync. Fix: `with_rotate_animation` uses `Animation::with_max_fps(10)`
(already in GPUI, unused). Applies to every spinner (14 call sites).
Coalescing `ActivityIndicator` notifies isn't worth it at that rate.

Possible further step: 10 fps is still ~10 full-window redraws/s (~1 ms each)
for minutes during indexing. A static icon, or 4–5 fps, would halve it again
at the cost of a choppier spinner.

### 2. Keep shaped lines longer than one frame

Unwrapped lines evicted from `previous_frame` go into an `lru::LruCache` of
4096 lines (`LineLayoutCache::finish_frame`), checked after both frame maps.
Hits move back into the current frame, so `used_lines` stays correct for
cached views. Wrapped lines aren't stored: their unwrapped line is. Cost per
frame is one LRU insert per line that left the frame; no measurable overhead.

Shaping costs ~3M instructions per line (~28M / 9.5 lines per frame), so the
remaining cost is first-visit shaping. Ideas if that matters: shape lines
about to scroll in during idle time, or reuse CoreText attributed-string /
font setup across lines (`OTL::GlyphLookups` setup dominated the sample).
