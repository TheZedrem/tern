---
status: next
blocked-by: []
type: bug
---
# T-063: A restored tab loses focus on its files pane

A saved tab whose focused pane is a files pane restores with the keyboard
in its terminal pane instead, and the active pane border (spec 0003,
flow 3) follows the terminal. Seen while checking T-046: a row split of
a terminal and a Local files pane, `focused` on the files pane in
`session.json`. Restore should focus the leaf the session saved. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
