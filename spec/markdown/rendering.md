# Markdown rendering

- `markdown` is the shared renderer (pulldown-cmark, selection, code
  highlighting via the language registry) used by hover
  popovers, completions docs, diagnostics, notifications, git and the preview.
  It knows nothing about diagrams: callers can take over closed fenced code
  blocks with `MarkdownElement::render_code_block_with`.
- Code blocks show a copy-on-hover button unless the caller hides it; there is
  no wrap button and no custom code-block renderer beyond that hook.
- Preview-only behaviour lives in `markdown_preview`: its style, and Mermaid
  diagrams (cache, Preview/Code tabs), plugged in through that hook.
- Only markdown files are previewed; HTML files are opened in the browser.
  Files never auto-open in the preview.
- HTML is not rendered anywhere: HTML blocks and inline tags show as their
  raw source text, except that HTML comments are hidden and `<code>` spans
  render as inline code.
- The preview is virtualized (`MarkdownElement::virtualized`): the element is
  its own scroll container (a `gpui::list`) whose items are the top-level
  blocks, so only visible blocks are built and laid out each frame. A reparse
  keeps the measured heights of blocks whose source (and footnote position)
  didn't change; edited blocks are remeasured in place so the scroll position
  holds. Selection, links and hit testing work on the visible blocks' text;
  Copy and select-all cover the whole document, with Copy's text derived
  from the parse instead of the layout. Other callers (popovers, docs) lay
  out the whole document in one element as before.
- Container nesting (blockquotes, lists, HTML blocks, ...) is capped at 64
  levels; deeper containers fold into the innermost one. GPUI layout, prepaint,
  paint and taffy recurse once per element on the main thread stack (no
  `stacksafe`), so the cap keeps pathological input from overflowing it.
- Smoke-test documents for the preview live in `test/`.
