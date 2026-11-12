//! Plugins (spec 0016): standalone programs in
//! `<data_local_dir>/tern/plugins/<id>/` that get events as JSON lines on
//! stdin and answer with actions on stdout. The [`Host`] finds them after
//! the first frame, starts an enabled one with its first event, hands it
//! one event at a time, and passes what it asks for on to the workspace.
//! It gates the protocol, not the process: a plugin runs as the user
//! (OQ-031).

mod manifest;
mod process;
mod protocol;

pub use manifest::{Right, Subscription};
pub use protocol::{Action, PaneInfo};

use crate::config::PluginSettings;
use futures::StreamExt;
use gpui::{AppContext, Context, EventEmitter};
use manifest::{Found, Manifest};
use process::{Message, Process};
use protocol::{Event, Outcome, Pane};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::task::Poll;
use std::time::{Duration, Instant};

/// How many events wait for a plugin; one more is dropped (TM-plug-10).
const WAITING_EVENTS: usize = 256;
/// The lines of a plugin's log that are kept, in memory only.
const LOG_LINES: usize = 10_000;
/// How long a plugin has to exit after its stdin closed, before it is
/// killed (spec 0016, EOF).
const STOP_GRACE: Duration = Duration::from_secs(2);
/// How many of a plugin's messages the UI thread takes before it does its
/// other work.
const BATCH: usize = 256;

/// `<data_local_dir>/tern/plugins`. Not the config dir: that one roams on
/// Windows, and a program is per machine.
pub fn root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("tern").join("plugins"))
}

/// What the host tells the workspace.
pub enum HostEvent {
    /// A plugin asked for something it may ask for.
    Action { plugin: String, action: Action },
    /// A plugin couldn't start, or crashed.
    Notice(String),
}

/// Where a found plugin is.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code, reason = "shown on the Plugins tab (T-067)")]
pub enum State {
    Unusable(String),
    Disabled,
    /// Enabled, with no process: its next event starts one.
    Idle,
    Running,
    /// It crashed or couldn't start; its events are dropped.
    Stopped,
}

pub struct Host {
    root: Option<PathBuf>,
    /// `plugins:` of the config: what the user enabled and ticked.
    granted: BTreeMap<String, PluginSettings>,
    plugins: Vec<Plugin>,
}

struct Plugin {
    id: String,
    folder: PathBuf,
    manifest: Result<Manifest, String>,
    running: Option<Running>,
    /// Counts the starts, so a late message of an earlier process isn't
    /// taken for the current one's.
    runs: u64,
    queue: VecDeque<Event>,
    stopped: bool,
    log: VecDeque<String>,
}

struct Running {
    process: Process,
    run: u64,
    /// The last event is in the plugin's pipe.
    written: bool,
    /// How many more events the plugin asked for: one by starting, one
    /// with each `ready`.
    asked: u32,
    /// Its stdin is closed: it was disabled.
    stopping: bool,
}

impl EventEmitter<HostEvent> for Host {}

impl Host {
    /// A host for the plugins in `root`. It finds nothing until
    /// [`Host::discover`], and ends every plugin when the app quits
    /// (flow 7).
    pub fn new(
        root: Option<PathBuf>,
        granted: BTreeMap<String, PluginSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_app_quit(|this, _| {
            this.shutdown();
            async {}
        })
        .detach();
        Self {
            root,
            granted,
            plugins: Vec::new(),
        }
    }

