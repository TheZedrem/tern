---
status: resolved
blocked-by: []
---
# T-078: Add a port forward from the palette: `pf 13305:13305`

With an SSH pane focused, typing `pf 13305:13305` in the command palette
offers "Add port forward 13305 → localhost:13305" as its first entry, and
Enter starts it like the sidebar's Add. ssh's syntax without a bind
address: `-L` by default, `pf -R 9000:3000`, `pf -D 1080`; always
`127.0.0.1`, never the network. Invalid input shows the entry dimmed
with flow 2's reason. Reuses `Forward::parse`'s checks (TM-fwd-05).
Area: src.

Spec: ../spec/0017-port-forwarding.md (flow 10, AC11)
