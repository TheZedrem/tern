---
status: next
blocked-by: [T-015]
---
# T-014: SSH prompts in the app and remembered passwords

zed-term as ssh's askpass helper (`SSH_ASKPASS_REQUIRE=force`), the local
socket with per-connection tokens, dialogs for every prompt ssh sends
(host keys, one-time codes, passwords), "Remember in keychain" through
`keyring`, auto-fill only for the connection's own password and key
passphrase prompts, one attempt per connection, "Forget saved password",
and the no-keychain fallback. The keychain keys need the target as ssh
resolves it: `ssh -G` at connect time (moved here from T-012). New crates:
`keyring` 4, `interprocess`, `getrandom`. Area: src.

Spec: ../spec/0006-ssh-hosts.md
