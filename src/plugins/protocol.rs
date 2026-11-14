//! Version 1 of the protocol (spec 0016, Model): events go to a plugin as
//! one JSON object per line, actions come back the same way. Everything a
//! plugin sends is untrusted input (threat model 0005, TB1).

use super::manifest::{API, Field, Right};
use super::panel::{self, Item};
use crate::config::Setting;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most of a log line that is kept, so 10,000 of them stay small.
const MAX_LOG_CHARS: usize = 1024;

/// What Tern tells a plugin.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Start,
    Command { command: String },
    PanelOpened { panel: String },
    PanelClosed { panel: String },
    PanelAction { panel: String, id: String },
    PanelInput { panel: String, text: String },
    PaneOpened { pane: Pane },
    PaneClosed { pane: Pane },
    SettingsChanged,
    ActionResult { id: Option<Id>, result: Outcome },
    Output { id: Option<Id>, text: String },
}

/// A pane as a plugin sees it: an opaque ID, and its details only with
/// the `pane_info` right (TM-plug-16).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Pane {
    pub id: u64,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub info: Option<PaneInfo>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PaneInfo {
    pub title: String,
    pub cwd: Option<String>,
    pub profile: String,
    /// `user@host` of an SSH pane.
    pub host: Option<String>,
}

/// What became of an action that asked for something.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Allowed,
    Denied,
    Refused,
    NoPane,
}

/// An action's `id`, sent back with its result as it came.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Id {
    Number(u64),
    Text(String),
}

#[derive(Serialize)]
struct Envelope<'a> {
    api: u32,
    #[serde(flatten)]
    event: &'a Event,
    settings: &'a BTreeMap<String, Setting>,
}

impl Event {
    /// The event's line, without the line break, carrying the plugin's
    /// current `settings`.
    pub fn line(&self, settings: &BTreeMap<String, Setting>) -> String {
        let envelope = Envelope {
            api: API,
            event: self,
            settings,
        };
        // Strings, numbers and maps with string keys always serialize.
        serde_json::to_string(&envelope).unwrap_or_default()
    }
}

/// What a plugin asks Tern to do.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Ready,
    SetPanel {
        panel: String,
        #[serde(default)]
        items: Vec<Item>,
    },
    AppendLog {
        panel: String,
        text: String,
    },
    OpenPanel {
        panel: String,
    },
    Notify {
        text: String,
    },
    SetSettings {
        #[serde(default, deserialize_with = "super::manifest::dynamic_fields")]
        fields: Vec<Field>,
    },
    FillCommand {
        #[serde(default)]
        id: Option<Id>,
        #[serde(default)]
        pane: Option<u64>,
        text: String,
    },
    RunCommand {
        #[serde(default)]
        id: Option<Id>,
        #[serde(default)]
        pane: Option<u64>,
        text: String,
    },
    ReadOutput {
        #[serde(default)]
        id: Option<Id>,
        #[serde(default)]
        pane: Option<u64>,
        #[serde(default)]
        lines: Option<u32>,
    },
    OpenTab {
        #[serde(default)]
        id: Option<Id>,
        argv: Vec<String>,
    },
}

impl Action {
    /// The action in `line`, or why the line is dropped. The reason never
    /// quotes the line (TM-plug-17).
    pub fn parse(line: &str) -> Result<Self, String> {
        let action = serde_json::from_str(line).map_err(|error| {
            use serde_json::error::Category;
            match error.classify() {
                Category::Data => "an unknown action, or one with a field missing",
                _ => "not a JSON object",
            }
            .to_owned()
        })?;
        match action {
            Action::SetPanel { panel, items } => Ok(Action::SetPanel {
                panel,
                items: panel::checked(items)?,
            }),
            action => Ok(action),
        }
    }

