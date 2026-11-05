---
status: next
blocked-by: []
---
# T-006: Session restore

Debounced atomic `session.json` saves; on boot, spawn only the active tab's
panes and spawn other tabs lazily on first activation; handle corrupt files.
Restored SSH panes start as `terminal::Start::OnEnter` (T-012 built it for
an SSH tab at boot), so they connect only on Enter. A tab never activated
that's dragged into a split starts its panes once they're visible (spec
0007, flow 7; T-020 has no unstarted panes to move yet). Verify that TTFP
with a restored 4-pane tab stays under budget. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
Spec: ../spec/0006-ssh-hosts.md (flow 6: restored SSH tabs wait for Enter)
Spec: ../spec/0007-drag-and-drop-splits.md (flow 7: unstarted panes moved into view)
