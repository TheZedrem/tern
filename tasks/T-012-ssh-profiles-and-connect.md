---
status: next
blocked-by: []
---
# T-012: SSH profiles and connections

`type: ssh` profiles open as tabs running the system `ssh` with only the
options the profile sets; `ssh -G` resolves the target at connect time;
exit 255 keeps the tab open and Enter reconnects; restored SSH tabs wait
for Enter instead of connecting at boot; a missing `ssh` is explained in
the pane. No askpass yet: ssh asks in the terminal as usual. Area: src.

Spec: ../spec/0006-ssh-hosts.md
