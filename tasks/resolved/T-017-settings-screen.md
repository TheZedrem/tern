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

## Result (2026-11-05)

TTFP warm median 347.3 ms (p90 357.2), budget 500 ms; the boot path only
gained the screen's Esc binding (T-002 measured 294.5 ms on a quieter
run).

Built: "Open settings" (`settings.open`) shows the screen over the window,
Tabby-style: tabs down the left (General, Appearance, Terminal, Keyboard,
Profiles), settings grouped under headings, and a search field, focused
on open, that finds settings, keybindings and profiles across all tabs by
label and keywords. Every config key has a field: font (family from the
installed fonts, size, line height), theme, cursor style and blink,
scrollback, close on exit, open links, copy on select, restore session,
the default profile; keybindings are recorded through a keystroke
interceptor, so a bound combination is caught before it runs; profiles
are added, edited and deleted (no password field). `src/settings/yaml.rs`
edits `config.yaml` line by line, keeping comments, and `Change::edited`
refuses an edit that wouldn't read back as asked; the write is
`config::save_edit`, which the theme picker now shares. The owner's
standing rule is in AGENTS.md's Definition of Done, and a test fails when
a template key has no field (`settings::FIELDS`).

Verified: 405 tests, 26 new from a separate agent (one caught a refusal
notice not naming `keybindings`, fixed). The owner found the font size
field couldn't be typed into: the screen's backdrop let clicks through to
the terminal below, which took the keyboard; it now blocks them.

The owner then checked the running app on Linux after the fix: all good.

Not verified: Windows (T-011).
