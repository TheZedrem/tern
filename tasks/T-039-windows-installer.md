---
status: later
---
# T-039: Windows MSI installer

A Windows 11 user downloads `tern-<version>-x64.msi` from a GitHub release,
installs Tern with a Start menu entry, and a newer MSI upgrades it in place.
Area: meta.

- `release.yml` gets a `windows` job next to `linux`, also `needs: checks`,
  that builds the MSI on `windows-2025` and attaches it to the same release.
- Needs a spec first (`write-spec`): the tool (WiX via `cargo-wix` or
  similar), a fixed upgrade code, per-user vs per-machine install, and
  whether unsigned installers are acceptable (SmartScreen warns) or code
  signing is needed. Add the choice to `spec/architecture.md` §Stack.

Spec: none yet
