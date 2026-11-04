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
landed with T-010. Also the config keys `copy_on_select` and `cursor`
(style and blink), which T-005 accepts but doesn't read yet; they apply
live on reload. Copy, paste and search become commands in
`src/commands/` (IDs, palette titles, the `Ctrl+Shift+C/V/F` defaults), so
spec 0004's AC7 holds. Area: src.

Spec: ../spec/0002-terminal-pane.md
Spec: ../spec/0004-commands-keybindings-palette.md (copy, paste, search commands)
Spec: ../spec/0005-config-and-themes.md (`copy_on_select`, `cursor`)
