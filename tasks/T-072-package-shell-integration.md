---
status: next
blocked-by: []
---
# T-072: Install the shell-integration snippets with the packages

The `.deb`, `.rpm`, AppImage and MSI carry `shell-integration/tern.bash`,
`tern.zsh` and `tern.ps1` (spec 0002, cwd; OQ-008) at a fixed, documented
path (e.g. `/usr/share/tern/shell-integration/` on Linux, next to
`tern.exe` on Windows), so users who installed a package can source them
without a clone. Still opt-in: nothing is added to an rc file. README's
Shell integration section names the paths. Area: packaging.

Spec: ../spec/0002-terminal-pane.md
