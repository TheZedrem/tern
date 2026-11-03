# Open questions

Unresolved, load-bearing unknowns. Agents append here (next free OQ-nnn, never
reuse an ID) when a spec is silent on something that matters; humans resolve.
When resolved: record the decision inline, mark ✅, and update the spec or
`architecture.md` that was blocked.

| ID | Status | Question | Context (task/spec) | Resolution |
|---|---|---|---|---|
| OQ-001 | ✅ resolved | How do we implement SFTP, given that SSH goes through the system `ssh` binary? Options: (a) spawn `ssh -s <host> sftp` and run the `russh-sftp` client over its stdio, reusing the user's ssh config, agent and known_hosts; (b) a full Rust SSH stack (`russh`) just for SFTP, with separate auth and known_hosts handling; (c) drive the `sftp` CLI and parse its output (fragile). | Phase 2; [architecture](architecture.md#key-decisions-and-their-costs) | (a): `ssh -s <host> sftp` with the `russh-sftp` client over its stdio. |
| OQ-002 | ✅ resolved | How far does macOS support go: does CI build it, and do we sign or notarize? | `architecture.md` §Platforms | Compile only: keep the code building on macOS; no CI builds, no signing. |
| OQ-003 | ✅ resolved | Default shell on Windows when the config doesn't set one: `pwsh` if installed, then `powershell.exe`, then `cmd.exe`? | [spec 0002](0002-terminal-pane.md) | Yes: `pwsh` → `powershell.exe` → `cmd.exe`. |
| OQ-004 | ✅ resolved | Which CI system and git host? Minimum pipeline: `cargo test`, frontend typecheck/lint, and builds for Linux and Windows. Should a TTFP benchmark fail the build on regression? | `architecture.md` §Stack | GitHub + GitHub Actions (Linux and Windows runners). TTFP is not gated in CI, because shared runners are too noisy; the bench stays local. |
| OQ-005 | ✅ resolved | Reference machine for the 300 ms TTFP budget, and does it apply to warm starts only or cold starts too? Cold-starting WebKitGTK alone may take 150–250 ms. | [spec 0001](0001-startup-performance.md) | Warm start, median, on the owner's Fedora machine. Cold start is measured and reported, with no hard limit. |
| OQ-006 | ✅ resolved | The name "zed-term" might be confused with the Zed editor. Keep it or rename before the first public release? | — | Keep for now; revisit before a public release. |
| OQ-007 | ✅ resolved | How do we detect the OS light/dark setting for `theme.default: system` without slowing startup? | [spec 0005](0005-config-and-themes.md) | Not on the boot path: the terminal starts `dark`, and the system theme is resolved after startup (at idle), then applied live. |
