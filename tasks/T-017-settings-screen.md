---
status: later
blocked-by: [spec 0009 approval]
---
# T-017: Graphical settings screen

A settings screen for editing `config.yaml` without opening the file:
fonts, theme, keybindings and profiles, applied live through the same
hot-reload path. Built as a GPUI view with `gpui-component` widgets, not
a webview (OQ-010). Spec 0009 pins it (drafted 2026-11-05, for the
owner's approval): in-place edits of the changed key only, generalizing
`save_theme_default`, and no password field. Area: src.

Spec: ../spec/0009-settings-screen.md
Spec: ../spec/0005-config-and-themes.md
