---
status: next
blocked-by: []
---
# T-091: Check middle-click close and `close_last_tab` in the running app

T-086 and T-087 were built on Linux and tested in GPUI's test window
only; nobody clicked them in the running app. On Linux (Wayland) and
Windows 11, run spec 0003 AC14 (middle click on an inactive tab, on a
side-list tab, press on one tab and release on another, the empty bar,
a pane) and AC15 (`close_last_tab: new_tab` by close button,
Ctrl+Shift+W and `exit`; the window's own close button still quits;
`quit` still quits), plus the setting's row on the General page.
Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
