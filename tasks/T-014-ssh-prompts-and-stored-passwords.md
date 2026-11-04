---
status: now
blocked-by: []
---
# T-014: SSH prompts in the app and remembered passwords

zed-term as ssh's askpass helper (`SSH_ASKPASS_REQUIRE=force`), the local
socket with per-connection tokens, dialogs for every prompt ssh sends
(host keys, one-time codes, passwords), "Remember in keychain" through
`keyring`, auto-fill only for the connection's own password and key
passphrase prompts, one attempt per connection, "Forget saved password",
and the no-keychain fallback. The keychain keys need the target as ssh
resolves it: `ssh -G` at connect time (moved here from T-012). Build the
mitigations of threat model 0001 as spec 0006's flows 2 to 4 now state
them (endpoint permissions and server-PID check, exact prompt patterns,
fill notices, safe host-key dialogs, size caps and timeouts), and settle
OQ-016 with the owner, `zeroize` included. New crates: `keyring` 4,
`interprocess`, `getrandom`. Area: src.

Spec: ../spec/0006-ssh-hosts.md
Threat model: ../spec/threat-model/0001-ssh-askpass.md
