# macos
- no support for hard click touchpad (lookup), add or put right click menu

# support to open in restricted mode? like not starting LSP?

# picker
- unify command palette and file picker and tab switcher? so i can use cmd+T on terminal mode

# run
- support run http script (shell script is supported) in markdown? or just something like httpie or curl is good enough

# git history
select multiple revision, merge the file change into one?

# rust
- crates/editor/src/rust_analyzer_ext.rs:104, expand_macro_recursively, think about how to make it easier to use, and add cmd+click for references
- crates/project/src/lsp_store/lsp_ext_command.rs coupled with rust, should abstraction?

# merge
Picker modals → one crate: language_selector, encoding_selector, lsp_command_selector, go_to_line, command_palette, open_path_prompt, recent_projects (about 4.5k total, each used by one crate).
Previews → one crate: svg_preview, image_viewer, markdown_preview (about 3.9k).
LSP UI: lsp_locations, activity_indicator, diagnostics, possibly with language_tools.
Tiny single-consumer crates into their consumer: assets (31 lines), icons, time_format, trash, refineable.
