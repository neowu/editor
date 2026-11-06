# Code style

- Keep comments minimal and focused on intent or non-obvious behaviour.
- Prefer straightforward code over unnecessary encapsulation and abstraction.
- Format Rust code with `cargo +nightly fmt --all`.
- Use explicit type names for concrete types, including signatures, constructors,
  struct fields and patterns, enum variants, and associated function or constant
  paths. This keeps uses searchable by type name and readable without the
  surrounding implementation. Include generic arguments where required.
- Keep `Self` in trait definitions and provided trait methods, where it denotes
  the implementing type, and in associated type projections such as
  `Self::Target` and `Self::Output`. Use explicit names inside concrete trait
  implementations wherever these exceptions do not apply. Generated code follows
  the same convention.
- Prefer explicit code over convenience macros: write an immediately-invoked
  closure (`(|| { ... })()`) or an `async` block directly to scope `?`, rather
  than a `maybe!`-style macro.
- No debug-only checks: no `debug_assert*!` and no `cfg(debug_assertions)`
  invariant checks or debug-only code paths; dev and release run the same
  logic. Verify invariants in unit tests and with `edctl` instead. Use
  `assert!` only when a violation must stop the program in every build;
  otherwise report it with `log::error!` and recover.
  Dev-build switches are limited to the single-instance bypass and rust-embed
  loading assets and grammar queries from the source tree at runtime (no
  `debug-embed`), so debug binaries only run from this checkout; release
  builds embed them.
