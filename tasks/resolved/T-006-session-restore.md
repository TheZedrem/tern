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

## Result (2026-11-05)

TTFP warm median 293.7 ms with one pane (p90 300.5, cold 365.9) and
304.3 ms with a restored 4-pane tab (p90 362.9, cold 304.4; the four PTY
spawns take 6.1 ms), budget 500 ms; T-020 measured 338.1 ms. The boot
path now reads `session.json` before the window and starts the shown
tab's panes there.

Built: `src/session.rs` holds the format (version 1: window size, tabs
with their name, split tree, focused and zoomed pane, and per pane its
profile ID, cwd and title), written atomically and readable by the user
only. Boot restores the shown tab's panes at their share of the saved
window; the other tabs' panes start when their tab is first shown or
dragged into view, SSH ones waiting for Enter; a missing profile opens
`default_profile` with a notice. The workspace saves a second after any
layout, title or window-size change, saves on close, saves an empty
session when the last tab closes, and deletes the file with
`restore_session: false`. A file that can't be restored boots one fresh
tab with a notice and is renamed `session.json.bad` after the first frame,
since the boot path writes no files. `ZED_TERM_BENCH_SESSION` lets the
bench restore a test session; the bench never touches the user's.

Verified: 343 tests, the 16 new ones written by a separate agent against
spec 0003. Its review also caught the `.bad` rename on the boot path, raw
titles on tabs not yet shown, an ID overflow from a hand-edited file and
the zoomed pane's boot size, all fixed. The bench restored a 4-pane test
session with a missing directory and a missing profile.

Not verified: the running app (the final save on close, restoring
window size and titles, starting panes on first activation), which the
owner tests at the end; Windows (T-011). cwd changes save only with the
next layout change or on close until T-002's OSC 7 tracking can trigger a
save.
