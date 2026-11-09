---
status: approved     # draft | approved | implemented
phase: 2
tasks: [T-018, T-011]
related: [spec 0005, spec 0006, spec 0009]
---

# Spec: Importing hosts from MobaXterm and Tabby

Pins a one-off import of the SSH hosts a user already keeps in MobaXterm
or Tabby, as Tern profiles, folders kept as host groups, run from the
command line: `tern --import-hosts <file>`.

## Problem & goal

People switching from MobaXterm or Tabby have dozens of hosts sorted in
folders. Retyping them is what keeps them from switching. The import
should bring the hosts and their grouping over in one go, and nothing
else: their saved passwords stay behind, because they'd bypass the
keychain flow of spec 0006.

Done means: `tern --import-hosts <file>` on a MobaXterm or Tabby file
adds its SSH hosts to `config.yaml` as `type: ssh` profiles in their
groups, prints what it did, and a running Tern has them in the host
picker at once.

## Scope

**In scope:**

- `tern --import-hosts <file> [--dry-run]`, which imports and exits without opening a window
- MobaXterm: `MobaXterm.ini` and `.mxtsessions` exports, SSH sessions and their folders (`SubRep`)
- Tabby: `config.yaml`, `type: ssh` profiles and their groups
- A printed report: what was imported, what was a duplicate, what was skipped and why; `--dry-run` prints it and writes nothing

**Out of scope:**

- Passwords, Tabby's vault, MobaXterm's stored credentials: never [load-bearing]
- Jump hosts, tunnels, X11 and terminal settings: not imported; the report lists what was left out (`~/.ssh/config` does jump hosts, spec 0006)
- RDP, VNC, serial, telnet and local shell sessions: skipped
- Syncing or re-importing: one-off, the user starts it
- An in-app import: the settings screen's Import tab, spec 0014
- Importing duplicates or picking single hosts: the user edits `config.yaml` or the source file instead

## Design

### Affected areas

- `src/import/` (one parser per format, the report), `src/config.rs` (appending profiles, spec 0009's in-place edits), `bench::Mode::parse` / `main.rs` (one more argv mode, dispatched before GPUI starts)

### Model

- **Imported host** = name, host, port (left out at 22), user, key file,
  group: what a `type: ssh` profile holds (spec 0006). Its ID is its name,
  lowercased and dashed, made unique against the config's IDs.
- **Group:** MobaXterm's folder path joined with ` / `; Tabby's group name.
- **Key files:** as written, except MobaXterm's `_ProfileDir_` prefix,
  which becomes `~`, and its `_CurrentDrive_`, which becomes `C`; Tabby's
  `file://` prefix is dropped, and a key in Tabby's vault isn't imported.
- **Skipped:** sessions that aren't SSH, and SSH hosts behind a jump host
  or a proxy (MobaXterm's gateway or proxy type; Tabby's `jumpHost`,
  `proxyCommand` or SOCKS/HTTP proxy): imported without it, they'd
  connect somewhere else. Tabby's missing `user` is its default, `root`.
- **Duplicate:** a host whose user, host and port match a profile or a
  `~/.ssh/config` alias already listed (spec 0006), or an earlier host
  in the same file. It's reported and not imported.
- **Formats** are told apart by content: an INI with `[Bookmarks` sections
  is MobaXterm, YAML with `profiles:` entries of `type:` is Tabby.
  MobaXterm's files are CP1252; its field positions follow the
  reverse-engineered format of MobaXterm 23 (there is no official one).

### Flows

1. **Run:** `tern --import-hosts <file>` is matched in `main` before
   anything else; it never starts GPUI, spawns a PTY or reads the session.
   A missing file argument prints the usage to stderr and exits 2.
2. **Read:** over 10 MB, unreadable, or not one of the formats: an error
   on stderr, exit 1, nothing written.
3. **Write:** with no `config.yaml` yet, it's created from the template
   first (`config::ensure_exists`). The new hosts are appended under
   `profiles:` in block style; no existing line changes [load-bearing].
   A config that doesn't parse, or a failed write, leaves the file as it
   was: an error on stderr, exit 1.
4. **Report:** to stdout, one line per imported host (name, destination,
   group), then the duplicates, then the skipped sessions counted by
   reason (not SSH, a jump host, ...). Exit 0, also when nothing was new.
5. **`--dry-run`:** steps 1, 2 and 4 with "would import"; nothing is
   written, not even a missing `config.yaml`.
6. **Running app:** its config watcher reloads, so the hosts are in the
   host picker at once; no IPC.

### Startup impact

One more arm in the argv match `main` already does for `--bench-startup`;
nothing else on the normal boot path. T-018's commit reports the TTFP to
show it.

### Platform notes

MobaXterm runs on Windows, so its exports name Windows paths; they're
kept as written (with `_ProfileDir_` resolved), which `ssh.exe` reads as
they are. On Linux an imported Windows key path won't exist; ssh says so
when connecting, as for any key path.

Tern is a console-subsystem binary on Windows today, so the report
reaches the shell it ran from. If that ever changes to the GUI subsystem,
the import must attach to the parent console (`AttachConsole`) or its
output is lost.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Hosts, users, key paths, folder names | Read from the named file; written to `config.yaml`; printed to the user's own terminal | The import the user starts | The user's own device; no one else processes it | As any profile (spec 0005) |

Nothing leaves the machine, and nothing is kept from the file beyond the
imported profiles.

Trust boundaries:

- **The named file is untrusted input:** names are printed with control
  characters stripped (no escape sequences reach the user's terminal),
  every value passes the config's own checks before it's written, and
  nothing in it is run.
- **No secrets [load-bearing]:** password fields, MobaXterm's credential
  entries and Tabby's vault are never parsed into memory beyond skipping
  them, never written or printed, and the keychain isn't touched.

## Acceptance criteria

1. A `MobaXterm.ini` with SSH sessions in nested folders imports each as a `type: ssh` profile with its host, port, user and key, grouped as `Folder / Subfolder`.
2. A Tabby `config.yaml` imports its SSH profiles with their groups; its local and serial profiles are skipped and counted in the report.
3. `--dry-run` prints the same hosts a real run would import and leaves `config.yaml` byte for byte as it was (or absent).
4. A host already in `config.yaml` or `~/.ssh/config` is reported as a duplicate and not imported; running the same import twice adds nothing the second time.
5. A file with saved passwords or a Tabby vault imports no secret: none in `config.yaml`, none in the output, none in the keychain.
6. The import appends without changing an existing line, and a running Tern has the hosts in its host picker within a second.
7. A malformed file, one over 10 MB, or a missing one exits non-zero with a message on stderr and writes nothing.
8. A MobaXterm key path under `_ProfileDir_` is imported under `~`.
9. `--import-hosts` opens no window and exits; a normal launch's TTFP is unchanged within noise.

## Open questions

- None.
