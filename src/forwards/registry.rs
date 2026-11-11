//! The app's port forwards (spec 0017, Model): one registry per app run,
//! owned by the workspace and created with the first forward, held in
//! memory only. Forwards belong to a host, not a pane: every SSH pane of a
//! host shows that host's forwards, and they outlive its tabs until
//! deleted or Tern quits (flows 6–8). Views observe it and redraw.

use super::process::Process;
use super::{Forward, Id};
use crate::terminal::pty::Command;
use gpui::{Context, SharedString, Task};
use std::time::{Duration, Instant};

/// How long ssh must run without exiting before its forward counts as up,
/// counted only while none of its prompts is open (spec 0017, Up;
/// TM-fwd-10).
pub const UP_AFTER: Duration = Duration::from_secs(2);

/// A host's identity: the pane's ssh arguments, which `~/.ssh/config` hosts
/// work through too, and the `user@host` label it's named by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub args: Vec<String>,
    pub label: String,
}

/// Where a forward is (spec 0017, Model).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Up,
    /// ssh's reason, plain text, or "cancelled".
    Failed(SharedString),
}

/// One forward the registry holds.
pub struct Entry {
    pub id: Id,
    pub host: Host,
    pub forward: Forward,
    pub state: State,
    process: Option<Process>,
    /// Counts the starts, so a late exit of an earlier one is ignored.
    run: u64,
    /// A prompt of this forward's ssh is open: the up clock waits.
    prompting: bool,
    /// When the up clock last started.
    clock: Instant,
    _exit: Option<Task<()>>,
}

#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    next: Id,
}

impl Registry {
    /// A registry that stops every forward when the app quits (flow 8).
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.on_app_quit(|this, _| {
            this.stop_all();
            async {}
        })
        .detach();
        Self::default()
    }

    /// The ID the next forward gets: its ssh's prompts are marked with it
    /// before it's added.
    pub fn next_id(&mut self) -> Id {
        self.next += 1;
        self.next
    }

    /// The forwards of `host`, oldest first.
    pub fn of<'a>(&'a self, host: &'a [String]) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.host.args == host)
    }

    /// Every forward, oldest first.
    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: Id) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Whether `host` already has a forward listening where `forward`
    /// would (flow 2).
    pub fn clashes(&self, host: &[String], forward: &Forward) -> bool {
        self.of(host).any(|entry| entry.forward.clashes(forward))
    }

    /// Adds forward `id` and starts `command`, its ssh (flow 2).
    pub fn add(
        &mut self,
        id: Id,
        host: Host,
        forward: Forward,
        command: &Command,
        cx: &mut Context<Self>,
    ) {
        self.entries.push(Entry {
            id,
            host,
            forward,
            state: State::Starting,
            process: None,
            run: 0,
            prompting: false,
            clock: Instant::now(),
            _exit: None,
        });
        self.start(id, command, cx);
    }

    /// Starts a failed forward's ssh again (flow 5).
    pub fn retry(&mut self, id: Id, command: &Command, cx: &mut Context<Self>) {
        if self
            .get(id)
            .is_some_and(|entry| matches!(entry.state, State::Failed(_)))
        {
            self.start(id, command, cx);
        }
    }

    /// Stops a forward's ssh and forgets it (flow 6).
    pub fn remove(&mut self, id: Id, cx: &mut Context<Self>) {
        self.entries.retain(|entry| entry.id != id);
        cx.notify();
    }

    /// Stops every forward's ssh (flow 8).
    pub fn stop_all(&mut self) {
        self.entries.clear();
    }

    /// A prompt of forward `id`'s ssh opened or closed. Closing restarts
    /// the up clock (TM-fwd-10).
    pub fn prompting(&mut self, id: Id, open: bool, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(id) else {
            return;
        };
        entry.prompting = open;
        if !open {
            entry.clock = Instant::now();
            let run = entry.run;
            self.check_up_later(id, run, cx);
        }
    }

    /// The user declined one of forward `id`'s prompts (flow 4).
    pub fn cancelled(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Some(entry) = self.entry(id).filter(|e| e.state == State::Starting) {
            entry.fail("cancelled".into());
            cx.notify();
        }
    }

    fn entry(&mut self, id: Id) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
    }

    fn start(&mut self, id: Id, command: &Command, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(id) else {
            return;
        };
        entry.run += 1;
        let run = entry.run;
        entry.prompting = false;
        entry.clock = Instant::now();
        // The old process, if any, goes before the new one binds.
        entry.process = None;
        match Process::spawn(command) {
            Ok((process, exit)) => {
                entry.process = Some(process);
                entry.state = State::Starting;
                entry._exit = Some(cx.spawn(async move |this, cx| {
                    if let Ok(reason) = exit.await {
                        let _ = this.update(cx, |this, cx| this.exited(id, run, reason, cx));
                    }
                }));
                self.check_up_later(id, run, cx);
            }
            Err(error) => entry.fail(format!("Couldn't start ssh: {error}").into()),
        }
        cx.notify();
    }

    /// Looks again once [`UP_AFTER`] has passed.
    fn check_up_later(&self, id: Id, run: u64, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(UP_AFTER);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| this.check_up(id, run, cx));
        })
        .detach();
    }

    fn check_up(&mut self, id: Id, run: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(id) else {
            return;
        };
        let due = entry.run == run
            && entry.state == State::Starting
            && !entry.prompting
            && entry.clock.elapsed() >= UP_AFTER;
        if due {
            entry.state = State::Up;
            cx.notify();
        }
    }

    /// ssh of start `run` exited: the forward failed, whenever it was
    /// (flow 5). Nothing reconnects by itself.
    fn exited(&mut self, id: Id, run: u64, reason: String, cx: &mut Context<Self>) {
        // No process: it was cancelled, and its reason stays.
        let current = |entry: &&mut Entry| entry.run == run && entry.process.is_some();
        if let Some(entry) = self.entry(id).filter(current) {
            entry.fail(reason.into());
            cx.notify();
        }
    }
}

