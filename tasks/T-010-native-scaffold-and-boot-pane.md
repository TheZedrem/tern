---
status: now
blocked-by: []
---
# T-010: Native scaffold and boot to a measured prompt

Replace the removed Tauri app with the native stack from
`spec/architecture.md`: `eframe`/`egui` window, one pane driven by
`alacritty_terminal` (its `tty` + `event_loop`), a grid widget that renders
visible rows with the bundled monospace font, key input to the PTY, resize,
exit handling, and the config read on the boot path. Ship the TTFP
machinery: dev badge, stdout marks, `--bench-startup [N]` self-spawning
bench. Record the first native TTFP numbers (Linux) in the task summary
against the 150 ms budget; the throwaway spike on `spike/native-ttfp`
measured 68 ms with a bare window. Area: src.

Keep `config.rs`, `theme.rs` and `timing.rs` from the Tauri version; they
were never webview-specific.

Spec: ../spec/0001-startup-performance.md
Spec: ../spec/0002-terminal-pane.md (spawn, rendering, input, resize, exit only)
