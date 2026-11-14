---
status: draft
date: 2026-11-11
author: Claude (T-076), for review by the owner
related: [spec 0017, spec 0006, spec 0008, threat model 0001, threat model 0003, architecture §Security 3, T-075, OQ-034]
---

# Threat model: port forwarding

## 1. Component overview

How spec 0017 runs local (`-L`), remote (`-R`) and dynamic (`-D`, SOCKS)
forwards. Each forward is its own `ssh … -N -o ExitOnForwardFailure=yes`
child of Tern, built from the SSH pane's arguments plus the fields the
user typed in the sidebar's add row. It has no terminal: stdout and stderr
are pipes, and prompts go through askpass. The registry holds every
forward in memory until the user deletes it or Tern quits. Forwards
keep running after their tabs close.

```
 TB5: user ── add row (kind, listen, to), Delete, Retry, forwards.stop
     │
     ▼
 UI thread: sidebar, registry ──spawn, Child handle──► ssh -N (one per forward)
     ▲    (state, last stderr lines)  ◄──stderr pipe (TB2)──┘   │  ▲ askpass, own token
     │                                                         │  │ (threat model 0001)
     │                                                         │  │
 TB3: local machine                                            │  TB1: network
 any local process, any OS user ──► listening socket ◄─────────┤  (encrypted SSH)
 (127.0.0.1, or 0.0.0.0 with           -L, -D on this machine   │
  "Let other machines connect")                                        ▼
 TB4: LAN ──(only on a non-loopback listen address)──► same socket
                                                     server (sshd)
                                     -R: listening socket on the server ◄── server's
                                         users and, with its GatewayPorts,
                                         its network (TB6)
                                     -L, -D: server dials `to` / SOCKS
                                         targets in its network
```

Trust boundaries:

- **TB1** this machine ↔ server: the SSH connection. A hostile server or
  a man in the middle is in scope.
- **TB2** ssh ↔ Tern: ssh's stderr (and a server banner in it) and its
  exit; Tern's arguments into ssh.
- **TB3** this machine's other processes and OS users ↔ a listening
  socket of a local or dynamic forward. Loopback is shared by every user
  of the machine, not only by the user's own processes.
- **TB4** other machines on the LAN ↔ that socket, when the user checked
  "Let other machines connect".
- **TB5** user ↔ sidebar and palette: which host a forward goes to, what
  it listens on, whether it is still running.
- **TB6** the server's users and network ↔ a remote forward's socket,
  which leads back into this machine.

Out of scope: the SSH protocol and OpenSSH itself, host keys and
credentials (threat model 0001), the user's own `~/.ssh/config`
(including `GatewayPorts`, `LocalForward` and `ProxyCommand` lines the
user wrote), the traffic between the server and the forward's target
(as plain as the service makes it, the same as with `ssh -L`), the
services the forwards reach, and an attacker running as the user or as
root.

## 2. Data classification

| Data | Category (GDPR) | Special constraints | Storage | Retention |
|---|---|---|---|---|
| Traffic through a forward | Can be anything: Art. 4(1) personal data, Art. 9 special categories, database contents, credentials | Private user content: Tern never sees it (ssh carries it), never logs it | None in Tern | None |
| Forward definitions: host, ports, bind and target addresses | Art. 4(1) where they name the user's accounts and infrastructure | Never logged, never saved (spec 0017) | Memory; ssh's command line | Until deleted or Tern quits |
| ssh's last stderr lines | Art. 4(1): names hosts, users, ports | Untrusted text (architecture §Security 1) | Memory | While the forward is listed |
| Askpass token of a forward | Not personal data | Threat model 0001 | Memory, ssh's environment | ssh process lifetime |

No sector law applies: architecture §Ground rules names none, and nothing
leaves the user's device except over the SSH connections the user starts.

## 3. STRIDE walkthrough

Impact and likelihood are H/M/L, judged with spec 0017's design and the
mitigations below in place. Every mitigation not already in spec 0017 is
added to it in the same change, and T-075 builds them all.

