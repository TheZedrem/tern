---
status: later
blocked-by: [T-022, T-023]
---
# T-034: SFTP follow-terminal mode

A follow-terminal mode for the SFTP sidebar: its listing follows each `cd`
the SSH pane's shell reports with OSC 7. Plus an opt-in per profile, off by
default, that makes a bash server report its cwd by sending a setup line
after login, as MobaXterm's "Follow SSH path" does. Tern then types
into the user's shell, which nothing else does: it only works on bash, can
land in the shell's history and replaces an existing `PROMPT_COMMAND`, so
spec 0008 must pin those costs before this is built. Planned by the owner
(2026-11-06). Area: src.

Spec: ../spec/0008-sftp-panel.md
