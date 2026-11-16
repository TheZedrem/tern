---
status: next
blocked-by: []
type: bug
---
# T-095: Tabs show the program's path instead of the profile's name

On Windows every tab reads `C:\Program Files\WindowsApps\…\pwsh.exe` or
`C:\WINDOWS\System32\OpenSSH\ssh.exe`: ConPTY sets the console title to
the started program's path, Tern takes it as the shell's OSC title, and it
wins over the profile's display name. Useless for any tab, and worst for
SSH tabs, which all read the same. Reported by the owner (2026-11-16).

A title that is only a program's path doesn't count, so the tab shows the
profile's name until the shell or the server sets a real title; a saved
title like that restores as the profile's name too. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
