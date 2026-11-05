---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-020, T-006, T-011]
related: [spec 0002, spec 0003, spec 0004, spec 0005, spec 0006]
---

# Spec: Moving tabs and panes into and out of splits

Pins how tabs and panes are dragged into, out of and around split views,
and how a split opens another profile, so one tab can hold different
shells and hosts. Builds on spec 0003's split tree and tab drag.

## Problem & goal

A split always opens the source pane's profile (spec 0003, flow 1), so a
local shell can't sit beside an SSH session, and two servers can't sit
side by side. Users who already have the tabs open want to drag them
together, the way Tabby does, and pull a pane back out when they're done.

Done means: any tab or pane can be dropped on a side of a pane in the
visible tab, a pane dragged to the tab bar becomes a tab again, a split
can open any profile or host, and nothing that moves restarts or
reconnects.

## Scope

**In scope:**

- Dragging a tab onto a side of a pane: its whole layout joins that split
- Dragging a pane by its grab handle onto a side of another pane, or to the tab bar
- "Split right with profile…" and "Split down with profile…" in the palette

**Out of scope:**

- Keyboard commands that move panes (to a new tab, into another tab, swapping): *Later* (owner, T-020)
- Switching tabs by hovering over one mid-drag, and dragging between windows (multiple windows are *Later*, spec 0003)

## Design

### Affected areas

- `src/layout/` (tree, tabs, split view, tab bar), the palette's profile picker (spec 0004)

### Model

- **Move [load-bearing]:** a move takes a subtree out of one tab's tree and
  puts it into another tree or a new tab. Pane IDs and their programs stay
  as they are: nothing that moves restarts, reconnects or loses scrollback.
- **Drop target:** a side (left, right, top, bottom) of a pane in the
  visible tab, or a place in the tab bar. Over a pane, the side nearest
  the pointer is the target, and the half the dropped layout would take is
  highlighted.
- **Insert:** a drop on a side splits that pane along the side's axis at
  ratio 0.5, with the dropped layout on that side. A dropped tab brings its
  whole tree, ratios included.
- **Room:** a side is a target only if both halves keep every pane at
  spec 0003's minimum of 10 × 3 cells; otherwise nothing is highlighted
  and a drop there does nothing.

### Flows

1. **Grab:** in a tab with two or more panes, a pane shows a small 6-dot
   handle in its top right corner while the pointer is over the pane. Only
   the handle starts a pane drag; a mouse drag anywhere else in a pane
   belongs to the terminal (selection, mouse reporting; spec 0002). A tab
   with one pane has no handle (the tab itself is dragged), and neither
   does a zoomed tab.
2. **Drag:** a ghost with the pane's or tab's title follows the pointer.
   A drop on no target, or on the dragged pane or tab itself, changes
   nothing.
3. **Drop a tab on a pane:** the tab's layout is inserted and the tab
   closes. Its focused pane takes focus, and the target tab's zoom ends.
4. **Drop a pane on a pane:** the pane leaves its place, its sibling taking
   the space as on close (spec 0003, flow 2), and is inserted at the
   target. It keeps focus.
5. **Drop a pane on the tab bar:** it becomes a tab of its own where the
   tab-reorder marker shows, or at the end over empty space. That tab
   activates with the pane focused.
6. **Split with profile:** "Split right with profile…"
   (`pane.splitRightWithProfile`) and "Split down with profile…"
   (`pane.splitDownWithProfile`) open a picker of every profile plus the
   host picker's hosts (spec 0006), grouped the same way. Picking one
   splits the focused pane like a plain split (spec 0003, flow 1), but with
   that profile: a local one starts in the source pane's cwd unless it sets
   its own, an SSH one connects right away. Without room, the picker
   doesn't open. No default keys; `keybindings:` can bind them (spec 0004).
7. **Save and restore:** a move is a layout change and saves like one
   (spec 0003, flow 5). A moved pane that hadn't started yet (a restored
   tab never activated, T-006) starts when it becomes visible, as on a
   tab's first activation; an SSH one waits for Enter (spec 0006, flow 6).

### Startup impact

None. Dragging is event handling on views that already exist, and the
profile picker reads profiles and `~/.ssh/config` when it opens, as the
host picker does.

### Platform notes

GPUI's drag and drop is in-app, not the OS's (no Wayland data device, no
OLE), so it works the same on Wayland and Windows 11. A moved pane gets
one PTY resize to its new size; ConPTY redraws on resize, so its repaint
after a move is checked on Windows (T-011).

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Layout with a profile ID per pane | `session.json` (spec 0003) | Restore tabs | The user's own device; no one else processes it | Overwritten on save (spec 0003) |

Nothing new is stored, and a moved pane's output stays in its own
scrollback, never copied. No new trust boundary: the drag ghost shows
titles under spec 0003's title rules (§Security 1).

## Acceptance criteria

1. Dropping a local tab on the right side of an SSH pane leaves one tab, the local shell on the right; both programs keep their PIDs and scrollback, and the SSH session doesn't reconnect.
2. Dropping a tab of three panes on a pane brings all three in their layout, and the dragged tab is gone from the tab bar.
3. A pane dragged by its handle to the tab bar becomes a tab at the marked place, and the panes left behind fill their tab with no dead space.
4. A pane dropped on the top side of another pane in the same tab ends up above it; dropping a pane on itself, or on no target, changes nothing.
5. A side whose half can't keep every pane at 10 × 3 cells isn't highlighted, and dropping there changes nothing.
6. The handle shows only while the pointer is over a pane of a tab with two or more panes that isn't zoomed, and dragging anywhere else in a pane doesn't move it.
7. "Split right with profile…" with an SSH host opens a connected session on the right; with a local profile that sets no cwd, the new shell starts in the source pane's cwd.
8. A pane moved while its askpass dialog is open keeps the dialog, and answering it still logs in.

## Open questions

- None.
