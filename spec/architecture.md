# Architecture — zed-term

**Single source of truth for the stack.** Every architecture-relevant
decision lives here; changing one means editing this file. `docs/` may link
here but never restates decisions. Anything not yet decided is an entry in
[`open-questions.md`](open-questions.md).

## Product summary

zed-term is a desktop terminal emulator with built-in SSH/SFTP and one-click
AI coding agents (Claude Code, Hermes Agent, …) — Tabby's feature set, minus
Tabby's boot time. It is for developers who live in terminals and remote
hosts and are tired of waiting for Electron. The constraint that shapes
everything: **a usable shell prompt in under 150 ms** from launch, the way a
native terminal feels. It is a local, single-user app: no accounts, no
backend, no telemetry.

## Roadmap

| Phase | Scope | Specs |
|---|---|---|
| 1 — Core terminal | Local shell and local profiles, tabs + splits, shortcuts, clipboard, scrollback, search, command palette, YAML config + themes, session restore | 0001–0005 |
| 2 — SSH / SFTP | `ssh` profile type, host manager UI (groups), `~/.ssh/config` import, connect via system `ssh` in a PTY, SFTP panel, secrets in the OS keychain | not yet written |
| 3 — AI agents | Agent profiles (command, cwd, optional SSH host), one-click launch into tab/pane, running-agents overview, "waiting for input" notification | not yet written |
| 4 — ACP | Structured agent integration via the Agent Client Protocol (diffs, plans, tool-call approvals) — keep the architecture open for it, build nothing yet | — |

Only the current phase gets specs and tasks. Later phases live here as one
row until they start.

## Platforms

Linux and Windows are **required**: every feature ships working on both.
macOS is **compile-only**: keep the code building there, with no CI builds,
no signing, and no blocking on it.

## Stack

Pure Rust, one process, one binary. No webview: the Tauri + WebKitGTK
version measured ~1.07 s to first prompt on the reference machine, of which
~800 ms was the webview; a native window measured 68 ms (OQ-009, OQ-010).

| Layer | Choice | Why |
|---|---|---|
| Language | Rust (edition 2024), single crate at the repo root | One language, one build, no bundler and no IPC |
| Window + UI | `eframe` / `egui` (immediate mode) with the `wgpu` backend | Smallest API that gives a window, input, IME, clipboard and simple widgets on Linux and Windows; boots in tens of ms. `glow` (OpenGL) is the fallback backend if `wgpu` init costs > 50 ms of boot |
| Terminal state | `alacritty_terminal` | The VT parser, grid, scrollback, selection, search and OSC handling behind Alacritty and Zed; we don't write a VT parser |
| PTY | `alacritty_terminal::tty` + its `event_loop` | Spawns the shell (Unix PTY, ConPTY on Windows) and reads/parses on its own thread; rendering reads the bounded grid, so a flood can't grow memory |
| Terminal rendering | Our own `egui` widget: one text galley per visible row, monospace font, per-cell colors | Simplest thing that renders a grid; egui caches galleys by content. A glyph-atlas mesh is the fallback if a full-screen `cat` at 4K drops frames |
| Fonts | One bundled monospace font (default); `font.family` from config resolved via `font-kit` (fontconfig / DirectWrite) | Boot never scans font directories; a system font costs one cached query, only when configured |
| Config format | YAML, parsed with `serde-saphyr` | Owner's choice. Pure Rust, never panics while parsing, maintained (`serde_yaml` is archived) |
| SSH (Phase 2) | System `ssh` binary spawned in a PTY | Owner's choice. Uses the user's `~/.ssh/config`, ProxyJump, agent, FIDO keys and known_hosts, with no crypto code of our own |
| SFTP (Phase 2) | `ssh -s <host> sftp`, with the `russh-sftp` client speaking SFTP over its stdio | Same ssh config, agent, keys and known_hosts as terminal sessions; no SSH code of our own |
| Secrets (Phase 2) | OS keychain via the `keyring` crate | Never in plain files |
| Package manager | cargo | |
| Hosting / CI | GitHub + GitHub Actions | Hosted Linux and Windows runners. TTFP isn't gated in CI (shared runners are too noisy); the bench runs locally |

## Code layout

One crate. `src/main.rs` is the boot path and does only what the first
frame needs. Everything else is a module under `src/`:

- `terminal/` — the pane: PTY lifecycle, grid widget, input mapping, search
- `layout/` — tabs, split tree, session restore
- `commands/` — command registry, keymap, palette
- `config.rs`, `theme.rs`, `session.rs` — files on disk
- `timing.rs` — TTFP marks and the bench

Modules are plain Rust that is testable without a window; the `egui`
widgets are thin over them.

## Key decisions and their costs

Load-bearing rules that code elsewhere depends on, plus the price we knowingly pay.

- **The UI thread never waits for a PTY.** Each pane's `alacritty_terminal`
  event loop runs on its own thread and owns the PTY. The grid sits behind
  alacritty's fair mutex; the UI thread locks it only to render a frame and
  to hand over input. A PTY started before the window exists just keeps
  parsing; the first frame renders whatever has arrived.
  *Cost:* rendering is a snapshot. Output that arrives between frames is
  only seen on the next repaint, which the event loop requests.
