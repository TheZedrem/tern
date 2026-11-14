//! The workspace's side of what plugins ask of a terminal (spec 0016,
//! Confirmation, Command buttons, flow 5; threat model 0005): the bar on
//! the target pane that shows exactly what would be typed, run or sent,
//! Allow and Deny on it and in the palette, and carrying out what was
//! allowed. The bar never takes the keyboard, so typing goes on to the
//! shell. The host keeps one request per plugin and logs each outcome.

use super::tree::LeafId;
use super::{Workspace, show_notice};
use crate::commands::{AllowPluginRequest, DenyPluginRequest, PluginCommand};
use crate::config::{LocalProfile, Profile, ProfileKind};
use crate::plugins::command::{self, Command};
use crate::plugins::{Action, Id, Outcome};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::ActiveTheme;
use std::time::{Duration, Instant};

/// Allow ignores clicks this long after its bar appears (TM-plug-09).
const ALLOW_GUARD: Duration = Duration::from_millis(500);
/// The lines `read_output` reads when it doesn't say.
const DEFAULT_READ_LINES: u32 = 100;

/// A request waiting on its bar.
pub(super) struct Request {
    plugin: String,
    /// The plugin's name, from the host's record of who asked.
    name: String,
    pane: LeafId,
    asked: Asked,
    /// The action's `id`; `None` for a command button.
    id: Option<Id>,
    /// A command button's click: the plugin gets no result.
    button: bool,
    since: Instant,
}

enum Asked {
    Command(Command),
    /// What `read_output` would send, read when it arrived.
    Read(String),
}

impl Asked {
    fn kind(&self) -> &'static str {
        match self {
            Asked::Command(command) => command.name(),
            Asked::Read(_) => "read_output",
        }
    }
}

impl Workspace {
    /// A `fill_command`, `run_command`, `read_output` or `open_tab` of
    /// `plugin`, whose right the host already checked: refused if its text
    /// breaks the rules, else shown on its pane's bar (flow 5).
    pub(super) fn plugin_request(
        &mut self,
        plugin: &str,
        action: &Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (asked, pane, id) = match action {
            Action::FillCommand { id, pane, text } => {
                (Ok(Command::Fill { text: text.clone() }), *pane, id.clone())
            }
            Action::RunCommand { id, pane, text } => {
                (Ok(Command::Run { text: text.clone() }), *pane, id.clone())
            }
            Action::OpenTab { id, argv } => (
                Ok(Command::OpenTab { argv: argv.clone() }),
                None,
                id.clone(),
            ),
            Action::ReadOutput { id, pane, lines } => (Err(*lines), *pane, id.clone()),
            _ => return,
        };
        let target = match pane {
            Some(pane) => Some(pane).filter(|pane| self.panes.contains_key(pane)),
            None => self.terminal_target(),
        };
        let asked = match asked {
            Ok(command) => Asked::Command(command),
            Err(lines) => {
                let lines = lines.unwrap_or(DEFAULT_READ_LINES);
                let text = target
                    .and_then(|pane| self.panes.get(&pane))
                    .map(|pane| pane.read(cx).output(lines))
                    .unwrap_or_default();
                Asked::Read(text)
            }
        };
        self.ask(plugin, asked, target, id, false, window, cx);
    }

    /// A command button was clicked (spec 0016, Command buttons): a
    /// `fill_command` types at once, Enter staying the user's; a
    /// `run_command` or `open_tab` raises the bar as if the plugin had
    /// sent it then. Its target is the terminal pane that last had focus.
    pub(super) fn plugin_button(
        &mut self,
        plugin: &str,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.plugin_host() else {
            return;
        };
        let kind = command.name();
        if !host.read(cx).may(plugin, command.right()) {
            let why = format!(
                "`{kind}` button: the `{}` right isn't granted",
                command.right().name()
            );
            host.update(cx, |host, cx| host.dropped(plugin, why, cx));
            return;
        }
        let Some(pane) = self.terminal_target() else {
            show_notice(
                "There's no terminal pane for the command".into(),
                window,
                cx,
            );
            return;
        };
        if let Command::Fill { text } = &command {
            if let Some(why) = command.refusal() {
                let why = format!("`{kind}` button was refused: {why}");
                host.update(cx, |host, cx| host.dropped(plugin, why, cx));
                return;
            }
            let typed = self
                .panes
                .get(&pane)
                .is_some_and(|view| view.read(cx).type_for_plugin(text, false));
            let outcome = if typed {
                Outcome::Allowed
            } else {
                Outcome::Refused
            };
            let label = self.pane_label(pane, cx);
            host.update(cx, |host, cx| {
                host.answered(
                    plugin,
                    "fill_command button",
                    &label,
                    None,
                    outcome,
                    false,
                    cx,
                );
            });
            return;
        }
        self.ask(
            plugin,
            Asked::Command(command),
            Some(pane),
            None,
            true,
            window,
            cx,
        );
    }

