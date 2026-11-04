---
status: next
blocked-by: []
---
# T-002: Finish the terminal pane

Mouse selection and clipboard (with the primary selection and middle-click
paste on Linux), copy-on-select option, scrollback
navigation, inline search bar on `gpui-component`'s input, OSC 8 and
detected links, cwd tracking (OSC 7 + `/proc` fallback) with the opt-in
bash/zsh/pwsh snippets under `shell-integration/` (OQ-008), IME input, and
killing every child on quit. Exit handling per `close_on_exit` already
landed with T-010. Area: src.

Spec: ../spec/0002-terminal-pane.md
