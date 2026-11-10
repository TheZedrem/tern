---
status: next
blocked-by: []
---
# T-059: Import hosts from PuTTY and WinSCP files

`tern --import-hosts` takes PuTTY's and WinSCP's sessions: a `.reg`
export of either app's registry key, `WinSCP.ini`, and PuTTY's
`~/.putty/sessions` directory on Linux. SSH sessions and SFTP/SCP sites
become `type: ssh` profiles (WinSCP folders as groups) through spec
0010's plan and write; `.ppk` keys are dropped with a `puttygen` hint,
passwords never read. Parsers in their own files under `src/import/`
(`mod.rs` is already past 1,000 lines with tests). Before building, get
real exports from the owner (a PuTTY and a WinSCP `.reg`, a `WinSCP.ini`,
passwords removed) to test against. Asked for by the owner (2026-11-10);
waits for spec 0015's approval. Area: src.

Spec: ../spec/0015-import-putty-winscp.md
Spec: ../spec/0010-import-hosts.md
