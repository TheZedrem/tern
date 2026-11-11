---
status: later
blocked-by: [T-068, T-069]
---
# T-071: Plugin author guide and example plugins in Python and Rust

`docs/plugins.md` for plugin authors: the manifest, each event and action
with a JSON example, the rights, what gets confirmed, that EOF on stdin
means stop, and that settings must hold no secrets.

Plus two example plugins in the repo, one in Python and one in Rust (owner,
2026-11-11), under `examples/plugins/python/` and `examples/plugins/rust/`,
each a ready plugin folder with its `plugin.yaml`. Both do the same: a
command, a panel with a button, a streamed log and one confirmed
`fill_command`, so the guide can show each step in both languages. The
Python one runs in on-demand mode and needs only the standard library; the
Rust one runs in background mode as its own small crate, outside Tern's
build and dependency tree. Only its source is in the repo, no built binary
(no binaries in git, architecture §Stack): CI's test job builds it, and the
release workflow never touches it. Once they exist, Tern's plugin tests also run against both, so a
protocol change that breaks either fails the build.
Area: meta.

Spec: ../spec/0016-plugin-system.md
