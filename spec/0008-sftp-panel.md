---
status: approved     # draft | approved | implemented
phase: 2
tasks: [T-024, T-023, T-035, T-036, T-011, T-032, T-033, T-034, T-049]
related: [spec 0013, spec 0017, spec 0003, spec 0005, spec 0006, spec 0009, OQ-001 ✅, OQ-019 ✅, OQ-023 ✅, threat model 0001, threat model 0003]
---

# Spec: SFTP sidebar

Pins the file sidebar for SSH panes: browsing, editing, downloading and
uploading over the host's own `ssh`, with the same config, keys and prompts
as its terminal. The sidebar draws the one file browser of
[spec 0013](0013-files-panel.md), which owns every browser feature; this
spec owns the SFTP target and where the sidebar sits.

## Problem & goal

Moving a file to or from a server means leaving the terminal for `scp` or
another app. MobaXterm shows the host's files in a sidebar next to its
shell; Tern should too, without an SSH implementation of its own.

Done means: one command opens a sidebar with the files of the focused SSH
pane's host, and that pane keeps its sidebar: switching to another pane
shows that pane's own sidebar, or none. Folders browse, a
click opens a file in the user's default app and its saves go back to the
host with one click (or on their own, if the user says so), files download
and upload with progress and cancel (also by dropping them from the file
manager), and the sidebar asks for passwords the way the terminal does.

## Scope

**In scope:**

- "Toggle SFTP sidebar" (`sftp.toggle`, `Ctrl+Shift+S`): opens or closes the focused SSH pane's sidebar, on the window's right (default) or left (`sftp_sidebar`), showing that pane's host; each pane remembers whether its sidebar is open
- A listing of one folder (name, size, modified, kind) as a tree whose folders expand in place (spec 0013, the same file browser as the files pane), entering folders, going up, and a path field that is always shown and always editable
- Starting in the folder the pane's remote shell is in, if it reported one, else the home folder
- Editing a remote file in the OS's default app for its type: what a click on a file does by default
- Uploading an edit on a click in the sidebar by default; a setting (`sftp_upload`) for upload on save or on focus, and a per-edit override
- A setting (`sftp_click`) to make a click download instead
- Downloading files to a folder the user picks; uploading files the user picks or drops onto the sidebar from the file manager
- Rename, delete (after a confirmation), new folder; transfers with progress and cancel

**Out of scope:**

- *Later (planned)*: `sftp_sidebar: split`, where the file view is a pane of the split tree that splits, moves and closes like a terminal (spec 0007), and a Detach button in the sidebar's header that turns the sidebar into that pane. The model below keeps a connection apart from where it's drawn, so neither needs a rewrite (T-032); that pane is the files pane of [spec 0013](0013-files-panel.md)
- *Later (planned)*: the sidebar on a local pane, showing the browser's Local target ([spec 0013](0013-files-panel.md), T-049)
- Browser features for every target, such as the fuzzy filter (T-037), path suggestions (T-038) and permissions (T-048): [spec 0013](0013-files-panel.md)
- *Later (planned)*: a follow-terminal mode, where the listing follows each `cd` the pane's shell reports, and an opt-in per profile that sets up those reports on bash servers after login, as MobaXterm's "Follow SSH path" does, for servers that don't send OSC 7 themselves (T-034)
- Dragging files out into the file manager: never for remote files (OQ-019 ✅); Download to… covers it. *Later*: drag-out of files that are already local (T-027)
- Folder transfers (a dropped folder is refused with a notice), sync, resuming broken transfers: *Later*
- *Later (planned)*: an `editor` setting that overrides the OS's default app for edit copies, maybe per file type (T-056)
- SCP or an SSH stack of our own: never (architecture §Stack)

## Design

### Affected areas

