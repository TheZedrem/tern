---
status: now
blocked-by: []
---
# T-046: Show clearly which pane is active

In a split tab it's hard to tell which pane has the keyboard, more so
with a files pane beside terminals. The active pane should stand out at a
glance: an accent border on the focused pane and a light dim on the
others, for terminal and files panes alike, in every theme (spec 0003
flow 3, chosen by the owner 2026-11-10). Both configurable:
`active_pane_border`, `inactive_pane_dim`, on the settings screen's
Appearance page and in `src/default-config.yaml` (spec 0005, 0009). Asked for by the owner
(2026-11-07). Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
Spec: ../spec/0005-config-and-themes.md
Spec: ../spec/0009-settings-screen.md
