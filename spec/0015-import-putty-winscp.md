---
status: implemented  # draft | approved | implemented
phase: 2
tasks: [T-059, T-060]
related: [spec 0010, spec 0014, spec 0006, OQ-029]
---

# Spec: Importing hosts from PuTTY and WinSCP

Pins PuTTY and WinSCP as two more import sources: the same one-off import
as spec 0010 (`tern --import-hosts <file>`) and two more cards on spec
0014's Import tab. It takes up spec 0014's *Later* item "other sources"
for these two, and leaves OQ-029 (how the registry is read) open.

## Problem & goal

PuTTY and WinSCP are what most Windows admins already have, often both,
with the same hosts saved twice. They keep their sessions in the Windows
registry, not a file the user can point at, so without help an import
means `reg export` first.

Done means: Settings → Import on Windows shows a PuTTY and a WinSCP card
with the count of new hosts read straight from the registry, one click
imports them as spec 0010 does, and `tern --import-hosts` takes their
`.reg` exports, `WinSCP.ini`, and PuTTY's `~/.putty/sessions` on Linux.
PuTTY sessions often save no user (PuTTY asks "login as:" when it
connects), so the import asks for one, for all hosts at once or per host.

## Scope

**In scope:**

- PuTTY: its sessions in the registry, a `.reg` export of them, and `~/.putty/sessions/` on Linux; SSH sessions only
- WinSCP: its sites in the registry, a `.reg` export of them, and `WinSCP.ini`; SFTP and SCP sites only, folders kept as groups
- Two cards on the Import tab, after Tabby, with spec 0014's states and flows
- `--import-hosts` accepting these files (and the `sessions` directory), told apart by content
- Asking for the user of hosts that have none, on the card and in the CLI

**Out of scope:**

