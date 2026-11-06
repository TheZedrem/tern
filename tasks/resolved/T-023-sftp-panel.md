---
status: now
blocked-by: []
---
# T-023: SFTP sidebar, connection and listing

Step 1 of spec 0008. The sidebar (`sftp.toggle`, `Ctrl+Shift+S`;
`sftp_sidebar: left | right`, inside a tab list on the same side), open or
closed per SSH pane and shown only while that pane is focused, with its
width dragged on the inner edge. One session per SSH pane on its own
thread running `russh-sftp` over `ssh -s <host> sftp` on a current-thread
tokio runtime, prompts through askpass as in spec 0006, failures with
"Connect again". The flat listing with its always-editable path field,
starting in the pane's reported cwd, else home. Adds the new crates
(`russh-sftp`, `tokio` as a direct dependency) and `sftp_sidebar` on the
settings screen. Carries TM-sftp-01, -06, -14 to -17 and -19. Area: src.

Spec: ../../spec/0008-sftp-panel.md (flows 1, 2, 7; AC1–3, 10's failure half)

## Result (2026-11-06)

Boot path: only the sidebar's four key bindings, registered with the
workspace's; nothing SFTP runs before the sidebar opens. Release
`--bench-startup 10`: warm median 155.8 ms, cold 336.9 ms (budget 500 ms).
Binary 33.8 MB with `russh-sftp` and tokio.

Built: `src/sftp/` (`client.rs`: the connection thread on `RawSftpSession`,
30 s request timeout, 50,000-entry cap, links resolved in batches of 64,
ssh's stderr tail as the reason; `view.rs`: header with `user@host`,
refresh and close, the path field, a virtualized flat listing with
arrow/Enter/Backspace and double-click, "Connect again"; `mod.rs`: paths,
sorting, sizes, dates). The workspace keeps a sidebar per pane, draws the
active pane's if open, inside a same-side tab list, resizable 200 px to
half the window. The pane gives each SFTP connection its own askpass token;
its prompts open the pane's dialog as "<profile> (SFTP)", taking turns with
the terminal's (spec 0008 flow 1 updated to say so). Without an askpass
server the SFTP ssh runs in batch mode instead of hanging. `sftp_sidebar`
is on the settings screen (Appearance → SFTP) and in the template; spec
0004's key table and spec 0005's schema list it. tokio got `macros` for
`select!`.

Verified: 476 tests, 21 new ones written by a separate agent against the
spec, among them the client against OpenSSH's `sftp-server` on stdio
(listing, links, a failed listing that keeps the connection, the 50,000
cap) and an ssh that can't connect ending with ssh's own words. They found
one bug, fixed: a long stderr lost its end. A debug build with a fake `ssh`
on `PATH` opened the sidebar and connected.

Not verified: the window itself (COSMIC's screenshot needs the portal's
dialog; the owner looks), a real host with a password prompt, Windows
(T-011). Start folder is home until T-022 reads OSC 7.
