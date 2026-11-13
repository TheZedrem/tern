---
status: next
blocked-by: []
---
# T-090: Reconnect with Enter after any SSH exit that leaves the pane open

An SSH tab whose session ended with a non-zero code other than 255 (e.g.
`exit` after a `^C` on the remote, code 130) stays open under
`close_on_exit: clean` but shows only `[process exited with code 130]`;
Enter does nothing. Every SSH pane that stays open after its ssh exited
should offer `press Enter to reconnect` and rerun the same command on
Enter. Today only exit 255 gets `AfterExit::Reconnect`
(`after_exit` in `src/terminal/exit.rs`). Local shells keep the current
behavior. Requested by the owner (2026-11-13). Area: src.

Spec: ../spec/0006-ssh-hosts.md