| ID | STRIDE | Threat | Affected element | Impact | Likelihood | Mitigation | Status |
|---|---|---|---|---|---|---|---|
| TM-fwd-01 | Information disclosure | A forward the user believes is loopback-only listens on every interface, because `GatewayPorts yes` in `~/.ssh/config` (for that host or `Host *`) makes ssh bind a `-L`/`-D` with no bind address to the wildcard. The LAN reaches the internal database, or an open SOCKS proxy into the server's network. | TB4 | H | M | `Forward::arg()` always writes the bind address: `127.0.0.1` unless "Let other machines connect" is checked (`-L 127.0.0.1:5432:db.internal:5432`, `-D 127.0.0.1:1080`, `-R 127.0.0.1:9000:localhost:3000`). OpenSSH binds an explicit address as given, whatever `GatewayPorts` says. Unit-tested in `src/forwards/`. AC2 checks the bound address. | mitigated (T-075) |
| TM-fwd-02 | Information disclosure | A forward ends up reachable from other machines without the user meaning it to, so they can use the user's SSH access. | TB4 | H | L | There is no free-typed bind address: `0.0.0.0` comes only from the "Let other machines connect" checkbox, which is off each time the add row opens, and while it is checked the row shows flow 3's warning before Add. Such a forward is marked "(network)" in its row and in `forwards.stop`. Tern never adds `-g` or `GatewayPorts`. | mitigated (T-075) |
| TM-fwd-03 | Elevation of privilege | Another OS user on the same machine connects to a loopback forward: they reach the internal database or use the SOCKS proxy with the user's SSH access. Loopback isn't per-user. | TB3 | H | L | Nothing in TCP forwarding can restrict this; OpenSSH has no per-user check on a forwarded port, as with `ssh -L` typed by hand. Unix socket forwards (with file permissions) would close it, but they are *Later* in spec 0017. Single-user desktops, the target audience, have no other OS users. | accepted pending OQ-034 |
| TM-fwd-04 | Spoofing | A local process binds the forward's port first and poses as the service: the user's `psql` sends its password to it. Or it listens on `::1`, which a forward never binds (it binds `127.0.0.1` only), and catches clients that resolve `localhost` to `::1` first. | TB3 | H | L | `ExitOnForwardFailure=yes` makes a forward whose port is taken fail with ssh's "Address already in use" line instead of showing as up (AC4). The `::1` case Tern can't see without probing the port itself; clients given `127.0.0.1` instead of `localhost` avoid it. | port taken: mitigated (T-075); other family: accepted pending OQ-034 |
| TM-fwd-05 | Elevation of privilege | Text typed or pasted into the listen or `to` field injects ssh options: `-oProxyCommand=…` as a host, or spaces and commas that turn one forward into something else. | TB5 → TB2 | H | L | The add row parses its fields into typed values before Add enables: ports as `u16` in 1–65535, the bind address as a bool (the checkbox), the `to` host as an IP (IPv6 in brackets) or a host name of letters, digits, `.`, `-` and `_` that doesn't start with `-`. Everything else keeps Add disabled with the reason. `Forward::arg()` builds the value from those typed fields only, and it goes to ssh as one argument after `-L`/`-R`/`-D`, never through a shell. Unit-tested with hostile inputs. | mitigated (T-075) |
| TM-fwd-06 | Elevation of privilege | A host from an imported or hand-edited profile starts with `-` and injects ssh options into the forward's command. | TB2 | H | L | The forward takes the pane's arguments as the SFTP sidebar does: built by `ssh::command`, whose hosts config parsing and import already drop when they start with `-` (`config/`, `import/mod.rs`); `ssh::forward_args` inserts `-N`, the `-o` and the forward before the host, never from config. | mitigated (existing guard; tested in T-075) |
| TM-fwd-07 | Spoofing | The user adds a forward on a sidebar showing host A while thinking it is host B, and exposes or reaches the wrong network. | TB5 | M | L | The sidebar belongs to the pane it was opened on and its header names `user@host` (TM-sftp-01); every row of the forwards list and every `forwards.stop` entry starts with that label. | mitigated (T-075) |
| TM-fwd-08 | Spoofing | A forward's password or host-key dialog looks like the pane's own, so the user answers a question they didn't mean to (a new host key for what they think is a reconnect). | TB1 → TB5 | M | L | Flow 4: the dialog says it is for the forward and shows its summary and host; host-key questions are never auto-answered (TM-askpass-07); each forward has its own askpass token, and auto-fill matches only its own connection's prompts (TM-askpass-04, -19). | mitigated (T-075, threat model 0001) |
| TM-fwd-09 | Spoofing | A server banner or crafted stderr text, shown as a failure reason, imitates a Tern message or tells the user to run something. | TB2 → TB5 | M | L | The reason is ssh's last three stderr lines, joined, as plain text by spec 0003's title rules (control, direction and invisible characters stripped), at most 200 characters, never clickable or run. | mitigated (T-075) |
| TM-fwd-10 | Tampering | A forward shows "up" while ssh is still waiting at a prompt or still connecting, so the user's client gets refused or reaches a squatter (TM-fwd-04) in that window. | TB5 | L | M | The 2 s up clock runs only while no prompt of that forward is open, and restarts when one is answered. A connection slower than that can still show up early; `ExitOnForwardFailure` then turns a failed bind into failed. | mitigated (T-075); slow-link window accepted pending OQ-034 |
| TM-fwd-11 | Elevation of privilege | Through a remote forward, any user on the server (or its network, if the server's `GatewayPorts` allows) reaches the service on this machine, which is often an unauthenticated dev server, or a LAN host the user typed as `to`. | TB6 | M | M | `-R` gets `127.0.0.1` as its bind address unless "Let other machines connect" is checked (TM-fwd-01), and that box shows flow 3's warning, worded for the server ("Reachable from other machines on `<host>`'s network, if the server allows it"). Who on the server can reach its loopback is the server's business, as with `ssh -R`. | mitigated (T-075); other server users accepted pending OQ-034 |
| TM-fwd-12 | Repudiation | A forward runs unnoticed after its tabs close, so a port or SOCKS proxy stays open long after the user stopped needing it, and they can't tell what is open. | TB5 | M | M | Flow 7: `forwards.stop` lists every running forward; any new pane of that host shows them; quitting stops them all. Nothing in the window shows them while no pane of the host is open. There is no log, by design: nothing about forwards is written. | partly mitigated (T-075); the missing indicator pending OQ-034 |
| TM-fwd-13 | Denial of service | Forwards outlive Tern: after a crash, `SIGKILL` or a forced end of `tern.exe`, their ssh processes keep ports and proxies open with no UI left to stop them. `-N` reads no stdin, so a closed pipe doesn't end it. | TB3, TB4 | M | M | Each forward's ssh is tied to Tern's life by the OS. Linux: `prctl(PR_SET_PDEATHSIG, SIGTERM)` in the child's `pre_exec`, spawned from a thread that lives as long as the app, since the signal fires when the spawning thread exits. Windows: one Job object per app run with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, every forward assigned to it at spawn. macOS: compile only, not tied. In `src/forwards/process.rs`; the job in `src/job.rs`, shared with the plugin host. `libc` is a direct dependency on Unix since OQ-035. | mitigated (T-077) |
| TM-fwd-14 | Tampering | Delete or `forwards.stop` kills a PID that a different process has taken over since the forward's ssh exited, and ends an unrelated program. | Local processes | M | L | Tern keeps each forward's `Child` handle, waits on it from the forward's thread, and stops it with `Child::kill` and `wait` while the handle is held, so the PID can't be reused under it (an unreaped child keeps its PID on Unix; the handle keeps the process object on Windows). Never a kill by name (AGENTS.md). | mitigated (T-075) |
| TM-fwd-15 | Denial of service | A hostile server sends an endless banner or stderr stream, or a huge line: ssh blocks on a full pipe, or Tern holds it all in memory. | TB2 | L | L | Each forward's thread reads stderr to the end as it arrives and keeps only its last KiB; stdout goes to null. Nothing on the UI thread reads a pipe. | mitigated (T-075) |
| TM-fwd-16 | Denial of service | Many forwards, or retries in a loop, spawn ssh processes or connect storms against the server. | TB1 | L | L | Every forward starts only on the user's Add or Retry (ground rule 1); nothing reconnects by itself (flow 5). | mitigated by design |
| TM-fwd-17 | Information disclosure | Forward definitions or ssh's messages end up in a log, `config.yaml`, `session.json` or a crash report. | All | M | L | The forwards module logs nothing about forwards; reasons are shown, never written; nothing is saved (AC7); Tern has no crash reporter. The forward's command line, like the pane's, is visible to other local users in the process list: host names and ports, never secrets (TM-askpass-12). | mitigated (T-075); process list accepted as for every ssh |

Every boundary of the sketch appears: TB1 in -08, -16; TB2 in -05, -06,
-09, -15; TB3 in -03, -04, -13; TB4 in -01, -02, -13; TB5 in -02, -05,
-07, -08, -09, -10, -12; TB6 in -11.

## 4. Mitigation tasks

- [x] T-075: builds TM-fwd-01, -02, -04, -05 to -12, -14, -15, -17 with
  the feature
- [x] T-077: TM-fwd-13, with `libc`, which OQ-035 made a direct
  dependency

## 5. Accepted risks

Pending the owner's sign-off in [OQ-034](../open-questions.md):

- TM-fwd-03: other OS users of the machine can use a loopback forward.
- TM-fwd-04 (`::1`): forwards bind `127.0.0.1` only, so a process on
  `::1` can catch clients that resolve `localhost` to it first.
- TM-fwd-10: on a slow link a forward can show up before its port is
  bound.
- TM-fwd-11: users on the server can reach a remote forward's port.
- TM-fwd-12: no indicator in the window for forwards whose tabs are
  closed.
