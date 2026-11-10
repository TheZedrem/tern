---
status: next
blocked-by: [T-059]
---
# T-060: PuTTY and WinSCP cards on the Import tab

Settings → Import gets a PuTTY and a WinSCP card after Tabby. On Windows
they read the apps' sessions straight from the registry (`HKCU` only, off
the UI thread) into the same session list T-059's parsers take, so a
click equals a CLI import of a `reg export`; on Linux the PuTTY card reads
`~/.putty/sessions` and the WinSCP card is "Choose file…" only. Needs
OQ-029 answered (how the registry is read). Check on Windows 11. Area: src.

Spec: ../spec/0015-import-putty-winscp.md
Spec: ../spec/0014-import-tab.md
