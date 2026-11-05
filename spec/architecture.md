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
everything: **a usable shell prompt in under 500 ms** from launch. It is a
local, single-user app: no accounts, no backend, no telemetry.

## Roadmap

| Phase | Scope | Specs |
|---|---|---|
| 1 — Core terminal | Local shell and local profiles, tabs + splits, shortcuts, clipboard, scrollback, search, command palette, YAML config + themes, session restore | 0001–0005, 0007 |
| 2 — SSH / SFTP | `ssh` profile type, host manager UI (groups), `~/.ssh/config` import, connect via system `ssh` in a PTY, SFTP panel, secrets in the OS keychain | 0006 (SFTP: T-016) |
| 3 — AI agents | Agent profiles (command, cwd, optional SSH host), one-click launch into tab/pane, running-agents overview, "waiting for input" notification | not yet written |
| 4 — ACP | Structured agent integration via the Agent Client Protocol (diffs, plans, tool-call approvals) — keep the architecture open for it, build nothing yet | — |

Only phases that have started get specs and tasks; later phases stay one
row here. Phase 2 started before Phase 1 finished (owner's call,
2026-11-04), so its tasks wait for the Phase 1 tasks they build on.

## Platforms

**Modern platforms only [load-bearing].** zed-term takes no dependency on
deprecated or end-of-life technology. That rules out X11 and XWayland on
Linux and Windows 10, and it rules out adding a dependency just to keep one
of them alive.

| Platform | Support | Notes |
|---|---|---|
| Linux, Wayland session | Required | Any compositor with `xdg-shell`. Needs a Vulkan driver (Mesa or NVIDIA) |
| Windows 11 | Required | DirectX 11, ConPTY |
| macOS | Compile-only | No CI builds, no signing, no blocking on it |
| X11 or XWayland sessions, Windows 10 and older | Not supported | In an X11 session zed-term exits with a clear message (spec 0001) |

## Stack

Pure Rust, one process, one binary, on Zed's UI framework. On the reference
machine a Tauri + WebKitGTK version measured ~1.07 s to first prompt
(OQ-009), a bare native window 68 ms (OQ-010), and GPUI 333 ms warm
(OQ-013).

