---
status: later
blocked-by: []
---
# T-007: CI pipeline for Linux and Windows

GitHub Actions (OQ-004). Needs the GitHub repo to exist first (owner
creates it). No TTFP gating in CI. Minimum: `cargo fmt --check`,
`cargo clippy -- -D warnings`, `cargo test`, and release builds on Linux and
Windows. Area: meta.

Written (2026-11-05): `.github/workflows/ci.yml` runs all four on
`ubuntu-24.04` and `windows-2025` with GitHub's own actions only. What's
left: push to the repo once it exists, see both jobs go green (the
Windows one is the first Windows build at all, T-011), and pin the
actions to commit SHAs.

Spec: ../spec/architecture.md
