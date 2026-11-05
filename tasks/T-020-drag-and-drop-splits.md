---
status: next
blocked-by: []
---
# T-020: Drag tabs and panes into and out of splits

Local and SSH panes in one tab: a local shell beside an SSH session, or
two servers side by side. Today a split always opens the source pane's
profile. Asked for by the owner during T-014, who settled the design:

- Dropping a tab or a pane on one of a pane's four sides puts it there,
  splitting that pane; a tab with splits of its own brings its whole
  layout.
- Dragging a pane to the tab bar makes it a tab of its own again.
- A pane in a split tab has a small 6-dot grab handle in its top right
  corner; a mouse drag anywhere else in a pane selects text.
- "Split with profile…" in the palette is the keyboard way to put another
  host next to a pane. Keyboard commands to move panes come later.

Spec 0003 doesn't pin any of it yet: revise it first and get the owner's
approval. Builds on the tab drag of T-003. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
