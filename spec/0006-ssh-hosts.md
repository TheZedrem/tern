---
status: approved     # draft | approved | implemented
phase: 2
tasks: [T-012, T-013, T-014, T-015, T-006, T-011]
related: [spec 0002, spec 0003, spec 0004, spec 0005, OQ-001 ✅, OQ-016, research 0001, threat model 0001]
---

# Spec: SSH hosts and connections

Pins how Tern lists SSH hosts, connects to them through the system
`ssh`, and answers ssh's prompts in the app, filling in a stored password
without it ever touching the terminal. SFTP is a separate, later spec.

## Problem & goal

Developers jump between many hosts. Tabby makes that one click but is
slow; plain terminals make you remember aliases and retype passwords.
Tern should list every host the user already has, connect in one
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

- `src/ssh/` (host list, command, askpass), `src/secrets.rs` (keychain), `src/config/` (ssh profiles), the palette's host provider (spec 0004)

### Model

- **Host** = id, label, group, destination. From a profile: the profile ID,
  its `name` (default: the ID), its `group` (default: "Hosts"). From
  `~/.ssh/config`: one host per `Host` alias without wildcards (`*`, `?`,
  `!`) and not starting with `-` (ssh would read it as an option), in
  group "SSH config", following `Include`; its ID is `ssh:<alias>`, which
  splits and session restore open again. A profile whose `host` is an
  alias hides that alias, so each destination is listed once.
- **Command [load-bearing]:** a config alias connects as `ssh <alias>`; a
  profile as `ssh [-l user] [-p port] [-i identity_file] <host>`, with only
  the options the profile sets, so `~/.ssh/config` still applies to it.
- **Target:** user, host and port as ssh resolves them (`ssh -G`, run only
  when the user connects to a host or forgets its password, never when
  listing hosts: `Match exec` lines run commands, TM-askpass-20). The host
  is `hostkeyalias` if set, else `hostname`, the name ssh writes into its
  prompts. Passwords are keyed by target (`user@host:port`) and
  passphrases by key path, so a profile and an alias for the same server
  share one entry.

### Module surface

- `ssh::hosts(config, ssh_config_path) -> Vec<Host>`, `ssh::command(&Host) -> pty::Command`, `ssh::forget(args)`
- `ssh::askpass`: the per-app-run server, and the helper entry point that `main` runs when ssh starts Tern as `SSH_ASKPASS`
- `ssh::target::Target`: `resolve` (`ssh -G`) and `secret_for(prompt)`, the auto-fill patterns
- `secrets::{get, set, delete}(key)` over `keyring`
- New crates: `keyring` 4 (already planned), `interprocess` (Unix socket or named pipe), `getrandom` (connection tokens), `zeroize` (overwriting secrets after use; OQ-016), and on Windows `widestring`, the type interprocess takes the pipe's security descriptor in

### Flows

1. **Pick:** `Ctrl+Shift+H` or "Connect to host…" opens the palette in host
   mode, grouped, filtered by label, host and group. Enter opens the host
   in a new tab named after it (spec 0003).
2. **Connect:** spawn the command in a pane (spec 0002) with
   `SSH_ASKPASS` set to the canonical absolute path of the running
   executable (never a name looked up on `PATH`),
   `SSH_ASKPASS_REQUIRE=force`, and the askpass endpoint, a fresh
   per-connection token and the app's PID in its environment. These go
   only to the ssh process, never to local panes. The endpoint opens with
   the first SSH connection:
   - Unix: a socket in a per-run directory, mode 0700, under
     `$XDG_RUNTIME_DIR`; the server drops any peer whose `SO_PEERCRED` uid
     isn't the app's. Without `$XDG_RUNTIME_DIR` there is no askpass, and
     ssh asks in the terminal, with a notice saying why (as for any
     endpoint that fails to open). A keychain lookup that fails or times
     out asks like nothing saved, with a notice.
   - Windows: a named pipe with 128 random bits in its name, created with
     `FILE_FLAG_FIRST_PIPE_INSTANCE` and a DACL for the current user only.
   - Either way the token is 128 bits from `getrandom`, compared in
     constant time and dropped when its ssh exits (threat model
     [0001](threat-model/0001-ssh-askpass.md), TM-askpass-01 to -03).
3. **Prompt:** ssh runs Tern as its askpass helper. `main` checks for
   helper mode first, only with both the endpoint and the token set, and
   the helper exits before any GPUI or config code runs. It checks that
   the endpoint's server process is the app's PID (`SO_PEERCRED`,
   `GetNamedPipeServerProcessId`), sends the prompt, the
   `SSH_ASKPASS_PROMPT` hint and the token in messages of at most 64 KiB,
   and writes the answer to stdout. It writes nothing to stderr, which is
   the PTY; any failure, a wrong or missing token among them, is exit
   code 1 with no output. The server handles each helper on a background
   task with a 30 s read timeout (TM-askpass-02, -11, -16, -18).
