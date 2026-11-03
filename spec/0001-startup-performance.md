---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-001]
related: [OQ-005 ✅]
---

# Spec: Startup performance and time-to-first-prompt

Pins the startup budget, the boot path every other feature must stay out
of, and how time-to-first-prompt (TTFP) is measured so regressions surface
during development instead of in reviews.

## Problem & goal

Slow boot is the reason this product exists (Tabby, T3 Code). Speed is a
feature that rots one innocent `import` at a time, so the budget needs a
number, a measurement and a guard.

Done means: a release build reaches a usable prompt in **< 300 ms median
on a warm start** on the owner's Fedora machine, and every dev run shows its
TTFP. Cold starts are measured and reported, with no hard limit.

## Scope

**In scope:**

- Boot path rules (Rust and frontend)
- TTFP definition, dev badge, stdout report, `--bench-startup`, `pnpm bench:startup`
- Entry-bundle size budget enforced at build time

**Out of scope:**

- CI gating on TTFP: never, because shared CI runners are too noisy. The bench runs locally.
- Shell startup time (the user's `.zshrc` is not our budget; see Measurement)

## Design

### Affected areas

- `src-tauri/` (boot sequence, timing), `src/main.tsx` (entry), `vite.config.ts` (budget check), `scripts/`

### Model

**TTFP** = time from Rust `main()` entry to xterm.js having rendered the
first output bytes of the first pane. First shell output is the practical
proxy for "prompt visible". Time spent before `main()` (exec, dynamic
linking) isn't counted; that's accepted as a small constant.

Intermediate marks, all relative to `main()`: `config` (config read),
`pty` (first PTY spawned), `window` (window created), `first_byte` (first
PTY output in Rust), `rendered` (frontend report).

### IPC surface

- `window.__BOOT__`, injected by the window's initialization script: resolved
  config subset, resolved theme palette, the IDs of the panes started at boot,
  and the layout to render. **[load-bearing]** No `invoke` round trip is
  needed before the first paint.
- `report_first_prompt() -> TtfpReport`: called once by the frontend after the
  first render. Rust prints the report, returns it for the badge, and exits
  the app in bench mode.

### Flows

Boot path, in order. **[load-bearing]** Nothing may be added to it without a
TTFP measurement in the commit.

1. `main()` records the start instant.
2. Read `config.yaml` and `session.json` synchronously (both small). On a
   missing or broken config, use defaults and queue a notice (architecture
   §Key decisions). A missing or broken session means one fresh tab
   (spec 0003). Never write files on the boot path.
3. Spawn the active tab's PTY(s) at an estimated size: the saved window size
   from `session.json` (default 1000×640 logical px) split per the saved
   layout, ÷ the font's estimated cell size. Output buffers in Rust until a
   Channel attaches (architecture §Key decisions). The shell boots while the
   webview loads.
4. Create the window with its background set to the theme background, so
   there's no white flash, and inject `__BOOT__`.
5. Frontend entry: render the layout, mount xterm, attach Channels, fit
   (this triggers one PTY resize if the estimate was off).
6. First render of PTY output → `report_first_prompt()`.
7. Only after step 6, when idle (with a `setTimeout` fallback, since WebKit
   lacks `requestIdleCallback`), run deferred work: config watcher,
   web-links/unicode addons, prefetching lazy chunks.

**Failure paths:** if the WebGL renderer fails to initialize, use the DOM
renderer and still report TTFP. If the PTY spawn fails, render the error in
the pane and report TTFP at that render, so the bench never hangs.

**Bench mode (`--bench-startup`):** the boot pane runs a minimal shell
command (`sh -c 'echo ready'` / `cmd /c echo ready`) instead of the user's
shell, so the number measures zed-term, not dotfiles. It prints one JSON
line with all marks and exits. `pnpm bench:startup [N=10]` runs the release
binary N times and prints the median and p90 for each mark.

### Startup impact

This spec *is* the startup path. Entry-chunk budget: **≤ 150 KB gzipped
JS** (Solid, xterm, WebGL/fit addons, layout, pane). `pnpm build` fails if
the entry chunk exceeds it. Raising the budget needs the owner's OK.

### Platform notes

Linux: WebKitGTK is the biggest unknown for the warm budget; measure it in
T-001 before optimizing anything else. Windows: WebView2 is
shared and usually warm. macOS: compile-only.

### Security & privacy

- Stored: nothing. Timings are printed to stdout only, with no paths,
  commands or output.
- `__BOOT__` carries resolved config and themes, never secrets.
- Trust boundaries: none crossed.

## Acceptance criteria

1. A dev run (`pnpm tauri dev`) shows a TTFP badge in a corner of the window and prints the marks to stdout.
2. A release build launched with `--bench-startup` prints one JSON line with all five marks and exits with code 0, with no window interaction.
3. On the owner's Fedora machine, `pnpm bench:startup` reports a warm-start median TTFP under 300 ms. The first (cold) run is reported separately.
4. A broken `config.yaml` (invalid YAML) still boots to a prompt with defaults and shows a notice naming the line.
5. Adding a 200 KB static import to `main.tsx` makes `pnpm build` fail with a budget message.
6. With networking disabled, startup time and behavior are unchanged, and no request is attempted before the first prompt.
7. No white flash: the first painted frame is the theme background.
8. With WebGL unavailable, the app boots on the DOM renderer and still reports TTFP.

## Open questions

- Whether coalescing tiny PTY reads matters for TTFP (expected: no).
