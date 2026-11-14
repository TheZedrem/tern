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

- Tabs, vertical tabs and split panes, rearranged by drag and drop; the focused pane gets an accent border and the others dim (`active_pane`); a middle click closes a tab, and `close_last_tab: new_tab` keeps the window open when the last tab closes
- Session restore: tabs, splits and the window come back as you left them, with a fresh tab opened on top, or a restored local shell focused instead (`open_new_tab: false` to skip it)
- Splits and restored tabs open in the shell's working directory, with the opt-in [shell integration](#shell-integration) on Windows and everywhere for SSH
- Search, clickable links, IME input, right-click paste, copy on select and the Linux primary selection
- A command palette, and shortcuts that never take `Ctrl`/`Alt` keys from the shell
- YAML config that reloads live, profiles and themes, and a settings screen for all of it

**SSH and SFTP**

- Hosts from `~/.ssh/config` and SSH profiles, connected through your system's `ssh`
- A host picker with groups and filtering
- ssh's prompts answered in the app, with passwords kept in the OS keychain if you choose
- Import hosts from MobaXterm, Tabby, PuTTY and WinSCP in one click. PuTTY's
  `.ppk` keys need converting with `puttygen` first, and Pageant users must
  turn on Pageant's OpenSSH pipe for `ssh` to use it
- A sidebar that stays open across tabs (`Ctrl+Shift+S`), showing the active pane's files: an SSH pane's over SFTP, a local shell's from its folder. Browse, download, upload (drag and drop too), rename and delete
- Your SSH hosts in the same sidebar, filtered as you type, opened with a double click, and edited or deleted from a right click
- Edit remote files in your own editor; saves are uploaded on a click, on save or on focus
- A files pane that shows the local disk or any saved host as a tree next to your terminals
- Port forwards (local, remote and SOCKS) from the sidebar, listening on 127.0.0.1 unless you allow the network; never saved

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
| `Ctrl+Shift+S` | Sidebar: the pane's files, SSH hosts, an SSH pane's port forwards |
| `Ctrl+Shift+B` / `Ctrl+Shift+O` | Files pane / switch its host |

## Shell integration

To have splits, restored tabs and an SSH pane's files in the sidebar open in the
folder you're in, source the snippet for your shell from its rc file. It
reports the working directory with OSC 7 at each prompt; nothing is set
up automatically.

| Shell | Add to | Line |
|---|---|---|
| bash | `~/.bashrc` | `. /path/to/tern/shell-integration/tern.bash` |
| zsh | `~/.zshrc` | `source /path/to/tern/shell-integration/tern.zsh` |
| PowerShell 7 | `$PROFILE` | `. /path/to/tern/shell-integration/tern.ps1` |

On Linux, local panes follow `cd` without it. On a server, the snippet
lets the sidebar's files start in the shell's folder. A report counts as
local only for this machine and an existing directory.

## Roadmap

**Next**

- In the file browser: fuzzy filter, path suggestions, permissions, deleting to the trash, dragging files out ([tasks](tasks/))
- Signed apt and dnf repositories ([T-031](tasks/T-031-package-repositories.md))

**Toward 1.0** ([roadmap](spec/architecture.md#roadmap))

- **Plugins:** an extensible sidebar, themes as plugins, and plugin registries you browse in the app
- SSH port forwarding (local, remote and SOCKS) from the SFTP sidebar, in a few clicks ([spec 0017](spec/0017-port-forwarding.md))
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
