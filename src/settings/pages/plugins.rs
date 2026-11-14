//! The Plugins tab (spec 0016, flows 2, 6 and 8): every found plugin under
//! the trusted-source warning, enabled only after a confirmation that
//! repeats it with the command line and folder, its rights, events and
//! settings ticked and typed here, Restart, and its log. Every change
//! writes the plugin's whole entry under `plugins:`.

use crate::config::{PluginSettings, Setting};
use crate::layout::answers::{Answer, Answers};
use crate::plugins::{FieldKind, ListedPlugin, State};
use crate::settings::change::Change;
use crate::settings::{Page, Row, SettingsView, number};
use crate::terminal::DEFAULT_FONT_FAMILY as MONOSPACE;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme, Disableable, Selectable, Sizable, WindowExt};
use std::collections::HashSet;
use std::rc::Rc;

/// The warning at the top of the tab and in every enable confirmation
/// (spec 0016, Trusted sources only).
const WARNING: &str = "A plugin is a program that runs as you, with your access to \
    files and the network. Tern can't check what it does: install plugins only \
    from a source you trust.";
/// The note above a plugin's settings (TM-plug-18).
const PLAIN_TEXT: &str = "Saved in config.yaml as plain text: no place for passwords \
    or tokens.";
/// How many of a plugin's last log lines the tab shows.
const LOG_SHOWN: usize = 200;

/// The typed settings fields of the plugins, and whose log is open.
#[derive(Default)]
pub struct PluginFields {
    /// (plugin, key) and its field.
    inputs: Vec<(String, String, Entity<InputState>)>,
    logs: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    /// The plugins the host found; none before the first frame.
    fn listed_plugins(&self, cx: &gpui::App) -> Vec<ListedPlugin> {
        self.host
            .as_ref()
            .map(|host| host.read(cx).plugins())
            .unwrap_or_default()
    }

    /// What `config.yaml` holds for plugin `id`, the start of every
    /// change to it.
    fn plugin_entry(&self, id: &str) -> PluginSettings {
        self.config.plugins.get(id).cloned().unwrap_or_default()
    }

    fn write_plugin(&self, id: &str, entry: PluginSettings, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.set(Change::Plugin { id, entry }, cx);
    }

