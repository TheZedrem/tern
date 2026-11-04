# Threat models

STRIDE models of the parts of zed-term that cross a trust boundary in
[architecture §Security](../architecture.md#security-binding). One file per
component, `NNNN-<component>.md`, from the `create-threat-model` skill's
template; numbers are never reused. Threat IDs (`TM-<component>-nn`) stay
stable so tasks and bugs can cite them.

## Priority list

1. **SSH askpass and stored credentials** (spec 0006, §Security 3):
   secrets cross a local socket and the OS keychain. Done: 0001.
2. **Escape sequences from programs and remote hosts** (§Security 1):
   titles, OSC 8 links and OSC 52 clipboard writes, before T-002 honors
   any of them.
3. **Agent profiles** (Phase 3, §Security 4): commands run locally or over
   SSH, before the Phase 3 spec is approved.

## Register

| Model | Component | Date | Status |
|---|---|---|---|
| [0001](0001-ssh-askpass.md) | SSH askpass and stored credentials | 2026-11-04 | draft, accepted risks wait in OQ-016 |
