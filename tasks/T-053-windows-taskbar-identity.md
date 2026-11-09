---
status: next
type: bug
blocked-by: []
---
# T-053: Match the Windows process identity to the MSI shortcut

The MSI shortcut sets `System.AppUserModel.ID` to `de.zedrem.tern`, but
the running Windows process never sets that identity. GPUI's Windows
backend does not implement `PlatformWindow::set_app_id`, so the existing
`WindowOptions.app_id` does not set it. A pinned shortcut and a process
launched directly can therefore appear as separate taskbar buttons. Area: src.

Set the process identity through GPUI's `App::set_app_identity` before
opening the window, then check that launching both the shortcut and the
installed exe groups under the pinned button (spec 0012 AC8).

Spec: ../spec/0012-windows-installer.md
