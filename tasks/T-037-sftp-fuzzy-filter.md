---
status: later
blocked-by: [T-023]
---
# T-037: SFTP fuzzy filter

Typing in the SFTP sidebar fuzzy-filters the current folder's listing, so a
file in a long folder is a few keystrokes away. Matches rank best first and
highlight the matched characters; clearing the filter or changing folder
restores the full listing. Filters only what's already listed: no recursive
search on the server. Planned by the owner (2026-11-06); needs spec 0008
extended first. Area: src.

Spec: ../spec/0008-sftp-panel.md
