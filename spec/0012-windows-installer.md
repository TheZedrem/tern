---
status: approved     # draft | approved | implemented
phase: 2
tasks: [T-040, T-039, T-052]
related: [spec 0001, spec 0011, T-026, OQ-024, OQ-025]
---

# Spec: Windows MSI installer

Pins how a `v*` tag turns into an MSI that installs Tern on Windows 11,
next to the Linux packages (T-026).

## Problem & goal

Windows 11 is a required platform, but a release ships only Linux
packages. A Windows user has to build Tern from source. Even then, the
binary opens a console window next to its own, has no icon and needs the
Visual C++ runtime installed.

Done means: a GitHub release carries `tern-<version>-x64.msi`. Installing
it needs no admin rights, puts Tern in the Start menu with its icon, and
launches to a prompt with no extra console window. A newer MSI upgrades it
in place, and uninstalling removes everything the MSI installed.

## Scope

**In scope:**

- A Windows-ready `tern.exe`: GUI subsystem, embedded icon and version info, static C runtime
- `packaging/windows/tern.wxs`, built with WiX Toolset v7
- A `windows` job in `release.yml` that attaches the MSI to the release
- A local build command in `docs/development.md`

**Out of scope:**

- Code signing (OQ-025). The first MSIs are unsigned, so SmartScreen warns on them
- ARM64 builds, a portable `.zip`, winget and the Microsoft Store (*Later*: own tasks)
- Adding `tern` to `PATH`, file associations, or "Open Tern here" in Explorer
- Auto-update: spec 0011 only tells the user about a release [load-bearing]

## Design

### Affected areas

- `src/main.rs` [new]: `#![windows_subsystem = "windows"]`, plus a parent console for CLI output
- `build.rs` [new]: embeds `assets/icon.ico` and the version info on Windows only
- `.cargo/config.toml` [new]: `+crt-static` for the `windows-msvc` targets
- `assets/icon.ico` [new]: 16–256 px, generated once from `assets/icon.png`
- `packaging/windows/tern.wxs` [new], `.github/workflows/release.yml`, `docs/development.md`

### Model

- **Subsystem:** a release build uses the `windows` subsystem, so no
  console opens from the Start menu. The CLI flags (`--bench-startup` and
  boot errors) attach to the parent console with
  `AttachConsole(ATTACH_PARENT_PROCESS)` when there is one, so the bench
  still prints from a terminal. Debug builds keep the console. A normal
  launch never attaches, or Ctrl+C in the launching `cmd` would close
  Tern. PowerShell doesn't wait for a GUI program, so the bench is piped
  there (`| Out-Host`) to print before the next prompt.
- **C runtime:** linked statically (`+crt-static`), so the MSI ships one
  file and needs no VC++ redistributable [load-bearing].
