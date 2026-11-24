---
status: now
blocked-by: []
---
# T-109: Offer to add the shell snippet in a WSL session

When a local pane on Windows runs a WSL shell that doesn't load the
snippet, show a bar with Add, Not now and Never ask. Add types the line
that sources the snippet from the shell's rc file; Never ask is
remembered as `snippet_offer: never`, with its row on the settings
screen and its template line in `src/default-config.yaml` — src.
Spec: ../spec/0022-wsl-snippet-offer.md
