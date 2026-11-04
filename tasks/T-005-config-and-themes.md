---
status: now
blocked-by: []
---
# T-005: YAML config, profiles, themes and hot reload

`config.yaml` with per-key fallback and notices, `local` profiles with
`default_profile` (new tabs open it), built-in, file and inline themes
(partial themes via `base`) resolved at load, `font.family` by exact
installed name with Lilex as the fallback, "Open config file", and the
watcher after the first frame. The palette parts of spec 0005 (profile
entries, theme picker) land with T-004. Area: src.

Spec: ../spec/0005-config-and-themes.md
