---
status: approved     # draft | approved | implemented
phase: 1
tasks: [T-010, T-011]
related: [OQ-005 ✅, OQ-009 ✅, OQ-010 ✅, OQ-012 ✅, OQ-013 ✅]
---

# Spec: Startup performance and time-to-first-prompt

Pins the startup budget, the boot path every other feature must stay out
of, and how time-to-first-prompt (TTFP) is measured so regressions surface
during development instead of in reviews.

## Problem & goal

Slow boot is the reason this product exists (Tabby, T3 Code). Speed is a
feature that rots one innocent initialization at a time, so the budget
needs a number, a measurement and a guard.

Done means: a release build reaches a usable prompt in **< 500 ms median
on a warm start** on the owner's Fedora machine (Wayland), and every dev run
shows its TTFP. Cold starts are measured and reported, with no hard limit.
The GPUI spike measured 333 ms warm and 811 ms cold there, so the budget
leaves room for a real UI on top of it (OQ-013).

## Scope

**In scope:**

- Boot path rules, including the Wayland check
- TTFP definition, dev badge, stdout report, `--bench-startup`
- Bench runner built into the binary (no scripts, works on Windows)

**Out of scope:**

- CI gating on TTFP: never, because shared CI runners are too noisy. The bench runs locally.
- Shell startup time (the user's `.zshrc` is not our budget; see Measurement)

## Design

### Affected areas

- `src/main.rs` (boot sequence), `src/timing.rs` (marks, bench)

### Model

**TTFP** = time from `main()` entry to the first frame drawn that contains
output from the first pane, taken in GPUI's next-frame callback after that
render. First shell output is the practical proxy for "prompt visible".
Time spent before `main()` (exec, dynamic linking) isn't counted; that's
accepted as a small constant.

Marks, all relative to `main()`: `config` (config and session read), `pty`
(active tab's PTYs spawned), `platform` (GPUI up: Wayland connection and
system fonts), `window` (window and GPU device created), `first_byte`
(first PTY output parsed), `rendered` (first frame with output drawn).

### Flows

Boot path, in order. **[load-bearing]** Nothing may be added to it without a
TTFP measurement in the commit.

1. `main()` records the start instant.
2. Linux: without `WAYLAND_DISPLAY`, print `Tern needs a Wayland session
   (X11 is not supported)` to stderr and exit with code 1. GPUI would
   otherwise start headless and hang with no window.
3. Read `config.yaml` and `session.json` synchronously (both small). On a
   missing or broken config, use defaults and queue a notice (architecture
   §Key decisions). A missing or broken session means one fresh tab
   (spec 0003). Never write files on the boot path.
4. Spawn the active tab's PTY(s) at the grid size implied by the saved
   window size (default 1000×640 logical px) and the default font's cell
   size. Each pane's event loop starts parsing immediately (architecture
   §Key decisions). The shell boots while GPUI comes up.
5. Start GPUI's platform, register the embedded default font and
   initialize `gpui-component`.
6. Open the window with the theme background as its clear color, so
   there's no white flash.
7. The first frame that includes PTY output records `rendered` and prints
   the report.
8. Only after step 7 run deferred work: config watcher, OS theme detection.

**Failure paths:** if the PTY spawn fails, render the error in the pane and
report TTFP at that frame, so the bench never hangs. If GPUI can't create a
window or GPU device, print its error to stderr and exit with code 1; no
software rendering path is promised.

**Bench mode (`--bench-startup`):** the boot pane runs a minimal shell
command (`sh -c 'echo ready'` / `cmd /c echo ready`) instead of the user's
shell, so the number measures Tern, not dotfiles. It prints one JSON
line with all six marks and exits. `tern --bench-startup N` re-spawns
itself once cold, then N times warm, and prints the median and p90 per
mark. The bench never reads or writes the user's session; with
`TERN_BENCH_SESSION=<file>` it restores that one, read-only, with the
same command in every pane (spec 0003, AC7).

### Startup impact

This spec *is* the startup path. The guard is the dev badge on every run
plus the bench. The largest fixed cost we don't control is GPUI's GPU
setup, which the `window` mark makes visible; the system font scan on
Linux shows in the `platform` mark. Widgets from `gpui-component` on the
first frame cost about 9 ms (OQ-012).

### Platform notes

Linux: Wayland only; GPUI renders through wgpu on Vulkan. Windows 11:
DirectX 11, ConPTY via `alacritty_terminal`. macOS: compile-only.

### Security & privacy

- Stored: nothing. Timings are printed to stdout only, with no paths,
  commands or output.
- Trust boundaries: none crossed.

## Acceptance criteria

1. A dev run (`cargo run`) shows a TTFP badge in a corner of the window and prints the marks to stdout.
2. A release build launched with `--bench-startup` prints one JSON line with all six marks and exits with code 0, with no window interaction.
3. On the owner's Fedora machine, `tern --bench-startup 10` reports a warm-start median TTFP under 500 ms. The first (cold) run is reported separately.
4. A broken `config.yaml` (invalid YAML) still boots to a prompt with defaults and shows a notice naming the line.
5. With networking disabled, startup time and behavior are unchanged, and no request is attempted before the first prompt.
6. No white flash: the first painted frame is the theme background.
7. Started with `WAYLAND_DISPLAY` unset, Tern prints that it needs a Wayland session and exits with code 1 instead of hanging.
8. The same binary and bench run on Windows 11 and report all six marks.

## Open questions

- None.