- Passwords: WinSCP's stored (obfuscated or master-password-encrypted) passwords are never read [load-bearing], as spec 0010
- Converting `.ppk` keys: the user runs `puttygen`; see Model
- Pageant: not touched; see Platform notes
- Tunnels, port forwards, terminal and colour settings: not imported, as spec 0010
- Other sources (SecureCRT, Termius, KiTTY's own store): *Later*, one spec each
- FTP, WebDAV and S3 sites, and PuTTY's telnet, rlogin, raw and serial sessions: skipped

## Design

### Affected areas

- `src/import/` (a `putty` and a `winscp` parser in their own files, a
  `.reg` reader, the registry reader for the tab), `src/settings/pages/import.rs`
  (two more `Source` cards). No change to the plan, duplicate rules or write.

### Model

- **Sessions as key/value sets [load-bearing]:** every source (registry,
  `.reg`, `WinSCP.ini`, a `~/.putty/sessions` file) is read into the same
  list of named sessions with their values; the PuTTY and WinSCP parsers
  only see that list. The registry thus goes through the same parser as
  an export, and a CLI import of `reg export` output equals a click on
  the card (spec 0014's one-source-of-truth rule).
- **Formats by content:** a `.reg` file (UTF-16 with BOM or UTF-8,
  `Windows Registry Editor` header) is PuTTY for sections under
  `…\SimonTatham\PuTTY\Sessions\`, WinSCP under `…\Martin Prikryl\WinSCP 2\Sessions\`;
  an INI with `[Sessions\…]` sections is `WinSCP.ini`; a directory is a
  PuTTY `sessions` directory. Session names are URL-decoded (`%20`).
- **PuTTY host:** `HostName` (a `user@host` in it is split), `PortNumber`,
  `UserName`, `PublicKeyFile`; name is the session name; no group (PuTTY
  has no folders). `Default Settings` is a template, not a host: skipped
  and not counted. A missing `Protocol` is SSH, PuTTY's default.
- **WinSCP host:** `HostName`, `PortNumber`, `UserName`, `PublicKeyFile`;
  the site name's last `/` segment is the name, the segments before it
  the group, joined with ` / ` as spec 0010 does. Workspace entries are
  skipped as "a WinSCP workspace" (they copy sites the user also has).
- **Skipped:** not SSH (by `Protocol` / `FSProtocol`); a jump host
  (PuTTY's SSH proxy type, WinSCP's `Tunnel`); any other proxy. Same
  reasons and report as spec 0010.
- **`.ppk` keys:** OpenSSH can't read them. A key path ending in `.ppk`
  is dropped and the host imported without it; the report lists those
  hosts with the hint `puttygen <key> -O private-openssh -o <key>.pem`.
  The key file itself is never opened [load-bearing].

- **Hosts without a user:** a PuTTY or WinSCP host with no `UserName`
  is asked for one (not MobaXterm or Tabby: their files always say who).
  The answer is a user *for all* plus one *per host*; a host's own entry
  wins, a blank one takes the "for all" entry, and with both blank the
  host is imported without a user, as before. Users are set before the
  plan, so duplicates are judged on the user that's written. An entered
  user is cleaned and checked like a read one. Hosts are told apart by
  their full site name (group and name).

### Module surface

- `Source::PuTTY` and `Source::WinSCP` beside MobaXterm and Tabby, with
  their default places: on Windows the registry key (WinSCP: the registry,
  or `%APPDATA%\WinSCP.ini` if the registry has no sites); on Linux
  `~/.putty/sessions` for PuTTY, none for WinSCP.
- `Outcome` and the CLI report gain a "without key" list; `Outcome`
  lists the hosts asking for a user.
- The import takes the users entered (for all, per host) beside the
  source; the card and the CLI collect them, the import applies them.

### Flows

1. **CLI:** `tern --import-hosts <file-or-dir>` reads, detects and imports
   as spec 0010, flows 1–5. A `.reg` with neither app's sessions is "not
   a known format", exit 1. One with both (an export of all of
   `HKCU\Software`) imports PuTTY's; WinSCP's take an export of its own key. Over 10 MB, as spec 0010.
2. **Tab opens (Windows):** the PuTTY and WinSCP cards read their registry
   keys on the background executor [load-bearing], as spec 0014 flow 1.
   No key or no sessions: "No PuTTY sessions found" with "Choose file…"
   (a `.reg` export or `WinSCP.ini`).
3. **Tab opens (Linux):** the PuTTY card reads `~/.putty/sessions`; the
   WinSCP card says WinSCP runs on Windows and offers "Choose file…".
4. **Import:** spec 0014 flow 4, unchanged: re-read, re-plan against the
   config as it is now, append. The card's result adds "N without key
   (PuTTY format)" when any `.ppk` was dropped.
5. **Users (card):** with hosts lacking a user, the card shows a "User
   for all" input, then one input per such host (labelled with its name
   and host) whose placeholder shows the "for all" entry. The entries
   stay on the card until it reads another file; Import uses them.
   Typing doesn't re-plan the count; Import re-plans as flow 4 does.
   Past 50 such hosts the rest get no input of their own and take the
   "for all" entry, so thousands of sessions keep the tab responsive.
6. **Users (CLI):** with stdin a terminal, after reading and before
   planning, the import asks on stderr "N hosts have no user. User for
   all (blank: ask per host):" and, if that was blank, one line per host
   ("  name (host): "). A blank answer leaves that host without a user;
   end of input leaves the rest without. With stdin not a terminal,
   nothing is asked [load-bearing: scripts never hang]. `--dry-run` asks
   too, since the user decides what's a duplicate.
7. **Overlap:** a host in both PuTTY and WinSCP imports from whichever
   card is clicked first; the second counts it as a duplicate (spec 0010).
8. **Failure:** an unreadable key or value, or more than 10 MB of values
   read: the card shows one error line and nothing is written. A single
   session with a bad host or port is skipped and counted, as spec 0010.

### Startup impact

None: the registry and files are read only on the Import tab or by
`--import-hosts`, which never starts GPUI.

### Platform notes

- Windows 11: both cards read `HKEY_CURRENT_USER` only, through
  `windows-sys`'s registry API (OQ-029), and only the values the parsers
  use, by name: a `Password` value is never queried.
- Windows 11, CLI: the release `tern.exe` is a GUI-subsystem binary that
  attaches to its parent's console; the shell doesn't wait for it and
  reads the same keyboard, so that build never asks for users (flow 6)
  and imports without them. The Import tab is where Windows users set
  them. A console-subsystem (debug) build asks as on Linux. Key paths stay as written.
  Many PuTTY users authenticate through Pageant; the system `ssh` doesn't
  talk to Pageant unless the user enables Pageant's OpenSSH pipe, so a
  host imported without key may ask for a password. Docs say so; Tern
  doesn't configure it.
- Linux (Wayland): PuTTY's `~/.putty/sessions` is found; a `.reg` from a
  Windows machine imports through "Choose file…" or the CLI; its Windows
  key paths won't exist, as spec 0010 says for MobaXterm.
- macOS: compiles; both cards are "Choose file…" only.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Users typed for hosts without one | On the card while it's open; written to `config.yaml` with the host | The import the user starts | As below | As any profile |
| Hosts, users, key paths, folder names | Read from the registry or the chosen file; written to `config.yaml`; shown as counts | The import the user starts | The user's own device; no one else processes it | As any profile (spec 0005) |

Trust boundaries as spec 0010: the registry and the file are untrusted
input, every value passes the config's checks, nothing is run. Only the
two apps' session keys under `HKEY_CURRENT_USER` are read. WinSCP's
`Password` values and master-password data are skipped unparsed, never
held, written, shown or sent to the keychain [load-bearing]; `.ppk` files
are never opened. Nothing leaves the machine.

## Acceptance criteria

1. On Windows with three PuTTY SSH sessions and one telnet session in the registry, the PuTTY card shows "3 new hosts" and 1 skipped (not SSH) without any click, and Import adds the three to `config.yaml`.
2. `tern --import-hosts putty.reg` on `reg export HKCU\Software\SimonTatham\PuTTY\Sessions putty.reg` output (UTF-16) writes the same `config.yaml` as the PuTTY card's Import on the same machine.
3. WinSCP sites `Prod/Web/app1` (SFTP) and `ftp-box` (FTP) import `app1` in group `Prod / Web` and skip `ftp-box` as not SSH; the same from the registry, a `.reg` export and `WinSCP.ini`.
4. A WinSCP site with a saved password and a PuTTY session with a `.ppk` key: no password in `config.yaml`, the output or the keychain; the PuTTY host is imported without `identity_file`, and the report names it with the `puttygen` hint.
5. A host saved in both PuTTY and WinSCP imports once: the second card counts it as a duplicate.
6. PuTTY's `Default Settings` session never shows up as a host or in a count; a session named `my%20box` is imported as `my box`; a session behind a PuTTY SSH proxy or a WinSCP tunnel is skipped as "behind a jump host".
7. Three PuTTY sessions without a user: `alice` for all and `bob` for the second imports `alice`, `bob`, `alice`; everything blank imports them without `user:`; a session that saved `carol` is never asked.
8. On Linux, a `~/.putty/sessions/` directory is found by the PuTTY card and imports its SSH sessions; the WinSCP card offers only "Choose file…".
9. `tern --import-hosts putty.reg` in a terminal asks for users as flow 6; with stdin piped or `/dev/null` it asks nothing and imports without users.
10. Opening the Import tab with 5,000 PuTTY sessions in the registry keeps the screen responsive, and a normal launch's TTFP is unchanged.

## Open questions

- OQ-029 ✅: the registry is read through `windows-sys`'s `Win32_System_Registry` feature.
