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

## Result (2026-11-04, Fedora 45, Wayland/COSMIC, Intel Arc B390)

`zed-term --bench-startup 10`, release build:

| Mark | Cold | Warm median | Warm p90 |
|---|---|---|---|
| config | 0.1 | 0.1 | 0.1 |
| pty | 1.8 | 1.8 | 2.1 |
| platform | 244.7 | 26.1 | 28.8 |
| window | 530.8 | 191.7 | 214.6 |
| first_byte | 2.8 | 2.8 | 3.1 |
| rendered | 749.0 | **329.8** | 355.8 |

Under the 500 ms budget, and the same as the bare GPUI spike: the terminal,
`gpui-component` and the embedded font add nothing measurable. GPU device
setup (`window`) and GPUI's first frame are the cost.

Verified on Linux: the login shell starts in the home directory with the
spec's environment and dies with the app; `exit 0` closes the app,
`exit 3` keeps it open; a missing shell shows its error and still reports
TTFP; a broken config boots with defaults; no Wayland session exits with
code 1. 58 tests (written by a separate agent against the specs) cover key
mapping, colors, grid layout, PTY spawn and exit, the bench and the Wayland
check. Not verified: the rendered output by eye (no screenshot, to keep the
desktop private) and Windows, which is T-011.

Fixed on the way: AltGr characters on Windows layouts would have been sent
as control sequences, and `font.size: 0` would have sized the grid at
billions of cells.
