# Lazy parsing review

Reviewed 2026-11-07:

- `16609809b0c410ebbd0bdc6daacb2f92be8a1818` — delay assigning a language to HEAD diff buffers.
- `68221af507115b3d6b6c9d9c75e4386c7e09f65a` — defer parsing loaded buffers until needed.

Reviewed both patches and related code in the current checkout, whose HEAD is `615596be0b`. Source locations below refer to that checkout. Existing uncommitted changes were left untouched. Findings are from static analysis; no build, unit tests, GUI tests, or memory measurements were run. The reproduction cases below are proposed regression tests, not claims of executed reproductions.

## Findings

### [P2] Await the initial parse before returning outline items

Introduced in `68221af507`, `crates/editor/src/document_symbols.rs:54–57`.

`buffer_outline_items` calls `ensure_parsed` and immediately takes a snapshot. `ensure_parsed` only starts parsing: `Buffer::reparse` waits synchronously for at most 1 ms, then completes asynchronously (`crates/language/src/buffer.rs:1712–1806`). When the initial parse exceeds that budget, the snapshot has no syntax tree. Moving outline extraction to a background task does not update that immutable snapshot, so the returned task can resolve to an empty outline despite symbols being present.

The comment relies on the outline panel refetching after `Reparsed`; that recovery exists in `crates/outline_panel/src/outline_panel.rs:4590`. However, this API also serves `Editor::go_to_symbol_by_offset` (`crates/editor/src/navigation.rs:1707`), which awaits one result and does not retry on `Reparsed`. A next/previous-symbol command issued before the first parse completes can silently do nothing. The panel recovery is not a sufficient contract for every caller.

Recommended change: start parsing, await `parsing_idle`, then take a fresh snapshot and extract the outline, following the pattern already added to `TaskStore` for remote task contexts. Keep the asynchronous task interface so the UI remains responsive.

Regression test: use a deferred buffer with outline captures and force asynchronous parsing; request outline items before drawing it. The returned task should contain the expected symbols without requiring an external retry. Also exercise one-shot symbol navigation.

### [P2] Parse the pre-edit state before constructing the first autoindent request

Introduced in `68221af507`, particularly the deferral gate in `crates/language/src/buffer.rs:1747–1750`; related code is `edit_internal` at line 2541 and `compute_autoindents` at line 1909.

The autoindent escape hatch enables parsing only once `autoindent_requests` is nonempty. By that time, `edit_internal` has already captured `before_edit = self.snapshot()` and performed the text edit. On a deferred buffer, that saved snapshot has no syntax tree or injected-language scopes. Parsing the new text cannot repair the saved snapshot.

`compute_autoindents` explicitly compares syntax-derived old and new indentation suggestions to avoid changing indentation when the edit did not change the structural indentation requirement. The first edit to an undrawn buffer instead compares an unparsed old state with a parsed new state. This can reindent existing lines unnecessarily. In injected languages, the request also captures language-specific settings and indent sizes from the unparsed outer language. This matters for programmatic autoindenting edits to bulk-loaded buffers, even if normal typing usually happens after a frame has parsed the buffer.

Recommended change: ensure the pre-edit syntax needed by autoindent is available before capturing its snapshot, or explicitly parse the retained pre-edit snapshot before using it. Simply starting an asynchronous parse before snapshot creation is insufficient unless completion is awaited.

Regression tests: apply the same autoindenting edit to eagerly parsed and deferred versions of a file and compare the resulting text. Include an edit on an existing intentionally indented line whose syntax nesting does not change, plus an injected-language case with different indentation settings.

## HEAD diff optimization

No separate correctness defect was established in `16609809b0` during this review.

- The HEAD buffer begins without a language. The comparison checks working text against HEAD and, when a staged diff exists, checks whether the index differs from HEAD. Missing HEAD text remains unparsed.
- Once enabled, language assignment stays enabled for the lifetime of `BufferGitState`. Recalculations continue to apply language changes.
- Including `enable_head_text_language` in the snapshot-refresh condition is necessary: the transition must produce a parsed base snapshot even when the HEAD text itself did not change. `snapshot_with_edits` explicitly reparses its snapshot on the background executor; it does not rely on the asynchronously scheduled buffer parse finishing first.
- The predicate is conservative: any text difference enables parsing, including insertion-only changes with no deleted lines. It also does not check whether the diff is visible. Thus changed, undrawn files can still retain parsed HEAD trees. This is a limit of the optimization, consistent with the broader condition documented in `spec/git.md`, rather than evidence that visible-row parsing covers diff bases.
- Ordinary loaded-buffer deferral is one-way too. Once a file has been drawn, later edits and external reloads keep parsing it even after it scrolls off-screen. Neither commit evicts syntax trees.

The helper tests cover comparison and deduplication logic, but not the full lifecycle: deferred language assignment, asynchronous completion, first autoindent, actual drawn-buffer activation, HEAD activation during edits/staging, or filesystem reloads. `parse_stays_deferred`'s truth-table test cannot detect either finding above.

