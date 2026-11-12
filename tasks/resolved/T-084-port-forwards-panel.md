---
status: resolved
blocked-by: []
---
# T-084: Port forwards as a sidebar panel of its own

Port forwards leaves the Files panel's switch and becomes a built-in
panel (`forwards`) after SSH hosts in the strip, listed only while the
active pane is an SSH pane (spec 0018, Port forwards panel). Switching
to a local pane while it is shown shows Files; `pane.portForwards`
opens the sidebar on it with the keyboard in the add row; a saved
`forwards` comes back only on an SSH pane. The Files panel loses its
switch. Amends spec 0017's flow 1 and its sidebar wording. Requested by
the owner (2026-11-12). Area: src.

Spec: ../spec/0018-app-sidebar.md
Spec: ../spec/0017-port-forwarding.md
