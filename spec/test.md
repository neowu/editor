# Tests

## Where a test goes

- **Unit test**: a `#[cfg(test)]` module next to the code, in the crate that
  owns it. It uses only the crate's own dependencies. Every function added or changed gets one.
- **End-to-end test**: a test that needs more than the crate under test, such as
  another dependency (a real grammar). "End-to-end" means real
  parts and runtime, no mocks. It does not mean launching the whole app: test
  the part in question directly. GUI flows through the running app go through
  `edctl` (`automation.md`) instead. Each one lives in its own package under
  `test/<name>/`:
  - `tests/*.rs` holds the tests;
  - `src/lib.rs` holds support code the tests share; the tests exercise it, so
    it has no unit tests of its own; a package without support code has no `src/`;
  - `assets/` holds fixed input files the tests read in place (via
    `CARGO_MANIFEST_DIR`); only per-run state goes to a temp dir.
- No crate has `[dev-dependencies]`. A test that needs one is an end-to-end
  test. If the code under test is private (e.g. inside the `ed` binary), move
  the dependency-free core into the library crate that fits, so the e2e
  package can call it, and keep the caller as a thin adapter.

## Packages

- `test/syntax_test` runs `language::walk_syntax` (behind `edctl syntax-tree`)
  over real tree-sitter grammar parses of files in `assets/`.
- `test/task_test` runs each language's bundled `runnables.scm` over real
  fixture files in `assets/` (the folder stands in for the worktree root) and
  feeds the matches to the task builders behind `ContextProvider::runnables`
  (exported from `languages::runnables`). It runs no app or project, so the
  outline symbol and the async `package.json` lookups are supplied by the
  test; the click-to-terminal path is checked with `edctl`. Rust runnables
  come from rust-analyzer and are not covered.
- `test/theme_test` is a GUI tool, not a test target: everything is in `src/`
  (`main.rs` plus modules with unit tests), and `cargo run -p theme_test`
  shows it (`theme.md`). GUI code can't run under `#[test]`: AppKit needs the
  process's main thread, libtest runs tests on spawned threads, and quitting
  the app ends the process before the harness reports. A GUI check is a binary
  like this, or goes through `edctl` against the running app.
- Name packages `<topic>_test`. Members are picked up by `test/*`.

## Running

- `cargo test -p <name>` runs one package; `cargo test` alone runs only the
  default member (`ed`).
