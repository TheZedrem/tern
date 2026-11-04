---
status: now
blocked-by: []
---
# T-012: SSH profiles and connections

`type: ssh` profiles open as tabs running the system `ssh` with only the
options the profile sets; exit 255 keeps the tab open and Enter
reconnects; SSH tabs at boot (and, with T-006, restored ones) wait for
Enter instead of connecting; a missing `ssh` is explained in the pane. No
askpass yet: ssh asks in the terminal as usual. Resolving the target with
`ssh -G` moved to T-014, the first task that needs it. Area: src.

Spec: ../spec/0006-ssh-hosts.md

## Result (2026-11-04)

TTFP warm median 327.2 ms (p90 340.6, cold 368.9), budget 500 ms; T-004
measured 330.6 ms.

Built: profiles are typed per `type` (`ProfileKind::Local` or `Ssh`); an
ssh profile needs a `host` (none starting with `-`), takes `user`, a port
of 1–65535 and `identity_file`, and runs exactly `ssh [-l] [-p] [-i]
<host>`; keys of the other type get a notice; `src/ssh/` builds the
command and explains a missing OpenSSH client; ssh's exit 255 keeps the
pane open with `[connection closed: press Enter to reconnect]`, and Enter
reruns ssh on the same terminal, keeping the scrollback; an SSH tab at
boot shows `Press Enter to connect to <label>` and starts nothing until
then (no network at startup), the start T-006 will use for restored tabs.

Verified: 224 tests, the new ones written by a separate agent against
spec 0006; a live boot with an SSH `default_profile` drew its first frame
and started no ssh process. Not verified: a real SSH session, pressing
Enter to connect or reconnect (no input automation here), and Windows
(added to T-011).

Known limitation: a key of the other type whose value doesn't fit that
key's type (`port: 70000` on a local profile) skips the profile with a
notice instead of being ignored; fixing it needs a hand-written profile
parser.
