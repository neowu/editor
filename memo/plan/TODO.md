- no support for hard click touchpad (lookup), add or put right click menu

# UI
- add workspace scoped UI settings, like title or border color, do i really need this? to distinguish project

# code style 
- add complete clippy lint

- review async crates, replace smol with tokio? (as smol dev seems to be stopped)

# should add low power mode? or my laptop is powerful enough
- Frame pacing (#63477, #66237): large changes to window.rs, and the macOS Low Power Mode part drags in Linux and Windows power code. I’d skip them unless you’ve noticed stutter in Low Power Mode.

# support to open in restricted mode? like not starting LSP?

# picker
- unify command palette and file picker and tab switcher? so i can use cmd+T on terminal mode

- support run http script (shell script is supported) in markdown? or just something like httpie or curl is good enough

# keyboard shortcuts
- rethink cmd+N, and "+" icon really needed
- make cmd+T to open terminal as others?

# close when task was running, like something in terminal, it should ask if we want to close
