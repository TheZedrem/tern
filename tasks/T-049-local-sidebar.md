---
status: later
blocked-by: []
---
# T-049: Sidebar on local panes

`Ctrl+Shift+S` on a local pane opens the sidebar too, showing the file
browser's Local target, starting in the folder the pane's shell is in
(OSC 7, else the pane's start folder), with a session per pane, kept
while the pane lives. Since spec 0018 (2026-11-12) the sidebar is the
window's and `Ctrl+Shift+S` is `sidebar.toggle`, so this task fills the
Files panel on a local pane, where T-080 leaves a text; the renames
(`sidebar.toggle`, `app_sidebar`) come with T-080. Requested by the owner
(2026-11-09); needs specs 0013 and 0018 extended first. Area: src.

Spec: ../spec/0013-files-panel.md
Spec: ../spec/0008-sftp-panel.md
Spec: ../spec/0018-app-sidebar.md
