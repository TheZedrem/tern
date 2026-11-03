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
everything: **a usable shell prompt in under 300 ms** from launch. It is a
local, single-user app: no accounts, no backend, no telemetry.

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

| Layer | Choice | Why |
|---|---|---|
| Shell | Tauri 2, system webview (WebKitGTK / WebView2 / WKWebView) | No Chromium + Node to boot on every launch, which is what Electron pays |
| Backend | Rust (edition 2024), single crate in `src-tauri/` | Comes with Tauri; good fit for PTYs and, later, keychain access |
| PTY | `portable-pty` | One API over Unix PTYs and Windows ConPTY |
| Terminal view | xterm.js 6 + `@xterm/addon-webgl`; DOM renderer as automatic fallback | Most battle-tested web terminal; we don't write a VT parser |
| Frontend | SolidJS + TypeScript + Vite | ~7 KB runtime, no virtual DOM, plain TSX checked by `tsc` with no separate type checker, close to React |
| Styling | Tailwind CSS 4 | Build-time only, no runtime cost |
| PTY ⇄ UI transport | One binary Tauri `Channel` per pane for output; `invoke` for input/resize/kill | Raw bytes with no JSON/base64 on the hot path; ordered per pane; no localhost port |
| Config format | YAML, parsed with `serde-saphyr` | Owner's choice. `serde-saphyr` is pure Rust, never panics while parsing, and is maintained (`serde_yaml` is archived) |
| SSH (Phase 2) | System `ssh` binary spawned in a PTY | Owner's choice. Uses the user's `~/.ssh/config`, ProxyJump, agent, FIDO keys and known_hosts, with no crypto code of our own |
| SFTP (Phase 2) | `ssh -s <host> sftp`, with the `russh-sftp` client speaking SFTP over its stdio | Same ssh config, agent, keys and known_hosts as terminal sessions; no SSH code of our own |
| Secrets (Phase 2) | OS keychain via the `keyring` crate | Never in plain files |
| Package managers | pnpm, cargo | |
| Hosting / CI | GitHub + GitHub Actions | Hosted Linux and Windows runners. TTFP isn't gated in CI (shared runners are too noisy); the bench runs locally |

## Code layout

One repository, two halves that talk only through the IPC contract
(§Key decisions):

- `src-tauri/` — Rust: window setup, PTY manager, config/session files,
  startup timing. Commands are thin; logic lives in plain modules that are
  testable without a window.
- `src/` — SolidJS: `main.tsx` is the boot path and imports only what the
  first prompt needs; every other feature lives under `src/features/<name>/`
  and is reached through `import()`.

## Key decisions and their costs

Load-bearing rules that code elsewhere depends on, plus the price we knowingly pay.

- **PTY streaming.** The frontend creates one Channel per pane and passes it
  to `pty_spawn` / `pty_attach`. A Rust reader thread sends each read as raw
  bytes, and xterm decodes UTF-8 across chunk boundaries. The exit notice is a
  single JSON message on the same Channel, so it can never overtake the
  pane's final output. A PTY started before the webview exists buffers its
  output in Rust until a Channel attaches.
  *Cost:* a Channel dies with the page (a dev reload), so panes must be
  re-attachable. Input costs one IPC round trip per batch; measure large
  pastes.
- **Webviews differ.** Three engines means three sets of quirks. WebKitGTK
  is the weakest, and its WebGL can fail. xterm 6 has no canvas renderer, so
  the only fallback is the slower DOM renderer, and that fallback is
  mandatory.
- **SolidJS is not React.** Components run once, and destructuring props
  breaks reactivity. Its ecosystem is smaller, so expect to hand-build
  dialogs and menus.
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
| Session state | `<data_dir>/zed-term/session.json` | Tab/split layout, each pane's cwd and title | Overwritten on every save; never scrollback |
| Secrets (Phase 2) | OS keychain | SSH passwords / key passphrases | Until the host profile is deleted |

`<config_dir>`/`<data_dir>` come from the `dirs` crate (XDG on Linux,
`%APPDATA%` on Windows). Scrollback is never written to disk — it can
contain secrets the user typed or printed.

## Cross-cutting rules

- **Startup budget (load-bearing).** Time-to-first-prompt (TTFP) < 300 ms on
  a release build. The boot path is: Rust `main` → read config → spawn the
  first shell → create window → first paint. Everything else is lazy. Full
  rules and measurement in [spec 0001](0001-startup-performance.md).
- **Lazy by default.** A feature that is not needed for the first prompt is
  loaded on first use or after idle — never at boot. "It's only a few KB" is
  how boot paths rot.
- **Typed IPC.** Every Tauri command has one typed wrapper on the frontend;
  components never call `invoke` with a string literal.
- **Errors.** Rust commands return `Result<T, String>` with a human-readable
  message; the frontend shows it in the affected pane, never a modal at
  boot.
- **ACP readiness (Phase 4).** A pane's content source is an abstraction
  (PTY today), so a future structured agent view can live in a pane without
  rewriting layout or tabs.

## Ground rules (binding)

1. **No network at startup.** No update checks, fonts, analytics, or
   anything else over the network before the first prompt; deferred work
   waits until idle. The only outbound traffic is what the user explicitly
   starts (SSH, agents).
2. **No telemetry.** Nothing about the user's sessions, hosts, or commands
   is collected or sent anywhere.
3. **Secrets live in the OS keychain only** — never in `config.yaml`,
   `session.json`, logs, or the frontend's storage.
4. **Self-hosted assets.** Fonts and icons ship inside the app bundle; no
   CDNs.
5. **Data minimization.** Session restore stores layout and cwd, not
   scrollback or command history.

## Security (binding)

The trust boundaries every spec checks against. A spec that crosses one
says how it handles it in its **Security & privacy** section.

1. **Webview → Rust.** The webview can spawn PTYs, so script running in the
   webview equals code execution on the machine. Keep a strict CSP, grant
   only the Tauri permissions that are needed, and never render
   terminal-supplied strings (titles, filenames, agent output) as HTML.
2. **Escape sequences are untrusted input.** Any program or remote host can
   emit OSC 52 (clipboard write), OSC 8 (links) and title sequences. Each one
   we honor is an explicit decision in a spec. Clipboard writes from the
   terminal are off unless enabled.
3. **Credentials (Phase 2).** Keychain only. Passwords reach `ssh` via
   `SSH_ASKPASS`, never through env vars, command-line args, logs or the
   PTY.
4. **Agents (Phase 3).** Agent profiles run arbitrary commands locally or
   over SSH. Launching one is always an explicit user action, never
   triggered by terminal output.
