---
status: later
---
# T-018: Import sessions from MobaXterm and Tabby

Turn an existing MobaXterm session export (`.mxtsessions` /
`MobaXterm.ini`) or Tabby `config.yaml` into zed-term profiles in
`config.yaml`, keeping folders as host groups, via
`zed-term --import-hosts <file> [--dry-run]` (no in-app UI). This is a
one-off import the user starts, not a sync. Hosts and settings only:
stored passwords and vault contents stay behind, because they would bypass
the keychain flow in spec 0006. Spec 0010 pins it (approved 2026-11-05).
Before building, get real exports from the owner (a
`MobaXterm.ini` and a Tabby `config.yaml`, passwords removed) to test the
parsers against. Area: src.

Spec: ../spec/0010-import-hosts.md
Spec: ../spec/0006-ssh-hosts.md
