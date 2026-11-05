# Configuration

## Decision

Settings are constructed entirely in Rust as one complete `SettingsContent`
object shared by every project. There is no default-settings JSON or separate
user/default settings layer. Language metadata supplies language conventions;
explicit per-language values in code take precedence. Keymaps and snippets are
also constructed in Rust, including the personal bindings and snippets.
Changing persistent settings, shortcuts, or snippets requires editing code and
rebuilding. There is no settings UI, keymap editor, snippet UI, settings/keymap
migrator, or Vim/Helix mode.

## Behaviour

- There is no database (no SQLite, key-value store, or `db`/`sqlez`/`session`
  crates). App state lives in two JSON files:
  - `~/.local/share/ed/projects.json`: recent projects (root folders in open
    order, plus the SSH host/user/port for remote ones), most recent first and
    capped at 100, each flagged `open` while it has a window (the open list
    is the recents, not a separate list). Window bounds are not saved. It is
    read once at launch; unreadable contents count as empty. Every change
    rewrites the whole file. A project moves to the front of the recents when
    opened or its window is focused.
    There is no separate list of remote servers: the Remote Servers modal
    lists the servers of the recent remote projects (most recently used
    first, each with its recent folders and Open Folder). Connecting a new
    server goes straight to its folder picker, and the server is remembered
    once a project on it opens. Removing a server drops its recent projects.
    There are no per-server nicknames, ssh args or port forwards.
  - `<first root folder>/.ed/workspace.json`, per project: the pane tree with
    each tab's kind and its own JSON state, pinned/active/preview tabs, dock
    visibility/active panel/zoom, and recent navigation history. Remote
    projects keep it on the server, read through the project's buffers and
    written through the worktree after connecting. Unreadable files are
    ignored.
    Identical contents aren't rewritten. Windows with no root folder aren't
    persisted.
- Tabs persist only their own state. An editor tab on a file keeps only its
  path: on restore the file is read from disk, and nothing else (scroll,
  selections, folds, unsaved edits) is kept, so editing or scrolling a file
  doesn't rewrite `workspace.json`. Untitled buffers aren't persisted at all.
  Image and markdown-preview tabs keep their path (and preview mode), branch
  diffs their base, and other git diff tabs nothing. Tabs on files inside a
  root folder's `.ed/` aren't persisted at all: it's project config, and
  recording a tab on `workspace.json` would rewrite the file it shows.
- No hot exit: unsaved edits are never persisted. Closing a window (with or
  without a root folder) or quitting prompts to save its dirty buffers,
  untitled ones included.
- Restore on startup (not configurable) reopens the projects flagged open at
  quit (closing a window clears its flag; quitting keeps it; removing a
  project from the recents drops it), most recent on top, falling back to the
  most recent project whose folders still exist. If nothing is restored, a
  window opens with an empty buffer.
- Autosave (not configurable) saves a buffer when focus leaves it and when the
  window deactivates, formatting on save; closing a dirty, autosavable buffer
  saves it instead of prompting. Reopening a file in the same pane restores
  its selections, folds and scroll position.
- Not persisted: panel sizes, window bounds (every new window, restored
  ones included, is created filling the visible area of its display so all
  windows match; not zoomed, since macOS zooms a not-yet-shown window against
  the wrong screen. The display is ed's frontmost window's, which works even
  when ed isn't the active app, else the screen the user is on (macOS
  `NSScreen.mainScreen`, gpui `App::main_display`)), dock layout for new
  windows, native (macOS) window state, the Text Finder's last query and
  filters, git panel commit drafts/signoff/commit-editor collapse, outline panel
  state, and the registry of git worktrees ed created.
- There is no restricted mode or worktree trust: every opened project is
  trusted, so its project settings apply and its language servers start
  without a prompt.
- Single-instance checks are bypassed only in debug builds.
- Settings types use ordinary Serde derives without optional-field macros or
  custom flattened-settings parsing. Ordinary optional fields reject invalid
  values and remain `None` when missing. Settings JSON error recovery and
  field-path diagnostics are removed. Bundled semantic-token rules use ordinary
  Serde errors; callers identify the source file when reporting failures.
