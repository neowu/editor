# External crate versions

Question: are all external crates on their latest release, and which ones look
unmaintained (no crates.io release for more than a year)?

Snapshot: 18 Nov 2026. Dates are the crates.io publish date of the latest
stable release, which is also the version in `Cargo.lock` after this update.
"Repo" dates are the last commit on the default branch (GitHub API), used to
tell "finished" from "abandoned". ⚠ = no release since 18 Nov 2025.

## Updated in this pass

| Crate | From | To | API change needed |
|---|---|---|---|
| any_vec | 0.14 | 0.15 | none |
| base64 | 0.22 | 0.23 | none |
| chardetng | 0.1 | 1.0 | `EncodingDetector::new(Iso2022JpDetection::Allow)`, `guess(_, Utf8Detection::Allow)`; same behaviour as 0.1 |
| convert_case | 0.11 | 0.12 | none |
| etagere | 0.2 | 0.3 | none |
| fancy-regex | 0.19 | 0.20 | none |
| linkify | 0.10 | 0.11 | none |
| parking_lot | 0.12 | 0.13 | none (`alacritty_terminal` still pulls 0.12) |
| resvg / usvg | 0.48 | 0.50 | none |
| sha2 | 0.10 | 0.11 | none |
| syn | 2 | 3 | `derive_refineable` builds its `T: Clone` where-predicate with `parse_quote!` instead of hand-built `PredicateType`/`TraitBound` |
| sysinfo | 0.39 | 0.40 | `System::new()` returns `Result`; infallible on macOS, so `expect` |
| taffy | =0.13.0 | 0.15 | alignment fields are no longer `Option` (unset maps to `NORMAL`); measure closure now gets `LayoutInput` and must call `compute_leaf_layout` itself. Pin removed |
| toml | 0.9 | 1.2 | none |
| tree-sitter-css | 0.23 | 0.25 | none |
| alacritty_terminal (git) | `5421470` (2026-10-16) | `aa5d942` (2026-11-06) | none |
| async-task (git) | `41fd335` | `3aea7f5` (2026-11-14, adds `Runnable::is_cancelled`) | none |

Trade-offs:

- syn 3 adds a second syn: `derive_more`, `pin-project`, `rust-embed`, `strum`
  and `cbindgen` still depend on syn 2, so both compile (build time only).
- parking_lot 0.12/0.13 is duplicated for the same reason. Small, but it
  costs binary size until `alacritty_terminal` moves.
- taffy 0.14/0.15 reworked alignment (`AlignItems` is now a keyword + safety
  struct). Unit tests pass, but layout regressions are only visible in the
  app; worth a visual check of panels, pickers and the terminal.

## All direct external crates

