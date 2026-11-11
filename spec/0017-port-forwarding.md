---
status: draft        # draft | approved | implemented
phase: 7
tasks: [T-075, T-076]
related: [spec 0005, spec 0006, spec 0008, spec 0009, spec 0016, OQ-033, threat model 0001]
---

# Spec: Port forwarding

Pins local, remote and dynamic (SOCKS) port forwards on an SSH profile,
passed to the system `ssh` as `-L`, `-R` and `-D` on the pane's
connection, and edited on the settings screen. Phase 7 started ahead of
Phases 3–6 (owner's call, 2026-11-11). Which of several connections to the
same profile holds the ports is OQ-033.

## Problem & goal

Developers reach databases, dashboards and internal web apps through SSH
tunnels. Today that takes `LocalForward` lines in `~/.ssh/config`, which
Tern honors but can't show or edit. A profile should carry its forwards,
so connecting to "prod-db" also makes `localhost:5432` reach the database.

Done means: a profile's forwards are set on the settings screen or in
`config.yaml`, open with the profile's terminal connection, listen on
loopback unless the user names another address, and close with it.

## Scope

**In scope:**

- A `forwards:` list on `type: ssh` profiles: local (`-L`), remote (`-R`), dynamic (`-D`), TCP only
- The forwards on the pane's own `ssh` connection, and on its reconnect
- The profile form's forwards rows (spec 0009, flow 4), with a warning for a non-loopback bind address
- A notice on connect naming the forwards asked for

**Out of scope:**

- Tunnels without a terminal (`ssh -N`, a tunnel manager with start/stop): *Later*; depends on OQ-033
- Unix socket forwards (`-L path:…`): *Later*
- Forwards on `~/.ssh/config` hosts: they're set in `~/.ssh/config` already, which `ssh` applies (spec 0006)
- Importing tunnels from MobaXterm, Tabby, PuTTY or WinSCP: *Later* (specs 0010, 0015 skip them)
- A live list of open forwards in the app sidebar: *Later*, with the sidebar (T-064)
- `request_portforward` for plugins: reserved in spec 0016, specced when plugins exist
- Agent forwarding and jump-host editors: still `~/.ssh/config` (spec 0006)

## Design

### Affected areas

- `src/config/` (the `forwards` key, its checks), `src/ssh/` (the command's arguments), `src/settings/pages/profiles.rs` (the form), `src/default-config.yaml` (the template line), spec 0009 flow 4 (amended by T-075)

### Model

```yaml
profiles:
  prod-db:
    type: ssh
    host: bastion
    forwards:
      - local: 5432              # or "127.0.0.1:5432", "[::1]:5432"
        to: db.internal:5432
      - remote: 9000             # a port on the server…
        to: localhost:3000       # …reaching this machine's port 3000
      - dynamic: 1080            # a SOCKS proxy on this machine
```

- **Forward** = kind (`local | remote | dynamic`), listen (`port` or
  `address:port`, IPv6 in brackets), and `to` (`host:port`) for local and
  remote. Ports 1–65535. `to` is required for local and remote and
  refused for dynamic.
- **Arguments [load-bearing]:** each forward becomes one `-L`, `-R` or
  `-D` argument before the host, in the order written:
  `ssh [-l user] [-p port] [-i file] -L 5432:db.internal:5432 -R 9000:localhost:3000 -D 1080 bastion`.
  A bind address is passed only if the user wrote one, so `ssh` binds to
  loopback by its default (`GatewayPorts no`, and the server's own for
  `-R`). The user's `~/.ssh/config` still applies, its own forwards
  included.
- **Only the terminal connection [load-bearing]:** the SFTP sidebar's
  connection, a files pane's (spec 0008, 0013) and `ssh -G` (spec 0006,
  Target) never carry forwards, so opening SFTP never binds a port twice.

### Module surface

- `config::Forward { kind, listen, to }`, `SshProfile.forwards: Vec<Forward>`
- `ssh::command(&SshProfile)` adds the forwards; `ssh::sftp_args` leaves them out
- `Forward::arg(&self) -> String`, the `-L`/`-R`/`-D` value; `Forward::summary(&self) -> String`, as the notice and form show it (`5432 → db.internal:5432`, `remote 9000 → localhost:3000`, `SOCKS 1080`)

### Flows

1. **Connect:** Enter on a waiting pane (spec 0006, flow 6) starts `ssh`
   with the forwards, and a notice says "Port forwards for `<label>`:
   …" with each summary. It names what was asked, not what worked:
   `ssh` reports a forward that failed in the pane (`bind … Address
   already in use`, `remote port forwarding failed`), and the connection
   goes on without it (`ssh`'s default, `ExitOnForwardFailure no`).
2. **Drop and reconnect:** the forwards close with the connection;
   Enter reruns the same command, forwards included (spec 0006, flow 5).
3. **More connections to the profile:** a split, a second tab or a
   restored tab of the same profile runs the same command. While another
   pane holds a port, `ssh` prints its bind warning there, and that pane
   works without the forward (TODO(OQ-033)).
4. **Edit:** the profile form (spec 0009, flow 4) shows one row per
   forward: kind, listen, to, and a remove button; "Add forward" adds a
   row. A listen address that isn't loopback (`127.0.0.1`, `::1`,
   `localhost`) shows a warning under the row: "Reachable from other
   machines on the network" (for dynamic: "an open proxy to `<host>`'s
   network"). Saving writes the `forwards:` block; a profile without
   forwards writes none.
5. **Bad config:** a forward with a bad port, a missing or extra `to`, or
   an unknown kind is skipped with a notice naming the profile and the
   forward; the profile and its other forwards still load (spec 0005's
   rule for bad entries). Two forwards listening on the same address and
   port: the second is skipped the same way.
6. **Close:** closing the pane ends its `ssh`, which closes its forwards.
   A program still using a forward sees its connection drop.

### Startup impact

None. Forwards are arguments of a connection the user starts; a restored
SSH tab still waits for Enter (spec 0006, flow 6), so nothing listens or
connects at boot.

### Platform notes

Both use the system OpenSSH client, which supports `-L`, `-R` and `-D`
on Linux and in Windows 11's built-in `ssh.exe`; arguments reach it whole
on Windows since T-074. Linux: a listen port below 1024 needs privileges
(`net.ipv4.ip_unprivileged_port_start`), and `ssh` says so in the pane.
Windows: no privileged ports, but listening on a non-loopback address
makes Windows Defender Firewall ask about `ssh.exe` once; loopback asks
nothing. macOS only needs to compile.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Forwards: ports, addresses, target hosts | `config.yaml`, written by the user | Open the forwards on connect | The user's own device; no one else processes it | Until the user edits it |

Nothing new is sent: the forwards travel inside the SSH connection the
user starts. Trust boundaries:

- **New: listening sockets [load-bearing].** A local or dynamic forward
  lets whatever can reach its address use the user's SSH access. Loopback
  by default limits that to this machine's processes, the same as
  `~/.ssh/config`'s `LocalForward`. Another address is only ever what the
  user typed, with the form's warning; Tern never widens a bind, and it
  never adds `-g` or `GatewayPorts`.
- A remote forward opens a port on the server that reaches this machine;
  the server's `GatewayPorts` decides who can use it, and the target is
  what the user typed.
- Ground rule 1 holds: no forward opens before the user connects.
- The STRIDE threat model of forwards is T-076, before T-075 builds it.

## Acceptance criteria

1. A profile with `local: 5432, to: db.internal:5432` connects with exactly `-L 5432:db.internal:5432` before the host, and `psql -h localhost -p 5432` in a local pane reaches the database.
2. With `dynamic: 1080`, `curl --socks5-hostname localhost:1080 http://intranet/` works, and the port listens on loopback only (`ss -ltn` on Linux, `Get-NetTCPConnection -State Listen` on Windows).
3. With `remote: 9000, to: localhost:3000`, `curl localhost:9000` on the server reaches the dev server on this machine.
4. Opening the SFTP sidebar or a files pane on that profile binds no port: its `ssh` has no `-L`, `-R` or `-D`, and no bind warning shows anywhere.
5. A second tab of the same profile connects, shows `ssh`'s bind warning, and the first tab's forward keeps working.
6. `local: 70000`, a `dynamic` with `to`, and a `local` without `to` each give one notice naming the profile, and the profile connects with its valid forwards.
7. Relaunching with a restored tab of that profile listens on no port until Enter is pressed in it.
8. Adding a forward in the profile form writes only a `forwards:` block under that profile, other comments and order kept; a listen address `0.0.0.0:5432` shows the network warning before saving.
9. Dropping the connection closes the port; Enter reconnects and opens it again.
10. On Windows 11, criteria 1 and 2 hold with the built-in `ssh.exe`, and a loopback-only forward raises no firewall prompt.

## Open questions

- OQ-033: which connection holds a profile's forwards when it's open in more than one pane.
