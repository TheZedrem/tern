---
status: next
blocked-by: [T-010]
---
# T-003: Tabs and split panes

Tab bar (new, close, reorder, rename, OSC titles) and the binary split tree
(split, close, drag-resize, directional focus, zoom), drawn with `egui`
over the layout model. Expose actions as plain functions now; T-004
registers them as commands. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
