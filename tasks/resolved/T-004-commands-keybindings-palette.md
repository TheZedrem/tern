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

## Result (2026-11-04)

TTFP warm median 330.6 ms (p90 341.4, cold 412.8), budget 500 ms; T-005
measured 320.8 ms.

Built: `src/commands/`, a registry giving every action an ID and a
palette title; the default keys, with `keybindings:` from config on top
and rebound live on reload (a command the config binds loses its default
keys, a key the config binds leaves the command it was the default of,
and unknown commands or keys that can't be pressed get notices); the
command palette on `Ctrl+Shift+P`, on gpui-component's command list with
our own fuzzy ranking, listing the commands (dimmed when they'd do
nothing), "Open profile" entries and "Open config file", with the
highlighted command's ID underneath; the theme picker (live preview, Esc
reverts, Enter writes only `theme.default` and keeps comments, also
through a linked config); and font size on `Ctrl+=` / `Ctrl+-` / `Ctrl+0`
(OQ-015).

Verified: 198 tests, the new ones written by a separate agent against
spec 0004 and the theme picker parts of spec 0005; the app boots with the
new keymap. Not verified: the palette and the picker in the running
window (no input automation here), and Windows (added to T-011).

Diverged from the spec, which now says so: Esc clears a typed query
before it closes the palette (the widget does that); the font-size keys
take readline's `Ctrl+-` undo, which stays on `Ctrl+_` and `Ctrl+/`
(OQ-015); macOS keeps `Ctrl+Tab` and `Alt+arrows`. Copy, paste and search
join the palette with T-002 (AC7).
