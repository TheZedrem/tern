---
status: now
blocked-by: []
---
# T-001: Scaffold the app and boot to a measured prompt

**Resolved as superseded (2026-11-04):** the Tauri + WebKitGTK app this
task built could not get under 1 s on the reference machine (numbers
below). OQ-010 chose a native rewrite; the Tauri code was removed in the
same change and T-010 restarts the scaffold on the native stack. The
measurements here are the reason, so they stay.

Scaffold Tauri 2 + SolidJS + Vite + Tailwind (pnpm), spawn the boot pane's
shell before the webview loads, stream it over a binary Channel into xterm
(WebGL with DOM fallback), with correct fit/resize. Ship the TTFP machinery:
dev badge, stdout marks, `--bench-startup`, `pnpm bench:startup`, and the
entry-chunk budget check. Record the first real TTFP numbers (Linux) in the
task summary: the warm median against the budget, with the cold
run reported separately. Area: src-tauri, src.

Spec: ../spec/0001-startup-performance.md
Spec: ../spec/0002-terminal-pane.md (spawn, streaming, resize, renderer, exit only)

## First TTFP numbers (2026-11-03, Fedora 45, Wayland/COSMIC, WebKitGTK 2.54)

Warm median over 10 bench runs: **1230 ms** (p90 1315, cold 1341). Over the
original 300 ms budget by 4×; OQ-009 raised it to 1 s. Breakdown of one warm
run, ms since `main()`:

| Phase | ms |
|---|---|
| config read + PTY spawn | 3 |
| Tauri app build (GTK init) | 125 |
| webview window shown (WebKitGTK) | 460 |
| page JS starts (WebKit web process up) | 800 |
| entry chunk parsed | 890 |
| xterm opened / WebGL / fit | 1005 / 1060 / 1080 |
| first PTY bytes in the page | 1120 |
| first render | 1180 |

~800 ms is platform floor before our first line of JS. Under `GDK_BACKEND=x11`
the window shows at ~255 ms and JS starts at ~520 ms. The WebKit sandbox
makes no difference.

After forcing the built-in Adwaita GTK theme (commit c9cf834): **1066 ms**
warm median. Of the remaining floor, ~150 ms is the owner's
`~/.config/gtk-3.0/settings.ini` loading two KDE GTK modules; with an
isolated config the same binary lands at ~900 ms.

## Native spike (2026-11-04, branch `spike/native-ttfp`)

Question: how fast is a pure-Rust window without a webview on the same
machine? Prototype: winit + softbuffer (CPU raster) + fontdue + vte +
portable-pty, same `--bench-startup` marks.

| Mark | Native | Tauri |
|---|---|---|
| window shown | 45 ms | 476 ms |
| prompt rendered (warm median, n=10) | **68 ms** | **1066 ms** |
| cold run | 64 ms | 1077 ms |

With the owner's real login shell the prompt renders at ~358 ms, of which
~300 ms is the shell's own startup. Verdict: a native UI is ~15× faster to
first prompt and is the only route to a native-terminal feel. Decision in
OQ-010.
