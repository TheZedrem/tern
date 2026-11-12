---
status: later
blocked-by: [T-064, T-066, T-080]
---
# T-069: Plugin panels in the app sidebar

A plugin's manifest panels appear in the app sidebar; `set_panel`,
`append_log` and `open_panel` fill and show them, clicks and input come
back as `panel_action` and `panel_input`, a log keeps its last 10,000
lines, and a stopped plugin's panels offer Restart (spec 0016). Draws
command buttons with all of their command under the label, disabled
without the right, ignoring clicks for 0.5 s after the panel changed
(§Model Command buttons); T-068 carries the click out. Needs the
app sidebar's spec and build first. Builds threat model 0005's
TM-plug-01 (panel header), -02, -10 (visible rows only), -17 and -23.
Area: src.

Spec: ../spec/0016-plugin-system.md
Spec: ../spec/0018-app-sidebar.md
