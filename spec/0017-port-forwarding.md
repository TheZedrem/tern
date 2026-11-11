---
status: approved     # draft | approved | implemented
phase: 7
tasks: [T-076, T-075, T-077]
related: [spec 0006, spec 0008, spec 0016, OQ-033, OQ-034, threat model 0001, threat model 0004]
---

# Spec: Port forwarding

Pins local, remote and dynamic (SOCKS) port forwards created from an SSH
pane's sidebar, which switches between its files and its forwards. Each
forward is its own background `ssh -N` to the pane's host, lives until
the user stops it or Tern quits, and is never saved. Phase 7 started
ahead of Phases 3–6 (owner's call, 2026-11-11). Resolves OQ-033.

## Problem & goal

Developers reach databases, dashboards and internal web apps through SSH
tunnels, usually for a while and then not. Today that takes
`LocalForward` lines in `~/.ssh/config` or an `ssh -L` typed by hand. In
Tern it should be a few clicks next to the connection they're already
in, and gone as quickly.

Done means: in an SSH pane's sidebar the user adds a forward in one row,
sees whether it's up, and deletes it with one click; it keeps running
after its tab closes, until it's deleted or Tern quits.

## Scope

**In scope:**

- A switch at the top of the SSH pane's sidebar (spec 0008) between Files and Port forwards
- Local (`-L`), remote (`-R`) and dynamic (`-D`) TCP forwards, one `ssh -N` process each, for the pane's host
- Their state (starting, up, failed with `ssh`'s reason), delete, and retry
- `ssh`'s prompts for a forward, answered in the app like the pane's own (spec 0006)
- A palette command listing every running forward, to stop one whose tabs are closed

**Out of scope:**

- Saving forwards, in the profile or the session: never in this spec (owner, 2026-11-11). `~/.ssh/config`'s `LocalForward` stays the way to keep one
- Unix socket forwards: *Later*
- Forwards on local panes, which have no host: never
- Importing tunnels from MobaXterm, Tabby, PuTTY or WinSCP: never, as nothing is saved
- `request_portforward` for plugins: reserved in spec 0016, specced with the plugin API
- Agent forwarding and jump-host editors: still `~/.ssh/config` (spec 0006)

## Design

### Affected areas

- New `src/forwards/` (the registry, processes, the sidebar view); `src/files/browser/` and the sidebar host in `src/layout/files.rs` (the switch); `src/ssh/` (the command); spec 0008 (the switch, amended by T-075)

### Model

- **Forward** = id, host (the pane's connection: its `ssh` arguments and
  label, as the SFTP sidebar takes them, so `~/.ssh/config` hosts work
  too), kind (`local | remote | dynamic`), listen port, `network`
  (off unless the user turns it on: listen on every IPv4 interface
  instead of `127.0.0.1`), `to` (`host:port`, for local and remote), and
  state (`starting | up | failed(reason)`). There is no free-typed bind
  address (owner, 2026-11-11).
- **Registry:** one per app run, owned by the workspace, keyed by host.
  Every SSH pane of a host shows that host's forwards; none belongs to a
  pane. Held in memory only.
- **Command [load-bearing]:** the pane's `ssh` options, then
  `-N -o ExitOnForwardFailure=yes` and one `-L`, `-R` or `-D`, then the
  host: `ssh -l me -N -o ExitOnForwardFailure=yes -L 127.0.0.1:5432:db.internal:5432 bastion`.
  The bind address is `127.0.0.1`, or `0.0.0.0` with `network` on, and
  is always written out: without one, `GatewayPorts yes` in
  `~/.ssh/config` would make `ssh` listen on every interface (TM-fwd-01).
  The same holds for `-R` (on the server) and `-D`. The value is built
  from the add row's typed fields only and passed as one argument
  (TM-fwd-05). It runs with stdout to null, stderr on a pipe the
  forward's thread reads to the end, keeping only its last KiB
  (TM-fwd-15), and no terminal, like the SFTP connection, with an
  askpass token of its own.
- **Process:** Tern keeps each forward's `Child` and stops it with
  `kill` and `wait` on that handle, so a reused PID is never hit
  (TM-fwd-14). The OS ends forwards with Tern, crash included: Linux
  `PR_SET_PDEATHSIG` set in `pre_exec`, spawned from a thread that lives
  as long as the app; Windows a Job object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (TM-fwd-13, pending OQ-034's
  `libc` answer).
- **Up:** a forward counts as up once its `ssh` has run 2 s without
  exiting, counted only while none of its prompts is open (TM-fwd-10);
  `ExitOnForwardFailure` makes a failed bind or a refused remote forward
  end it before that.

### Module surface

- `forwards::Registry`: `add(host, Forward) -> Id`, `remove(Id)`, `retry(Id)`, `of(host)`, `all()`, `stop_all()`; emits a change event the sidebar redraws on
- `forwards::Forward::arg()` (the `-L`/`-R`/`-D` value) and `summary()` (`5432 → db.internal:5432`, `remote 9000 → localhost:3000`, `SOCKS 1080`, `5432 (network) → db.internal:5432`)
- `ssh::forward_args(pane_args, &Forward) -> Vec<String>`, next to `ssh::sftp_args`
- Commands `pane.portForwards` (open the sidebar on Port forwards, the keyboard in the add row) and `forwards.stop` (pick a running forward to stop), in the palette and bindable, no default keys
- Keys of the Port forwards view: Tab and Shift+Tab walk the kind buttons, fields, checkbox, Add and the list; Enter in a field adds; Esc closes the sidebar. In the list, Up and Down select, Enter retries a failed forward, Delete or Backspace deletes

### Flows

1. **Open:** `pane.portForwards`, or the switch in an open sidebar, shows
   Port forwards; the sidebar keeps the last view it showed. Opening it
   connects nothing.
2. **Add:** a form at the top takes kind (Local, Remote, SOCKS), listen
   port, to, and a "Let other machines connect" checkbox, off each time
   the form opens. It says in words which side listens and where the
   forward leads: one sentence per kind, and each field labeled with its
   side ("Port on this machine", "Port on `<host>`", "Leads to (as
   `<host>` sees it)"); under the checkbox, who can connect. The fields
   are checked as typed: a port outside 1–65535, a missing
   `to` for Local and Remote, a `to` host that is neither an IP (IPv6 in
   brackets) nor a name of letters, digits, `.`, `-` and `_` not
   starting with `-` (TM-fwd-05), or a listen port this host already has
   a forward on keeps Add disabled with the reason. Add starts the
   forward and lists it as starting.
3. **Let other machines connect:** only checking the box binds `0.0.0.0`, and
   while it is checked the row shows, before Add, "Reachable from other
   machines on the network" (SOCKS: "an open proxy into `<host>`'s
   network"; Remote: "Reachable from other machines on `<host>`'s
   network, if the server allows it"). Such a forward's row and summary
   say "network" (`5432 (network) → db.internal:5432`).
4. **Prompts:** a password, passphrase or host-key question opens the
   dialog over the pane the forward was added from, marked as the
   forward's, by spec 0006's flow 4 and its keychain fill. Esc cancels:
   the forward fails with "cancelled".
5. **Fail:** `ssh` exiting within 2 s, or later (connection lost, server
   gone), marks it failed with the last three lines `ssh` wrote to
   stderr, joined with ` · `, as plain text up to 200 characters (spec
   0003's title rules), else its exit code. Three, because a failed bind
   ends on "Could not request local forwarding.", two lines after the one
   that names the port. A failed forward stays listed with Retry and Delete.
   Nothing reconnects by itself.
6. **Delete:** stops its `ssh` (the `Child` kept from spawn, never a kill
   by name) and removes the row. A program using the forward sees its
   connection drop.
7. **Tabs closed:** closing every pane of a host keeps its forwards up;
   `forwards.stop` lists every forward as `<label>: <summary>`, and
   picking one stops it. Opening a pane of that host again shows them in
   its sidebar.
8. **Quit:** Tern quitting stops every forward. Nothing is saved; a
   restored SSH tab has no forwards (spec 0006, flow 6).
9. **`~/.ssh/config` forwards:** the host's own `LocalForward` lines apply
   to every `ssh` it starts. If one of those is already bound (by the
   pane's connection), `ExitOnForwardFailure` fails the new forward, and
   its reason names that port.

### Startup impact

None. Nothing is saved, so nothing is restored; the registry is created
with the first forward.

### Platform notes

Both use the system OpenSSH client: `-N`, `-L`, `-R`, `-D` and
`ExitOnForwardFailure` work alike on Linux and in Windows 11's
`ssh.exe`, and arguments reach it whole on Windows (T-074). OpenSSH's
connection sharing (`ControlMaster`), which could add a forward to the
pane's own connection, doesn't exist on Windows, hence one process per
forward. Linux: a listen port below 1024 needs privileges, and `ssh`'s
reason says so. Windows: a forward with "Let other machines connect" makes
Windows Defender Firewall ask about `ssh.exe` once; loopback asks
nothing. Stopping uses the process handle on both. Tying forwards to
Tern's life differs: `PR_SET_PDEATHSIG` on Linux, a Job object on
Windows; macOS isn't tied. macOS only needs to compile.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| Forwards: ports, addresses, target hosts | Memory | Run and list them | The user's own device; no one else processes it | Until deleted or Tern quits |

Nothing is written to disk, and nothing is sent beyond the SSH
connections the user starts. Trust boundaries:

- **New: listening sockets [load-bearing].** A local or SOCKS forward lets
  whatever reaches its address use the user's SSH access. `127.0.0.1` by
  default limits that to this machine's processes. `0.0.0.0` only ever
  comes from the user checking "Let other machines connect", after flow 3's
  warning; Tern never adds `-g` or `GatewayPorts`.
- A remote forward opens a port on the server that reaches this machine;
  the server's `GatewayPorts` decides who can use it.
- §Security 3: a forward's prompts go through askpass with a token of
  its own, as the SFTP connection's do (threat model 0001).
- `ssh`'s stderr is shown as plain text only, never interpreted.
- Killing Tern (`kill -9`, Task Manager) ends its forwards on Linux
  and Windows (TM-fwd-13).
- Ground rule 1: no forward starts without the user's Add or Retry.
- STRIDE threat model: [0004](threat-model/0004-port-forwarding.md).
  Its residual risks (other OS users on loopback, the other loopback
  family, other users on the server, no indicator for forwards whose
  tabs are closed) await sign-off in OQ-034.

## Acceptance criteria

1. In a pane connected to `bastion`, adding Local `5432` → `db.internal:5432` starts one `ssh` with exactly `-N -o ExitOnForwardFailure=yes -L 127.0.0.1:5432:db.internal:5432` before the host, shows it up within 3 s, and `psql -h 127.0.0.1 -p 5432` in a local pane reaches the database.
2. SOCKS `1080` serves `curl --socks5-hostname 127.0.0.1:1080 http://intranet/`, and the port listens on `127.0.0.1` only (`ss -ltn` on Linux, `Get-NetTCPConnection -State Listen` on Windows), also with `GatewayPorts yes` for the host in `~/.ssh/config`.
3. Remote `9000` → `localhost:3000` makes `curl 127.0.0.1:9000` on the server reach the dev server on this machine.
4. Adding a forward on a port another program already listens on shows it failed with `ssh`'s "Address already in use" line, and Retry after that program exits brings it up.
5. Delete closes the port within a second, and no `ssh` of that forward is left running.
6. Closing the SSH tab keeps the forward working; `forwards.stop` lists it, and picking it closes the port. A new tab to the same host shows the forward in its sidebar.
7. Quitting Tern leaves no forward's `ssh` running and nothing about forwards in `config.yaml` or `session.json`; the relaunch listens on no forwarded port. Killing Tern instead (`kill -9`; on Windows, End task) leaves no forward's `ssh` running either.
8. A host with a remembered password starts a forward without typing; one without asks in the dialog over the pane, and Esc there marks the forward failed with "cancelled".
9. Only checking "Let other machines connect" makes a forward listen on `0.0.0.0`, and it shows the network warning before Add; a port of `70000` or `-oProxyCommand=x` as the `to` host keeps Add disabled.
10. On Windows 11, criteria 1, 2 and 5 hold with the built-in `ssh.exe`, and a loopback forward raises no firewall prompt.

## Open questions

- None. OQ-033 is resolved by the owner's choices above.
