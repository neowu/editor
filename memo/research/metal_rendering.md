# Window presentation: why ed owns its swap chain (2026-11-22)

Started as "why does ed use ~300 MB with one project open". Short version:
**most of it was `CAMetalLayer` drawables we could not bound, and the same
path scheduled every frame about 41 ms ahead. ed now draws into its own
`IOSurface`s on a plain `CALayer`, paced by the view's `CADisplayLink`.**
The design itself is in `spec/platform.md`; this memo records how we got
there and what was measured.

## Setup

- MacBook Pro `Mac14,6`, Apple M2 Max, macOS 27.2.1, built-in display at
  120 Hz. No external display was attached for any of this.
- One window at full screen size: 1728×1084 logical, 3456×2168 device
  pixels. A BGRA8 surface of that size is 28.6 MiB (shown as 29 MB below).
- Memory: `footprint <pid>` and `vmmap <pid>` (window surfaces show up as
  `IOSurface` regions with their pixel size). Heap attribution:
  `MallocStackLogging=lite` plus `malloc_history <pid> -callTree`.
- ed was driven with `edctl launch --fresh`; frame times from `edctl perf`.
- Core Animation behaviour was isolated in two throwaway Swift apps (same
  window size, a full-window clear per frame). They are not in the repo.

## Where the memory was

Fresh release build, this repo open, clean tree: **235 MB**.

| Category | Size |
|---|---|
| Window drawables (5 × 29 MB `CAMetalLayer Display Drawable`) | 146 MB |
| Malloc heap, dirty (36 MB live, the rest freed-but-retained pages) | 60 MB |
| Other GPU memory, mapped files, everything else | ~29 MB |

- A 17-hour instance with two full-size windows was at 467 MB: 10
  drawables (263 MB), 89 MB live heap, ~52 MB of other GPU memory.
- The heap is spread thin (frame scene/element arenas ~10 MB, spell-check
  dictionary 4.2 MB, buffers, language and LSP state). Nothing there was
  worth chasing.
- With paths on screen (selections, git hunks), the window-sized path
  resolve texture adds another 29 MB of private GPU memory per window. It
  is unchanged by this work.

## Why `CAMetalLayer` could not be bounded

Measured in the test app, then confirmed in ed where noted.

- **The pool ignores `maximumDrawableCount` under a `CAMetalDisplayLink`.**
  It grew to 5 drawables at 120 Hz, and 4 when forced to 60 fps, with the
  cap at 2 or at 3. `preferredFrameLatency` (1, 2, 3) changed nothing.
- **A lone frame takes 3.** One on screen, one drawn, one handed out for
  the trailing idle update.
- **The pool never shrinks on its own.** Idle, hide/show and link
  recreation all left it at 5. It is only dropped when `drawableSize`
  changes.
- **Direct `nextDrawable` honours the cap** (the pool stayed at 2), but
  that path blocks: 6.6 ms mean per frame here, and ~33 ms per frame on
  base M-series Macs on macOS 26+, which is why ed had moved to the link.
- **The link schedules each frame ~41 ms ahead.** The gap between the link
  callback and the update's target presentation time was 41.4 ms (five
  refreshes) in the test app and 41.6 ms across 246 frames of scrolling
  and typing in ed. In the test app the drawable's `presentedTime` matched
  that target within 0.02 ms, so this looks like real latency, not just an
  early callback. That last step was not measured inside ed.

## Attempt 1: trim idle drawables (built, measured, dropped)

Nudging `drawableSize` by one pixel and back releases every drawable that
is not on screen. A version that did this about a second after a window
stopped drawing took a fresh instance from 235 MB to ~140 MB (5 → 2
drawables) with no visible effect on frame times.

It was dropped because it is a workaround with a policy attached:

- It needs a timer and a rule for when a window counts as idle. A blinking
  cursor draws a lone frame every 500 ms, so "no frame for a second" never
  holds in the focused window; the first version polled once a second
  forever there.
