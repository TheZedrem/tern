---
status: next
blocked-by: []
---
# T-081: Check the plugin host on Linux and in the running app

T-066 was built and tested on Windows 11; its Unix lines (the process
group and `killpg` in `src/plugins/process.rs`, the `sh` plugin of the
host tests) were never compiled or run before CI. Check on Linux
(Wayland) that `cargo test plugin` passes, and in the running app on
both systems, with a plugin enabled by hand in `config.yaml`:

- TM-plug-14: the plugin process holds only its three pipes
  (`/proc/<pid>/fd` lists 0, 1 and 2; on Windows a handle listing shows
  no ConPTY pipe). A leak is a `type: bug` before T-068 ships.
- AC10: quitting Tern leaves no plugin process, a child the plugin
  started included, and starting a plugin on Windows flashes no console.
- AC2: `--bench-startup 10` with three enabled plugins that get `start`
  against none. T-066 measured only without a `plugins:` block, because
  the benchmark reads the user's own config.

**Linux half done (2026-11-12, Fedora 45, COSMIC on Wayland, release
build).** Three `sh` plugins that get `start` (one exits after it, one
reads until EOF, one ignores EOF and starts a child), Tern run with
`XDG_CONFIG_HOME` and `XDG_DATA_HOME` pointing at a scratch folder:

- `cargo test plugin`: 69 passed.
- TM-plug-14: a plugin and the child it started hold only descriptors 0,
  1 and 2, all pipes, while Tern itself holds a PTY master. No leak.
- Quit: the window closed with its last tab, and no plugin process was
  left, the EOF-ignoring plugin's child included.
- AC2: each plugin started about 3 ms after the first-frame report, none
  before. `--bench-startup 10`, two rounds each: warm median 379 and
  328 ms with no `plugins:` block, 320 and 345 ms with three enabled
  plugins, so within noise. The benchmark quits at the first frame, so it
  never starts a plugin: it measures reading the `plugins:` block.

**Still open: the Windows 11 half.** In the running app there: a handle
listing of a plugin process shows no ConPTY pipe, quitting leaves no
plugin process or child, and starting a plugin flashes no console.

Area: src.

Spec: ../spec/0016-plugin-system.md
