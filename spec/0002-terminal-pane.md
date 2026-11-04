---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-010, T-002]
related: [spec 0005, OQ-003 ✅, OQ-008 ✅]
---

# Spec: Terminal pane

Pins one terminal pane end to end: spawning the shell, parsing, rendering,
resize, clipboard, scrollback, search, links and exit. Tabs and splits
(spec 0003) are arrangements of these panes.

## Problem & goal

The pane is what users touch 99% of the time. It must feel at least as good
as a native terminal: correct sizing, no garbled output, familiar clipboard
keys, fast search.

Done means: daily driving `vim`, `htop`, `tmux`, `less`, and a large `cat`
works in a pane on Linux and Windows without glitches.

## Scope

**In scope:**

- Spawning by profile ID (spec 0005): command, environment, working directory
- Parsing and rendering, input, resize, exit handling
- Copy/paste, scrollback, search, clickable links
- A `command` override on spawn, so Phase 2 (`ssh …`) and Phase 3 (agents) reuse panes unchanged

**Out of scope:**

- Multi-line paste warning, "process still running" close prompt: *Later*
- SSH and agent specifics: Phase 2/3 specs
- Persisting scrollback: never (`architecture.md` §Ground rules)
- Ligatures, and glyphs beyond egui's built-in Hack and Noto Emoji (CJK, Nerd Font icons): *Later* (OQ-011)

## Design

### Affected areas

- `src/terminal/` (pane, grid widget, input, search)

### Model

`Pane { id, term: alacritty_terminal::Term, event_loop handle, title, cwd?, exit? }`.
`alacritty_terminal` owns the VT state (grid, scrollback, selection,
damage) and the PTY; we own the widget that draws the grid and maps input.
Pane IDs are `u32`, unique per app run.

- **Resolved from config [load-bearing]:** the caller names a profile; the
  pane module resolves command, cwd and env from config (spec 0005).
- **Command:** the profile's `command` if set. Otherwise `$SHELL` (Linux/macOS,
  started as a login shell) or, on Windows, the first one found of `pwsh`,
  `powershell.exe`, `cmd.exe`.
- **Environment:** inherit, then the profile's `env`, plus `TERM=xterm-256color`,
  `COLORTERM=truecolor`, `TERM_PROGRAM=zed-term`.
- **cwd:** explicit, else the home directory. Tracked live from OSC 7 (an
  `alacritty_terminal` event), with a fallback to `/proc/<pid>/cwd` on Linux;
  unknown on Windows without OSC 7. The app ships opt-in shell-integration
  snippets for bash, zsh and pwsh under `shell-integration/`, which the user
  sources from their rc file (OQ-008). Nothing is injected automatically.

### Flows

1. **Resize:** the widget's allocated rect ÷ cell size gives cols/rows; the
   PTY is resized only when they change, at most once per frame.
2. **Input:** our input module maps key events to bytes, following xterm
   conventions and the terminal's mode flags (application cursor and
   keypad, bracketed paste); `alacritty_terminal` has no key mapping of its
   own. App shortcuts are matched first (spec 0004); unbound keys always
   reach the shell, including plain `Ctrl+C`/`Ctrl+X`/`Ctrl+V`
   **[load-bearing]** (architecture §Key decisions). IME commit text is
   written as-is.
3. **Output:** the pane's event loop thread parses PTY bytes into the grid
   and requests a repaint. The widget renders the visible rows as one galley
   each, so a flood costs one parse per byte and one layout per visible row
   per frame, and memory is bounded by `scrollback`.
4. **Copy/paste:** `Ctrl+Shift+C` / `Ctrl+Shift+V` (`Cmd+C/V` on macOS).
   With default bindings `Ctrl+C` reaches the shell. Copy hands the
   selection to egui's clipboard; paste requests the clipboard from egui and
   writes the text it returns. Paste is bracketed when the program asked
   for it. Copy-on-select is a config option, default off. Mouse selection
   is `alacritty_terminal`'s (simple, word on double-click, line on triple).
5. **Scrollback:** `config.scrollback` lines, default 10 000. Mouse wheel
   and `Shift+PageUp/PageDown` scroll; any input snaps back to the bottom.
6. **Search:** `Ctrl+Shift+F` opens an inline search bar. It supports
   next/previous, case-sensitive and regex toggles (`alacritty_terminal`'s
   regex search), and highlights all matches. `Esc` closes it and returns
   focus to the pane.
7. **Links:** OSC 8 hyperlinks and detected URLs are underlined on hover and
   open with Ctrl/Cmd+click in the OS handler, with the target shown in a
   tooltip.
8. **Exit:** the pane shows `[process exited with code N]`.
   `config.close_on_exit` (`always | clean | never`, default `clean`) decides
   whether it closes. A clean exit (code 0) closes the pane.
9. **Spawn failure** (bad shell path, missing binary, unknown profile): the
   pane shows the error and the command it tried, and stays open until closed.
10. **App quit:** every child process is killed, so no orphaned shells
    survive the window.

### Startup impact

A pane adds one thread and one PTY spawn to the boot path (spec 0001). The
search bar and link detection initialize on first use.

### Platform notes

Windows: ConPTY via `alacritty_terminal::tty`. ConPTY rewrites output and
can reflow-redraw on resize, so verify resize behavior with `vim` on Windows
explicitly. Key mapping differs (no `$SHELL`, AltGr), so the input map is
tested on both.

### Security & privacy

- Stored: nothing. Output and scrollback live in memory only and are
  dropped when the pane closes. PTY bytes are never logged, even at debug
  level.
- Trust boundaries: crosses §Security 1 and 2, because the pane renders
  untrusted program output. Decisions:
  - **Titles (OSC 0/2) and cwd (OSC 7):** honored. Titles are plain text (spec 0003).
  - **Links (OSC 8 and detected URLs):** open only on Ctrl/Cmd+click, with the target shown on hover. Only `http`/`https` URLs open.
  - **Clipboard writes (OSC 52):** not handled in Phase 1 (the event is ignored).

## Acceptance criteria

1. Resizing the window while `vim` is open redraws vim at the new size with no stale rows or wrapping artifacts (Linux and Windows).
2. `cat` of a 50 MB text file finishes without freezing the UI (tab switching stays responsive during it), the last line shown matches the file's last line, and memory doesn't grow by more than the scrollback.
3. Multi-byte UTF-8 (emoji, CJK) split across read chunks renders correctly for characters the bundled font covers.
4. `Ctrl+C` interrupts `sleep 100`. `Ctrl+Shift+C` copies the selection, and `Ctrl+Shift+V` pastes it as one bracketed paste when the program enabled it.
5. Search finds a term 5 000 lines up in scrollback, highlights it, and `Esc` returns focus to the shell.
6. A configured nonexistent shell shows an error in the pane instead of a blank pane or a crash. `exit 0` closes the pane under the default config, and `exit 3` leaves it open showing code 3.
7. Quitting the app leaves no child shell processes running.
8. An OSC 8 link to `javascript:alert(1)` or `file:///etc/passwd` does nothing on Ctrl+click, while an `https://` link opens in the browser.
9. With the pwsh snippet sourced on Windows, `cd C:\Users` then split: the new pane starts in `C:\Users`.
10. Typing with an IME (e.g. ibus with a CJK layout on Linux) delivers the committed text to the shell.

## Open questions

- [OQ-011](open-questions.md): font fallback for glyphs the bundled font lacks.
