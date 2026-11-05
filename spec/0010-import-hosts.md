---
status: draft        # draft | approved | implemented
phase: 2
tasks: [T-018, T-011]
related: [spec 0005, spec 0006, spec 0009]
---

# Spec: Importing hosts from MobaXterm and Tabby

Pins a one-off import of the SSH hosts a user already keeps in MobaXterm
or Tabby, as zed-term profiles, folders kept as host groups.

## Problem & goal

People switching from MobaXterm or Tabby have dozens of hosts sorted in
folders. Retyping them is what keeps them from switching. The import
should bring the hosts and their grouping over in one go, and nothing
else: their saved passwords stay behind, because they'd bypass the
keychain flow of spec 0006.

Done means: picking a MobaXterm or Tabby file shows the SSH hosts it holds;
confirming adds them to `config.yaml` as `type: ssh` profiles in their
groups, and they're in the host picker at once.

## Scope

**In scope:**

- "Import hosts…" (`hosts.import`) in the palette, reading a file the user picks
- MobaXterm: `MobaXterm.ini` and `.mxtsessions` exports, SSH sessions and their folders (`SubRep`)
- Tabby: `config.yaml`, `type: ssh` profiles and their groups
- A preview to confirm, with duplicates marked

**Out of scope:**

- Passwords, Tabby's vault, MobaXterm's stored credentials: never [load-bearing]
- Jump hosts, tunnels, X11 and terminal settings: not imported; the preview lists what was left out (`~/.ssh/config` does jump hosts, spec 0006)
- RDP, VNC, serial, telnet and local shell sessions: skipped
- Syncing or re-importing: one-off, the user starts it

## Design

### Affected areas

- `src/import/` (one parser per format, the preview), `src/config.rs` (appending profiles, spec 0009's in-place edits)

### Model

- **Imported host** = name, host, port (left out at 22), user, key file,
  group: what a `type: ssh` profile holds (spec 0006). Its ID is its name,
  lowercased and dashed, made unique against the config's IDs.
- **Group:** MobaXterm's folder path joined with ` / `; Tabby's group name.
- **Key files:** as written, except MobaXterm's `_ProfileDir_` prefix,
  which becomes `~`; Tabby's `file://` prefix is dropped.
- **Duplicate:** a host whose user, host and port match a profile or a
  `~/.ssh/config` alias already listed (spec 0006). It's unticked in the
  preview.
- **Formats** are told apart by content: an INI with `[Bookmarks` sections
  is MobaXterm, YAML with `profiles:` entries of `type:` is Tabby. The
  parsers are tested against real exports the owner provides (T-018).

### Flows

1. **Pick:** the command opens the OS's file dialog. The file is read off
   the UI thread; over 10 MB, or not one of the formats, it shows a notice
   and imports nothing.
2. **Preview:** a list of the SSH hosts found, ticked to import, with name,
   destination and group; duplicates unticked; below it, how many
   sessions were skipped and why (not SSH, a jump host, ...).
3. **Confirm:** the ticked hosts are appended under `profiles:` in block
   style; no existing line changes. The watcher reloads the config, so
   they're in the host picker at once. A notice counts what was imported.
4. **Cancel**, Esc or a failed write: nothing changes; a failure says why.

### Startup impact

None: only the palette command starts it.

### Platform notes

MobaXterm runs on Windows, so its exports name Windows paths; they're
kept as written (with `_ProfileDir_` resolved), which `ssh.exe` reads as
they are. On Linux an imported Windows key path won't exist; ssh says so
when connecting, as for any key path.

### Admin area

None: zed-term is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Hosts, users, key paths, folder names | Read from the picked file; written to `config.yaml` once confirmed | The import the user starts | The user's own device; no one else processes it | As any profile (spec 0005) |

Nothing leaves the machine, and nothing is kept from the picked file
beyond the confirmed profiles.

Trust boundaries:

- **The picked file is untrusted input:** names show as plain text by the
  title rules (spec 0003), every value passes the config's own checks
  before it's written, and nothing in it is run.
- **No secrets [load-bearing]:** password fields, MobaXterm's credential
  entries and Tabby's vault are never parsed into memory beyond skipping
  them, never written, and the keychain isn't touched.

## Acceptance criteria

1. A `MobaXterm.ini` with SSH sessions in nested folders imports each as a `type: ssh` profile with its host, port, user and key, grouped as `Folder / Subfolder`.
2. A Tabby `config.yaml` imports its SSH profiles with their groups; its local and serial profiles are skipped and counted in the preview.
3. Nothing is written before the preview is confirmed, and canceling leaves `config.yaml` byte for byte as it was.
4. A host already in `config.yaml` or `~/.ssh/config` shows as a duplicate, unticked, and isn't imported unless ticked.
5. A file with saved passwords or a Tabby vault imports no secret: none in `config.yaml`, none in the keychain.
6. Confirming appends the profiles without changing an existing line, and the hosts are in the host picker within a second.
7. A malformed file, or one over 10 MB, shows a notice and imports nothing.
8. A MobaXterm key path under `_ProfileDir_` is imported under `~`.

## Open questions

- None.
