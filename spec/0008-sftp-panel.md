---
status: draft        # draft | approved | implemented
phase: 2
tasks: [T-024, T-023, T-011]
related: [spec 0003, spec 0006, spec 0007, spec 0009, OQ-001 ✅, OQ-019 ✅, threat model 0001]
---

# Spec: SFTP panel

Pins the file panel for an SSH host: browsing, editing, downloading and
uploading over the host's own `ssh`, with the same config, keys and prompts
as its terminal.

## Problem & goal

Moving a file to or from a server means leaving the terminal for `scp` or
another app. Tabby and MobaXterm show the host's files next to its shell;
Tern should too, without an SSH implementation of its own.

Done means: from an SSH pane, one command opens the host's files beside
it; folders browse, a click opens a file in the user's default app and
its saves go back to the host with one click (or on their own, if the user
says so), files download and upload with progress and
cancel (also by dropping them from the file manager), and the panel asks
for passwords the way the terminal does.

## Scope

**In scope:**

- "Open SFTP panel" (`sftp.open`, `Ctrl+Shift+S`) on an SSH pane: a panel pane split off to its right
- Listing (name, size, modified, kind), entering folders, going up, a path field
- Editing a remote file in the OS's default app for its type: what a click on a file does by default
- Uploading an edit on a click in the panel by default; a setting (`sftp_upload`) for upload on save or on focus, and a per-edit override
- A setting (`sftp_click`) to make a click download instead
- Downloading files to a folder the user picks; uploading files the user picks or drops onto the panel from the file manager
- Rename, delete (after a confirmation), new folder; transfers with progress and cancel

**Out of scope:**

- Dragging files out of the panel into the file manager: never for remote files (OQ-019 ✅: the file must exist locally in full when the drag leaves the window, on Wayland and Windows alike, and it almost never does). Download to… covers it. *Later*: drag-out of files that are already local, once GPUI has a drag source on Windows (T-027)
- Folder transfers (a dropped folder is refused with a notice), permissions, a choice of editor per file type: *Later*
- A local file browser beside it, sync, resuming broken transfers: *Later*
- SCP or an SSH stack of our own: never (architecture §Stack)

## Design

### Affected areas

- `src/sftp/` (client thread, model), the split tree's pane kinds (spec 0003), `src/ssh/` (command, askpass)

### Model

- **Connection [load-bearing]:** `ssh <the profile's args> -s <host> sftp`,
  with the askpass environment of a new token (spec 0006, flow 2), so
  `~/.ssh/config`, the agent, keys and `known_hosts` apply as in the
  terminal. ssh's stderr is read for a failure message, never shown raw.
- **Thread [load-bearing]:** each panel runs ssh and the `russh-sftp`
  client on a thread of its own with a current-thread tokio runtime; the
  panel sends requests over a channel and draws the results. Nothing waits
  on the network on the UI thread.
- **Panel** = remote path, entries (name, kind, size, modified),
  transfers (direction, name, bytes done of total), and edits (copy,
  remote path, upload mode, state: `clean | changed | uploading | failed`). It's a pane kind of
  the split tree, so it splits, moves and closes like a terminal pane
  (spec 0007); closing it ends its ssh.
- **Edit copies [load-bearing]:** a file opened for editing is downloaded
  to `<data_local_dir>/tern/edit/<panel>/<n>/<name>` (folders `0700`,
  never an execute bit), where `<name>` follows the download name rule
  below. The panel's thread polls each copy's size and mtime every second;
  a change marks the edit `changed`, and its upload mode decides the rest. A panel's copies are deleted when it closes, and
  leftovers of a crashed run after the first frame of the next.
- **Upload mode [load-bearing]:** `manual` (default) uploads only on the
  user's click; `save` uploads each change the poll sees; `focus` uploads
  every `changed` edit when Tern's window gains focus. Each edit starts
  with `sftp_upload` and can be switched on its own in the panel; the
  override lives as long as the edit and is never written to config.
- **Not saved:** a restored tab comes back without its SFTP panels (spec
  0003): reopening one is a network connection the user starts.

### Flows

1. **Open:** on a focused SSH pane, `sftp.open` splits it with a panel
   that connects and lists the remote home folder. Prompts open the
   panel's dialog, filled from the keychain as in spec 0006, flow 4.
2. **Browse:** Enter or a double click enters a folder, Backspace goes up,
   and the path field goes anywhere. A listing that fails says why in the
   panel and keeps the last one. On a file, Enter or a double click does
   what `sftp_click` says: `edit` (default) or `download`; the context
   menu always offers Edit, Download and Download to….
3. **Edit:** the file downloads to its edit copy with progress (cancel
   deletes it), then opens with the OS's default app for its type and is
   listed in the panel's edits with its upload mode. A save marks it
   changed; it uploads when its mode says: the edit's Upload button
   (`manual`, and always available), the save itself (`save`), or the
   window gaining focus (`focus`). If the remote file changed since it was
   fetched, the panel asks before overwriting; a failed upload says why,
   stays `changed`, and retries on the next trigger. A type the OS would run instead of open (Windows
   `PATHEXT`, `.lnk`, `.url`, `.desktop`, `.appimage`, `.jar`, …) is not
   opened: the panel downloads it and says so. Closing a panel with edits
   open asks first, naming them; edits with changes not yet uploaded are
   called out, and the dialog offers to upload them before closing.
