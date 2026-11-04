---
status: approved     # draft | approved | implemented
phase: 2
tasks: [T-012, T-013, T-014, T-015, T-006, T-011]
related: [spec 0002, spec 0003, spec 0004, spec 0005, OQ-001 ✅, research 0001]
---

# Spec: SSH hosts and connections

Pins how zed-term lists SSH hosts, connects to them through the system
`ssh`, and answers ssh's prompts in the app, filling in a stored password
without it ever touching the terminal. SFTP is a separate, later spec.

## Problem & goal

Developers jump between many hosts. Tabby makes that one click but is
slow; plain terminals make you remember aliases and retype passwords.
zed-term should list every host the user already has, connect in one
keystroke, and remember a password only when asked, in the OS keychain.

Done means: every `~/.ssh/config` host and every `type: ssh` profile is in
the host picker; picking one opens a connected tab; a remembered password
logs in without typing and never passes through the PTY, process
arguments, the environment or a log.

## Scope

**In scope:**

- `type: ssh` profiles (spec 0005's schema plus `group`), and hosts read live from `~/.ssh/config`
- The host picker: a palette mode with groups and filtering that connects in a new tab
- Connecting through the system `ssh` in a PTY pane, and reconnecting after a drop
- ssh's prompts (passwords, passphrases, host keys, one-time codes) answered in the app through `SSH_ASKPASS`; passwords remembered in the OS keychain on request

**Out of scope:**

- SFTP panel: its own spec (T-016)
- Adding and editing hosts in a form: *Later*; hosts are edited in `config.yaml` ("Open config file")
- Port forwarding, agent forwarding and jump-host editors: *Later*; `~/.ssh/config` does all of them today
- An SSH implementation of our own: never (architecture §Stack)

## Design

### Affected areas

- `src/ssh/` (host list, command, askpass), `src/secrets.rs` (keychain), `src/config.rs` (ssh profiles), the palette's host provider (spec 0004)

### Model

- **Host** = id, label, group, destination. From a profile: the profile ID,
  its `name` (default: the ID), its `group` (default: "Hosts"). From
  `~/.ssh/config`: one host per `Host` alias without wildcards (`*`, `?`,
  `!`), in group "SSH config", following `Include`. A profile whose `host`
  is an alias hides that alias, so each destination is listed once.
- **Command [load-bearing]:** a config alias connects as `ssh <alias>`; a
  profile as `ssh [-l user] [-p port] [-i identity_file] <host>`, with only
  the options the profile sets, so `~/.ssh/config` still applies to it.
- **Target:** user, host and port as ssh resolves them (`ssh -G`, run at
  connect time). Passwords are keyed by target (`user@host:port`) and
  passphrases by key path, so a profile and an alias for the same server
  share one entry.

### Module surface

- `ssh::hosts(config, ssh_config_path) -> Vec<Host>`, `ssh::command(&Host) -> pty::Command`
- `ssh::askpass`: the per-app-run server, and the helper entry point that `main` runs when ssh starts zed-term as `SSH_ASKPASS`
- `secrets::{get, set, delete}(key)` over `keyring`
- New crates: `keyring` 4 (already planned), `interprocess` (Unix socket or named pipe), `getrandom` (connection tokens)

### Flows

1. **Pick:** `Ctrl+Shift+H` or "Connect to host…" opens the palette in host
   mode, grouped, filtered by label, host and group. Enter opens the host
   in a new tab named after it (spec 0003).
2. **Connect:** spawn the command in a pane (spec 0002) with
   `SSH_ASKPASS=<zed-term binary>`, `SSH_ASKPASS_REQUIRE=force`, the
   askpass endpoint and a fresh per-connection token in its environment.
3. **Prompt:** ssh runs zed-term as its askpass helper. The helper starts
   no UI and exits before any GPUI code runs; it sends the prompt, the
   `SSH_ASKPASS_PROMPT` hint and the token to the running app over the
   local socket, and prints the answer. A wrong or missing token gets
   nothing and exit code 1.
4. **Answer [load-bearing]:**
   - ssh's password prompt for the connection's own target
     (`<user>@<host>'s password: `, or `(<user>@<host>) Password: ` for
     keyboard-interactive) and its passphrase prompt for the profile's
     `identity_file` are filled from the keychain, if a secret is stored
     and hasn't been offered on this connection yet.
   - Every other prompt opens a dialog over the pane: host-key questions,
     one-time codes, a jump host's password, a second password prompt
     after the stored one failed. Nothing else is answered automatically.
   - `confirm` hint: a Yes/No dialog. `none` hint: a notice that closes
     when ssh ends the helper.
   - Password and passphrase dialogs offer "Remember in keychain"; after a
     stored password failed, the dialog offers to replace it.
   - Esc cancels: the helper exits 1 and ssh counts a failed attempt.
5. **Drop:** when ssh exits with 255 (connection failed or lost), the pane
   stays open with `[connection closed: press Enter to reconnect]`, and
   Enter reruns the same command. Other exits follow `close_on_exit`
   (spec 0002).
6. **Restore [load-bearing]:** a restored SSH tab never connects by itself;
   it shows `Press Enter to connect to <label>`. Connecting at boot would
   be network at startup (architecture §Ground rules). The same holds for
   the first tab when `default_profile` is an SSH profile.
7. **Forget:** the picker offers "Forget saved password" on a host that
   has one.
8. **Failures:** no `ssh` on `PATH` shows that in the pane, with how to
   install the OpenSSH client on this OS. No keychain provider (no Secret
   Service running) hides "Remember" and shows a notice once; connecting
   still works. An unreadable `~/.ssh/config` or `Include` shows a notice
   naming the file, and its hosts are missing.

### Startup impact

None. The host list is built when the picker first opens, the askpass
socket opens with the first SSH connection, and the keychain is touched
only to answer a prompt. Helper mode exits before any UI code.

### Platform notes

Linux: Secret Service (GNOME Keyring, KWallet, KeePassXC) through
`keyring`'s pure-Rust D-Bus store; the askpass socket lives in a private
directory under `$XDG_RUNTIME_DIR`. Windows 11: the built-in OpenSSH client
(`ssh.exe`, an optional feature that is on by default), which honors
askpass only with `SSH_ASKPASS_REQUIRE` set; Credential Manager; a named
pipe for the socket; `~/.ssh/config` is `%USERPROFILE%\.ssh\config`.

### Admin area

None: zed-term is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| SSH profiles: host, user, port, key path, group | `config.yaml`, written by the user | Connect | The user's own device; no one else processes it | Until the user edits it |
| `~/.ssh/config` hosts | Read only, never copied | List hosts | Same | Not stored |
| Passwords and key passphrases | OS keychain, service `zed-term` | Log in without typing | The user's explicit "Remember" | Until "Forget" or replaced |
| Host ID per SSH tab | `session.json` | Restore tabs | Same | Overwritten on save (spec 0003) |
| Host keys | ssh's own `known_hosts` | Host verification | Same | Managed by ssh |

Nothing leaves the machine except the SSH connection the user starts.

Trust boundaries:

- **§Security 3 [load-bearing]:** a secret travels keychain → app → helper
  → ssh, over the authenticated local socket only. It is never in an
  environment variable, an argument, the PTY, a log, `config.yaml` or
  `session.json`.
- **§Security 1 [load-bearing]:** prompts come only from the local ssh
  through askpass. Nothing the remote side prints can trigger, read or
  fill a secret.
- The token is random (128 bits), per connection, and only in that ssh
  process's environment. The socket is private to the user and closes with
  the app.
- A STRIDE threat model of the askpass path (T-015) comes before T-014 is
  built.

## Acceptance criteria

1. A host defined only in `~/.ssh/config`, pulled in through `Include`, is in the picker; `Host *` and `Host !foo` entries are not.
2. Typing part of a group name narrows the picker to that group; Enter opens a connected tab named after the host.
3. A profile with `user`, `port` and `identity_file` runs exactly `ssh -l <user> -p <port> -i <file> <host>`, and the user's `~/.ssh/config` options for that host still apply.
4. With a remembered password, connecting logs in without typing, and the password appears nowhere in the PTY output, `/proc/<ssh pid>/environ`, `/proc/<ssh pid>/cmdline`, `config.yaml` or `session.json`.
5. A wrong remembered password leads to exactly one dialog, with no retry loop, offering to replace it.
6. A first connection to an unknown host shows ssh's full host-key question with the fingerprint in a dialog, and nothing answers it automatically.
7. Running the helper by hand with a wrong token prints no secret and exits 1.
8. Cutting the network mid-session leaves the tab open with the reconnect hint, and Enter reconnects.
9. Relaunching with a saved SSH tab opens no network connection until Enter is pressed in that tab.
10. On Windows 11, the same remembered-password login works through Credential Manager and a named pipe.

## Open questions

- None.
