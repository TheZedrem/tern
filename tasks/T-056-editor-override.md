---
status: later
blocked-by: []
---
# T-056: Let the user override the default editor

Edit copies (a remote file from the SFTP sidebar, a local edit-only or
Edit-chosen file from the files pane) open in the OS's default app for
their type, which on a fresh machine is Notepad, or a browser for `.html`.
An `editor` config key names the editor Tern opens them in instead, as
a command plus arguments with the file's path appended, run directly and
never through a shell; empty (the default) keeps the OS default. The
spec decides whether a configured editor also takes the types the OS
would run (`.sh`, `.desktop`, `.exe`), since an editor never runs them,
and whether a per-type choice (spec 0008's *Later* line) fits the same
key or waits. The key gets its template line in `default-config.yaml`
and a settings-screen row with a file picker, per the owner's standing
rule. Planned by the owner for the roadmap (2026-11-09); needs specs
0008 and 0013 extended first. Area: src.

Spec: ../spec/0008-sftp-panel.md, ../spec/0013-files-panel.md
