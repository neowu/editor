# Theme

## Verifying

- `test/theme_test` paints both appearances side by side from hardcoded state
  (hand-tagged code, fills, list and popup states, the ANSI grid, the contrast
  ladder) with theme colours only: no project, parsing or language server.
  `cargo run -p theme_test` opens it; `cargo run -p theme_test -- <file.png>`
  saves a screenshot and exits. Check palette changes there, and add a sample
  to it when a new colour role needs one.
- Tune a colour by editing the palette in `theme::built_in`, running
  `theme_test`, and reading the ladder's APCA numbers; the palette tests hold
  the rules below, so a change that breaks one fails `cargo test -p theme`.

## Contrast and roles

- The role of a colour decides its shade, and both appearances keep the same
  text ladder on the card, each step clearly weaker than the one before: text,
  code, muted, comments and punctuation, inlay hints, line numbers. Comments
  stay under two thirds of the contrast of code and above
  `theme::contrast::MINIMUM_CONTRAST`. The light dim steps fade towards the
  card instead of lightening, so a light comment is about 3:1 in WCAG terms:
  matching the dark hierarchy was chosen over WCAG AA for comments.
- Contrast is measured in APCA everywhere (`theme::contrast`): the palette
  tests, and the runtime fix that recolours text below `MINIMUM_CONTRAST` (45)
  in the editor, the Markdown preview and the terminal. WCAG 2 ratios overrate
  dark themes, so one ratio gave a soft dark comment and a loud light one.
- Fills behind code are roles too, tuned per appearance so code over them
  needs no recolouring: search matches keep the pastel Nord yellow and red at
  a per-appearance opacity; occurrences of the symbol under the cursor and the
  matching bracket use `symbol`, between the active line and the selection in
  strength; filled-element hover uses `hover`, which is visible on the chrome
  in both appearances.
- Accepted gap: dark Nord red and deep blue are below `MINIMUM_CONTRAST` on
  the card, so the terminal lightens them while the editor shows them as they
  are.

## Themes and palette

- There are two themes, Nord (dark) and Nord Light, both built in Rust in
  `theme::built_in`, so there is no theme JSON, no refinement or fallback
  derivation, and no `theme_settings` crate. To change colours, edit
  `theme::built_in` and rebuild.
- The theme follows the OS light/dark appearance and nothing else: there is no
  setting or toggle. `ed` syncs it at startup, when a workspace window opens,
  and when a window reports an appearance change
  (`Window::observe_appearance`), then refreshes every window. Native window
  chrome is not overridden, so it follows the OS too.
- `theme::built_in` is the single origin of every colour, in three layers:
  raw hex values (the Nord palette, plus the light inks), a `Palette` of role
  colours per appearance derived from them by formula (`shade` scales RGB
  channels, `mix` flattens a translucent colour onto a surface, alpha via
  `Hsla::alpha`), and theme fields that reference roles only, through
  constructors shared by both appearances. No theme field holds a colour from
  outside the palette (except the macOS traffic lights).
- Both appearances follow the same rules. The card is the far end of the
  lightness scale from the text, with the chrome one step in: darker than the
  chrome when dark, lighter when light. Popups are the lightest surface.
  Borders, hovers and selected fills step from the chrome towards the text.
- Light inks: the pastel Nord accents are too pale for text on a light
  surface, and shading them only greys them, so the light palette has its own
  accents, the Nord hues deepened to read as text on the light card (APCA 65)
  and chrome (APCA 55).
- Each theme is one lazily built `static`: `built_in::theme_for(appearance)`
  returns it, and `theme::theme()` (re-exported `built_in::theme()`) returns the
  active one as `&'static Theme` with no context argument, picked by one
  process-wide atomic flag
  (`built_in::set_appearance`). There is no theme global, no `theme::init`,
  and no `Arc` around the theme, the syntax theme or the accents (a fixed
  array). Tests cannot install another theme.
- Both syntax themes list the same captures in the same order, so a
  `HighlightId` means the same capture in either and highlight maps are
  resolved once from `theme::theme()`, when a grammar loads its highlights
  query (and lazily for the fallback highlighter), never on a switch. Nothing
  hands the theme to the language registry, and cached highlight runs are
  never checked against the map. A `&'static SyntaxTheme` held across
  frames is the theme of one appearance, so holders re-read it when rendering.
- Colours resolved outside rendering are refreshed lazily, by whoever holds
  them noticing the switch when it next renders, not by a theme-changed event:
  an editor styled with another theme drops its minimap rows and scrollbar
  markers and re-resolves semantic tokens, LSP document symbols, the matching
  bracket and spell-check highlights; mermaid diagrams re-render. Accepted
  gaps, which keep the old colours until their next update: tree-sitter
  outlines and search excerpts in the outline panel, the commit details ref
  colour, and an open diagnostic hover.
- `WindowAppearance` is `Light` or `Dark` only. macOS vibrant and
  high-contrast appearances map onto them by name.
- Terminal bright / dim ANSI colours are the normal colour shaded away from /
  towards the card: +20% / -30% when dark, -20% / +35% when light. Light ANSI
  white and bright white are greys that stay readable on the light card;
  bright white, which programs use for emphasis, is the darker one.
- `StatusColors` only has the fields the UI reads; unused `_background` /
  `_border` variants and `predictive` are gone.
- There are no Vim/Helix theme colours (`vim_*`).
- There is no theme registry, theme picker, light/dark mode toggle, `Settings`
  app-menu submenu, or theme settings (`theme`, `markdown_preview.theme`,
  `theme_overrides`, `experimental.theme_overrides`).
