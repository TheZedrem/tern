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

Area: src.

Spec: ../spec/0016-plugin-system.md
