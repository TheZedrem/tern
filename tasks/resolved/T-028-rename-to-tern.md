---
status: now
blocked-by: []
---
# T-028: Rename zed-term to Tern

The app is called Tern, with the app ID `de.zedrem.tern` (OQ-006, OQ-021),
before the first package ships. Area: src / meta.

Done (2026-11-05): crate and binary `tern`, Wayland `app_id`
`de.zedrem.tern`, window title and messages say Tern, config and data in
`<config_dir>/tern/` and `<data_local_dir>/tern/`, keychain service `tern`,
`TERN_*` variables, `TERM_PROGRAM=tern`; code, tests, `AGENTS.md`, docs and
the clean specs renamed. Resolved tasks, OQ answers and spikes keep the old
name as history.

No migration code: nothing was released, so the only existing `zed-term`
folders are the owner's, moved by hand once. Passwords saved under the old
keychain service are asked for again and re-saved on the next connect.

Left for the owner: rename the GitHub repo, and move the old folders once
(`~/.config/zed-term` → `~/.config/tern`, `~/.local/share/zed-term` →
`~/.local/share/tern`) after closing any running zed-term build.

Spec: ../spec/architecture.md
