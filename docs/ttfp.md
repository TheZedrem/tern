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

Measured afterwards from the release tags. On Windows, opening the window
takes about 90 % of the time; config, PTY spawn and the shell's first byte
all finish while it opens.

## Linux (owner's Fedora machine, Wayland)

The GPUI spike measured 333 ms warm and 811 ms cold (spec 0001). The
releases haven't been benchmarked here yet; add a row with the next
release.
