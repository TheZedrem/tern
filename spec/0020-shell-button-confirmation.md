---
status: implemented
phase: 5
tasks: [T-104]
related: [spec 0016, spec 0019]
---

# Confirm a Docker shell from its preview

## Problem & goal

The Docker sidebar already shows the host, container and exact command. Clicking
Shell there currently repeats the command in a confirmation bar on the terminal.
The click on that preview should authorize one shell split.

## Scope

This amendment covers the Shell command preview in spec 0019. The first Shell
button in the container summary opens that preview so the user can choose the
executable. Other Docker actions retain their spec 0019 flows.

## Design

### Affected areas and model

The plugin requests confirmation at its command button. Tern owns the button,
the displayed arguments and the activation. A plugin message or a button label
alone cannot authorize a split.

### Flows

1. Select Shell in the container summary. Review the command or submit an
   alternate executable to update the preview.
2. Activate Shell on the displayed preview. Tern checks the granted rights,
   target revision and command, then opens one interactive split immediately.
3. If the target or command changes, invalidate the old activation. If the
   split cannot open, show the reason.

### Startup impact

All work happens while the panel is open.

### Platform notes

Windows Docker Desktop, local Linux and SSH use the same activation flow and
the existing command construction for their respective targets.

### Security & privacy

The host draws the complete command and applies the existing command-button
click guard after every preview change. Activation grants one execution of
those arguments on that target. Tern stores no new permission or terminal data.

## Acceptance criteria

1. Opening the preview starts no process.
2. A single Shell click in the settled preview opens one split without an Allow bar.
3. Keyboard activation of that same focused button has the same effect.
4. Changing the executable updates the preview and resets its click guard.
5. Revoking the split permission prevents execution.
6. Switching between local and SSH panes invalidates the previous target's preview.
7. Receiving a panel update without user activation never starts a shell.
