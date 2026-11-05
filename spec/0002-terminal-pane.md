---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-010, T-011, T-002]
related: [spec 0005, OQ-003 ✅, OQ-008 ✅, OQ-011 ✅]
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
works in a pane on Linux and Windows 11 without glitches.

## Scope

**In scope:**

- Spawning by profile ID (spec 0005): command, environment, working directory
- Parsing and rendering, input and IME, resize, exit handling
- Copy/paste (plus the primary selection on Linux), scrollback, search, clickable links
- A `command` override on spawn, so Phase 2 (`ssh …`) and Phase 3 (agents) reuse panes unchanged

**Out of scope:**

- Multi-line paste warning, "process still running" close prompt: *Later*
- SSH and agent specifics: Phase 2/3 specs
- Persisting scrollback: never (`architecture.md` §Ground rules)
- Ligatures: *Later*

## Design

### Affected areas

- `src/terminal/` (pane, terminal element, input, search)

### Model

`Pane { id, term, event loop handle, title, cwd?, exit? }`, one GPUI entity
per pane. `alacritty_terminal` owns the VT state (grid, scrollback,
selection) and the PTY; we own the element that draws the grid and the
mapping from keys to bytes. Pane IDs are `u32`, unique per app run.

- **Resolved from config [load-bearing]:** the caller names a profile; the
  pane module resolves command, cwd and env from config (spec 0005).
- **Command:** the profile's `command` if set. Otherwise `$SHELL` (Linux/macOS,
  started as a login shell) or, on Windows, the first one found of `pwsh`,
  `powershell.exe`, `cmd.exe`.
- **Environment:** inherit, then the profile's `env`, plus `TERM=xterm-256color`,
  `COLORTERM=truecolor`, `TERM_PROGRAM=zed-term`. `alacritty_terminal`
  also sets `WINDOWID` and `ALACRITTY_WINDOW_ID`.
- **cwd:** explicit, else the home directory. Tracked live from OSC 7, which
  vte doesn't parse, so zed-term scans the PTY bytes for it itself, with a
  fallback to `/proc/<pid>/cwd` on Linux; unknown on Windows without OSC 7.
  The app ships opt-in shell-integration
  snippets for bash, zsh and pwsh under `shell-integration/`, which the user
  sources from their rc file (OQ-008). Nothing is injected automatically.
- **Glyphs:** the default font is the embedded Lilex. Characters it lacks
  (CJK, emoji, Nerd Font icons) fall back per glyph to installed fonts
  through GPUI's text system (OQ-011).

### Flows

1. **Resize:** the element's bounds ÷ cell size gives cols/rows; the PTY is
   resized only when they change, at most once per frame.
2. **Input:** key events that no action claims (spec 0004) reach the pane,
   which maps them to bytes following xterm conventions and the terminal's
   mode flags (application cursor and keypad, bracketed paste);
   `alacritty_terminal` has no key mapping of its own. With default
   bindings, plain `Ctrl+C`/`Ctrl+X`/`Ctrl+V` reach the shell
   **[load-bearing]** (architecture §Key decisions). The pane implements
   GPUI's input handler, so IME preedit shows at the cursor and committed
   text is written as-is.
3. **Output:** the pane's event loop thread parses PTY bytes into the grid
   and wakes the pane, which redraws on the next frame. Only visible rows
   are shaped, so a flood costs one parse per byte and one layout per
   visible row per frame, and memory is bounded by `scrollback`.
4. **Copy/paste:** `Ctrl+Shift+C` / `Ctrl+Shift+V` (`Cmd+C/V` on macOS)
   through GPUI's clipboard. With default bindings `Ctrl+C` reaches the
   shell. Paste is bracketed when the program asked for it. On Linux every
   selection also becomes the primary selection, and middle-click pastes
   it. Copy-on-select is a config option, default off. Mouse selection is
   `alacritty_terminal`'s (simple, word on double-click, line on triple).
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

Windows 11: ConPTY via `alacritty_terminal::tty`. ConPTY rewrites output and
can reflow-redraw on resize, so verify resize behavior with `vim` on Windows
explicitly. Key mapping differs (no `$SHELL`, AltGr), so the input map is
tested on both. Linux: the primary selection and middle-click paste exist
only there.

### Security & privacy

- Stored: nothing. Output and scrollback live in memory only and are
  dropped when the pane closes. PTY bytes are never logged, even at debug
  level.
- Trust boundaries: crosses §Security 1 and 2, because the pane renders
  untrusted program output. The STRIDE model is
  [threat model 0002](threat-model/0002-escape-sequences.md); decisions:
  - **Titles (OSC 0/2):** honored as plain text (spec 0003).
  - **cwd (OSC 7) [load-bearing]:** honored only in a local pane, as a `file://` URL whose host is empty, `localhost` or this machine's name, naming an existing local directory, at most 4 KiB. An SSH pane's reports set no local cwd (TM-escape-09).
  - **Links (OSC 8 and detected URLs) [load-bearing]:** only `http`/`https` targets, checked on the parsed URL, are links at all, and they open only on Ctrl/Cmd+click, through GPUI's URL opener, never a shell. Hovering shows the target as plain text, marking a host with non-ASCII letters; targets over 2 KiB are ignored (TM-escape-02, -03, -10).
  - **Clipboard (OSC 52) [load-bearing]:** writes are ignored and reads are never answered (TM-escape-04, -05).
  - **Replies:** only fixed strings and numbers go back to the program; none echoes terminal text (TM-escape-06).
  - **Paste:** ESC and every other control character but tab, `\n` and `\r` are dropped, then the text is bracketed when the program asked for it (TM-escape-07). Copy drops zero-width and direction-override characters (TM-escape-12).

## Acceptance criteria

1. Resizing the window while `vim` is open redraws vim at the new size with no stale rows or wrapping artifacts (Linux and Windows 11).
2. `cat` of a 50 MB text file finishes without freezing the UI (tab switching stays responsive during it), the last line shown matches the file's last line, and memory doesn't grow by more than the scrollback.
3. Multi-byte UTF-8 split across read chunks renders correctly, and CJK and emoji render through installed fallback fonts.
4. `Ctrl+C` interrupts `sleep 100`. `Ctrl+Shift+C` copies the selection, `Ctrl+Shift+V` pastes it as one bracketed paste when the program enabled it, and on Linux middle-click pastes the last selection.
5. Search finds a term 5 000 lines up in scrollback, highlights it, and `Esc` returns focus to the shell.
6. A configured nonexistent shell shows an error in the pane instead of a blank pane or a crash. `exit 0` closes the pane under the default config, and `exit 3` leaves it open showing code 3.
7. Quitting the app leaves no child shell processes running.
8. An OSC 8 link to `javascript:alert(1)` or `file:///etc/passwd` does nothing on Ctrl+click, while an `https://` link opens in the browser.
9. With the pwsh snippet sourced on Windows 11, `cd C:\Users` then split: the new pane starts in `C:\Users`.
10. Typing with an IME (e.g. ibus with a CJK layout on Linux) shows the preedit at the cursor and delivers the committed text to the shell.

## Open questions

- None.
