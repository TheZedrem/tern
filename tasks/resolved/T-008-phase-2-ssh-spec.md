---
status: later
blocked-by: [T-006]
---
# T-008: Phase 2 spec: SSH host manager and SFTP

**Resolved 2026-11-04:** written as spec 0006 (SSH hosts and connections)
with research 0001, at the owner's request ahead of Phase 1's end. SFTP
moved to its own spec task, T-016. Implementation: T-012 to T-015.

Write the Phase 2 spec (host profiles and groups, `~/.ssh/config` import,
system `ssh` panes, SFTP panel, keychain secrets via `SSH_ASKPASS`). Its
Security & privacy section covers architecture §Security point 3.
SFTP runs over `ssh -s <host> sftp` with `russh-sftp` (OQ-001). Area: meta.

Spec: ../spec/architecture.md
