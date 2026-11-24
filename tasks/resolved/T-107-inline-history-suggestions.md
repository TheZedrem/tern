---
status: now
blocked-by: []
---
# T-107: Suggest past commands inline and accept them with Right

The snippets report that a prompt begins, and Tern reads where its
input starts at the first key (OQ-041). Draw
the newest matching command as grey text after the cursor at a
local prompt, and accept it with the bindable
`history.acceptSuggestion` (default Right), which reaches the shell unchanged when nothing is
suggested — src.
Spec: ../spec/0021-command-history.md
