# Terminal rendering research

Evaluated 2026-11-24 (macOS only, release build).

Question: the terminal uses a fixed-width font on a cell grid, so can text
shaping be skipped, and did keeping spaces inside text runs (d5020f798c) cost
performance?

Answer: whole-line runs are a net win; keep them. Skipping shaping would save
at most ~0.5 ms per frame while output streams and nothing otherwise, so it is
not worth a second code path today.

## How it renders

- `alacritty_terminal` is only the emulator core: pty, escape parser, cell
  grid. Alacritty's renderer (OpenGL, own glyph cache) lives in its
  application crate and cannot paint inside a GPUI element.
- `terminal_view/src/terminal_element.rs` adapts the grid to GPUI:
  `layout_grid` batches adjacent same-styled cells on a line into
  `BatchedTextRun`s, merges backgrounds into rects, and places cursor,
  selection and hyperlinks.
- Each run is painted with one `shape_line` call at
  `column * cell_width`; `force_width` snaps glyphs back onto the grid
  (`apply_force_width_to_layout`). Positions never depend on shaping.
- Shaping is there to pick glyphs, not widths:
  - system font fallback for characters the font lacks,
  - combining marks, zero-width joiners, variation selectors,
  - complex scripts.
  Ligatures are off in the terminal, so they are not a reason.
- Shaped lines are cached by run text + font + size: current frame, previous
  frame, then a 4096-entry LRU (`gpui/src/text_system/line_layout.rs`). A hit
  is one hash lookup.

## Run granularity

- Before d5020f798c blank cells were skipped, so runs broke at every space:
  one run, cache key and paint per word. This was a side effect, not a design.
- Now blanks between same-styled cells stay in the run when the run has no
  background, underline or strikethrough. A plain line is one run. `edctl wait
  --text` can match phrases because of it.

## Font coverage

Checked with `CTFontGetGlyphsForCharacters` on the installed
"JetBrainsMono Nerd Font":

| Characters | Has glyph |
|---|---|
| Latin, box drawing, Nerd icons | yes |
| CJK, kana, hangul | no |
| Emoji | no |

- CJK and emoji render today only through CoreText's automatic fallback
  during shaping. This is separate from the configurable fallback list
  (`Font.fallbacks`), which was always `None` and has been removed.
- A merged font such as JetBrainsLxgwNerdMono (not inspected) would cover
  common CJK, but not colour emoji (Apple Color Emoji) or rare characters.
- So dropping the shaper entirely always loses emoji and combining
  sequences, whichever font is used.

## Measurement

Workload: a script streams 20,000 lines into the terminal, 10 lines every
5 ms (~12 s, ~1,500 frames). Window 1400x900, empty project, measured with
`edctl perf-reset` / `perf`. One clean run per variant; per-word numbers
repeated within 1% over two rounds.

| Workload | Instructions per frame, per-word -> whole-line | Mean paint per frame | Lines shaped |
|---|---|---|---|
| Cargo-style build log (styled prefix, unique tail) | 10.0M -> 7.6M (-24%) | 1.30 -> 0.90 ms | 32,200 -> 16,000 |
| Test output (`test x ... ok`) | 6.9M -> 6.85M | 0.91 -> 0.91 ms | 16,000 -> 15,900 |
| Random prose, 12 words from a 1,500-word vocabulary | 6.8M -> 7.8M (+15%) | 0.92 -> 1.00 ms | 11,000 -> 18,000 |
| Same line repeated (all cache hits) | 5.3M -> 4.6M (-13%) | 0.52 -> 0.37 ms | ~0 |

- Random prose is the case built to favour per-word caching (repeated words
  across different lines); it costs under 0.1 ms per frame more.
- A whole frame draws in ~1.7-1.8 ms either way, against 8.3 ms at 120 Hz.
- Shaping new lines costs ~0.5-0.6 ms per frame over the static case, about
  50 us per new line. That is the ceiling for any no-shaping fast path.

Measuring notes:

- macOS throttles a covered window; runs with far fewer than ~1,500 frames
  were discarded.
- Right after `edctl launch`, `terminal_panel::ToggleShell` can be ignored;
  retry until `terminal-view` holds focus.
- Launch on an empty folder: `--fresh` on a real project still restores its
  open tabs.

## Option if it ever matters: fast path without visual trade-off

Per run:

1. If every character has a glyph in the font (cmap lookup, cacheable per
   char) and the run has no zero-width characters, emit glyph ids directly on
   the grid.
2. Otherwise shape as today.

- Renders identically with any font; a CJK-inclusive font only makes more
  runs take the fast path.
- Gains only on cache misses (streaming output). Cached lines are unchanged.
- Costs a second code path; the shaper path stays.
- Terminal only: the editor's buffer font has ligatures on and needs shaping.

Revisit if profiles of fast terminal output show shaping as a large share of
frame time, e.g. much larger grids or higher output rates than tested here.