    /// Puts `asked` on `pane`'s bar, if the host lets it wait there.
    #[allow(clippy::too_many_arguments)]
    fn ask(
        &mut self,
        plugin: &str,
        asked: Asked,
        pane: Option<LeafId>,
        id: Option<Id>,
        button: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.plugin_host() else {
            return;
        };
        let kind = asked.kind();
        if let Asked::Command(command) = &asked
            && let Some(why) = command.refusal()
        {
            let why = format!("`{kind}` was refused: {why}");
            let label = pane.map_or("no pane".to_owned(), |pane| self.pane_label(pane, cx));
            host.update(cx, |host, cx| {
                host.dropped(plugin, why, cx);
                host.answered(plugin, kind, &label, id, Outcome::Refused, false, cx);
            });
            return;
        }
        let Some(pane) = pane else {
            host.update(cx, |host, cx| {
                host.answered(plugin, kind, "no pane", id, Outcome::NoPane, false, cx);
            });
            return;
        };
        let may = host.update(cx, |host, cx| host.may_ask(plugin, kind, id.clone(), cx));
        if !may {
            return;
        }
        let name = host
            .read(cx)
            .name_of(plugin)
            .unwrap_or_else(|| plugin.to_owned());
        let Some(plugins) = &mut self.plugins else {
            return;
        };
        plugins.requests.push(Request {
            plugin: plugin.to_owned(),
            name,
            pane,
            asked,
            id,
            button,
            since: Instant::now(),
        });
        cx.notify();
    }

    /// The terminal pane a request without a pane goes to: the shown
    /// tab's focused pane, if it is a terminal.
    fn terminal_target(&self) -> Option<LeafId> {
        let focused = self.tabs.active().focused;
        self.panes.contains_key(&focused).then_some(focused)
    }

    /// A pane as a bar and the log name it: its title, and `user@host` for
    /// an SSH pane (TM-plug-06).
    fn pane_label(&self, pane: LeafId, cx: &gpui::App) -> String {
        let title = self.pane_title(pane, cx).to_string();
        match self
            .panes
            .get(&pane)
            .and_then(|view| view.read(cx).ssh_target())
        {
            Some(target) if !title.contains(&target) => format!("{title} ({target})"),
            _ => title,
        }
    }

    /// Allow or Deny on the request at `ix`.
    fn answer_request(
        &mut self,
        ix: usize,
        allow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(plugins) = &mut self.plugins else {
            return;
        };
        if ix >= plugins.requests.len() {
            return;
        }
        if allow && plugins.requests[ix].since.elapsed() < ALLOW_GUARD {
            return;
        }
        let request = plugins.requests.remove(ix);
        // The next request on the pane comes up now: its Allow waits too.
        if let Some(next) = plugins
            .requests
            .iter_mut()
            .find(|next| next.pane == request.pane)
        {
            next.since = Instant::now();
        }
        let host = plugins.host.clone();
        let label = self.pane_label(request.pane, cx);
        let kind = match request.button {
            true => format!("{} button", request.asked.kind()),
            false => request.asked.kind().to_owned(),
        };
        let id = request.id.clone().filter(|_| !request.button);
        let enabled = host.read(cx).is_enabled(&request.plugin);
        let outcome = match (allow, enabled) {
            (false, _) => Outcome::Denied,
            (true, false) => Outcome::Refused,
            (true, true) => self.carry_out(&request, window, cx),
        };
        host.update(cx, |host, cx| {
            host.answered(
                &request.plugin,
                &kind,
                &label,
                id.clone(),
                outcome,
                true,
                cx,
            );
            if let (Outcome::Allowed, Asked::Read(text)) = (outcome, &request.asked) {
                host.send_output(&request.plugin, id, text.clone(), cx);
            }
        });
        cx.notify();
    }

    /// Does what the user allowed.
    fn carry_out(
        &mut self,
        request: &Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let pane = self.panes.get(&request.pane).cloned();
        match &request.asked {
            Asked::Command(Command::Fill { text }) | Asked::Command(Command::Run { text }) => {
                let enter = matches!(request.asked, Asked::Command(Command::Run { .. }));
                match pane.is_some_and(|pane| pane.read(cx).type_for_plugin(text, enter)) {
                    true => Outcome::Allowed,
                    false => Outcome::NoPane,
                }
            }
            Asked::Command(Command::OpenTab { argv }) => {
                let profile = Profile {
                    id: format!("plugin-{}", request.plugin),
                    name: Some(request.name.clone()),
                    theme: None,
                    kind: ProfileKind::Local(LocalProfile {
                        command: Some(argv.clone()),
                        cwd: None,
                        env: Default::default(),
                    }),
                };
                let leaf = self.tabs.new_tab();
                self.spawn_pane(leaf, profile, None, window, cx);
                self.show_active(window, cx);
                Outcome::Allowed
            }
            Asked::Read(_) => Outcome::Allowed,
        }
    }

