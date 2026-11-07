---
status: next
blocked-by: []
---
# T-045: Check the files pane on Windows 11

Spec 0013 AC10 on a Windows 11 machine: Local lists `C:\`, the path field
opens `D:\`, `run.bat` runs through `cmd /c` in a new pane, Edit opens
Notepad (OQ-027), deleting a symlink or junction to a folder removes the
link and not its target, and an SFTP target logs in through `ssh.exe`.
Built on Linux only so far (T-041, T-043). Area: src.

Spec: ../spec/0013-files-panel.md
