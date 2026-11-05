---
status: next
blocked-by: []
---
# T-021: Threat model: escape sequences from programs and remote hosts

STRIDE model of what terminal output can make the app do (architecture
§Security 1 and 2), priority 2 in `spec/threat-model/README.md`, due
before T-002 honors links, cwd reports or paste: titles (OSC 0/2), cwd
(OSC 7), links (OSC 8 and detected URLs), clipboard writes and reads
(OSC 52), the replies the terminal sends back (colors, size, device
attributes), and bracketed paste. Fold the mitigations into spec 0002 and
T-002; residual risks go to `spec/open-questions.md` for the owner's
sign-off. Area: meta.

Spec: ../spec/0002-terminal-pane.md
Spec: ../spec/architecture.md (§Security 1 and 2)

## Result (2026-11-05)

Threat model [0002](../../spec/threat-model/0002-escape-sequences.md): 13
threats over titles, cwd reports, links, OSC 52, replies, paste and copy.
Already mitigated: titles (T-003, T-006), OSC 52 writes and reads and
query replies (T-010; vte 0.15 has no title or string reports). T-002
builds the rest as spec 0002's Security & privacy now states it: links
only for `http`/`https` on Ctrl+click with the target shown, cwd reports
only from local panes for local directories, paste with control
characters dropped, copy without zero-width or direction characters, and
size caps. Residual risks (multi-line paste without bracketed paste, query
replies, copied text in the background color) await the owner's sign-off
in OQ-017. One finding corrected spec 0002: vte doesn't parse OSC 7, so
zed-term scans the PTY bytes for it.
