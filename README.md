# zed-term

A fast terminal emulator with built-in SSH/SFTP and one-click AI coding
agents. Tabby's feature set, with a prompt in under 500 ms. Pure Rust:
GPUI, Zed's UI framework, for the window and `alacritty_terminal` for the
terminal. Runs on Linux (Wayland) and Windows 11.

**Status:** Phase 1 (core terminal) in progress. A single-pane terminal
boots to a prompt in about 330 ms (T-010); clipboard, tabs, the palette and
config follow. A Tauri/WebKitGTK version was built and measured first
(1.07 s) and dropped; see OQ-009, OQ-010 and OQ-013.

- What we're building and why: [`spec/architecture.md`](spec/architecture.md)
- Backlog: [`tasks/`](tasks/)
- Dev setup: [`docs/development.md`](docs/development.md)
- Working agreement for humans and agents: [`AGENTS.md`](AGENTS.md)
