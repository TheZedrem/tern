---
status: next
blocked-by: [T-004]
---
# T-005: YAML config, profiles, themes and hot reload

`config.yaml` with per-key fallback and notices, `local` profiles with
`default_profile` and palette entries, built-in, file and inline themes
(partial themes via `base`) resolved at load, bundled default font with
`font.family` lookup via `font-kit`, "Open config file", watcher after the
first frame, palette theme picker with live preview and a
comment-preserving write. Area: src.

Spec: ../spec/0005-config-and-themes.md
