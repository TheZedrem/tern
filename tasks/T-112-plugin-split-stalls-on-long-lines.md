---
status: now
type: bug
blocked-by: []
---
# T-112: Plugin split stalls on very long output lines

On Windows 11, the Docker plugin's "Follow logs" split for a remote
`open-webui` container (over SSH) stops partway through a line and never
moves again. Meanwhile `docker logs -f` for the same container in an SSH
pane keeps getting a new line every second. The container ID in the
split's argv matches, so the split shows the right container. That
container logs each chat request at debug level as one Python-repr line
of tens to hundreds of KB, and the split freezes in the middle of such a
line, so it looks like another container's logs.

The split is a normal PTY pane (`open_plugin_split` →
`split_command`, src/layout/requests.rs) running
`ssh -T … sh -c '… exec docker logs --tail 100 --follow <id>'`.
Find which layer stalls before fixing:

- the same argv in a plain Tern pane on Windows (plugin out of the loop);
- the same argv in Windows Terminal (Tern out of the loop: ConPTY or
  Win32-OpenSSH with `-T`);
- a local repro without Docker: a process that prints a single
  200 KB+ line, then a short line every second, through ConPTY and on
  Linux;
- whether Tern's PTY reader, the VT parser or rendering a hugely wrapped
  line blocks (per-frame work must not scale with line length, AGENTS.md
  "ways to hurt" #6).

Done when the split keeps following past long lines on Windows and Linux,
with a test for whichever layer was at fault — src.
Spec: ../spec/0016-plugin-system.md
