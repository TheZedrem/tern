# TTFP history

The startup time of each release, so drift across releases shows up and
isn't blamed on whichever commit happens to cross the line. What TTFP
measures and the 500 ms budget are in
[spec 0001](../spec/0001-startup-performance.md).

How: build the release tag, then run `tern --bench-startup 10` (on
Windows, `tern.exe --bench-startup 10 | Out-Host`). The cold run comes
first, then the median and p90 of 10 warm launches. Add a row before
tagging each release ([conventions](conventions.md#releases)). Compare
rows only within the same machine: the absolute numbers depend on the
hardware.

## Windows 11 (owner's machine)

| Release | Cold | Warm median | Warm p90 | Window ready (median) | Measured |
|---|---|---|---|---|---|
| v0.1.0 | 276.5 ms | 270.6 ms | 284.6 ms | 253.8 ms | 2026-11-09 |
| v0.1.1 | 291.4 ms | 275.4 ms | 289.7 ms | 250.0 ms | 2026-11-09 |
| v0.2.0 | 332.3 ms | 267.0 ms | 277.5 ms | 243.9 ms | 2026-11-09 |
| v0.3.0 | 298.4 ms | 282.8 ms | 286.3 ms | 258.8 ms | 2026-11-09 |
| v0.4.0 | 300.5 ms | 289.4 ms | 316.8 ms | 265.9 ms | 2026-11-09 |
| v0.4.1 | 290.5 ms | 275.8 ms | 280.7 ms | 258.3 ms | 2026-11-09 |
| v0.6.1 | 305.2 ms | 282.8 ms | 297.4 ms | 269.9 ms | 2026-11-11 |

Measured afterwards from the release tags, one after another in the same
session. On Windows, opening the window takes about 90 % of the time;
config, PTY spawn and the shell's first byte all finish while it opens.
From v0.2.0 to v0.4.0 the warm median grew by 22 ms, all of it in
"window ready": the files pane and settings import arrived in that range.
That's well inside the budget, but it's the trend to watch.

## Linux (owner's Fedora machine, Wayland)

The GPUI spike measured 333 ms warm and 811 ms cold (spec 0001).

| Release | Cold | Warm median | Warm p90 | Window ready (median) | Measured |
|---|---|---|---|---|---|
| v0.5.0 | 346.1 ms | 304.0 ms | 319.8 ms | 190.4 ms | 2026-11-10 |
