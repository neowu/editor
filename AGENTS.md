# Overview

this is Zed fork and trim down everything i don't need

always value performance / efficiency / simplicity over strictly correctness / defensive design, let me know if there is any trade off to make

# code style

- Keep comments minimal and focused on intent or non-obvious behaviour
- Prefer straightforward code over unnecessary encapsulation and abstraction
- Format Rust code with `cargo +nightly fmt --all`

# spec

- update `/spec` to reflect what code and design changed
- for spec doc, only keep key design decisions / behaviour / requirements, always goes to code for impl details

# unit test

- add unit tests for every function added or changed

# automation ui test

- follow `spec/automation.md`
