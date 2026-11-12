---
status: later
blocked-by: []
type: bug
---
# T-088: A PTY test for OSC 7 fails now and then

`terminal::pty::tests::a_local_programs_osc7_report_of_another_host_or_a_missing_dir_is_ignored`
failed once in a full `cargo test` run on Linux (2026-11-12, while other
test binaries were building) and passed in the next full run and in five
runs alone. Find what it waits on that a loaded machine can miss, and
make it wait for that instead of for time. Area: src.

Spec: ../spec/0003-tabs-splits-and-session-restore.md
