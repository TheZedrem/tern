---
status: now
blocked-by: []
---
# T-004: Command registry, keybindings and palette

Config-facing IDs for every action (T-003 already binds the tab and split
defaults), user keybinding overrides with notices, the palette with fuzzy
search and the provider interface, built on `gpui-component`'s input and
list. Includes spec 0005's palette parts: "Open profile" entries, "Open
config file" (its `OpenConfig` action landed with T-005, unbound), and the
theme picker with `theme::list()`, live preview and the comment-preserving
write. The font-size keys need OQ-015 settled first. Area: src.

Spec: ../spec/0004-commands-keybindings-palette.md
Spec: ../spec/0005-config-and-themes.md (palette parts, `keybindings`)