    /// Reads the plugin folders off the UI thread (flow 1). An enabled
    /// plugin that hears `start` gets it, which starts it.
    pub fn discover(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let found = cx.background_spawn(async move { manifest::discover(&root) });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            let _ = this.update(cx, |this, cx| this.found(found, cx));
        })
        .detach();
    }

    fn found(&mut self, found: Vec<Found>, cx: &mut Context<Self>) {
        self.plugins = found.into_iter().map(Plugin::new).collect();
        for ix in 0..self.plugins.len() {
            if self.enabled(ix) {
                self.enabled_now(ix, cx);
            }
        }
        cx.notify();
    }

    /// Takes the `plugins:` of a reloaded config (flow 2): a plugin the
    /// user disabled is told to stop, one they enabled may start.
    pub fn set_granted(
        &mut self,
        granted: BTreeMap<String, PluginSettings>,
        cx: &mut Context<Self>,
    ) {
        let before: Vec<bool> = (0..self.plugins.len()).map(|ix| self.enabled(ix)).collect();
        let old = std::mem::replace(&mut self.granted, granted);
        for (ix, was) in before.into_iter().enumerate() {
            let id = &self.plugins[ix].id;
            let settings = |granted: &BTreeMap<String, PluginSettings>| {
                granted.get(id).map(|granted| granted.settings.clone())
            };
            let changed = settings(&old) != settings(&self.granted);
            match (was, self.enabled(ix)) {
                (true, false) => self.stop(ix, cx),
                (false, true) => {
                    self.plugins[ix].stopped = false;
                    self.enabled_now(ix, cx);
                }
                // Every event carries the settings, so only a plugin that
                // runs has old ones.
                (true, true) if changed && self.plugins[ix].running.is_some() => {
                    self.enqueue(ix, Event::SettingsChanged, cx);
                }
                _ => {}
            }
        }
        cx.notify();
    }

    fn enabled_now(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.hears(ix, Subscription::Start) {
            self.enqueue(ix, Event::Start, cx);
        }
    }

    /// A terminal pane opened: told to the plugins that hear it, with its
    /// details only for those holding `pane_info` (TM-plug-16).
    pub fn pane_opened(&mut self, pane: u64, info: &PaneInfo, cx: &mut Context<Self>) {
        self.tell_panes(Subscription::PaneOpened, pane, Some(info), cx);
    }

    pub fn pane_closed(&mut self, pane: u64, cx: &mut Context<Self>) {
        self.tell_panes(Subscription::PaneClosed, pane, None, cx);
    }

    fn tell_panes(
        &mut self,
        about: Subscription,
        id: u64,
        info: Option<&PaneInfo>,
        cx: &mut Context<Self>,
    ) {
        for ix in 0..self.plugins.len() {
            if !self.hears(ix, about) {
                continue;
            }
            let info = info.filter(|_| self.holds(ix, Right::PaneInfo)).cloned();
            let pane = Pane { id, info };
            let event = match about {
                Subscription::PaneClosed => Event::PaneClosed { pane },
                _ => Event::PaneOpened { pane },
            };
            self.enqueue(ix, event, cx);
        }
    }

    /// An action the workspace can't carry out yet: said in the plugin's
    /// log, and refused if it waits for a result.
    // TODO(T-068, T-069): gone once every action is carried out.
    pub fn unhandled(&mut self, plugin: &str, action: &Action, cx: &mut Context<Self>) {
        let Some(ix) = self.index(plugin) else {
            return;
        };
        self.log(ix, format!("`{}` isn't available yet", action.name()), cx);
        if action.right().is_some() {
            self.refuse(ix, action, cx);
        }
    }

    /// Where plugin `id` is, if it was found.
    #[allow(dead_code, reason = "shown on the Plugins tab (T-067)")]
    pub fn state(&self, id: &str) -> Option<State> {
        let ix = self.index(id)?;
        let plugin = &self.plugins[ix];
        Some(match &plugin.manifest {
            Err(reason) => State::Unusable(reason.clone()),
            Ok(_) if !self.enabled(ix) => State::Disabled,
            Ok(_) if plugin.stopped => State::Stopped,
            Ok(_) if plugin.running.is_some() => State::Running,
            Ok(_) => State::Idle,
        })
    }

    /// Plugin `id`'s log, oldest line first: its stderr and what Tern
    /// dropped or refused. Never an event's or an action's payload
    /// (TM-plug-17).
    #[allow(dead_code, reason = "shown on the Plugins tab (T-067)")]
    pub fn log_of(&self, id: &str) -> impl Iterator<Item = &str> {
        self.index(id)
            .into_iter()
            .flat_map(|ix| self.plugins[ix].log.iter().map(String::as_str))
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.plugins.iter().position(|plugin| plugin.id == id)
    }

    /// Enabled by the user, for the command line they confirmed
    /// (TM-plug-12).
    fn enabled(&self, ix: usize) -> bool {
        let plugin = &self.plugins[ix];
        match (&plugin.manifest, self.granted.get(&plugin.id)) {
            (Ok(manifest), Some(granted)) => granted.enabled && granted.run == manifest.run,
            _ => false,
        }
    }

    /// A right in force: declared by the manifest and ticked by the user.
    fn holds(&self, ix: usize, right: Right) -> bool {
        let plugin = &self.plugins[ix];
        self.enabled(ix)
            && plugin
                .manifest
                .as_ref()
                .is_ok_and(|manifest| manifest.rights.contains(&right))
            && self
                .granted
                .get(&plugin.id)
                .is_some_and(|granted| granted.rights.contains(&right))
    }

    /// An event in force, as [`Host::holds`] for a right.
    fn hears(&self, ix: usize, event: Subscription) -> bool {
        let plugin = &self.plugins[ix];
        self.enabled(ix)
            && plugin
                .manifest
                .as_ref()
                .is_ok_and(|manifest| manifest.events.contains(&event))
            && self
                .granted
                .get(&plugin.id)
                .is_some_and(|granted| granted.events.contains(&event))
    }

    /// Queues `event` for an enabled plugin (flow 3).
    fn enqueue(&mut self, ix: usize, event: Event, cx: &mut Context<Self>) {
        if self.plugins[ix].stopped {
            return;
        }
        if self.plugins[ix].queue.len() >= WAITING_EVENTS {
            let dropped = format!("an event was dropped: {WAITING_EVENTS} are waiting");
            self.log(ix, dropped, cx);
            return;
        }
        self.plugins[ix].queue.push_back(event);
        self.pump(ix, cx);
    }

    /// Writes the plugin's next event, if it can take one: a process
    /// started for it takes its first at once, a running one after its
    /// `ready` (spec 0016, One event at a time).
    fn pump(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.plugins[ix].queue.is_empty() {
            return;
        }
        if self.plugins[ix].running.is_none() {
            self.spawn(ix, cx);
        }
        let plugin = &mut self.plugins[ix];
        let Some(running) = &mut plugin.running else {
            return;
        };
        if running.stopping || !running.written || running.asked == 0 {
            return;
        }
        let Some(event) = plugin.queue.pop_front() else {
            return;
        };
        let settings = self
            .granted
            .get(&plugin.id)
            .map(|granted| &granted.settings);
        let line = event.line(settings.unwrap_or(&BTreeMap::new()));
        running.written = false;
        running.asked -= 1;
        running.process.write(line);
    }

    fn spawn(&mut self, ix: usize, cx: &mut Context<Self>) {
        let plugin = &mut self.plugins[ix];
        let Ok(manifest) = &plugin.manifest else {
            return;
        };
        let (program, args) = manifest.command(&plugin.folder);
        match Process::spawn(&program, args, &plugin.folder) {
            Ok((process, mut messages)) => {
                plugin.runs += 1;
                let run = plugin.runs;
                plugin.running = Some(Running {
                    process,
                    run,
                    written: true,
                    asked: 1,
                    stopping: false,
                });
                let id = plugin.id.clone();
                // Not kept: what the process wrote before it exited is
                // still read after it.
                cx.spawn(async move |this, cx| {
                    while let Some(first) = messages.next().await {
                        let mut batch = vec![first];
                        while batch.len() < BATCH
                            && let Ok(message) = messages.try_recv()
                        {
                            batch.push(message);
                        }
                        let taken = this.update(cx, |this, cx| {
                            for message in batch {
                                this.message(&id, run, message, cx);
                            }
                        });
                        if taken.is_err() {
                            break;
                        }
                        breathe().await;
                    }
                })
                .detach();
            }
            Err(error) => {
                let name = manifest.name.clone();
                plugin.stopped = true;
                plugin.queue.clear();
                self.log(ix, format!("couldn't start: {error}"), cx);
                cx.emit(HostEvent::Notice(format!(
                    "Plugin {name} couldn't start: {error}"
                )));
            }
        }
    }

    /// What process `run` of plugin `id` said (flows 4 and 6).
    fn message(&mut self, id: &str, run: u64, message: Message, cx: &mut Context<Self>) {
        let Some(ix) = self.index(id) else {
            return;
        };
        let current = self.plugins[ix]
            .running
            .as_mut()
            .filter(|running| running.run == run);
        match message {
            Message::Log(line) => self.log(ix, line, cx),
            Message::Line(Err(reason)) => self.log(ix, format!("dropped {reason}"), cx),
            Message::Line(Ok(Action::Ready)) => {
                if let Some(running) = current {
                    running.asked = running.asked.saturating_add(1);
                    self.pump(ix, cx);
                }
            }
            Message::Line(Ok(action)) => self.action(ix, action, cx),
            Message::Written => {
                if let Some(running) = current {
                    running.written = true;
                    self.pump(ix, cx);
                }
            }
            Message::Exited(code) => {
                if current.is_some() {
                    self.exited(ix, code, cx);
                }
            }
        }
    }

    /// Checks an action against the plugin's rights and manifest, then
    /// hands it to the workspace (flow 4).
    fn action(&mut self, ix: usize, action: Action, cx: &mut Context<Self>) {
        if !self.enabled(ix) {
            return;
        }
        let name = action.name();
        if let Some(right) = action.right()
            && !self.holds(ix, right)
        {
            let missing = right.name();
            self.log(
                ix,
                format!("`{name}` was refused: the `{missing}` right isn't granted"),
                cx,
            );
            self.refuse(ix, &action, cx);
            return;
        }
        let declares = |panel: &str| {
            self.plugins[ix]
                .manifest
                .as_ref()
                .is_ok_and(|manifest| manifest.panels.iter().any(|known| known.id == panel))
        };
        if action.panel().is_some_and(|panel| !declares(panel)) {
            let dropped = format!("`{name}` was dropped: the manifest has no such panel");
            self.log(ix, dropped, cx);
            return;
        }
        let plugin = self.plugins[ix].id.clone();
        cx.emit(HostEvent::Action { plugin, action });
    }

    fn refuse(&mut self, ix: usize, action: &Action, cx: &mut Context<Self>) {
        let result = Event::ActionResult {
            id: action.id().cloned(),
            result: Outcome::Refused,
        };
        self.enqueue(ix, result, cx);
    }

    /// The plugin's process ended (flow 6). After exit status 0, or a stop
    /// Tern asked for, its next event starts it again; any other exit is a
    /// crash, and nothing restarts it.
    fn exited(&mut self, ix: usize, code: Option<i32>, cx: &mut Context<Self>) {
        let plugin = &mut self.plugins[ix];
        let Some(running) = plugin.running.take() else {
            return;
        };
        if running.stopping || code == Some(0) {
            self.pump(ix, cx);
        } else {
            plugin.stopped = true;
            plugin.queue.clear();
            let said = match code {
                Some(code) => format!("exited with code {code}"),
                None => "was killed".to_owned(),
            };
            let name = plugin.manifest.as_ref().map_or(&plugin.id, |m| &m.name);
            let notice = format!("Plugin {name} stopped: it {said}");
            self.log(ix, said, cx);
            cx.emit(HostEvent::Notice(notice));
        }
        cx.notify();
    }

    /// Tells a disabled plugin to stop: its stdin closes, and after
    /// [`STOP_GRACE`] it is killed with what it started.
    fn stop(&mut self, ix: usize, cx: &mut Context<Self>) {
        let plugin = &mut self.plugins[ix];
        plugin.queue.clear();
        let Some(running) = plugin.running.as_mut().filter(|running| !running.stopping) else {
            return;
        };
        running.stopping = true;
        running.process.close();
        let (id, run) = (plugin.id.clone(), running.run);
        let grace = cx.background_executor().timer(STOP_GRACE);
        cx.spawn(async move |this, cx| {
            grace.await;
            let _ = this.update(cx, |this, _| {
                let running = this
                    .plugins
                    .iter_mut()
                    .find(|plugin| plugin.id == id)
                    .and_then(|plugin| plugin.running.as_mut());
                if let Some(running) = running.filter(|running| running.run == run) {
                    running.process.kill();
                }
            });
        })
        .detach();
    }

    /// Ends every plugin as the app quits (flow 7): every stdin closes,
    /// then what still runs after [`STOP_GRACE`] in total is killed. It
    /// blocks, because GPUI doesn't wait that long for a quit's futures.
    fn shutdown(&mut self) {
        let mut running: Vec<Running> = self
            .plugins
            .iter_mut()
            .filter_map(|plugin| {
                plugin.queue.clear();
                plugin.running.take()
            })
            .collect();
        for running in &mut running {
            running.process.close();
        }
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && running.iter().any(|r| !r.process.exited()) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Dropping a process kills what is left of it.
    }

    fn log(&mut self, ix: usize, line: String, cx: &mut Context<Self>) {
        let log = &mut self.plugins[ix].log;
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
        cx.notify();
    }
}

impl Plugin {
    fn new(found: Found) -> Self {
        Self {
            id: found.id,
            folder: found.folder,
            manifest: found.manifest,
            running: None,
            runs: 0,
            queue: VecDeque::new(),
            stopped: false,
            log: VecDeque::new(),
        }
    }
}