4. **Download:** a click goes to the OS's Downloads folder, Download to…
   picks a folder (the OS's file dialog); each file streams in 256 KiB
   chunks with progress. An existing local file is only replaced after a
   confirmation. Cancel stops and deletes the partial file.
5. **Upload:** the user picks local files, or drops them from the file
   manager onto the panel (into the folder shown, or the folder row under
   the pointer, which highlights while dragging); an existing remote file
   is only replaced after a confirmation; cancel stops and removes the
   partial remote file.
6. **Change:** rename and new folder ask for a name; delete asks for a
   confirmation naming what goes. The listing refreshes after each.
7. **Failures:** no sftp subsystem, a refused login or a dropped connection
   shows ssh's reason in the panel with a "Connect again" button; nothing
   hangs.

### Startup impact

None. Nothing about SFTP starts before the user opens a panel; the sweep
of a crashed run's edit copies runs after the first frame.

### Settings

- `sftp_click: edit | download` (default `edit`)
- `sftp_upload: manual | save | focus` (default `manual`): when an edit
  uploads; an edit's own mode overrides it (Model)

Both on the settings screen and in `src/default-config.yaml` (spec 0009).

### Platform notes

Windows 11's `ssh.exe` supports `-s … sftp` and askpass the same way (spec
0006). File dialogs are GPUI's: xdg-desktop-portal on Linux, the system
dialog on Windows. Opening a copy is GPUI's `open_with_system`
(`xdg-open` on Linux, `ShellExecute` on Windows); OS file drops arrive as
GPUI's `ExternalPaths` on Wayland and Windows alike. Windows' default apps
may lock a file while it's open; an upload reads the copy only after a
change is seen, and a read that fails retries on the next poll. New
crates, approved with this spec: `russh-sftp` 3, and `tokio` as a direct
dependency (already built for GPUI's HTTP client) with `rt`, `process`,
`io-util`, `fs` and `sync`.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Remote listings: names, sizes, dates | Memory only | Show the folder | The user's own device; no one else processes it | Until the panel closes |
| Transferred files | Where the user picks, local or remote | The transfer the user started | Same | The user's |
| Edit copies | `<data_local_dir>/tern/edit/` | Open the file in the user's app | Same | Until the panel closes (next launch after a crash) |

Nothing is logged, nothing is kept on disk but the edit copies, and
nothing leaves the machine except the connection the user starts.

Trust boundaries:

- **Remote names are untrusted [load-bearing]:** shown as plain text by the
  title rules (spec 0003), used verbatim only to name the remote file. A
  download is written into the chosen folder only, under its last path
  component, refused if that's empty, `.`, `..`, or holds `/`, `\`, a NUL
  or a drive prefix (path traversal).
- **Credentials:** spec 0006 and threat model 0001 apply unchanged: the
  panel's ssh gets its own token and its prompts go through askpass.
- **Writes are the user's:** no download overwrites, and no upload
  replaces, without a confirmation (an edit's upload, by click or by the mode
  the user chose, over a remote file that hasn't changed since it was
  fetched is the user's confirmation).
- **Opening is not running [load-bearing]:** only an edit copy the user
  clicked is opened, only with the OS's default app, and never a type the
  OS runs (flow 3). Downloads are never opened or run by the app.
- A STRIDE model of this surface (T-024) comes before T-023 builds it.

## Acceptance criteria

1. On a key-auth host, "Open SFTP panel" lists the remote home folder within 2 s, as the same user and port the terminal pane uses; on a password host its prompt fills from the keychain, and the password appears in no process's arguments or environment.
2. Downloading a 1 GB file shows progress while the terminal pane beside it still takes typing, the copy matches the original's checksum, and cancel deletes the partial file.
3. A remote file named `../../.bashrc`, `a/b` or with control characters shows as plain text, and its download or edit copy lands only inside its folder or is refused.
4. Double-clicking a remote `.txt` opens it in the OS's default app; by default a save leaves the host's file unchanged and marks the edit changed, its Upload button puts the new content on the host within 3 s, and an upload after the host's copy changed asks before overwriting.
5. With `sftp_upload: save` a save reaches the host within 3 s without a click; with `focus` only after Tern's window regains focus; an edit switched to another mode in the panel follows that mode while other edits keep the setting's.
6. With `sftp_click: download`, the same double click downloads it to the Downloads folder and opens nothing; a remote `run.exe` or `x.desktop` is never opened in either mode.
7. Dropping two files from the file manager onto a folder row uploads both into that folder; one that exists there asks first, and declining leaves the remote file as it was; a dropped folder uploads nothing and says why.
8. Rename, delete after its confirmation, and new folder work, and the listing shows the result.
9. A host without the sftp subsystem shows ssh's reason in the panel within 5 s; closing a panel with a changed, not-uploaded edit asks first and can upload it, then ends its ssh and deletes its edit copies; a relaunch restores the tab without the panel and makes no connection (spec 0003, 0006).
10. On Windows 11 the same works through `ssh.exe`, Explorer drops and Notepad saves included.

## Open questions

None open. OQ-019 (drag-out) is resolved: not for remote files; see Scope.
