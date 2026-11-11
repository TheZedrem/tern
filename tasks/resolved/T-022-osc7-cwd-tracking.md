---
status: resolved
blocked-by: []
---
# T-022: Track the cwd shells report (OSC 7)

Split out of T-002. vte doesn't parse OSC 7, so wrap alacritty_terminal's
PTY in a type whose reader passes the bytes through untouched and scans
them for `ESC ] 7 ; file://host/path` up to 4 KiB, implementing
`EventedReadWrite`/`EventedPty`/`OnResize` by delegation, with
`Reader = Self` so it works on Unix and ConPTY alike. A report counts only
in a local pane, for a host that is empty, `localhost` or this machine's
name, naming an existing local directory (spec 0002, TM-escape-09); the
pane's cwd prefers it over `/proc`, and a change saves the session
(T-006). In an SSH pane, keep the last report's path in memory only as
the remote cwd its SFTP session starts in (spec 0008). Ship the opt-in bash, zsh and pwsh snippets under
`shell-integration/` (OQ-008) that emit it. Needs `polling` as a direct
dependency (OQ-018). Area: src.

Spec: ../spec/0002-terminal-pane.md (cwd, AC9)
Spec: ../spec/threat-model/0002-escape-sequences.md (TM-escape-09)

**Resolved (2026-11-11):** `src/terminal/osc7.rs` wraps the PTY
(`Osc7Pty`) and keeps the last report in `Reported`; a local pane's cwd
prefers it over `/proc` and a change saves the session; an SSH pane's
sidebar starts in it. On Windows only drive paths count (no UNC, so no
network on the check). Snippets in `shell-integration/`. Checked on
Windows 11: the pwsh snippet's report passes through ConPTY, and the
session saved `C:/Users` with no `/proc` to fall back on. Linux, zsh and
the SFTP start folder are checked in T-073.
