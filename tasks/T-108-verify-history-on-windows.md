---
status: next
blocked-by: [T-107]
---
# T-108: Verify command history suggestions on Windows 11

In pwsh with the snippet, and in a WSL shell, check recording, the
suggestion's placement after resizes (ConPTY repaints), stepping aside
for PSReadLine's prediction, and Right with and without a suggestion.
Also check in the running app, on Windows and on Linux (Wayland): a
command run in a pane with the snippet lands in `history.jsonl`, the
settings screen's Command history row switches `command_history`, and
**Clear command history** deletes the file. T-106 verified these with
tests and with the snippets run directly, not by hand in the app —
meta.
Spec: ../spec/0021-command-history.md
