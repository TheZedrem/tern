---
status: now
blocked-by: []
---
# T-094: Check the file browser's filter, trash, permissions and drag out on Linux

T-037, T-038, T-044, T-048 and T-027 were built and checked on Windows 11
(2026-11-16), where no Linux toolchain was at hand, so their Linux code was
never compiled. On Linux (Wayland):

- `cargo clippy --all-targets -- -D warnings` and `cargo test` are clean,
  including the `#[cfg(unix)]` code in `src/files/permissions.rs` (names
  through `libc`), `src/files/local.rs` (`chmod`/`chown`, the trash's
  error text) and its Unix-only test; run the ignored real-trash test once
  by hand (`cargo test -- --ignored a_file_a_full_folder_and_a_link`).
- Spec 0013 AC14, AC15 and AC17 in the running app: a local delete lands in
  the freedesktop trash (a link's target stays); Ctrl+P on a Local file and
  on an SFTP file shows mode, owner and group by name, `755` and Enter
  applies, another user's file shows the refusal; dragging a Local file and
  an edit copy out to the file manager copies them.

Area: src.

Spec: ../spec/0013-files-panel.md
