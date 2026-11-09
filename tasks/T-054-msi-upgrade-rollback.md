---
status: next
type: bug
blocked-by: []
---
# T-054: Restore the installed version when an MSI upgrade fails

`packaging/windows/tern.wxs` leaves `MajorUpgrade.Schedule` at its default,
`afterInstallValidate`. The generated MSI runs `RemoveExistingProducts`
at sequence 1401, before `InstallInitialize` at 1500. If the new install
fails or is cancelled after removing the old version, rollback leaves
the user without Tern. Area: meta.

Schedule removal after `InstallInitialize` so it participates in rollback.
On a disposable Windows installation, verify both a successful N-to-N+1
upgrade and a failed upgrade that restores N and its shortcut.

Spec: ../spec/0012-windows-installer.md
