---
status: next
blocked-by: []
---
# T-077: Port forwards end with Tern, crash included

A forward's `ssh -N` keeps its port open after Tern crashes or is killed,
because `-N` reads no stdin and nothing tells it Tern is gone
(TM-fwd-13). Tie each forward's ssh to Tern's life: on Linux
`prctl(PR_SET_PDEATHSIG, SIGTERM)` in `pre_exec`, spawned from a thread
that lives as long as the app; on Windows one Job object per run with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The site is the `TODO(OQ-034)` in
`src/forwards/process.rs`. Waits for OQ-034's answer on `libc` as a
direct dependency. Checked by spec 0017's AC7 (`kill -9`, End task).
Area: src.

Spec: ../spec/0017-port-forwarding.md
