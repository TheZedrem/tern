---
status: later
blocked-by: [T-006]
---
# T-008: Phase 2 spec: SSH host manager and SFTP

Write the Phase 2 spec (host profiles and groups, `~/.ssh/config` import,
system `ssh` panes, SFTP panel, keychain secrets via `SSH_ASKPASS`). Its
Security & privacy section covers architecture §Security point 3.
SFTP runs over `ssh -s <host> sftp` with `russh-sftp` (OQ-001). Area: meta.

Spec: ../spec/architecture.md