- There is one settings layer, so settings types have no merge trait or derive
  macros (`settings_macros` is removed). The only merge is per language:
  language defaults, then language metadata (tab size, hard tabs, soft wrap,
  auto-indent on paste), then the built-in per-language overrides, each overlaid
  field by field (nested groups such as `completions` too). Lists and maps are
  plain values that replace; there is no `"..."` splicing or extend-only list
  type. Typed settings register with `settings::register_setting!(Type)`.
- User, global, server, and `.ed/settings.json` files are not loaded, watched,
  created, or rewritten. There are no settings profiles or project overrides.
  Changing persistent preferences requires editing the built-in settings and
  rebuilding. Existing settings controls affect only the running session.
- Built-in preferences preserve the applicable values from the personal Zed
  configuration, including language overrides and Rust language-server options.
  Entries for removed features are omitted. Preferences that never varied are
  removed as settings and hardcoded:
  - title bar: no expanded menu bar, no open-on-hover menus; the project
    name, worktree and branch pickers, and the branch change-status icon are
    always shown; there are no title bar settings;
  - terminal: no title breadcrumbs, no count badge, no bell sound (the tab
    still marks the bell), does not open on startup, always docked at the
    bottom (no `dock` or `default_width` setting);
  - git panel: no file icons, no count badge, does not open on startup;
    there are no git panel settings (see `git.md`);
  - git: there are no `git` settings (see `git.md`);
  - project panel: folder labels are not bold; there are no project panel
    settings (see `project-panel.md`);
  - status bar: no active file name item; the cursor position item always
    uses the long selection format (`2 selections, 3 lines, 12 characters`);
  - outline panel: docked right only, 360px default width, 20px indent with
    indent guides, file and folder icons (no folder chevrons), single-child
    directories auto-fold, the active entry is always revealed, outlines are
    not collapsed by default depth; there are no outline panel settings;
  - file finder: file icons, width `window − 768px` (at least the default
    modal width), the active file is skipped as the first history match unless
    the query has a position, ignored files follow the worktree unless toggled,
    and the query is pre-filled from the focused selection; file icons also
    show in the open-path prompt and language selector;
  - tab switcher: previews the selected tab while cycling;
  - diagnostics: status bar button shown; LSP pull diagnostics on with a 50ms
    debounce; inline diagnostics show errors only, 150ms debounce, 4 columns
    after the line end;
  - image viewer: file sizes in binary units (KiB, MiB);
  - mouse cursor hides on typing and on keystrokes that resolve to an action;
  - workspace: a panel's focus toggle never closes the panel; the window title
    is fixed to `<project names> — <active file name>` (no title template, so
    no settings or Git branch subscriptions drive title updates);
  - no accessible mode.
- Markdown preview preferences are hardcoded too: UI font family and size for
  text, buffer font family for code, semibold (600) headings, content centered
  and capped at 1200px, and no auto-opening of markdown files in the preview.
  Preview zoom (cmd +/-/0) is in-memory only.
- Fonts and font sizes are not settings; see "Fonts" in `ui-layout.md`. There
  are no theme settings: buffer line height (1.618) and unnecessary-code fade
  (0.3) are constants in `theme::built_in`.
- Semantic token fallback mappings are constructed in Rust. Personal rules take
  precedence over language/extension rules, which take precedence over fallback
  mappings. Rule order and theme-style fallback order are preserved. There is no
  bundled semantic-token settings JSON or default-rules viewer.
- Settings-file actions, shortcuts, menu entries, and welcome/error links are
  removed, along with the project/server settings commands and profile selector.
  The default-settings view is removed too.
- User and bundled keymap JSON files are removed from the loading path. There is
  no keymap watcher, file editor/viewer action, base-keymap selection, or keymap
  JSON schema. Task files are not supported (see `tasks.md`).
- EditorConfig and file modelines (vim/emacs `vim:`/`-*-` comments) are not
  supported, so language settings come only from built-in settings. There is no
  project-local settings layer or worktree-settings sync with remote peers.
  Settings lookups are global only (`Settings::get_global`); there is no
  per-file/worktree location argument, and the remote terminal-shell request
  carries no worktree id.