/// Lets the UI thread do its other work before the next batch, so a
/// plugin that floods doesn't hold up typing.
async fn breathe() {
    let mut waited = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut waited, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// The host against real plugin processes (spec 0016: Model, flows 1, 3,
/// 4, 6 and 7, AC1, 2, 3, 5 and 9; threat model 0005). A plugin here is one
/// script, run by `sh` on Unix and by Windows PowerShell on Windows, that
/// writes what it got into its own folder. Its threads wake the test's
/// tasks from outside GPUI's test scheduler, so each test allows it to park
/// and waits in real time; only the 2 s grace runs on the test clock.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use gpui::{Entity, TestAppContext};
    use serde_json::Value;
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    /// How long a test waits for something a plugin does.
    const LIMIT: Duration = Duration::from_secs(30);
    const STEP: Duration = Duration::from_millis(20);
    /// How long a test watches for something that must not happen, after
    /// another plugin showed that plugins do start by then.
    const SETTLE: Duration = Duration::from_millis(500);

    /// The plugin, by its mode (the last entry of `run`). Every start adds
    /// the PID to `pids` and writes the file `err`, if there is one, to
    /// stderr.
    ///
    /// - `once`: reads one event into `events.log` and exits 0; `overlap`
    ///   appears if two of them ran at once.
    /// - `crash`: reads one event into `events.log` and exits 3.
    /// - `stay`: reads events into `events.log` and answers each with
    ///   `ready`; after the first one it writes the file `say` to stdout
    ///   before that. While the file `gate` is there, it sends `ready`
    ///   only once the file `go` appears, which it removes. On EOF it
    ///   writes `eof` and exits 0.
    /// - `flag`: reads one event, sends `ready` twice, then reads on
    ///   without ever sending `ready` again.
    /// - `hang`: never reads its stdin and ignores EOF.
    /// - `child`: as `hang`, after starting a child whose PID it writes to
    ///   `child-pid`.
    #[cfg(unix)]
    const SCRIPT: (&str, &str) = (
        "plugin.sh",
        r##"echo $$ >> pids
if [ -f err ]; then cat err >&2; fi
case "$1" in
once)
  if [ -e running ]; then echo overlap >> overlap; fi
  : > running
  if IFS= read -r line; then printf '%s\n' "$line" >> events.log; fi
  rm -f running
  exit 0 ;;
crash)
  if IFS= read -r line; then printf '%s\n' "$line" >> events.log; fi
  exit 3 ;;
stay)
  first=1
  while IFS= read -r line; do
    printf '%s\n' "$line" >> events.log
    if [ "$first" = 1 ] && [ -f say ]; then cat say; fi
    first=0
    if [ -f gate ]; then
      while [ ! -f go ]; do sleep 0.05; done
      rm -f go
    fi
    echo '{"action":"ready"}'
  done
  echo eof > eof
  exit 0 ;;
flag)
  if IFS= read -r line; then printf '%s\n' "$line" >> events.log; fi
  echo '{"action":"ready"}'
  echo '{"action":"ready"}'
  while IFS= read -r line; do printf '%s\n' "$line" >> events.log; done
  exit 0 ;;
hang)
  exec sleep 60 ;;
child)
  sleep 60 &
  echo $! > child-pid
  exec sleep 60 ;;
