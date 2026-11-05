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

## Result (2026-11-05)

TTFP warm median 294.5 ms with one pane (p90 305.6, cold 425.2) and
319.3 ms with a restored 4-pane tab (p90 347.5), budget 500 ms; T-006
measured 293.7 and 304.3 ms. Nothing new runs before the first frame;
link detection builds on first hover.

Built, to threat model 0002 (T-021):
- Mouse selection (drag, word on a double click, line on a triple), the
  primary selection and middle-click paste on Linux, `copy_on_select`.
- `pane.copy`/`pane.paste`/`pane.search` on `Ctrl+Shift+C/V/F`: copy
  drops zero-width and direction characters, paste drops control
  characters and is bracketed when the program asked.
- Scrollback by wheel and `Shift+PageUp/PageDown`; in full-screen
  programs the wheel sends arrow keys.
- The search bar (case and regex toggles, Enter and Shift+Enter, all
  matches on screen highlighted).
- OSC 8 and detected `http`/`https` links, underlined on hover with the
  target shown, opening on a plain click by the owner's call (Tabby's
  Ctrl+click annoyed), or with `open_links: ctrl_click`.
- IME preedit at the cursor; `cursor` style and blink; OSC 52 off at the
  parser and every reply fixed text; panes stop when the window closes.

Spec changes: links open on a click (spec 0002 flow 7 and AC8,
architecture §Security 2, TM-escape-02, spec 0005); OSC 7 cwd tracking
and the shell-integration snippets moved to T-022, which needs `polling`
as a direct dependency (OQ-018).

Verified: 379 tests, 36 new from separate agents against the specs and
the threat model. They caught three bugs, all fixed: unknown `cursor` keys
had no notice, a `\` could hide a link's real host, and Ctrl+double-click
opened a link twice.
The debug build boots and renders a restored 4-pane tab.

Not verified: everything the mouse, keyboard and an IME do in the running
window (the owner tests at the end); quitting while a program ignores
SIGHUP; Windows (added to T-011).
