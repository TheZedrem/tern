---
status: next
blocked-by: []
---
# T-075: Port forwards in the SSH pane's sidebar

The SSH pane's sidebar switches between Files and Port forwards. There
the user adds a local, remote or SOCKS forward in one row, sees it start,
up or failed with `ssh`'s reason, and deletes or retries it. Each runs
its own `ssh -N -o ExitOnForwardFailure=yes` until deleted or Tern
quits, never saved; `forwards.stop` reaches it once its tabs are closed.
New module `src/forwards/`; spec 0008 is amended for the switch, and the
two new commands show on the settings screen's Keyboard tab. Builds
threat model 0004's mitigations; TM-fwd-13 (forwards ending with Tern)
waits for OQ-034's answer on `libc`. Area: src.

Spec: ../spec/0017-port-forwarding.md
