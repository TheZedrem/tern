---
status: next
blocked-by: []
---
# T-073: Check OSC 7 cwd tracking on Linux and over SSH

T-022 was built and checked on Windows 11 only. On Linux (Wayland):
`cargo clippy --all-targets -- -D warnings` and `cargo test` (the Unix
tests didn't run); with `tern.bash` and `tern.zsh` sourced, `cd` into a
folder with a space and non-ASCII letters, then split: the new pane
starts there; a report for another host is ignored. Over SSH, with the
snippet sourced on the server, `cd /var/log` and open the SFTP sidebar:
it lists `/var/log` (spec 0008, criterion 1). zsh's snippet has never
run. Area: src.

Spec: ../spec/0002-terminal-pane.md (cwd, AC9)
Spec: ../spec/0008-sftp-panel.md
