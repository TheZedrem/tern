---
status: now
type: bug
blocked-by: []
---
# T-082: TTFP is about 735 ms on Windows, over the 500 ms budget

On 2026-11-12 `tern --bench-startup 10` measured a warm median of
734–736 ms on the owner's Windows 11 machine, for the installed build of
that morning and for the T-066 build alike, run back to back.
`docs/ttfp.md` records 325–358 ms. The marks up to `window` (about
250 ms) look as before; the time goes between the window and the first
rendered frame (`first_byte` at 33 ms, `rendered` at 735 ms). Find out
whether it is the machine that day or a regression since v0.7.0, by
running v0.5.0's and v0.7.0's binaries in the same session. Area: src.

Spec: ../spec/0001-startup-performance.md
