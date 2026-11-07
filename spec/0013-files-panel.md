---
status: draft        # draft | approved | implemented
phase: 2
tasks: [T-042, T-033, T-041, T-043]
related: [spec 0008, spec 0006, spec 0003, spec 0002, spec 0009, OQ-026, threat model 0003]
---

# Spec: Files panel

Pins a window-level files panel that browses the local disk or any saved
SSH host, whichever the window's pane is on, and the one file browser
behind both it and the SFTP sidebar of spec 0008. The browser shows a
tree of folders. The SFTP sidebar keeps its behavior and gets the tree;
spec 0008 stays the source for SFTP sessions, edit copies, transfers and
remote names.

## Problem & goal

The SFTP sidebar only exists on an SSH pane and only shows that pane's
host. Browsing local files, or a host without opening a shell on it,
means leaving Tern for a file manager. A flat listing also means
entering and leaving folders to look one level down.

Done means: one command opens a files panel on any pane, starting on that
pane's local folder or host. Its header switches between Local and any
saved SSH host, and each keeps its folder and connection. Folders expand
in place as a tree, in the panel and in the SFTP sidebar alike, because
both are the same browser.

## Scope

**In scope:**

- "Toggle files panel" (`files.toggle`, `Ctrl+Shift+B`): opens or closes
  the window's files panel, on the `files_panel` side (default left)
- Targets: Local, or any saved SSH profile (spec 0006), picked in the
  panel's header; switching keeps the previous target's session
- One file browser for both surfaces: the files panel and the SFTP sidebar
  draw the same view over a target **[new]**
- A tree view: folders expand and collapse in place, loaded on expand
  (T-033) **[new]**
- The local target: browse, open in the OS's default app, rename, new
  folder, delete after a confirmation (T-041) **[new]**
- On an SSH target, everything spec 0008 offers: edit, download, upload,
  drops, rename, delete, new folder **[carry-over]**

**Out of scope:**

- *Later*: copying between targets: dragging from a Local row onto a
  remote folder uploads, the other way downloads
- *Later*: S3, SMB and other targets from plugins (architecture §Roadmap,
  Considered); they need the plugin system first
- Detaching either surface into a split pane: stays T-032 in spec 0008
- Folder transfers, permissions, sync: as spec 0008
- Local file operations beyond rename, delete and new folder (copy, move,
  properties): the OS's file manager does those

## Design

### Affected areas

- `src/files/` (new): the browser view, the tree, the target enum, the
  local target, the files panel; `src/sftp/` keeps the SFTP session, client
  thread, edit copies and transfers and loses its view to `files/`;
  `src/layout/` (where the panel sits); config, settings, keymap
- `spec/architecture.md` §Code layout gains `files/` in the same change
  that builds T-042

### Model

- **Browser [load-bearing]:** one view, drawn over one target session:
  the header, the always-editable path field and the tree of spec 0008's
  columns (name, size, modified, kind), the context menu, transfers and
  edits. The SFTP sidebar and the files panel are two places that draw it;
  neither has listing or tree code of its own.
- **Target [load-bearing]:** an enum, `Local | Sftp(profile)`, not a
  trait: two targets don't earn an abstraction, and the plugin system
  decides how a plugin adds one. Operations the target doesn't support
  (Local: download, upload, drops, edit copies) aren't offered, not
  greyed out.
- **Session:** per target, the root folder, the tree's expanded folders,
  and for SFTP everything spec 0008's session holds. An SFTP session of
  the panel is the panel's own: it is not the SSH pane's sidebar session,
  and it connects with its own ssh (spec 0008 §Connection). Local
  listings run on a background thread like SFTP requests; nothing reads
  the disk on the UI thread.
- **Files panel [load-bearing]:** one per window, open or closed. It
  holds one session per target it has shown, and the header's dropdown
  lists Local, then the saved SSH profiles by group as the host manager
  does; a target with a session shows a Disconnect item that ends it.
  Sessions end when their window closes. The first time a window opens
  its panel, it picks the active pane's target: Local at a local pane's
  cwd (spec 0002), else home; an SSH pane's profile at the remote cwd the
  pane reported (spec 0008 §Start folder), else home. Later toggles show
  what it showed last.
- **Placement:** the panel sits on the `files_panel` side and has its own
  width, resized like the SFTP sidebar's (200 px up to half the window,
  for as long as Tern runs). On a side shared with the tab list and the
  SFTP sidebar, the order from the window's edge is tabs, files panel,
  SFTP sidebar, panes: the SFTP sidebar stays next to the panes it
  belongs to.
- **Tree [load-bearing]:** rooted at the path field's folder. A folder row
  (a symlink to one too) has a chevron; expanding it lists that folder
  once, shown as a loading row and then its entries indented under it.
  A failed expand says why on that row. Each folder's entries are sorted
  and capped as spec 0008's listing (folders first, by name; 50,000 then a
  notice). Only visible rows are laid out. Changing the root collapses
  everything; Refresh (F5) re-lists the root and every expanded folder; a
  rename, delete, new folder or upload re-lists the folder it changed.
