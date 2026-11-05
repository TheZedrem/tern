---
status: next
blocked-by: []
---
# T-028: Rename zed-term to Tern

The app is called Tern, with the app ID `de.zedrem.tern` (OQ-006, OQ-021),
before the first package ships. Area: src / meta.

- Crate and binary `tern`; Wayland `app_id` `de.zedrem.tern`; window title
  and UI text say Tern.
- Config and data move to `<config_dir>/tern/` and `<data_local_dir>/tern/`.
  An existing `zed-term` folder is moved over once, when no `tern` folder
  exists yet, so the owner's config, themes and session survive. This
  touches the boot path: report the TTFP.
- Secrets saved in the keychain under the old service name are still found
  (or moved), so no saved SSH password is lost.
- Specs, docs, `AGENTS.md`, CI and `src/default-config.yaml` say Tern; the
  historical records (resolved OQs, resolved tasks, spikes) stay as written.
- Renaming the GitHub repo is the owner's step.

Spec: ../spec/architecture.md
