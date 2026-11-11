---
status: later
blocked-by: [T-064, T-066]
---
# T-069: Plugin panels in the app sidebar

A plugin's manifest panels appear in the app sidebar; `set_panel`,
`append_log` and `open_panel` fill and show them, clicks and input come
back as `panel_action` and `panel_input`, a log keeps its last 10,000
lines, and a stopped plugin's panels offer Restart (spec 0016). Needs the
app sidebar's spec and build first. Area: src.

Spec: ../spec/0016-plugin-system.md
