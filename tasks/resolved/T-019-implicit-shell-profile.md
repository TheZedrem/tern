---
status: now
blocked-by: []
---
# T-019: An implicit `shell` profile that new tabs open

`shell`, the login shell, is a profile without being configured, and new
tabs open it unless `default_profile` names another one; an unknown
`default_profile` falls back to it with a notice. A configured profile
with the ID `shell` replaces it. The palette lists it with the others.
Asked for by the owner after T-012, so profiles like SSH hosts don't take
over new tabs. Area: src.

Spec: ../spec/0005-config-and-themes.md

## Result (2026-11-04)

TTFP warm median 334.7 ms (p90 355.6, cold 591.2), budget 500 ms; T-012
measured 327.2 ms.

Built: `Config::all_profiles`, `Config::profile` and `config::SHELL`, so
the implicit `shell` exists in every config and is what new tabs open
without a `default_profile`; spec 0005 and the config template say so.

Verified: 229 tests; a separate agent updated the two that encoded the old
"first profile" rule and added ones for an SSH-only config, a configured
`shell` replacing the implicit one, and a gone profile falling back.
