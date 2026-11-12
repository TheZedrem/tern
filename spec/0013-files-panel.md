---
status: approved     # draft | approved | implemented
phase: 2
tasks: [T-042, T-033, T-041, T-043, T-044, T-045, T-037, T-038, T-048, T-049]
related: [spec 0008, spec 0007, spec 0006, spec 0003, spec 0002, spec 0009, T-022, OQ-026 ✅, OQ-027, OQ-028, threat model 0003]
---

# Spec: File browser

Pins the one file browser, which browses the local disk or any saved SSH
host as a tree of folders, and the files pane, which draws it as a pane
of the split tree with everything a terminal pane can do. The sidebar of
spec 0008 draws the same browser. This spec owns every browser feature,
for every target and both surfaces (architecture §Key decisions, "One
file browser, many targets"); spec 0008 stays the source for the SFTP
target (sessions, connection, edit copies, transfers, remote names) and
for the sidebar's placement.

## Problem & goal

The SFTP sidebar only exists on an SSH pane and only shows that pane's
host. Browsing local files, or a host without opening a shell on it,
means leaving Tern for a file manager. A flat listing also means
entering and leaving folders to look one level down.

Done means: one command splits the focused pane with a files pane that
starts on that pane's local folder or host. Its header switches between
Local and any saved SSH host, and each keeps its folder and connection.
The pane splits, moves, zooms, becomes a tab and restores like a
terminal pane. Folders expand in place as a tree, in the pane and in the
SFTP sidebar alike, because both are the same browser.

## Scope

**In scope:**

- "Open files" (`files.open`, `Ctrl+Shift+B`): splits the focused pane
  with a files pane, or focuses the tab's files pane if it has one;
  "Open files in a new tab" (`files.newTab`, no default key)
- Targets: Local, or any saved SSH profile (spec 0006), picked in the
  pane's header; switching keeps the previous target's session
- A pane of the split tree: split, close, resize, focus, zoom, drag into
  and out of splits and to the tab bar, and session restore, all as
  specs 0003 and 0007 say for a terminal pane **[new]**
- One file browser for both surfaces: the files pane and the SFTP sidebar
  draw the same view over a target **[new]**
- A tree view: folders expand and collapse in place, loaded on expand
  (T-033) **[new]**
- The local target: browse, open in the OS's default app, run or edit an
  executable (`files_executable`), rename, new folder, delete after a
  confirmation (T-041) **[new]**
- On an SSH target, everything spec 0008 offers: edit, download, upload,
  drops, rename, delete, new folder **[carry-over]**

**Out of scope:**

- *Later*: copying between targets: dragging from a Local row onto a
  remote folder uploads, the other way downloads
- *Later (planned)*: a local delete moves to the OS's trash (the freedesktop trash, the Recycle Bin) instead of deleting for good, so a slip can be undone; needs the `trash` crate (T-044)
- *Later (considered)*: a watcher that refreshes a Local folder when the
  disk changes; until then, F5
- *Later*: S3, SMB and other targets from plugins (architecture §Roadmap,
  Considered); they need the plugin system first
- Detaching the SFTP sidebar into a files pane with its session: T-032
  in spec 0008, which now targets this pane
- Folder transfers, sync: as spec 0008
- *Later (planned)*: a fuzzy filter over the loaded tree, best matches first with the matched characters highlighted; no recursive search on disk or on the server (T-037)
- *Later (planned)*: an `editor` setting that overrides the OS's default app for edit copies and Edit-chosen files (T-056)
- *Later (planned)*: suggestions in the path field: a dropdown of matching folders as the user types, fuzzy-matched like the filter, with Tab to complete (T-038)
- *Later (planned)*: permissions: view and change a file's mode, owner and group, on every target that supports them; a Windows disk has ACLs instead, and what it offers is decided when this is specified (T-048)
- The sidebar's Files panel on a local pane, showing the Local target from the folder the pane's shell is in: pinned in [spec 0018](0018-app-sidebar.md), Files panel (T-049)
- Local file operations beyond rename, delete and new folder (copy, move,
  properties): the OS's file manager does those

## Design

### Affected areas

- `src/files/` (new): the browser view, the tree, the target enum, the
  local target, the files pane; `src/sftp/` keeps the SFTP session, client
  thread, edit copies and transfers and loses its view to `files/`;
  `src/layout/` (a second kind of leaf), `src/session.rs` (saving it),
  config, settings, keymap
- `spec/architecture.md` §Code layout gains `files/` in the same change
  that builds T-042; spec 0003's leaf model and the session row in
  architecture §Data gain the files leaf with T-043

### Model

- **Browser [load-bearing]:** one view, drawn over one target session:
  the header, the always-editable path field and the tree of spec 0008's
  columns (name, size, modified, kind), the context menu, transfers and
  edits. The SFTP sidebar and the files pane are two places that draw it;
  neither has listing or tree code of its own.
- **Target [load-bearing]:** an enum, `Local | Sftp(profile)`, not a
  trait: two targets don't earn an abstraction, and the plugin system
  decides how a plugin adds one. Every browser feature is written once
  over the targets' operations, never per target or per surface, so a
  plugin target gets it too. Operations the target doesn't support
  (Local: download, upload, drops, edit copies) aren't offered, not
  greyed out.
- **Session:** per target, the root folder, the tree's expanded folders,
  and for SFTP everything spec 0008's session holds. An SFTP session of
  a files pane is the pane's own: it is not an SSH pane's sidebar
  session, and it connects with its own ssh (spec 0008 §Connection).
  Local listings run on a background thread like SFTP requests; nothing
  reads the disk on the UI thread, and nothing watches it: a Local folder
  changes on F5 and on the browser's own writes only.
- **Files pane [load-bearing]:** a leaf of a tab's split tree (spec 0003)
  that draws the browser instead of a terminal. It splits, closes,
  resizes, focuses, zooms, moves by its grab handle and becomes a tab
  exactly as a terminal pane does (specs 0003, 0007), under the same
  minimum size; a tab holds any mix of the two. The pane holds one
  session per target it has shown, and the header's dropdown lists
  Local, then the saved SSH profiles by group as the host manager does;
  an SSH target with a session shows a Disconnect item that ends it
  (Local has none). Sessions end when the pane closes. A target whose
  profile left the config on a reload keeps its session, listed under
  its old name, until Disconnect. On open, the pane picks the focused
  pane's target: Local at a local pane's cwd (spec 0002; the live cwd
  needs T-022, else `/proc` on Linux and home on Windows), else home; an
  SSH pane's profile at the remote cwd the pane reported (spec 0008 §Start
  folder), else home.
