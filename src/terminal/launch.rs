//! Starting a pane's program: at first, at Enter in a waiting pane, and
//! again when an ended SSH connection reconnects (spec 0006).

use super::pty::{self, GridSize};
use super::{Asker, Running, State, TerminalPane, answer_prompts};
use crate::ssh::{self, askpass};
use crate::timing::Timing;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use futures::StreamExt;
use gpui::{Context, Window};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// What starts a pane's program: at first, at Enter in a waiting pane, and
/// again when an ended connection reconnects.
pub struct Launch {
    pub command: pty::Command,
    pub cwd: Option<PathBuf>,
    pub timing: Arc<Timing>,
}

/// How a pane begins.
pub enum Start {
    /// With its program started before the window existed, or the reason it
    /// couldn't start: the boot pane (spec 0001).
    Started(Result<pty::Session, String>),
    /// Starting its program now, in a grid of this size.
    Now(GridSize),
    /// Showing this until Enter starts the program: an SSH tab opened at
    /// boot or restored never connects by itself (spec 0006, flow 6).
    OnEnter(String),
}

/// What a waiting SSH pane shows (spec 0006, flow 6).
pub fn connect_prompt(label: &str) -> String {
    format!("Press Enter to connect to {label}")
}

/// Why `command` didn't start, as its pane says it. A missing `ssh` comes
/// with how to install it (spec 0006, flow 8).
fn failure(command: &pty::Command, error: &str) -> String {
    if command.program == ssh::PROGRAM && !ssh::installed() {
        return ssh::not_installed();
    }
    format!("Could not start {}: {error}", command.describe())
}