- **Not saved:** a restored window comes back with the panel closed and
  no sessions, like the SFTP sidebar (spec 0008).

### Flows

1. **Open:** `files.toggle` opens or closes the window's panel; it works
   on any pane, and with none focused it opens on Local at home. An
   SFTP target's prompts open a dialog over the panel, named
   "<profile> (Files)", filled from the keychain as in spec 0006, flow 4.
2. **Switch target:** picking a target in the header shows its session,
   or creates one at its home folder. The previous session keeps its
   folder, tree, transfers and edits. Disconnect asks first if the session
   has edits with changes not on the host, as closing an SSH pane does in
   spec 0008, flow 3.
3. **Browse:** Up/Down move the selection; Right or the chevron expands a
   folder, Left collapses it or moves to its parent; Enter or a double
   click on a folder makes it the root; Backspace makes the root's parent
   the root; the path field goes anywhere. On Windows, Local's path field
   takes any drive (`D:\x`), and going up stops at a drive's root.
4. **Open a local file:** Enter or a double click opens the file itself
   with the OS's default app: no copy, nothing to upload. A type the OS
   would run instead of open (spec 0008, Security) isn't opened, and the
   panel says why.
5. **Change a local file:** rename and new folder ask for a name, which
   must be one path component; delete asks for a confirmation naming
   what goes. Whether delete goes to the OS's trash is OQ-026; until it's
   answered, delete is permanent, after that confirmation. Delete removes
   files, links (never followed) and empty folders; a folder with anything
   in it isn't deleted, as on SFTP.
6. **SFTP target:** spec 0008's flows 2–7, with the tree's rules for
   browsing. Closing a window whose panel has edits asks as closing an
   SSH pane does.

### Startup impact

None. Nothing about the panel starts before the user opens it, and it's
never open at boot.

### Settings

- `files_panel: left | right` (default `left`); `files.toggle` is
  bindable like every command (spec 0004)

On the settings screen and in `src/default-config.yaml` (spec 0009).
Changing `files_panel` moves an open panel on reload. `sftp_click` and
`sftp_upload` apply to SFTP targets in the panel too.

### Platform notes

Local listings use `std::fs` on both platforms; Windows paths show with
`\` and drive letters. Opening is GPUI's `open_with_system` as in spec
0008, and the same per-platform list of types that run decides what
isn't opened. SFTP targets need nothing beyond spec 0008. No new crates
unless OQ-026 picks the trash.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Local listings: names, sizes, dates | Memory only | Show the folder | The user's own device; no one else processes it | Until the session ends |
| Remote listings, transfers, edit copies | As spec 0008 | As spec 0008 | Same | As spec 0008 |

Nothing is logged, and nothing new is stored on disk. The only network
traffic is the SSH connection to the target the user picked; choosing a
target is starting that connection. On a first open from an SSH pane,
the user's command is what starts it.

Trust boundaries:

- **Names are plain text:** local names are shown by the title rules of
  spec 0003, like remote ones; a downloaded file can carry a name a server
  chose.
- **Opening is not running [load-bearing]:** spec 0008's rule holds for
  local files too. A file a click opens is never a type the OS would run,
  even though it's on the user's own disk, so a click in Tern never starts
  a program.
- **Writes are the user's:** delete always asks; rename never replaces an
  existing name without a confirmation.
- Threat model 0003 covers the SFTP target; the local target crosses
  only TB4 (opening) and TB5 (confirmations), under the rules above.

## Acceptance criteria

1. `Ctrl+Shift+B` on a local pane opens the panel on Local at that pane's folder; on an SSH pane, a window's first open shows that pane's host after its own login, while the pane's SFTP sidebar keeps its own session.
2. Switching the header from host A to Local and back shows A's folder, expanded folders and running transfers as they were, without a new login; Disconnect ends A's ssh and asks first if A has a changed, not-uploaded edit.
3. Expanding a folder shows its children indented below it without changing the path field; collapsing hides them; Enter on a folder makes it the root and collapses the rest; a folder that can't be listed shows why on its row.
4. Expanding a folder of 50,000 entries keeps the pane beside it taking typing, and scrolling through it stays smooth.
5. An SSH pane's SFTP sidebar shows the same tree, and spec 0008's acceptance criteria still pass.
6. On Local, double-clicking a `.txt` opens that file in the OS's default app and makes no copy under the edit folder; `run.exe`, `x.desktop` or `a.LNK` isn't opened, and the panel says why.
7. On Local, new folder and rename work; a name with `/` (or `\` on Windows) is refused; renaming onto an existing name and every delete ask first, and declining changes nothing.
8. With `files_panel: right`, `tab_bar: right` and the default `sftp_sidebar`, the order from the right edge is tabs, files panel, SFTP sidebar, panes; a reload moves an open panel.
9. A relaunch comes back with the panel closed and makes no connection, and the TTFP is unchanged.
10. On Windows 11, Local lists `C:\`, the path field opens `D:\`, and an SFTP target works through `ssh.exe`.

## Open questions

- OQ-026: should a local delete go to the OS's trash?
