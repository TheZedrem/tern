---
status: now
blocked-by: []
---
# T-025: Vertical tabs

`tab_bar: left` lists the tabs down the left side instead of a bar on
top, with the same clicks, double-click rename, close, drag to reorder
and pane drops; default `top`. Asked for by the owner after T-017; the
setting is on the settings screen (Appearance → Tabs), per the standing
rule. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
Spec: ../spec/0005-config-and-themes.md (`tab_bar`)

## Result (2026-11-05)

Not on the boot path: the first frame draws the bar or the list as the
config says, from the same tab model.

Built: `config::TabBarSide` (`top | left`), the side list in
`Workspace::render_side_tabs` (200 px, the shown tab highlighted, "New
tab" under the list, the space below it taking dropped panes as the last
tab, drop markers above and below), and with the tabs on the side only
the title bar stays on top where the compositor draws no frame. Switching
applies live on reload.

Verified: 405 tests, among them the settings completeness test, which
now covers `tab_bar`; a debug build boots and renders a restored 2-tab
session with `tab_bar: left`.

Then, asked for by the owner: any side. `tab_bar: top | bottom | left |
right`, top and bottom as a bar (only the top one doubles as the title
bar; elsewhere a slim title bar stays on top where the compositor draws
no frame), left and right as the list. A debug build boots and renders
with `bottom` and with `right`.

Not verified: the running window (the owner tests it), Windows (T-011).
