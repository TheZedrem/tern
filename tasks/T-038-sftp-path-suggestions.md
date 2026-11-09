---
status: later
blocked-by: [T-023]
---
# T-038: Path suggestions in the file browser

Typing in the file browser's path field opens a dropdown of the folders
under what's typed so far, fuzzy-matched and best first, as an IDE suggests
completions. Arrow keys pick, Tab completes, Enter opens; typing past a `/`
lists that folder on the target. Built once in the browser, so it works on
every target and both surfaces. Uses the same matcher as the filter
(T-037). Planned by the owner (2026-11-06, made a browser feature
2026-11-09); needs spec 0013 extended first. Area: src.

Spec: ../spec/0013-files-panel.md
