# Tasks

## Decision

User-defined tasks are removed. The only way to run something from the editor
is the gutter run icon (runnables), which spawns the command in a terminal tab
("task output"). Anything else is run from a script in the terminal.

The `tasks_ui` crate is removed. The `task` crate stays only as the shared
template/variable/spawn types the run icon needs. A template is just label,
command, args, env, cwd and runnable tags; there are no per-task knobs.
`TaskTemplate` stays (de)serializable only because LSP runnables reach remote
clients as JSON.

## Behaviour

- Each language has exactly one source of run icons, with no fallback:
  - a language whose `ContextProvider::lsp_task_source` names a server (Rust:
    rust-analyzer) takes them from LSP runnables only, for the whole buffer.
    Its `runnables.scm` is not queried. Nothing shows until the server
    answers: the request has no timeout, and it is sent again when the
    buffer is reparsed, when the server registers the buffer, and when the
    server asks for an inlay hint refresh (rust-analyzer does once it has
    loaded the project);
  - every other language uses its tree-sitter `runnables.scm` tags, matched
    against the task templates the language provides
    (`ContextProvider::associated_tasks`). The query covers only the visible
    rows plus one screen above and below, and runs again when scrolling
    leaves that range, so runnables far from the viewport are unknown.
- Only the buffer's own language contributes tree-sitter runnables; code
  injected into it (e.g. a markdown code block) does not, since templates
  target the whole file.
- Markdown fenced code blocks tagged `sh`/`shell`, `bash`, `zsh` or `fish` get
  a run icon on the opening fence that runs the block's code with that shell
  (`<shell> -c`), from the markdown file's folder. The code reaches the shell
  through the `CODE_BLOCK` env var, so the outer task shell never re-parses it.
- `languages.<lang>.tasks.enabled = false` hides a language's template-based icons.
- Clicking the icon is the only way to run one (there is no keyboard action
  for the nearest task). It resolves the template with
  `$ED_*` variables and spawns it in the terminal panel. The icon shows
  running / passed / failed state.
- In a single-file worktree (a file opened without its folder) there is no
  worktree root: `$ED_WORKTREE_ROOT`, the relative path variables and the
  default cwd are unset, so templates that need them don't resolve.
- Templates are built in code, so substitution only knows `$ED_NAME`: an
  unset one fails the template, and nothing else is interpreted (no `${..}`,
  defaults or `$$` escape; any other `$` reaches the shell as written). Only
  the template's own env values are substituted, not the project environment.
  Task variables are not exported to the task's environment.
- A task always runs in the terminal dock with the default system shell, saves nothing first, and leaves its tab
  open with no summary lines when it finishes. Running a task whose tab
  already exists waits for that run to finish, then replaces the tab's output.
- A task terminal tab has a rerun button (and `terminal::RerunTask`) that
  respawns that tab's own command in place; there is no global rerun. The tab
  keeps the task as requested; wrapping it in the shell happens on each spawn,
  so a rerun never wraps an already wrapped command.
- Rust `#[cfg(test)] mod <name>` icons run `cargo test -p <pkg> -- <file module>::<name>::`
  (just `<name>::` in a crate root); the file's full module path isn't known,
  which is fine because libtest filters by substring.
- Not supported: `tasks.json` (global, `.ed/`), `.vscode/tasks.json`, the task
  picker/modal, `task::Spawn`, `task::Rerun`, task history, task hooks (including
  worktree-creation setup tasks and the worktree picker's "Automate Setup"),
  git "Custom Commands" (`git-command` tagged tasks) in commit/ref context menus,
  the tasks JSON schema, and the "Run" app menu.
