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

## Result (2026-11-04)

TTFP warm median 320.8 ms (p90 341.5, cold 375.5), budget 500 ms; T-003
measured 340.8 ms.

Built: `config.yaml` where an unknown key is a notice and a value of the
wrong type resets its top-level key, profile or inline theme on its own,
each notice naming the line; `local` profiles (command, cwd, env, theme)
that the first tab and new tabs open through `default_profile`, while
splits reopen the source pane's profile in its directory; the built-in
`dark` plus a new `light`, theme files and inline themes, partial themes
via `base`, and `system` following the OS after the first frame; the
installed-font check with Lilex as the fallback; the `OpenConfig` action,
which gets its palette entry with T-004; and hot reload through `notify`,
debounced at 200 ms, that also follows a `config.yaml` or theme file
linked into a dotfiles repo.

Verified: 150 tests, written by a separate agent against spec 0005: every
in-scope criterion, the watcher on real files (writes, renames, links, a
config directory created later), and the spec's example config. A live
run on a throwaway `XDG_CONFIG_HOME` resized the running pane's grid
within 1.5 s of a font-size edit. Not verified: how themes and notices
look in the window, and Windows (added to T-011).

Diverged from the spec, which now says so: the built-in themes are Rust
constants rather than embedded YAML, and a wrong type inside `font` or
`theme` resets that whole key. The keys `cursor`, `copy_on_select`,
`keybindings` and `restore_session` are accepted but read only by T-002,
T-004 and T-006.

Fixed on the way: what waits for the first frame (boot notices, now also
the reload watcher) never started for a program that prints nothing; it
now starts after a second at the latest.
