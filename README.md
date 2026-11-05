# Tern

A fast terminal emulator with built-in SSH/SFTP. Tabby's feature set, with
a prompt in under 500 ms. Pure Rust: GPUI, Zed's UI framework, for the
window and `alacritty_terminal` for the terminal. Runs on Linux (Wayland)
and Windows 11.

**Status:** The core terminal works: tabs, splits, vertical tabs, the
command palette, YAML config with a settings screen, themes and session
restore, booting to a prompt in about 330 ms. SSH profiles, the host
picker, stored passwords and host import from MobaXterm and Tabby are in,
as are an opt-in daily check for new releases on GitHub and Linux
packages; the SFTP panel and package repositories are next. A Tauri/WebKitGTK version was
built and measured first (1.07 s) and dropped; see OQ-009, OQ-010 and
OQ-013.

## Why Tern

My main problem with Tabby is speed: Electron slows it down, so a quick SSH
session becomes a wait. MobaXterm is faster, but it's unintuitive and in
places hard to use and understand. PuTTY is great for SSH, but it has no
local shell and no built-in SFTP. Tern aims for both: native speed and an
interface that explains itself.

## Features

**Terminal**

- [x] Boots to a prompt in about 330 ms, with a TTFP badge in dev builds ([spec 0001](spec/0001-startup-performance.md))
- [x] Terminal pane: rendering, input and IME, resize, scrollback, search, clickable links, copy/paste and the Linux primary selection ([spec 0002](spec/0002-terminal-pane.md))
- [x] Tabs, vertical tabs and split panes ([spec 0003](spec/0003-tabs-splits-and-session-restore.md))
- [x] Dragging tabs and panes into and out of splits ([spec 0007](spec/0007-drag-and-drop-splits.md))
- [x] Session restore ([spec 0003](spec/0003-tabs-splits-and-session-restore.md))
- [x] Command palette and rebindable keybindings that leave `Ctrl`/`Alt` to the shell ([spec 0004](spec/0004-commands-keybindings-palette.md))
- [x] YAML config with hot reload, profiles and themes ([spec 0005](spec/0005-config-and-themes.md))
- [x] Settings screen ([spec 0009](spec/0009-settings-screen.md))
- [ ] Tracking the shell's working directory via OSC 7, with opt-in shell snippets ([T-022](tasks/T-022-osc7-cwd-tracking.md))

**SSH / SFTP**

- [x] SSH profiles and hosts from `~/.ssh/config`, connecting through the system `ssh` ([spec 0006](spec/0006-ssh-hosts.md))
- [x] Host picker with groups and filtering
- [x] ssh prompts answered in the app, passwords remembered in the OS keychain on request
- [x] Importing hosts from MobaXterm and Tabby ([spec 0010](spec/0010-import-hosts.md))
- [ ] SFTP panel: browse, edit remote files in a local app, upload, download, rename, delete ([spec 0008](spec/0008-sftp-panel.md), [T-023](tasks/T-023-sftp-panel.md))
- [ ] Dragging files out of the SFTP panel into the file manager ([T-027](tasks/T-027-sftp-drag-out-local-files.md))

**Distribution**

- [x] Opt-in daily check for new releases on GitHub ([spec 0011](spec/0011-update-check.md))
- [x] `.deb`, `.rpm` and AppImage packages
- [ ] Packages checked on their distros ([T-030](tasks/T-030-check-linux-packages.md))
- [ ] Signed dnf and apt repositories on GitHub Pages ([T-031](tasks/T-031-package-repositories.md))
- [ ] CI for Linux and Windows ([T-007](tasks/T-007-ci-pipeline.md))
- [ ] Verified on Windows 11 ([T-011](tasks/T-011-verify-t010-on-windows.md))

**Later** (on the [roadmap](spec/architecture.md#roadmap), not yet planned in detail)

- [ ] Suggestions from command history as you type, like Windows Terminal and Warp
- [ ] Theme editor in the app, with a live preview
- [ ] SSH port forwarding (local, remote and dynamic) on a profile
- [ ] Plugin system (considered, not committed to)

- What we're building and why: [`spec/architecture.md`](spec/architecture.md)
- Backlog: [`tasks/`](tasks/)
- Dev setup: [`docs/development.md`](docs/development.md)
- Working agreement for humans and agents: [`AGENTS.md`](AGENTS.md)

## Contributing

Suggestions and feature requests are welcome as
[GitHub issues](https://github.com/TheZedrem/tern/issues). Tern is a
personal project, so a request isn't a promise that it gets built.

Forks and pull requests are welcome too, as long as they keep Tern fast and
easy to use and add something genuinely useful.
