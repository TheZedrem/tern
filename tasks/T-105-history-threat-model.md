---
status: next
blocked-by: []
---
# T-105: Amend the escape-sequence threat model for command history

Add `TM-escape` entries to threat model 0002 for the shell-integration
history reports: forged or replayed reports, the per-run token and who
can read it, secrets in stored commands (the never-stored rules and the
token patterns), and inserting a suggestion into the shell — meta.
Spec: ../spec/0021-command-history.md
