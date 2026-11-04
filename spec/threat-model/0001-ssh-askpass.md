---
status: draft
date: 2026-11-04
author: Claude (T-015), for review by the owner
related: [spec 0006, architecture §Security 1 and 3, T-014, OQ-016]
---

# Threat model: SSH askpass and stored credentials

## 1. Component overview

How spec 0006 gets a password or passphrase from the OS keychain (or the
user) into the system `ssh` without it touching the PTY, an argument, the
environment or a log. T-014 builds it; nothing of it exists yet, so every
mitigation below is a requirement on T-014, folded into spec 0006.

zed-term starts `ssh` with `SSH_ASKPASS` set to its own binary and
`SSH_ASKPASS_REQUIRE=force`. When ssh needs an answer it runs that binary
as a helper, with the prompt as its argument. The helper hands the prompt
to the running app over a local socket, with a per-connection token, and
prints the answer for ssh. The app answers from the keychain for exactly
the connection's own password and passphrase prompts; everything else is
a dialog for the user.

```
            TB1: network                TB2: exec, argv, stdout     TB3: local socket
remote host ────prompts────► ssh ──argv: prompt text──► helper ──token, prompt──► app: askpass
jump host   ◄───answers───── (OpenSSH) ◄──stdout: answer── (zed-term ◄──answer─────── server
                               ▲                          binary)                  │   ▲
                               │ TB6: env (SSH_ASKPASS, endpoint, token, app PID)  │   │ TB5:
                               └───────────── spawned by the app ──────────────────┤   │ dialogs
                                                                                   │   user
                             TB4: D-Bus Secret Service / Credential Manager        ▼
                                                                         OS keychain, service
                                                                         "zed-term"
ssh's stdout/stderr ─────────── PTY (untrusted, Security 1) ──────────────► terminal pane
```

Trust boundaries:

- **TB1** remote and jump hosts ↔ local ssh: untrusted. A server chooses
  the text of its keyboard-interactive prompts (ssh adds the
  `(user@host) ` prefix itself) and its host key.
- **TB2** ssh ↔ helper: argv carries the prompt, stdout the answer; the
  helper's stderr is ssh's, which is the PTY.
- **TB3** helper ↔ app: a Unix socket or named pipe that other local
  processes could try to reach or to create first.
- **TB4** app ↔ keychain: the OS store, shared with every program the
  user runs.
- **TB5** app ↔ user: dialogs drawn by the app, next to a terminal grid
  that the remote side writes into.
- **TB6** app → ssh's environment: readable through
  `/proc/<pid>/environ` by processes of the same user.