- The built-in keymap retains contexts and binding order. Only the US/ABC QWERTY
  layout is supported: macOS has no per-layout key-equivalent remapping, so
  shortcuts always bind to the literal key. The keymap, menu bar, and dock menu
  are built once at startup.
- Keybindings should be simple single keystrokes. There is no UI for pending
  multi-stroke input (no which-key popup or status bar indicator).
- Personal bindings are applied last: global `cmd-[`/`cmd-]` navigate backward/
  forward, Editor `f1` opens documentation and `cmd-shift-u` toggles case, and
  Terminal `shift-enter` sends Escape followed by carriage return.
- Settings are not synced to remote servers. Client and remote server are built
  from the same code and both start from the built-in settings; runtime setting
  changes only affect client-side UI (docks, font sizes, theme, panel
  sorting/filters), so no server-side behaviour can diverge. Remote settings
  files are not loaded. Any new runtime toggle for a server-side setting
  (formatting, language servers, scanning, git) would need sync reintroduced.
- JSON schemas are not generated: nothing derives `JsonSchema` and `schemars`
  is not a dependency. Actions still derive `Deserialize`, because the built-in
  keymap builds actions from a name plus JSON input. Bindings may omit input,
  so an action whose fields have non-`false` defaults uses struct-level
  `#[serde(default)]` with a hand-written `Default`; `Default::default()` and
  deserializing `{}` give the same action. Built-in data that is never omitted
  (settings, task templates, language `config.toml`) states every such field
  explicitly instead of relying on serde defaults. The JSON language server
  only gets the bundled `tsconfig.json`/`package.json` schemas and a
  trailing-comma schema for JSONC files; there are no settings, keymap, or
  per-action schemas, and no language-server configuration schema probing.
- The upstream documentation website preprocessor is removed; this fork does not
  build the upstream mdBook site.
- The command palette has no "Add/Change Keybinding" flow; secondary confirm runs
  the selected command. It lists every available action as-is: there is no
  global action filter or query interceptor (`command_palette_hooks` is removed).
- The command palette keeps no persisted state: commands are listed
  alphabetically with no usage-based ranking, there is no query history
  (up/down only moves the selection), and no `command_palette` setting.
- Pickers keep no persisted state: every picker opens with the default
  preview layout and size, and a resize or preview-layout change lasts only
  until it closes (switching layouts resets to that layout's default size).
  `PickerDelegate` has no `name()` key.
- Terminals keep no persisted state: neither terminal-panel terminals
  (and their splits) nor terminal tabs in the center are restored after a
  restart, and a terminal's custom title and working directory are not saved.
  The terminal panel opens a fresh shell when it is first shown empty.
- App-level actions (quit, about, font size, `NoAction`/`Unbind`, ...)
  use the `ed` namespace (`ed::Quit`, shown as "ed: quit"), not upstream's
  `zed`. Keymaps using `zed::*` names do not resolve.
- Actions resolve only by their current names. Deprecated aliases and action
  deprecation metadata are removed; built-in bindings use the current names.
- Snippets are built in (`snippet` crate): Rust has `main`, `async_main`,
  `tests`, `test`, and `async_test`. Language snippets are offered with global
  ones and do not require a project. The user snippets directory is not loaded or
  watched, and there is no snippet picker/configure action or snippets JSON schema.
- `vim_mode`, `helix_mode`, and the `vim` settings section do not exist. There is
  no Vim keymap layer, mode indicator, Vim/Helix toggles (quick action bar), or Vim-style keybinding notation.
- Journaling is removed: no journal entry command or `journal` settings section.
- Onboarding is removed: no first-launch onboarding page, `ed::OpenOnboarding`,
  base keymap picker, or multibuffer hint toolbar. There is no VS Code/Cursor
  settings importer (actions or the underlying translation code). The first
  launch opens an empty editor like any other launch.
- The welcome page is removed: no `ed::ShowWelcome` action, no Get Started /
  Recent Projects page, and no `welcome::OpenRecentProject` (cmd-1…5) bindings.
  Every empty pane shows the keybinding hints instead (see ui-layout.md).
- Bookmarks are removed: no bookmark actions, labels, navigation, project view,
  bookmark gutter markers, or `gutter.bookmarks` setting (the gutter hover
  button now copies the file location; see editor.md).
