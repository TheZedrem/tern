---
status: next
blocked-by: [T-106]
---
# T-107: Suggest past commands inline and accept them with Right

Draw the newest matching command as grey text after the cursor at a
local prompt, and accept it with the bindable `accept_suggestion`
(default Right), which reaches the shell unchanged when nothing is
suggested — src.
Spec: ../spec/0021-command-history.md