- **Tree [load-bearing]:** rooted at the path field's folder. A folder row
  (a symlink to one too) has a chevron; expanding it lists that folder
  once, shown as a loading row and then its entries indented under it.
  A failed expand says why on that row. Each folder's entries are sorted
  and capped as spec 0008's listing (folders first, by name; 50,000 then a
  notice). Only visible rows are laid out. Changing the root collapses
  everything; Refresh (F5) re-lists the root and every expanded folder; a
  rename, delete, new folder or upload re-lists the folder it changed.
- **Saved [load-bearing]:** a files leaf saves its shown target (Local or
  the profile ID) and root folder, nothing else: no expanded folders,
  transfers, edits or other targets' sessions. It restores as spec 0003
  flow 6 restores a leaf: Local lists its folder (home if it's gone) on
  its thread once the leaf is shown; an SSH target comes back
  disconnected with a Connect button, like an SSH leaf, so a restore
  makes no connection. A profile that no longer exists restores as Local
  at home with a notice.

### Flows

1. **Open:** `files.open` splits the focused pane to the right at ratio
   0.5 with a files pane on that pane's target (Model) and focuses it;
   if the tab already has a files pane, it focuses that one instead.
   Without room (spec 0003's minimum), a notice says so. `files.newTab`
   opens a tab holding one files pane on Local at home; with no pane
   focused, `files.open` does the same. An SFTP target's prompts open a
   dialog over the pane, named "<profile> (Files)", filled from the
   keychain as in spec 0006, flow 4.
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
   with the OS's default app: no copy, nothing to upload. A **runnable**
   file (Security) does what `files_executable` says: `ask` (default)
   offers Run, Edit and Cancel; `run` and `edit` skip the question. Run
   splits the files pane to the right with a local pane whose command is
   the file, in the file's folder, so its output and exit show as in
   spec 0002 (without room, a notice; the file isn't run). That pane
   stays open after the program exits, whatever `close_on_exit` says, or
   a program that exits at once would leave nothing to see. The question's
   Enter means Edit: running takes its button, never a key pressed in
   passing. Edit opens
   the file with the app the OS opens plain text with, never the app for
   the file's own type. An **edit-only** file (Security) always opens
   that way, with no question.
5. **Change a local file:** rename and new folder ask for a name, which
   must be one path component and not `.`, `..` or empty; anything else
   the OS refuses shows the OS's reason. Renaming onto an existing name,
   on Local and on SFTP alike, asks first: the browser looks for the
   target before renaming, and the gap between that look and the rename
   is accepted. Delete asks for a confirmation naming what goes, the same
   confirmation as on SFTP, saying it can't be undone. Delete is
   permanent (OQ-026 ✅; the trash is T-044). It removes files, links
   (never followed) and empty folders; a folder with anything in it isn't
   deleted, as on SFTP.
6. **Move and close:** dragging the pane by its handle, dropping a tab or
   pane on its side, zooming and closing follow specs 0003 and 0007;
   nothing that moves reconnects or re-lists. Closing a files pane (or
   its tab, or the window) whose sessions have edits asks as closing an
   SSH pane does, naming them, and ends every session's ssh.
7. **SFTP target:** spec 0008's flows 2–7, with the tree's rules for
   browsing.

### Keys

Everything the browser does has a key; the mouse is a convenience
(owner's rule, 2026-11-07). The keys are the browser's own, bound in its
key context, so no terminal is under them:

| Key | Does |
|---|---|
| Up, Down | Move the selection |
| Right, Left | Expand (then step in), collapse (then go to the folder above) |
| Enter | Open (flows 3, 4); connect while not connected |
| Backspace | The root's parent becomes the root |
| F5 | Refresh |
| F2, Delete | Rename, delete (after its question) |
| Ctrl+N | New folder |
| Ctrl+E, Ctrl+D, Ctrl+Alt+D | SFTP: edit, download, download to… |
| Ctrl+U | SFTP: upload files… |
| Ctrl+L, Esc | To the path field, and back to the tree |
| Ctrl+Enter | The selected entry's context menu, as a right click opens it |
| Tab, Shift+Tab | Through the tree, the header, the path field, a question, edits and transfers; Enter or Space presses a button |
| `Ctrl+Shift+O` (`files.switchTarget`) | The target switcher in the palette: Local, a host, Disconnect |

A question takes the keyboard when it opens, on its safe answer (Edit
for a Run question, else the one Enter has always meant); Left and Right
move between its buttons, Esc cancels it. The context menu lists each action with its key.

### Startup impact

A restored active tab with a files leaf draws the pane's frame empty on
the first frame, as a terminal leaf draws before its first bytes; the
Local listing starts on its thread after the first frame, and an SSH
target starts nothing. Nothing else runs before the user opens a pane.
T-043 reports the TTFP with a restored files pane in the active tab.

### Settings

- `files_executable: ask | run | edit` (default `ask`): what Enter or a
  double click does on a runnable local file (flow 4)
- `files.open`, `files.newTab` and `files.switchTarget` are bindable like
  every command (spec 0004)

On the settings screen and in `src/default-config.yaml` (spec 0009).
`sftp_click` and `sftp_upload` apply to SFTP targets in the pane too.

### Platform notes

Local listings use `std::fs` on both platforms; Windows paths show with
`\` and drive letters. Opening is GPUI's `open_with_system` as in spec
0008. Edit opens the plain-text handler: Linux the `text/plain` handler
`xdg-mime` names, started as its desktop entry's `Exec` line says;
Windows the `.txt` association, Notepad until OQ-027 is answered. Local
names that aren't UTF-8 are left out of a listing until OQ-028 is
answered. Run on Windows
goes through `cmd /c "<file>"`, so every `PATHEXT` type runs; on Linux
the file is the command. Deleting a symlink to a folder on Windows is a
`remove_dir` of the link (a directory symlink or junction is a directory
reparse point, and `remove_file` refuses it); on Linux it is a
`remove_file`; neither touches the target. Rename on Linux replaces an
existing target silently and on Windows fails, which is why the browser
looks first (flow 5). SFTP targets need nothing beyond spec 0008. No new
crates.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Local listings: names, sizes, dates | Memory only | Show the folder | The user's own device; no one else processes it | Until the session ends |
| A files leaf's target and root folder | `session.json` (spec 0003) | Restore the pane | Same | Overwritten on save (spec 0003) |
| Remote listings, transfers, edit copies | As spec 0008 | As spec 0008 | Same | As spec 0008 |

Nothing is logged. The only network traffic is the SSH connection to the
target the user picked; choosing a target, or Connect after a restore,
is starting that connection. On an open from an SSH pane, the user's
command is what starts it.

Trust boundaries:

- **Names are plain text:** local names are shown by the title rules of
  spec 0003, like remote ones; a downloaded file can carry a name a server
  chose.
- **A click never runs a program [load-bearing]:** spec 0008's rule
  holds for remote files unchanged: an edit copy is never a type the OS
  runs. On Local the file keeps its execute bit, so the browser sorts
  local files first. *Runnable*: on Linux a file with any execute bit;
  on Windows an extension in `PATHEXT`. *Edit-only*: the rest of spec
  0008's lists of types the OS would run (`.desktop`, `.appimage`,
  `.jar` and the Wine types on Linux; `.lnk`, `.url`, `.hta`, `.msi` and
  the rest on Windows): their default app runs them, so they only ever
  open as plain text. Everything else opens with its default app. Run is
  the user's named choice, on Local only, and with `files_executable:
  run` it is the user's standing choice; what runs is a program on the
  user's own disk, in a pane whose output they see.
- **The header names the target [load-bearing]:** a files pane can sit
  beside any pane, so its header always shows Local or `user@host`, and
  every delete and overwrite confirmation names the target, as threat
  model 0003's TM-sftp-01 asks of the sidebar.
- **Writes are the user's:** delete always asks; rename never replaces an
  existing name without a confirmation, on either target.
- Threat model 0003 covers the SFTP target; the local target crosses
  only TB4 (opening and running) and TB5 (confirmations), under the
  rules above.

## Acceptance criteria

1. `Ctrl+Shift+B` on a local pane splits it with a files pane on Local at that pane's folder; on an SSH pane, on that pane's host after its own login, while the pane's SFTP sidebar keeps its own session; `Ctrl+Shift+B` again in that tab focuses the files pane instead of opening another.
2. Switching the header from host A to Local and back shows A's folder, expanded folders and running transfers as they were, without a new login; Disconnect ends A's ssh and asks first if A has a changed, not-uploaded edit; Local offers no Disconnect.
3. Expanding a folder shows its children indented below it without changing the path field; collapsing hides them; Enter on a folder makes it the root and collapses the rest; a folder that can't be listed shows why on its row.
4. Expanding a folder of 50,000 entries keeps the pane beside it taking typing, and scrolling through it stays smooth.
5. An SSH pane's SFTP sidebar shows the same tree, and spec 0008's acceptance criteria still pass.
6. On Local, double-clicking a `.txt` opens that file in the OS's default app and makes no copy under the edit folder; a `run.sh` with an execute bit asks Run or Edit by default, Run shows its output in a new pane beside the files pane, and Edit opens it in the plain-text app; `x.desktop` without an execute bit opens as plain text and launches nothing, in every `files_executable` mode.
7. On Local, new folder and rename work; a name with `/` (or `\` on Windows), `.` or `..` is refused; renaming onto an existing name, on Local and on an SFTP target, and every delete ask first, and declining changes nothing.
8. A files pane dragged by its handle to the tab bar becomes a tab of its own, and an SSH tab dropped on its side splits beside it; neither reconnects or re-lists, and the files pane keeps its minimum size when the divider is dragged.
9. A relaunch with a files pane on Local in the active tab and host A as another target brings the pane back in its place with Local listed at its folder, A gone from the pane and only reachable through the header, no connection made, and the TTFP unchanged (spec 0001); a tab whose files pane showed A comes back disconnected with a Connect button.
10. On Windows 11, Local lists `C:\`, the path field opens `D:\`, `run.bat` runs through `cmd /c` in a new pane, deleting a symlink to a folder removes the link and not its target, and an SFTP target works through `ssh.exe`.

## Open questions

- OQ-026 is resolved: a local delete is permanent, after the same
  confirmation as on SFTP; the trash is planned (T-044).
- OQ-027: how Edit finds the `.txt` app on Windows (a new crate);
  Notepad until then.
- OQ-028: local names that aren't UTF-8; left out until then.
