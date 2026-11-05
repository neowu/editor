- no support for hard click touchpad (lookup), add or put right click menu

- add webkit tab to render html? still need markdown html preview? or just use webkit to open html, then how to open remote html? 

- remove settingContent entirely? just hard code into code

- add workspace scoped UI settings, like title or border color, do i really need this? to distinguish project

- add complete rust / clippy lint

- review async crates, replace smol with tokio? (as smol dev seems to be stopped)

- think about remote "download" vs local "open in default app", copy/paste file across local/remote

# should add low power mode? or my laptop is powerful enough
- Frame pacing (#63477, #66237): large changes to window.rs, and the macOS Low Power Mode part drags in Linux and Windows power code. I’d skip them unless you’ve noticed stutter in Low Power Mode.

# support to open in restricted mode? like not starting LSP?

# lsp
- display "not installed", e.g. for terraform

# git
- smart pull (like stash then pull)

# mermaid
- pick the diagrams will use

# picker
- i probably do not need preview and action
