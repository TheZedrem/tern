---
status: implemented
phase: 5
tasks: [T-100, T-101, T-102, T-103]
related: [spec 0003, spec 0008, spec 0013, spec 0016, spec 0018, OQ-040]
---

# Docker container and Compose actions

## Problem & goal

Go from a container or project in the Docker sidebar to an investigation
on the same machine without retyping its name, host or source path.

## Scope

Shell, Inspect and Follow logs belong to container rows. Edit Compose
belongs to a project row. Container lifecycle and Compose deployment
controls are later decisions.

## Design

### Affected areas and model

The Docker plugin owns Docker behavior. Tern owns terminal panes and its
Local/SFTP edit workflow. Spec 0016 exposes confirmed, target-bound split and file-edit buttons.
A container summary opens first. Selecting an action shows its command before confirmation.

### Flows

1. Shell starts a new interactive process inside the selected container
   in a new terminal split. Choose a shell appropriate to the container
   OS. Report a missing shell and allow another executable to be chosen.
2. Inspect shows image, state, health when available, published ports,
   networks and mounts. Full inspection JSON opens on explicit request.
3. Edit Compose opens the reported source file in the text editor.
   With multiple files, show all paths in merge order and let the user
   pick one. SSH edits follow spec 0008 upload and conflict handling.
   An unavailable path stays visible with its host and the reason.
4. Follow logs opens a separately titled terminal split with the latest
   100 lines and follows new output. Closing it ends that follower.

### Startup impact

Actions run only on invocation. Session restore never reconnects a
container shell or restarts a log stream automatically.

### Platform notes

Windows 11 Docker Desktop and local Linux use their selected local
engine. SSH actions preserve the pane's destination and connection
options. Paths reported by Compose belong to the machine or environment
that submitted the project and may be inaccessible from the SSH host.

### Security & privacy

Bind each action to the engine and container ID or selected project
path at invocation. An active-pane change cannot redirect a pending
confirmation. Container metadata and log bytes are untrusted. Keep
inspect and log output in memory. Full inspection can expose environment
secrets. Editing uses the existing file-edit lifetime and protections.

## Acceptance criteria

1. With identically named containers on two hosts, opening a shell then
   switching hosts before confirmation cannot open a shell on the second.
2. Exiting the shell leaves the container's main process running. An
   image without the chosen shell reports the failure in the new pane.
3. Inspect works for local and SSH containers, offers full JSON on
   request and reports when the selected ID has disappeared.
4. Logs shows an existing line and a newly emitted line. Closing the
   pane leaves the container running and ends the follower.
5. A project with a base and override file offers both exact paths.
   Saving a Compose edit never recreates services automatically.
6. A remote file changed after download triggers the normal SFTP conflict
   check. An inaccessible remote path never opens a local namesake.
7. Relaunching Tern after using Shell or Logs starts neither command.

## Open questions

The owner resolved OQ-040 in favor of terminal splits for Shell and Follow logs.
