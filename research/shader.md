# Shaders (gpui_apple / Metal)

## Build pipeline (`crates/gpui_apple/build.rs`)

1. **cbindgen → `scene.h`**: exports gpui's Rust structs (`Quad`, `Shadow`, `MonochromeSprite`, …) as C structs so the Metal shader reads instance buffers with exactly the layout Rust writes.
2. **Compile**, one of two ways:
   - `runtime_shaders` (**default**, `crates/gpui_apple/Cargo.toml`): joins `scene.h` + `src/shaders.metal` into one source, embeds it in the binary, and compiles it at renderer startup via `newLibraryWithSource` (`metal_renderer.rs`).
   - Offline: `xcrun metal` + `metallib`, embeds the `.metallib`. Needs the Xcode Metal toolchain; Command Line Tools alone don't include it.

## Pipelines (all in use — don't delete)

Each is a vertex + fragment pair. They are gpui's complete set of drawing primitives.

| Pipeline | Draws | Used by |
|---|---|---|
| `quad` | Rounded rects: background, border, gradients, patterns (SDF) | Every `div` bg/border, `linear_gradient` (gradient_fade, project_panel), `pattern_slash` (editor), `checkerboard` (image_viewer) |
| `shadow` | Gaussian-blurred box shadows | Elevation: popovers, context menus, tooltips |
| `underline` | Straight / wavy lines | Diagnostic squiggles, links, strikethrough |
| `monochrome_sprite` | Alpha-mask texture tinted with a color | All text glyphs, SVG icons |
| `polychrome_sprite` | Full-color texture | Images, emoji, image_viewer, avatars |
| `path_rasterization` + `path_sprite` | Arbitrary vector paths (MSAA texture, then composite) | Editor selection shapes (`editor/src/element.rs`), git graph, circular progress |

Only dead code found: **dashed-border** logic in `quad_fragment` (`dash_alpha`, `corner_dash_velocity`) — nothing outside gpui uses `BorderStyle::Dashed`. A never-taken branch at runtime; removing it only slightly shortens the startup compile.

## Runtime shaders: trade-offs

- **Startup cost**: ~1200 lines of MSL compiled every launch. Cold compile is roughly tens of ms to ~100 ms; macOS's Metal compiler cache makes later launches faster while the source is unchanged.
- **Errors at runtime, not build time**: a shader typo builds fine and panics at `expect("error building metal library")` on launch.
- **Binary carries the source** (~40 KB) — negligible.
- **Runtime GPU performance is the same** once compiled.

Conclusion: fine for a personal build without Xcode; the cost is the first-launch compile.

## Animation

Animation does **not** need shaders. gpui animations (`with_animation`, `Animation::new`; e.g. spinner_label, loading_label, `animation_ext` rotate) run on the CPU: each frame changes element properties (opacity, rotation, color, position) and the same 7 pipelines redraw the scene. Cursor blink (`editor/src/blink_manager.rs`) just toggles visibility on a timer; the cursor is a quad.

- **Use CPU interpolation + existing primitives** for: cursor blink, smooth cursor movement, fades, slides, smooth scrolling, spinners. A smooth-moving cursor = interpolate the cursor quad's position per frame and request the next frame (in `editor/src/element.rs`), no GPU changes.
- **A new shader is warranted only** for per-pixel effects the primitives can't express: cursor trail/glow, blur behind panels, noise/shimmer, a custom line style. Adding one touches four places: `Scene` primitive type in gpui, struct exports in `build.rs`, pipeline in `metal_renderer.rs`, shader code in `shaders.metal`.
- **Real perf cost of animation is CPU**: every animated frame re-runs layout + paint for the window. Keep it cheap with `with_max_fps` and by keeping animated elements small.
