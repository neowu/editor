# Shaped-line cache review

Reviewed 2026-11-07: `365a15c5f8d2fd9925d7502034f1f33f54f5adcd` — “Throttle spinners, keep shaped lines in an LRU, run all crate tests”.

Scope: the commit, line-layout lookup and frame rotation, wrapped layouts, cached-view replay, font invalidation, text shaping callers, animation scheduling, and manifest changes. Related source locations below refer to current HEAD `9ae712ee97`. Later edits to the cache mostly change imports and remove an unrelated method; the reviewed cache behavior remains present.

## Result

The unwrapped-line LRU path preserves the existing lookup keys and frame bookkeeping. I found no newly introduced wrong-text or stale-font rendering defect in that path. One confirmed performance gap prevents wrapped lines from receiving the same recency behavior. A separate, pre-existing wrapped-cache key defect is noted because it is directly relevant to this cache.

### [P2] Wrapped hits do not keep the underlying shaped line recent

Relevant changed code: `crates/gpui/src/text_system/line_layout.rs:532–535` (`finish_frame`); related hit paths at lines 555–562 (`layout_wrapped_line`).

Only entries remaining in `previous_frame.lines` enter the LRU. A wrapped cache hit returns before calling `layout_line`, so it neither promotes nor touches the underlying unwrapped entry. After the first two frames, the unwrapped line can already be in the LRU even while its wrapped layout is being used every frame. Other text can evict that entry. When the wrapped layout subsequently leaves the two frame maps, clearing `wrapped_lines` does not put its underlying layout back into the LRU.

Consequence: repeatedly used wrapped text can require shaping again after only a short absence, while less recently used unwrapped entries remain cached. This is a gap in the new optimization, not a new visual correctness regression. It contradicts the unconditional explanation that a discarded wrapped layout's unwrapped line is retained for cheap reconstruction.

**Executed probe:**

1. Shape `"hot wrapped line"` and keep requesting its wrapped layout each frame.
2. Shape one distinct unwrapped line per frame for 4096 frames, keeping the wrapped line visible throughout.
3. Request the wrapped line once more, then let its frame entries expire.
4. Request it again.

The final request incremented `shaped_line_count` by one and returned a different underlying `Arc<LineLayout>`. A temporary unit test using the existing `NoopTextSystem` fixture passed assertions for precisely this behavior. The probe was removed afterward; the production source is unchanged.

Recommendation: account for wrapped-layout use or retirement when maintaining the underlying line's recency. Keep existing `used_lines`/`used_wrapped_lines` index behavior intact. There is a tradeoff: touching the unwrapped tier on every wrapped hit adds lookup/bookkeeping cost. A retirement-based solution may be cheaper but must avoid duplicating entries already present in a frame map. Benchmark before adopting either approach; this gap is less urgent than the measured editor scrolling win.

Regression coverage: keep a wrapped line hot while churning more than the LRU capacity, let it leave view, then assert that returning does not increment the shaper count. Existing `layout_wrapped_line_reuses_recent_unwrapped_line` tests expiration without intervening cache pressure and therefore misses this case.

### [P2, pre-existing] Wrapped cache keys omit the line clamp

`crates/gpui/src/text_system/line_layout.rs:537–579` and `CacheKeyRef` at line 690.

`layout_wrapped_line` accepts `max_lines` and passes it to `compute_wrap_boundaries`, where it changes how many boundaries are produced. Neither the owned nor borrowed cache key includes it. Two requests for identical text, font runs, size, and width but different line clamps reuse whichever wrapped layout was cached first. The resulting height and wrapping can therefore depend on call order.

This is reachable through `WindowTextSystem::shape_text` and the text element's `text_style.line_clamp` (`crates/gpui/src/elements/text.rs:455`). It already exists in the reviewed commit's parent; the new LRU neither introduces nor fixes it. This finding is from source inspection, not a GUI reproduction.

Recommendation: include `max_lines` in the wrapped-layout key only. Unwrapped shapes should remain reusable across clamp and width changes. Test requests in both clamp orders, in the same frame and across consecutive frames, comparing against fresh-cache results.

## Memory and eviction tradeoffs

- **4096 entries is not a byte budget.** Retained entries own text keys, font-run arrays, and glyph vectors. The spec's “a few MB” describes typical short lines, not a bound. At approximately 32 bytes per glyph, 4096 entries averaging 1024 glyphs represent roughly 128 MiB of glyph storage alone. This is an illustrative calculation, not a measured RSS result.
- The editor passes `MAX_LINE_LEN = 1024` to its normal line-building path (`crates/editor/src/editor.rs:684`, `element.rs:2211`), which limits that workload. GPUI's general `shape_text`/`shape_line` APIs do not impose the same cap, so large labels or paragraphs can retain substantially more memory. Each window has its own cache.
- Content and style variants consume distinct slots. Editing a long line can retain multiple historical shapes until eviction. Closing a file does not clear its window's LRU; entries disappear through replacement, a font-generation reset, or destruction of the window text system.
- The cache ages entries by frame retirement. `previous_frame.lines.drain()` uses hash-map order, so eviction within one retired frame does not preserve exact call order. The existing eviction test proves that an older batch loses to a newer batch, not exact LRU order within a batch. This is a reasonable simplicity tradeoff unless measurements show it matters.