    /// `plugin.allowRequest` and `plugin.denyRequest`: the focused pane's
    /// request (spec 0016, Confirmation).
    pub(super) fn allow_plugin_request(
        &mut self,
        _: &AllowPluginRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.focused_request() {
            self.answer_request(ix, true, window, cx);
        }
    }

    pub(super) fn deny_plugin_request(
        &mut self,
        _: &DenyPluginRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.focused_request() {
            self.answer_request(ix, false, window, cx);
        }
    }

    fn focused_request(&self) -> Option<usize> {
        let focused = self.tabs.active().focused;
        let requests = &self.plugins.as_ref()?.requests;
        requests.iter().position(|request| request.pane == focused)
    }

    /// A pane closed under its requests: each is dropped, and its plugin
    /// told `no_pane` (flow 5).
    pub(super) fn drop_requests_of(&mut self, pane: LeafId, cx: &mut Context<Self>) {
        let Some(plugins) = &mut self.plugins else {
            return;
        };
        let (gone, kept) = std::mem::take(&mut plugins.requests)
            .into_iter()
            .partition(|request| request.pane == pane);
        plugins.requests = kept;
        let host = plugins.host.clone();
        for request in gone {
            let Request {
                plugin,
                asked,
                id,
                button,
                ..
            } = request;
            let id = id.filter(|_| !button);
            host.update(cx, |host, cx| {
                host.answered(
                    &plugin,
                    asked.kind(),
                    "a closed pane",
                    id,
                    Outcome::NoPane,
                    true,
                    cx,
                );
            });
        }
    }

    /// A plugin's manifest command from the palette or a key (spec 0016,
    /// Commands); a missing plugin or command says so.
    pub(super) fn run_plugin_command(
        &mut self,
        command: &PluginCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.plugin_host() else {
            show_notice("Plugins haven't been looked for yet".into(), window, cx);
            return;
        };
        let ran = host.update(cx, |host, cx| {
            host.run_command(&command.plugin, &command.command, cx)
        });
        if let Err(why) = ran {
            show_notice(why, window, cx);
        }
    }

    /// Whether the focused pane has a request, for the palette.
    pub(super) fn has_focused_request(&self) -> bool {
        self.focused_request().is_some()
    }

    /// The bar over `pane`, if a request waits on it.
    pub(super) fn request_bar(&self, pane: LeafId, cx: &mut Context<Self>) -> Option<AnyElement> {
        let plugins = self.plugins.as_ref()?;
        let ix = plugins
            .requests
            .iter()
            .position(|request| request.pane == pane)?;
        let request = &plugins.requests[ix];
        let theme = cx.theme();
        let (muted, border, background) = (theme.muted_foreground, theme.border, theme.popover);
        let label = self.pane_label(pane, cx);
        let verb = match &request.asked {
            Asked::Command(Command::Fill { .. }) => "type into",
            Asked::Command(Command::Run { .. }) => "run in",
            Asked::Command(Command::OpenTab { .. }) => "open a new tab from",
            Asked::Read(_) => "read from",
        };
        let what: AnyElement = match &request.asked {
            Asked::Command(command) => {
                command::shown(command, self.font.family.clone(), cx).into_any_element()
            }
            Asked::Read(text) => {
                let lines = if text.is_empty() {
                    0
                } else {
                    text.lines().count()
                };
                let chars = text.chars().count();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_xs()
                            .child(format!("{lines} lines, {chars} characters:")),
                    )
                    .child(
                        div()
                            .id(("plugin-read-preview", pane))
                            .max_h(px(120.))
                            .overflow_y_scroll()
                            .text_xs()
                            .font_family(self.font.family.clone())
                            .child(text.clone()),
                    )
                    .into_any_element()
            }
        };
        let answer = |label: &'static str, allow: bool, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("plugin-{label}-{pane}")))
                .px(px(10.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .border_color(border)
                .text_sm()
                .cursor_pointer()
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(ix) = this.request_on(pane) {
                        this.answer_request(ix, allow, window, cx);
                    }
                }))
        };
        Some(
            div()
                .id(("plugin-request", pane))
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .occlude()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .bg(background)
                .border_b_1()
                .border_color(border)
                .shadow_md()
                .child(
                    div()
                        .text_sm()
                        .child(format!("Plugin {} wants to {verb} {label}:", request.name)),
                )
                .child(what)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(answer("Allow", true, cx))
                        .child(answer("Deny", false, cx))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Typing still goes to the pane."),
                        ),
                )
                .into_any_element(),
        )
    }

    fn request_on(&self, pane: LeafId) -> Option<usize> {
        let requests = &self.plugins.as_ref()?.requests;
        requests.iter().position(|request| request.pane == pane)
    }
}

