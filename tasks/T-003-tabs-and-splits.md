---
status: next
blocked-by: []
---
# T-003: Tabs and split panes

Tab bar (new, close, reorder, rename, OSC titles) and the binary split tree
(split, close, drag-resize, directional focus, zoom), as GPUI views over
the layout model. The tab bar doubles as the title bar on GNOME, where the
compositor draws no frame. Expose actions as plain functions now; T-004
registers them as GPUI actions. Build the tab bar, title bar, window border
and split dividers from `gpui-component` (OQ-012). Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