impl Entry {
    /// Marks it failed and stops its ssh, if it still runs; its exit is
    /// then ignored.
    fn fail(&mut self, reason: SharedString) {
        self.process = None;
        self.state = State::Failed(reason);
    }
}

/// The registry against a fake ssh (`sh -c`). The up clock is wall time, so
/// the tests that reach "up" wait [`UP_AFTER`] for real, and the forwards'
/// threads wake the test's tasks from outside GPUI's test scheduler, so
/// each test allows it to park.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use crate::forwards::Kind;
    use crate::forwards::process::tests::spawned::{alive, read_pid, sh};
    use gpui::{AppContext as _, Entity, TestAppContext};

    const MS: Duration = Duration::from_millis(1);

    fn host(name: &str) -> Host {
        Host {
            args: vec!["-l".into(), "me".into(), name.into()],
            label: format!("me@{name}"),
        }
    }

    fn socks(port: u16) -> Forward {
        Forward::parse(Kind::Socks, &port.to_string(), "", false).expect("a valid forward")
    }

    /// A fake ssh that writes its PID to `pidfile`, then runs until killed.
    fn running(pidfile: &std::path::Path) -> Command {
        sh(&format!("echo $$ > '{}'; exec sleep 30", pidfile.display()))
    }

    fn registry(cx: &mut TestAppContext) -> Entity<Registry> {
        cx.executor().allow_parking();
        cx.new(Registry::new)
    }

    fn add(registry: &Entity<Registry>, cx: &mut TestAppContext, command: &Command) -> Id {
        add_to(registry, cx, host("bastion"), socks(1080), command)
    }

    fn add_to(
        registry: &Entity<Registry>,
        cx: &mut TestAppContext,
        host: Host,
        forward: Forward,
        command: &Command,
    ) -> Id {
        registry.update(cx, |registry, cx| {
            let id = registry.next_id();
            registry.add(id, host, forward, command, cx);
            id
        })
    }

    fn state(registry: &Entity<Registry>, cx: &mut TestAppContext, id: Id) -> Option<State> {
        cx.run_until_parked();
        registry.read_with(cx, |registry, _| registry.get(id).map(|e| e.state.clone()))
    }

    /// Moves the test clock on by `by`, so the up check's timer fires.
    fn advance(cx: &mut TestAppContext, by: Duration) {
        cx.executor().advance_clock(by);
        cx.run_until_parked();
    }

    /// The forward's state once it has failed, waiting for ssh's exit in
    /// real time.
    fn failed(registry: &Entity<Registry>, cx: &mut TestAppContext, id: Id) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(State::Failed(reason)) = state(registry, cx, id) {
                return reason.to_string();
            }
            assert!(Instant::now() < deadline, "forward {id} never failed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Flow 5, AC4: a port that's taken makes ssh exit at once
    /// (`ExitOnForwardFailure`), and the forward fails with the line
    /// naming it.
    #[gpui::test]
    fn ssh_exiting_fails_the_forward_with_its_reason(cx: &mut TestAppContext) {
        let registry = registry(cx);
        let id = add(
            &registry,
            cx,
            &sh(
                "printf 'debug1: hi\\nbind [127.0.0.1]:15432: Address already in use\\n\
                 channel_setup_fwd_listener_tcpip: cannot listen to port: 15432\\n\
                 Could not request local forwarding.\\n' >&2; exit 255",
            ),
        );
        let reason = failed(&registry, cx, id);
        assert!(
            reason.contains("bind [127.0.0.1]:15432: Address already in use"),
            "{reason}"
        );
        assert!(!reason.contains("debug1"), "{reason}");
    }

    /// A missing ssh fails the forward instead of leaving it starting.
    #[gpui::test]
    fn ssh_that_cannot_start_fails_the_forward(cx: &mut TestAppContext) {
        let registry = registry(cx);
        let missing = Command {
            program: "/nonexistent/tern-test-ssh".into(),
            ..sh("")
        };
        let id = add(&registry, cx, &missing);
        assert!(matches!(state(&registry, cx, id), Some(State::Failed(_))));
    }

    /// Spec 0017, Up, and flow 5: a forward is up once ssh has run
    /// `UP_AFTER` without exiting, not before; ssh exiting later (the
    /// connection lost) still fails it.
    #[gpui::test]
    fn a_forward_is_up_after_two_seconds_and_fails_when_ssh_exits_later(cx: &mut TestAppContext) {
        let registry = registry(cx);
        let id = add(
            &registry,
            cx,
            &sh("sleep 3; echo 'Connection to bastion closed by remote host.' >&2; exit 255"),
        );
        assert_eq!(state(&registry, cx, id), Some(State::Starting));
        advance(cx, UP_AFTER - MS);
        assert_eq!(state(&registry, cx, id), Some(State::Starting));
        std::thread::sleep(UP_AFTER);
        advance(cx, MS);
        assert_eq!(state(&registry, cx, id), Some(State::Up));
        assert_eq!(
            failed(&registry, cx, id),
            "Connection to bastion closed by remote host."
        );
    }

    /// TM-fwd-10: the up clock stands still while a prompt of the forward
    /// is open and starts over when it closes.
    #[gpui::test]
    fn an_open_prompt_keeps_the_forward_starting(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-prompt");
        let registry = registry(cx);
        let id = add(&registry, cx, &running(&dir.path().join("pid")));
        registry.update(cx, |registry, cx| registry.prompting(id, true, cx));
        std::thread::sleep(UP_AFTER);
        advance(cx, UP_AFTER);
        assert_eq!(state(&registry, cx, id), Some(State::Starting));

        registry.update(cx, |registry, cx| registry.prompting(id, false, cx));
        advance(cx, UP_AFTER - MS);
        assert_eq!(state(&registry, cx, id), Some(State::Starting));
        std::thread::sleep(UP_AFTER);
        advance(cx, MS);
        assert_eq!(state(&registry, cx, id), Some(State::Up));
    }

    /// Flow 4, AC8: Esc on a forward's prompt fails it with "cancelled"
    /// and ends its ssh, whose exit doesn't overwrite the reason.
    #[gpui::test]
    fn a_cancelled_prompt_fails_the_forward_as_cancelled(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-cancel");
        let pidfile = dir.path().join("pid");
        let registry = registry(cx);
        let id = add(
            &registry,
            cx,
            &sh(&format!(
                "echo 'Permission denied' >&2; echo $$ > '{}'; exec sleep 30",
                pidfile.display()
            )),
        );
        let pid = read_pid(&pidfile);
        registry.update(cx, |registry, cx| registry.cancelled(id, cx));
        assert_eq!(
            state(&registry, cx, id),
            Some(State::Failed("cancelled".into()))
        );
        assert!(!alive(&pid), "ssh {pid} still runs");
        // Its exit reaches the registry from the forward's thread.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            state(&registry, cx, id),
            Some(State::Failed("cancelled".into()))
        );
    }

    /// Flow 6, AC5: Delete removes the forward and its ssh is gone.
    #[gpui::test]
    fn removing_a_forward_ends_its_ssh(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-remove");
        let pidfile = dir.path().join("pid");
        let registry = registry(cx);
        let id = add(&registry, cx, &running(&pidfile));
        let pid = read_pid(&pidfile);
        assert!(alive(&pid));
        registry.update(cx, |registry, cx| registry.remove(id, cx));
        assert!(!alive(&pid), "ssh {pid} still runs");
        assert_eq!(state(&registry, cx, id), None);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(state(&registry, cx, id), None, "its exit brought it back");
    }

    /// Flow 8, AC7: stopping all (as quitting does) ends every forward's
    /// ssh, of every host.
    #[gpui::test]
    fn stopping_all_ends_every_forwards_ssh(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-stop-all");
        let registry = registry(cx);
        let pids: Vec<String> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let pidfile = dir.path().join(name);
                add_to(&registry, cx, host(name), socks(1080), &running(&pidfile));
                read_pid(&pidfile)
            })
            .collect();
        registry.update(cx, |registry, _| registry.stop_all());
        for pid in pids {
            assert!(!alive(&pid), "ssh {pid} still runs");
        }
        assert!(registry.read_with(cx, |registry, _| registry.all().is_empty()));
    }

    /// Flow 8, AC7: Tern quitting stops every forward.
    #[gpui::test]
    fn quitting_ends_every_forwards_ssh(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-quit");
        let pidfile = dir.path().join("pid");
        let registry = registry(cx);
        add(&registry, cx, &running(&pidfile));
        let pid = read_pid(&pidfile);
        cx.quit();
        assert!(!alive(&pid), "ssh {pid} still runs");
    }

    /// Flow 5, AC4: Retry starts a failed forward's ssh again; a forward
    /// that's starting or up isn't started twice.
    #[gpui::test]
    fn retry_restarts_only_a_failed_forward(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-retry");
        let registry = registry(cx);
        let id = add(&registry, cx, &sh("exit 255"));
        failed(&registry, cx, id);

        let first = dir.path().join("first");
        registry.update(cx, |registry, cx| registry.retry(id, &running(&first), cx));
        assert_eq!(state(&registry, cx, id), Some(State::Starting));
        let pid = read_pid(&first);

        let second = dir.path().join("second");
        registry.update(cx, |registry, cx| registry.retry(id, &running(&second), cx));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(state(&registry, cx, id), Some(State::Starting));
        assert!(!second.exists(), "a starting forward was started again");
        assert!(alive(&pid), "retry stopped the running ssh {pid}");
    }

    /// Model and flow 2: forwards belong to a host; one host's listen
    /// port doesn't block another host's.
    #[gpui::test]
    fn forwards_are_listed_and_clash_per_host(cx: &mut TestAppContext) {
        let dir = TempDir::new("forward-registry-hosts");
        let registry = registry(cx);
        add_to(
            &registry,
            cx,
            host("a"),
            socks(1080),
            &running(&dir.path().join("pid")),
        );
        registry.read_with(cx, |registry, _| {
            assert_eq!(registry.of(&host("a").args).count(), 1);
            assert_eq!(registry.of(&host("b").args).count(), 0);
            assert!(registry.clashes(&host("a").args, &socks(1080)));
            assert!(!registry.clashes(&host("b").args, &socks(1080)));
            assert!(!registry.clashes(&host("a").args, &socks(1081)));
        });
    }
}
