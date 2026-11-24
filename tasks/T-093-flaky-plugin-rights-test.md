---
status: later
blocked-by: []
type: bug
---
# T-093: A plugin rights test flaked under a full test run

`plugins::tests::an_action_without_its_right_is_refused_and_the_log_names_the_right`
failed once at `src/plugins/mod.rs:1464` (the "a notice per refused line"
assertion's neighbourhood) during a full `cargo test` on Windows 11
(2026-11-16), and passed three times on its own right after. It looks
timing-dependent, like T-088: the test may settle before the plugin's
lines are all handled when the machine is busy. Make it wait on what it
asserts instead of on time. Area: src.

Spec: ../spec/0016-plugin-system.md

Seen again on 2026-11-24, in the same module: `plugins::tests::an_enabled_plugin_has_no_process_until_its_first_event`
hung twice in eight full runs on Windows 11 (once for over ten minutes,
until the run was stopped) and passed in the other six. A hang, not a
failed assertion, so the test also needs a time limit of its own.
