---
status: later
blocked-by: [T-012]
---
# T-016: Phase 2 spec: SFTP panel

Write the SFTP spec: a file panel for the connected host over
`ssh -s <host> sftp` with the `russh-sftp` client (OQ-001), sharing the
connection's ssh config and askpass prompts from spec 0006. Area: meta.

Spec: ../spec/architecture.md

## Result (2026-11-05)

Drafted [spec 0008](../../spec/0008-sftp-panel.md), for the owner's
approval: the panel as a pane kind of the split tree, `ssh -s <host> sftp`
with its own askpass token, `russh-sftp` on a thread of its own with a
current-thread tokio runtime (`russh-sftp` 3 needs tokio, which is
already built for GPUI's HTTP client), file transfers with progress and
cancel, and remote names treated as untrusted (plain text, no path
traversal). Folder transfers and drag and drop are *Later*. Filed T-024
(its threat model, after approval) and T-023 (building it).
