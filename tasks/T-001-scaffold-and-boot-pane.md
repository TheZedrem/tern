---
status: now
blocked-by: []
---
# T-001: Scaffold the app and boot to a measured prompt

Scaffold Tauri 2 + SolidJS + Vite + Tailwind (pnpm), spawn the boot pane's
shell before the webview loads, stream it over a binary Channel into xterm
(WebGL with DOM fallback), with correct fit/resize. Ship the TTFP machinery:
dev badge, stdout marks, `--bench-startup`, `pnpm bench:startup`, and the
entry-chunk budget check. Record the first real TTFP numbers (Linux) in the
task summary: the warm median against the 300 ms budget, with the cold
run reported separately. Area: src-tauri, src.

Spec: ../spec/0001-startup-performance.md
Spec: ../spec/0002-terminal-pane.md (spawn, streaming, resize, renderer, exit only)
