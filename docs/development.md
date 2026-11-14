# Development

How to get a working dev setup. Day-to-day commands are in
[`AGENTS.md` §Operations](../AGENTS.md#operations).

## Prerequisites

- Rust (stable, edition 2024) with `rustfmt` and `clippy`
  - **Fedora:** `sudo dnf install rust cargo rustfmt clippy`
  - elsewhere: `rustup component add rustfmt clippy`
- **Linux:** a Wayland session (X11 is not supported) and a Vulkan driver.
  The build links only `libxkbcommon`, but GPUI's font loader also needs
  fontconfig's pkg-config file at build time (the binary doesn't link it).
  The Wayland client and the Vulkan loader are loaded at runtime.
  - **Fedora:** `sudo dnf install libxkbcommon-devel fontconfig-devel vulkan-loader mesa-vulkan-drivers`
  - **Debian/Ubuntu:** `sudo apt install libxkbcommon-dev libfontconfig-dev libvulkan1 mesa-vulkan-drivers`
- **Windows 11:** the MSVC toolchain (Visual Studio Build Tools). Only
  for the MSI: the .NET SDK, for WiX (see [Windows installer](#windows-installer)).
- **macOS:** Xcode Command Line Tools (compile-only platform). GPUI
  compiles its shaders with `xcrun metal` at build time, which only full
  Xcode ships (`xcodebuild -downloadComponent MetalToolchain`). Without it,
  build with `--features runtime-shaders`, which compiles them at every
  launch instead and costs startup time; `packaging/macos/install.sh` does
  this on its own, with a warning.

## First run

```sh
cargo run
```

The TTFP badge in the window corner is the startup time of this run. See
[spec 0001](../spec/0001-startup-performance.md) for what it measures, and
use `cargo build --release && target/release/tern --bench-startup 10`
for numbers you can compare. With `TERN_BENCH_SESSION=<session.json>`
the bench restores that session instead of starting one pane, to measure a
restored layout (spec 0003, AC7).

On Windows a release `tern.exe` is a GUI program, so PowerShell returns
the prompt without waiting for it and its output lands after the prompt.
Pipe it to wait: `target\release\tern.exe --bench-startup 10 | Out-Host`.

Sessions are saved in `~/.local/share/tern/session.json` (Linux). To
try one without touching yours, point `XDG_DATA_HOME` elsewhere.

To try the title bar Tern draws on GNOME (spec 0003) on any other
Wayland compositor, start it with `TERN_CLIENT_DECORATIONS=1`.

To try a config without touching your own, point `XDG_CONFIG_HOME`
elsewhere (Linux): `XDG_CONFIG_HOME=/tmp/zt cargo run` reads and watches
`/tmp/zt/tern/config.yaml`.

To run this checkout as `tern` from anywhere, `packaging/linux/install.sh`
(or `packaging/macos/install.sh`) builds the release binary and links it into
`${XDG_BIN_HOME:-~/.local/bin}`. The Linux script also installs the desktop
entry and icon into `~/.local/share`, so launchers and taskbars show Tern's
icon. The macOS script also writes an ad-hoc
signed `~/Applications/Tern.app` with a copy of the binary. On Windows,
`pwsh -File packaging/windows/install.ps1` copies `tern.exe` into
`%USERPROFILE%\.local\bin` (a copy, since symlinks need Developer Mode) and
adds a Start menu shortcut, with an icon it writes to `%LOCALAPPDATA%\tern`.
The copy is locked while it runs, so close that Tern first. Re-run it after
pulling to update.

## Tests

`cargo test` runs everything that needs no window. `cargo test -- --ignored`
adds the live startup test, which opens a real window and needs a Wayland
session.

## Where things are written at runtime

| File | Path (Linux / Windows) |
|---|---|
| Config | `~/.config/tern/config.yaml` / `%APPDATA%\tern\config.yaml` |
| User themes | `…/tern/themes/*.yaml` |
| Session state | `~/.local/share/tern/session.json` / `%LOCALAPPDATA%\tern\session.json` |

## Linux packages

A `v*` tag matching `Cargo.toml`'s version packages the `.deb`, `.rpm` and
AppImage in CI (`.github/workflows/release.yml`) from the binary its
commit's CI run built, once that run passed, and attaches them to a
GitHub release ([CI](conventions.md#ci)). To build them locally after `cargo build --release`:

```sh
cargo install --locked cargo-deb cargo-generate-rpm   # once
cargo deb --no-build         # target/debian/tern_<version>-1_amd64.deb
cargo generate-rpm           # target/generate-rpm/tern-<version>-1.x86_64.rpm
packaging/linux/appimage.sh <path to appimagetool>   # target/packages/Tern-<version>-x86_64.AppImage
```

`appimagetool` comes from
[AppImage/appimagetool](https://github.com/AppImage/appimagetool/releases);
it downloads the AppImage runtime while it runs. A local build links
against your distro's glibc, so its packages only install on that distro
version or newer. The release packages are built on Ubuntu 24.04 and need
glibc 2.39.

Every package takes its icon from `assets/icon.png`, rendered from the
source `assets/icon.svg`. After changing the SVG, re-render it:

```sh
magick -background none -density 192 assets/icon.svg -resize 512x512 -depth 8 assets/icon.png
magick assets/icon.png -define icon:auto-resize=256,128,64,48,32,24,16 assets/icon.ico
```

`assets/icon.ico` is the Windows icon `build.rs` embeds in `tern.exe`.

### Package repositories

After each release, `.github/workflows/repositories.yml` downloads the
`.rpm` and `.deb` of the latest 20 releases, signs them, and publishes a
dnf/zypper repository (`rpm/`) and a flat apt repository (`deb/`) as one
GitHub Pages site, with the public key at its root as `tern.asc`
(T-031). Nothing binary is committed to git. The work is
`packaging/linux/repositories.sh`, which needs `rpmsign`, `createrepo_c`,
`apt-ftparchive` and a GPG key without a passphrase; to build the site
locally in a container:

```sh
podman run --rm -v "$PWD:/src:ro,z" -v "$PWD/target/repo:/work:z" ubuntu:24.04 bash -c '
  apt-get update && apt-get install -y rpm createrepo-c apt-utils gnupg &&
  gpg --batch --passphrase "" --quick-gen-key "Tern test" rsa4096 sign never &&
  id=$(gpg --list-secret-keys --with-colons | awk -F: "/^fpr:/ {print \$10; exit}") &&
  /src/packaging/linux/repositories.sh /work/packages /work/site "$id" file:///site'
```

with the packages in `target/repo/packages`. The workflow does nothing
without the `TERN_SIGNING_KEY` secret. `packaging/linux/setup-repositories.sh`
walks the owner through making the key, backing it up, storing the
secret and switching Pages to GitHub Actions; run it again to rotate the
key, which every user then has to trust anew.

## Windows installer

CI builds the MSI on every push, and a tag adds that one to the release
as `tern-<version>-x64.msi` ([spec 0012](../spec/0012-windows-installer.md)).
It's a per-user install, so it needs no admin rights. Release builds link
the C runtime statically (`.cargo/config.toml`), so the MSI ships
`tern.exe` alone. To build it locally after `cargo build --release`:

```powershell
dotnet tool install --global wix --version 7.0.1   # once
wix build -arch x64 -acceptEula wix7 packaging/windows/tern.wxs -o target/packages/tern-<version>-x64.msi
```

`-acceptEula wix7` accepts WiX v7's EULA for this build (OQ-024). The
MSI's version is `tern.exe`'s, which comes from `Cargo.toml`, so it has to
be plain `x.y.z`. `wix msi validate -acceptEula wix7 <msi>` runs the ICE
checks. Expect two warnings: ICE69, because the shortcut and the exe are
in separate components, and ICE91, because the MSI is per-user only.
