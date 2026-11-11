# Remote cleanup: further directions

Written 2026-11-11, after the second cleanup pass (see
`cleanup-remote-handoff.md` for what is done). None of this is started. Items
are roughly ordered by payoff per effort.

## Dead code exposed by the CRDT simplification

- `language::unified_diff_with_offsets` (`crates/language/src/text_diff.rs`)
  is now only re-exported; its last user was `EditPreview`. Delete it, and
  check whether the rest of `text_diff` is still used.
- `text::Buffer::lamport_clock` is a `pub` field, but nothing outside `text`
  reads it now. Make it private.
- `text::Anchor::opaque_id` still returns `[u8; 20]` with bytes 17–19 always
  zero (the replica id used to live there). It could shrink to `[u8; 17]` if
  `gpui::ElementId::OpaqueId` takes a different size, or stay as is.
- `clock::Global` wraps a `u32` but is not `Copy`, to avoid churning ~90
  `.clone()` call sites. Making it `Copy` (and fixing the `clone_on_copy`
  clippy hits) is a mechanical cleanup.
- `clock::Global::value()` is used only in tests.
- `text::UndoMap` still records an undo id per (edit, undo) pair, so that
  `was_undone` can answer for older versions. That's needed for
  `rope_for_version` / `edits_since`, so it stays, but it is the last
  multi-version structure worth a second look.

## Leftovers of the mechanical rewrites

- `crates/project/src/lsp_store.rs` has ~64 `let local = &self.local;`-style
  aliases. Many sit in bare `{ ... }` blocks created when
  `if let Some(local) = self.as_local_mut() { ... }` was unwrapped. Inline
  `self.local` where the borrow allows it and drop the extra blocks.
- `LocalLspStore` itself could merge into `LspStore`, like `LocalWorktree`
  into `Worktree`. That's a large change, with many methods on both and
  borrow-splitting that relies on the separate struct. Do it only if the
  split stops paying for itself.
- `crates/project/src/git_store.rs`: the `RepositoryState` rewrite left
  `{ let RepositoryState { backend, .. } = state; ... }` blocks as tail
  expressions. Flatten them to plain `let` statements where they aren't
  needed as expressions.
- `Worktree::abs_path()` (returns `Arc<Path>`) now shadows
  `Snapshot::abs_path()` (returns `&Arc<SanitizedPath>`) inside worktree code,
  since `Worktree` derefs to `LocalSnapshot`. It compiles and behaves the same,
  but the two methods should get distinct names or one should go.
- `Worktree::local(...)` is still named for the old variant. Rename it to
  `new`.

## Internal splits kept on purpose (optional merges)

- `BufferStore.local: LocalBufferStore` and `ImageStore.state:
  Entity<LocalImageStore>`. Merging means renaming clashing methods
  (`open_buffer`, `save_buffer`, `save_buffer_as`, `create_buffer`,
  `reload_buffers`) and deduplicating `worktree_store`. This is structure
  only; there's no dead branch to remove.

## `language::File::as_local` returns an `Option`

There are three implementors. `worktree::File` always returns `Some` now;
`IndexTextFile` (`git_store.rs`) and `GitBlob` (`commit_view.rs`) presumably
return `None`. The `Option` still means "has a path on disk", so it can't
simply go. Options:

- Leave it.
- Replace it with an explicit `abs_path(&self, cx) -> Option<PathBuf>` on
  `File`, and drop the `LocalFile` trait if its other methods (`load`,
  `load_bytes`) only serve worktree files.

## Workspace prompts

- `on_prompt_for_open_path` / `on_prompt_for_new_path` only run when the macOS
  open or save panel returns an error. Dropping them (just showing the error)
  would also remove the `DirectoryLister` argument from the prompt APIs and the
  registration in `file_finder`. The trade-off is that the user gets no in-app
  picker if the system panel fails.

## Platform (from the Linux removal pass)

- `Window::gpu_specs` is always `None` on macOS, so the check that uses it in
  `crates/ed/src/ed.rs` never runs.
- `PlatformDispatcher::increase_timer_resolution` exists for Windows timers.
- `prefer_character_input` is always `false` on macOS.
- `ActivationPolicy` / `set_activation_policy` have no callers outside gpui.
- `crates/icons` has a `Linux` icon variant.
- `crates/project/src/biome.rs:109` has a `#[cfg(unix)]` block in a test that
  can simply be unwrapped.
- `crates/paths` still uses XDG-style directories. Changing this moves user
  data, so it needs its own decision and a migration.

## Mermaid

- `gantt` is reported as unsupported, but leftovers remain:
  - detection in `crates/mermaid/src/detect.rs`;
  - `gantt` config merging in `preprocess.rs`;
  - `Options::now` in `lib.rs`;
  - the "current time" highlight in the `svg/theme.rs` docs.
- `pie` and `kanban` were dropped from the tests for the same reason; check
  for similar leftovers.

## Testing gap

- No crate has `#[gpui::test]` tests, so entity-level behaviour can't be unit
  tested. Examples:
  - `language::Buffer` emitting `LineEndingChanged` or `DiagnosticsUpdated`;
  - undo restoring the clean state.

  These are covered only by edctl runs. A minimal gpui test-support setup for
  `language` and `project` would close that.

## Not verified yet

- The activity indicator's progress refresh, which relies on
  `LspStoreEvent::PendingWorkUpdated`
  (`crates/activity_indicator/src/activity_indicator.rs`). Watch a
  rust-analyzer indexing bar update on a large project.
