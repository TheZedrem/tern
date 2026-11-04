---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-003, T-006, T-011]
related: [spec 0001, spec 0002, spec 0005]
---

# Spec: Tabs, split panes and session restore

Pins how panes are arranged (tabs, each holding a split tree) and how that
arrangement survives a restart without costing startup time.

## Problem & goal

Users juggle several shells per project and expect a terminal to come back
the way they left it. Restoring must not turn "open 8 tabs" into "spawn 8
shells at boot".

Done means: tabs and splits work entirely from the keyboard and mouse, and
relaunching restores the layout with only the visible tab's shells started
immediately.

## Scope

**In scope:**

- Tabs: new, close, reorder (drag), rename, next/prev/go-to-N, title from the shell (OSC 0/2) or the user
- Splits: split right/down, close pane, resize by dragging a divider, directional focus, zoom (maximize) a pane
- Session restore: layout, titles and cwd per pane
- Title bar: on compositors that draw no window frame (GNOME), the tab bar doubles as the title bar

**Out of scope:**

- Multiple windows, detaching a tab into a window: *Later*
- Restoring scrollback or running programs: never (`architecture.md` §Ground rules)
- Tab colors / profile colors: Phase 2 (host profiles)

## Design

### Affected areas

- `src/layout/` (model, tab bar, title bar, split view), `src/session.rs`

### Model

```
Session  { window: { width, height }, tabs: Tab[], activeTab }
Tab      { id, title?, root: Node, focusedLeaf, zoomedLeaf? }
Node     = Split { direction: row|column, ratio, a: Node, b: Node }
         | Leaf  { id, pane: PaneId | null, profile, cwd? }  // null = not spawned yet
```

The tree is binary: repeated splits nest. No pane drops below 10 cols × 3
rows: divider drags are clamped, a split without room for two such panes
does nothing, and a shrinking window stops at the tree's minimum size.
Leaf IDs are stable across restarts;
**`PaneId` is per run and never written to `session.json`** **[load-bearing]**.

A tab's title is the user's rename if set, else the focused pane's OSC
title, else the profile's display name.

### Persistence

- Saves write atomically (temp file, then rename) from a background thread.
- Boot reads `session.json` before the window exists and spawns the active
  tab's leaves right away (spec 0001).

### Flows

1. **Split:** the new pane opens the source pane's profile in the source
   pane's cwd (spec 0002) and takes focus; a cwd that no longer exists
   falls back to home. A new tab opens `default_profile` (spec 0005).
2. **Close pane:** the PTY is killed and the sibling takes the freed space.
   Closing the last pane closes the tab, and closing the last tab quits the
   app.
3. **Focus:** directional focus picks the nearest pane in that direction by
   geometry. Clicking a pane focuses it.
4. **Zoom:** the focused pane fills the tab while other panes keep running.
   Toggling again restores the layout.
5. **Save:** debounced at 1 s after any layout, title, cwd or window-size
   change, plus a final save on quit. A save failure shows a notice
   (architecture §Cross-cutting rules) and never blocks.
6. **Restore:** the active tab's leaves spawn at boot. Other tabs render as
   tab-bar entries only, and their leaves spawn on first activation. A
   missing cwd falls back to the home directory. A leaf whose profile no
   longer exists opens `default_profile` with a notice. SSH leaves restore
   disconnected and connect only on Enter (spec 0006).
7. **Empty, corrupt or unknown-version `session.json`:** boot one fresh tab.
   Quitting by closing the last tab saves an empty session, which is the
   normal case and leaves no `.bad` file. A corrupt or unknown-version file is renamed
   `session.json.bad` (overwritten on the next occurrence) so the user's
   next save doesn't silently discard it.
8. **Title bar:** when GPUI reports client-side decorations (the compositor
   draws no frame), the tab bar becomes the title bar. Dragging its empty
   area moves the window, double-clicking it toggles maximize, it shows
   minimize, maximize and close buttons at the end, and the window edges
   resize. With server-side decorations none of this is drawn.

### Startup impact

The first frame draws the tab bar and split view; they are GPUI views over
the layout model and cost nothing to initialize. Restore adds one
small file read and N PTY spawns for the active tab only. The TTFP report
covers a restored 4-pane tab (spec 0001).

### Platform notes

GNOME offers no server-side decorations to Wayland clients, so it is where
the title-bar flow runs; KDE, COSMIC and wlroots compositors draw the
frame themselves. Windows 11 always draws its own frame.

### Security & privacy

- Stored: `<data_local_dir>/zed-term/session.json` holds window size, layout,
  and profile ID, cwd and title per pane. No command history, no scrollback, no environment.
  Overwritten on every save. Setting `restore_session: false` deletes it on
  the next quit.
- Trust boundaries: crosses §Security 1, because tab titles come from
  terminal output (OSC 0/2). They render as plain text on one line, capped
  at 200 characters, with control characters and invisible ones that could
  hide or reorder text (zero-width spaces, direction overrides, tag
  characters) stripped.

## Acceptance criteria

1. Split right, split down, then close the middle pane: the remaining panes fill the tab with no dead space.
2. Dragging a divider resizes both panes, and each PTY gets at most one resize per frame.
3. Quit with 5 tabs (one with 4 panes), relaunch: the window size, layout and titles come back, and only the active tab's shells are running (process list).
4. Activating a restored tab for the first time starts its shells in the saved cwd.
5. A saved cwd that no longer exists opens the home directory without an error dialog.
6. A truncated `session.json` boots one fresh tab and leaves `session.json.bad` behind. Closing the last tab and relaunching boots one fresh tab with no `.bad` file.
7. TTFP with a restored 4-pane active tab stays under budget (spec 0001).
8. `session.json` contains no scrollback, commands or environment variables.
9. A 5 000-character title set via OSC 2 shows in the tab bar capped at 200 characters, on one line, and the tab bar keeps its layout.
10. On GNOME, the window moves by dragging the tab bar's empty area, resizes from its edges, and closes from its close button.

## Open questions

- Fallback tab title: the profile's display name for now. Foreground process name or cwd basename is a *Later* refinement.