| Layer | Choice | Why |
|---|---|---|
| Language | Rust (edition 2024), single crate at the repo root | One language, one build, no bundler and no IPC |
| Window, UI, rendering | GPUI, Zed's GPU UI framework, as the exact-pinned `gpui-pre` snapshot of Zed's main branch plus `gpui-pre-platform` built with only its `wayland` feature | Owner's choice: the stack Zed ships. GPU text and layout, IME, clipboard, accessibility, actions and key contexts come with it. It renders through wgpu on Vulkan on Linux, and DirectX 11 with DirectWrite on Windows. The official `gpui` crate (0.2.2, 2025-10) no longer compiles against current dependencies, so the weekly snapshot is the maintained channel |
| Widgets | `gpui-component` with `gpui-kit-assets` (Lucide icons), pinned together with `gpui-pre`; every window's root view is its `Root` | 60+ ready widgets: inputs with IME, lists, tabs, title bar, resizable panes, notifications, tables, trees. They cover the tab bar, palette and toasts now and the SFTP panel in Phase 2. Measured cost: ~9 ms on the first frame and 7.6 MB of binary (OQ-012) |
| Terminal state | `alacritty_terminal` | The VT parser, grid, scrollback, selection, search and OSC handling behind Alacritty and Zed's terminal; we don't write a VT parser |
| PTY | `alacritty_terminal::tty` + its `event_loop` | Spawns the shell (Unix PTY, ConPTY on Windows) and reads and parses on its own thread; the grid is bounded by scrollback, so a flood can't grow memory |
| Terminal rendering | Our own GPUI element: each visible row shaped once as styled runs at the cell width; cell backgrounds, cursor and selection as quads | How Zed's terminal works. Zed's terminal code is GPL, so it is a design reference only; `gpui-terminal` (MIT/Apache) may be reused with attribution |
| Fonts | Lilex (Zed's monospace, OFL) embedded and registered at boot as the default; `font.family` names any installed font | GPUI resolves exact family names only, and falls back per glyph to installed fonts for CJK and emoji |
| Config format | YAML, parsed with `serde-saphyr` | Owner's choice. Pure Rust, never panics while parsing, maintained (`serde_yaml` is archived) |
| SSH (Phase 2) | System `ssh` binary spawned in a PTY; its prompts answered in the app through `SSH_ASKPASS` (spec 0006) | Owner's choice. Uses the user's `~/.ssh/config`, ProxyJump, agent, FIDO keys and known_hosts, with no crypto code of our own |
| SFTP (Phase 2) | `ssh -s <host> sftp`, with the `russh-sftp` client speaking SFTP over its stdio | Same ssh config, agent, keys and known_hosts as terminal sessions; no SSH code of our own |
| Secrets (Phase 2) | OS keychain via `keyring` 4: Secret Service over pure-Rust D-Bus on Linux, Credential Manager on Windows | Never in plain files |
| Askpass channel (Phase 2) | `interprocess` local socket (Unix socket or named pipe) with a per-connection token from `getrandom` | No TCP port, one API on both platforms |
| Package manager | cargo | |
| Hosting / CI | GitHub + GitHub Actions | Hosted Linux and Windows runners. TTFP isn't gated in CI (shared runners are too noisy); the bench runs locally |

## Code layout

One crate. `src/main.rs` is the boot path and does only what the first
frame needs. Everything else is a module under `src/`:

- `terminal/` — the pane: PTY lifecycle, terminal element, input mapping, search
- `layout/` — tabs, title bar, split tree, session restore
- `commands/` — actions, keymap, palette
- `ssh/` — hosts, the ssh command, askpass (spec 0006)
- `config.rs`, `theme.rs`, `session.rs` — files on disk
- `secrets.rs` — the OS keychain
- `timing.rs`, `bench.rs` — TTFP marks and the `--bench-startup` runner

Modules are plain Rust that is testable without a window; GPUI views are a
thin layer over them.

## Key decisions and their costs

Load-bearing rules that code elsewhere depends on, plus the price we knowingly pay.

- **GPUI's Wayland backend is opt-in [load-bearing].** `gpui-pre-platform`
  ships with no windowing backend. Without its `wayland` feature GPUI
  compiles fine and then runs headless: no window, no error. We enable
  exactly `wayland`, and boot refuses to start GPUI without a Wayland
  session. *Cost:* none, as long as nobody "simplifies" the feature list.
- **GPUI and its widgets are pinned exactly and bumped together.**
  `gpui-pre` is a weekly snapshot of Zed's main branch, and
  `gpui-component` pins it exactly; both come from the `gpui-component`
  author, not from Zed. They move in lockstep: a bump is its own commit
  with a TTFP measurement and a look at the Zed commits it pulls in.
  *Cost:* API churn arrives in bulk at each bump, one third-party publisher
  sits in the supply chain, and the release build's final link takes about
  twice as long.
- **One palette for terminal and widgets.** The resolved zed-term theme is
  written into `gpui-component`'s theme at load and on every reload, so
  widgets never fall back to the library's own colors (spec 0005).
- **The UI thread never waits for a PTY.** Each pane's `alacritty_terminal`
  event loop runs on its own thread and owns the PTY. Its wakeups reach the
  pane's GPUI entity through a channel and `notify`, so GPUI redraws only
  that view, only when something changed. The grid lock is held just long
  enough to snapshot the visible rows. *Cost:* output that arrives between
  frames shows on the next frame.
- **Keys go to the shell unless an action claims them [load-bearing].** App
  shortcuts are GPUI actions bound in key contexts (workspace, palette,
  search). The terminal's context binds no plain `Ctrl`/`Alt` letters, so
  `Ctrl+C`, `Ctrl+V` and `Ctrl+R` reach the shell (spec 0004).
  `gpui-component`'s `Root` binds `tab`, `shift-tab` and `ctrl-c` above
  every pane; the terminal's context unbinds them with GPUI's `NoAction`,
  and every library bump re-checks `Root`'s bindings.
- **System fonts load at boot on Linux.** GPUI scans the installed fonts
  when its platform starts: ~21 ms warm and ~135 ms cold on the reference
  machine. Accepted, because the same scan gives per-glyph fallback.
  *Cost:* a machine with thousands of fonts boots slower; it is measured by
  the `platform` mark, not guarded.
- **Window decorations follow the compositor.** Where the compositor offers
  server-side decorations (KDE, COSMIC, wlroots-based), it draws the frame.
  GNOME doesn't, so there the tab bar doubles as the title bar (spec 0003).
  Windows draws its own frame. *Cost:* we own a minimal title bar.
- **A broken config never blocks boot.** Fall back to defaults and show the
  parse error with its line number. YAML's implicit typing (`no` → false) is
  contained by typed serde structs.
- **System `ssh` hides connection state.** We see bytes, not events:
  "connected", "auth failed" and password prompts must be inferred from the
  PTY output or the exit code. Stored passwords reach `ssh` via
  `SSH_ASKPASS` (our binary as the askpass helper), never by typing into the
  PTY. Requires an `ssh` binary (shipped with Windows 11, but it can be
  disabled).

## Data stores

All local, all plain files the user can read — nothing leaves the machine.

| Store | Location | Contents | Retention |
|---|---|---|---|
| Config | `<config_dir>/zed-term/config.yaml` | Settings, profiles, keybindings, inline themes | Until the user edits/deletes it |
| User themes | `<config_dir>/zed-term/themes/*.yaml` | Color schemes | Same |
| Session state | `<data_local_dir>/zed-term/session.json` | Window size, tab/split layout, each pane's profile, cwd and title | Overwritten on every save; never scrollback |
| Secrets (Phase 2) | OS keychain | SSH passwords / key passphrases the user chose to remember | Until the user forgets or replaces it (spec 0006) |

`<config_dir>`/`<data_local_dir>` come from the `dirs` crate (XDG on
Linux; `%APPDATA%` and `%LOCALAPPDATA%` on Windows, so dotfile-style config
roams but machine-specific cwd paths don't). Scrollback is never written to
disk — it can contain secrets the user typed or printed.

## Cross-cutting rules

- **Startup budget (load-bearing).** Time-to-first-prompt (TTFP) < 500 ms
  warm median on a release build on the reference machine. The boot path
  is: `main` → check for Wayland → read config and session → spawn the
  active tab's PTYs → start GPUI → open the window → first frame with
  output. Everything else happens after that frame. Full rules and
  measurement in [spec 0001](0001-startup-performance.md).
- **Lazy by default.** A feature that is not needed for the first frame is
  initialized on first use or after the first frame — never before it.
  "It's only a few ms" is how boot paths rot.
- **Errors.** Fallible operations return `Result<T, String>` with a
  human-readable message; the UI shows it in the affected pane, never a
  modal at boot.
- **Notices.** Non-fatal problems (broken config, skipped profile, bad
  keybinding, failed write) become notices: plain-text toasts stacked in
  the window's bottom-right corner, dismissed by click or after 5 s (the
  widget library's fixed timeout). Boot
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
   and only on the user's click: a plain click by default, Ctrl/Cmd+click
   with `open_links: ctrl_click` (owner's decision, 2026-11-05: having to
   hold Ctrl is what annoys in Tabby). Hovering shows the target first.
   The app never executes a path or command that came from terminal
   output.
3. **Credentials (Phase 2).** Keychain only. Passwords reach `ssh` via
   `SSH_ASKPASS`, never through env vars, command-line args, logs or the
   PTY.
4. **Agents (Phase 3).** Agent profiles run arbitrary commands locally or
   over SSH. Launching one is always an explicit user action, never
   triggered by terminal output.