- Every resume after a pause reallocates the pool, and type / read / type
  is the normal rhythm of an editor.
- The eviction is observed behaviour, not a documented guarantee.
- It does nothing about the 41 ms.

`memo/metal_render_review.md` is an independent review of that version.
Its findings about the trim were right; its recommendation (trim only
occluded windows) does not fit a window that is almost always visible.

## Attempt 2: own swap chain (shipped)

Test app, 10 s of continuous drawing at 120 Hz, mean of 3 runs:

| | `CAMetalLayer` + `CAMetalDisplayLink` | Own, 3 surfaces | Own, 2 surfaces |
|---|---|---|---|
| Frames rendered / skipped | 1199 / 0 | 1200 / 0 | 602 / 598 |
| Callback interval p95 / max | 9.8 / 16.7 ms | 9.2 / 11.3 ms | 8.4 / 10.6 ms |
| Our process CPU | 1065 ms | 835 ms | 661 ms |
| WindowServer CPU | 4207 ms | 4307 ms | 2530 ms |
| Window surfaces held | 5 (146 MB) | 3 (86 MB) | 2 (57 MB) |
| Callback → target display time | 41.4 ms | 8.1 ms | 8.1 ms |

- **It costs the OS nothing extra.** WindowServer does the same work
  (within ~2%) and our process uses ~22% less CPU. A cursor-blink pattern
  (one frame every 500 ms) showed the same.
- **Two surfaces cannot hold 120 Hz.** The compositor lets go of a replaced
  surface ~11 ms later (p50 10.2, p95 14, max 16.7), after the next 8.3 ms
  refresh, so every other frame is skipped. Three had zero skips.
- **Hence the pool rule:** allocate on demand up to 3, skip the frame if
  the compositor still holds all of them. Frames are full images, so a
  skipped one is simply drawn on the next refresh.
- The main-thread cost per frame is higher (0.48 vs 0.11 ms) because the
  frame waits for the GPU to schedule before the layer shows the surface.

## Result in ed

| | Before | After |
|---|---|---|
| Window surfaces | 5 (146 MB) | 2–3 (57–86 MB) |
| Footprint, fresh instance | 271 MB | 207–211 MB |
| Callback → target display time | ~41 ms | next refresh (≤ 8.3 ms at 120 Hz) |

Both footprints are on a tree with uncommitted changes, which puts the
29 MB path texture in both; the clean-tree baseline was 235 MB.

- About 400 frames across scrolling and three `edctl resize` steps: zero
  skipped frames, never more than 3 surfaces, old sizes freed, no errors.
- The front surface shows in `vmmap` as shared with WindowServer.
- Scroll burst: draw p95 3.5 ms, present p95 0.4 ms.
- Typing, scrolling and resizing were checked by eye.
- Besides memory, this removed the second presentation path: the
  synchronous `nextDrawable` draw for resize and activation, the link
  detach/reattach around it, and the present-with-transaction toggle.

## Not measured

- **On-screen latency of the new path.** There is no public presented-time
  API for layer contents, and `IOSurfaceIsInUse` only tracks our own
  layer. The link targets the next refresh; whether the frame lands there
  or one later is unknown. Either is well under 41 ms.
- **A 60 Hz display.** The 11 ms release time suggests the pool stays at 2
  there; not verified.
- Direct-to-display promotion, GPU power and WindowServer memory (need
  `sudo`), transparent or blurred windows, native fullscreen, and ed's real
  scene in the test app (it only cleared).

## If this comes up again

- A window holds 3 surfaces after any burst and keeps them until it is
  resized. Dropping back to 2 when idle would save 29 MB per window but
  needs the same idle policy the trim had; it was left out on purpose.
- If latency needs proof, the missing piece is a way to timestamp when a
  layer's contents reach the screen (Metal System Trace, or a camera).
