---
status: draft        # draft | approved | implemented
phase: 1
tasks: [T-010]
related: [OQ-005 ✅, OQ-009 ✅, OQ-010 ✅]
---

# Spec: Startup performance and time-to-first-prompt

Pins the startup budget, the boot path every other feature must stay out
of, and how time-to-first-prompt (TTFP) is measured so regressions surface
during development instead of in reviews.

## Problem & goal

Slow boot is the reason this product exists (Tabby, T3 Code). Speed is a
feature that rots one innocent initialization at a time, so the budget
needs a number, a measurement and a guard.

Done means: a release build reaches a usable prompt in **< 150 ms median
on a warm start** on the owner's Fedora machine, and every dev run shows its
TTFP. Cold starts are measured and reported, with no hard limit. The native
spike (OQ-010) measured 68 ms, so the budget leaves room for a real UI
without inviting rot.

## Scope

**In scope:**

- Boot path rules
- TTFP definition, dev badge, stdout report, `--bench-startup`
- Bench runner built into the binary (no scripts, works on Windows)

**Out of scope:**

- CI gating on TTFP: never, because shared CI runners are too noisy. The bench runs locally.
- Shell startup time (the user's `.zshrc` is not our budget; see Measurement)

## Design

### Affected areas

- `src/main.rs` (boot sequence), `src/timing.rs` (marks, bench)

### Model

**TTFP** = time from `main()` entry to the first frame presented that
contains output from the first pane. First shell output is the practical
proxy for "prompt visible". Time spent before `main()` (exec, dynamic
linking) isn't counted; that's accepted as a small constant.

Marks, all relative to `main()`: `config` (config and session read),
`pty` (active tab's PTYs spawned), `window` (window created), `first_byte`
(first PTY output parsed), `rendered` (first frame with output presented).

### Flows

Boot path, in order. **[load-bearing]** Nothing may be added to it without a
TTFP measurement in the commit.

1. `main()` records the start instant.
2. Read `config.yaml` and `session.json` synchronously (both small). On a
   missing or broken config, use defaults and queue a notice (architecture
   §Key decisions). A missing or broken session means one fresh tab
   (spec 0003). Never write files on the boot path.
3. Spawn the active tab's PTY(s) at the grid size implied by the saved
   window size (default 1000×640 logical px) and the font's cell size. Each
   pane's event loop starts parsing immediately (architecture §Key
   decisions). The shell boots while the window comes up.
4. Create the window with its clear color set to the theme background, so
   there's no white flash. The bundled font is already in memory; no font
   lookup happens unless `font.family` is configured.
5. First frame: render the layout and every visible pane's grid.
6. The first frame that includes PTY output records `rendered` and prints
   the report.
7. Only after step 6, on the next frame, run deferred work: config watcher,
   OS theme detection, prefetching anything lazy.

**Failure paths:** if `wgpu` can't create a device, `eframe` falls back to
the `glow` backend and TTFP is still reported. If the PTY spawn fails,
render the error in the pane and report TTFP at that frame, so the bench
never hangs.

**Bench mode (`--bench-startup`):** the boot pane runs a minimal shell
command (`sh -c 'echo ready'` / `cmd /c echo ready`) instead of the user's
shell, so the number measures zed-term, not dotfiles. It prints one JSON
line with all five marks and exits. `zed-term --bench-startup 10` runs the
release binary ten times (one cold, N warm) by re-spawning itself and prints
the median and p90 per mark; the default N is 10.

### Startup impact

This spec *is* the startup path. The guard is the dev badge on every run
plus the bench; there is no bundle to size.

### Platform notes

Linux: Wayland and X11 via `winit`; `wgpu` on Vulkan. Windows: `wgpu` on
DX12, ConPTY via `alacritty_terminal`. macOS: compile-only.

### Security & privacy

- Stored: nothing. Timings are printed to stdout only, with no paths,
  commands or output.
- Trust boundaries: none crossed.

## Acceptance criteria

1. A dev run (`cargo run`) shows a TTFP badge in a corner of the window and prints the marks to stdout.
2. A release build launched with `--bench-startup` prints one JSON line with all five marks and exits with code 0, with no window interaction.
3. On the owner's Fedora machine, `zed-term --bench-startup 10` reports a warm-start median TTFP under 150 ms. The first (cold) run is reported separately.
4. A broken `config.yaml` (invalid YAML) still boots to a prompt with defaults and shows a notice naming the line.
5. With networking disabled, startup time and behavior are unchanged, and no request is attempted before the first prompt.
6. No white flash: the first painted frame is the theme background.
7. With `wgpu` forced off (env `ZED_TERM_RENDERER=glow`), the app boots on the OpenGL backend and still reports TTFP.
8. The same binary and bench run on Windows and report all five marks.

## Open questions

- None.
