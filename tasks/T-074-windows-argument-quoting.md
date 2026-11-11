---
status: next
blocked-by: []
type: bug
---
# T-074: Arguments with spaces split apart on Windows

`pty::rerun` leaves alacritty_terminal's `tty::Options.escape_args` at
its default, `false`, so on Windows a program's arguments are joined into
the command line unquoted. An SSH profile whose key file sits under a
path with a space (`ssh -i "C:\Users\me\OneDrive - Firma\id_ed25519"`)
reaches `ssh` as several arguments and fails; since T-057 the key file
picker writes such paths in one click. A local profile's `command`
arguments with spaces split the same way. Setting `escape_args: true`
quotes them by the C runtime's rules, but changes how existing configs
behave: an argument the user quoted by hand gets quoted again, so check
`cmd.exe /c` and `pwsh -Command` profiles and say in the release notes
what changed. Found by the T-022/T-057 test agent. Area: src.

Spec: ../spec/0002-terminal-pane.md
