---
status: next
blocked-by: []
---
# T-052: Check the MSI on Windows 11

Spec 0012 AC2–9 on a Windows 11 machine, with the first release MSI.
These checks need an install, so CI and T-039 didn't cover them:
- No UAC prompt, and a "Tern" Start menu shortcut with the icon.
- No console window.
- `--bench-startup 3 | Out-Host` prints.
- It starts without the VC++ redistributable (e.g. Windows Sandbox).
- N+1 upgrades N with config kept, and N over N+1 is refused.
- Uninstall leaves `%APPDATA%\tern\`.
- A pinned icon groups with the window.
- Windows 10 is refused.

Also type in a `cmd`, PowerShell and `ssh` pane of the GUI-subsystem
build (T-040).

Installing it replaces the Start menu shortcut `install.ps1` writes
(same name), and uninstalling removes it, so run `install.ps1` again
afterwards. Area: meta.

Spec: ../spec/0012-windows-installer.md
