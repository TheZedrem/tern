---
status: next
blocked-by: [T-076]
---
# T-075: Port forwards on SSH profiles

An SSH profile's `forwards:` list (local, remote, dynamic) opens with its
terminal connection as `-L`/`-R`/`-D` arguments, never on the SFTP or
files pane connection; a notice on connect names them; bad entries are
skipped with a notice. The profile form gets the forwards rows with the
non-loopback warning, spec 0009's flow 4 is amended to match, and
`src/default-config.yaml` gets the template lines (spec 0009, the
owner's standing rule). Builds OQ-033's interim choice (a). Area: src.

Spec: ../spec/0017-port-forwarding.md
