---
status: next
blocked-by: []
---
# T-029: Opt-in check for new releases on GitHub

After the first launch, Tern asks once whether to check GitHub for new
releases. With "Check daily", a newer release shows a notice that links
its release notes; `app.checkForUpdates` checks on demand. The setting
`update_check` is on the settings screen and in the config template.
Area: src.

HTTP client: `ureq` 3 with `rustls` and `platform-verifier` (OQ-022). Report the TTFP.

Spec: ../spec/0011-update-check.md
