---
status: later
blocked-by: []
---
# T-049: Sidebar on local panes

`Ctrl+Shift+S` on a local pane opens the sidebar too, showing the file
browser's Local target, starting in the folder the pane's shell is in
(OSC 7, else the pane's start folder). Same sidebar model as an SSH pane
(spec 0008 §Model): a flag per pane, a session per pane, kept while the
pane lives. The command and setting are named for SFTP today
(`sftp.toggle`, `sftp_sidebar`, "Toggle SFTP sidebar"); the spec decides
their new names and keeps the old ones working in existing configs.
Requested by the owner (2026-11-09); needs specs 0013 and 0008 extended
first. Area: src.

Spec: ../spec/0013-files-panel.md
Spec: ../spec/0008-sftp-panel.md
