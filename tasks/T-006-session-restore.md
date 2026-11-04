---
status: next
blocked-by: []
---
# T-006: Session restore

Debounced atomic `session.json` saves; on boot, spawn only the active tab's
panes and spawn other tabs lazily on first activation; handle corrupt files.
Restored SSH panes start as `terminal::Start::OnEnter` (T-012 built it for
an SSH tab at boot), so they connect only on Enter. Verify that TTFP with
a restored 4-pane tab stays under budget. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
Spec: ../spec/0006-ssh-hosts.md (flow 6: restored SSH tabs wait for Enter)