| Crate | Latest | Released |
|---|---|---|
| aho-corasick | 1.1.5 | 2026-08-03 |
| any_vec | 0.15.0 | 2025-08-25 ⚠ |
| anyhow | 1.0.104 | 2026-07-18 |
| arc-swap | 1.9.5 | 2026-11-09 |
| async-channel | 2.5.0 | 2025-07-06 ⚠ |
| async-lock | 3.5.0 | 2026-11-02 |
| async-trait | 0.1.92 | 2026-08-08 |
| base64 | 0.23.1 | 2026-08-04 |
| bitflags | 2.13.2 | 2026-09-10 |
| block2 | 0.6.2 | 2025-10-04 ⚠ |
| cbindgen | 0.29.4 | 2026-06-09 |
| chardetng | 1.0.0 | 2026-03-30 |
| clap | 4.6.8 | 2026-11-16 |
| convert_case | 0.12.0 | 2026-08-27 |
| derive_more | 2.1.1 | 2025-12-22 |
| dispatch2 | 0.3.1 | 2026-02-26 |
| emojis | 0.9.1 | 2026-10-17 |
| encoding_rs | 0.8.44 | 2026-10-30 |
| etagere | 0.3.0 | 2026-03-18 |
| fancy-regex | 0.20.2 | 2026-11-11 |
| futures | 0.3.34 | 2026-08-11 |
| globset | 0.4.21 | 2026-10-27 |
| heapless | 0.9.3 | 2026-04-30 |
| hex | 0.4.3 | 2021-03-03 ⚠ |
| ignore | 0.4.34 | 2026-10-27 |
| image | 0.25.10 | 2026-03-10 |
| imara-diff | 0.2.0 | 2025-06-14 ⚠ |
| indexmap | 2.14.2 | 2026-09-05 |
| inventory | 0.3.25 | 2026-10-07 |
| itertools | 0.15.0 | 2026-06-16 |
| libc | 0.2.191 | 2026-10-16 |
| linkify | 0.11.0 | 2026-04-12 |
| log | 0.4.35 | 2026-10-30 |
| lru | 0.19.0 | 2026-10-18 |
| lyon | 1.0.19 | 2026-03-08 |
| mimalloc | 0.1.52 | 2026-05-22 |
| nucleo-matcher | 0.3.1 | 2024-02-20 ⚠ |
| num_cpus | 1.17.0 | 2025-05-30 ⚠ |
| objc2 | 0.6.5 | 2026-10-05 |
| objc2-app-kit | 0.3.2 | 2025-10-04 ⚠ |
| objc2-core-foundation | 0.3.2 | 2025-10-04 ⚠ |
| objc2-core-graphics | 0.3.2 | 2025-10-04 ⚠ |
| objc2-core-text | 0.3.2 | 2025-10-04 ⚠ |
| objc2-foundation | 0.3.2 | 2025-10-04 ⚠ |
| objc2-metal | 0.3.2 | 2025-10-04 ⚠ |
| objc2-quartz-core | 0.3.2 | 2025-10-04 ⚠ |
| ordered-float | 5.5.1 | 2026-10-25 |
| parking | 2.2.1 | 2024-09-07 ⚠ |
| parking_lot | 0.13.0 | 2026-10-21 |
| percent-encoding | 2.3.2 | 2025-08-21 ⚠ |
| pin-project | 1.1.13 | 2026-05-13 |
| proc-macro2 | 1.0.107 | 2026-07-19 |
| pulldown-cmark | 0.13.6 | 2026-10-30 |
| quote | 1.0.47 | 2026-07-19 |
| rand | 0.10.4 | 2026-11-15 |
| raw-window-handle | 0.6.2 | 2024-05-17 ⚠ |
| regex | 1.13.1 | 2026-07-15 |
| resvg | 0.50.0 | 2026-11-11 |
| rust-embed | 8.14.0 | 2026-11-11 |
| rustc-hash | 2.1.3 | 2026-07-02 |
| rustix | 1.1.6 | 2026-10-27 |
| seahash | 4.1.0 | 2021-01-16 ⚠ |
| serde | 1.0.229 | 2026-07-18 |
| serde_json | 1.0.154 | 2026-10-11 |
| sha2 | 0.11.1 | 2026-10-12 |
| shlex | 2.0.1 | 2026-05-17 |
| slotmap | 1.1.1 | 2025-12-06 |
| smallvec | 1.16.5 | 2026-11-06 |
| smol_str | 0.3.7 | 2026-10-20 |
| spellbook | 0.4.2 | 2026-06-03 |
| strum | 0.28.0 | 2026-02-22 |
| syn | 3.0.10 | 2026-11-09 |
| syntect | 5.3.0 | 2025-09-27 ⚠ |
| sysinfo | 0.40.4 | 2026-11-16 |
| taffy | 0.15.1 | 2026-10-25 |
| tempfile | 3.27.0 | 2026-03-11 |
| thiserror | 2.0.21 | 2026-09-23 |
| time | 0.3.55 | 2026-08-01 |
| toml | 1.2.1 | 2026-11-06 |
| tree-sitter | 0.27.2 | 2026-11-07 |
| tree-sitter-bash | 0.25.1 | 2025-12-02 |
| tree-sitter-css | 0.25.0 | 2025-09-28 ⚠ |
| tree-sitter-go | 0.25.0 | 2025-08-29 ⚠ |
| tree-sitter-html | 0.23.2 | 2024-11-11 ⚠ |
| tree-sitter-json | 0.24.8 | 2024-11-11 ⚠ |
| tree-sitter-md | 0.5.4 | 2026-10-24 |
| tree-sitter-python | 0.25.0 | 2025-09-11 ⚠ |
| tree-sitter-rust | 0.24.2 | 2026-03-27 |
| tree-sitter-typescript | 0.23.2 | 2024-11-11 ⚠ |
| tree-sitter-yaml | 0.7.2 | 2025-10-07 ⚠ |
| ttf-parser | 0.25.1 | 2024-11-29 ⚠ |
| two-face | 0.5.2 | 2026-08-07 |
| unicase | 2.10.0 | 2026-10-05 |
| unicode-normalization | 0.1.26 | 2026-10-16 |
| unicode-script | 0.5.9 | 2026-11-02 |
| unicode-segmentation | 1.13.3 | 2026-06-01 |
| url | 2.5.8 | 2026-01-05 |
| urlencoding | 2.1.3 | 2023-07-21 ⚠ |
| usvg | 0.50.0 | 2026-11-11 |
| uuid | 1.29.3 | 2026-11-17 |
| vte | 0.15.0 | 2025-02-02 ⚠ |
| waker-fn | 1.2.0 | 2024-05-13 ⚠ |
| which | 8.0.6 | 2026-08-26 |
| alacritty_terminal (git) | `aa5d942` | 2026-11-06 |
| async-task (git) | `3aea7f5` (crates.io 4.7.1 is 2024-04-27) | 2026-11-14 |
| framework_lint (git, own) | branch head | - |

