# zed-term

A fast terminal emulator with built-in SSH/SFTP and one-click AI coding
agents. Tabby's feature set, with a prompt in under 150 ms. Pure Rust:
`egui` for the window, `alacritty_terminal` for the terminal.

**Status:** Phase 1 (core terminal) is specified; the native implementation
starts with T-010. A Tauri/WebKitGTK version was built and measured first
(1.07 s to first prompt) and dropped; see OQ-009 and OQ-010.

- What we're building and why: [`spec/architecture.md`](spec/architecture.md)
- Backlog: [`tasks/`](tasks/)
- Dev setup: [`docs/development.md`](docs/development.md)
- Working agreement for humans and agents: [`AGENTS.md`](AGENTS.md)
