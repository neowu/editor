# Tasks

## Decision

User-defined tasks are removed. The only way to run something from the editor
is the gutter run icon (runnables), which spawns the command in a terminal tab
("task output"). Anything else is run from a script in the terminal.

The `tasks_ui` crate is removed. The `task` crate only holds `SpawnInTerminal`
(what a terminal tab runs) and `RunnableTag`. There are no task templates or
`$ED_*` variables: each language builds `SpawnInTerminal`s directly in Rust
(`ContextProvider::runnables`), so a missing value is an explicit `None`
instead of a template silently failing to resolve.

## Behaviour

- Each language has exactly one source of run icons, with no fallback:
  - a language whose `ContextProvider::lsp_task_source` names a server (Rust:
    rust-analyzer) takes them from LSP runnables only, for the whole buffer.
    It has no `runnables.scm`. The server's runnables become
    `SpawnInTerminal`s as they arrive; ones without a location are dropped.
    Nothing shows until the server answers: the request has no timeout, and
    it is sent again when the buffer is reparsed, when the server registers
    the buffer, and when the server asks for an inlay hint refresh
    (rust-analyzer does once it has loaded the project);
  - every other language uses its tree-sitter `runnables.scm` tags. The query
    covers only the visible rows plus one screen above and below, and runs
    again when scrolling leaves that range, so runnables far from the
    viewport are unknown.
- A tree-sitter runnable with any tag shows an icon. Its tasks are built only
  when the icon is clicked: `ContextProvider::runnables` gets a `RunContext`
  (tag, the match's captures, file path, worktree root, innermost outline
  symbol) once per tag, and the results are concatenated in tag order. The
  first task is what a left click launches. A runnable whose language builds
  nothing (e.g. a JS test with no detected test runner) shows an icon that
  does nothing.
- Only the buffer's own language contributes tree-sitter runnables; code
  injected into it (e.g. a markdown code block) does not.
- Markdown fenced code blocks tagged `sh`/`shell`, `bash`, `zsh` or `fish` get
  a run icon on the opening fence that runs the block's code with that shell
  (`<shell> -c`), from the markdown file's folder. The code reaches the shell
  through the `CODE_BLOCK` env var, so the outer task shell never re-parses it.
- Clicking the icon is the only way to run one (there is no keyboard action
  for the nearest task). The icon shows running / passed / failed state.
- In a single-file worktree (a file opened without its folder) there is no
  worktree root; languages that need it (Python, JS tests) build nothing.
- Each language picks its task's cwd: file-based tools (go, package/composer
  scripts, markdown blocks) run from the file's folder so the tool finds the
  nearest manifest; Python and bash scripts run from the worktree root. The
  task env holds only the task's own values (e.g. `CODE_BLOCK`, LSP runnable
  env); the terminal adds the directory environment.
- A task always runs in the terminal dock with the default system shell, saves nothing first, and leaves its tab
  open with no summary lines when it finishes. A task's full label identifies
  its tab: running a task whose tab already exists waits for that run to
  finish, then replaces the tab's output.
- A task terminal tab has a rerun button (and `terminal::RerunTask`) that
  respawns that tab's own command in place; there is no global rerun. The tab
  keeps the task as requested; wrapping it in the shell happens on each spawn,
  so a rerun never wraps an already wrapped command.
- Not supported: `tasks.json` (global, `.ed/`), `.vscode/tasks.json`, the task
  picker/modal, `task::Spawn`, `task::Rerun`, task history, task hooks (including
  worktree-creation setup tasks and the worktree picker's "Automate Setup"),
  git "Custom Commands" (`git-command` tagged tasks) in commit/ref context menus,
  the tasks JSON schema, the "Run" app menu, language tasks not attached to a
  runnable tag (e.g. "cargo check", "go test ./...", "execute selection"), and
  the `languages.<lang>.tasks.enabled` setting.
