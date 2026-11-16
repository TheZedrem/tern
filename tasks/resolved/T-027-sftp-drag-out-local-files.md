---
status: now
blocked-by: []
---
# T-027: Drag files that are already local out of the file browser

Let the user drag a Local row, or an SFTP edit's copy, out of the file
browser into the file manager. Remote files that aren't downloaded yet are
never draggable (OQ-019 ✅). Wayland only: `gpui-pre` 0.3.11 starts an OS
drag only there (`start_external_drag` returns `false` on Windows). The
owner chose to ship it on Linux now (2026-11-16); Windows follows in T-092.
Area: src.

Spec: ../spec/0013-files-panel.md
