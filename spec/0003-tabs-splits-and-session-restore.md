---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-003, T-006]
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

**Out of scope:**

- Multiple windows, detaching a tab into a window: *Later*
- Restoring scrollback or running programs: never (`architecture.md` §Ground rules)
- Tab colors / profile colors: Phase 2 (host profiles)

## Design

### Affected areas

- `src/layout/` (store, tab bar, split view), `src-tauri/src/session.rs`

### Model

```
Session  { window: { width, height }, tabs: Tab[], activeTab }
Tab      { id, title?, root: Node, focusedLeaf, zoomedLeaf? }
Node     = Split { direction: row|column, ratio, a: Node, b: Node }
         | Leaf  { id, pane: PaneId | null, profile, cwd? }  // null = not spawned yet
```

The tree is binary: repeated splits nest. Ratios are clamped so no pane
drops below 10 cols × 3 rows. Leaf IDs are stable across restarts;
**`PaneId` is per run and never written to `session.json`** **[load-bearing]**.

A tab's title is the user's rename if set, else the focused pane's OSC
title, else the profile's display name.

### IPC surface

- `session_save(session)`: writes atomically (temp file, then rename).
- Boot: Rust reads `session.json` and includes the active tab's layout in
  `__BOOT__`, with PTYs already spawned for that tab's leaves (spec 0001).

### Flows

1. **Split:** the new pane opens the source pane's profile in the source
   pane's cwd (spec 0002) and takes focus. A new tab opens
   `default_profile` (spec 0005).
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
   longer exists opens `default_profile` with a notice.
7. **Empty, corrupt or unknown-version `session.json`:** boot one fresh tab.
   Quitting by closing the last tab saves an empty session, which is the
   normal case and leaves no `.bad` file. A corrupt or unknown-version file is renamed
   `session.json.bad` (overwritten on the next occurrence) so the user's
   next save doesn't silently discard it.

### Startup impact

The tab bar and split view are in the entry chunk because the first frame
needs them. Drag-reorder and rename UI load on first interaction. Restore
adds one small file read and N PTY spawns for the active tab only. The TTFP
report covers a restored 4-pane tab (spec 0001).

### Platform notes

None beyond spec 0002.

### Security & privacy

- Stored: `<data_local_dir>/zed-term/session.json` holds window size, layout,
  and profile ID, cwd and title per pane. No command history, no scrollback, no environment.
  Overwritten on every save. Setting `restore_session: false` deletes it on
  the next quit.
- Trust boundaries: crosses §Security 1, because tab titles come from
  terminal output (OSC 0/2). They render as plain text only, never HTML,
  capped at 200 characters.

## Acceptance criteria

1. Split right, split down, then close the middle pane: the remaining panes fill the tab with no dead space.
2. Dragging a divider resizes both panes, and each PTY gets at most one resize per frame.
3. Quit with 5 tabs (one with 4 panes), relaunch: the window size, layout and titles come back, and only the active tab's shells are running (process list).
4. Activating a restored tab for the first time starts its shells in the saved cwd.
5. A saved cwd that no longer exists opens the home directory without an error dialog.
6. A truncated `session.json` boots one fresh tab and leaves `session.json.bad` behind. Closing the last tab and relaunching boots one fresh tab with no `.bad` file.
7. TTFP with a restored 4-pane active tab stays under budget (spec 0001).
8. `session.json` contains no scrollback, commands or environment variables.
9. `printf '\e]0;<img src=x onerror=alert(1)>\a'` shows that literal text as the tab title and runs nothing.

## Open questions

- Fallback tab title: the profile's display name for now. Foreground process name or cwd basename is a *Later* refinement.
