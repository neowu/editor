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

- Rotating spinners (`CommonAnimationExt::with_rotate_animation`) schedule
  their own redraws at up to 10 Hz (`Animation::with_max_fps`) instead of every
  vsync. Other updates can still render them sooner; phase follows the clock.
  The status bar shows one while a language server reports progress, which
  can last minutes; at display rate it redrew the whole window ~115 times a
  second.
