---
status: resolved
blocked-by: []
---
# T-080: Build the sidebar with the Files and SSH hosts panels

`Ctrl+Shift+S` (`sidebar.toggle`, taking the key over from `sftp.toggle`)
shows one sidebar for the whole window on the `app_sidebar` side,
resizable by its inner edge. It stays open across tabs and opens on the
panel it showed last, Files the first time; `sidebar.focus` moves the
keyboard into it and Esc gives it back to the shell. The per-pane sidebar
goes: its file browser and port forwards become the Files panel, drawn
for the active pane, and a switch to an SSH pane without a session
connects nothing and offers "Show files". The SSH hosts panel lists the
picker's hosts with a filter, and Enter or a double click opens one in a
new tab. Panels are registered through one extension point
(`sidebar::Panel`) that the built-in panels use like any plugin panel
will (T-069), with the strip, the header and the order of spec 0018.
Open, width and panel are saved in `session.json`; a sidebar saved open
is on the first frame as an empty column, filled after it. Includes the
`sftp.toggle` old name in `keys:`, the `sftp_sidebar` to `app_sidebar`
migration (old key still read, rewritten by the settings screen), the README's Status, the amendments to
specs 0008, 0017 and 0004, threat model 0003 and the architecture that
spec 0018 names under Affected areas, and the `--bench-startup 10`
comparison with the sidebar saved open against closed. Checked on
Windows 11 too (ConPTY redraw on toggle).
Area: src.

Spec: ../spec/0018-app-sidebar.md
