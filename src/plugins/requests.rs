//! The host's side of what a plugin asks the user for (spec 0016,
//! Commands, Confirmation, flow 5): its manifest commands, the one request
//! per plugin that may wait on a bar, the refusals after a Deny, and the
//! results and log lines each request ends with. The workspace draws the
//! bar and carries out what was allowed.

use super::manifest::Right;
use super::protocol::{Event, Id, Outcome};
use super::{Host, HostEvent};
use gpui::Context;

/// A manifest command, as the palette lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedCommand {
    pub plugin: String,
    pub command: String,
    /// `<plugin name>: <title>`.
    pub title: String,
}

impl Host {
    /// The commands of every enabled, usable plugin, for the palette.
    pub fn commands(&self) -> Vec<ListedCommand> {
        let mut listed = Vec::new();
        for (ix, plugin) in self.plugins.iter().enumerate() {
            let Ok(manifest) = &plugin.manifest else {
                continue;
            };
            if !self.enabled(ix) {
                continue;
            }
            listed.extend(manifest.commands.iter().map(|command| ListedCommand {
                plugin: plugin.id.clone(),
                command: command.id.clone(),
                title: format!("{}: {}", manifest.name, command.title),
            }));
        }
        listed
    }

    /// The user ran `plugin.<plugin>.<command>`: it gets a `command` event,
    /// an act of the user's on it. Else why it can't run, for a notice.
    pub fn run_command(
        &mut self,
        plugin: &str,
        command: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let id = format!("plugin.{plugin}.{command}");
        let ix = self
            .index(plugin)
            .ok_or_else(|| format!("{id}: there is no plugin `{plugin}`"))?;
        let Ok(manifest) = &self.plugins[ix].manifest else {
            return Err(format!("{id}: the plugin is unusable"));
        };
        let name = manifest.name.clone();
        if !manifest.commands.iter().any(|known| known.id == command) {
            return Err(format!("{id}: {name} has no command `{command}`"));
        }
        if !self.enabled(ix) {
            return Err(format!("Plugin {name} isn't enabled"));
        }
        if self.plugins[ix].stopped {
            return Err(format!("Plugin {name} stopped; restart it first"));
        }
        self.user_acted(plugin);
        let command = command.to_owned();
        self.enqueue(ix, Event::Command { command }, cx);
        Ok(())
    }

    /// Whether `plugin` is enabled now: an allowed request of one that
    /// was disabled meanwhile is not carried out.
    pub fn is_enabled(&self, plugin: &str) -> bool {
        self.index(plugin).is_some_and(|ix| self.enabled(ix))
    }

    /// Whether `plugin` holds `right` now, for a command button.
    pub fn may(&self, plugin: &str, right: Right) -> bool {
        self.index(plugin).is_some_and(|ix| self.holds(ix, right))
    }

    /// The plugin's name, for a bar; never from an action.
    pub fn name_of(&self, plugin: &str) -> Option<String> {
        let ix = self.index(plugin)?;
        let manifest = self.plugins[ix].manifest.as_ref().ok()?;
        Some(manifest.name.clone())
    }

    /// Lets a request of `plugin` wait on a bar: none of its others is
    /// waiting, and it wasn't denied since the user last acted on it.
    /// Otherwise the request is refused, said in the log, and an action's
    /// `id` gets the result.
    pub fn may_ask(
        &mut self,
        plugin: &str,
        kind: &str,
        id: Option<Id>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ix) = self.index(plugin) else {
            return false;
        };
        let why = match (&self.plugins[ix].asking, self.plugins[ix].denied) {
            (true, _) => "another of its requests is waiting",
            (false, true) => "the user denied its last request",
            (false, false) => {
                self.plugins[ix].asking = true;
                return true;
            }
        };
        self.log(ix, format!("`{kind}` was refused: {why}"), cx);
        self.result(ix, id, Outcome::Refused, cx);
        false
    }

    /// A request ended. Its line in the log names its kind, the pane and
    /// the outcome, never its text (TM-plug-07); a Deny refuses the
    /// plugin's next ones until the user acts on it. `asked` is false for
    /// what never waited on a bar (a refusal, a `fill_command` button).
    #[allow(clippy::too_many_arguments)]
    pub fn answered(
        &mut self,
        plugin: &str,
        kind: &str,
        pane: &str,
        id: Option<Id>,
        outcome: Outcome,
        asked: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.index(plugin) else {
            return;
        };
        if asked {
            self.plugins[ix].asking = false;
        }
        if outcome == Outcome::Denied {
            self.plugins[ix].denied = true;
        }
        let said = match outcome {
            Outcome::Allowed => "allowed",
            Outcome::Denied => "denied",
            Outcome::Refused => "refused",
            Outcome::NoPane => "dropped: its pane closed",
        };
        let time = clock();
        self.log(ix, format!("{time} UTC `{kind}` on {pane}: {said}"), cx);
        self.result(ix, id, outcome, cx);
    }

    /// The text an allowed `read_output` sends, and nothing else.
    pub fn send_output(
        &mut self,
        plugin: &str,
        id: Option<Id>,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.index(plugin).filter(|ix| self.enabled(*ix)) {
            self.enqueue(ix, Event::Output { id, text }, cx);
        }
    }

    /// `notify`: a notice naming the plugin. One at a time per plugin: one
    /// that comes while the last is up is dropped with a log line.
    pub fn notify(&mut self, plugin: &str, text: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.index(plugin) else {
            return;
        };
        let now = std::time::Instant::now();
        if self.plugins[ix]
            .notified
            .is_some_and(|at| now.duration_since(at) < NOTICE_SHOWN)
        {
            self.log(
                ix,
                "`notify` was dropped: its last notice is still up".into(),
                cx,
            );
            return;
        }
        self.plugins[ix].notified = Some(now);
        let name = self.name_of(plugin).unwrap_or_else(|| plugin.to_owned());
        cx.emit(HostEvent::Notice(format!("{name}: {text}")));
    }

    /// Sends a request's result, if the action had an `id` to send it with
    /// or always for a confirmed action: the plugin may wait for it.
    fn result(&mut self, ix: usize, id: Option<Id>, result: Outcome, cx: &mut Context<Self>) {
        if self.enabled(ix) {
            self.enqueue(ix, Event::ActionResult { id, result }, cx);
        }
    }
}

/// How long a notice stays up, so a plugin's next one waits for it.
const NOTICE_SHOWN: std::time::Duration = std::time::Duration::from_secs(5);

/// The time of day in UTC as `HH:MM:SS`, for a log line; the log is in
/// memory only.
fn clock() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, day % 3600 / 60, day % 60)
}