    /// Its name on the wire, for the plugin's log.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Ready => "ready",
            Action::SetPanel { .. } => "set_panel",
            Action::AppendLog { .. } => "append_log",
            Action::OpenPanel { .. } => "open_panel",
            Action::Notify { .. } => "notify",
            Action::SetSettings { .. } => "set_settings",
            Action::FillCommand { .. } => "fill_command",
            Action::RunCommand { .. } => "run_command",
            Action::ReadOutput { .. } => "read_output",
            Action::OpenTab { .. } => "open_tab",
        }
    }

    /// The right it needs.
    pub fn right(&self) -> Option<Right> {
        match self {
            Action::FillCommand { .. } | Action::RunCommand { .. } => Some(Right::TerminalWrite),
            Action::ReadOutput { .. } => Some(Right::TerminalRead),
            Action::OpenTab { .. } => Some(Right::OpenTab),
            _ => None,
        }
    }

    /// The panel it fills or shows, which the manifest must declare.
    pub fn panel(&self) -> Option<&str> {
        match self {
            Action::SetPanel { panel, .. }
            | Action::AppendLog { panel, .. }
            | Action::OpenPanel { panel } => Some(panel),
            _ => None,
        }
    }

    /// The `id` its result is sent back with; only the actions that get a
    /// result have one.
    pub fn id(&self) -> Option<&Id> {
        match self {
            Action::FillCommand { id, .. }
            | Action::RunCommand { id, .. }
            | Action::ReadOutput { id, .. }
            | Action::OpenTab { id, .. } => id.as_ref(),
            _ => None,
        }
    }
}

/// A line of a plugin's log as it's kept: plain text without control or
/// invisible characters, cut at 1,024 characters.
pub fn log_line(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control() && !crate::terminal::is_invisible_format(*c))
        .take(MAX_LOG_CHARS)
        .collect()
}

