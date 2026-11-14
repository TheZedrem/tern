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
