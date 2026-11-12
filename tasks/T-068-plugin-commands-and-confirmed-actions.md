---
status: later
blocked-by: [T-066]
---
# T-068: Plugin commands and confirmed terminal actions

Manifest commands in the palette and the keymap (`plugin.<plugin>.<command>`),
`notify`, and the confirmed actions `fill_command`, `run_command`,
`read_output` and `open_tab` with the confirmation bar on the target pane
that never takes keyboard focus, plus `plugin.allowRequest` and
`plugin.denyRequest` (spec 0016, §Model Confirmation, flow 5), with threat
model 0005's TM-plug-01 (labels), -03 to -09, -16 (read count) and -20.
Also carries out a command button's click (§Model Command buttons,
TM-plug-23): the same text rules, right check and log line, no bar; the
button itself is drawn in T-069. Checked on
Windows too: it writes to ConPTY. Area: src.

Spec: ../spec/0016-plugin-system.md
Spec: ../spec/0004-commands-keybindings-palette.md
