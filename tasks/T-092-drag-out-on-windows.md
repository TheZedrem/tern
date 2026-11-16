---
status: later
blocked-by: [T-027]
---
# T-092: Drag files out of the file browser on Windows

T-027's drag out works on Wayland only, because `gpui-pre` 0.3.11 has no
drag source on Windows. The Windows drag source (`DoDragDrop` with
`CF_HDROP`, copy only) belongs upstream in GPUI, next to its existing OLE
drop target, not as a patch carried across bumps. Start this once a pinned
GPUI has it: then the browser's drag out needs only a check on Windows 11.
Area: src.

Spec: ../spec/0013-files-panel.md
