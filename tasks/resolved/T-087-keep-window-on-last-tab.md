---
status: next
blocked-by: []
---
# T-087: Setting to keep the window open when the last tab closes

`close_last_tab: quit | new_tab` (default `quit`). With `new_tab`,
anything that closes the last tab (close button, middle click,
Ctrl+Shift+W on its last pane, its program exiting under
`close_on_exit`) opens one fresh `default_profile` tab instead of
quitting; closing the window itself still quits. The SFTP-edit question
comes first as for any tab. On the settings screen's General page next
to `open_new_tab`, with its template line in `src/default-config.yaml`
(spec 0009). Requested by the owner (2026-11-12). Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
Spec: ../spec/0005-config-and-themes.md
Spec: ../spec/0009-settings-screen.md
