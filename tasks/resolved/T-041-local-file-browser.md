---
status: later
blocked-by: [T-042]
---
# T-041: Local target for the file browser

The Local target: browse the local disk on a background thread, open a
file in the OS's default app, run or edit a runnable file per
`files_executable` (ask by default; edit-only types open as plain text),
rename with a confirmation onto an existing name, new folder and delete
after the same confirmation as on SFTP (permanent, OQ-026 ✅; a folder
symlink on Windows is a `remove_dir`). Windows drives in the path field.
Planned by the owner (2026-11-07). Area: src.

Spec: ../spec/0013-files-panel.md
