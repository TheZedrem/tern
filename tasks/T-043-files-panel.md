---
status: later
blocked-by: [T-042]
---
# T-043: Files pane

The files pane (`files.open`, `Ctrl+Shift+B`; `files.newTab`): a leaf of
the split tree that draws the file browser, opens on the focused pane's
target, switches between Local and any saved SSH host in its header with
one session per target, and splits, moves, zooms, becomes a tab and
restores like a terminal pane (specs 0003, 0007). Includes the files
leaf in `session.json`, the `files_executable` setting on the settings
screen and in `src/default-config.yaml`, and the TTFP with a restored
files pane in the active tab. Area: src.

Spec: ../spec/0013-files-panel.md
