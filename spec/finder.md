# Finder

- One finder modal (`cmd-p`) searches files, project symbols, or open tabs.
  A bar at its bottom shows the three modes with their keys (`cmd-1/2/3`,
  also clickable). There is no separate symbol picker or tab switcher, and no
  `cmd-t` / `ctrl-tab`.
- Switching mode keeps the query and the window's frame: every mode has the
  same width, a results area fixed at the picker's max height (it doesn't
  shrink to fit), and one-line rows (a symbol's location sits beside its
  label). The finder always opens in Files; Symbols and Tabs are built when
  first shown.
- Tabs lists the tabs of all editor panes, most recently used first, with the
  previous tab preselected (`cmd-p cmd-3 enter` jumps back). Moving the
  selection doesn't activate tabs; only confirming does. `ctrl-backspace`
  closes the selected tab.

- History (recently opened files, the current file, and selected matches)
  only lists paths that are still files. They are checked when the finder
  opens and again on every worktree update while it is open, so a file deleted
  meanwhile drops out without reopening the finder. Paths outside the worktree
  are checked on disk.
- Path ranking (`fuzzy`) adds a small bonus for matches near the end of
  the path, so a path that repeats the query (`x/x.py` for `x`) ranks above one
  matching only once (`y/x.py`).
- All fuzzy matching (pickers, completions, outline, file finder) goes through
  one `fuzzy` crate built on `nucleo-matcher`; there is no second in-house
  matcher. Consequences: whitespace splits the query into terms matched
  independently in any order; smart case only downranks a case mismatch (0.9
  per character), never drops it; the length penalty is a mild tie-break; and
  accents are ignored (`cafe` matches `café`).
- Pickers are a plain results list: there is no preview pane (the
  `picker_preview` crate is gone), no Actions… footer menu, and no drag
  resizing. A picker has a fixed width and a max height it shrinks below to
  fit its content.
- The file finder only opens files (`enter`, `OpenWithoutDismiss`, and
  multi-select); it has no open-in-split actions. The text finder keeps its
  split keys (`cmd-h/j/k/l`).
- An absolute path of an existing file is a query too. A file inside a visible
  worktree is listed as that worktree's file; any other file (outside the
  project, or in a hidden single-file worktree) is listed first by its
  absolute path and opens without adding a worktree to the project, like a
  history entry from outside it. Fuzzy search is skipped when the path
  resolves. Upstream's related handling of excluded files and `a/`/`b/` diff
  prefixes for excluded directories isn't ported.
