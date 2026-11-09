<div align="center">

<img src="assets/icon.svg" alt="Tern icon" width="112" height="112">

# Tern

**A terminal with built-in SSH and SFTP that shows a prompt in about 330 ms.**

Tabby's feature set at the speed of a native terminal. Pure Rust on
[GPUI](https://www.gpui.rs) and `alacritty_terminal`, for Linux (Wayland)
and Windows 11.

[![CI](https://github.com/TheZedrem/tern/actions/workflows/ci.yml/badge.svg)](https://github.com/TheZedrem/tern/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/TheZedrem/tern)](https://github.com/TheZedrem/tern/releases/latest)
[![License: EUPL-1.2](https://img.shields.io/badge/license-EUPL--1.2-blue)](LICENSE)

[Install](#install) · [Features](#features) · [Keys](#keys) · [Roadmap](#roadmap) · [Contributing](#contributing)

</div>

## Why Tern

Tabby has the right features, but Electron makes it slow, so a quick SSH
session turns into a wait. MobaXterm is fast, but it's hard to learn and
confusing in places. PuTTY handles SSH well, but it has no local shell and
no SFTP. Tern aims for all of it: native speed, and an interface you don't
need a manual for.

Speed is the main requirement: every feature loads after the first frame,
and every release is benchmarked ([TTFP history](docs/ttfp.md)). An
earlier Tauri version took 1.07 s to start and was dropped.

## Features

**Terminal**

- Tabs, vertical tabs and split panes, rearranged by drag and drop
- Session restore: tabs, splits and the window come back as you left them
- Search, clickable links, IME input, right-click paste, copy on select and the Linux primary selection
- A command palette, and shortcuts that never take `Ctrl`/`Alt` keys from the shell
- YAML config that reloads live, profiles and themes, and a settings screen for all of it

**SSH and SFTP**

- Hosts from `~/.ssh/config` and SSH profiles, connected through your system's `ssh`
- A host picker with groups and filtering
- ssh's prompts answered in the app, with passwords kept in the OS keychain if you choose
- Import hosts from MobaXterm and Tabby in one click
- An SFTP sidebar on every SSH pane: browse, download, upload (drag and drop too), rename and delete
- Edit remote files in your own editor; saves are uploaded on a click, on save or on focus
- A files pane that shows the local disk or any saved host as a tree next to your terminals

**Privacy**

- No telemetry and no network traffic at startup. The release check is opt-in.
- Secrets go only in the OS keychain, and scrollback never touches the disk.

## Install

**Linux:** `.deb`, `.rpm` and AppImage packages are on the
[releases page](https://github.com/TheZedrem/tern/releases/latest). They
need glibc 2.39 or newer and a Wayland session.

**Windows 11:** releases after 0.4.0 carry `tern-<version>-x64.msi`.
It installs for your user only, so it needs no admin rights, and a newer
MSI upgrades it in place. It's unsigned for now, so SmartScreen warns
before you run it.

**From source** (Linux, Windows 11, macOS), with a Rust toolchain and the
[prerequisites](docs/development.md#prerequisites):

```sh
packaging/linux/install.sh                 # Linux
pwsh -File packaging/windows/install.ps1   # Windows 11
packaging/macos/install.sh                 # macOS (builds, not officially supported)
```

The script builds a release binary and adds `tern` to `~/.local/bin`
(`%USERPROFILE%\.local\bin` on Windows) plus a desktop or Start menu
entry. To update, pull and run it again. On Windows, close Tern first.

## Keys

App shortcuts use `Ctrl+Shift`, so plain `Ctrl` and `Alt` keys go to the
shell. You can rebind all of them in the settings or in `config.yaml`.

| Keys | Action |
|---|---|
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+Shift+T` / `Ctrl+Shift+H` | New tab with a profile / connect to a host |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Split right / split down |
| `Ctrl+Shift+W` | Close the pane |
| `Alt+1`–`9`, `Alt+←↑→↓` | Go to a tab, or to the next pane in that direction |
| `Ctrl+Shift+F` | Search the scrollback |
| `Ctrl+Shift+S` | SFTP sidebar of an SSH pane |
| `Ctrl+Shift+B` / `Ctrl+Shift+O` | Files pane / switch its host |

## Roadmap

**Next**

- Following the shell's working directory (OSC 7) ([T-022](tasks/T-022-osc7-cwd-tracking.md))
- In the file browser: fuzzy filter, path suggestions, permissions, deleting to the trash, a local sidebar, dragging files out ([tasks](tasks/))
- Signed apt and dnf repositories ([T-031](tasks/T-031-package-repositories.md))

**Toward 1.0** ([roadmap](spec/architecture.md#roadmap))

- **Plugins:** an extensible sidebar, themes as plugins, and plugin registries you browse in the app
- SSH port forwarding (local, remote and dynamic) on a profile
- Suggestions from your command history as you type
- A theme editor with a live preview
- Profiles that run a command (a coding agent, say) in the current folder or a folder you pick

Tern builds in the terminal, SSH and SFTP because nearly everyone needs
them. More specialized tools will be plugins, so you add only what you
use. Some ideas: Docker and Compose containers in a sidebar, S3 buckets
and SMB shares in the file browser, and an AI assistant that hands errors
to an agent you already run (Claude Code, Codex). Full macOS support is
being considered.

## Contributing

Bug reports and feature requests are welcome as
[GitHub issues](https://github.com/TheZedrem/tern/issues). Tern is a
personal project, so a request isn't a promise that it gets built.

Pull requests are welcome too, as long as they keep Tern fast and easy to
use. Tern is built spec-first: read [`AGENTS.md`](AGENTS.md) for the
working agreement, [`spec/architecture.md`](spec/architecture.md) for
the design, and [`docs/development.md`](docs/development.md) to set up a
dev environment.

## License

[EUPL-1.2](LICENSE). The bundled Lilex font is under the
[SIL Open Font License](assets/fonts/lilex/OFL.txt).
