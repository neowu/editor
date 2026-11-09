# tombi bugs

## Completion edit range is empty when a token follows the cursor

- Version: tombi 1.11.1
- Buffer (Cargo.toml): `smol = {v<cursor>"2.0"`
- Expected: accepting `version` replaces the typed `v` → `smol = {version"2.0"`
- Actual: `smol = {vversion"2.0"`

tombi sends a plain `textEdit` with an empty range at the cursor (`5:9..5:9`), so the typed `v` is not replaced. Without the trailing `"2.0"` (`smol = {v<cursor>`), the range correctly covers the prefix (`5:8..5:9`). It looks like tombi parses `v"2.0"` as one broken token instead of a key prefix.

The editor applies the server's range as-is, as the LSP spec requires. The menu still looks correct because the editor filters suggestions by the word before the cursor.

Possible editor workaround (not implemented): in `process_completion_for_edit` (`crates/editor/src/editor.rs`), when the server range is empty and the word before the cursor is a prefix of the label, extend the range back over that word.
