# Handoff: cleanup after removing remote projects

Written 2026-11-11. Follows the local-only change (remote projects removed).

## State when this was written

- The `remote`, `remote_connection`, `remote_server`, `rpc` and `proto` crates
  are deleted. `Project`, `Worktree`, the stores (buffer, worktree, lsp, git,
  task, image), terminals and search have no remote mode and no protobuf layer.
- The change was **uncommitted** in the working tree (169 files, about 31,300
  lines removed). Commit it first, or build on top of it; do not revert it.
- `cargo check --workspace --all-targets`, `cargo clippy --workspace
  --all-targets` and `cargo +nightly fmt --all -- --check` are clean.
- `cargo test --workspace --no-fail-fast` passes except two `mermaid` tests
  (see task 4), which fail independently of this change.
- Specs are updated; the open items are also listed under Follow-ups in
  `spec/collaboration-and-telemetry.md`. Remove entries there as they are done.

## Status (2026-11-11, second pass)

Done, uncommitted, on top of the removal change:

- Task 1: single-writer buffer. `Buffer::branch`, `merge_into_base`,
  `EditPreview` / `preview_edits` and `text::Buffer::branch` had no callers and
  are gone (with `ApplyDiffHunk` / `ApplyAllDiffHunks`). `ReplicaId` is gone;
  `Lamport` is a `u32`; `clock::Global` is one counter. Removed `Operation`,
  `BufferEvent::Operation`, `BufferEditSource`, `apply_ops`, the deferred-op
  queue, `wait_for_*`, remote selections, per-edit op history and the
  insertion-version fields of the fragment summary. Undo keeps the undo map.
- Task 2: `Worktree` is a struct (no `LocalWorktree`, `as_local*`, `is_local`);
  `worktree::File::is_local` gone; `LspStore::as_local*` replaced by
  `self.local`; `RepositoryState` is a struct; `LanguageServerLogType::Rpc`,
  `Project::capability()`, `BackgroundPathChecks`,
  `HighlightedMatchWithPaths.prefix` gone; `ContextLocation.fs` is required.
- Task 3: Linux / non-macOS code removed (FSEvents-only watcher, trash crate
  macOS-only, dead gpui platform APIs).
- Task 4: mermaid tests fixed, `disclaim.md` updated.

Left on purpose:

- `BufferStore.local` / `ImageStore.state`: internal splits, not remote modes.
- `on_prompt_for_open_path` / `on_prompt_for_new_path`: live fallbacks after a
  system dialog error.
- `NSLocalNetworkUsageDescription` stays (terminals, git and servers may use
  the local network).
- Possible next trims (noted by the platform pass): `Window::gpu_specs`,
  `increase_timer_resolution`, `prefer_character_input`, `ActivationPolicy`,
  the `Linux` icon variant, gantt support in `mermaid`.
- Not checked in the app: the activity indicator's progress refresh.

The original task list follows for reference.

## 1. Simplify the buffer CRDT to a single writer

Background: `memo/research/crdt.md` (option A). The only writers left are the
local replica and local branch buffers (`ReplicaId::LOCAL_BRANCH`,
`Buffer::branch` / `merge_into_base`, used by `crates/editor/src/git.rs`).

Crates: `clock`, `text`, `language`.

- Remove `ReplicaId::REMOTE_SERVER` and `is_remote()` (`crates/clock/src/clock.rs`;
  two tests in `crates/text/src/text.rs` still build a `REMOTE_SERVER` buffer).
- Decide first, and write it down, whether `ReplicaId` and `clock::Global`
  version vectors can collapse to one counter or must stay for branch merge.
  This is the open question; everything below depends on it.
- Candidates to delete: the deferred-op queue, `wait_for_version` /
  `give_up_waiting`, `apply_ops` paths and `Operation` variants no longer
  constructed, `BufferEvent::Operation { is_local }`, `remote_selections` and
  similar state in `crates/language/src/buffer.rs`, undo-as-ops (`UndoMap`
  counts sent as operations).
- Must stay: the fragment tree, tombstones, `Locator` and insertion ids.
  Anchors (selections, diagnostics, multibuffer excerpts, git hunks) need them.
- Stop and report if branch buffers really need the multi-writer parts.

## 2. Collapse single-variant leftovers

Mechanical, but many call sites. Kept during the removal to limit churn.