## How filesystem changes reach the editor

The filesystem notification path is independent of visibility and tree-sitter parsing. These commits do not disable watches or postpone disk reloads until drawing.

1. **Native events.** `RealFs::watch` (`crates/fs/src/fs.rs:776`) delegates to `fs_watcher::watch`. The shared `OsWatcher` uses `notify::RecommendedWatcher`; macOS watches recursively through FSEvents, while Linux watches directories through inotify. The backend subscribes to core events, and access events are discarded. Native callbacks enqueue events; a separate dispatcher routes them to relevant registrations (`crates/fs/src/fs_watcher.rs`, `ensure_backend`, `enqueue`, `dispatch_batch`, `dispatch`).
2. **Batching and recovery.** Registration callbacks translate native events into `PathEvent`s. Create, modify, and remove become created/changed/removed events; overflow/rescan markers become rescan requests. Pathless overflow events are coalesced per drained batch and delivered to registrations. The worktree watch uses a 100 ms batching latency (`crates/worktree/src/worktree.rs:133`). Missing/skipped watch registrations are retried in the background; stale nonrecursive watches are reinstalled. This is asynchronous notification, not immediate consistency with every filesystem write.
3. **Worktree scan.** `BackgroundScanner` drains event batches and calls `process_events` (`crates/worktree/src/worktree.rs:3979`). It refreshes affected entries and relevant Git metadata, then publishes a snapshot. `LocalWorktree::set_snapshot` at line 1268 emits `UpdatedEntries` and `UpdatedGitRepositories` when their respective changes are nonempty. Initial scanning also drains events accumulated during the scan.
4. **Loaded-buffer lookup.** `LocalBufferStore::subscribe_to_worktree` listens for `UpdatedEntries` (`crates/project/src/buffer_store.rs:424`). Changes are matched to already-open buffers by entry ID or project path, preserving rename handling. The new file handle records path, entry ID, and disk state, including modification time and size. Unchanged handles return early; changed handles go through `Buffer::file_updated`. A watch event alone does not necessarily imply a text reload: the entry/file-state comparison matters.
5. **Reload and local edits.** `Buffer::file_updated` (`crates/language/src/buffer.rs:1560`) emits `ReloadNeeded` when a clean buffer's disk state changes to a present file. `Project::on_buffer_event` (`crates/project/src/project.rs:2132`) handles it by reloading. Dirty buffers are not automatically overwritten. Reload reads and decodes bytes, calculates a text diff in the background, and applies it only if the buffer version still matches; otherwise it preserves concurrent edits and records a conflict as appropriate. `did_reload` emits `Reloaded` and calls `cx.notify()`.
6. **Parse and redraw.** Applying the text diff reaches the normal edit/reparse path. A still-deferred buffer without autoindent requests keeps its new text but builds no syntax tree. A previously activated buffer reparses normally. When an editor later draws a deferred buffer, `parse_drawn_buffers` starts parsing. Parse completion emits `Reparsed` and notifies observers; MultiBuffer forwards this to Editor, which refreshes syntax-dependent features. The initial frame can therefore precede syntax highlighting.
7. **Git updates.** Edited/reloaded buffers request diff recalculation through `Project::on_buffer_event`. Worktree entry and Git repository events also reach `GitStore::on_worktree_store_event` (`crates/project/src/git_store.rs:1982`), updating repository paths/status and reloading relevant diff bases. `BufferGitState::diff_bases_changed` then recalculates diffs and reevaluates HEAD-language activation. File-content reload and Git-base refresh are separate asynchronous paths.
8. **Remote projects.** Host-side filesystem events update the host worktree and buffers. File metadata is forwarded through `UpdateBufferFile`; buffer operations and saved/reloaded state have their own protocol handlers. A remote buffer's deferred syntax parsing does not suppress receiving those text or metadata updates.

Current deletion behavior deserves a version distinction: clean local buffers are emptied when their file is deleted, while dirty buffers retain their text (`Buffer::file_updated`). That behavior comes from later commit `e304097524`, not either reviewed commit.

## Recommended validation before changing the implementation

- Add lifecycle tests for the two findings, then verify language assignment without parsing, first draw, repeated `ensure_parsed`, asynchronous completion, and remote task context extraction.
- Add Git integration coverage for unchanged HEAD, first working edit, staged-only changes with working text equal to HEAD, missing HEAD, language change, and interrupted/restarted recalculation.
- Exercise external write, rename, and deletion for both undrawn and drawn buffers, including a dirty-buffer case. Assert text freshness separately from parse activation.
- Follow `spec/automation.md` with `edctl` for project-search scrolling, syntax appearance, outline/navigation, diff highlighting, and external reload behavior. The existing helper-only tests do not validate those UI paths.

This review adds documentation only. No implementation or design contract was changed, so `/spec` was not modified. If the findings are fixed, update the language spec to state the completion requirement for off-screen syntax consumers and the pre-edit requirement for autoindent.
