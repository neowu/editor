# Pane motion review

Reviewed the current uncommitted changes, including `pane_motion.rs`. Focus: performance, efficiency, and simplicity.

The final-layout-plus-overlay approach is a sensible tradeoff: it avoids continuously resizing editor/terminal content, at the cost of showing flat colour instead of moving content outside its final bounds. Keep that design. The main improvement is to make motion bookkeeping cheap when nothing changes, then reduce work during animation. The findings below are source-level observations, not measured frame-time regressions.

## Findings and improvements

### [P2] Add an unchanged-layout fast path and retain recording storage

Location: `crates/workspace/src/pane_motion.rs:141–185`, `214–222`.

Every time the group is prepainted, `update` takes both vectors, checks every previous track against the recorded panes, searches previous tracks again for each pane, reconstructs the track vector, and checks all four springs. `render_overlay` then calculates an overlay even when `update` returned false. This work also runs on ordinary workspace redraws after motion has settled. Identity reconciliation is quadratic in pane count, despite most frames having identical IDs and targets. Taking and consuming `recorded` also discards its reusable capacity.

Compare recorded IDs and bounds with the existing tracks first. When they match, preserve the tracks; if motion is settled, clear the recording buffer and return immediately. When only springs remain active, advance the existing tracks without rebuilding them. Reconcile membership only when layout actually changes. Retain the recording buffer's capacity and skip overlay construction whenever all tracks have snapped or settled.

This needs only a linear comparison and the existing active-state information (`last_frame`), not a hash map or new invalidation framework. Preserve the pointer/region snap rules on the active path. Add tests for repeated unchanged frames, active motion with unchanged targets, and a changed target after settling.

### [P3] Share the spring propagator across all pane edges in a frame

Location: `crates/workspace/src/pane_motion.rs:63–69`; `crates/gpui/src/spring.rs:64–103`.

During animation, each track calls `SPRING.step` four times. Each call constructs the same propagator for the same configuration and elapsed time. With the underdamped JELLY preset this repeats exponential and trigonometric calculations for every edge, including stationary edges. `SpringConfig::propagator` already exists specifically for sharing this calculation.

Compute the matrix once per active group frame and apply it to each edge's displacement and velocity. Precompute the velocity settling threshold for the fixed spring as well. Keep the existing exact integration and retarget behavior; there is no need to approximate the spring or reduce frame rate. Test the batched calculation against `SpringConfig::step`, including interrupted motion.

The redundant work is clear, but its wall-clock impact needs measurement. Prioritize the idle path above this optimization.

### [P3] Stop clipping when no ghost pieces remain and reuse scratch storage

Location: `crates/workspace/src/pane_motion.rs:189–205`.

The ghost loop visits every higher track even when subtraction produced no pieces, or an earlier track already occluded them all. Settled tracks start with no pieces but still enter this loop. During active motion, converting the inline `SmallVec` into a `Vec` and repeatedly collecting clipping results also creates avoidable temporary-buffer work.

Immediately skip tracks with no ghost pieces and break when clipping empties the list. Keep the initial pieces inline, or use reusable scratch buffers for clipping. Cache each track's presented rectangle once for the frame if profiling shows this loop matters. Preserve all tracks as potential occluders: a stationary pane can still cover a moving ghost.

The early exits are simple and worthwhile. Only add buffer-management machinery if allocation measurements justify it for realistic pane counts; avoid a spatial index for the usual small number of panes.

### [P2] Size dock overlays from actual layout bounds

Location: `crates/workspace/src/mode/editor.rs:218–221`.

The transition uses the stored/default panel size, but `render_dock` explicitly allows horizontal docks to flex-shrink and applies a minimum width. In a constrained window with both side docks open, the actual dock width can differ from the stored size. The opening curtain then extends into the center editor, and the closing ghost starts at the wrong width. This is more than the intended small spring overshoot.

Record the laid-out dock bounds and retain the last visible extent for closing. Use the actual extent for the overlay. This preserves the cheap single-quad design without changing pane layout during animation. Add a constrained-width test with both docks visible; the current span tests only exercise an already-correct size argument.

## Suggested implementation order

1. Add the idle/unchanged-layout path and keep recording capacity.
2. Add empty-piece early exits.
3. Share the spring matrix across edges.
4. Correct dock overlay sizing.
5. Measure before adding further caching or abstractions.

For an A/B comparison, use visible release builds and `edctl perf-reset` / `perf`. Compare a settled workspace under repeated cursor movement, then split/close/swap workloads with 1, 4, and 8 panes. Report instructions and build/paint cost per frame as well as total frames. Check shaped-line counts to verify pane content remains cached after the initial layout change. The new specification's approximately 3–5 ms/frame claim was not independently verified in this review.

## Validation

Passed:

- `cargo test -p workspace pane_motion --lib --offline`: 7 tests.
- `cargo test -p workspace mode::editor::tests --lib --offline`: 4 tests.
- `cargo test -p gpui spring::tests --lib --offline`: 4 tests.
- `cargo test -p editor cursor_animation::tests --lib --offline`: 4 tests.
- `cargo test -p control --offline`: 4 tests across library and CLI.

UI/performance validation is incomplete. The initial sandboxed launch failed; an approved `edctl launch --fresh` then opened the existing release binary. Socket access from the sandbox failed, and the escalation request to inspect/drive that instance was rejected. No live screenshots or performance samples were obtained, and the release binary was not rebuilt or verified against the exact current source. The fresh instance (PID 3064) was not closed because further control access was unavailable.

Only this review document was added. Application code and specifications were not changed.
