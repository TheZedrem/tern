---
status: draft        # draft | approved | implemented
phase: 2
tasks: [T-024, T-023, T-011]
related: [spec 0003, spec 0006, spec 0007, OQ-001 ✅, threat model 0001]
---

# Spec: SFTP panel

Pins the file panel for an SSH host: browsing, downloading and uploading
over the host's own `ssh`, with the same config, keys and prompts as its
terminal.

## Problem & goal

Moving a file to or from a server means leaving the terminal for `scp` or
another app. Tabby and MobaXterm show the host's files next to its shell;
zed-term should too, without an SSH implementation of its own.

Done means: from an SSH pane, one command opens the host's files beside
it; folders browse, files download and upload with progress and cancel,
and the panel asks for passwords the way the terminal does.

## Scope

**In scope:**

- "Open SFTP panel" (`sftp.open`, `Ctrl+Shift+S`) on an SSH pane: a panel pane split off to its right
- Listing (name, size, modified, kind), entering folders, going up, a path field
- Downloading files to a folder the user picks; uploading files the user picks
- Rename, delete (after a confirmation), new folder; transfers with progress and cancel

**Out of scope:**

- Folder transfers, drag and drop from the OS, editing remote files in place, permissions: *Later*
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
- **Panel** = remote path, entries (name, kind, size, modified), and
  transfers (direction, name, bytes done of total). It's a pane kind of
  the split tree, so it splits, moves and closes like a terminal pane
  (spec 0007); closing it ends its ssh.
- **Not saved:** a restored tab comes back without its SFTP panels (spec
  0003): reopening one is a network connection the user starts.

### Flows

1. **Open:** on a focused SSH pane, `sftp.open` splits it with a panel
   that connects and lists the remote home folder. Prompts open the
   panel's dialog, filled from the keychain as in spec 0006, flow 4.
2. **Browse:** Enter or a double click enters a folder, Backspace goes up,
   and the path field goes anywhere. A listing that fails says why in the
   panel and keeps the last one.
3. **Download:** the user picks a local folder (the OS's file dialog); each
   file streams in 256 KiB chunks with progress. An existing local file is
   only replaced after a confirmation. Cancel stops and deletes the
   partial file.
4. **Upload:** the user picks local files; an existing remote file is only
   replaced after a confirmation; cancel stops and removes the partial
   remote file.
5. **Change:** rename and new folder ask for a name; delete asks for a
   confirmation naming what goes. The listing refreshes after each.
6. **Failures:** no sftp subsystem, a refused login or a dropped connection
   shows ssh's reason in the panel with a "Connect again" button; nothing
   hangs.

### Startup impact

None. Nothing about SFTP starts before the user opens a panel.

### Platform notes

Windows 11's `ssh.exe` supports `-s … sftp` and askpass the same way (spec
0006). File dialogs are GPUI's: xdg-desktop-portal on Linux, the system
dialog on Windows. New crates, approved with this spec: `russh-sftp` 3,
and `tokio` as a direct dependency (already built for GPUI's HTTP client)
with `rt`, `process`, `io-util`, `fs` and `sync`.

### Admin area

None: zed-term is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Remote listings: names, sizes, dates | Memory only | Show the folder | The user's own device; no one else processes it | Until the panel closes |
| Transferred files | Where the user picks, local or remote | The transfer the user started | Same | The user's |

Nothing is cached or logged, and nothing leaves the machine except the
connection the user starts.

Trust boundaries:

- **Remote names are untrusted [load-bearing]:** shown as plain text by the
  title rules (spec 0003), used verbatim only to name the remote file. A
  download is written into the chosen folder only, under its last path
  component, refused if that's empty, `.`, `..`, or holds `/`, `\`, a NUL
  or a drive prefix (path traversal).
- **Credentials:** spec 0006 and threat model 0001 apply unchanged: the
  panel's ssh gets its own token and its prompts go through askpass.
- **Writes are the user's:** no download overwrites, and no upload
  replaces, without a confirmation; downloaded files are never opened or
  run by the app.
- A STRIDE model of this surface (T-024) comes before T-023 builds it.

## Acceptance criteria

1. On a key-auth host, "Open SFTP panel" lists the remote home folder within 2 s, as the same user and port the terminal pane uses.
2. On a password host, the panel's prompt dialog fills from the keychain, and the password appears in no process's arguments or environment.
3. Downloading a 1 GB file shows progress while the terminal pane beside it still takes typing, the copy matches the original's checksum, and cancel deletes the partial file.
4. Uploading over an existing remote file asks first; declining leaves the remote file as it was.
5. A remote file named `../../.bashrc`, `a/b` or with control characters shows as plain text, and its download lands only inside the chosen folder or is refused.
6. Rename, delete after its confirmation, and new folder work, and the listing shows the result.
7. A host without the sftp subsystem shows ssh's reason in the panel within 5 s; closing the panel ends its ssh process.
8. A relaunch restores the tab without its SFTP panel and makes no connection (spec 0003, 0006).
9. On Windows 11 the same works through `ssh.exe`.

## Open questions

- None.
