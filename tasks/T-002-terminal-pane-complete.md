---
status: next
blocked-by: [T-001]
---
# T-002: Finish the terminal pane

Clipboard (plugin-backed), copy-on-select option, scrollback, lazy search
bar, links after idle, exit handling per `close_on_exit`, output
backpressure (`pty_ack`), cwd tracking (OSC 7 + `/proc` fallback) with the
opt-in bash/zsh/pwsh snippets under `shell-integration/` (OQ-008), and
killing every child on quit. Area: src-tauri, src.

Spec: ../spec/0002-terminal-pane.md
