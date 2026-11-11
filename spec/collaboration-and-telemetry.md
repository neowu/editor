# Collaboration and telemetry

This fork is a single-user editor. Collaboration panels, channels and channel
notes, calls, screen sharing, participant controls, account menus, and sign-in
entry points are removed, together with the collaboration server, UI, client,
and account API crates. Unused account, channel, call, and following protocol
definitions are removed.

The editor is local-only: there are no remote (SSH) projects, no server
binary, and no RPC or protobuf layer (the `remote`, `remote_connection`,
`remote_server`, `rpc` and `proto` crates are gone). `Project`, its stores,
worktrees, terminals and search have a single local mode. `projects.json`
entries carrying a `remote` key, written by older versions, are dropped on
load.
GPUI has no screen-capture API or `screen-capture` feature, and the app does not
link ScreenCaptureKit or ReplayKit.
GPUI has no video `surface` element or CoreVideo pixel-buffer render path.

The window header and status bar retain project, worktree, Git, and platform window controls.
These controls initialize independently of collaboration. Old collaboration and
account visibility settings no longer expose or enable features.

There are no collaborator or server replicas: the local buffer is the only
writer. The text buffer keeps the fragment tree, tombstones and insertion ids
that anchors need, but its clock is one counter: a `Lamport` timestamp is a
`u32` and a version (`clock::Global`) is the newest timestamp applied, because
edits apply in timestamp order. There are no operations to send or replay, no
deferred-op queue, no branch buffers, no buffer-held selections for other
replicas, and undo/redo stays local (an undo map of per-edit counts). A buffer's
`BufferId` (`buffer_id()`) is just its identity within the process, used by
anchors and the stores' lookup maps.
Dead sharing/unsharing APIs and the call-status-icon and public-channel-sharing
settings are removed.

There is no player concept: the theme has one `CursorColors` (cursor and
selection), greyed out in read-only editors. Every rendered selection is the
local user's, so cursors carry no collaborator name label. Places that need several distinct colors (git blame, Mermaid diagrams, debug
ranges) use the theme's `accents` palette.

No usage events, activity metrics, machine identifiers, remote telemetry forwarding,
hang reports, or automatic crash uploads are collected or sent. Telemetry settings,
onboarding switches, and telemetry log actions are removed. Local diagnostic logs
and manually requested performance reports remain available. There is no crash
handler or minidump support: a panic logs its message and backtrace to the log
(stdout when attached to a terminal) and exits the process.

The local log is for error review only; interactive debugging goes through edctl
automation (`automation.md`). The `log` backend is a small module in the `ed`
binary, not a separate crate. The editor logs at a fixed `info` level (errors only
for `usvg`/`simplecss`). There is no `log` setting and no `ED_LOG`/`RUST_LOG`
filtering.

The application updater, update checks, downloads/installers, helper executable,
release-note actions, and update indicators are removed. Updates are installed
externally. The obsolete extension auto-update setting is also removed.

Feature flags and local flag overrides are removed.
Diff-review gutter controls and intentional panic/crash
actions are disabled and cannot be enabled through settings or environment variables.

Feedback actions, bug-report links, and feature-request/email menu entries are
removed.

The editor has a single application identity and icon. There is no
release-channel concept. The app
version is the `ed` crate's Cargo.toml version; no commit SHA, build ID, or
bundle type is embedded or shown.

The ztracing instrumentation and Tracy integration are removed. The GPUI
`profiler` feature is removed too: no task/frame timing collection, performance
profiler panel or `instrumentation.performance_profiler` setting, FPS/frame debug
overlay, input-latency histogram, or hang monitor. GPUI has no
`profiling` crate instrumentation (no puffin/optick/superluminal/tracy hooks).
Diagnostic logs remain available.

Editor code has no direct `tracing` dependency; warnings and errors use `log`
and the local logging pipeline. Third-party dependencies may still use `tracing` internally.
