---
status: next
blocked-by: []
---
# T-006: Session restore

Debounced atomic `session.json` saves; on boot, spawn only the active tab's
panes and spawn other tabs lazily on first activation; handle corrupt files.
Verify that TTFP with a restored 4-pane tab stays under budget. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