esac
"##,
    );

    #[cfg(windows)]
    const SCRIPT: (&str, &str) = (
        "plugin.ps1",
        r##"param([string]$mode)
$dir = $PSScriptRoot
function Note([string]$file, [string]$text) {
    [System.IO.File]::AppendAllText((Join-Path $dir $file), $text + "`n")
}
function Has([string]$file) { Test-Path (Join-Path $dir $file) }
function Take { [Console]::In.ReadLine() }
function Say([string]$file) {
    [Console]::Out.Write([System.IO.File]::ReadAllText((Join-Path $dir $file)))
}
Note 'pids' "$PID"
if (Has 'err') { [Console]::Error.Write([System.IO.File]::ReadAllText((Join-Path $dir 'err'))) }
switch ($mode) {
    'once' {
        if (Has 'running') { Note 'overlap' 'overlap' }
        Note 'running' 'running'
        $line = Take
        if ($null -ne $line) { Note 'events.log' $line }
        Remove-Item (Join-Path $dir 'running')
        exit 0
    }
    'crash' {
        $line = Take
        if ($null -ne $line) { Note 'events.log' $line }
        exit 3
    }
    'stay' {
        $first = $true
        while ($null -ne ($line = Take)) {
            Note 'events.log' $line
            if ($first -and (Has 'say')) { Say 'say' }
            $first = $false
            if (Has 'gate') {
                while (-not (Has 'go')) { Start-Sleep -Milliseconds 50 }
                Remove-Item (Join-Path $dir 'go')
            }
            [Console]::Out.WriteLine('{"action":"ready"}')
        }
        Note 'eof' 'eof'
        exit 0
    }
    'flag' {
        $line = Take
        if ($null -ne $line) { Note 'events.log' $line }
        [Console]::Out.WriteLine('{"action":"ready"}')
        [Console]::Out.WriteLine('{"action":"ready"}')
        while ($null -ne ($line = Take)) { Note 'events.log' $line }
        exit 0
    }
    'hang' { Start-Sleep -Seconds 60 }
    'child' {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = 'powershell.exe'
        $info.Arguments = '-NoProfile -Command Start-Sleep -Seconds 60'
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        $child = [System.Diagnostics.Process]::Start($info)
        Note 'child-pid' "$($child.Id)"
        Start-Sleep -Seconds 60
    }
}
"##,
    );

    /// The command line of the plugin in `mode`: what its manifest names
    /// and what the user confirmed.
    fn run(mode: &str) -> Vec<String> {
        let interpreter: &[&str] = if cfg!(windows) {
            &[
                "powershell.exe",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]
        } else {
            &["sh"]
        };
        let argv = interpreter.iter().copied().chain([SCRIPT.0, mode]);
        argv.map(str::to_owned).collect()
    }

    /// What the user ticked for an enabled plugin running in `mode`.
    fn grant(mode: &str, rights: &[Right], events: &[Subscription]) -> PluginSettings {
        PluginSettings {
            enabled: true,
            run: run(mode),
            rights: rights.to_vec(),
            events: events.to_vec(),
            settings: BTreeMap::new(),
        }
    }

    /// A plugin that hears panes open and close, in the manifest and in
    /// the config.
    const HEARS_PANES: &str = "events: [pane_opened, pane_closed]\n";
    const PANES: &[Subscription] = &[Subscription::PaneOpened, Subscription::PaneClosed];

    fn granted<'a>(
        plugins: impl IntoIterator<Item = (&'a str, PluginSettings)>,
    ) -> BTreeMap<String, PluginSettings> {
        let named = plugins
            .into_iter()
            .map(|(id, plugin)| (id.to_owned(), plugin));
        named.collect()
    }

    /// What the host told the workspace.
    #[derive(Clone, Debug, PartialEq)]
    enum Said {
        Action(String, Action),
        Notice(String),
    }

    type Seen = Rc<RefCell<Vec<Said>>>;

    fn notices(seen: &Seen) -> Vec<String> {
        let seen = seen.borrow();
        let notices = seen.iter().filter_map(|said| match said {
            Said::Notice(notice) => Some(notice.clone()),
            Said::Action(..) => None,
        });
        notices.collect()
    }

    fn actions(seen: &Seen) -> Vec<(String, Action)> {
        let seen = seen.borrow();
        let actions = seen.iter().filter_map(|said| match said {
            Said::Action(plugin, action) => Some((plugin.clone(), action.clone())),
            Said::Notice(_) => None,
        });
        actions.collect()
    }

    /// A plugins folder of its own for one test.
    struct Lab {
        /// The app of the host, to quit before the folder goes.
        app: RefCell<Option<TestAppContext>>,
        dir: TempDir,
    }

    /// Ends the plugins before their folder is removed: on Windows a
    /// folder that is a running process's working directory stays. A
    /// failed test quits too, without a second panic hiding its own.
    impl Drop for Lab {
        fn drop(&mut self) {
            let Some(app) = self.app.take() else {
                return;
            };
            if std::thread::panicking() {
                let quit = std::panic::AssertUnwindSafe(|| app.quit());
                let _ = std::panic::catch_unwind(quit);
            } else {
                app.quit();
            }
        }
    }

    impl Lab {
        /// `name` must be unique among all tests.
        fn new(name: &str) -> Self {
            Self {
                app: RefCell::new(None),
                dir: TempDir::new(&format!("plugins-host-{name}")),
            }
        }

        /// Plugin `id` running the script in `mode`; `declares` are the
        /// manifest's further lines. Returns its folder.
        fn plugin(&self, id: &str, mode: &str, declares: &str) -> PathBuf {
            let run = serde_json::to_string(&run(mode)).expect("an argv is JSON");
            let manifest = format!("api: 1\nid: {id}\nname: Name of {id}\nrun: {run}\n{declares}");
            self.folder(id, &manifest)
        }

        /// Plugin `id` with the script and `manifest` as its `plugin.yaml`.
        fn folder(&self, id: &str, manifest: &str) -> PathBuf {
            let folder = self.dir.path().join(id);
            std::fs::create_dir_all(&folder).expect("create the plugin's folder");
            std::fs::write(folder.join("plugin.yaml"), manifest).expect("write plugin.yaml");
            std::fs::write(folder.join(SCRIPT.0), SCRIPT.1).expect("write the plugin");
            folder
        }

        /// A host that found this lab's plugins, with `granted` as the
        /// config's `plugins:`, and what it tells the workspace.
        fn host<'a>(
            &self,
            cx: &mut TestAppContext,
            plugins: impl IntoIterator<Item = (&'a str, PluginSettings)>,
        ) -> (Entity<Host>, Seen) {
            cx.executor().allow_parking();
            self.app.replace(Some(cx.clone()));
            let root = Some(self.dir.path().to_owned());
            let granted = granted(plugins);
            let host = cx.new(|cx| Host::new(root, granted, cx));
            let seen = Seen::default();
            cx.update(|cx| {
                let seen = Rc::clone(&seen);
                cx.subscribe(&host, move |_, event: &HostEvent, _| {
                    seen.borrow_mut().push(match event {
                        HostEvent::Action { plugin, action } => {
                            Said::Action(plugin.clone(), action.clone())
                        }
                        HostEvent::Notice(notice) => Said::Notice(notice.clone()),
                    });
                })
                .detach();
            });
            host.update(cx, |host, cx| host.discover(cx));
            wait_until(cx, "the plugins to be found", |cx| {
                host.read_with(cx, |host, _| !host.plugins.is_empty())
            });
            (host, seen)
        }
    }

    /// Runs the app's tasks until `done`, in real time: a plugin's threads
    /// aren't on the test scheduler.
    fn wait_until(
        cx: &mut TestAppContext,
        what: &str,
        mut done: impl FnMut(&mut TestAppContext) -> bool,
    ) {
        let deadline = Instant::now() + LIMIT;
        loop {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(STEP);
        }
    }

    /// Gives something that must not happen the time to happen.
    fn settle(cx: &mut TestAppContext) {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            cx.run_until_parked();
            std::thread::sleep(STEP);
        }
        cx.run_until_parked();
    }

    fn state(host: &Entity<Host>, cx: &mut TestAppContext, id: &str) -> Option<State> {
        cx.run_until_parked();
        host.read_with(cx, |host, _| host.state(id))
    }

    fn log(host: &Entity<Host>, cx: &mut TestAppContext, id: &str) -> Vec<String> {
        cx.run_until_parked();
        host.read_with(cx, |host, _| host.log_of(id).map(str::to_owned).collect())
    }

    /// What `pane_info` adds to a pane; each part is easy to find in a
    /// line.
    fn details() -> PaneInfo {
        PaneInfo {
            title: "TITLE-vim-secret".into(),
            cwd: Some("CWD-projects".into()),
            profile: "PROFILE-work".into(),
            host: Some("HOST-me-at-bastion".into()),
        }
    }

    const DETAILS: [&str; 4] = [
        "TITLE-vim-secret",
        "CWD-projects",
        "PROFILE-work",
        "HOST-me-at-bastion",
    ];

    fn open(host: &Entity<Host>, cx: &mut TestAppContext, pane: u64) {
        host.update(cx, |host, cx| host.pane_opened(pane, &details(), cx));
    }

    fn close(host: &Entity<Host>, cx: &mut TestAppContext, pane: u64) {
        host.update(cx, |host, cx| host.pane_closed(pane, cx));
    }

    /// The config was reloaded with `plugins` as its `plugins:`.
    fn regrant<'a>(
        host: &Entity<Host>,
        cx: &mut TestAppContext,
        plugins: impl IntoIterator<Item = (&'a str, PluginSettings)>,
    ) {
        let granted = granted(plugins);
        host.update(cx, |host, cx| host.set_granted(granted, cx));
    }

    /// The finished lines of `file` in a plugin's folder; none if it isn't
    /// there.
    fn lines(folder: &Path, file: &str) -> Vec<String> {
        let text = std::fs::read_to_string(folder.join(file)).unwrap_or_default();
        let finished = text.rfind('\n').map_or("", |end| &text[..end]);
        let lines = finished.lines().filter(|line| !line.is_empty());
        lines.map(str::to_owned).collect()
    }

    /// The events the plugin in `folder` got so far, in order.
    fn events(folder: &Path) -> Vec<Value> {
        let parse = |line: String| {
            serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("an event isn't JSON ({error}): {line}"))
        };
        lines(folder, "events.log").into_iter().map(parse).collect()
    }

    fn wait_for_lines(
        cx: &mut TestAppContext,
        folder: &Path,
        file: &str,
        count: usize,
    ) -> Vec<String> {
        let what = format!("{count} lines in {}", folder.join(file).display());
        wait_until(cx, &what, |_| lines(folder, file).len() >= count);
        lines(folder, file)
    }

    fn wait_for_events(cx: &mut TestAppContext, folder: &Path, count: usize) -> Vec<Value> {
        wait_for_lines(cx, folder, "events.log", count);
        events(folder)
    }

    /// The name of each event, in order.
    fn names(events: &[Value]) -> Vec<&str> {
        let names = events.iter().map(|event| event["event"].as_str());
        names.map(|name| name.expect("an event's name")).collect()
    }

    /// The pane each event is about, in order.
    fn panes(events: &[Value]) -> Vec<u64> {
        let panes = events.iter().map(|event| event["pane"]["id"].as_u64());
        panes.map(|pane| pane.expect("a pane's ID")).collect()
    }

    /// The PIDs of every start of the plugin in `folder`.
    fn pids(folder: &Path) -> Vec<String> {
        lines(folder, "pids")
    }

    /// The PID of the plugin's first process, once it runs.
    fn first_pid(cx: &mut TestAppContext, folder: &Path) -> String {
        wait_for_lines(cx, folder, "pids", 1).remove(0)
    }

    /// Whether `pid` is a process: `kill -0` only asks, it signals nothing.
    #[cfg(unix)]
    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Whether `pid` is a process, asked of `tasklist`.
    #[cfg(windows)]
    fn alive(pid: &str) -> bool {
        let listed = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .expect("tasklist runs");
        String::from_utf8_lossy(&listed.stdout).contains(&format!(",\"{pid}\","))
    }

    /// Waits for process `pid` to be gone; what was killed may take a
    /// moment to leave the process table.
    fn wait_gone(pid: &str, what: &str) {
        let deadline = Instant::now() + LIMIT;
        while alive(pid) {
            assert!(Instant::now() < deadline, "{what} {pid} still runs");
            std::thread::sleep(STEP);
        }
    }

    /// AC1, §Model "Disabled until enabled": a found plugin is listed and
    /// its program hasn't run, whether the config doesn't name it or says
    /// `enabled: false`. Flow 2: enabling it starts it only because
    /// `start` is ticked.
    #[gpui::test]
    fn a_found_plugin_is_listed_and_not_run_until_it_is_enabled(cx: &mut TestAppContext) {
        let lab = Lab::new("disabled");
        let declares = "events: [start]\n";
        let fresh = lab.plugin("fresh", "stay", declares);
        let off = lab.plugin("off", "stay", declares);
        let control = lab.plugin("control", "stay", declares);
        let hears_start = || grant("stay", &[], &[Subscription::Start]);
        let disabled = || PluginSettings {
            enabled: false,
            ..hears_start()
        };
        let (host, seen) = lab.host(cx, [("off", disabled()), ("control", hears_start())]);

        assert_eq!(state(&host, cx, "fresh"), Some(State::Disabled));
        assert_eq!(state(&host, cx, "off"), Some(State::Disabled));
        assert_eq!(state(&host, cx, "absent"), None);
        wait_for_events(cx, &control, 1);
        settle(cx);
        assert_eq!(pids(&fresh), [""; 0], "a plugin nobody enabled ran");
        assert_eq!(pids(&off), [""; 0], "a disabled plugin ran");
        assert_eq!(state(&host, cx, "fresh"), Some(State::Disabled));

        let all = [
            ("fresh", hears_start()),
            ("off", hears_start()),
            ("control", hears_start()),
        ];
        regrant(&host, cx, all);
        assert_eq!(names(&wait_for_events(cx, &fresh, 1)), ["start"]);
        assert_eq!(names(&wait_for_events(cx, &off, 1)), ["start"]);
        settle(cx);
        // `start` is sent when a plugin is enabled, not on every reload.
        assert_eq!(names(&events(&control)), ["start"]);
        assert_eq!(pids(&control).len(), 1);
        assert_eq!(notices(&seen), [""; 0]);
    }

    /// AC1 and flow 1: a manifest with `api: 2`, a name another plugin
    /// has, or the name `Tern` is listed as unusable and never started,
    /// even enabled with `start` ticked.
    #[gpui::test]
    fn an_unusable_plugin_is_listed_with_its_reason_and_never_started(cx: &mut TestAppContext) {
        let lab = Lab::new("unusable");
        let argv = serde_json::to_string(&run("stay")).expect("an argv is JSON");
        let manifest = |api: u32, id: &str, name: &str| {
            format!(
                "api: {api}\nid: {id}\nname: {name}\nrun: {argv}\nevents: [start, pane_opened]\n"
            )
        };
        let folders = [
            lab.folder("future", &manifest(2, "future", "Future")),
            lab.folder("moved", &manifest(1, "elsewhere", "Moved")),
            lab.folder("twin-a", &manifest(1, "twin-a", "Twin")),
            lab.folder("twin-b", &manifest(1, "twin-b", "TWIN")),
            lab.folder("tern", &manifest(1, "tern", "tern")),
            lab.folder("broken", "api: 1\nid: broken\nname: {oops\n"),
        ];
        let control = lab.plugin("control", "stay", "events: [start, pane_opened]\n");
        let ids = ["future", "moved", "twin-a", "twin-b", "tern", "broken"];
        let hears = || {
            grant(
                "stay",
                &[],
                &[Subscription::Start, Subscription::PaneOpened],
            )
        };
        let all = ids.into_iter().chain(["control"]).map(|id| (id, hears()));
        let (host, seen) = lab.host(cx, all);

        open(&host, cx, 1);
        wait_for_events(cx, &control, 2);
        settle(cx);
        for (id, folder) in ids.into_iter().zip(&folders) {
            let state = state(&host, cx, id);
            assert!(
                matches!(&state, Some(State::Unusable(reason)) if !reason.is_empty()),
                "{id} is {state:?}"
            );
            assert_eq!(pids(folder), [""; 0], "{id} ran");
        }
        assert_eq!(notices(&seen), [""; 0]);
    }

    /// §Model, Events: `start` is "sent only when the manifest lists" it
    /// "and the user ticked" it, "once after discovery".
    #[gpui::test]
    fn start_is_sent_once_and_only_when_declared_and_ticked(cx: &mut TestAppContext) {
        let lab = Lab::new("start");
        let both = lab.plugin("both", "stay", "events: [start]\n");
        let unticked = lab.plugin("unticked", "stay", "events: [start, pane_opened]\n");
        let undeclared = lab.plugin("undeclared", "stay", "events: [pane_opened]\n");
        let (host, seen) = lab.host(
            cx,
            [
                ("both", grant("stay", &[], &[Subscription::Start])),
                ("unticked", grant("stay", &[], &[Subscription::PaneOpened])),
                (
                    "undeclared",
                    grant(
                        "stay",
                        &[],
                        &[Subscription::Start, Subscription::PaneOpened],
                    ),
                ),
            ],
        );

        assert_eq!(names(&wait_for_events(cx, &both, 1)), ["start"]);
        settle(cx);
        assert_eq!(names(&events(&both)), ["start"], "`start` came twice");
        assert_eq!(pids(&both).len(), 1);
        for (id, folder) in [("unticked", &unticked), ("undeclared", &undeclared)] {
            assert_eq!(pids(folder), [""; 0], "{id} was started");
            assert_eq!(state(&host, cx, id), Some(State::Idle));
        }
        // Their first event starts them, and `start` never comes.
        open(&host, cx, 1);
        for folder in [&unticked, &undeclared] {
            assert_eq!(names(&wait_for_events(cx, folder, 1)), ["pane_opened"]);
        }
        settle(cx);
        assert_eq!(names(&events(&both)), ["start"], "an event nobody ticked");
        assert_eq!(notices(&seen), [""; 0]);
    }

    /// AC2, §Model "started by its first event": enabling starts nothing;
    /// the first event starts the process and is its first line, one JSON
    /// object with the plugin's settings.
    #[gpui::test]
    fn an_enabled_plugin_has_no_process_until_its_first_event(cx: &mut TestAppContext) {
        use crate::config::Setting;
        let lab = Lab::new("lazy");
        let lazy = lab.plugin("lazy", "stay", HEARS_PANES);
        let control = lab.plugin("control", "stay", "events: [start]\n");
        let settings = BTreeMap::from([
            (
                "socket".to_owned(),
                Setting::Text("/var/run/docker.sock".into()),
            ),
            ("tls".to_owned(), Setting::Bool(true)),
        ]);
        let ticked = PluginSettings {
            settings,
            ..grant("stay", &[], PANES)
        };
        let started = grant("stay", &[], &[Subscription::Start]);
        let (host, _seen) = lab.host(cx, [("lazy", ticked), ("control", started)]);

        wait_for_events(cx, &control, 1);
        settle(cx);
        assert_eq!(state(&host, cx, "lazy"), Some(State::Idle));
        assert_eq!(pids(&lazy), [""; 0], "it runs before any event");

        open(&host, cx, 3);
        let got = wait_for_events(cx, &lazy, 1);
        assert_eq!(state(&host, cx, "lazy"), Some(State::Running));
        assert_eq!(pids(&lazy).len(), 1);
        assert_eq!(got[0]["api"], 1);
        assert_eq!(got[0]["event"], "pane_opened");
        assert_eq!(panes(&got), [3]);
        assert_eq!(
            got[0]["settings"],
            serde_json::json!({ "socket": "/var/run/docker.sock", "tls": true })
        );
    }

    /// AC3, §Model "One process per plugin" and "One event at a time": a
    /// plugin that reads one line and exits 0 is started once per event,
    /// each process after the one before it exited, each getting one line,
    /// none lost, in order, with no notice.
    #[gpui::test]
    fn a_plugin_that_exits_after_each_event_is_started_again_for_the_next(cx: &mut TestAppContext) {
        let lab = Lab::new("once");
        let once = lab.plugin("once", "once", HEARS_PANES);
        let (host, seen) = lab.host(cx, [("once", grant("once", &[], PANES))]);

        for pane in 1..=5 {
            open(&host, cx, pane);
        }
        close(&host, cx, 5);
        let got = wait_for_events(cx, &once, 6);
        assert_eq!(panes(&got), [1, 2, 3, 4, 5, 5]);
        assert_eq!(names(&got)[5], "pane_closed");
        wait_until(cx, "the last process to exit", |cx| {
            state(&host, cx, "once") == Some(State::Idle)
        });
        settle(cx);
        assert_eq!(events(&once).len(), 6, "an event came twice");
        assert_eq!(pids(&once).len(), 6, "one process per event");
        assert!(!once.join("overlap").exists(), "two processes ran at once");
        assert_eq!(notices(&seen), [""; 0], "exit status 0 shows nothing");
        assert_eq!(state(&host, cx, "once"), Some(State::Idle));
    }

    /// AC3, §Model "One event at a time": a plugin that stays running gets
    /// its next event only after it sent `ready`, in the same process.
    #[gpui::test]
    fn a_plugin_that_stays_gets_its_next_event_only_after_ready(cx: &mut TestAppContext) {
        let lab = Lab::new("ready");
        let stays = lab.plugin("stays", "stay", HEARS_PANES);
        std::fs::write(stays.join("gate"), "").expect("write gate");
        let (host, seen) = lab.host(cx, [("stays", grant("stay", &[], PANES))]);

        for pane in 1..=3 {
            open(&host, cx, pane);
        }
        for arrived in 1..=3 {
            let got = wait_for_events(cx, &stays, arrived);
            settle(cx);
            let expected: Vec<u64> = (1..=arrived as u64).collect();
            assert_eq!(panes(&got), expected);
            assert_eq!(
                panes(&events(&stays)),
                expected,
                "an event came before `ready`"
            );
            std::fs::write(stays.join("go"), "").expect("write go");
        }
        settle(cx);
        assert_eq!(pids(&stays).len(), 1, "one process got them all");
        assert_eq!(state(&host, cx, "stays"), Some(State::Running));
        assert_eq!(notices(&seen), [""; 0]);
    }

    /// §Model: "Each `ready` asks for one more event: a plugin that sends
    /// two gets two." Two `ready` after the first event let two events
    /// through, and no third.
    #[gpui::test]
    fn each_ready_asks_for_one_more_event(cx: &mut TestAppContext) {
        let lab = Lab::new("ready-count");
        let flag = lab.plugin("flag", "flag", HEARS_PANES);
        let (host, _seen) = lab.host(cx, [("flag", grant("flag", &[], PANES))]);

        for pane in 1..=4 {
            open(&host, cx, pane);
        }
        assert_eq!(panes(&wait_for_events(cx, &flag, 3)), [1, 2, 3]);
        settle(cx);
        assert_eq!(
            panes(&events(&flag)),
            [1, 2, 3],
            "the fourth event came without a third `ready`"
        );
    }

    /// §Model, TM-plug-12: a manifest whose `run` differs from the command
    /// line the user confirmed is not started, by `start` or by any other
    /// event, until the config names the manifest's `run`.
    #[gpui::test]
    fn a_plugin_whose_run_differs_from_the_confirmed_one_is_not_started(cx: &mut TestAppContext) {
        let lab = Lab::new("run-differs");
        let declares = "events: [start, pane_opened]\n";
        let swapped = lab.plugin("swapped", "stay", declares);
        let longer = lab.plugin("longer", "stay", declares);
        let unconfirmed = lab.plugin("unconfirmed", "stay", declares);
        let control = lab.plugin("control", "stay", declares);
        let hears = |run: Vec<String>| PluginSettings {
            run,
            ..grant(
                "stay",
                &[],
                &[Subscription::Start, Subscription::PaneOpened],
            )
        };
        let mut more = run("stay");
        more.push("--verbose".into());
        let (host, seen) = lab.host(
            cx,
            [
                // What the user confirmed was the plugin in another mode.
                ("swapped", hears(run("once"))),
                ("longer", hears(more)),
                ("unconfirmed", hears(Vec::new())),
                ("control", hears(run("stay"))),
            ],
        );

        open(&host, cx, 1);
        wait_for_events(cx, &control, 2);
        settle(cx);
        let folders = [
            ("swapped", &swapped),
            ("longer", &longer),
            ("unconfirmed", &unconfirmed),
        ];
        for (id, folder) in folders {
            assert_eq!(pids(folder), [""; 0], "{id} was started");
            assert_ne!(state(&host, cx, id), Some(State::Running), "{id}");
        }
        assert_eq!(notices(&seen), [""; 0]);

        // The confirmation again, for the manifest's command line.
        regrant(
            &host,
            cx,
            ["swapped", "longer", "unconfirmed", "control"].map(|id| (id, hears(run("stay")))),
        );
        for (_, folder) in folders {
            assert_eq!(names(&wait_for_events(cx, folder, 1)), ["start"]);
        }
    }

    /// AC5, flow 4, §Model "The rights … in force are the granted ones
    /// that the manifest still declares": an action whose right isn't
    /// ticked, or isn't declared, never reaches the workspace; the plugin
    /// gets `refused` with the action's `id`, and its log names the right,
    /// never the action's text (TM-plug-17). No other plugin hears of it
    /// (TM-plug-19).
    #[gpui::test]
    fn an_action_without_its_right_is_refused_and_the_log_names_the_right(cx: &mut TestAppContext) {
        let lab = Lab::new("refused");
        let rights = "rights: [terminal_write, terminal_read, open_tab]\n";
        let unticked = lab.plugin("unticked", "stay", &format!("{HEARS_PANES}{rights}"));
        let undeclared = lab.plugin("undeclared", "stay", HEARS_PANES);
        let bystander = lab.plugin("bystander", "stay", HEARS_PANES);
        let asks = concat!(
            r#"{"action":"run_command","id":11,"text":"echo SECRET-TEXT"}"#,
            "\n",
            r#"{"action":"read_output","id":"twelve","lines":5}"#,
            "\n",
            r#"{"action":"open_tab","id":13,"argv":["SECRET-TEXT","-x"]}"#,
            "\n",
        );
        for folder in [&unticked, &undeclared] {
            std::fs::write(folder.join("say"), asks).expect("write say");
        }
        let every = [Right::TerminalWrite, Right::TerminalRead, Right::OpenTab];
        let (host, seen) = lab.host(
            cx,
            [
                ("unticked", grant("stay", &[], PANES)),
                ("undeclared", grant("stay", &every, PANES)),
                ("bystander", grant("stay", &every, PANES)),
            ],
        );

        open(&host, cx, 1);
        for (id, folder) in [("unticked", &unticked), ("undeclared", &undeclared)] {
            let got = wait_for_events(cx, folder, 4);
            let results = ["action_result"; 3];
            assert_eq!(names(&got), [&["pane_opened"][..], &results[..]].concat());
            let ids: Vec<&Value> = got[1..].iter().map(|event| &event["id"]).collect();
            assert_eq!(
                ids,
                [&Value::from(11), &Value::from("twelve"), &Value::from(13)]
            );
            for result in &got[1..] {
                assert_eq!(result["result"], "refused", "{id}: {result}");
            }
            let log = log(&host, cx, id).join("\n");
            for right in ["terminal_write", "terminal_read", "open_tab"] {
                assert!(
                    log.contains(right),
                    "{id}'s log doesn't name {right}: {log}"
                );
            }
            assert!(!log.contains("SECRET-TEXT"), "{id}'s log holds a payload");
        }
        settle(cx);
        assert!(
            actions(&seen).is_empty(),
            "a refused action was carried out"
        );
        assert_eq!(notices(&seen), [""; 0], "a notice per refused line");
        assert_eq!(names(&events(&bystander)), ["pane_opened"]);
        assert_eq!(log(&host, cx, "bystander"), [""; 0]);
    }

    /// Flow 4: an unknown action, a malformed line and an action for a
    /// panel the manifest doesn't declare are dropped, each "with a line
    /// in the plugin's log, not a notice per line". The plugin carries on,
    /// and its next action is carried out.
    #[gpui::test]
    fn a_line_tern_cannot_carry_out_is_dropped_with_a_log_line(cx: &mut TestAppContext) {
        let lab = Lab::new("dropped");
        let panels = "panels: [{ id: containers, title: Containers }]\n";
        let chatty = lab.plugin("chatty", "stay", &format!("{HEARS_PANES}{panels}"));
        let says = concat!(
            "SECRET-TEXT is no JSON\n",
            r#"{"action":"fly","to":"SECRET-TEXT"}"#,
            "\n",
            r#"{"action":"open_panel","panel":"other"}"#,
            "\n",
            r#"{"action":"append_log","panel":"other","text":"SECRET-TEXT"}"#,
            "\n",
            r#"{"action":"open_panel","panel":"containers"}"#,
            "\n",
        );
        std::fs::write(chatty.join("say"), says).expect("write say");
        let (host, seen) = lab.host(cx, [("chatty", grant("stay", &[], PANES))]);

        open(&host, cx, 1);
        wait_until(cx, "the declared panel's action", |_| {
            !actions(&seen).is_empty()
        });
        open(&host, cx, 2);
        assert_eq!(panes(&wait_for_events(cx, &chatty, 2)), [1, 2]);
        let shown = Action::OpenPanel {
            panel: "containers".into(),
        };
        assert_eq!(actions(&seen), [("chatty".to_owned(), shown)]);
        let log = log(&host, cx, "chatty");
        assert_eq!(log.len(), 4, "one line for each dropped line: {log:?}");
        assert!(!log.join("\n").contains("SECRET-TEXT"), "{log:?}");
        assert_eq!(notices(&seen), [""; 0]);
        assert_eq!(state(&host, cx, "chatty"), Some(State::Running));
        assert_eq!(pids(&chatty).len(), 1);
    }

    /// Flow 4, TM-plug-01: an action a plugin may ask for reaches the
    /// workspace as it was sent, under the ID of the plugin whose process
    /// sent it. `ready` is the host's own business, and the host answers
    /// no action that waits for the user.
    #[gpui::test]
    fn a_permitted_action_reaches_the_workspace_under_its_plugins_id(cx: &mut TestAppContext) {
        let lab = Lab::new("permitted");
        let declares = format!("{HEARS_PANES}rights: [terminal_write]\n");
        let writer = lab.plugin("writer", "stay", &declares);
        let other = lab.plugin("other", "stay", &declares);
        let say = |folder: &Path, text: &str| {
            let lines = format!(
                "{{\"action\":\"run_command\",\"id\":9,\"pane\":4,\"text\":\"{text}\"}}\n\
                 {{\"action\":\"notify\",\"text\":\"{text}\"}}\n"
            );
            std::fs::write(folder.join("say"), lines).expect("write say");
        };
        say(&writer, "from writer");
        say(&other, "from other");
        let ticked = || grant("stay", &[Right::TerminalWrite], PANES);
        let (host, seen) = lab.host(cx, [("writer", ticked()), ("other", ticked())]);

        open(&host, cx, 1);
        wait_until(cx, "four actions", |_| actions(&seen).len() >= 4);
        settle(cx);
        let of = |plugin: &str| -> Vec<Action> {
            let all = actions(&seen).into_iter();
            let theirs = all.filter(|(sender, _)| sender == plugin);
            theirs.map(|(_, action)| action).collect()
        };
        for (plugin, text) in [("writer", "from writer"), ("other", "from other")] {
            let expected = [
                Action::RunCommand {
                    id: Some(protocol::Id::Number(9)),
                    pane: Some(4),
                    text: text.into(),
                },
                Action::Notify { text: text.into() },
            ];
            assert_eq!(of(plugin), expected, "{plugin}");
        }
        assert_eq!(actions(&seen).len(), 4);
        for folder in [&writer, &other] {
            assert_eq!(names(&events(folder)), ["pane_opened"]);
        }
        assert_eq!(notices(&seen), [""; 0]);
    }

    /// §Model: "stderr is the plugin's log, never parsed": an action
    /// written there is a log line, not an action.
    #[gpui::test]
    fn stderr_is_the_plugins_log_and_never_an_action(cx: &mut TestAppContext) {
        let lab = Lab::new("stderr");
        let noisy = lab.plugin("noisy", "stay", HEARS_PANES);
        let quiet = lab.plugin("quiet", "stay", HEARS_PANES);
        let err = "{\"action\":\"notify\",\"text\":\"on stderr\"}\nconnecting to the daemon\n";
        std::fs::write(noisy.join("err"), err).expect("write err");
        let ticked = || grant("stay", &[], PANES);
        let (host, seen) = lab.host(cx, [("noisy", ticked()), ("quiet", ticked())]);

        open(&host, cx, 1);
        wait_until(cx, "stderr in the log", |cx| {
            log(&host, cx, "noisy").len() >= 2
        });
        wait_for_events(cx, &quiet, 1);
        settle(cx);
        let said = log(&host, cx, "noisy");
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].contains("on stderr"), "{said:?}");
        assert_eq!(said[1], "connecting to the daemon");
        assert!(actions(&seen).is_empty(), "{:?}", actions(&seen));
        assert_eq!(notices(&seen), [""; 0]);
        // One plugin's log is its own (TM-plug-19).
        assert_eq!(log(&host, cx, "quiet"), [""; 0]);
    }

    /// AC9, flow 6: a plugin that ends with another status than 0 crashed:
    /// one notice naming it, it is stopped, nothing restarts it, and its
    /// events are dropped. The other plugins carry on. Disabling and
    /// enabling it makes its next event start it again.
    #[gpui::test]
    fn a_crash_is_one_notice_and_stops_only_that_plugin(cx: &mut TestAppContext) {
        let lab = Lab::new("crash");
        let crasher = lab.plugin("crasher", "crash", HEARS_PANES);
        let steady = lab.plugin("steady", "stay", HEARS_PANES);
        let both = || {
            [
                ("crasher", grant("crash", &[], PANES)),
                ("steady", grant("stay", &[], PANES)),
            ]
        };
        let (host, seen) = lab.host(cx, both());

        open(&host, cx, 1);
        wait_until(cx, "the crash", |cx| {
            state(&host, cx, "crasher") == Some(State::Stopped)
        });
        open(&host, cx, 2);
        close(&host, cx, 1);
        assert_eq!(panes(&wait_for_events(cx, &steady, 3)), [1, 2, 1]);
        settle(cx);
        let shown = notices(&seen);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].contains("Name of crasher"), "{}", shown[0]);
        assert_eq!(state(&host, cx, "crasher"), Some(State::Stopped));
        assert_eq!(pids(&crasher).len(), 1, "it was restarted");
        assert_eq!(panes(&events(&crasher)), [1]);
        assert_eq!(state(&host, cx, "steady"), Some(State::Running));
        assert_eq!(pids(&steady).len(), 1);

        let disabled = PluginSettings {
            enabled: false,
            ..grant("crash", &[], PANES)
        };
        regrant(
            &host,
            cx,
            [("crasher", disabled), ("steady", grant("stay", &[], PANES))],
        );
        assert_eq!(state(&host, cx, "crasher"), Some(State::Disabled));
        regrant(&host, cx, both());
        assert_eq!(state(&host, cx, "crasher"), Some(State::Idle));
        open(&host, cx, 7);
        // What was dropped while it was stopped stays dropped.
        assert_eq!(panes(&wait_for_events(cx, &crasher, 2)), [1, 7]);
        assert_eq!(pids(&crasher).len(), 2);
        assert_eq!(panes(&wait_for_events(cx, &steady, 4)), [1, 2, 1, 7]);
        assert_eq!(
            pids(&steady).len(),
            1,
            "a reload restarted a running plugin"
        );
    }

    /// Flow 3: "A spawn that fails (missing program, no execute bit) shows
    /// a notice and marks the plugin stopped." TM-plug-15: a bare name is
    /// looked up on `PATH`, never in the plugin's folder, where a program
    /// of that name is waiting.
    #[gpui::test]
    fn a_program_that_cannot_be_started_is_one_notice_and_stops_the_plugin(
        cx: &mut TestAppContext,
    ) {
        let lab = Lab::new("spawn-fails");
        let manifest = |id: &str, program: &str| {
            format!("api: 1\nid: {id}\nname: Name of {id}\nrun: [{program}]\n{HEARS_PANES}")
        };
        lab.folder("missing", &manifest("missing", "./no-such-program"));
        let planted = lab.folder("planted", &manifest("planted", "tern-test-planted-program"));
        #[cfg(windows)]
        {
            let system = std::env::var("SystemRoot").expect("%SystemRoot%");
            let whoami = Path::new(&system).join("System32").join("whoami.exe");
            std::fs::copy(whoami, planted.join("tern-test-planted-program.exe"))
                .expect("plant a program");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let program = planted.join("tern-test-planted-program");
            std::fs::write(&program, "#!/bin/sh\necho ran > ran\n").expect("plant a program");
            let runnable = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&program, runnable).expect("make it runnable");
        }
        let steady = lab.plugin("steady", "stay", HEARS_PANES);
        let confirmed = |program: &str| PluginSettings {
            run: vec![program.to_owned()],
            ..grant("stay", &[], PANES)
        };
        let (host, seen) = lab.host(
            cx,
            [
                ("missing", confirmed("./no-such-program")),
                ("planted", confirmed("tern-test-planted-program")),
                ("steady", grant("stay", &[], PANES)),
            ],
        );
        assert_eq!(state(&host, cx, "missing"), Some(State::Idle));
        assert_eq!(state(&host, cx, "planted"), Some(State::Idle));

        open(&host, cx, 1);
        open(&host, cx, 2);
        assert_eq!(panes(&wait_for_events(cx, &steady, 2)), [1, 2]);
        settle(cx);
        for id in ["missing", "planted"] {
            assert_eq!(state(&host, cx, id), Some(State::Stopped), "{id}");
            let named = format!("Name of {id}");
            let theirs = notices(&seen).iter().filter(|n| n.contains(&named)).count();
            assert_eq!(theirs, 1, "{id}: {:?}", notices(&seen));
        }
        assert_eq!(notices(&seen).len(), 2);
        assert!(!planted.join("ran").exists(), "the folder's program ran");
    }

    /// §Model "EOF on stdin means stop", flow 2: disabling closes a
    /// running plugin's stdin, and a plugin that honors EOF ends by
    /// itself, within the grace, with no notice.
    #[gpui::test]
    fn disabling_closes_stdin_and_a_plugin_that_honors_eof_exits(cx: &mut TestAppContext) {
        let lab = Lab::new("eof");
        let polite = lab.plugin("polite", "stay", HEARS_PANES);
        let (host, seen) = lab.host(cx, [("polite", grant("stay", &[], PANES))]);
        open(&host, cx, 1);
        wait_for_events(cx, &polite, 1);
        let pid = first_pid(cx, &polite);
        assert!(alive(&pid), "plugin {pid} runs before it is disabled");

        let disabled = PluginSettings {
            enabled: false,
            ..grant("stay", &[], PANES)
        };
        regrant(&host, cx, [("polite", disabled)]);
        // The test clock stands still: nothing but EOF ends it.
        wait_until(cx, "the plugin to see EOF", |_| polite.join("eof").exists());
        wait_gone(&pid, "the plugin");
        assert_eq!(state(&host, cx, "polite"), Some(State::Disabled));
        open(&host, cx, 2);
        settle(cx);
        assert_eq!(
            panes(&events(&polite)),
            [1],
            "a disabled plugin got an event"
        );
        assert_eq!(pids(&polite).len(), 1);
        assert_eq!(notices(&seen), [""; 0], "a stop Tern asked for is no crash");
    }

    /// §Model "EOF on stdin means stop", flow 2, OQ-035: a plugin that
    /// ignores EOF is killed once the 2 s grace is over, not before, and
    /// what it started goes with it.
    #[gpui::test]
    fn a_plugin_that_ignores_eof_is_killed_with_its_children_after_the_grace(
        cx: &mut TestAppContext,
    ) {
        let lab = Lab::new("grace");
        let deaf = lab.plugin("deaf", "child", HEARS_PANES);
        let (host, seen) = lab.host(cx, [("deaf", grant("child", &[], PANES))]);
        open(&host, cx, 1);
        let pid = first_pid(cx, &deaf);
        let child = wait_for_lines(cx, &deaf, "child-pid", 1).remove(0);
        assert!(alive(&pid) && alive(&child), "{pid} and {child} run");

        let disabled = PluginSettings {
            enabled: false,
            ..grant("child", &[], PANES)
        };
        regrant(&host, cx, [("deaf", disabled)]);
        cx.executor()
            .advance_clock(STOP_GRACE - Duration::from_millis(1));
        settle(cx);
        assert!(
            alive(&pid),
            "plugin {pid} was killed before the grace ended"
        );
        assert!(alive(&child), "its child {child} was killed early");

        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        wait_gone(&pid, "the plugin");
        wait_gone(&child, "the plugin's child");
        settle(cx);
        assert_eq!(notices(&seen), [""; 0], "a stop Tern asked for is no crash");
        assert_eq!(state(&host, cx, "deaf"), Some(State::Disabled));
    }

    /// AC9, flow 7: quitting leaves no plugin process behind: not one
    /// that honors EOF, not one that ignores it, and not its child.
    #[gpui::test]
    fn quitting_leaves_no_plugin_process_behind(cx: &mut TestAppContext) {
        let lab = Lab::new("quit");
        let polite = lab.plugin("polite", "stay", HEARS_PANES);
        let deaf = lab.plugin("deaf", "hang", HEARS_PANES);
        let parent = lab.plugin("parent", "child", HEARS_PANES);
        let (host, _seen) = lab.host(
            cx,
            [
                ("polite", grant("stay", &[], PANES)),
                ("deaf", grant("hang", &[], PANES)),
                ("parent", grant("child", &[], PANES)),
            ],
        );
        open(&host, cx, 1);
        let mut running: Vec<String> = [&polite, &deaf, &parent]
            .into_iter()
            .map(|folder| first_pid(cx, folder))
            .collect();
        running.push(wait_for_lines(cx, &parent, "child-pid", 1).remove(0));
        wait_for_events(cx, &polite, 1);
        for pid in &running {
            assert!(alive(pid), "process {pid} runs before the quit");
        }

        cx.quit();
        for pid in &running {
            wait_gone(pid, "after the quit, process");
        }
        assert!(
            polite.join("eof").exists(),
            "the quit didn't close stdin first"
        );
    }

    /// Flow 7 and the module's promise: a host that goes away takes its
    /// plugins with it.
    #[gpui::test]
    fn dropping_the_host_ends_its_plugins(cx: &mut TestAppContext) {
        let lab = Lab::new("drop");
        let deaf = lab.plugin("deaf", "child", HEARS_PANES);
        let (host, _seen) = lab.host(cx, [("deaf", grant("child", &[], PANES))]);
        open(&host, cx, 1);
        let pid = first_pid(cx, &deaf);
        let child = wait_for_lines(cx, &deaf, "child-pid", 1).remove(0);
        assert!(alive(&pid) && alive(&child), "{pid} and {child} run");

        drop(host);
        cx.update(|_| {});
        cx.run_until_parked();
        wait_gone(&pid, "the plugin");
        wait_gone(&child, "the plugin's child");
    }

    /// §Model Events, TM-plug-16: "Panes are opaque IDs unless `pane_info`
    /// is granted", which takes the manifest declaring it and the user
    /// ticking it.
    #[gpui::test]
    fn pane_details_reach_only_a_plugin_that_holds_pane_info(cx: &mut TestAppContext) {
        let lab = Lab::new("pane-info");
        let declares = format!("{HEARS_PANES}rights: [pane_info]\n");
        let holds = lab.plugin("holds", "stay", &declares);
        let unticked = lab.plugin("unticked", "stay", &declares);
        let undeclared = lab.plugin("undeclared", "stay", HEARS_PANES);
        let (host, _seen) = lab.host(
            cx,
            [
                ("holds", grant("stay", &[Right::PaneInfo], PANES)),
                ("unticked", grant("stay", &[Right::TerminalRead], PANES)),
                ("undeclared", grant("stay", &[Right::PaneInfo], PANES)),
            ],
        );

        open(&host, cx, 42);
        close(&host, cx, 42);
        wait_for_events(cx, &holds, 2);
        let got = lines(&holds, "events.log");
        for detail in DETAILS {
            assert!(got[0].contains(detail), "no {detail} in {}", got[0]);
        }
        for folder in [&unticked, &undeclared] {
            let got = wait_for_events(cx, folder, 2);
            assert_eq!(names(&got), ["pane_opened", "pane_closed"]);
            assert_eq!(panes(&got), [42, 42]);
            let text = lines(folder, "events.log").join("\n");
            for detail in DETAILS {
                assert!(
                    !text.contains(detail),
                    "{detail} reached {}",
                    folder.display()
                );
            }
        }
    }

    /// AC5, §Model: "an event that isn't ticked is never sent", and
    /// neither is one the manifest doesn't list. The event that is in
    /// force still comes.
    #[gpui::test]
    fn an_event_that_is_not_declared_and_ticked_is_never_sent(cx: &mut TestAppContext) {
        let lab = Lab::new("unticked-event");
        let unticked = lab.plugin("unticked", "stay", HEARS_PANES);
        let undeclared = lab.plugin("undeclared", "stay", "events: [pane_closed]\n");
        let closes = &[Subscription::PaneClosed];
        let (host, _seen) = lab.host(
            cx,
            [
                ("unticked", grant("stay", &[], closes)),
                ("undeclared", grant("stay", &[], PANES)),
            ],
        );

        open(&host, cx, 1);
        open(&host, cx, 2);
        close(&host, cx, 1);
        for folder in [&unticked, &undeclared] {
            let got = wait_for_events(cx, folder, 1);
            assert_eq!(names(&got), ["pane_closed"]);
        }
        settle(cx);
        for folder in [&unticked, &undeclared] {
            assert_eq!(names(&events(folder)), ["pane_closed"]);
        }
    }

    /// §Bounds, TM-plug-10: "Events wait in a queue of 256 per plugin; one
    /// that doesn't fit … is dropped with a line in the plugin's log." The
    /// plugin isn't killed for being slow, the events that fit all arrive
    /// in order once it sends `ready`, and another plugin isn't held up.
    #[gpui::test]
    fn events_wait_in_a_queue_of_256_and_one_more_is_dropped_with_a_log_line(
        cx: &mut TestAppContext,
    ) {
        let lab = Lab::new("queue");
        let slow = lab.plugin("slow", "stay", "events: [pane_opened]\n");
        let quick = lab.plugin("quick", "stay", "events: [pane_closed]\n");
        std::fs::write(slow.join("gate"), "").expect("write gate");
        let (host, seen) = lab.host(
            cx,
            [
                ("slow", grant("stay", &[], &[Subscription::PaneOpened])),
                ("quick", grant("stay", &[], &[Subscription::PaneClosed])),
            ],
        );

        // The plugin holds pane 0's event and doesn't send `ready`.
        open(&host, cx, 0);
        wait_for_events(cx, &slow, 1);
        for pane in 1..=256 {
            open(&host, cx, pane);
        }
        let before = log(&host, cx, "slow");
        assert_eq!(before, [""; 0], "an event that fits was dropped");
        open(&host, cx, 257);
        let after = log(&host, cx, "slow");
        assert_eq!(after.len(), 1, "{after:?}");
        open(&host, cx, 258);
        assert_eq!(log(&host, cx, "slow").len(), 2);

        close(&host, cx, 1);
        assert_eq!(names(&wait_for_events(cx, &quick, 1)), ["pane_closed"]);
        assert_eq!(state(&host, cx, "slow"), Some(State::Running));
        assert_eq!(panes(&events(&slow)), [0]);

        std::fs::remove_file(slow.join("gate")).expect("remove gate");
        std::fs::write(slow.join("go"), "").expect("write go");
        let got = wait_for_events(cx, &slow, 257);
        settle(cx);
        let expected: Vec<u64> = (0..=256).collect();
        assert_eq!(panes(&got), expected);
        assert_eq!(panes(&events(&slow)), expected, "a dropped event arrived");
        assert_eq!(pids(&slow).len(), 1);
        assert_eq!(notices(&seen), [""; 0], "a slow plugin is no notice");
    }
}
