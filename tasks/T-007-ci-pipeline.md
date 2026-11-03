---
status: later
blocked-by: [T-001]
---
# T-007: CI pipeline for Linux and Windows

GitHub Actions (OQ-004). Needs the GitHub repo to exist first (owner
creates it). No TTFP gating in CI. Minimum: `cargo test`,
`cargo clippy`, frontend typecheck, entry-budget check, and Linux and Windows
builds. Area: meta.

Spec: ../spec/architecture.md
