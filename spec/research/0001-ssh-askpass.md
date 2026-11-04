# Research 0001: SSH prompts, askpass and the keychain

Feeds [spec 0006](../0006-ssh-hosts.md). Checked 2026-11-04 against OpenSSH
10.6p1 (the reference machine's `ssh`, and the `openssh-portable` master
sources).

## How ssh runs an askpass program

- `readpass.c`: ssh runs `$SSH_ASKPASS` with the prompt text as its only
  argument and reads the answer from its stdout (up to 1023 bytes). A
  failing helper counts as no answer.
- `SSH_ASKPASS_REQUIRE=force` makes ssh use the helper for every prompt,
  even with a terminal and without `DISPLAY`/`WAYLAND_DISPLAY`. `prefer`
  needs one of those set; `never` disables it.
- The helper learns the prompt kind from `SSH_ASKPASS_PROMPT`: `confirm`
  for yes/no permission prompts (agent confirmations), `none` for
  notifications such as "touch your security key" (ssh kills the helper
  when done). Normal passphrase prompts set nothing.
- The unknown-host-key question (`sshconnect.c`, `confirm()`) is read with
  `read_passphrase(msg, RP_ECHO)`, so with `force` it reaches the helper as
  a normal prompt with no hint. A helper must never answer prompts it
  doesn't recognize.

## Prompt texts (sshconnect2.c)

- Password authentication: `"%s@%s's password: "`, user and host, where
  host is `HostKeyAlias` if set, else the resolved host name.
- Keyboard-interactive: `"(%s@%s) %s"`, the same user and host, then the
  server's own prompt (`Password: ` with PAM, or a one-time-code question).
- Key passphrase: `"Enter passphrase for key '%.100s': "`, the key path,
  truncated to 100 characters.
- `ssh -G <destination>` prints the effective `user`, `hostname`, `port`
  and `hostkeyalias` without connecting, which is how the app learns what
  those prompts will say.

## Windows

- Win32-OpenSSH 8.6.0.0p1 made askpass require `SSH_ASKPASS_REQUIRE`
  (`prefer` or `force`); OpenSSH maintainers recommend `force` on Windows,
  where `DISPLAY` means nothing. Windows 11 ships a newer client.
- Local IPC: named pipes. The `interprocess` crate (2.4.7, 0BSD or
  Apache-2.0) offers one local-socket API over Unix sockets and named pipes.

## Keychain

- `keyring` 4.2.0 (MIT or Apache-2.0): default stores are Secret Service
  over `zbus` (pure-Rust D-Bus, no libdbus) on Linux, Credential Manager on
  Windows, Keychain on macOS. Secret Service needs a provider running
  (GNOME Keyring, KWallet, KeePassXC).

## Sources

- [openssh-portable `readpass.c`](https://github.com/openssh/openssh-portable/blob/master/readpass.c)
- [openssh-portable `sshconnect.c`](https://github.com/openssh/openssh-portable/blob/master/sshconnect.c)
- [openssh-portable `sshconnect2.c`](https://github.com/openssh/openssh-portable/blob/master/sshconnect2.c)
- `man ssh`, ENVIRONMENT, OpenSSH 10.6p1
- [Win32-OpenSSH V8.6.0.0p1-Beta release notes](https://github.com/PowerShell/Win32-OpenSSH/releases/tag/V8.6.0.0p1-Beta)
- [OpenSSH bug 3237 thread on askpass and `DISPLAY`](https://lists.mindrot.org/pipermail/openssh-bugs/2020-December/022174.html)
- crates.io metadata for `keyring` 4.2.0 and `interprocess` 2.4.7
