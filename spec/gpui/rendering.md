# Rendering cost

## Line layout cache

- `LineLayoutCache` (per window) keeps shaped lines for the current and
  previous frame, as upstream. Cached views rely on the frame maps' order
  (`layout_index` / `reuse_layouts` / `truncate_layouts`), so they are unchanged.
- Behind them, unwrapped lines that leave both frames go into a bounded LRU
  (4096 lines; a count, not a byte cap: a few MB for typical code, more for
  long lines), checked before calling the platform shaper. Scrolling
  back over text shaped earlier shapes nothing. CoreText shaping costs about
  3M instructions per line, so a repeated scroll drops from ~63M to ~35M
  instructions per frame.
- Wrapped lines aren't kept in the LRU: when one retires, its unwrapped line
  is put back in the LRU (wrapped hits don't refresh it), and recomputing
  wrap boundaries doesn't shape.
- Wrapped keys include `max_lines` (line clamp); unwrapped keys don't, so one
  shape serves every clamp and width.
- A font database change clears all three tiers.

## Font metrics

- The text system keeps each font's `FontMetrics` for the life of the app.
  The platform fills in the width and advance of `m` and `0` (font units,
  0 when the font lacks the glyph) with the rest of the metrics, so
  `em_width`, `em_advance`, `ch_width` and `ch_advance` scale a stored value
  by the font size and cannot fail. A font size change needs no invalidation.

## Animations

- `gpui::spring` holds only the shared motion math; there is no generic
  animated-element layer. Each caller owns its state and frame loop (advance in
  prepaint, `request_animation_frame` while moving, return early once
  settled), which keeps per-frame work specific and skippable.
- Motion timing is global, like a theme: `gpui::motion::BASE` (seconds to
  cover ~95% of the distance, 0.12s) is the one knob; every animation derives
  its duration from it by a ratio. Surfaces (panes, docks) use `BASE`;
  navigation that follows each keystroke (cursor trail, smooth scroll) uses
  `NAVIGATION` = 0.75 × `BASE`, faster so input never feels like it waits.
  Curves stay per use; `Decay::settling_in` and `SpringConfig::critically_damped`
  convert a settle time into each curve's parameter.
- `Decay` is friction-style deceleration (exponential decay, no overshoot,
  like a flung scroll) and is the feel for surfaces moving: pane motion and
  dock open/close use `motion::GLIDE`. Callers compute `remaining(dt)` once per
  frame and apply it to every value.
- `SpringConfig` (exact closed-form steps for any damping ratio, frame-rate
  independent) is for motion that must keep velocity across retargets: the
  cursor trail (critically damped, `critically_damped(settle_time)`).
- Animation must not cost layout: animate a cheap overlay or paint-only value
  over the final layout rather than sizes that invalidate cached views (their
  cache key includes bounds). Dragging is never animated. A dock or pane
  animation frame costs about one cursor-move frame (~3–5 ms, no shaping).
- Cached views nest: when a cached view re-renders, a clean cached view
  inside it is still reused (upstream re-renders everything inside). A view's
  recorded output is only replayed in the frame right after the one that
  recorded it (`ViewElementState::frame_number`), because a view inside a
  reused view isn't visited and its recorded ranges go stale; it then renders
  once and is cacheable again. `Window::refresh` still redraws every view.
- Rotating spinners (`CommonAnimationExt::with_rotate_animation`,
  `Svg::with_rotation_period`) never re-render a view. The SVG is painted once
  as a sprite tagged with a scene animation (kept across cached-view replay);
  before each present the window turns those sprites to the shared animation
  clock, and while the scene has any it presents again every 100 ms
  (`SPRITE_ANIMATION_FRAME_INTERVAL`, a timer, not vsync ticks). Other draws
  still advance them sooner. The status bar shows one while a language server
  reports progress, which can last minutes. Only rotation is animated this
  way; upstream's general `TimedTransformation` (easings, alpha, scale) isn't
  ported. `Animation` has no frame-rate cap any more.
