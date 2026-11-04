---
status: next
blocked-by: []
---
# T-011: Verify the scaffold on Windows 11

T-010 was built and verified on Linux (Wayland) only; its Windows code
paths compile only in theory. On a Windows 11 machine: build release, run
`zed-term --bench-startup 10` and record the numbers, then check by hand
that the default shell is found (pwsh, powershell, cmd), typing works
including AltGr characters on a German layout (`@`, `{`, `€`), resizing the
window with `vim` open redraws cleanly under ConPTY, and `exit 0` closes
while `exit 3` stays open. Fix what fails. Area: src.

Spec: ../spec/0001-startup-performance.md (AC8)
Spec: ../spec/0002-terminal-pane.md (spawn, input, resize, exit on Windows)
