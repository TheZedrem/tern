---
status: resolved
blocked-by: []
---
# T-076: STRIDE threat model for port forwarding

`spec/threat-model/0004-port-forwarding.md`, in the format of 0001–0003:
the listening sockets a forward opens on this machine and on the server,
who can reach them, bind addresses, a dynamic forward as a proxy, and
what the sidebar lets through, the processes outliving their pane. Its mitigations go into spec
0017 before T-075 builds it. Area: meta.

Spec: ../spec/0017-port-forwarding.md
