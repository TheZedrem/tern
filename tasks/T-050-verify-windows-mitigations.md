---
status: next
blocked-by: []
---
# T-050: Check the Windows security mitigations on Windows 11

The Windows half of the threat models, split out of T-011 because daily use
doesn't exercise it. On Windows 11: the askpass pipe's DACL
(`D:P(A;;GA;;;OW)`) lets only the user open it; a second Tern opens its own
pipe; a squatted pipe name fails the start (`FILE_FLAG_FIRST_PIPE_INSTANCE`)
and a helper pointed at another process's pipe exits 1 (TM-askpass-01,
-02). A download named `x.txt:evil`, `CON`, `aux.txt` or with a trailing
dot or space lands under its escaped name (TM-sftp-04), an edit copy named like `run.CMD` or with an extension from `PATHEXT` is downloaded but not opened (TM-sftp-05), and downloads and
edit copies carry `Zone.Identifier` with `ZoneId=3` (TM-sftp-07). Fix what
fails. Area: src.

Spec: ../spec/threat-model/0001-ssh-askpass.md
Spec: ../spec/threat-model/0003-sftp.md
