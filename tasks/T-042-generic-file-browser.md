---
status: later
blocked-by: []
---
# T-042: Generalize the SFTP sidebar into a file browser

The sidebar becomes a file browser over a backend: what it lists, opens,
transfers and edits goes through one backend interface, and SFTP is the
first backend. That's what lets a local backend (T-041) and later S3 or
SMB backends from plugins (architecture §Roadmap, Considered) reuse the
same sidebar, header, transfers and edit flow. The UI and behavior of
spec 0008 don't change. Planned by the owner (2026-11-07); needs spec 0008
and the code layout in `spec/architecture.md` extended first. Area: src.

Spec: ../spec/0008-sftp-panel.md