Out of scope: the SSH protocol and its crypto (OpenSSH's), `~/.ssh/config`
commands like `ProxyCommand` (the user's own config), and an attacker with
root.

## 2. Data classification

| Data | Category (GDPR) | Special constraints | Storage | Retention |
|---|---|---|---|---|
| SSH passwords, key passphrases | Art. 4(1) personal data: credentials of the user's accounts; not Art. 9 | Secret: keychain only (architecture §Security 3); never in the PTY, env, argv, logs, `config.yaml`, `session.json` | OS keychain, service `zed-term` | Until "Forget" or replaced |
| Keychain item names: `user@host:port`, key paths | Art. 4(1): which servers and accounts the user uses | Readable item metadata in some keychains, also while locked | OS keychain attributes | With the secret |
| Prompt text, typed one-time codes | Art. 4(1) where it names a user or host; codes are credentials | Untrusted where the server writes it; never stored | Memory only | Until answered |
| Per-connection token, endpoint name | Not personal data | A capability: whoever has it may ask the app | Memory, ssh's environment | ssh process lifetime |
| Host keys and fingerprints | Not personal data (server identity) | Decided by the user only | ssh's `known_hosts` | ssh's |

No sector law applies: architecture §Ground rules names none, and the
data never leaves the user's device (the household case: no controller
other than the user processes it).

## 3. STRIDE walkthrough

Impact and likelihood are H/M/L. Likelihood is judged with the mitigation
in place where it is a design decision of spec 0006, and without it where
the mitigation is new.

| ID | STRIDE | Threat | Affected element | Impact | Likelihood | Mitigation | Status |
|---|---|---|---|---|---|---|---|
| TM-askpass-01 | Spoofing | A process of another OS user connects to the askpass endpoint to fetch stored passwords or to answer prompts. | TB3 endpoint | H | L | Unix: the socket lives in a per-run directory created with mode 0700 under `$XDG_RUNTIME_DIR`, and the server drops any peer whose `SO_PEERCRED` uid isn't the app's; without `$XDG_RUNTIME_DIR` there is no askpass and ssh asks in the terminal as today. Windows: the pipe's DACL grants only the current user's SID. In `src/ssh/askpass.rs`. | open (T-014) |
| TM-askpass-02 | Spoofing | A squatter creates the endpoint first and plays the app: the helper sends it the prompt and token, and it answers a host-key question "yes" or feeds a wrong password. Windows pipe names are one namespace for all users. | TB3, helper side | H | M | The endpoint name carries 128 random bits per app run; Windows creates the pipe with `FILE_FLAG_FIRST_PIPE_INSTANCE` and fails rather than share it; the helper checks that the endpoint's server process is the app PID from its environment (`SO_PEERCRED` on Unix, `GetNamedPipeServerProcessId` on Windows) before it sends anything, else exits 1. In the helper entry point in `src/ssh/askpass.rs`. | open (T-014) |
| TM-askpass-03 | Spoofing | A local process guesses or replays a token to make the app fill in a password for a fake prompt. | TB3 protocol | H | L | A fresh 128-bit token from `getrandom` per connection, compared in constant time, dropped when that connection's ssh exits; each stored secret is offered once per connection. Reading a live token from `/proc/<ssh pid>/environ` takes code running as the user: TM-askpass-14. | open (T-014) |
| TM-askpass-04 | Spoofing | A server or jump host words a keyboard-interactive prompt so the app fills in the password stored for another target. | TB1 → TB2 prompt text | H | L | Auto-fill only when the whole prompt equals one of the connection's own patterns, built from `ssh -G` at connect time: `<user>@<host>'s password: `, `(<user>@<host>) ` plus a server text of `password:` (any case, optional spaces), and `Enter passphrase for key '<file>': ` for one of its `identityfile`s; `<host>` is `hostkeyalias` if set, else `hostname`, as ssh writes it. ssh writes the prefix itself, so a jump host's prompts name the jump host and get a dialog. Spec 0006 flow 4. | open (T-014) |
| TM-askpass-05 | Spoofing | The remote prints a fake zed-term password prompt into the terminal, and the user types the secret into the PTY. | TB5, PTY → user | M | M | zed-term never asks for a secret in the grid: prompts are dialogs in app chrome over the pane, naming the connection, which nothing in the grid can open, read or fill (architecture §Security 1). A user typing into a fake prompt anyway is a risk every terminal shares. | accepted? (OQ-016) |
| TM-askpass-06 | Spoofing | `SSH_ASKPASS` points at a file someone else can replace, so ssh hands prompts to another program. | TB2 exec | H | L | `SSH_ASKPASS` is the canonical absolute path of the running executable (`std::env::current_exe`, canonicalized), never a name looked up on `PATH`. A copy in a directory the user can write is TM-askpass-14. In `src/ssh/askpass.rs`. | open (T-014) |
| TM-askpass-07 | Tampering | A host-key question gets answered without the user, letting a man in the middle in. | TB1 host keys, TB5 | H | L | Only the TM-askpass-04 patterns are ever answered automatically. A `confirm` hint and every other text open a dialog with ssh's full text, fingerprint included, where Enter and Esc mean No and Yes needs an explicit choice (AC6). | open (T-014) |
| TM-askpass-08 | Tampering | Server-written prompt text with control, direction or invisible characters makes a dialog show another host than the one asking. | TB1 → TB5 dialog | M | M | Dialogs show the prompt as plain text with control characters (but line breaks) and invisible or direction characters stripped, by the title rules of spec 0003, capped at 4 KiB. Matching (TM-askpass-04) uses the raw text, so any such character also stops auto-fill. | open (T-014) |
| TM-askpass-09 | Tampering | A message between helper and app is changed on the way. | TB3 | M | L | Both endpoints are local point-to-point channels with no network transport; reaching one at all needs TM-askpass-01 or -02 to fail. | mitigated by design |
| TM-askpass-10 | Repudiation | The app fills in a stored password silently, so the user can't tell it was used, say after an edit to `~/.ssh/config`. | TB4 → TB3 | L | M | Each time the keychain answers, the pane gets a notice naming the target (never the secret): "Logged in with the password saved for `<target>`". There is no log file to keep. Spec 0006 flow 4. | open (T-014) |
| TM-askpass-11 | Information disclosure | The secret lands in the PTY and its scrollback because the helper writes it, or an error containing it, to stderr, which ssh passes to the terminal. | TB2 helper stderr | H | L | The helper writes the answer to stdout (ssh's pipe) and nothing at all to stderr; any failure exits 1 silently. AC4 checks the PTY output. | open (T-014) |
| TM-askpass-12 | Information disclosure | The secret ends up in ssh's environment or arguments, a log, `config.yaml` or `session.json`. | TB6, TB4 | H | L | Secrets travel only keychain → app → socket → helper stdout; the environment carries the endpoint, token and app PID, never a secret; zed-term never logs askpass traffic, prompts included, since they name users and hosts. AC4. | open (T-014) |
| TM-askpass-13 | Information disclosure | A secret lingers in memory and reaches swap or a core dump. | App and helper memory | M | L | The app holds a secret only between the keychain or the dialog and the socket write, then drops it; the helper lives for one prompt. Overwriting freed memory needs the `zeroize` crate, a new crate for the owner to decide in T-014. | accepted? (OQ-016) |
| TM-askpass-14 | Information disclosure | Code running as the user reads the token from `/proc/<ssh pid>/environ`, talks to the endpoint, asks the Secret Service directly, lists the keychain's item names, or swaps a stored password for another one. | TB4, TB6 | H | M | None zed-term can give: the OS keychain doesn't isolate apps of the same user, on Linux desktops nor in Windows Credential Manager, and that user can already read and change everything zed-term can. | accepted? (OQ-016) |
| TM-askpass-15 | Denial of service | A server sends endless keyboard-interactive prompts, and dialogs pile up. | TB1 → TB5 | L | L | One dialog per connection at a time, the next prompt waits; closing the tab cancels its prompts (their helpers exit 1); ssh's own `NumberOfPasswordPrompts` caps retries. | open (T-014) |
| TM-askpass-16 | Denial of service | An oversized prompt or a helper that connects and stalls exhausts memory or blocks the server. | TB3 server | M | L | Messages are capped at 64 KiB; each connection is served on its own background task with a 30 s read timeout; the server never runs on the UI thread. | open (T-014) |
| TM-askpass-17 | Denial of service | A keychain call blocks the UI: a locked keyring waiting for its unlock dialog, or a hung D-Bus. | TB4 | M | M | Keychain calls run only on the background executor, with a timeout that counts as "nothing stored" and opens the dialog; with no keychain provider, "Remember" is hidden and one notice explains it (flow 8). | open (T-014) |
| TM-askpass-18 | Elevation of privilege | The binary takes helper mode by accident, or runs app code (GPUI, the config) as ssh's child. | TB2 helper entry | M | L | `main` checks for helper mode first, only when both the endpoint and the token variables are set, and the helper exits before any GPUI or config code; the variables go only into the ssh process's environment, never into local panes. | open (T-014) |
| TM-askpass-19 | Elevation of privilege | A jump host's ssh or a `ProxyCommand` inherits the token and gets the app to answer for it as if it were the connection's target. | TB6 → TB3 | M | L | A token identifies a connection, not a target: whether a prompt is answered from the keychain depends only on its text matching the connection's own patterns (TM-askpass-04); everything else is a dialog. | open (T-014) |
| TM-askpass-20 | Elevation of privilege | Resolving a host runs commands from `~/.ssh/config` (`Match exec`) when the user only browsed the host picker. | `ssh -G` | M | L | The picker lists hosts with zed-term's own parser, which runs nothing (T-013); `ssh -G` runs only when the user connects, the moment ssh itself would run them. | open (T-014) |

Every boundary of the sketch appears: TB1 in -04, -07, -08, -15; TB2 in
-06, -11, -18; TB3 in -01, -02, -03, -09, -16, -19; TB4 in -10, -14, -17;
TB5 in -05, -07, -08; TB6 in -12, -14, -19, -20.

## 4. Mitigation tasks

- [ ] T-014 — builds the askpass path to spec 0006 as updated by this model
  (covers TM-askpass-01, -02, -03, -04, -06, -07, -08, -10, -11, -12, -15,
  -16, -17, -18, -19, -20; decides `zeroize` for -13)
- [ ] T-011 — checks the Windows half on Windows 11: the pipe's DACL,
  `FILE_FLAG_FIRST_PIPE_INSTANCE` and the server-PID check (covers
  TM-askpass-01, -02)

## 5. Accepted risks

Human sign-off required; until then they wait in
[OQ-016](../open-questions.md).

- TM-askpass-05: a user typing a secret into a prompt the remote printed.
- TM-askpass-13: secrets in memory between use and drop, if `zeroize` is
  declined.
- TM-askpass-14: code running as the user.
