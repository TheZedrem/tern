---
status: now
blocked-by: []
---
# T-010: GPUI scaffold and boot to a measured prompt

Replace the removed Tauri app with the stack from `spec/architecture.md`:
`gpui-pre` plus `gpui-pre-platform` with only its `wayland` feature, the
Wayland check before GPUI starts, Lilex embedded and registered as the
default font, `gpui-component` initialized with its `Root` as the window's
root view and the theme written into its colors, one pane driven by
`alacritty_terminal` (its `tty` +
`event_loop`), a terminal element that shapes the visible rows, key input
to the PTY, resize, exit handling, and the config read on the boot path.
Ship the TTFP machinery: dev badge, stdout marks (six, including
`platform`), `--bench-startup [N]` self-spawning bench. Record the first
TTFP numbers (Linux) in the task summary against the 500 ms budget. The
throwaway spike on `spike/gpui-ttfp` measured 333 ms warm (811 ms cold)
with a bare GPUI window. Area: src.

Keep `config.rs`, `theme.rs` and `timing.rs` from the Tauri version; they
were never webview-specific.

Spec: ../spec/0001-startup-performance.md
Spec: ../spec/0002-terminal-pane.md (spawn, rendering, input, resize, exit only)
