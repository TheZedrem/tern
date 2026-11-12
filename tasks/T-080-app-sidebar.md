---
status: later
blocked-by: [T-064]
---
# T-080: Build the app sidebar with the SSH hosts panel

`Ctrl+Shift+L` (`sidebar.toggle`) shows a sidebar for the whole window,
on the `app_sidebar` side between the tab list and the pane sidebar,
resizable by its inner edge; `sidebar.focus` moves the keyboard into it
and Esc gives it back to the shell. Its first panel lists the SSH hosts
of the picker with a filter, and Enter or a double click opens one in a
new tab. Panels are registered through one extension point
(`sidebar::Panel`) that the hosts panel uses like any plugin panel will
(T-069), with the strip, the header and the order of spec 0018. Open,
width and panel are saved in `session.json`; a sidebar saved open is on
the first frame as an empty column, filled after it. Includes the
`app_sidebar` setting on the settings screen with its template line in
`src/default-config.yaml`, the README's Status, the architecture and
spec lines spec 0018 names under Affected areas, and the
`--bench-startup 10` comparison with the sidebar saved open against
closed. Checked on Windows 11 too (ConPTY redraw on toggle). Needs spec
0018 approved first. Area: src.

Spec: ../spec/0018-app-sidebar.md
