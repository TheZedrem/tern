---
status: next
blocked-by: []
---
# T-089: Put the add-profile buttons above the profile list

On the settings screen's Profiles page, "Add local profile" and "Add SSH
profile" render below the list, so with many profiles they scroll out of
reach. Move them above the first profile group (`profile_buttons` in
`src/settings/pages/profiles.rs`, pushed in `src/settings/mod.rs`).
Requested by the owner (2026-11-13). Area: src.

Spec: ../spec/0009-settings-screen.md
