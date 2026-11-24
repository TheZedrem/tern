---
status: now
blocked-by: []
---
# T-110: Verify the WSL snippet offer in the running app

On Windows 11, walk spec 0022's acceptance criteria by hand: the bar
after typing `wsl` in a PowerShell pane and in a profile that runs
`wsl.exe`, Add (one line in `~/.bashrc`, suggestions work at once, no
second line from a second pane), no bar with the snippet sourced, Not
now, Never ask and the settings row that turns it back on, and that
keys still reach the shell while the bar shows. T-109 checked the typed
line in a real WSL bash and the rest with tests, not in the app — meta.
Spec: ../spec/0022-wsl-snippet-offer.md
