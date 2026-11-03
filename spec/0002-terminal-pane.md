---
status: draft        # draft | approved | implemented
phase: 1
tasks: [T-001, T-002]
related: [spec 0005, OQ-003 ✅]
---

# Spec: Terminal pane

Pins one terminal pane end to end: spawning the shell, streaming, resize,
clipboard, scrollback, search, links and exit. Tabs and splits (spec 0003)
are arrangements of these panes.

## Problem & goal

The pane is what users touch 99% of the time. It must feel at least as good
as a native terminal: correct sizing, no garbled output, familiar clipboard
keys, fast search.

Done means: daily driving `vim`, `htop`, `tmux`, `less`, and a large `cat`
works in a pane on Linux and Windows without glitches.

## Scope

**In scope:**

- Local process spawn from a profile (spec 0005), environment, working directory
- Output streaming and input, resize, exit handling
- Copy/paste, scrollback, search, clickable links
- Renderer selection with fallback
- A `command` field on spawn, so Phase 2 (`ssh …`) and Phase 3 (agents) reuse panes unchanged

**Out of scope:**

- Multi-line paste warning, "process still running" close prompt: *Later*
- SSH and agent specifics: Phase 2/3 specs
- Persisting scrollback: never (`architecture.md` §Ground rules)

## Design

### Affected areas

- `src-tauri/src/pty.rs`, `src/terminal/`, `src/features/search/`

### Model

`Pane { id, pty, cwd?, title, exit? }`. The PTY side lives in Rust (writer,
master, child). The view side (xterm instance, addons) lives in the
frontend. Pane IDs are `u32`, unique per app run.

- **Command:** the pane's profile `command` (spec 0005) if set. Otherwise `$SHELL` (Linux/macOS, started
  as a login shell) or, on Windows, the first one found of `pwsh`,
  `powershell.exe`, `cmd.exe`.
- **Environment:** inherit, plus `TERM=xterm-256color`, `COLORTERM=truecolor`,
  `TERM_PROGRAM=zed-term`.
- **cwd:** explicit, else the home directory. Tracked live from OSC 7, with a
  fallback to `/proc/<pid>/cwd` on Linux; unknown on Windows without OSC 7.

### IPC surface

- `pty_spawn({ cols, rows, cwd?, command? }, onOutput: Channel) -> PaneId`
- `pty_attach(id, onOutput: Channel)`: for boot panes and after a page reload
- `pty_write(id, data: string)`, `pty_resize(id, cols, rows)`, `pty_kill(id)`
- `pty_cwd(id) -> string | null`
- Channel messages: raw bytes, or a JSON `{ "exit": code | null }` sent last (architecture §Key decisions)

### Flows

1. **Resize:** a container `ResizeObserver` triggers fit, batched to at most
   one per animation frame. `pty_resize` is called only when cols/rows
   actually change.
2. **Input:** `onData` → `pty_write`. App shortcuts are intercepted before
   xterm (spec 0004). Unbound keys always reach the shell.
3. **Copy/paste:** `Ctrl+Shift+C` / `Ctrl+Shift+V` (`Cmd+C/V` on macOS).
   `Ctrl+C` always reaches the shell. Clipboard goes through
   `tauri-plugin-clipboard-manager`, because WebKitGTK gates
   `navigator.clipboard` reads. Bracketed paste is handled by xterm.
   Copy-on-select is a config option, default off.
4. **Scrollback:** `config.scrollback` lines, default 10 000.
5. **Search:** `Ctrl+Shift+F` opens an inline search bar, a lazy chunk with
   `addon-search`. It supports next/previous, case-sensitive and regex
   toggles, and highlights all matches. `Esc` closes it and returns focus to
   the pane.
6. **Links:** URLs are clickable with Ctrl/Cmd+click and open in the OS
   handler. The addon loads after idle.
7. **Exit:** the pane shows `[process exited with code N]`.
   `config.close_on_exit` (`always | clean | never`, default `clean`) decides
   whether it closes. A clean exit (code 0) closes the pane.
8. **Renderer:** WebGL first. If init fails or the context is lost, dispose
   it and continue on the DOM renderer without losing buffer content.
9. **Spawn failure** (bad shell path, missing binary): the pane shows the
   error and the command it tried, and stays open until closed.
10. **App quit:** every child process is killed, so no orphaned shells
    survive the window.

### Startup impact

xterm core, WebGL and fit are in the entry chunk (spec 0001). Search,
web-links, unicode11 and the clipboard JS wrapper load lazily or after
idle.

### Platform notes

Windows: ConPTY via `portable-pty`. ConPTY rewrites output and can
reflow-redraw on resize, so verify resize behavior with `vim` on Windows
explicitly. Linux: WebKitGTK may refuse WebGL on some GPUs, so the fallback
path is not theoretical.

### Security & privacy

- Stored: nothing. Output and scrollback live in memory only and are
  dropped when the pane closes. PTY bytes are never logged, even at debug
  level.
- Trust boundaries: crosses §Security 1 and 2, because the pane renders
  untrusted program output. Decisions:
  - **Titles (OSC 0/2) and cwd (OSC 7):** honored. Titles are plain text, never HTML (spec 0003).
  - **Links (OSC 8 and detected URLs):** open only on Ctrl/Cmd+click, with the target shown on hover. Only `http`/`https` URLs open.
  - **Clipboard writes (OSC 52):** not handled in Phase 1.

## Acceptance criteria

1. Resizing the window while `vim` is open redraws vim at the new size with no stale rows or wrapping artifacts (Linux and Windows).
2. `cat` of a 50 MB text file finishes without freezing the UI, and the last line shown matches the file's last line.
3. Multi-byte UTF-8 (emoji, CJK) split across read chunks renders correctly.
4. `Ctrl+C` interrupts `sleep 100`. `Ctrl+Shift+C` copies the selection, and `Ctrl+Shift+V` pastes it as one bracketed paste.
5. Search finds a term 5 000 lines up in scrollback, highlights it, and `Esc` returns focus to the shell.
6. A configured nonexistent shell shows an error in the pane instead of a blank pane or a crash.
7. `exit 0` closes the pane under the default config. `exit 3` leaves it open showing code 3.
8. Quitting the app leaves no child shell processes running.
9. Forcing WebGL off still yields a fully working pane on the DOM renderer.
10. An OSC 8 link to `javascript:alert(1)` or `file:///etc/passwd` does nothing on Ctrl+click, while an `https://` link opens in the browser.

## Open questions

- OSC 7 needs shell integration on some shells. Do we ship snippets for bash/zsh/pwsh, or rely on `/proc` on Linux only?
