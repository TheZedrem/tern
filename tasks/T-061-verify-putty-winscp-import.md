---
status: next
blocked-by: []
---
# T-061: Check the PuTTY and WinSCP import on Linux and at scale

Spec 0015 was built and checked on Windows 11 only (registry cards, real
`reg export` files, the user inputs on the card). Still to check:
AC8 on a Wayland session (a real `~/.putty/sessions` directory found by
the PuTTY card, the WinSCP card "Choose file…" only), AC9 in a Linux
terminal (the user prompt, and nothing asked with stdin piped), `cargo
clippy` on Linux (the registry code is `cfg(windows)`), and AC10 (5,000
PuTTY sessions keep the Import tab responsive). Area: src.

Spec: ../spec/0015-import-putty-winscp.md
