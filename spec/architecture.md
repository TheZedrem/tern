# Architecture — Tern

**Single source of truth for the stack.** Every architecture-relevant
decision lives here; changing one means editing this file. `docs/` may link
here but never restates decisions. Anything not yet decided is an entry in
[`open-questions.md`](open-questions.md).

## Product summary

Tern is a desktop terminal emulator with built-in SSH/SFTP — Tabby's
feature set, minus Tabby's boot time. It is for developers who live in
terminals and remote hosts and are tired of waiting for Electron. The constraint that shapes
everything: **a usable shell prompt in under 500 ms** from launch. It is a
local, single-user app: no accounts, no backend, no telemetry.

## Roadmap

| Phase | Scope | Specs |
|---|---|---|
| 1 — Core terminal | Local shell and local profiles, tabs + splits, shortcuts, clipboard, scrollback, search, command palette, YAML config + themes, session restore | 0001–0005, 0007, 0009 |
| 2 — SSH / SFTP | `ssh` profile type, host manager UI (groups), `~/.ssh/config` import, host import from MobaXterm, Tabby, PuTTY and WinSCP, connect via system `ssh` in a PTY, one file browser over the local disk or any host, drawn as a sidebar next to any pane or as a files pane (with file permissions: view and change mode, owner and group), secrets in the OS keychain; opt-in update check | 0006, 0008, 0010, 0011, 0013, 0014, 0015 |
| 3 — Command history | Detect the commands the user runs and suggest past ones inline as they type, like Windows Terminal and Warp | — |
| 4 — Theming | Edit themes in the app with a live preview, saved as `themes/*.yaml` files (spec 0005) | — |
| 5 — Plugins | A plugin system: runtime, API and per-plugin permissions; an extensible app sidebar with the SSH hosts as its first built-in panel and plugins adding their own; themes installable as plugins | 0016, 0018 |
| 6 — Plugin store | Plugin registries the user configures; browse, install and update plugins in the app, with an opt-in update check like spec 0011's | — |
| 7 — Port forwarding | Local, remote and dynamic SSH forwards created in an SSH pane's sidebar, each its own system `ssh -N` with `-L`/`-R`/`-D`, kept until deleted or Tern quits (deferred in spec 0006) | 0017 |
| 8 — Command profiles | Custom profiles that run a command (e.g. a coding agent like `claude` or `codex`) instead of the shell, launched in the focused pane's cwd; from an SSH pane there's no local cwd, so a folder picker asks for one. Any profile can also be set to ask for its working directory on launch | — |

**1.0 is Phases 1–8.**

