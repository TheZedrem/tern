---
status: now
blocked-by: []
---
# T-015: Threat model for SSH credentials

STRIDE threat model of the askpass path in spec 0006: the helper, the
local socket and its tokens, the keychain, and prompts that a remote host
or a jump host can influence. Use the `create-threat-model` skill; fold
anything it changes back into spec 0006 before T-014 starts. Area: meta.

Spec: ../spec/0006-ssh-hosts.md

## Result (2026-11-04)

[Threat model 0001](../../spec/threat-model/0001-ssh-askpass.md), the
first in the new `spec/threat-model/` (with a README holding the priority
list and the register): 20 threats over six trust boundaries, each with a
named mechanism. Folded into spec 0006's Target, flows 2 to 4 and its
trust boundaries, among them:
- the endpoint's protections and the helper's server-PID check (a
  squatted Windows pipe could answer host keys "yes");
- exact prompt patterns from `ssh -G`, with `hostkeyalias`;
- a notice whenever a stored password is used;
- host-key dialogs where Enter means No;
- prompt text stripped of direction and invisible characters;
- size caps and timeouts;
- `ssh -G` only when connecting, since `Match exec` runs commands.

Three residual risks wait for the owner's sign-off in OQ-016: code
running as the user, secrets in memory until freed (`zeroize` is T-014's
call), and a user typing a secret into a prompt the remote printed. T-014
builds the rest; T-011 checks the Windows half.
