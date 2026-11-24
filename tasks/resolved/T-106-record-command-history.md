---
status: now
blocked-by: []
---
# T-106: Record the commands local shells run

The bash, zsh and pwsh snippets report the last command with the
per-run token. Tern drops what the never-stored rules
and token patterns catch and keeps one history per shell kind, in
`history.jsonl` or only in memory as `command_history` says. Includes
loading after the first frame, the setting on the settings screen with
its template line in `src/default-config.yaml`, and **Clear command
history** — src.
Spec: ../spec/0021-command-history.md
