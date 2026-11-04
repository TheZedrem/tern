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

## Result (2026-11-04)

TTFP warm median 339.8 ms (p90 352.9, cold 443.8), budget 500 ms; nothing
was added to the boot path (the host list is read when the picker opens).

Built: `src/ssh/config.rs` reads the `Host` aliases of `~/.ssh/config`
the way ssh does (case-insensitive keywords, `=`, quotes, trailing `#`
comments, `Include` with `~`, relative paths and globs, 16 levels, bytes
that aren't UTF-8), skipping wildcard, negated and `-` patterns;
`ssh::hosts` puts the SSH profiles (with their `group`, default "Hosts")
before the aliases no profile already points at (group "SSH config", ID
`ssh:<alias>`, which `Config::profile` resolves to plain `ssh <alias>`);
the host picker on `Ctrl+Shift+H` ("Connect to host…" in the palette) is
the palette with groups, filtered by label, destination and group, the
highlighted host's destination underneath, and Enter connects in a new
tab.

Verified: 266 tests, the new ones written by a separate agent against
spec 0006 (AC1 with a real Include, AC2 at the matcher). Not verified: the
picker in the running window (no input automation here), and Windows
(added to T-011).