    /// Makes a field for each text and number setting, and shows what
    /// the file says in those not being typed into.
    pub(in crate::settings) fn sync_plugin_fields(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for plugin in self.listed_plugins(cx) {
            for field in &plugin.fields {
                if !matches!(field.kind, FieldKind::Text | FieldKind::Number) {
                    continue;
                }
                let value = self
                    .plugin_entry(&plugin.id)
                    .settings
                    .get(&field.key)
                    .map(setting_text)
                    .unwrap_or_default();
                let known = self
                    .plugins
                    .inputs
                    .iter()
                    .find(|(id, key, _)| *id == plugin.id && *key == field.key);
                if let Some((_, _, input)) = known {
                    let input = input.clone();
                    let typing = input.read(cx).focus_handle(cx).is_focused(window);
                    if !typing && input.read(cx).value() != value {
                        input.update(cx, |input, cx| input.set_value(value, window, cx));
                    }
                    continue;
                }
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                let (id, key, number) = (
                    plugin.id.clone(),
                    field.key.clone(),
                    field.kind == FieldKind::Number,
                );
                let subscription = cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, _, cx| {
                        if let InputEvent::PressEnter { .. } | InputEvent::Blur = event {
                            let typed = input.read(cx).value().trim().to_owned();
                            this.commit_plugin_setting(&id, &key, number, typed, cx);
                        }
                    },
                );
                self.plugins._subscriptions.push(subscription);
                self.plugins
                    .inputs
                    .push((plugin.id.clone(), field.key.clone(), input));
            }
        }
    }

    /// Writes a typed setting that changed; a number that isn't one is
    /// left unwritten.
    fn commit_plugin_setting(
        &mut self,
        id: &str,
        key: &str,
        number: bool,
        typed: String,
        cx: &mut Context<Self>,
    ) {
        let value = match number {
            true => match typed.parse::<f64>() {
                Ok(value) if value.is_finite() => Setting::Number(value),
                _ => return,
            },
            false => Setting::Text(typed),
        };
        self.set_plugin_setting(id, key, value, cx);
    }

    fn set_plugin_setting(&mut self, id: &str, key: &str, value: Setting, cx: &mut Context<Self>) {
        let mut entry = self.plugin_entry(id);
        if entry.settings.get(key) == Some(&value) {
            return;
        }
        entry.settings.insert(key.to_owned(), value);
        self.write_plugin(id, entry, cx);
    }

    /// The confirmation every enable asks for, with the warning, the
    /// command line and the folder; Cancel is what Enter means
    /// (spec 0016, flow 2; TM-plug-12).
    fn confirm_enable(
        &mut self,
        plugin: &ListedPlugin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let (id, run) = (plugin.id.clone(), plugin.run.clone());
        let enable = Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
            let (id, run) = (id.clone(), run.clone());
            let _ = view.update(cx, |this, cx| {
                let mut entry = this.plugin_entry(&id);
                entry.enabled = true;
                entry.run = run;
                this.write_plugin(&id, entry, cx);
            });
        });
        let answers = Answers::new(
            vec![
                Answer {
                    label: "Cancel".into(),
                    style: |button| button,
                    act: Rc::new(|_, _| {}),
                },
                Answer {
                    label: "Enable".into(),
                    style: |button| button.primary(),
                    act: enable,
                },
            ],
            0,
            cx,
        );
        answers.focus_safe(window);
        let title = format!("Enable {}?", plugin.name);
        let command = plugin.run.join(" ");
        let folder = plugin.folder.display().to_string();
        window.open_dialog(cx, move |dialog, _, _| {
            answers.apply(dialog).title(title.clone()).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_sm()
                    .child(WARNING)
                    .child("It runs this command line:")
                    .child(
                        div()
                            .pl(px(8.))
                            .font_family(MONOSPACE)
                            .child(command.clone()),
                    )
                    .child(format!("from {folder}")),
            )
        });
    }

    pub(in crate::settings) fn plugin_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let muted = cx.theme().muted_foreground;
        let intro = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_sm().child(WARNING))
            .child(
                Button::new("plugins-open-folder")
                    .label("Open plugins folder")
                    .small()
                    .outline()
                    .w(px(160.))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::commands::OpenPluginsFolder), cx)
                    }),
            );
        let mut rows = vec![Row::new(
            Page::Plugins,
            "Plugins",
            "Trusted sources only",
            "plugins folder open install trust warning",
            intro.into_any_element(),
        )];
        let listed = self.listed_plugins(cx);
        if listed.is_empty() {
            let none = div()
                .text_sm()
                .text_color(muted)
                .child("No plugins yet: put a plugin's folder in the plugins folder.");
            rows.push(Row::new(
                Page::Plugins,
                "Found plugins",
                "None",
                "plugins",
                none.into_any_element(),
            ));
        }
        for (ix, plugin) in listed.into_iter().enumerate() {
            let keywords = format!("plugin plugins {} {}", plugin.id, plugin.run.join(" "));
            let label: SharedString = plugin.name.clone().into();
            let card = self.plugin_card(ix, plugin, cx);
            rows.push(Row {
                page: Page::Plugins,
                group: "Found plugins",
                label,
                keywords,
                control: card,
            });
        }
        rows
    }

    /// One plugin: where it is, what it runs, and everything the user
    /// decides about it.
    fn plugin_card(&self, ix: usize, plugin: ListedPlugin, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let small = |text: String| div().text_xs().text_color(muted).child(text);
        let mono = |text: String| div().text_xs().font_family(MONOSPACE).child(text);
        let mut card = div().flex().flex_col().gap(px(6.)).py(px(4.)).flex_1();
        let state = match &plugin.state {
            State::Unusable(why) => {
                return card
                    .child(div().text_sm().child(format!("Unusable: {why}")))
                    .child(small(plugin.folder.display().to_string()))
                    .into_any_element();
            }
            State::Disabled => "Disabled",
            State::Idle => "Enabled, not running",
            State::Running => "Running",
            State::Stopped => "Stopped: it crashed or couldn't start; see its log",
        };
        let entry = self.plugin_entry(&plugin.id);
        let enabled = entry.enabled && !plugin.run_changed;
        let version = match plugin.version.is_empty() {
            true => String::new(),
            false => format!(" · version {}", plugin.version),
        };
        let toggle = {
            let listed = plugin.clone();
            Switch::new(("plugin-enabled", ix))
                .checked(enabled)
                .on_click(cx.listener(move |this, on: &bool, window, cx| {
                    if *on {
                        this.confirm_enable(&listed, window, cx);
                    } else {
                        let mut entry = this.plugin_entry(&listed.id);
                        entry.enabled = false;
                        this.write_plugin(&listed.id, entry, cx);
                    }
                }))
        };
        let restart = {
            let id = plugin.id.clone();
            Button::new(("plugin-restart", ix))
                .label("Restart")
                .xsmall()
                .ghost()
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(host) = &this.host {
                        host.update(cx, |host, cx| host.restart_plugin(&id, cx));
                    }
                }))
        };
        let log_open = self.plugins.logs.contains(&plugin.id);
        let show_log = {
            let id = plugin.id.clone();
            Button::new(("plugin-log", ix))
                .label(if log_open { "Hide log" } else { "Show log" })
                .xsmall()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.plugins.logs.remove(&id) {
                        this.plugins.logs.insert(id.clone());
                    }
                    cx.notify();
                }))
        };
        card = card
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(toggle)
                    .child(div().text_sm().child(format!("{state}{version}")))
                    .child(restart)
                    .child(show_log),
            )
            .child(mono(plugin.run.join(" ")))
            .child(small(format!(
                "Runs as you, from {}",
                plugin.folder.display()
            )));
        if plugin.run_changed {
            let listed = plugin.clone();
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().text_sm().child(
                        "Its command line changed since you enabled it, so it doesn't start \
                         until you confirm the new one.",
                    ))
                    .child(
                        Button::new(("plugin-reconfirm", ix))
                            .label("Confirm…")
                            .xsmall()
                            .outline()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_enable(&listed, window, cx);
                            })),
                    ),
            );
        }
        card = card
            .children(self.plugin_rights(ix, &plugin, &entry, cx))
            .children(self.plugin_events(ix, &plugin, &entry, cx))
            .children(self.plugin_settings(ix, &plugin, &entry, cx));
        if log_open {
            card = card.child(self.plugin_log(ix, &plugin.id, cx));
        }
        card.into_any_element()
    }

    /// A checkbox per right the manifest declares (flow 2).
    fn plugin_rights(
        &self,
        ix: usize,
        plugin: &ListedPlugin,
        entry: &PluginSettings,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if plugin.rights.is_empty() {
            return None;
        }
        let boxes = plugin.rights.iter().enumerate().map(|(n, &right)| {
            let id = plugin.id.clone();
            Checkbox::new(("plugin-right", ix * 16 + n))
                .label(right.name())
                .checked(entry.rights.contains(&right))
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let mut entry = this.plugin_entry(&id);
                    entry.rights.retain(|held| *held != right);
                    if *on {
                        entry.rights.push(right);
                    }
                    this.write_plugin(&id, entry, cx);
                }))
        });
        Some(labelled(
            "Rights",
            div().flex().flex_wrap().gap(px(10.)).children(boxes),
        ))
    }

    /// A checkbox per event the manifest declares (flow 2).
    fn plugin_events(
        &self,
        ix: usize,
        plugin: &ListedPlugin,
        entry: &PluginSettings,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if plugin.events.is_empty() {
            return None;
        }
        let boxes = plugin.events.iter().enumerate().map(|(n, &event)| {
            let id = plugin.id.clone();
            Checkbox::new(("plugin-event", ix * 16 + n))
                .label(event.name())
                .checked(entry.events.contains(&event))
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let mut entry = this.plugin_entry(&id);
                    entry.events.retain(|heard| *heard != event);
                    if *on {
                        entry.events.push(event);
                    }
                    this.write_plugin(&id, entry, cx);
                }))
        });
        Some(labelled(
            "Events",
            div().flex().flex_wrap().gap(px(10.)).children(boxes),
        ))
    }

    /// The manifest's settings and those of `set_settings`, under the
    /// plain-text note (flow 8, TM-plug-18).
    fn plugin_settings(
        &self,
        ix: usize,
        plugin: &ListedPlugin,
        entry: &PluginSettings,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if plugin.fields.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let mut fields = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().text_xs().text_color(muted).child(PLAIN_TEXT));
        for (n, field) in plugin.fields.iter().enumerate() {
            let key = field.key.clone();
            let id = plugin.id.clone();
            let element_id = ix * 256 + n;
            let control: AnyElement = match &field.kind {
                FieldKind::Bool => {
                    let on = matches!(entry.settings.get(&key), Some(Setting::Bool(true)));
                    Switch::new(("plugin-setting-bool", element_id))
                        .checked(on)
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            this.set_plugin_setting(&id, &key, Setting::Bool(*on), cx);
                        }))
                        .into_any_element()
                }
                FieldKind::Choice(choices) => {
                    let current = match entry.settings.get(&key) {
                        Some(Setting::Text(text)) => text.clone(),
                        _ => String::new(),
                    };
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(choices.iter().enumerate().map(|(c, choice)| {
                            let (id, key, choice) = (id.clone(), key.clone(), choice.clone());
                            Button::new(("plugin-setting-choice", element_id * 64 + c))
                                .label(choice.clone())
                                .xsmall()
                                .ghost()
                                .selected(current == choice)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let value = Setting::Text(choice.clone());
                                    this.set_plugin_setting(&id, &key, value, cx);
                                }))
                        }))
                        .into_any_element()
                }
                FieldKind::Text | FieldKind::Number => {
                    let input = self
                        .plugins
                        .inputs
                        .iter()
                        .find(|(p, k, _)| *p == plugin.id && *k == field.key)
                        .map(|(_, _, input)| input.clone());
                    div()
                        .w(px(220.))
                        .children(input.map(|input| Input::new(&input).small()))
                        .into_any_element()
                }
            };
            fields = fields.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(140.))
                            .text_xs()
                            .truncate()
                            .child(field.label.clone()),
                    )
                    .child(control),
            );
        }
        Some(labelled("Settings", fields))
    }

    /// The plugin's last log lines: its stderr and what Tern dropped or
    /// refused (flow 6).
    fn plugin_log(&self, ix: usize, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let lines: Vec<String> = self
            .host
            .as_ref()
            .map(|host| {
                let log: Vec<&str> = host.read(cx).log_of(id).collect();
                let skip = log.len().saturating_sub(LOG_SHOWN);
                log[skip..].iter().map(|line| (*line).to_owned()).collect()
            })
            .unwrap_or_default();
        let theme = cx.theme();
        let empty = lines.is_empty();
        div()
            .id(("plugin-log-lines", ix))
            .max_h(px(200.))
            .overflow_y_scroll()
            .p(px(6.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .font_family(MONOSPACE)
            .when(empty, |log| log.child("Nothing logged yet."))
            .children(lines.into_iter().map(|line| div().child(line)))
            .into_any_element()
    }
}

/// `content` under a small heading inside a plugin's card.
fn labelled(title: &'static str, content: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(content)
}

/// A setting as its field shows it.
fn setting_text(setting: &Setting) -> String {
    match setting {
        Setting::Bool(on) => on.to_string(),
        Setting::Number(value) => number(*value),
        Setting::Text(text) => text.clone(),
    }
}

/// The Plugins tab against a real plugin host (spec 0016, Model, flow 2,
/// AC1; spec 0009, Plugins tab). The element tree has no text to query,
/// so what a row shows is checked through what the search finds, and the
/// enable switch, which has no selector, is stood in for by the
/// confirmation it opens.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::plugins::Host;
    use crate::plugins::tests::{Lab, pids, run, settle, wait_until};
    use crate::settings::matches;
    use crate::settings::tests::{Screen, open_screen_on};
    use gpui::TestAppContext;
    use std::collections::BTreeMap;

    /// A host that found `lab`'s plugins, none of them in the config.
    fn found(lab: &Lab, cx: &mut TestAppContext) -> Entity<Host> {
        lab.ends_with(cx);
        let host = cx.new(|cx| Host::new(Some(lab.root()), BTreeMap::new(), cx));
        host.update(cx, |host, cx| host.discover(cx));
        wait_until(cx, "the plugins to be found", |cx| {
            host.read_with(cx, |host, _| !host.plugins().is_empty())
        });
        host
    }

    /// The screen's Plugins rows: (label, what a search looks through).
    fn rows(screen: &mut Screen) -> Vec<(String, String)> {
        screen.view.update(screen.cx, |view, cx| {
            let rows = view.plugin_rows(cx);
            rows.into_iter()
                .map(|row| {
                    assert_eq!(row.page, Page::Plugins);
                    let haystack = format!(
                        "{} {} {} {}",
                        row.label,
                        row.group,
                        row.page.title(),
                        row.keywords
                    );
                    (row.label.to_string(), haystack)
                })
                .collect()
        })
    }

    /// Opens the enable confirmation for `id`, as the switch does.
    fn ask_to_enable(screen: &mut Screen, id: &str) {
        screen.view.update_in(screen.cx, |view, window, cx| {
            let listed = view.listed_plugins(cx);
            let plugin = listed
                .iter()
                .find(|plugin| plugin.id == id)
                .expect("the plugin is listed");
            view.confirm_enable(plugin, window, cx);
        });
        draw(screen);
    }

    fn draw(screen: &mut Screen) {
        screen.cx.update(|window, cx| window.draw(cx).clear(cx));
        screen.cx.run_until_parked();
    }

    /// Clicks Enable, the last answer: the bottom-right button inside the
    /// dialog's 16 px padding. The test window never asks for the frame
    /// that gives the safe answer the keyboard, so Right can't reach it.
    fn click_enable(screen: &mut Screen) {
        let dialog = screen.cx.debug_bounds("dialog-0").expect("a dialog");
        let corner = dialog.bottom_right() - gpui::point(px(16. + 6.), px(16. + 6.));
        screen.cx.simulate_click(corner, gpui::Modifiers::none());
        draw(screen);
    }

    fn dialog_open(screen: &mut Screen) -> bool {
        screen.cx.update(|window, cx| window.has_active_dialog(cx))
    }

    /// Spec 0016, Trusted sources only and Open plugins folder: the tab
    /// opens with the warning row, which a search for the plugins folder
    /// finds. A found, disabled plugin is listed under its name, and a
    /// search for its command line finds it. Listing it starts nothing.
    #[gpui::test]
    fn the_tab_warns_first_and_lists_a_found_plugin_by_its_command_line(cx: &mut TestAppContext) {
        let lab = Lab::new("settings-tab-rows");
        let docker = lab.plugin("docker", "stay", "events: [start]\n");
        let host = found(&lab, cx);
        let mut screen = open_screen_on(Config::default(), Some(host), cx);

        let rows = rows(&mut screen);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].0, "Trusted sources only");
        for query in ["open plugins folder", "trust"] {
            assert!(matches(query, &rows[0].1), "{query}: {}", rows[0].1);
        }
        assert_eq!(rows[1].0, "Name of docker");
        let command = run("stay").join(" ");
        assert!(rows[1].1.contains(&command), "{}", rows[1].1);
        assert!(matches("stay", &rows[1].1), "{}", rows[1].1);

        settle(screen.cx);
        assert_eq!(pids(&docker), [""; 0], "the tab started a plugin");
        assert!(screen.changes.borrow().is_empty());
    }

    /// With no plugin found yet the tab still has the warning first.
    #[gpui::test]
    fn the_tab_warns_with_no_plugins_found(cx: &mut TestAppContext) {
        let mut screen = open_screen_on(Config::default(), None, cx);
        let rows = rows(&mut screen);
        assert_eq!(rows[0].0, "Trusted sources only");
        assert!(rows.len() == 2 && rows[1].0 == "None", "{rows:?}");
    }

    /// AC1, flow 2: enabling asks first and writes nothing until it is
    /// answered; Cancel, which Enter means, writes nothing and starts
    /// nothing.
    #[gpui::test]
    fn cancelling_the_enable_confirmation_writes_nothing(cx: &mut TestAppContext) {
        let lab = Lab::new("settings-tab-cancel");
        let docker = lab.plugin("docker", "stay", "events: [start]\n");
        let host = found(&lab, cx);
        let mut screen = open_screen_on(Config::default(), Some(host), cx);

        ask_to_enable(&mut screen, "docker");
        assert!(dialog_open(&mut screen), "enabling didn't ask");
        assert!(screen.changes.borrow().is_empty(), "written before asked");

        screen.cx.simulate_keystrokes("enter");
        draw(&mut screen);
        assert!(!dialog_open(&mut screen), "Enter left the question open");
        assert!(screen.changes.borrow().is_empty(), "Cancel wrote");

        ask_to_enable(&mut screen, "docker");
        screen.cx.simulate_keystrokes("escape");
        draw(&mut screen);
        assert!(!dialog_open(&mut screen), "Esc left the question open");
        assert!(screen.changes.borrow().is_empty(), "Esc wrote");
        settle(screen.cx);
        assert_eq!(pids(&docker), [""; 0]);
    }

    /// Flow 2, §Model: Enable writes the plugin's whole entry, enabled and
    /// with the manifest's command line, keeping what the file already
    /// ticked and set for it.
    #[gpui::test]
    fn enable_writes_enabled_and_the_manifests_command_line(cx: &mut TestAppContext) {
        use crate::config::PluginSettings;
        use crate::plugins::{Right, Subscription};
        let lab = Lab::new("settings-tab-enable");
        lab.plugin(
            "docker",
            "stay",
            "rights: [pane_info]\nevents: [pane_opened]\n",
        );
        let host = found(&lab, cx);
        let before = PluginSettings {
            enabled: false,
            run: vec!["./old-command".into()],
            rights: vec![Right::PaneInfo],
            events: vec![Subscription::PaneOpened],
            settings: BTreeMap::from([("socket".to_owned(), Setting::Text("/run/x".into()))]),
        };
        let mut config = Config::default();
        config.plugins.insert("docker".into(), before.clone());
        let mut screen = open_screen_on(config, Some(host), cx);

        ask_to_enable(&mut screen, "docker");
        click_enable(&mut screen);
        assert!(!dialog_open(&mut screen));
        let changes = screen.changes.borrow();
        assert_eq!(changes.len(), 1, "{changes:?}");
        let Change::Plugin { id, entry } = &changes[0] else {
            panic!("not a plugin's entry: {:?}", changes[0]);
        };
        assert_eq!(id, "docker");
        let expected = PluginSettings {
            enabled: true,
            run: run("stay"),
            ..before
        };
        assert_eq!(*entry, expected);
    }
}
