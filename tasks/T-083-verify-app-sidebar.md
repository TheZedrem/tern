---
status: next
blocked-by: []
---
# T-083: Check the sidebar in the running app, on Windows and Linux

T-080 was built on Windows 11 and tested in GPUI's test window only: a
dev build on Windows uses the owner's live config and `session.json`, so
it was never run on screen. Check in the running app on Windows 11 and
on Linux (Wayland) what the tests can't see (spec 0018): each
`Ctrl+Shift+S` changes the shell's `tput cols` and leaves no stale rows,
ConPTY's redraw included (AC1); the order tab list, sidebar, panes for
each `tab_bar` and `app_sidebar` value, and a side change on the
settings screen moving an open sidebar (AC4); "Show files" and the
toggle on an SSH pane connecting, and a pane's folder and transfers
coming back without reconnecting (AC2); the SSH hosts panel against
`Ctrl+Shift+H`, F5, and a double click opening a connected tab (AC5);
Files on a local pane listing the shell's folder, and a program
run from it opening beside the pane (T-049); Port forwards appearing
in the strip only on an SSH pane (T-084); the SSH hosts menu (T-085): Edit… saving `config.yaml` and the list
showing the change, Delete… with Enter on the dialog meaning Cancel,
and Delete removing the profile (tests can't answer the dialog or save,
since `write_change` writes the real `config.yaml`); Tab, Shift+Tab, Left and Right walking the strip, the header and the
panel (flow 2); a zoomed pane keeping the sidebar and the settings
screen drawing over it; the first frame of a relaunch with the sidebar
saved open at 320 px (AC7). File what fails as bugs. To make the hosts
panel testable without the real `~/.ssh/config`, pass the ssh config
path into `HostsPanel`. Area: src.

Spec: ../spec/0018-app-sidebar.md