Only phases that have started get specs and tasks; later phases stay one
row here. Phase 2 started before Phase 1 finished (owner's call,
2026-11-04), so its tasks wait for the Phase 1 tasks they build on.
Phase 7 started ahead of Phases 3–6 (owner's call, 2026-11-11); it builds
only on Phase 2.

**Constraints for the plugin phases.** Their specs must settle these;
until then they're recorded here so earlier work doesn't paint them into
a corner.

- **Theme plugins are data, not code.** The active theme is needed for the
  first frame, so a theme plugin is YAML the boot path reads directly. Code
  plugins load after the first frame, never before it.
- **Plugins are standalone programs, never code inside Tern's process.**
  Rust has no stable ABI and a dylib runs with every right the app has. A
  plugin is a program that exchanges JSON lines with Tern over stdio
  (spec 0016, the owner's design, 2026-11-11), which needs no new crate.
- **Tern can't sandbox a plugin, and says so (OQ-031).** A plugin runs
  with the user's rights: its own network and file access are beyond
  Tern's control. Tern warns the user to install plugins only from a
  source they trust, and enabling a plugin is that trust decision; no
  plugin starts before it.
- **Rights are granted by the user, per plugin.** What crosses Tern's
  protocol (typing into a terminal, reading its output, opening tabs,
  hearing about the user's panes) is declared by the plugin and granted
  by the user, and every write to or read from a terminal is confirmed
  each time. Phase 5 gets its own STRIDE threat model.
- **The store only goes online when the user asks.** Browsing and
  installing are user-started; update checks are opt-in. Downloads are
  verified against a hash in the registry index.

**Considered.** Ideas that aren't committed to a phase yet. They get no
specs or tasks until they're promoted to one.

- **Full macOS support.** Promote macOS from compile-only to a required
  platform: verified in the running app (PTY, `Cmd` shortcuts, Keychain,
  paths), in CI, and packaged. Signed and notarized builds would need a paid
  Apple Developer account.
- **AI assistant plugin.** A plugin, not built into Tern, that helps with
  errors, commands and general CLI use by delegating to an agent harness
  the user already runs (e.g. Claude Code, Codex), the way T3 Code does.
  Tern runs no model and holds no API keys. Needs Phase 5. Terminal output
  carries secrets, so only what the user picks per request is handed to
  the harness.
- **Docker / Compose plugin.** A plugin that shows running containers in a
  sidebar, possibly with controls to attach, view logs and restart. Needs
  Phase 5.
- **S3 and SMB.** Browse and transfer files in S3 buckets and SMB shares
  as plugins that add a target to the file browser (spec 0013). Needs
  Phase 5.
- **Passwords in an extra encrypted file.** A vault file beside the
  config, encrypted with a master password, as Tabby's vault is: it would
  travel with the config to another machine, and imports (specs 0010,
  0015) could bring saved passwords along instead of leaving them behind.
  It breaks ground rule 3 (secrets only in the OS keychain), so promoting
  it means changing that rule first, plus a threat model (key derivation,
  unlock prompt, how long the key stays in memory) and a crypto crate.

## Platforms

**Modern platforms only [load-bearing].** Tern takes no dependency on
deprecated or end-of-life technology. That rules out X11 and XWayland on
Linux and Windows 10, and it rules out adding a dependency just to keep one
of them alive.

| Platform | Support | Notes |
|---|---|---|
| Linux, Wayland session | Required | Any compositor with `xdg-shell`. Needs a Vulkan driver (Mesa or NVIDIA) |
| Windows 11 | Required | DirectX 11, ConPTY |
| macOS | Compile-only | Built and installed from source with `packaging/macos/install.sh`. No CI builds, no packages, no signing, no blocking on it. Full support is only considered (see Roadmap) |
| X11 or XWayland sessions, Windows 10 and older | Not supported | In an X11 session Tern exits with a clear message (spec 0001) |

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
| SFTP edit copies (Phase 2) | `<data_local_dir>/tern/edit/` | Remote files the user opened for editing | Until their panel closes; a crashed run's on the next launch (spec 0008) |
| Secrets (Phase 2) | OS keychain via `keyring` 4: Secret Service over pure-Rust D-Bus on Linux, Credential Manager on Windows | Never in plain files |
| Askpass channel (Phase 2) | `interprocess` local socket (Unix socket or named pipe) with a per-connection token from `getrandom` | No TCP port, one API on both platforms |
| Update check | `ureq` 3 with `rustls` and the OS certificate store (`platform-verifier`), blocking, on its own thread (spec 0011) | Small, pure Rust, no async runtime; honors `HTTPS_PROXY` (OQ-022) |
| Package manager | cargo | |
| Linux packages | `.deb` (Debian, Ubuntu), `.rpm` (Fedora, openSUSE) and AppImage, all from one release binary built on `ubuntu-24.04` (glibc 2.39) | Owner's choice (2026-11-05): native packages for the two big families, AppImage for every other distro. No Flatpak for now: its sandbox changes how shells and `ssh` are spawned (OQ-020) |
| Windows packages | A per-user MSI from one hand-written `packaging/windows/tern.wxs`, built with WiX Toolset v7 (`wix` .NET tool, pinned) on `windows-2025`; `tern.exe` links the C runtime statically | No admin rights or VC++ redistributable needed (spec 0012). WiX v3, which `cargo-wix` targets, is end-of-life; v7's EULA is accepted (OQ-024) |
| Hosting / CI | GitHub + GitHub Actions | Hosted Linux and Windows runners. TTFP isn't gated in CI (shared runners are too noisy); the bench runs locally |
| Package repositories | Signed dnf/zypper and apt repositories on GitHub Pages; the packages stay in GitHub Releases | Owner's choice (2026-11-05): no host besides GitHub, and no binaries in git (T-031) |

## Code layout

One crate. `src/main.rs` is the boot path and does only what the first
frame needs. Everything else is a module under `src/`:

- `terminal/` — the pane: PTY lifecycle, terminal element, input mapping, search. `mod.rs` holds `TerminalPane` and its render; `launch.rs` starts and restarts the program, `exit.rs` titles and exit handling, `answers.rs` prompt answering, `mouse.rs` mouse, selection and clipboard, `font.rs` font settings, `input.rs` keys and IME, `search.rs` the search bar
- `layout/` — tabs, title bar, split tree, session restore. `mod.rs` holds `Workspace`, `Boot`, the action wiring and render; each feature it hosts has its own file: `session.rs`, `panes.rs`, `files.rs` (files panes and each SSH pane's views for the sidebar), `sidebar.rs` (the sidebar's wiring: commands, placement, saving), `profiles.rs` (palette and profiles), `reload.rs` (config reload and the settings screen), `closing.rs`, `splits.rs` (splits and drag and drop), `chrome.rs` (tab bar and title bar)
- `commands/` — actions, keymap, palette
- `ssh/` — hosts, the ssh command, askpass (spec 0006)
- `files/` — the one file browser: its view, tree and every feature on it, the Local target, and the two places it is drawn, the sidebar and the files pane (spec 0013). `browser.rs` holds `Browser`; its features live in `browser/`: `navigate.rs`, `transfers.rs`, `changes.rs` (rename, delete, new folder), `edits.rs`, `menu.rs`, `render.rs`
- `sftp/` — the SFTP target: connections, transfers and edit copies (spec 0008)
- `sidebar/` — the window's sidebar (spec 0018): `mod.rs` holds `Sidebar`, which draws the strip, the header and the shown panel and knows no panel by name; `panel.rs` the `Panel` every panel registers as; `pane.rs` the panel that follows the active pane (Files and Port forwards) and `hosts.rs` the SSH hosts panel
- `plugins/` — the plugin host (spec 0016): `mod.rs` holds `Host`, which finds plugins and hands each its events one at a time; `manifest.rs` reads `plugin.yaml`, `protocol.rs` the events and actions, `process.rs` one plugin's process, its pipes and the kill of its tree
- `settings/` — the settings screen (spec 0009): `mod.rs` the view and shared rows, one file per tab in `pages/`
- `config/` (`mod.rs` the types, `load.rs` the lenient reader, `edit.rs` in-place edits, `watch.rs` the watcher), `theme.rs`, `session.rs` — files on disk
- `secrets.rs` — the OS keychain
- `update.rs` — the opt-in release check (spec 0011)
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
- **One file browser, many targets [load-bearing].** The sidebar and the
  files pane draw the same browser, and the browser runs over a target:
  Local or SFTP today, plugin targets later (Phase 5). Every browser
  feature (tree, filter, path suggestions, permissions, transfers, edits)
  is written once in `files/` against the targets' operations; a target
  only supplies operations and says which it supports, and what it
  doesn't support isn't offered. No feature has code per surface or per
  target, so a plugin target gets each feature without work of its own.
  *Cost:* a feature that one target can't do well (e.g. permissions on a
  Windows disk) needs a capability flag instead of a quick special case.
- **One palette for terminal and widgets.** The resolved Tern theme is
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
| Config | `<config_dir>/tern/config.yaml` | Settings, profiles, keybindings, inline themes | Until the user edits/deletes it |
| User themes | `<config_dir>/tern/themes/*.yaml` | Color schemes | Same |
| Session state | `<data_local_dir>/tern/session.json` | Window size, tab/split layout, each pane's profile, cwd and title; a files pane's target and folder (spec 0013); the sidebar's open flag, width and shown panel (spec 0018) | Overwritten on every save; never scrollback |
| Update check | `<data_local_dir>/tern/update.json` | Time of the last check | Overwritten on each check (spec 0011) |
| SFTP edit copies | `<data_local_dir>/tern/edit/<run>/…`, `<run>.lock` | Remote files the user opened for editing (`0600`), and each running Tern's lock | Until their SFTP session ends; a crashed run's after the next launch's first frame (spec 0008) |
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
  human-readable message. Every error the user can hit is shown as a
  notice, never swallowed (the owner's rule); one that leaves a pane
  without its program also shows in that pane. Never a modal at boot.
- **Notices.** Non-fatal problems (broken config, skipped profile, bad
  keybinding, failed write) become notices: plain-text toasts stacked in
  the window's bottom-right corner, dismissed by click or after 5 s (the
  widget library's fixed timeout). Boot
  notices are collected before the window exists and shown on the first
  frame. A notice never takes keyboard focus from the pane.

## Ground rules (binding)

1. **No network at startup.** No update checks, fonts, analytics, or
   anything else over the network before the first prompt; deferred work
   waits until after the first frame. The only outbound traffic is what the
   user explicitly starts (SSH, SFTP), plus the daily update check the
   user agrees to once (spec 0011). A plugin the user enabled is the
   user's own program and may use the network after the first frame;
   Tern itself sends nothing for it and warns before it is enabled
   (spec 0016, OQ-031).
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
   and only on the user's click: Ctrl/Cmd+click by default, a plain click
   too with `open_links: click` (owner's decision, 2026-11-05).
   Hovering shows the target first.
   The app never executes a path or command that came from terminal
   output.
3. **Credentials (Phase 2).** Keychain only. Passwords reach `ssh` via
   `SSH_ASKPASS`, never through env vars, command-line args, logs or the
   PTY.
