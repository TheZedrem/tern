---
status: next
blocked-by: [T-010]
---
# T-003: Tabs and split panes

Tab bar (new, close, reorder, rename, OSC titles) and the binary split tree
(split, close, drag-resize, directional focus, zoom), as GPUI views over
the layout model. The tab bar doubles as the title bar on GNOME, where the
compositor draws no frame. Expose actions as plain functions now; T-004
registers them as GPUI actions. Widgets are hand-built on GPUI primitives
unless the owner settles OQ-012 the other way first. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
