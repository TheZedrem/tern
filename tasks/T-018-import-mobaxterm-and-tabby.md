---
status: later
blocked-by: [T-012]
---
# T-018: Import sessions from MobaXterm and Tabby

Turn an existing MobaXterm session export (`.mxtsessions` /
`MobaXterm.ini`) or Tabby `config.yaml` into zed-term profiles in
`config.yaml`, keeping folders as host groups. This is a one-off import the
user starts, not a sync. Hosts and settings only: stored passwords and
vault contents stay behind, because they would bypass the keychain flow in
spec 0006. It needs its own spec before building. Area: src.

Spec: ../spec/0006-ssh-hosts.md
