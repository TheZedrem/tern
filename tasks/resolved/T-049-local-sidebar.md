---
status: resolved
blocked-by: []
---
# T-049: Local files in the sidebar's Files panel

With a local pane active, the sidebar's Files panel shows the file
browser's Local target instead of a text: at once, since nothing
connects, also after a pane switch and a relaunch. It starts in the
folder the pane's shell last reported (OSC 7), else the pane's start
folder, and each pane keeps its own browser while it lives (spec 0018,
Files panel). A program run from it opens in a pane beside the active
one, as from a files pane. With a files pane active the panel says that
the pane has its own browser. Requested by the owner (2026-11-09, again
2026-11-12 after trying T-080). Area: src.

Spec: ../spec/0018-app-sidebar.md
Spec: ../spec/0013-files-panel.md
