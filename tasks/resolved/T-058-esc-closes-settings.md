---
status: now
blocked-by: []
type: bug
---
# T-058: Esc closes the settings screen

Pressing Esc on the settings screen should close it wherever focus is in
the panel: in the search field, in any other field, or after a click on a
tab or empty space. Spec 0009 already pins this (flow 1 and acceptance
criterion 8), but the owner reported on 2026-11-09 that Esc doesn't close
the panel. Area: src.

Where to look: `SettingsView` binds `escape` to `CloseSettings` in its own
key context (`src/settings/mod.rs`), but opening the panel focuses the
search input. gpui-component's Input binds `escape` in its deeper context,
so it probably handles the key and the panel's binding never fires. A
click that leaves no element focused inside the panel would also lose the
key. Fields that use Esc to cancel an edit (the key recorder in the
Keyboard tab) keep that behavior: there, the first Esc cancels the edit
and the next one closes the panel.

Spec: ../../spec/0009-settings-screen.md

Resolved: the search input never swallowed Esc. A tab click (or the
profile form closing) stopped drawing the field that had the keyboard,
which left focus on an element that isn't drawn; keys then dispatch from
the window root and miss the panel's binding. Those three handlers now
give the keyboard back to the panel (`SettingsView::keep_keyboard`).
