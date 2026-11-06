---
status: later
blocked-by: [T-040, OQ-024]
---
# T-039: Windows MSI installer

A Windows 11 user downloads `tern-<version>-x64.msi` from a GitHub release,
installs it without admin rights, and finds Tern in the Start menu; a newer
MSI upgrades it in place. Area: meta.

- `packaging/windows/tern.wxs`, built with WiX v7 (spec 0012 §Model).
- A `windows` job in `release.yml`, `needs: [checks, linux]`, that uploads
  the MSI to the release `linux` created.
- Once spec 0012 is approved: add a "Windows packages" row to
  `spec/architecture.md` §Stack.

Spec: ../spec/0012-windows-installer.md
