---
status: next
blocked-by: [T-028]
---
# T-026: Linux packages: .deb, .rpm and AppImage

A user on Debian/Ubuntu, Fedora/openSUSE or any other distro installs
zed-term from a package and finds it in their app launcher with its icon.
Area: meta.

- One release binary (`cargo build --locked --release` on `ubuntu-24.04`)
  goes into every package, so they all need glibc 2.39 or newer (Debian 13,
  Ubuntu 24.04, Fedora 40).
- Every package installs the binary, a `.desktop` file and `assets/icon.png`
  under the app ID `de.zedrem.tern` (OQ-021), which T-028 sets as the
  Wayland `app_id`.
- Runtime dependencies: `libxkbcommon` and the Vulkan loader; `ssh` as a
  recommendation (Phase 2). Debian: `libxkbcommon0`, `libvulkan1`,
  `openssh-client`. Fedora: `libxkbcommon`, `vulkan-loader`,
  `openssh-clients`.
- Proposed tooling, to confirm with the owner (new build tools, not
  dependencies of the binary): `cargo-deb` and `cargo-generate-rpm`
  configured in `Cargo.toml`'s `[package.metadata]`, and `appimagetool` for
  the AppImage.
- No Flatpak for now (OQ-020).
- CI builds all packages on a `v*` tag and attaches them to the GitHub
  release once T-007 is green.
- Check each package on its distro: it installs, the launcher shows the
  icon, a prompt appears, and it uninstalls cleanly.

Spec: ../spec/architecture.md