- `src/sftp/` (client thread, session model, sidebar view), `src/layout/` (where the sidebar sits, which pane's sidebar shows), `src/ssh/` (command, askpass), config and settings

### Model

- **Sidebar [load-bearing]:** open or closed is a flag of each SSH pane,
  not of the window. The window shows a sidebar only while its *active
  pane* (the focused pane of the shown tab) has the flag set, and then it
  draws that pane's **SFTP session**; focus moving into the sidebar
  doesn't change which pane is active. Switching to a pane without the
  flag hides the sidebar; switching back shows it again. A local pane
  has none yet; T-049 gives it one over the Local target (spec 0013). The sidebar sits on the `sftp_sidebar` side, inside the
  tab list when the tabs are on the same side (`tab_bar: left | right`):
  tabs, then sidebar, then panes. Dragging its inner edge resizes it
  (200 px up to half the window); the width is one per window and lasts
  until Tern quits.
- **SFTP session [load-bearing]:** belongs to an SSH pane. It is created
  the first time that pane's sidebar opens, and ends when the pane
  closes. Closing the sidebar or switching panes keeps it, its folder, its
  transfers and its edits; reopening shows them where they were. The
  header always names its target (`user@host`).
- **Connection [load-bearing]:** `ssh <the profile's args> -s <host> sftp`,
  built by the same `ssh::command` as the pane, with the askpass
  environment of a new token (spec 0006, flow 2), so `~/.ssh/config`, the
  agent, keys and `known_hosts` apply as in the terminal. ssh's stderr is
  read for a failure message: plain text, control characters stripped,
  capped at 1 KiB.
- **Thread [load-bearing]:** each session runs ssh and the `russh-sftp`
  client on a thread of its own with a current-thread tokio runtime; the
  sidebar sends requests over a channel and draws the results. Nothing waits
  on the network on the UI thread. A request with no reply in 30 s ends the
  connection as dropped (flow 7).
- **Session state** = remote path, entries (name, kind, size, modified;
  at most 50,000 per listing, the rest left out with a notice), transfers
  (direction, name, bytes done of total), and edits (copy, remote path,
  remote size and mtime when fetched, upload mode, state:
  `clean | changed | uploading | failed`).
- **Start folder:** the remote cwd the pane's shell last reported with
  OSC 7 (T-022), read once when the session is created, else the remote
  home folder. The report is untrusted text used only as a path to list
  on the pane's own host (spec 0002, TM-escape-09); if listing it fails,
  the session lists home instead. It never follows later `cd`s (that
  is the planned follow-terminal mode, T-034).
- **Path field [load-bearing]:** always shown above the listing, always
  editable, and always showing the folder listed. Enter lists what was
  typed; a path that fails says why and keeps the last listing.
- **Edit copies [load-bearing]:** a file opened for editing is downloaded
  to `<data_local_dir>/tern/edit/<run>/<session>/<n>/<name>` (folders
  `0700`, files `0600`, never an execute bit; on Windows also marked as
  from the internet, see Platform notes), where `<name>` follows the
  download name rule (Security). A background task looks at each copy's
  size and mtime every second; a change that holds still for one look
  marks the edit `changed`, and its upload mode decides the rest. A
  session's copies are deleted when it ends. Each running Tern holds a
  lock on `edit/<run>.lock` **[load-bearing]**; after the first frame, a
  sweep deletes the copies of every run whose lock is free (a crash, a
  kill), never those of another Tern that's still open.
- **Upload mode [load-bearing]:** `manual` (default) uploads only on the
  user's click; `save` uploads each change the poll sees; `focus` uploads
  every `changed` edit when Tern's window gains focus. Each edit starts
  with `sftp_upload` and can be switched on its own; the override lives as
  long as the edit and is never written to config.
- **Not saved:** a restored window comes back with the sidebar closed and
  no sessions (spec 0003): reconnecting is a network connection the user
  starts.

### Flows

1. **Open:** `sftp.toggle` opens the active pane's sidebar on its files,
   or closes it if it shows them already (its session stays). A switch
   at the top of the sidebar shows Files or Port forwards (spec 0017,
   flow 1); the sidebar keeps the last one it showed, and showing Port
   forwards connects nothing. If the pane has no session yet, it connects and
   lists its start folder (Model). Its prompts open the pane's dialog,
   named "<profile> (SFTP)", filled from the keychain as in spec 0006,
   flow 4; they take turns with the terminal's prompts. On a local pane the
   command does nothing but show a notice that SFTP needs an SSH pane.
   Moving a pane to another tab or split (spec 0007) keeps its flag and
   session.
2. **Browse:** the listing is a tree (spec 0013, flow 3): one folder,
   folders first, then files, by name, each folder expanding in place.
   Enter or a double click enters a folder (a symlink to one
   too), Backspace goes up, and the path field goes anywhere (`~` and
   relative paths resolve against the home folder). A listing
   that fails says why in the sidebar and keeps the last one. On a file,
   Enter or a double click does what `sftp_click` says: `edit` (default) or
   `download`; the context menu always offers Edit, Download and
   Download to…, plus Rename and Delete (also F2 and Delete).
3. **Edit:** the file downloads to its edit copy with progress (cancel
   deletes it), then opens with the OS's default app for its type and is
   listed in the session's edits with its upload mode (a dropdown of the
   three, the current one checked), Upload, Open again and Close; closing an
   edit with changes not on the host asks first. Editing a file that's
   already open opens its copy again. A save marks it
   changed; it uploads when its mode says: the edit's Upload button
   (`manual`, and always available), the save itself (`save`), or the
   window gaining focus (`focus`). Before each upload the remote file's
   size and mtime are compared with the fetched ones; if they differ, the
   sidebar asks before overwriting. A failed upload says why, stays
   `changed`, and retries on the next trigger. A type the OS would run
   instead of open (Security) is not opened: it is downloaded and the
   sidebar says so. Closing an SSH pane (or its tab, or the window) whose
   session has edits asks first, naming them; edits with changes not yet
   uploaded are called out, and the dialog offers to upload them first
   ("Upload and close": it closes once the host has every change, and
   stays open, saying so, if one fails or its host's file changed). Edits
   outlive a dropped connection and upload after "Connect again".
4. **Download:** a click goes to the OS's Downloads folder, Download to…
   picks a folder (the OS's file dialog). Each file streams in 64 KiB
   requests, 16 in flight, with progress, into `<name>.tern-part` beside
   its target, renamed into place when complete **[load-bearing]**: a
   cancel or failure never touches a file the user agreed to replace. An
   existing local file is only replaced after a confirmation. Cancel, a
   full disk or a dropped connection stops and deletes the part.
5. **Upload:** the user picks local files, or drops them from the file
   manager onto the sidebar (into the folder shown, or the folder row under
   the pointer, which highlights while dragging). Each writes to
   `.<name>.tern-part` in its folder, renamed into place when complete; a
   file it replaces lends it its permissions, so a script stays
   executable. An existing remote file is only replaced after a
   confirmation naming the host; cancel stops and removes the part. A
   dropped folder fails in the transfer list, saying why.
6. **Change:** rename and new folder ask for a name; a rename onto an
   existing name asks first (spec 0013, flow 5); delete asks for a
   confirmation naming what goes and on which host; while a question is
   open, Enter confirms it and Esc cancels it instead of acting on the
   listing, and Delete asks nothing more. It removes files,
   links (as links, never followed) and empty folders; a folder with
   anything in it fails with the server's reason. The listing refreshes
   after each.
7. **Failures:** no sftp subsystem, a refused login or a dropped connection
   shows ssh's reason in the sidebar with a "Connect again" button; edits
   stay listed and upload after reconnecting. Nothing hangs. A pane whose
   ssh has exited keeps its session until the pane closes.

### Startup impact

None. Nothing about SFTP starts before the user opens the sidebar; the
sweep of a crashed run's edit copies runs after the first frame. The
sidebar is never open at boot.

### Settings

- `sftp_sidebar: left | right` (default `right`)
- `sftp_click: edit | download` (default `edit`)
- `sftp_upload: manual | save | focus` (default `manual`): when an edit
  uploads; an edit's own mode overrides it (Model)

All on the settings screen and in `src/default-config.yaml` (spec 0009).
Changing `sftp_sidebar` moves an open sidebar on reload.

### Platform notes

Windows 11's `ssh.exe` supports `-s … sftp` and askpass the same way (spec
0006). File dialogs are GPUI's: xdg-desktop-portal on Linux, the system
dialog on Windows. Opening a copy is GPUI's `open_with_system`
(`xdg-open` on Linux, `ShellExecute` on Windows); OS file drops arrive as
GPUI's `ExternalPaths` on Wayland and Windows alike. On Windows, edit
copies and downloads get a `Zone.Identifier` stream with `ZoneId=3`, so
SmartScreen and Office's Protected View treat them as from the internet;
the parts of a remote name Windows can't hold (reserved device names
like `CON`, `:`, a trailing dot or space) are escaped (Security). Windows' default
apps may lock a file while it's open; an upload reads the copy only after
a change is seen, and a read that fails retries on the next poll. New
crates, approved with this spec: `russh-sftp` 3, and `tokio` as a direct
dependency (already built for GPUI's HTTP client) with `rt`, `process`,
`io-util`, `fs`, `sync` and `time`.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Remote listings: names, sizes, dates | Memory only | Show the folder | The user's own device; no one else processes it | Until the session ends |
| Transferred files | Where the user picks, local or remote | The transfer the user started | Same | The user's |
| Edit copies | `<data_local_dir>/tern/edit/` | Open the file in the user's app | Same | Until the session ends (next launch after a crash) |

Nothing is logged (no names, paths or contents), nothing is kept on disk
but the edit copies, and nothing leaves the machine except the connection
the user starts. Threat model [0003](threat-model/0003-sftp.md) covers
this surface.

Trust boundaries:

- **Remote names are untrusted [load-bearing]:** shown as plain text by the
  title rules (spec 0003), used verbatim only to name the remote file.
  A download or edit copy is never refused for its name: one function
  escapes every character that could make it a path or disguise it, as
  `%XX` per UTF-8 byte (`../x` → `..%2Fx`, `a\b` → `a%5Cb`). Escaped are
  `/`, `\`, control characters and invisible or direction characters
  (Unicode `Cf`); a name that is `.` or `..` has its dots escaped; on
  Windows also `< > : " | ? *`, the first character of a reserved device
  name (`CON`, `aux.txt` → `%61ux.txt`) and a trailing dot or space. `%`
  itself stays: the escape only has to be safe, not reversible, and a
  name that collides with an existing file gets the overwrite
  confirmation. The escaped name is joined to the target folder, and a
  result that isn't a direct child of it is refused as a backstop. The
  listing hides `.`, `..` and empty names.
- **Credentials:** spec 0006 and threat model 0001 apply unchanged: each
  session's ssh gets its own token and its prompts go through askpass.
- **Writes are the user's:** no download overwrites, and no upload
  replaces, without a confirmation (an edit's upload, by click or by the mode
  the user chose, over a remote file that hasn't changed since it was
  fetched is the user's confirmation).
- **Opening is not running [load-bearing]:** only an edit copy the user
  clicked is opened, only with the OS's default app, and never a type the
  OS runs: Windows' `PATHEXT` plus `.lnk .url .scr .pif .msi .msp .cpl
  .hta .reg .appref-ms .application .settingcontent-ms .library-ms
  .search-ms .scf .wsh .xll .jar` on Windows; `.desktop .appimage .jar`
  on Linux, plus `.exe .com .bat .cmd .msi .scr .lnk`, which Wine runs
  where it's installed; the check is case-insensitive and on the name as
  written to disk.
  Scripts like `.sh` stay editable: an edit copy has no execute bit, so
  `xdg-open` hands them to an editor. Downloads are never opened or run by
  the app.

## Acceptance criteria

1. On a key-auth host, opening the sidebar on its SSH pane lists the folder the pane's shell reported with OSC 7 within 2 s, or the home folder if it reported none or that folder can't be listed, as the same user and port the pane uses; typing a path into the always-visible path field and pressing Enter lists it; on a password host its prompt fills from the keychain, and the password appears in no process's arguments or environment.
2. With `tab_bar: right` and the default `sftp_sidebar`, the sidebar sits between the tab list and the panes; with `sftp_sidebar: left` it sits on the left, and a reload moves an open sidebar.
3. With the sidebar open on SSH pane A, focusing SSH pane B (or a local pane) hides it; focusing A again shows it with its folder and running transfers where they were, without reconnecting; opening B's sidebar shows B's host in the header; `sftp.toggle` on a local pane opens nothing and says why.
4. Downloading a 1 GB file shows progress while the pane beside it still takes typing, the copy matches the original's checksum, and cancel deletes the partial file.
5. A remote file named `../../.bashrc`, `a/b`, with control characters or (on Windows) `CON` or `x.txt:evil` shows as plain text, and its download or edit copy lands directly inside its folder under the escaped name (`..%2F..%2F.bashrc`, `a%2Fb`, `%43ON`, `x.txt%3Aevil`).
6. Double-clicking a remote `.txt` opens it in the OS's default app; by default a save leaves the host's file unchanged and marks the edit changed, its Upload button puts the new content on the host within 3 s, and an upload after the host's copy changed asks before overwriting.
7. With `sftp_upload: save` a save reaches the host within 3 s without a click; with `focus` only after Tern's window regains focus; an edit switched to another mode follows that mode while other edits keep the setting's.
8. With `sftp_click: download`, the same double click downloads it to the Downloads folder and opens nothing; a remote `run.exe`, `x.desktop` or `a.LNK` is never opened in either mode.
9. Dropping two files from the file manager onto a folder row uploads both into that folder; one that exists there asks first, and declining leaves the remote file as it was; a dropped folder uploads nothing and says why. Rename, delete after its confirmation, and new folder work, and the listing shows the result.
10. A host without the sftp subsystem shows ssh's reason within 5 s; closing an SSH pane whose session has a changed, not-uploaded edit asks first and can upload it, then ends its ssh and deletes its edit copies; a relaunch comes back with the sidebar closed and makes no connection. On Windows 11 the same works through `ssh.exe`, Explorer drops and Notepad saves included.

## Open questions

- OQ-023: accept the residual risks of threat model 0003.
- OQ-019 (drag-out) is resolved: not for remote files; see Scope.
