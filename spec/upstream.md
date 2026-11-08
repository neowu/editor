# Upstream sync

## Decision

The fork is a squashed snapshot of Zed (upstream `7dc397b61b`, v1.25.0) with
no shared git history, and it diverges further every pass. Upstream is a
source of ideas and fixes, not something to stay in sync with.

- Changes are ported by reading the upstream change and re-applying its
  intent against the fork's code. No merges, rebases, cherry-picks or
  `git apply`. Vendored crates are owned code too and are never re-synced.
- Reviews go by subsystem, not by commit: decide whether the subsystem a
  change touches still exists in the fork and is worth taking from, and skip
  everything else without recording it. There's no per-commit ledger of
  declined changes.
- Take correctness and performance fixes in subsystems the fork keeps. Take
  features only when they're asked for. Skip refactors, renames, version
  bumps and churn that only make sense for upstream's direction.
- If the fork already solves a problem another way, port only the
  user-visible behaviour, not upstream's structure.
- A noisy diff is fine. Reformatting, lint fixes and cleanups in passing are
  welcome. Warning cleanup must preserve runtime behaviour (truncation,
  overflow, branch order, rendering) and must not add clamping, panics or
  fallback values.
- The app version stays at the snapshot's version unless it's deliberately
  bumped.

## Subsystems

Skip upstream changes that only touch areas the fork removed. The current
crate list (`ls crates`) is the source of truth. The major removals:

- Agent/ACP, language models, collab, livekit, organization accounts,
  extensions and Wasm/managed Node.
- Windows, Linux, iOS, web, wgpu and OpenHarmony platforms.
- Debugger, vim/helix, sidebar, multi-workspace windows and project groups,
  git hosting providers, docs site, CI, crates.io client tooling.
- Tests, benches, `test-support` features and GPUI examples (see
  `testing-and-benchmarks.md`).

Directions the fork deliberately doesn't follow, even in kept crates:

- Context menu submenus: no further submenu features (embedded views,
  persistent refresh, side placement, custom triggers, keyboard handling) or
  UI built on them. Nested menus are cumbersome; prefer flat menus or pickers.
- `gpui_macos` objc2 migration, `PathStyle`/`join_path` path refactors,
  Low Power Mode frame-pacing variants and Display P3.
- Large git panel modes and settings surfaces (branch-compare mode, date
  format settings) unless they're specifically asked for.

## Porting

1. Each port is its own commit: the upstream subject with the `(#12345)` PR
   number, a short body saying what was adapted or dropped, and a
   `(cherry picked from commit <full hash>)` trailer. Search the fork's log by
   PR number to see if something was already ported.
2. Drop changes to deleted files, removed-platform code, tests and
   test-support helpers. Add unit tests for the fork's changed functions.
3. `cargo +nightly fmt --all`, and keep `cargo clippy --workspace
   --all-targets` warning-free.
4. Dependency bumps: `cargo update -p <crate> --precise <version>` per crate,
   revert unrelated transitive reselections in `Cargo.lock`, and confirm with
   `cargo check --locked -p ed`.
5. Update `/spec` where a port changes behaviour or sets a lasting rule.

## Status

- Last reviewed upstream commit: `b5cd310b3b` (2026-11-07). The next pass
  starts at `git log --reverse b5cd310b3b..upstream/main`.
- Open candidates from earlier passes, worth taking if wanted: #64624
  (UAX #14 line breaking), #53089 (git auto fetch), #66284 (no worktree hooks
  in restricted mode), #66423 (drag git panel files to the terminal), #66296
  (mouse-wheel zoom in terminals and diffs), #66450 (hide the edit prediction
  status bar button), git UI polish (#66260, #66187, #66226), #63909, #65474,
  #66144, #66384, #66330/#66137 (JetBrains keymap).
- Watch: #66487 (parallel LSP loads), reverted upstream as #66517. Revisit
  if it lands again.