- **Resources:** `build.rs` embeds the icon and the version info
  (`ProductName` Tern, `FileVersion` from `CARGO_PKG_VERSION`) only when
  the target is Windows. Linux builds don't run it. The build-only crate
  is `winresource` (owner's call, 2026-11-09), a Windows-only build
  dependency. `FileDescription` is "Tern" too, since Task Manager shows it
  as the process name.
- **Installer tool:** WiX Toolset v7 as a pinned .NET tool (`wix`) with
  one hand-written `.wxs`. There's no `cargo-wix`, because it targets WiX
  v3, which is end-of-life. The EULA is accepted per build with
  `-acceptEula wix7` (OQ-024).
- **Install scope:** per-user (`Scope="perUser"`), into
  `%LOCALAPPDATA%\Programs\Tern\`, with no UAC prompt. Tern needs no
  admin rights, and per-user installs keep Windows from asking for them.
- **Identity [load-bearing]:** `UpgradeCode`
  `C03A9729-B0DE-4204-8B88-AF1B7B757ECC`, fixed for the product's
  lifetime. Changing it means Windows installs a second copy next to the
  old one instead of replacing it. Each release is a major upgrade
  (`<MajorUpgrade>`, scheduled after `InstallInitialize` so a failed
  upgrade rolls back to the old version): a new `ProductCode`, and the version bound from
  `tern.exe`'s version info, which `build.rs` takes from `Cargo.toml`.
  Installing an older version over a newer one is refused with a message.
- **Contents:** `tern.exe` and a Start menu shortcut named "Tern" with
  the AppUserModelID `de.zedrem.tern` (OQ-021). Tern sets the same ID on
  its process at boot (`set_app_identity`), because GPUI's Windows backend
  ignores the window's `app_id`. With both, pinned taskbar icons group with
  the running window. Windows Installer requires a per-user
  component's key path to be an HKCU value, so each of the two components
  also writes one marker value under `HKCU\Software\Tern`. That's all.
  Uninstall removes all of it.
- **Launch condition:** the MSI refuses Windows builds below 22000, so
  Windows 10 gets a clear message instead of a half-working install.
- **User data:** config, session, the update-check state and keychain
  entries are never in the MSI. Uninstall leaves them, as the Linux
  packages do.

### Flows

1. **Release:** a `v*` tag runs `checks` (ci.yml). Once it passes, the
   `linux` job creates the release with its packages. Then `windows` (on
   `windows-2025`, `needs: [checks, linux]`) builds `--release`, runs
   `wix build` and adds the MSI with `gh release upload`. One job creates
   the release and the other only uploads, so they can't race on it.
2. **Failure:** a failed `wix build` fails the job. The Linux packages are
   already published by then, so a failed Windows build leaves a release
   without an MSI. Re-running the job adds it; nothing is deleted.
3. **Install:** the user double-clicks the MSI → SmartScreen warns while
   it's unsigned → "More info → Run anyway" → no UAC → Start menu has
   Tern.
4. **Upgrade:** a newer MSI removes the old version and installs the new
   one. Config and session survive, because they live outside the install
   folder.
5. **Uninstall:** Settings → Apps → Tern → Uninstall removes the
   folder and the shortcut, and nothing else.

### Startup impact

None measurable expected. The subsystem and the static CRT change process
creation, not the boot path. T-040's commit reports the TTFP on Windows
before and after, to show it.

### Platform notes

- Windows 11 only. Linux builds must not run `build.rs`'s resource
  step, and `cargo build` on Linux stays unchanged.
- The GUI subsystem affects ConPTY child processes: shells spawned by
  Tern must still get a working console. Alacritty and WezTerm run the
  same setup. Check `cmd`, PowerShell and `ssh` in a pane after the change.
- macOS: nothing changes; it only needs to compile.

### Admin area

None: Tern is a local, single-user app with no admin area.

### Security & privacy

| Data | Where | Purpose | Legal basis | Retention |
|---|---|---|---|---|
| None | — | The MSI installs files only; it stores and sends nothing | — | — |

Ground rules touched: none. The installer makes no network call and writes
only the install folder, the shortcut, two marker values under
`HKCU\Software\Tern` and the uninstall registry entry Windows Installer
creates.

Trust boundaries:

- **An unsigned MSI can't be told apart from a tampered one** by the
  user. Until OQ-025 is decided, the release notes link the GitHub release
  page as the only download source. *Later*: signing, or published
  checksums, close this.
- **Supply chain:** WiX is pinned to an exact version, like the Linux
  packaging tools. CI downloads it from NuGet, the only new source.

## Acceptance criteria

1. A `v*` tag whose checks pass produces a release with `tern-<version>-x64.msi` next to the Linux packages; a failed check produces neither.
2. Installing the MSI as a standard user shows no UAC prompt and puts a "Tern" shortcut with Tern's icon in the Start menu.
3. Launching from the shortcut opens Tern's window to a prompt and no console window.
4. `tern.exe --bench-startup 3 | Out-Host`, run from PowerShell, prints its TTFP lines to that PowerShell.
5. On a clean Windows 11 without the VC++ redistributable, the installed Tern starts.
6. Installing version N+1 over N leaves one entry in Settings → Apps, at N+1, and keeps `config.yaml` and `session.json`; installing N over N+1 is refused.
7. Uninstalling removes the install folder and the shortcut, and leaves `%APPDATA%\tern\` untouched.
8. A pinned taskbar icon and Tern's running window share one taskbar button.
9. On Windows 10 the MSI refuses to install with a message naming Windows 11.
10. `cargo build` and `cargo test` on Linux are unchanged, and the Windows TTFP is unchanged within noise.

## Open questions

- OQ-024: accept the WiX v7 EULA for this project.
- OQ-025: sign the MSI, and with what.
