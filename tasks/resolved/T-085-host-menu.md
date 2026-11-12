---
status: resolved
blocked-by: []
---
# T-085: Edit and delete a host from the sidebar's SSH hosts panel

A right click on a host in the SSH hosts panel (or the Menu key /
Shift+F10 on the selected one) opens a menu with Connect, Edit… and
Delete…. Edit… opens the settings screen's profile form for that
profile; Delete… asks in the window's question dialog, Cancel being the
default and Esc, and only then removes the profile from `config.yaml`
by the settings screen's checked edit. Delete or Backspace on the
selected host asks the same. A `~/.ssh/config` host shows both entries
disabled, "Defined in ~/.ssh/config". The list reloads after either.
Choices awaiting the owner in OQ-039. Requested by the owner
(2026-11-12). Area: src.

Spec: ../spec/0018-app-sidebar.md
