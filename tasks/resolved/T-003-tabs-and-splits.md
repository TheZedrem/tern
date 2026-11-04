---
status: now
blocked-by: []
---
# T-003: Tabs and split panes

Tab bar (new, close, reorder, rename, OSC titles) and the binary split tree
(split, close, drag-resize, directional focus, zoom), as GPUI views over
the layout model. The tab bar doubles as the title bar on GNOME, where the
compositor draws no frame. Tab and split operations are GPUI actions with
spec 0004's default keys, so they work from the keyboard now; T-004 adds
their config IDs, user overrides and the palette. Build the tab bar, title
bar and window border from `gpui-component` (OQ-012). Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md

## Result (2026-11-04)

TTFP warm median 341.7 ms (p90 370.1, cold 980.9), up from 329.8 ms for
the tab bar and window border on the first frame; budget 500 ms.

Built: tabs (new, close, drag to reorder, double-click to rename, OSC
titles), the binary split tree (split, close, divider drag, Alt+arrow
focus, zoom), the default keys from spec 0004, and the GNOME title bar
(the tab bar's free space moves and maximizes the window, plus window
controls and resize edges). Split dividers are our own: the layout model
keeps the ratios, which session restore will save. A split without room
for two minimum panes does nothing, and split halves carry minimum sizes.

Verified: 104 tests, written by a separate agent against spec 0003, cover
the split tree, the tab model, title cleaning, the split-room check and
spawning in a given or missing directory. The app boots normally, in bench
mode and with `ZED_TERM_CLIENT_DECORATIONS=1`. Not verified: anything
needing keyboard or mouse input in the running window (no input
automation here), and GNOME itself. The owner checks those by hand.

Fixed on the way: a split from a deleted directory started in zed-term's
own directory instead of home; tab titles could carry invisible
text-direction and tag characters that hide or reorder text.
