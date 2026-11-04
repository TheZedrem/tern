---
status: later
blocked-by: []
---
# T-007: CI pipeline for Linux and Windows

GitHub Actions (OQ-004). Needs the GitHub repo to exist first (owner
creates it). No TTFP gating in CI. Minimum: `cargo fmt --check`,
`cargo clippy -- -D warnings`, `cargo test`, and release builds on Linux and
Windows. Area: meta.

Spec: ../spec/architecture.md
