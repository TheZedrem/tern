---
status: next
blocked-by: []
---
# T-026: Linux packages: .deb, .rpm and AppImage

A user on Debian/Ubuntu, Fedora/openSUSE or any other distro installs
Tern from a package and finds it in their app launcher with its icon.
Area: meta.

- One release binary (`cargo build --locked --release` on `ubuntu-24.04`)
  goes into every package, so they all need glibc 2.39 or newer (Debian 13,
  Ubuntu 24.04, Fedora 40).
- Every package installs the binary, a `.desktop` file and `assets/icon.png`
  under the app ID `de.zedrem.tern` (OQ-021), also the Wayland
  `app_id`.
- Runtime dependencies: `libxkbcommon` and the Vulkan loader; `ssh` as a
  recommendation (Phase 2). Debian: `libxkbcommon0`, `libvulkan1`,
  `openssh-client`. Fedora: `libxkbcommon`, `vulkan-loader`,
  `openssh-clients`.
- Tooling (owner's OK, 2026-11-05): `cargo-deb` and `cargo-generate-rpm`
  configured in `Cargo.toml`'s `[package.metadata]`, and `appimagetool` for
  the AppImage. License EUPL-1.2, maintainer "Andreas Zeidler" (owner).
- No Flatpak for now (OQ-020).
- CI builds all packages on a `v*` tag and attaches them to the GitHub
  release once T-007 is green.
- Check each package on its distro: it installs, the launcher shows the
  icon, a prompt appears, and it uninstalls cleanly.

Done (2026-11-05): all three build locally from the release binary, the
AppImage boots to a prompt in 349 ms warm (`--bench-startup 3`), and
`.github/workflows/release.yml` builds them on a `v*` tag. Not yet checked:
installing each package on Debian/Ubuntu, Fedora and openSUSE (T-030) and
the release workflow's first run (needs T-007's repo).

Spec: ../spec/architecture.md