4. **Answer [load-bearing]:**
   - The connection's own password and passphrase prompts are filled from
     the keychain, if a secret is stored and hasn't been offered on this
     connection yet: a prompt that is exactly `<user>@<host>'s password: `,
     or `(<user>@<host>) ` followed by a server text of `password:` in any
     case with optional spaces (keyboard-interactive), or
     `Enter passphrase for key '<file>': ` for one of the connection's
     `identityfile`s, all from `ssh -G` (Target). ssh writes the
     `(user@host)` prefix itself, so a server can't name another target,
     and a jump host's prompts name the jump host (TM-askpass-04, -19).
   - Each fill shows a notice naming the target, never the secret:
     "Used the password saved for `<target>`" (TM-askpass-10). It doesn't
     claim the login worked: a wrong one gets the replace dialog next.
   - Every other prompt opens a dialog over the pane: host-key questions,
     one-time codes, a jump host's password, a second password prompt
     after the stored one failed. Nothing else is answered automatically.
     One dialog per connection at a time; closing the tab cancels its
     prompts (TM-askpass-15).
   - Dialogs show the prompt as plain text, control characters (but line
     breaks) and invisible or direction characters stripped by the title
     rules of spec 0003, capped at 4 KiB (TM-askpass-08).
   - `confirm` hint and host-key questions: a Yes/No dialog with ssh's
     full text, fingerprint included, where Enter and Esc mean No and Yes
     needs an explicit choice (TM-askpass-07). `none` hint: a notice that
     closes when ssh ends the helper.
   - Password and passphrase dialogs offer "Remember in keychain"; after a
     stored password failed, the dialog offers to replace it. Next to the
     option, a warning: any program the user runs can read what's saved
     there, and logging in with a private key is safer than a saved
     password (TM-askpass-14, accepted with this warning).
   - Esc cancels: the helper exits 1 and ssh counts a failed attempt.
   - A secret is held only between the keychain or the dialog and the
     socket write, in a buffer that `zeroize` overwrites when it's dropped,
     in the app and in the helper; keychain calls run on a thread of their
     own with a timeout that counts as nothing stored (TM-askpass-13,
     -17). The dialog's text field is the widget library's and isn't
     overwritten; it's cleared as soon as the answer is taken from it.
5. **Drop:** when ssh exits with 255 (connection failed or lost), the pane
   stays open with `[connection closed: press Enter to reconnect]`, and
   Enter reruns the same command. Other exits follow `close_on_exit`
   (spec 0002).
6. **Restore [load-bearing]:** a restored SSH tab never connects by itself;
   it shows `Press Enter to connect to <label>`. Connecting at boot would
   be network at startup (architecture §Ground rules). The same holds for
   the first tab when `default_profile` is an SSH profile.
7. **Forget:** "Forget saved password…" (`host.forget`) opens the host
   picker; picking a host resolves its target and forgets the password and
   key passphrases saved for it, with a notice naming what was forgotten,
   or saying nothing was saved. Knowing beforehand which hosts have one
   would take `ssh -G` while browsing (TM-askpass-20) or listing the
   keychain.
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

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| SSH profiles: host, user, port, key path, group | `config.yaml`, written by the user | Connect | The user's own device; no one else processes it | Until the user edits it |
| `~/.ssh/config` hosts | Read only, never copied | List hosts | Same | Not stored |
| Passwords and key passphrases | OS keychain, service `tern` | Log in without typing | The user's explicit "Remember" | Until "Forget" or replaced |
| Host ID per SSH tab | `session.json` | Restore tabs | Same | Overwritten on save (spec 0003) |
| Host keys | ssh's own `known_hosts` | Host verification | Same | Managed by ssh |

Nothing leaves the machine except the SSH connection the user starts.

Trust boundaries:

- **§Security 3 [load-bearing]:** a secret travels keychain → app → helper
  → ssh, over the authenticated local socket only. It is never in an
  environment variable, an argument, the PTY, a log, `config.yaml` or
  `session.json`. Tern never logs askpass traffic, prompts included:
  they name users and hosts.
- **§Security 1 [load-bearing]:** prompts come only from the local ssh
  through askpass. Nothing the remote side prints can trigger, read or
  fill a secret, and secrets are never asked for inside the grid.
- The token is random (128 bits), per connection, and only in that ssh
  process's environment. The socket is private to the user and closes with
  the app.
- The STRIDE threat model of this path is
  [0001](threat-model/0001-ssh-askpass.md) (T-015); flows 2 to 4 carry its
  mitigations. Accepted by the owner (OQ-016): code running as the user
  can read what the keychain holds (hence the warning at "Remember"), and
  a user may type a secret into a prompt the remote printed.

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
