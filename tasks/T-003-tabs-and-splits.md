---
status: now
blocked-by: []
---
# T-003: Tabs and split panes

Tab bar (new, close, reorder, rename, OSC titles) and the binary split tree
(split, close, drag-resize, directional focus, zoom), as GPUI views over
the layout model. The tab bar doubles as the title bar on GNOME, where the
compositor draws no frame. Tab and split operations are GPUI actions with
spec 0004's default keys, so they work from the keyboard now; T-004 adds
their config IDs, user overrides and the palette. Build the tab bar, title
bar and window border from `gpui-component` (OQ-012). Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
