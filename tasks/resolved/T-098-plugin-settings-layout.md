---
status: resolved
blocked-by: []
---
# T-098: Give each plugin a readable entry in Settings

The Windows screenshot of the disabled Docker plugin shows its name
centered in a mostly empty left column. The switch, version, actions,
command, folder and raw permission ID compete for space on the right.
Use the proposed layout in spec 0016 for all found plugins. Check the
disabled Docker entry, a running plugin and a stopped plugin at narrow
and normal window widths, including settings search results.

Area: src. Spec: ../spec/0016-plugin-system.md

Verified the disabled Docker card in the native Windows renderer. GPUI
tests cover the Plugins tab and search at 575 and 1050 px, wrapping a
long command, log access and restart availability. Restart from Settings
also resends the active target to a visible pane-following plugin.
