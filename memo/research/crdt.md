# Buffer CRDT in a single-user editor

Question: the editor is single-user now (`crates/clock`, `crates/text`). Do we
still need the collaboration CRDT, and is a redesign worth it?

> 2026-11-11: remote projects were removed (no `remote_server`, RPC or proto
> layer), and option A below is done: the local buffer is the only writer.
> `Buffer::branch` / `merge_into_base` and `EditPreview` had no callers and were
> deleted, so no second writer remains. `ReplicaId` is gone, `Lamport` is a
> `u32`, `clock::Global` is one counter, and operations, the deferred queue,
> `wait_for_*`, remote selections and per-edit op history are removed. The
> sections below describe the old design.

## How a remote buffer worked (before removal)

Both the client and the server keep a full copy of each open buffer, and both edit it.

```
 client (ed)                                  server (ed_server)
 ┌────────────────────┐   UpdateBuffer ops    ┌────────────────────┐
 │ Buffer, replica 0  │ ───────────────────▶  │ Buffer, replica 1  │
 │ full text + CRDT   │ ◀───────────────────  │ full text + CRDT   │
 └────────────────────┘   UpdateBuffer ops    └────────────────────┘
```

1. **Typing is applied on the client straight away.** A keystroke edits the
   client copy. That produces an `Operation`, which `Project::on_buffer_event`
   (`crates/project/src/project.rs`) sends as `UpdateBuffer`. The client never
   waits for a round trip.
2. **The server edits its own copy too.** For format-on-save, the client sends
   `FormatBuffers`. `handle_format_buffers` (`crates/project/src/lsp_store.rs`)
   runs the LSP and applies the edits to the server buffer, then sends back ops
   plus a transaction. Code actions, renames and reloads from disk work the same
   way, because the LSP and the files live on the server.
3. **These can overlap.** You can type while a format is in flight, so both
   sides edit at the same time from different versions. The CRDT makes both
   copies end up with the same text without locking.
4. **Requests are tagged with a version.** LSP requests carry the client's
   version vector, and the server calls `wait_for_version` (in
   `lsp_command.rs`) before answering.

Branch buffers (`ReplicaId::LOCAL_BRANCH`, `Buffer::branch` /
`merge_into_base`, used by `crates/editor/src/git.rs`) are another local writer.

## What in `crates/text` is really for multiple writers

| Part | Needed because of… |
|---|---|
| `ReplicaId` in Lamport timestamps, ordering ties between concurrent inserts | several writers |
| `Global` version vectors, the deferred-op queue, `wait_for_version` | several writers, ops arriving out of order |
| `UndoMap` (undo as counts per edit, sent as ops) | undo that works when several writers edit concurrently |
| Fragment tree with tombstones (deleted text kept), `Locator`, insertion ids | **anchors**, even with only one writer |

An `Anchor` is "insertion X, offset Y", resolved lazily against any snapshot.
Tombstones keep old anchors resolvable. Selections, diagnostics, inlays,
multibuffer excerpts, git hunks, the display map and branch buffers all depend
on this. Edits never visit live anchors, which matters for large multibuffers.

## Already done

- The clock is down to three fixed replicas. `Global` is a
  `SmallVec<[u32; 4]>` that never allocates.
- Removed: the unused `SystemClock`, `Lamport::as_u64`, the per-replica
  `deferred_replicas` set, and `Operation::replica_id`. Ops carry their full
  causal version, so the version check alone keeps each source in order.

## Redesign options

**A. One writer, keep the anchor model (medium effort).** The client is the only writer.
The server never edits buffers; it computes edits (format, code actions,
rename, reload diffs) and returns them as plain text edits against a version
number. The client applies them or recomputes if they're stale. The server copy
becomes a mirror that applies ops in order.
- Can go: `ReplicaId`, version vectors (replaced by one `u32`), the deferred
  queue, tie-breaking between concurrent edits, undo as ops (undo becomes local only).
- Must stay: the fragment tree, tombstones, `Locator` and insertion ids, because anchors need them.
- Cost: rewrite how format, code actions, rename and reload work in
  `lsp_store` / `buffer_store`, plus the proto messages. Needs a rule for a
  format result that arrives after more typing: rebase it or drop it.
- Gain: less code and a simpler protocol, but little runtime speed.

**B. Plain rope plus offset anchors, as Helix and VS Code do (very large).**
Anchors become offsets that are shifted on every edit.
- Touches almost every crate (`editor`, `multi_buffer`, display map,
  `language`, `project`, `git`). Every live anchor must be updated per edit or
  tracked through edit history.
- Drops tombstone memory but loses cheap lazy anchor resolution.

**C. Keep as is.** It works, and it costs very little at runtime.

## Recommendation

With remote projects gone, option A's prerequisite (the server stops editing
buffers) is moot: nothing else edits them. What remains of A is deleting the
multi-writer parts: `ReplicaId`, version vectors (one `u32` instead), the
deferred-op queue, tie-breaking between concurrent inserts, and undo as ops.
The fragment tree, tombstones, `Locator` and insertion ids stay, because
anchors need them. Branch buffers (`ReplicaId::LOCAL_BRANCH`) are the one
remaining second writer and need a plan first. It's a code-size cleanup, not a
speed win. B still costs far more than it saves.
