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

## Layout nodes

- Taffy nodes are kept between frames (from upstream #67222), so Taffy's own
  cache answers for subtrees whose style, children and measure function did
  not change. A node is matched by the hash of the element id path it is
  requested under plus its request order there; elements without ids are
  told apart only by order under the nearest ancestor with one. Matching only
  decides how much cache survives: style and children are always brought up
  to date. Only the layout fields of `Style` are compared (`LayoutStyle`); a
  field that affects layout must be listed there.
- Eviction goes by id path. Under a path with requests in the frame, the
  nodes left unrequested are detached at once (the tree there changed) and
  offered for reuse during the next frame, then removed. Under a path with no
  requests at all, nodes stay for 240 frames (`UNREQUESTED_FRAMES`): a cached
  view requests nothing while it is clean, and finds its tree intact when it
  renders again. Upstream evicts everything after one unrequested frame,
  which threw away the project panel, tab bar and status bar between the
  frames they re-render in while typing. A cached view's placeholder node is
  requested under an id path of its own (`cached-view`) so that it doesn't
  count as a request under the view's path.
- A plain measure function is dropped in the first frame its path goes
  unrequested, since it can hold on to anything (the auto-height editor's
  holds its entity). Retained ones stay with their nodes, so they must not
  capture handles whose lifetime matters.
- A tree is skipped whole when its root was laid out as a root the last time
  it was requested, in the same available space and no other in that frame,
  and nothing under it was invalidated since. This is the common case: a
  frame re-renders a few views, each its own root, and most of them come out
  the same. A root laid out in two different spaces in a frame (editor blocks)
  is laid out again the next time: its text answered the later space from the
  earlier one's layout.
- A scale factor change (window moved to another display) drops every node
  (`TaffyLayoutEngine::reset`): cached sizes are in device pixels, and a
  style can convert to the same Taffy style at two scales. It is rare, so
  nothing finer is attempted.
- A window root stretched to the viewport is only given a new style when the
  stretched style differs, so it stays valid from frame to frame.
- Measured nodes: `Window::request_measured_layout` replaces the measure
  function and invalidates the node (and its ancestors) every frame.
  `request_retained_measured_layout` keeps last frame's function when the
  element says it measures the same (`retain` compares a `MeasureState`).
  Text compares text, runs, text style, font size and line height; `list`
  and `uniform_list` compare what they are sized from. The auto-height
  editor is not retained.
- A measure function that leaves what the element paints in its state (text
  keeps its shaped lines; `MeasureState::RESETS`) depends on the sizes Taffy
  asked for and their order. Such state is only carried into the next frame
  when the whole tree is skipped; a tree that is laid out again resets it
  and measures as a rebuilt tree would. Text takes over last frame's lines by
  moving them into the new element's `TextLayout`, so handles cloned from the
  element before layout keep working.
- Accepted differences from a tree rebuilt every frame:
  - Bounds of nodes below a `display: none` node are whatever they were last;
    elements don't draw children there.
  - A root laid out in one space last frame and in that space and then
    another in this frame measures its text from empty state for the second
    space only.
  - A font added after text was laid out does not invalidate retained text
    (nothing adds fonts at runtime).
- Unit tests drive the engine without a window (`TaffyLayoutEngine<M>` with a
  test measure function), including a randomized run that compares bounds and
  text state against an engine rebuilt every frame. Any change to matching,
  eviction, skipping or measure retention must keep it passing.

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
- A notify during a draw marks the view dirty for the next frame only. A
  parent that finds out in its own prepaint that a cached child must render
  again calls `Window::invalidate_cached_view` before prepainting the child
  (the editor does this for its minimap).
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
