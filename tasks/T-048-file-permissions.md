---
status: later
blocked-by: []
---
# T-048: File permissions in the file browser

View and change a file's mode, owner and group in the file browser, on
every target that supports them, from both surfaces. SFTP sets them with
`setstat` (numeric owner and group; names come from the listing), and a
Linux disk the same way. A target says whether it supports permissions,
and the browser offers them only where it does. A Windows disk has ACLs,
not mode bits: the spec decides what it offers there (e.g. the read-only
flag plus a way to open the Explorer properties dialog). Planned by the
owner for 1.0 (2026-11-09); needs spec 0013 extended first. Area: src.

Spec: ../spec/0013-files-panel.md
