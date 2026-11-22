# Async runtime research

Re-evaluated 2026-11-16 (macOS only, tokio already removed).

Question: keep smol / async-task / async-process, or reimplement on tokio?

Answer: stay off tokio. It cannot replace the executor, and for the rest it is
the same mechanism with more weight. Instead smol was removed entirely: IO now
runs on GCD dispatch sources (`async_macos` crate, see `spec/platform.md`).

Sections below up to "Patches" describe the state before that change and why
tokio was rejected.

## Current usage

| Layer | What runs it | Crate |
|---|---|---|
| Task spawn / wake | `async_task::Builder` + `RunnableMeta` (`crates/gpui/src/executor.rs`) | async-task |
| Scheduling | GCD: global queues (3 priorities), main queue, `dispatch_after` for timers (`crates/gpui_macos/src/dispatcher.rs`) | dispatch2 |
| Child processes | `smol::process` behind `util::command` / `util::process`; piped stdio in lsp, git, fs (git clone), terminal | async-process, async-io |
| File IO | `smol::fs` (19 call sites, all in `fs`) and `smol::unblock` (4) | async-fs, blocking |
| Sockets | `smol::net::unix` in `ed/control_server.rs` only | async-net, async-io |
| Misc | `yield_now` (4), `smol::lock::RwLock` (1), `smol::channel` (3), `block_on` in tests | futures-lite, async-lock, async-channel |

- 8 crates depend on `smol`. `smol::spawn`, `smol::Timer` and `smol::Executor`
  are unused, so smol's own executor threads never start.
- smol contributes 32 crates to the build, most shared with other deps.
- Background threads from smol: one `async-io` kqueue reactor, one
  `async-process` reaper, and a `blocking` pool that grows on demand and exits
  after 500ms idle.

## Tokio

- Executor: no gain possible. Background work is already multi-threaded on
  GCD. Tokio has no "give me a runnable, I schedule it" API, so main-thread
  dispatch, QoS priorities and the deterministic test scheduler would all
  still need async-task. Tokio would sit beside it, not replace it.
- IO / process / fs: same mechanisms. kqueue reactor thread, SIGCHLD reaper,
  `fs` as a blocking thread pool. Not faster.
- Extra cost: a runtime (at least one driver thread plus its own blocking
  pool), a `Handle::enter()` guard on every GCD thread that touches tokio IO
  (this is what `gpui_tokio` was), and `tokio-util` compat shims because the
  code uses `futures::io` traits (lsp, git, fs).
- Net: more threads, more memory, larger binary, a second scheduler, and a
  rewrite of ~10 crates for no measurable win.

## "Not in active development"

smol-rs is mature rather than dead: async-executor 1.14.1 (2026-11-02),
blocking 1.7.0 (2026-08-23), async-task merged two PRs on 2026-11-14. APIs are
stable, so there is no "latest API" to chase.

## Patches

- async-process (zed fork `0b6d671`, adds `adopt_raw_pid`): removed
  2026-11-16, crates.io 2.5.0 is used. The only caller
  (`crates/util/src/command/darwin.rs`) was already gone.
- async-task (zed fork `015845a`, waker vtable identity fix): upstream merged
  the same fix as smol-rs/async-task#108 (`41fd335`, 2026-11-14), not released
  yet. The patch now points at upstream head (`3aea7f5`, 2026-11-14, adds
  `Runnable::is_cancelled`); delete it on 4.7.2.

## Done 2026-11-16: below smol, onto GCD

- `smol`, `futures-lite`, async-io, async-process, async-net, async-fs,
  async-signal, async-executor, blocking and piper are gone (Cargo.lock 490 ->
  481 packages). `polling` and `rustix` remain via `alacritty_terminal`.
- `async_macos` crate: `AsyncFd` (read/write dispatch sources), `ProcessExit`
  (`DISPATCH_SOURCE_TYPE_PROC` + `waitpid`), `unblock`, `yield_now`, with
  `command`, `net` and `fs` modules on top. About 930 lines with tests.
  `util` keeps only policy on top (`process::Child`, `shell_env`).
- Unified on `futures`.
- Still from the smol family: async-task (executor), async-channel,
  async-lock, event-listener, parking, flume.

## Next steps

- [ ] Delete the async-task patch when 4.7.2 is released.
