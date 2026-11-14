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

**Not reproduced on Windows 11 (2026-11-11):** a restored row split of a
terminal and a Local files pane, `focused` on the files pane, gave the
files pane the border and the keyboard both when shown at boot
(`open_new_tab: false`) and when its tab was clicked behind a fresh one.
A GPUI test (`a_restored_tab_focused_on_its_files_pane_keeps_that_focus`
in `src/layout/mod.rs`) passes too. Next: reproduce on Linux (Wayland);
the owner's steps from T-046 (which tab, `open_new_tab`, any click
before typing) would narrow it.

**Code read (2026-11-14):** the saved `focused` survives boot on every
path (`Boot::restore`, `restore_behind`, `focus_active`), and only two
things move a tab's focus to a terminal pane: a left or right click in
it (`PaneEvent::Focused`) and the `pane.focus*` keys. A click that
activates the Tern window lands on the pane under the pointer on
Wayland, which would give exactly this report. Next: the owner's steps,
in particular whether the window was clicked into before typing.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
