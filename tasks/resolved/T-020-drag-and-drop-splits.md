---
status: next
blocked-by: []
---
# T-020: Drag tabs and panes into and out of splits

Local and SSH panes in one tab: a local shell beside an SSH session, or
two servers side by side, where today a split always opens the source
pane's profile. Drop a tab or a pane on a side of a pane to split it
there, drag a pane by its 6-dot handle to the tab bar to make it a tab
again, and split with any profile from the palette. Asked for by the
owner during T-014, who settled the design; spec 0007 pins it and needs
the owner's approval first. Builds on the tab drag of T-003. Area: src.

Spec: ../spec/0007-drag-and-drop-splits.md

## Result (2026-11-05)

Not on the boot path: the first frame draws one pane, which has no
handle, so no TTFP run was needed.

Built: the split tree puts a pane or a whole tree on any side of a pane
(`Node::insert`), with room only if each half of the even split holds its
side at the 10 × 3 minimum (`Node::can_insert`). `Tabs` drops a tab or a
pane into the active tab's split, moves a pane out to a tab of its own,
and closes panes through the same detach. A pane in a split tab shows a
6-dot grab handle on hover; while a tab or pane is dragged, the pane side
nearest the pointer is marked, a drop there moves it, and a pane dropped
on the tab bar becomes the tab after the one under it, or the last tab.
"Split right with profile…" and "Split down with profile…"
(`pane.splitRightWithProfile`, `pane.splitDownWithProfile`) pick from the
local profiles (group "Local") and the host picker's hosts.

Verified: 327 tests, the 15 new ones written by a separate agent against
spec 0007 (AC1–AC5 at the model, the commands, the picker entries). One
caught the room check adding both minimums together instead of fitting
each into its half of the even split; fixed before the commit. The debug
build boots and renders (`--bench-startup 1`).

Not verified: dragging in the running window (no input automation here),
so AC6–AC8 and the on-screen halves of AC1–AC5; Windows (added to T-011).
Flow 7 waits for T-006: before session restore there are no unstarted
panes to move.
