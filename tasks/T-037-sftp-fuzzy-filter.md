---
status: later
blocked-by: [T-023]
---
# T-037: Fuzzy filter in the file browser

Typing in the file browser fuzzy-filters its tree, so a file in a long
folder is a few keystrokes away. Built once in the browser, so it works on
every target and both surfaces (sidebar and files pane). Matches rank best
first and highlight the matched characters. A match inside an expanded
folder keeps its parent folders visible. Clearing the filter restores the
full tree. Filters only what's already loaded: no recursive search on disk
or on the server. Planned by the owner (2026-11-06, made a browser feature
2026-11-09); needs spec 0013 extended first. Area: src.

Spec: ../spec/0013-files-panel.md
