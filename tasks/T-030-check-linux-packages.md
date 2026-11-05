---
status: next
blocked-by: []
---
# T-030: Check the Linux packages on their distros

Install the release workflow's first `.deb` on Debian 13 and Ubuntu 24.04,
its `.rpm` on Fedora and openSUSE Tumbleweed, and run its AppImage on one
more distro. Each installs without missing dependencies, shows Tern with
its icon in the launcher, opens a window with a prompt, groups its windows
under that icon (app ID `de.zedrem.tern`), and uninstalls cleanly. Area:
meta.

Spec: ../spec/architecture.md