/// Spec 0016, Confirmation, Command buttons, Untrusted text and flow 5,
/// against real panes: each runs a program that records what reaches its
/// PTY in `typed-<pane>`, so what was typed is what the file holds.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::commands::{ClosePane, FocusLeft, FocusRight};
    use crate::layout::plugins::tests::{start, view};
    use crate::layout::sidebar::tests::{Rig, two_hosts};
    use crate::layout::tests::open_workspace;
    use crate::layout::tree::Axis;
    use crate::layout::{Boot, BootPane};
    use crate::plugins::tests::{Lab, events, grant, names, settle, wait_for_events, wait_until};
    use crate::plugins::{Host, Right};
    use crate::session::Session;
    use crate::session::tests::{leaf, split, tab};
    use crate::sidebar::plugin::PluginPanelEvent;
    use crate::terminal::pty::{self, GridSize};
    use crate::terminal::{Launch, Start};
    use crate::theme;
    use crate::timing::Timing;
    use gpui::{Entity, TestAppContext};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// The plugin that asks; its folder is the lab's `asker`.
    const ASKER: &str = "asker";
    const MANIFEST: &str = concat!(
        "rights: [terminal_write, terminal_read, open_tab]\n",
        "commands: [{ id: go, title: Go }]\n",
        "panels: [{ id: main, title: Main }]\n",
    );

    /// A pane's program: two lines of output, then everything that
    /// reaches its PTY, byte for byte, into `typed-<pane>`. Raw mode, so
    /// text without Enter arrives too; echo stays on, so typing shows.
    fn recorder(pane: LeafId) -> pty::Command {
        let script =
            format!("printf 'one\\ntwo\\n'; stty raw; : > ready-{pane}; exec cat > typed-{pane}");
        pty::Command {
            program: "sh".into(),
            args: vec!["-c".into(), script],
            env: BTreeMap::new(),
        }
    }

    /// One tab of two recording panes side by side, pane 1 focused and
    /// pane 4 beside it, both in `dir`.
    fn recording_rig<'a>(dir: &Path, cx: &'a mut TestAppContext) -> Rig<'a> {
        cx.executor().allow_parking();
        let panes = split(
            Axis::Row,
            0.5,
            leaf(1, "shell", None, None),
            leaf(4, "shell", None, None),
        );
        let session = Session::new(None, vec![tab(panes, 1)], 0);
        let config = two_hosts();
        let boot = Boot::restore(&session, (1200.0, 800.0), |saved, area| {
            let profile = config.profile(&saved.profile).expect("a known profile");
            let launch = Launch {
                command: recorder(saved.id),
                cwd: Some(dir.to_owned()),
                timing: Arc::new(Timing::new(Instant::now())),
            };
            let started = pty::spawn(
                &launch.command,
                GridSize::fit(area, (8.0, 16.0)),
                launch.cwd.as_deref(),
                1000,
                false,
                Arc::clone(&launch.timing),
            );
            BootPane {
                start: Start::Started(started),
                launch,
                profile: (&profile).into(),
                theme: theme::DARK,
                title: saved.title.clone(),
            }
        })
        .expect("a session with tabs");
        let (workspace, cx) = open_workspace(boot, config, cx);
        Rig { workspace, cx }
    }

    /// The lab with the asking plugin, the workspace of two recording
    /// panes with its host started, and the plugin's folder. Both panes
    /// are recording and show their two lines.
    fn bench<'a>(name: &str, cx: &'a mut TestAppContext) -> (Lab, PathBuf, Rig<'a>, Entity<Host>) {
        let lab = Lab::new(&format!("requests-{name}"));
        let folder = lab.plugin(ASKER, "stay", MANIFEST);
        let every = [Right::TerminalWrite, Right::TerminalRead, Right::OpenTab];
        let mut rig = recording_rig(&lab.root(), cx);
        let host = start(&mut rig, &lab, &[(ASKER, grant("stay", &every, &[]))]);
        let root = lab.root();
        for pane in [1, 4] {
            wait_until(rig.cx, "the pane to record", |cx| {
                let shown = rig.workspace.read_with(cx, |workspace, cx| {
                    workspace.panes[&pane].read(cx).output(10)
                });
                root.join(format!("ready-{pane}")).exists() && shown.ends_with("two")
            });
        }
        (lab, folder, rig, host)
    }

    /// What reached pane `pane`'s PTY so far.
    fn typed(dir: &Path, pane: LeafId) -> String {
        std::fs::read_to_string(dir.join(format!("typed-{pane}"))).unwrap_or_default()
    }

    /// Waits for pane `pane` to have got exactly `text`, and nothing more
    /// after a while.
    fn wait_typed(rig: &mut Rig<'_>, dir: &Path, pane: LeafId, text: &str) {
        wait_until(rig.cx, &format!("{text:?} in pane {pane}"), |_| {
            typed(dir, pane).len() >= text.len()
        });
        settle(rig.cx);
        assert_eq!(typed(dir, pane), text, "pane {pane}");
    }

    /// The requests waiting on bars: the pane and the kind of each.
    fn pending(rig: &mut Rig<'_>) -> Vec<(LeafId, &'static str)> {
        rig.draw();
        rig.workspace.read_with(rig.cx, |workspace, _| {
            let requests = &workspace.plugins.as_ref().expect("a host").requests;
            let shown = requests
                .iter()
                .map(|request| (request.pane, request.asked.kind()));
            shown.collect()
        })
    }

    /// Whether pane `pane` draws a bar.
    fn has_bar(rig: &mut Rig<'_>, pane: LeafId) -> bool {
        rig.workspace.update(rig.cx, |workspace, cx| {
            workspace.request_bar(pane, cx).is_some()
        })
    }

    /// `action` of the asking plugin arrives, its right already checked by
    /// the host.
    fn arrives(rig: &mut Rig<'_>, action: Action) {
        rig.workspace.update_in(rig.cx, |workspace, window, cx| {
            workspace.plugin_request(ASKER, &action, window, cx);
        });
        rig.draw();
    }

    fn run(text: &str, id: u64, pane: Option<LeafId>) -> Action {
        Action::RunCommand {
            id: Some(Id::Number(id)),
            pane,
            text: text.into(),
        }
    }

    fn fill(text: &str, id: u64) -> Action {
        Action::FillCommand {
            id: Some(Id::Number(id)),
            pane: None,
            text: text.into(),
        }
    }

    /// Answers the focused pane's request from the palette, once Allow
    /// takes clicks.
    fn allow(rig: &mut Rig<'_>) {
        std::thread::sleep(ALLOW_GUARD + Duration::from_millis(100));
        rig.run(AllowPluginRequest);
    }

    fn deny(rig: &mut Rig<'_>) {
        rig.run(DenyPluginRequest);
    }

    /// The `action_result`s the plugin got so far: each one's `id` and
    /// result.
    fn results(folder: &Path) -> Vec<(Value, String)> {
        let got = events(folder);
        let results = got.iter().filter(|event| event["event"] == "action_result");
        let said = results.map(|event| {
            let result = event["result"].as_str().expect("a result").to_owned();
            (event["id"].clone(), result)
        });
        said.collect()
    }

    /// Waits for the plugin's `count`th `action_result`, and returns all.
    fn wait_for_results(rig: &mut Rig<'_>, folder: &Path, count: usize) -> Vec<(Value, String)> {
        let what = format!("{count} action results");
        wait_until(rig.cx, &what, |_| results(folder).len() >= count);
        results(folder)
    }

    fn result(id: u64, result: &str) -> (Value, String) {
        (Value::from(id), result.to_owned())
    }

    fn log(rig: &mut Rig<'_>, host: &Entity<Host>) -> Vec<String> {
        host.read_with(rig.cx, |host, _| {
            host.log_of(ASKER).map(str::to_owned).collect()
        })
    }

    /// Runs `plugin.asker.go`, as the palette or a key does.
    fn run_its_command(rig: &mut Rig<'_>) {
        rig.run(PluginCommand {
            plugin: ASKER.into(),
            command: "go".into(),
        });
    }

    /// AC4, Confirmation [load-bearing], flow 5, from the plugin's own
    /// line on: `run_command` shows a bar on the focused pane and types
    /// nothing until Allow; typing `ls` and Enter meanwhile goes to the
    /// shell and confirms nothing; Allow ignores the first 0.5 s; then it
    /// types exactly the text and Enter, and the plugin gets `allowed`
    /// with its `id`. The log line names the kind, the pane and the
    /// outcome, never the text (TM-plug-07).
    #[gpui::test]
    fn a_run_command_waits_for_allow_and_then_types_its_text_and_enter(cx: &mut TestAppContext) {
        let (lab, folder, mut rig, host) = bench("run", cx);
        let dir = lab.root();
        let ask = r#"{"action":"run_command","id":7,"text":"echo SECRET-RUN"}"#;
        std::fs::write(folder.join("say"), format!("{ask}\n")).expect("write say");
        run_its_command(&mut rig);
        wait_until(rig.cx, "the bar", |cx| {
            rig.workspace.read_with(cx, |workspace, _| {
                workspace
                    .plugins
                    .as_ref()
                    .is_some_and(|p| !p.requests.is_empty())
            })
        });
        assert_eq!(pending(&mut rig), [(1, "run_command")]);
        assert!(has_bar(&mut rig, 1));
        assert!(!has_bar(&mut rig, 4));
        assert!(rig.pane_has_the_keyboard(1), "the bar took the keyboard");

        rig.run(AllowPluginRequest);
        assert_eq!(pending(&mut rig).len(), 1, "Allow took a click at once");

        rig.keys("l s enter");
        wait_typed(&mut rig, &dir, 1, "ls\r");
        assert_eq!(pending(&mut rig).len(), 1, "Enter confirmed the bar");

        allow(&mut rig);
        wait_typed(&mut rig, &dir, 1, "ls\recho SECRET-RUN\r");
        assert_eq!(typed(&dir, 4), "", "it typed into another pane");
        assert_eq!(pending(&mut rig), []);
        assert!(!has_bar(&mut rig, 1));
        let got = wait_for_events(rig.cx, &folder, 2);
        assert_eq!(names(&got), ["command", "action_result"]);
        assert_eq!(results(&folder), [result(7, "allowed")]);

        let label = rig
            .workspace
            .read_with(rig.cx, |workspace, cx| workspace.pane_label(1, cx));
        let said = log(&mut rig, &host);
        assert_eq!(said.len(), 1, "{said:?}");
        for part in ["run_command", label.as_str(), "allowed", "UTC"] {
            assert!(said[0].contains(part), "no {part:?} in {said:?}");
        }
        assert!(!said.iter().any(|line| line.contains("SECRET")), "{said:?}");
    }

    /// AC4 and Confirmation: Deny writes nothing and the plugin gets
    /// `denied`. Its next confirmed actions are `refused`, with no bar,
    /// until the user runs one of its commands; and again after the next
    /// Deny, until the user acts in one of its panels.
    #[gpui::test]
    fn deny_writes_nothing_and_refuses_the_plugin_until_the_user_acts_on_it(
        cx: &mut TestAppContext,
    ) {
        let (lab, folder, mut rig, _host) = bench("deny", cx);
        let dir = lab.root();
        arrives(&mut rig, run("rm -rf build", 1, None));
        assert_eq!(pending(&mut rig).len(), 1);
        deny(&mut rig);
        assert_eq!(pending(&mut rig), []);
        assert_eq!(
            wait_for_results(&mut rig, &folder, 1),
            [result(1, "denied")]
        );

        arrives(&mut rig, run("echo again", 2, None));
        arrives(&mut rig, fill("echo again", 3));
        assert_eq!(pending(&mut rig), [], "a bar after a Deny");
        assert!(!has_bar(&mut rig, 1));
        assert_eq!(
            wait_for_results(&mut rig, &folder, 3)[1..],
            [result(2, "refused"), result(3, "refused")]
        );

        run_its_command(&mut rig);
        arrives(&mut rig, run("echo after the command", 4, None));
        assert_eq!(pending(&mut rig), [(1, "run_command")]);
        deny(&mut rig);
        arrives(&mut rig, run("echo refused", 5, None));
        assert_eq!(pending(&mut rig), []);

        let panel = view(&mut rig, ASKER, "main");
        panel.update(rig.cx, |_, cx| {
            cx.emit(PluginPanelEvent::Action("refresh".into()));
        });
        rig.draw();
        arrives(&mut rig, run("echo after the panel", 6, None));
        assert_eq!(pending(&mut rig), [(1, "run_command")]);
        assert_eq!(
            wait_for_results(&mut rig, &folder, 5)[3..],
            [result(4, "denied"), result(5, "refused")]
        );
        settle(rig.cx);
        assert_eq!(typed(&dir, 1), "", "a denied or refused request typed");
    }

    /// Confirmation: "One request per plugin is pending at a time; more
    /// are refused." The waiting one stays, and Allow carries out only it.
    #[gpui::test]
    fn a_second_request_while_one_waits_is_refused(cx: &mut TestAppContext) {
        let (lab, folder, mut rig, _host) = bench("second", cx);
        let dir = lab.root();
        arrives(&mut rig, run("echo first", 1, None));
        arrives(&mut rig, run("echo second", 2, Some(4)));
        arrives(&mut rig, fill("echo third", 3));
        assert_eq!(pending(&mut rig), [(1, "run_command")]);
        assert!(!has_bar(&mut rig, 4));
        assert_eq!(
            wait_for_results(&mut rig, &folder, 2),
            [result(2, "refused"), result(3, "refused")]
        );

        allow(&mut rig);
        wait_typed(&mut rig, &dir, 1, "echo first\r");
        assert_eq!(typed(&dir, 4), "");
        assert_eq!(
            wait_for_results(&mut rig, &folder, 3)[2],
            result(1, "allowed")
        );
    }

    /// AC7, Untrusted text [load-bearing]: `fill_command` or `run_command`
    /// text with a control character (newline, ESC) or a format character
    /// (U+202E, a zero-width space), or over 4 KiB, is refused with no
    /// bar. Exactly 4 KiB, and non-ASCII, still get one.
    #[gpui::test]
    fn text_with_a_control_or_format_character_or_over_4_kib_gets_no_bar(cx: &mut TestAppContext) {
        let (lab, folder, mut rig, _host) = bench("untrusted", cx);
        let dir = lab.root();
        let over = "a".repeat(4 * 1024 + 1);
        let refused = [
            fill("ls\nrm -rf ~", 1),
            fill("echo \u{1b}[31mred", 2),
            fill("echo \u{202e}txt.exe", 3),
            run("cu\u{200b}rl example.com", 4, None),
            run("echo\ttab", 5, None),
            fill(&over, 6),
        ];
        for action in refused {
            arrives(&mut rig, action);
            assert_eq!(pending(&mut rig), [], "a bar for refused text");
        }
        let got = wait_for_results(&mut rig, &folder, 6);
        let outcomes: Vec<&str> = got.iter().map(|(_, result)| result.as_str()).collect();
        assert_eq!(outcomes, ["refused"; 6]);

        // A refusal isn't a Deny: the plugin's next request gets its bar.
        let full = "a".repeat(4 * 1024);
        arrives(&mut rig, fill(&full, 7));
        assert_eq!(pending(&mut rig), [(1, "fill_command")]);
        deny(&mut rig);
        arrives(&mut rig, run("сurl example.com", 8, None));
        assert_eq!(pending(&mut rig), [], "refused after the Deny");
        run_its_command(&mut rig);
        arrives(&mut rig, run("сurl example.com", 9, None));
        assert_eq!(
            pending(&mut rig),
            [(1, "run_command")],
            "non-ASCII is shown"
        );
        settle(rig.cx);
        assert_eq!(typed(&dir, 1), "");
    }

    /// AC6, flow 5: `read_output` sends nothing until Allow, and then
    /// exactly the text its bar previewed, read when it arrived: what the
    /// pane shows by Allow isn't in it. With Deny the plugin gets
    /// `denied` and no text.
    #[gpui::test]
    fn read_output_sends_exactly_what_its_bar_showed(cx: &mut TestAppContext) {
        let (_lab, folder, mut rig, _host) = bench("read", cx);
        let read = |id: u64| Action::ReadOutput {
            id: Some(Id::Number(id)),
            pane: Some(4),
            lines: Some(5),
        };
        arrives(&mut rig, read(1));
        assert_eq!(pending(&mut rig), [(4, "read_output")]);
        let shown = rig.workspace.read_with(rig.cx, |workspace, _| {
            match &workspace.plugins.as_ref().expect("a host").requests[0].asked {
                Asked::Read(text) => text.clone(),
                Asked::Command(_) => unreachable!("a read was asked"),
            }
        });
        assert_eq!(shown, "one\ntwo");

        // Typing in pane 4 echoes there, after the bar came up.
        rig.run(FocusRight);
        rig.keys("x y z");
        wait_until(rig.cx, "the echo", |cx| {
            rig.workspace.read_with(cx, |workspace, cx| {
                workspace.panes[&4].read(cx).output(5).contains("xyz")
            })
        });
        settle(rig.cx);
        assert_eq!(events(&folder), [Value::Null; 0], "sent before Allow");

        allow(&mut rig);
        let got = wait_for_events(rig.cx, &folder, 2);
        assert_eq!(names(&got), ["action_result", "output"]);
        assert_eq!(got[0]["result"], "allowed");
        assert_eq!(got[1]["id"], 1);
        assert_eq!(got[1]["text"], shown.as_str());

        arrives(&mut rig, read(2));
        deny(&mut rig);
        let got = wait_for_events(rig.cx, &folder, 3);
        settle(rig.cx);
        assert_eq!(
            names(&events(&folder)),
            ["action_result", "output", "action_result"]
        );
        assert_eq!(got[2]["id"], 2);
        assert_eq!(got[2]["result"], "denied");
    }

    /// Confirmation: the target is the action's `pane`, else the pane
    /// focused when it arrived, and "never follows focus later"; the
    /// palette's Allow and Deny act on the focused pane's request only.
    #[gpui::test]
    fn a_request_keeps_its_pane_and_the_palette_answers_the_focused_ones(cx: &mut TestAppContext) {
        let (lab, folder, mut rig, _host) = bench("target", cx);
        let dir = lab.root();
        arrives(&mut rig, fill("make", 1));
        assert_eq!(pending(&mut rig), [(1, "fill_command")]);

        rig.run(FocusRight);
        assert!(rig.pane_has_the_keyboard(4));
        assert_eq!(
            pending(&mut rig),
            [(1, "fill_command")],
            "it followed focus"
        );
        allow(&mut rig);
        deny(&mut rig);
        assert_eq!(pending(&mut rig).len(), 1, "answered from another pane");

        rig.run(FocusLeft);
        allow(&mut rig);
        wait_typed(&mut rig, &dir, 1, "make");
        assert_eq!(typed(&dir, 4), "");

        rig.run(FocusRight);
        arrives(&mut rig, fill("make test", 2));
        assert_eq!(pending(&mut rig), [(4, "fill_command")]);
        allow(&mut rig);
        wait_typed(&mut rig, &dir, 4, "make test");
        assert_eq!(
            wait_for_results(&mut rig, &folder, 2),
            [result(1, "allowed"), result(2, "allowed")]
        );
    }

    /// Flow 5: the target pane closing first drops the request, and the
    /// plugin gets `no_pane`. Its next request may wait on a bar again.
    #[gpui::test]
    fn closing_the_target_pane_drops_its_request_with_no_pane(cx: &mut TestAppContext) {
        let (_lab, folder, mut rig, host) = bench("close", cx);
        arrives(&mut rig, run("echo gone", 1, Some(1)));
        assert_eq!(pending(&mut rig), [(1, "run_command")]);
        rig.run(ClosePane);
        assert!(
            !rig.workspace
                .read_with(rig.cx, |w, _| w.panes.contains_key(&1))
        );
        assert_eq!(pending(&mut rig), []);
        assert_eq!(
            wait_for_results(&mut rig, &folder, 1),
            [result(1, "no_pane")]
        );
        let said = log(&mut rig, &host);
        assert!(
            said.iter().any(|line| line.contains("run_command")),
            "{said:?}"
        );
        assert!(
            !said.iter().any(|line| line.contains("echo gone")),
            "{said:?}"
        );

        arrives(&mut rig, run("echo next", 2, None));
        assert_eq!(pending(&mut rig), [(4, "run_command")]);
    }

    /// AC10, Command buttons [load-bearing]: a `fill_command` button types
    /// its text into the focused terminal pane at once, without Enter and
    /// without a bar, and the plugin gets `panel_action`. A `run_command`
    /// button writes nothing on its click and raises the bar; Allow types
    /// the text and Enter, Deny writes nothing. Each adds a log line
    /// without the text.
    #[gpui::test]
    fn a_fill_button_types_at_once_and_a_run_button_raises_the_bar(cx: &mut TestAppContext) {
        let (lab, folder, mut rig, host) = bench("buttons", cx);
        let dir = lab.root();
        let panel = view(&mut rig, ASKER, "main");
        let click = |rig: &mut Rig<'_>, id: &str, command: Command| {
            panel.update(rig.cx, |_, cx| {
                cx.emit(PluginPanelEvent::Command {
                    id: id.into(),
                    command,
                });
            });
            rig.draw();
        };

        rig.run(FocusRight);
        click(
            &mut rig,
            "status",
            Command::Fill {
                text: "git status".into(),
            },
        );
        assert_eq!(pending(&mut rig), [], "a fill button raised a bar");
        wait_typed(&mut rig, &dir, 4, "git status");
        assert_eq!(typed(&dir, 1), "");
        let got = wait_for_events(rig.cx, &folder, 1);
        assert_eq!(names(&got[..1]), ["panel_action"]);
        assert_eq!(got[0]["id"], "status");

        click(
            &mut rig,
            "build",
            Command::Run {
                text: "make".into(),
            },
        );
        assert_eq!(pending(&mut rig), [(4, "run_command")]);
        settle(rig.cx);
        assert_eq!(
            typed(&dir, 4),
            "git status",
            "a run button wrote on its click"
        );
        allow(&mut rig);
        wait_typed(&mut rig, &dir, 4, "git statusmake\r");

        click(
            &mut rig,
            "build",
            Command::Run {
                text: "make".into(),
            },
        );
        assert_eq!(pending(&mut rig), [(4, "run_command")]);
        deny(&mut rig);
        settle(rig.cx);
        assert_eq!(typed(&dir, 4), "git statusmake\r", "Deny wrote");

        let said = log(&mut rig, &host);
        assert_eq!(said.len(), 3, "{said:?}");
        assert!(
            said[0].contains("fill_command") && said[0].contains("allowed"),
            "{said:?}"
        );
        assert!(
            said[1].contains("run_command") && said[1].contains("allowed"),
            "{said:?}"
        );
        assert!(
            said[2].contains("run_command") && said[2].contains("denied"),
            "{said:?}"
        );
        let texts = ["git status", "make"];
        assert!(
            !said
                .iter()
                .any(|line| texts.iter().any(|text| line.contains(text))),
            "{said:?}"
        );
    }

    /// Actions, `open_tab`: nothing opens before Allow; then a tab running
    /// exactly the argv opens and is shown.
    #[gpui::test]
    fn an_allowed_open_tab_opens_a_tab_running_its_argv(cx: &mut TestAppContext) {
        let (lab, folder, mut rig, _host) = bench("open-tab", cx);
        let opened = lab.root().join("opened");
        // It stays, reading its PTY, so its tab does too.
        let script = format!("printf '%s' \"$1\" > '{}'; exec cat", opened.display());
        let argv = vec!["sh".into(), "-c".into(), script, "sh".into(), "a b".into()];
        let tabs = |rig: &mut Rig<'_>| rig.workspace.read_with(rig.cx, |w, _| w.tabs.tabs().len());
        arrives(
            &mut rig,
            Action::OpenTab {
                id: Some(Id::Number(1)),
                argv,
            },
        );
        assert_eq!(pending(&mut rig), [(1, "open_tab")]);
        settle(rig.cx);
        assert_eq!(tabs(&mut rig), 1);
        assert!(!opened.exists(), "it ran before Allow");

        allow(&mut rig);
        assert_eq!(tabs(&mut rig), 2);
        wait_until(rig.cx, "the tab's program", |_| {
            std::fs::read_to_string(&opened).is_ok_and(|text| text == "a b")
        });
        assert_eq!(
            wait_for_results(&mut rig, &folder, 1),
            [result(1, "allowed")]
        );
    }
}