/// Spec 0016 §Model (Events, Actions, Untrusted text) and flow 4: the
/// lines Tern writes, and what it makes of the lines a plugin writes.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn settings() -> BTreeMap<String, Setting> {
        BTreeMap::from([
            (
                "socket".into(),
                Setting::Text("/var/run/docker.sock".into()),
            ),
            ("tls".into(), Setting::Bool(true)),
            ("retries".into(), Setting::Number(3.0)),
        ])
    }

    fn pane(info: Option<PaneInfo>) -> Pane {
        Pane { id: 7, info }
    }

    fn details() -> PaneInfo {
        PaneInfo {
            title: "vim secret.txt".into(),
            cwd: Some("/home/me/projects".into()),
            profile: "work".into(),
            host: Some("me@bastion".into()),
        }
    }

    /// One of each event, with its name on the wire.
    fn every_event() -> Vec<(Event, &'static str)> {
        let panel = || "containers".to_owned();
        vec![
            (Event::Start, "start"),
            (
                Event::Command {
                    command: "containers".into(),
                },
                "command",
            ),
            (Event::PanelOpened { panel: panel() }, "panel_opened"),
            (Event::PanelClosed { panel: panel() }, "panel_closed"),
            (
                Event::PanelAction {
                    panel: panel(),
                    id: "restart".into(),
                },
                "panel_action",
            ),
            (
                Event::PanelInput {
                    panel: panel(),
                    text: "two\nlines \"quoted\"".into(),
                },
                "panel_input",
            ),
            (
                Event::PaneOpened {
                    pane: pane(Some(details())),
                },
                "pane_opened",
            ),
            (Event::PaneClosed { pane: pane(None) }, "pane_closed"),
            (Event::SettingsChanged, "settings_changed"),
            (
                Event::ActionResult {
                    id: Some(Id::Number(4)),
                    result: Outcome::Allowed,
                },
                "action_result",
            ),
            (
                Event::Output {
                    id: Some(Id::Number(5)),
                    text: "line one\r\nline two\n".into(),
                },
                "output",
            ),
        ]
    }

    fn object(line: &str) -> Value {
        let value: Value = serde_json::from_str(line).expect("an event is JSON");
        assert!(value.is_object(), "{line} isn't an object");
        value
    }

    /// §Model: each event is `{"api":1,"event":…,"settings":{…},…}`, "one
    /// JSON object per line", whatever text it carries.
    #[test]
    fn every_event_is_one_json_object_on_one_line_with_api_event_and_settings() {
        for (event, name) in every_event() {
            let line = event.line(&settings());
            assert!(!line.contains(['\n', '\r']), "{name} spans lines: {line:?}");
            let value = object(&line);
            assert_eq!(value["api"], 1, "{name}");
            assert_eq!(value["event"], name);
            assert_eq!(
                value["settings"],
                json!({ "socket": "/var/run/docker.sock", "tls": true, "retries": 3.0 }),
                "{name}"
            );
        }
    }

    /// A plugin without settings still gets the `settings` object.
    #[test]
    fn an_event_without_settings_carries_an_empty_settings_object() {
        let value = object(&Event::Start.line(&BTreeMap::new()));
        assert_eq!(value["settings"], json!({}));
    }

    /// Text reaches the plugin as it was, escaped by JSON.
    #[test]
    fn an_events_text_survives_the_line() {
        let text = "two\nlines \"quoted\" \\ \u{1b}[31m é";
        let input = Event::PanelInput {
            panel: "p".into(),
            text: text.into(),
        };
        assert_eq!(object(&input.line(&settings()))["text"], text);
        let output = Event::Output {
            id: None,
            text: text.into(),
        };
        assert_eq!(object(&output.line(&settings()))["text"], text);
    }

    /// Flow 5: the plugin gets `action_result` (`allowed`, `denied`,
    /// `refused`, `no_pane`) with the action's `id`.
    #[test]
    fn an_action_result_names_its_outcome_and_the_actions_id() {
        let outcomes = [
            (Outcome::Allowed, "allowed"),
            (Outcome::Denied, "denied"),
            (Outcome::Refused, "refused"),
            (Outcome::NoPane, "no_pane"),
        ];
        for (result, name) in outcomes {
            let event = Event::ActionResult {
                id: Some(Id::Number(42)),
                result,
            };
            let value = object(&event.line(&settings()));
            assert_eq!(value["event"], "action_result");
            assert_eq!(value["result"], name);
            assert_eq!(value["id"], 42);
        }
    }

    /// The `id` a plugin gave, a number or text, comes back as it came.
    #[test]
    fn an_actions_id_comes_back_as_it_came() {
        for id in [json!(7), json!("req-7"), json!("7")] {
            let line = json!({ "action": "run_command", "id": id, "text": "ls" }).to_string();
            let action = Action::parse(&line).expect("a run_command");
            let result = Event::ActionResult {
                id: action.id().cloned(),
                result: Outcome::Refused,
            };
            assert_eq!(object(&result.line(&settings()))["id"], id);
        }
    }

    /// §Model, TM-plug-16: "Panes are opaque IDs unless `pane_info` is
    /// granted, which adds title, cwd, profile and host."
    #[test]
    fn a_pane_carries_its_details_only_when_they_are_given() {
        let opaque = Event::PaneOpened { pane: pane(None) }.line(&settings());
        let value = object(&opaque);
        assert_eq!(value["pane"], json!({ "id": 7 }), "only the ID");

        let detailed = Event::PaneOpened {
            pane: pane(Some(details())),
        };
        let value = object(&detailed.line(&settings()));
        assert_eq!(
            value["pane"],
            json!({
                "id": 7,
                "title": "vim secret.txt",
                "cwd": "/home/me/projects",
                "profile": "work",
                "host": "me@bastion",
            })
        );
    }

    /// The actions table of §Model: each action by its name, and the right
    /// it needs.
    #[test]
    fn every_action_of_the_spec_parses_and_needs_the_right_the_spec_says() {
        let actions = [
            (json!({ "action": "ready" }), None),
            (
                json!({ "action": "set_panel", "panel": "containers", "items": [{ "type": "text", "text": "web" }] }),
                None,
            ),
            (
                json!({ "action": "append_log", "panel": "containers", "text": "started" }),
                None,
            ),
            (
                json!({ "action": "open_panel", "panel": "containers" }),
                None,
            ),
            (json!({ "action": "notify", "text": "done" }), None),
            (json!({ "action": "set_settings", "fields": [] }), None),
            (
                json!({ "action": "fill_command", "id": 1, "pane": 7, "text": "docker ps" }),
                Some(Right::TerminalWrite),
            ),
            (
                json!({ "action": "run_command", "id": 2, "text": "docker ps" }),
                Some(Right::TerminalWrite),
            ),
            (
                json!({ "action": "read_output", "id": 3, "lines": 100 }),
                Some(Right::TerminalRead),
            ),
            (
                json!({ "action": "open_tab", "id": 4, "argv": ["docker", "exec", "-it", "web", "sh"] }),
                Some(Right::OpenTab),
            ),
        ];
        for (line, right) in actions {
            let action = Action::parse(&line.to_string())
                .unwrap_or_else(|reason| panic!("{line} was dropped: {reason}"));
            assert_eq!(action.name(), line["action"], "{line}");
            assert_eq!(action.right(), right, "{line}");
        }
    }

    #[test]
    fn a_parsed_action_holds_what_the_plugin_sent() {
        assert_eq!(Action::parse(r#"{"action":"ready"}"#), Ok(Action::Ready));
        assert_eq!(
            Action::parse(r#"{"action":"run_command","id":"a","pane":7,"text":"ls -la"}"#),
            Ok(Action::RunCommand {
                id: Some(Id::Text("a".into())),
                pane: Some(7),
                text: "ls -la".into(),
            })
        );
        assert_eq!(
            Action::parse(r#"{"action":"fill_command","text":"ls"}"#),
            Ok(Action::FillCommand {
                id: None,
                pane: None,
                text: "ls".into(),
            })
        );
        assert_eq!(
            Action::parse(r#"{"action":"open_tab","id":4,"argv":["docker","ps"]}"#),
            Ok(Action::OpenTab {
                id: Some(Id::Number(4)),
                argv: vec!["docker".into(), "ps".into()],
            })
        );
        assert_eq!(
            Action::parse(r#"{"action":"append_log","panel":"logs","text":"a\nb"}"#),
            Ok(Action::AppendLog {
                panel: "logs".into(),
                text: "a\nb".into(),
            })
        );
        assert_eq!(
            Action::parse(r#"{"action":"read_output","id":3,"pane":7,"lines":100}"#),
            Ok(Action::ReadOutput {
                id: Some(Id::Number(3)),
                pane: Some(7),
                lines: Some(100),
            })
        );
    }

    /// §Model: actions are "each `{"action":…,"id":…}`", so an `id` is
    /// fine on every one of them, `ready` included.
    #[test]
    fn any_action_may_carry_an_id() {
        let lines = [
            r#"{"action":"ready","id":1}"#,
            r#"{"id":"x","action":"ready"}"#,
            r#"{"action":"notify","id":2,"text":"done"}"#,
            r#"{"action":"open_panel","id":3,"panel":"containers"}"#,
            r#"{"action":"set_panel","id":4,"panel":"containers","items":[]}"#,
            r#"{"action":"append_log","id":5,"panel":"containers","text":"x"}"#,
            r#"{"action":"set_settings","id":6,"fields":[]}"#,
        ];
        for line in lines {
            assert!(Action::parse(line).is_ok(), "{line} was dropped");
        }
    }

    /// The panel an action fills or shows is known, so flow 4 can check it
    /// against the manifest.
    #[test]
    fn panel_actions_name_their_panel() {
        for name in ["set_panel", "append_log", "open_panel"] {
            let line = json!({ "action": name, "panel": "containers", "text": "x" }).to_string();
            let action = Action::parse(&line).expect(name);
            assert_eq!(action.panel(), Some("containers"), "{name}");
        }
        let notify = Action::parse(r#"{"action":"notify","text":"x"}"#).expect("notify");
        assert_eq!(notify.panel(), None);
    }

    /// Flow 4: an "unknown action" and a "malformed line" are dropped.
    #[test]
    fn an_unknown_action_or_a_malformed_line_is_dropped_with_a_reason() {
        let lines = [
            "",
            "   ",
            "hello",
            "ready",
            r#""ready""#,
            "42",
            "null",
            r#"[{"action":"ready"}]"#,
            r#"{"action":"ready""#,
            r#"{"action":"fly"}"#,
            r#"{"action":"Ready"}"#,
            r#"{"action":7}"#,
            r#"{"id":1}"#,
            r#"{"event":"start"}"#,
            // A field the action can't do without, or of the wrong type.
            r#"{"action":"run_command","id":1}"#,
            r#"{"action":"run_command","text":["ls"]}"#,
            r#"{"action":"fill_command","text":"ls","pane":"left"}"#,
            r#"{"action":"open_tab","argv":"docker ps"}"#,
            r#"{"action":"open_panel"}"#,
            r#"{"action":"append_log","panel":"logs"}"#,
            r#"{"action":"read_output","lines":-1}"#,
            "\u{feff}\u{0}\u{1b}[2J",
        ];
        for line in lines {
            let reason = Action::parse(line).expect_err(line);
            assert!(!reason.is_empty(), "no reason for {line:?}");
        }
    }

    /// TM-plug-17: "Tern logs no event or action payload", so the reason
    /// a line is dropped for never quotes the line.
    #[test]
    fn the_reason_a_line_is_dropped_for_never_quotes_it() {
        let lines = [
            "hunter2-secret-token",
            r#"{"action":"run_command","text":["hunter2-secret-token"]}"#,
            r#"{"action":"hunter2-secret-token"}"#,
            r#"{"action":"notify","hunter2-secret-token":1}"#,
        ];
        for line in lines {
            let reason = Action::parse(line).expect_err(line);
            assert!(!reason.contains("hunter2"), "{reason}");
        }
    }

    /// §Untrusted text: a plugin's stderr is kept "as plain text with
    /// control characters removed", and no invisible format character
    /// either.
    #[test]
    fn a_log_line_is_plain_text() {
        assert_eq!(log_line("container web started"), "container web started");
        assert_eq!(log_line("größe: 3 MiB ✓"), "größe: 3 MiB ✓");
        let hostile =
            "ok\u{1b}[2J\u{7}\u{0} \u{202E}evil\u{202C}\u{200B}\u{2066}x\u{2069}\u{FEFF}\r";
        let shown = log_line(hostile);
        let hidden = |c: char| {
            c.is_control()
                || matches!(
                    c,
                    '\u{061C}'
                        | '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{FEFF}'
                )
        };
        assert!(!shown.chars().any(hidden), "{shown:?}");
        assert!(
            shown.starts_with("ok") && shown.contains("evil"),
            "{shown:?}"
        );
    }

    /// Bounds: the log keeps 10,000 lines in memory, so one endless line
    /// can't be kept whole.
    #[test]
    fn an_endless_log_line_is_cut() {
        let line = "é".repeat(1024 * 1024);
        assert!(log_line(&line).len() <= 64 * 1024);
    }

    /// A `set_panel` line for the panel `containers` with `items`.
    fn set_panel(items: Value) -> String {
        json!({ "action": "set_panel", "panel": "containers", "items": items }).to_string()
    }

    /// The items Tern took from a `set_panel` with `items`.
    fn items_of(items: Value) -> Vec<Item> {
        match Action::parse(&set_panel(items.clone())) {
            Ok(Action::SetPanel { panel, items }) => {
                assert_eq!(panel, "containers");
                items
            }
            other => panic!("{items} gave {other:?}"),
        }
    }

    /// A `set_panel` whose one item is a button carrying `command`.
    fn command_button(command: Value) -> String {
        set_panel(json!([{ "type": "button", "id": "go", "label": "Go", "command": command }]))
    }

    /// Actions table: `set_panel` "replaces a panel's items: text, rows
    /// with buttons, buttons, command buttons, one input field, a log".
    /// Each arrives typed, with what the plugin sent.
    #[test]
    fn a_set_panel_holds_every_kind_of_item() {
        use crate::plugins::command::Command;
        use crate::plugins::panel::RowButton;
        let items = items_of(json!([
            { "type": "text", "text": "3 running" },
            { "type": "row", "id": "web", "text": "web", "detail": "Up 2 h",
              "buttons": [{ "id": "web.restart", "label": "Restart" }] },
            { "type": "row", "text": "db" },
            { "type": "button", "id": "refresh", "label": "Refresh" },
            { "type": "button", "id": "ps", "label": "List",
              "command": { "action": "fill_command", "text": "docker ps" } },
            { "type": "button", "id": "prune", "label": "Prune",
              "command": { "action": "run_command", "text": "docker system prune" } },
            { "type": "button", "id": "sh", "label": "Shell",
              "command": { "action": "open_tab", "argv": ["docker", "exec", "-it", "web", "sh"] } },
            { "type": "input", "placeholder": "Filter" },
            { "type": "log" },
        ]));
        let button = |id: &str, label: &str, command: Option<Command>| Item::Button {
            id: id.into(),
            label: label.into(),
            command,
        };
        let argv = ["docker", "exec", "-it", "web", "sh"];
        let expected = [
            Item::Text {
                text: "3 running".into(),
            },
            Item::Row {
                id: Some("web".into()),
                text: "web".into(),
                detail: Some("Up 2 h".into()),
                buttons: vec![RowButton {
                    id: "web.restart".into(),
                    label: "Restart".into(),
                }],
            },
            Item::Row {
                id: None,
                text: "db".into(),
                detail: None,
                buttons: Vec::new(),
            },
            button("refresh", "Refresh", None),
            button(
                "ps",
                "List",
                Some(Command::Fill {
                    text: "docker ps".into(),
                }),
            ),
            button(
                "prune",
                "Prune",
                Some(Command::Run {
                    text: "docker system prune".into(),
                }),
            ),
            button(
                "sh",
                "Shell",
                Some(Command::OpenTab {
                    argv: argv.map(str::to_owned).to_vec(),
                }),
            ),
            Item::Input {
                placeholder: Some("Filter".into()),
            },
            Item::Log,
        ];
        assert_eq!(items, expected);
    }

    /// AC7, Untrusted text, TM-plug-04: a command with "a newline, ESC or
    /// a right-to-left override (U+202E)", a zero-width character or over
    /// 4 KiB "is refused", and "the same holds on a command button": the
    /// whole `set_panel` is dropped, so no part of it is drawn.
    #[test]
    fn a_set_panel_with_a_command_tern_wont_show_is_dropped_whole() {
        let texts = [
            "ls\nrm -rf ~".to_owned(),
            "ls \u{1b}[2K".to_owned(),
            "ls \u{202E}fdp.exe".to_owned(),
            "cu\u{200B}rl example.com".to_owned(),
            "x".repeat(4097),
        ];
        for text in &texts {
            let shown: String = text.escape_debug().take(24).collect();
            for action in ["fill_command", "run_command"] {
                let line = command_button(json!({ "action": action, "text": text }));
                assert!(Action::parse(&line).is_err(), "{action} with {shown}");
            }
            let line = command_button(json!({ "action": "open_tab", "argv": ["sh", "-c", text] }));
            assert!(Action::parse(&line).is_err(), "open_tab with {shown}");
        }
        // One bad button among good items drops them all.
        let line = set_panel(json!([
            { "type": "text", "text": "3 running" },
            { "type": "button", "id": "ok", "label": "List",
              "command": { "action": "fill_command", "text": "docker ps" } },
            { "type": "button", "id": "bad", "label": "Refresh",
              "command": { "action": "fill_command", "text": "ls\n" } },
        ]));
        assert!(Action::parse(&line).is_err());
    }

    /// TM-plug-05: an `open_tab` button with no program has nothing to
    /// show or run, so its `set_panel` is dropped.
    #[test]
    fn a_set_panel_with_an_open_tab_button_without_a_program_is_dropped() {
        for argv in [json!([]), json!([""]), json!(["", "-it"])] {
            let line = command_button(json!({ "action": "open_tab", "argv": argv }));
            assert!(Action::parse(&line).is_err(), "{argv}");
        }
    }

    /// Command buttons, TM-plug-23: a button "may carry one
    /// `fill_command`, `run_command` or `open_tab`"; "`read_output` is
    /// never on a button", nor is any other action, and a command it can't
    /// read never becomes a plain button under the same label.
    #[test]
    fn a_button_carries_no_other_action_than_the_three() {
        let commands = [
            json!({ "action": "read_output", "lines": 100 }),
            json!({ "action": "notify", "text": "hi" }),
            json!({ "action": "open_panel", "panel": "containers" }),
            json!({ "action": "launch_missiles" }),
            json!({ "action": "run_command" }),
            json!({ "text": "ls" }),
            json!("ls"),
        ];
        for command in commands {
            let line = command_button(command.clone());
            assert!(Action::parse(&line).is_err(), "{command}");
        }
    }

    /// AC7, Untrusted text (owner, OQ-035): `сurl example.com` with a
    /// Cyrillic `с` is not refused on a command button, and its text
    /// reaches the panel as it was sent, for the highlight to show.
    #[test]
    fn a_command_button_with_a_look_alike_letter_is_kept_as_sent() {
        use crate::plugins::command::Command;
        for action in ["fill_command", "run_command"] {
            let items = items_of(json!([
                { "type": "button", "id": "get", "label": "Fetch",
                  "command": { "action": action, "text": "сurl example.com" } },
            ]));
            let [Item::Button { command, .. }] = &items[..] else {
                panic!("{items:?}");
            };
            let text = match command {
                Some(Command::Fill { text } | Command::Run { text }) => text,
                other => panic!("{other:?}"),
            };
            assert_eq!(text, "сurl example.com", "{action}");
        }
    }

    /// Actions table, TM-plug-02: "Panel items are a fixed set Tern
    /// renders": an item of a kind Tern doesn't know, or one missing what
    /// its kind needs, drops the `set_panel`, as does a second input field
    /// or log ("one input field, a log").
    #[test]
    fn a_set_panel_with_an_item_tern_cannot_draw_is_dropped() {
        let item_lists = [
            json!([{ "type": "html", "html": "<b>hi</b>" }]),
            json!([{ "type": "image", "path": "/etc/passwd" }]),
            json!([{ "type": "Text", "text": "x" }]),
            json!([{ "text": "no type" }]),
            json!(["text"]),
            json!([{ "type": "text" }]),
            json!([{ "type": "row", "id": "web" }]),
            json!([{ "type": "button", "label": "no id" }]),
            json!([{ "type": "button", "id": "no-label" }]),
            json!([{ "type": "row", "text": "web", "buttons": [{ "label": "no id" }] }]),
            json!({ "type": "text", "text": "not a list" }),
            json!([{ "type": "input" }, { "type": "text", "text": "x" }, { "type": "input" }]),
            json!([{ "type": "log" }, { "type": "log" }]),
        ];
        for items in item_lists {
            let reason = Action::parse(&set_panel(items.clone())).expect_err("dropped");
            assert!(!reason.is_empty(), "no reason for {items}");
        }
        // One input field and one log are what a panel may have.
        assert_eq!(
            items_of(json!([{ "type": "input" }, { "type": "log" }])).len(),
            2
        );
    }

    /// TM-plug-17: the reason a `set_panel` is dropped for goes into the
    /// plugin's log, so it never quotes the command or the items.
    #[test]
    fn the_reason_a_set_panel_is_dropped_for_never_quotes_it() {
        let lines = [
            command_button(json!({ "action": "run_command", "text": "hunter2-secret\n" })),
            command_button(json!({ "action": "open_tab", "argv": ["", "hunter2-secret"] })),
            command_button(json!({ "action": "hunter2-secret" })),
            set_panel(json!([{ "type": "hunter2-secret" }])),
            set_panel(
                json!([{ "type": "input", "placeholder": "hunter2-secret" }, { "type": "input" }]),
            ),
        ];
        for line in lines {
            let reason = Action::parse(&line).expect_err("dropped");
            assert!(!reason.contains("hunter2"), "{reason}");
        }
    }

    /// AC7, Untrusted text: "panel text containing escape sequences
    /// renders as plain text": what reaches the panel holds no control
    /// character and no character that hides or reorders text, in a text,
    /// a row, a label or a placeholder.
    #[test]
    fn a_set_panels_texts_arrive_as_plain_text() {
        let hostile = "ok\u{1b}[2J\u{1b}]0;title\u{7}\u{0} \u{202E}evil\u{202C}\u{200B}\u{FEFF}\r";
        let items = items_of(json!([
            { "type": "text", "text": hostile },
            { "type": "row", "id": "r", "text": hostile, "detail": hostile,
              "buttons": [{ "id": "b", "label": hostile }] },
            { "type": "button", "id": "c", "label": hostile },
            { "type": "input", "placeholder": hostile },
        ]));
        let hidden = |c: char| {
            (c.is_control() && c != '\n')
                || matches!(
                    c,
                    '\u{200B}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
                )
        };
        // Every string of the items, through their debug form: none may
        // hold an escaped control or invisible character.
        let texts: Vec<String> = items
            .iter()
            .flat_map(|item| match item {
                Item::Text { text } => vec![text.clone()],
                Item::Row {
                    text,
                    detail,
                    buttons,
                    ..
                } => {
                    let labels = buttons.iter().map(|button| button.label.clone());
                    let mut texts = vec![text.clone(), detail.clone().unwrap_or_default()];
                    texts.extend(labels);
                    texts
                }
                Item::Button { label, .. } => vec![label.clone()],
                Item::Input { placeholder } => vec![placeholder.clone().unwrap_or_default()],
                Item::Log => Vec::new(),
            })
            .collect();
        assert_eq!(texts.len(), 6);
        for text in texts {
            assert!(!text.chars().any(hidden), "{text:?}");
            assert!(text.starts_with("ok") && text.contains("evil"), "{text:?}");
        }
    }
}