impl TerminalPane {
    /// Shows `session`'s program, or why it couldn't start, and forwards its
    /// events to the pane.
    pub(super) fn attach(&mut self, session: Result<pty::Session, String>, cx: &mut Context<Self>) {
        let session = match session {
            Ok(session) => session,
            Err(error) => {
                let message = failure(&self.launch.command, &error);
                self.notice(message.clone(), cx);
                self.state = State::Failed(message);
                self.end_prompts(cx);
                return;
            }
        };
        let mut events = session.events;
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let mut batch = vec![event];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                let alive = this.update(cx, |pane, cx| {
                    for event in batch {
                        pane.handle_event(event, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        // The cursor the config asks for, from the first frame on.
        session.term.lock().set_options(self.settings.term_config());
        self.state = State::Running(Running {
            term: session.term,
            proxy: session.proxy,
            notifier: session.notifier,
            size: session.size,
            child_pid: session.child_pid,
            reported: session.reported,
        });
    }

    /// Starts the program of a waiting pane, sized to the window until the
    /// next frame measures the pane.
    pub(super) fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.settings.font.estimated_cell(),
        );
        self.spawn(grid, window, cx);
        cx.notify();
    }

    /// Starts the program in a new PTY of `grid`.
    pub(super) fn spawn(&mut self, grid: GridSize, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.connection_command(window, cx);
        let session = pty::spawn(
            &command,
            grid,
            self.launch.cwd.as_deref(),
            self.settings.scrollback,
            self.profile.ssh,
            Arc::clone(&self.launch.timing),
        );
        self.attach(session, cx);
    }

    /// The command to start now. An SSH connection's gets the askpass
    /// environment of a new token, and the pane answers its prompts until
    /// the next start (spec 0006, flow 2). Without an askpass server, ssh
    /// asks in the terminal, with a notice saying why.
    fn connection_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> pty::Command {
        let mut command = self.launch.command.clone();
        self.end_prompts(cx);
        if self.profile.ssh {
            match askpass::connect(cx) {
                Ok(connection) => {
                    command.env.extend(connection.env);
                    let args = command.args.clone();
                    let prompts = connection.prompts;
                    let turn = Rc::clone(&self.dialog_turn);
                    let task = answer_prompts(prompts, args, Asker::Terminal, turn, window, cx);
                    self.askpass = Some(task);
                }
                Err(error) => self.notice(format!("ssh asks in the terminal: {error}"), cx),
            }
        }
        command
    }

    /// The remote folder an SSH pane's shell last reported with OSC 7,
    /// where its SFTP sidebar starts (spec 0008, Model). Untrusted text,
    /// held in memory only.
    pub fn remote_cwd(&self) -> Option<String> {
        match &self.state {
            State::Running(running) => running.reported.remote(),
            _ => None,
        }
    }

    /// `user@host` of an SSH pane, as its SFTP sidebar names it.
    pub fn ssh_target(&self) -> Option<String> {
        self.profile
            .ssh
            .then(|| ssh::target_label(&self.launch.command.args))
    }

    /// What starts a new SFTP connection to this SSH pane's host, with the
    /// same options as the pane (spec 0008, Model). It gets a token of its
    /// own, and its prompts open this pane's dialog, marked as the SFTP
    /// connection's; a new one ends the last one's prompts. Without an
    /// askpass server ssh can't ask at all, so it runs in batch mode and
    /// fails rather than wait on a terminal it doesn't have.
    pub fn sftp_command(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<pty::Command> {
        if !self.profile.ssh {
            return None;
        }
        let pane_args = self.launch.command.args.clone();
        let mut args = ssh::sftp_args(&pane_args)?;
        self.sftp_askpass = None;
        self.close_dialog(Asker::Sftp, cx);
        let mut env = self.launch.command.env.clone();
        match askpass::connect(cx) {
            Ok(connection) => {
                env.extend(connection.env);
                let prompts = connection.prompts;
                let turn = Rc::clone(&self.dialog_turn);
                let task = answer_prompts(prompts, pane_args, Asker::Sftp, turn, window, cx);
                self.sftp_askpass = Some(task);
            }
            Err(_) => {
                args.insert(0, "BatchMode=yes".to_owned());
                args.insert(0, "-o".to_owned());
            }
        }
        Some(pty::Command {
            program: self.launch.command.program.clone(),
            args,
            env,
        })
    }

    /// This SSH pane's host as port forwards key it: its ssh arguments, and
    /// the `user@host` label (spec 0017, Model).
    pub fn forward_host(&self) -> Option<crate::forwards::registry::Host> {
        let label = self.ssh_target()?;
        Some(crate::forwards::registry::Host {
            args: self.launch.command.args.clone(),
            label,
        })
    }

    /// What starts port forward `id` to this SSH pane's host (spec 0017,
    /// Command), with a token of its own: its prompts open this pane's
    /// dialog, named with `forward`'s summary (flow 4). A new start of the
    /// same forward ends the last one's prompts. Without an askpass server
    /// it runs in batch mode, like the SFTP connection.
    pub fn forward_command(
        &mut self,
        id: crate::forwards::Id,
        forward: &crate::forwards::Forward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<pty::Command> {
        if !self.profile.ssh {
            return None;
        }
        let pane_args = self.launch.command.args.clone();
        let mut args = ssh::forward_args(&pane_args, forward)?;
        self.forward_askpass.remove(&id);
        self.close_dialog(Asker::Forward(id), cx);
        let mut env = self.launch.command.env.clone();
        match askpass::connect(cx) {
            Ok(connection) => {
                env.extend(connection.env);
                let prompts = connection.prompts;
                let turn = Rc::clone(&self.dialog_turn);
                let asker = Asker::Forward(id);
                let task = answer_prompts(prompts, pane_args, asker, turn, window, cx);
                self.forward_askpass
                    .insert(id, (forward.summary().into(), task));
            }
            Err(_) => {
                args.insert(0, "BatchMode=yes".to_owned());
                args.insert(0, "-o".to_owned());
            }
        }
        Some(pty::Command {
            program: self.launch.command.program.clone(),
            args,
            env,
        })
    }

    /// Starts the program again in the same terminal, after its SSH
    /// connection dropped: the old session stays in the scrollback. The new
    /// connection gets a token of its own.
    pub(super) fn rerun(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, State::Running(_)) {
            return;
        }
        let command = self.connection_command(window, cx);
        let State::Running(running) = &mut self.state else {
            return;
        };
        let started = pty::rerun(
            &command,
            running.size,
            self.launch.cwd.as_deref(),
            &running.term,
            &running.proxy,
            &running.reported,
        );
        let failed = match started {
            Ok((notifier, child_pid)) => {
                running.notifier = notifier;
                running.child_pid = child_pid;
                self.exited = false;
                self.reconnect = false;
                false
            }
            Err(error) => {
                let notice = format!("\r\n{}", failure(&command, &error));
                let mut parser = Processor::<StdSyncHandler>::new();
                parser.advance(&mut *running.term.lock(), notice.as_bytes());
                true
            }
        };
        if failed {
            self.end_prompts(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0006, flow 6: a restored SSH tab names the host it waits for.
    #[test]
    fn a_waiting_ssh_pane_asks_for_enter_naming_its_host() {
        assert_eq!(
            connect_prompt("Production DB"),
            "Press Enter to connect to Production DB"
        );
    }
}
