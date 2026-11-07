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

## Animations

- `gpui::spring` holds only the shared motion math; there is no generic
  animated-element layer. Each caller owns its state and frame loop (advance in
  prepaint, `request_animation_frame` while moving, return early once
  settled), which keeps per-frame work specific and skippable.
- `Decay` is friction-style deceleration (exponential decay, no overshoot,
  like a flung scroll) and is the feel for surfaces moving: pane motion and
  dock open/close use `motion::GLIDE`. Callers compute `remaining(dt)` once per
  frame and apply it to every value.
- `SpringConfig` (exact closed-form steps for any damping ratio, frame-rate
  independent) is for motion that must keep velocity across retargets: the
  cursor trail (critically damped, `from_canonical(ω, 1)`).
- Animation must not cost layout: animate a cheap overlay or paint-only value
  over the final layout rather than sizes that invalidate cached views (their
  cache key includes bounds). Dragging is never animated. A dock or pane
  animation frame costs about one cursor-move frame (~3–5 ms, no shaping).
- Rotating spinners (`CommonAnimationExt::with_rotate_animation`) schedule
  their own redraws at up to 10 Hz (`Animation::with_max_fps`) instead of every
  vsync. Other updates can still render them sooner; phase follows the clock.
  The status bar shows one while a language server reports progress, which
  can last minutes; at display rate it redrew the whole window ~115 times a
  second.