- `Worktree` is an enum with only `Local`; `as_local()` / `as_local_mut()` are
  always `Some`, `is_local()` always true, `replica_id()` always `LOCAL`
  (`crates/worktree/src/worktree.rs`). The one-arm `match self` blocks go with it.
- `worktree::File::is_local` field.
- `LspStore::as_local()` / `as_local_mut()` (about 119 call sites) with dead
  `else { Err("no upstream client…") }` branches (`crates/project/src/lsp_store.rs`).
- `RepositoryState::Local` (about 58 job closures match on it) in
  `crates/project/src/git_store.rs`.
- `BufferStore.local: LocalBufferStore` could merge into `BufferStore`
  (`crates/project/src/buffer_store.rs`); same for `ImageStore.state`.
- `LanguageServerLogType::Rpc { received, elapsed }` is only built inside
  `crates/project/src/lsp_store/log_store.rs`; it can leave the
  `LspStoreEvent::LanguageServerLog` payload type.
- `Project::capability()` always returns `ReadWrite`.
- `workspace::path_link::BackgroundPathChecks::ProjectPathResolution`: check
  whether anything still passes it; the default is now always `LocalFileSystem`.
- `on_prompt_for_open_path` / `on_prompt_for_new_path` hooks in
  `crates/workspace/src/workspace.rs`: the open hook is now only the fallback
  after a system-dialog error; the new-path hook may be unused.
- `HighlightedMatchWithPaths.prefix` is always `None` from `recent_projects`.
- `language::ContextLocation.fs` is `Option<Arc<dyn Fs>>` but always `Some`
  (`crates/project/src/task_store.rs` wraps it).

## 3. Remove Linux-only code

It existed only for the headless remote server. macOS is the only target now.

- Find it: `grep -rn 'target_os = "linux"\|not(target_os = "macos")\|cfg(unix)' crates --include='*.rs' --include='*.toml'`
- Known spots: `crates/gpui/src/gpui.rs` (`compile_error!` text and the Linux
  allowance), `crates/gpui/src/platform/keystroke.rs`,
  `crates/gpui/src/svg_renderer.rs`, `crates/gpui/src/window.rs`,
  `crates/gpui/src/app.rs` (headless/windowed notes), `crates/fs/src/fs.rs`,
  `crates/fs/src/fs_watcher.rs` (inotify), `crates/trash` (`freedesktop.rs`,
  Linux `cfg`s in `trash.rs`), `crates/util/src/util.rs`,
  `crates/ed/src/ed/app_menus.rs`.
- `crates/paths` uses XDG-style directories (`~/.local/share/ed`, …). Changing
  that moves user data, so treat it as a separate decision, not part of this.
- Keep code about file *content* (Unix paths, line endings).
- Drop dependencies that become unused (unused crate deps are a denied warning).
- Update `spec/platform.md`.

## 4. Small items

- `mermaid` tests `output_has_no_browser_markup` and `styles_label_markup`
  (`crates/mermaid/src/lib.rs`, around lines 196–212) render `pie` and `kanban`
  sources, which the crate now reports as unsupported. Drop those sources from
  the tests.
- `Info.plist` keeps `NSLocalNetworkUsageDescription` (reworded for terminals,
  git and language servers). Remove the key and its bullet in
  `spec/platform.md` if that prompt is not wanted.
- `memo/plan/disclaim.md` still describes ssh as the remote transport.
- Not checked in the running app: the activity indicator's progress refresh,
  which now relies on `LspStoreEvent::PendingWorkUpdated`
  (`crates/activity_indicator/src/activity_indicator.rs`). Watch a
  rust-analyzer indexing bar update on a large project.

## How to verify

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets` (zero warnings)
- `cargo +nightly fmt --all`
- `cargo test --workspace --no-fail-fast`
- Running app, per `spec/automation.md`: `cargo build -p ed -p control`, then
  `target/debug/edctl launch --fresh <project> <file>`. Zsh does not word-split
  variables, so export `ED_CONTROL_SOCKET` instead of storing
  `edctl --socket …` in a variable. Flows that covered the removal: `wait-lsp`,
  type + `workspace::FormatAndSave`, `editor::GoToDefinition`,
  `pane::DeploySearch`, `workspace::NewTerminal`. For task 1 add undo/redo and
  the git diff / branch-buffer flows; for task 3 add trashing a file from the
  project panel.

Suggested order: 4 (quick), then 1, then 2 (it overlaps 1 in `worktree` and
`language`), then 3 (independent of the rest).
