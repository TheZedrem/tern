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
fill notices, safe host-key dialogs, size caps and timeouts, `zeroize`d
secrets, and the warning at "Remember" that recommends a private key).
New crates, approved with spec 0006 and OQ-016: `keyring` 4,
`interprocess`, `getrandom`, `zeroize`. Area: src.

Spec: ../spec/0006-ssh-hosts.md
Threat model: ../spec/threat-model/0001-ssh-askpass.md

## Result (2026-11-05)

TTFP warm median 337.8 ms (p90 347.0, cold 462.8), budget 500 ms; T-013
measured 339.8 ms. The boot path gained the helper-mode check at the top
of `main`, before the TTFP clock starts: it reads two environment
variables.

Built: an SSH connection starts ssh with `SSH_ASKPASS` set to the app's
own binary and a token of its own. The helper checks that the endpoint's
server is the app's PID, hands the prompt over a local socket (a 0700
directory in `$XDG_RUNTIME_DIR`, peer UID checked; an owner-only named
pipe on Windows) and prints only the answer (`src/ssh/askpass.rs`).
`src/ssh/target.rs` resolves the target with `ssh -G` and matches exactly
the connection's own password and passphrase prompts; `src/secrets.rs`
keeps those in the keychain through `keyring`, each call on a thread of
its own with a timeout. The pane fills a saved secret once per connection
with a notice, and opens a dialog over the pane for everything else
(`src/terminal/prompt.rs`): masked input with a reveal toggle, "Remember
in keychain" with the warning, "Replace" after a saved secret failed, and
Yes/No with No focused for host keys. "Forget saved password…"
(`host.forget`) forgets what's saved for a host picked in the host
picker. Without a keychain, "Remember" is hidden and a notice says why.

Spec changes: flow 7's picker lists every host and resolves only the one
picked, since knowing beforehand which hosts have a saved password would
take `ssh -G` while browsing (TM-askpass-20); `widestring` is a
Windows-only dependency for the pipe's security descriptor.

Verified: 312 tests, the new ones written by a separate agent against
spec 0006 and threat model 0001, and an ignored one that runs the real
OpenSSH 10.6 against a throwaway sshd: the host-key question, then the
passphrase, which never shows up in ssh's `/proc` environ or cmdline. By
hand, the helper answers with the right token and exits 1 with no output
for a wrong token, a wrong server PID or a decline.

Not verified: the dialogs in the running app on Linux (untested for now,
the owner's call) and Windows (added to T-011).
