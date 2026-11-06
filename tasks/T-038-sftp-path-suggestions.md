---
status: later
blocked-by: [T-023]
---
# T-038: SFTP path suggestions

Typing in the SFTP sidebar's path field opens a dropdown of the folders
under what's typed so far, fuzzy-matched and best first, as an IDE suggests
completions. Arrow keys pick, Tab completes, Enter opens; typing past a `/`
lists that folder on the host. Uses the same matcher as the listing filter
(T-037). Planned by the owner (2026-11-06); needs spec 0008 extended first.
Area: src.

Spec: ../spec/0008-sftp-panel.md
