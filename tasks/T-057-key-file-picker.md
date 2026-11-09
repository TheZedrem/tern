---
status: next
blocked-by: []
---
# T-057: A file picker for an SSH profile's key file

The profile form's key file field gets a "Browse…" button that opens the
OS's file picker (the XDG portal on Linux) and fills in the pick, `~` for
home, as the directory field's folder picker already does. Typing the path
keeps working. Asked for by the owner (2026-11-09); needs spec 0009's
flow 4 extended first. Area: src.

Spec: ../spec/0009-settings-screen.md