Given this repository's preference for performance and simplicity, I would keep the count cap unless a representative long-line workload shows unacceptable retention. A simple oversized-entry admission limit is an alternative to full byte accounting. Document the memory estimate as workload-dependent rather than a guarantee.

## Paths checked and why they matter

### Lookup, frames, and cached views

`WindowTextSystem::shape_line` constructs fresh decoration runs and obtains geometry through `layout_line`. The effective key is text, font size, resolved font runs, wrap width, and forced width. Color/underline/background data live outside the cached `LineLayout`, so changing decorations does not inherently reuse stale colors. Forced-width terminal geometry remains separately keyed.

Unwrapped lookup order is current frame, previous frame, LRU, platform shaper. A previous-frame or LRU hit moves the entry into the current map and records its key in `used_lines`. A current-frame hit retains the pre-existing behavior of returning without another index entry. No new index convention was introduced by this commit.

At the end of `Window::draw` (`crates/gpui/src/window.rs:2724`), `finish_frame` retires unused previous-frame unwrapped entries, swaps frame maps, and clears old wrapped entries and usage vectors. Cached-view prepaint/paint replay calls `reuse_layouts`; speculative layout rollback calls `truncate_layouts`. Those operations continue to use frame usage ranges, not LRU iteration order. The LRU is a fallback rather than a replacement for the frame maps.

This cache stores CPU text geometry, not rendered scenes or GPU glyph atlas allocations. The commit does not eliminate wrapping, decoration generation, painting, or rasterization costs merely because shaping hits the cache.

### Font and style changes

Successful `TextSystem::add_fonts` clears font resolution and increments the shared font generation (`crates/gpui/src/text_system.rs:305`). Cache entry points check it and replace the entire `LineLayoutCacheState`, clearing all three tiers. Generation-tagged old reuse/truncation indices are ignored. The new LRU participates in that reset correctly.

Font size and resolved font IDs are part of the key; wrap and forced widths are also represented where relevant. No additional invalidation requirement introduced by this commit was found. This conclusion does not claim that every OS font-change notification path has been tested.

### Spinner scheduling

`rotate_animation` adds `with_max_fps(10)` while preserving duration and shared-clock phase synchronization (`crates/ui/src/traits/animation_ext.rs:43`). Its call sites include search, Git loading UI, remote connection, and terminal loading indicators.

`AnimationElement::request_layout` (`crates/gpui/src/elements/animation.rs:395`) schedules a delayed notification instead of requesting every animation frame. The persistent `delayed_frame_pending` flag prevents repeated renders of the same mounted animation from stacking timers. Phase is still derived from elapsed time, so a delayed frame does not slow the rotation clock.

The 10 fps value limits animation-driven scheduling for one mounted animation. It is **not a strict render-rate cap**: another UI update can render the spinner before the timer expires, and its phase is recomputed then. Separate spinners have separate timers; shared phase does not coalesce their wakeups. A removed spinner can leave one detached notification pending, but that task does not itself schedule a continuing loop. These are limits of the existing scheduler, not evidence of a new runaway animation bug.

The spec should say “schedules animation-driven redraws at up to 10 Hz” rather than promising all spinner rendering is capped at 10 fps. The commit's measured indexing frame count is compatible with additional non-animation redraws.

### Test-target changes

The manifest changes remove library test/doctest/bench opt-outs and add GPUI's `lru` dependency. The reviewed reorderings do not establish another runtime behavior change. Current manifests contain no `test = false`, `doctest = false`, or `bench = false` matches. Later changes removed Rust documentation examples and the associated GPUI development dependency, so current test results must not be presented as historical validation of every newly enabled doctest in this commit.

## Validation performed

- `cargo test -p gpui --lib text_system::line_layout::tests -- --nocapture`: **10 passed**.
- Temporary focused wrapped-recency probe: **1 passed**, confirming the extra shape described above. Removed after execution; verified no remaining diff in `line_layout.rs`.
- `cargo test -p ui --lib rotate_animation_repeats_synced_and_is_throttled -- --nocapture`: **1 passed**.

These tests ran against the current checkout, not a checkout of the historical commit. The cache tests use `NoopTextSystem`; they validate cache behavior, not CoreText/HarfBuzz output or actual GPU rendering. No full workspace test run, historical doctest run, GUI automation, RSS measurement, or performance A/B was performed. Performance numbers in the commit and `plan/gpui_optimization.md` were inspected but not independently reproduced.

For follow-up implementation, run `edctl` checks per `spec/automation.md`: repeated scrolling, wrapped text under cache pressure, font/size changes, and loading indicators alongside other animations. Measure warm-return shaper counts and per-frame instructions, and include a long-line memory workload before changing the admission policy.

Only this review document is added. No implementation or design decision was changed, so `/spec` was not edited.