## Stale (no release > 1 year): verdict

Grouped by what to do. "Calls" = call sites in this repo.

### Keep: release-stale but the project is alive

| Crate | Last release | Repo | Why keep |
|---|---|---|---|
| objc2-* / block2 | 2025-10-04 | 2026-10-17 | Same repo as `objc2` 0.6.5 (2026-10-05); framework crates release in batches. No alternative. |
| async-channel | 2025-07-06 | 2026-07-13 | smol-rs, feature-complete. |
| percent-encoding | 2025-08-21 | 2026-11-11 | rust-url (servo); `url` released 2026-01. |
| vte | 2025-02-02 | 2026-11-17 | alacritty; `alacritty_terminal` depends on it, so it stays regardless. |
| syntect | 2025-09-27 | 2026-10-31 | Active on main; no tree-sitter-free alternative for the long tail of languages. |
| imara-diff | 2025-06-14 | 2026-06-21 | 0.2 is a recent rewrite, used by helix/gitoxide. |
| nucleo-matcher | 2024-02-20 | 2026-06-22 | helix maintains it; matcher is done. Watch it. |
| raw-window-handle | 2024-05-17 | 2026-04-05 | Interface crate, intentionally frozen at 0.6. |
| tree-sitter-json / -yaml | 2024-11 / 2025-10 | 2026-08 / 2026-05 | Grammars change rarely; repo active. Could pin a git rev for newer grammar. |
| tree-sitter-css / -go / -python | 2025-08..09 | 2025-09 | Same: grammar is stable, the crate tracks the tree-sitter ABI. Re-check next year. |

### Replaced (done 2026-11-18)

These were small and easy to drop, so they are gone (together with the version bumps, Cargo.lock goes from 462 to 454
packages). `parking` stays as a transitive dependency of `event-listener`.

| Crate | Last release | Replaced with |
|---|---|---|
| urlencoding | 2023-07-21 | `percent_encoding::percent_decode_str(..).decode_utf8()` (now a workspace dep) |
| hex | 2021-03-03 | `to_hex` helper in `task_template.rs` |
| num_cpus | 2025-05-30 | `std::thread::available_parallelism()` |
| waker-fn + parking | 2024-05 / 2024-09 | `executor::block` parks the current thread with a `Wake` impl that `unpark`s it; `util::block_on` reuses it |
| seahash | 2021-01-16 | `rustc_hash::FxHasher` for the clipboard text hash |
| any_vec | 2025-08-25 | `workspace::searchable::AnyMatches` (`Box<dyn Any>` over a `Vec<T>` plus its length); `replace`/`replace_all` take the match list and an index |
| ttf-parser | 2024-11-29 | `skrifa` (same version usvg already uses) for the emoji glyph check |

Trade-offs:

- The clipboard text hash is stored in the pasteboard and compared on paste.
  FxHasher is unseeded, so it is stable between running instances of the same
  build, but its output could change with a `rustc-hash` upgrade. The effect is
  only that paste metadata from an older build gets ignored once.
- `dirs` / `shellexpand` (not stale, dropped 2026-11-24 for size: 7 fewer
  packages, 438 total): home dir is `std::env::home_dir()`, tilde expansion is
  `util::paths::expand_tilde`, task template substitution is a `$ED_NAME`-only
  scanner in `task_template.rs` (see `spec/tasks.md`).
- `std::thread::park` shares one token per thread. A future that itself calls
  `thread::park` inside `poll` could swallow the wake, but nothing in the
  codebase does that. `futures::executor::block_on` works the same way.

### Watch: grammar crates without upstream activity

| Crate | Last release | Repo | Note |
|---|---|---|---|
| tree-sitter-html | 2024-11-11 | 2025-09-15 | Fine while the grammar ABI stays compatible with tree-sitter 0.27. |
| tree-sitter-typescript | 2024-11-11 | 2025-01-30 | Oldest grammar. If a tree-sitter ABI bump breaks it, switch to a git rev or vendor `src/parser.c`. |
