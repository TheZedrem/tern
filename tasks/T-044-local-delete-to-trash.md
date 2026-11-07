---
status: later
blocked-by: [T-041]
---
# T-044: Local delete moves to the trash

Deleting on the files panel's Local target moves the file to the OS's
trash (the freedesktop trash on Linux, the Recycle Bin on Windows)
instead of deleting it for good, so a slip can be undone in the file
manager. Needs the `trash` crate, so ask before adding it, and spec 0013
extended first (OQ-026 ✅ picked permanent delete for now). Planned by the
owner (2026-11-07). Area: src.

Spec: ../spec/0013-files-panel.md
