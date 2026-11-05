---
status: later
blocked-by: [T-023]
---
# T-027: Drag already-local files out of the SFTP panel

Let the user drag a file that already exists locally in full (an edit
copy, or a file the panel just downloaded) from the SFTP panel into the
file manager, on Wayland and Windows 11 alike. Remote files that aren't
downloaded yet are never draggable (OQ-019 ✅). Area: src.

Blocked on GPUI itself: `gpui-pre` 0.3.11 starts an OS drag only on
Wayland (`start_external_drag` returns `false` on Windows). The Windows
drag source (`DoDragDrop` with `CF_HDROP`, copy only) belongs upstream
in GPUI, next to its existing OLE drop target, not as a patch carried
across bumps. Start this task once a pinned GPUI has it on both platforms.

Spec: ../spec/0008-sftp-panel.md
