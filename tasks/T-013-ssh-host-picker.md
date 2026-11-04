---
status: now
blocked-by: []
---
# T-013: Host picker

Hosts read live from `~/.ssh/config` (following `Include`, skipping
wildcard patterns) merged with `type: ssh` profiles, shown as a palette
mode with groups and filtering on `Ctrl+Shift+H`; Enter connects in a new
tab; unreadable config files become notices. Profiles' `group` is read
here (T-012 accepts it but ignores it). Area: src.

Spec: ../spec/0006-ssh-hosts.md