- **Immediate mode.** `egui` rebuilds the UI every frame from state, and
  repaints only on input or when a PTY asks. There is no retained widget
  tree, so tabs, splits and the palette are plain functions over the layout
  model. *Cost:* every frame re-lays-out the visible rows; cheap for a
  terminal, but keep per-frame work bounded by what is visible.
- **One bundled font on the boot path.** The default font is embedded in
  the binary, so the first frame needs no font lookup. *Cost:* binary size
  (~100 KB per face), and a configured family adds one system query.
- **Fixed window chrome.** The window is undecorated-agnostic: we take
  whatever the platform gives (`eframe` default) and never load a desktop
  theme engine; the title bar is the compositor's. *Cost:* no custom title
  bar in Phase 1.
- **A broken config never blocks boot.** Fall back to defaults and show the
  parse error with its line number. YAML's implicit typing (`no` → false) is
  contained by typed serde structs.
- **System `ssh` hides connection state.** We see bytes, not events:
  "connected", "auth failed" and password prompts must be inferred from the
  PTY output or the exit code. Stored passwords reach `ssh` via
  `SSH_ASKPASS` (our binary as the askpass helper), never by typing into the
  PTY. Requires an `ssh` binary (shipped with Windows 10+, but it can be
  disabled).

## Data stores

All local, all plain files the user can read — nothing leaves the machine.

| Store | Location | Contents | Retention |
|---|---|---|---|
| Config | `<config_dir>/zed-term/config.yaml` | Settings, profiles, keybindings, inline themes | Until the user edits/deletes it |
| User themes | `<config_dir>/zed-term/themes/*.yaml` | Color schemes | Same |
| Session state | `<data_local_dir>/zed-term/session.json` | Window size, tab/split layout, each pane's profile, cwd and title | Overwritten on every save; never scrollback |
| Secrets (Phase 2) | OS keychain | SSH passwords / key passphrases | Until the host profile is deleted |

`<config_dir>`/`<data_local_dir>` come from the `dirs` crate (XDG on
Linux; `%APPDATA%` and `%LOCALAPPDATA%` on Windows, so dotfile-style config
roams but machine-specific cwd paths don't). Scrollback is never written to
disk — it can contain secrets the user typed or printed.

## Cross-cutting rules

- **Startup budget (load-bearing).** Time-to-first-prompt (TTFP) < 150 ms
  warm median on a release build on the reference machine. The boot path
  is: `main` → read config and session → spawn the active tab's PTYs →
  create the window → first frame with output. Everything else happens
  after that frame. Full rules and measurement in
  [spec 0001](0001-startup-performance.md).
- **Lazy by default.** A feature that is not needed for the first frame is
  initialized on first use or after the first frame — never before it.
  "It's only a few ms" is how boot paths rot.
- **Errors.** Fallible operations return `Result<T, String>` with a
  human-readable message; the UI shows it in the affected pane, never a
  modal at boot.
- **Notices.** Non-fatal problems (broken config, skipped profile, bad
  keybinding, failed write) become notices: plain-text toasts stacked in
  the window's bottom-right corner, dismissed by click or after 10 s. Boot
  notices are collected before the window exists and shown on the first
  frame. A notice never takes keyboard focus from the pane.
- **ACP readiness (Phase 4).** A pane's content source is an abstraction
  (a terminal today), so a future structured agent view can live in a pane
  without rewriting layout or tabs.

## Ground rules (binding)

1. **No network at startup.** No update checks, fonts, analytics, or
   anything else over the network before the first prompt; deferred work
   waits until after the first frame. The only outbound traffic is what the
   user explicitly starts (SSH, agents).
2. **No telemetry.** Nothing about the user's sessions, hosts, or commands
   is collected or sent anywhere.
3. **Secrets live in the OS keychain only** — never in `config.yaml`,
   `session.json` or logs.
4. **Self-contained binary.** The default font and icons are embedded.
   Nothing is fetched from a CDN.
5. **Data minimization.** Session restore stores window size, layout and
   cwd, not scrollback or command history.

## Security (binding)

The trust boundaries every spec checks against. A spec that crosses one
says how it handles it in its **Security & privacy** section.

1. **Escape sequences are untrusted input.** Any program or remote host can
   emit OSC 52 (clipboard write), OSC 8 (links) and title sequences. Each one
   we honor is an explicit decision in a spec. Clipboard writes from the
   terminal are off unless enabled. Terminal-supplied strings (titles,
   links, filenames) are rendered as plain text and never interpreted.
2. **Opening things.** Only `http`/`https` links open in the OS handler,
   and only on an explicit Ctrl/Cmd+click. The app never executes a path
   or command that came from terminal output.
3. **Credentials (Phase 2).** Keychain only. Passwords reach `ssh` via
   `SSH_ASKPASS`, never through env vars, command-line args, logs or the
   PTY.
4. **Agents (Phase 3).** Agent profiles run arbitrary commands locally or
   over SSH. Launching one is always an explicit user action, never
   triggered by terminal output.
