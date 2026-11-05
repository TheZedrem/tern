# Tern

A fast terminal emulator with built-in SSH/SFTP. Tabby's feature set, with
a prompt in under 500 ms. Pure Rust: GPUI, Zed's UI framework, for the
window and `alacritty_terminal` for the terminal. Runs on Linux (Wayland)
and Windows 11.

**Status:** The core terminal works: tabs, splits, vertical tabs, the
command palette, YAML config with a settings screen, themes and session
restore, booting to a prompt in about 330 ms. SSH profiles, the host
picker, stored passwords and host import from MobaXterm and Tabby are in,
as is an opt-in daily check for new releases on GitHub;
the SFTP panel and Linux packages are next. A Tauri/WebKitGTK version was
built and measured first (1.07 s) and dropped; see OQ-009, OQ-010 and
OQ-013.

- What we're building and why: [`spec/architecture.md`](spec/architecture.md)
- Backlog: [`tasks/`](tasks/)
- Dev setup: [`docs/development.md`](docs/development.md)
- Working agreement for humans and agents: [`AGENTS.md`](AGENTS.md)
