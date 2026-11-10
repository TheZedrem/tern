//! The Keyboard tab: every command's keys, recorded in place.

use crate::commands::{self, AppCommand, keymap};
use crate::settings::change::Change;
use crate::settings::{Page, Row, SettingsView};
use gpui::prelude::FluentBuilder;
use gpui::{Context, InteractiveElement, IntoElement, Keystroke, ParentElement, Styled, div, px};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};

impl SettingsView {
    /// Takes the next key combination as `command`'s keys; Esc cancels
    /// (spec 0009, flow 3). It's caught before any binding runs it.
    fn record(&mut self, command: &'static str, cx: &mut Context<Self>) {
        self.recording = Some(command);
        let this = cx.entity().downgrade();
        self._recorder = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            if keystroke.is_modifier_key() {
                return;
            }
            cx.stop_propagation();
            let _ = this.update(cx, |this, cx| {
                let Some(command) = this.recording.take() else {
                    return;
                };
                this._recorder = None;
                let cancel = keystroke.key == "escape" && !keystroke.modifiers.modified();
                if !cancel && let Some(keys) = config_keys(&keystroke) {
                    this.set(
                        Change::Bind {
                            command,
                            keys: Some(keys),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(in crate::settings) fn keybinding_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let (planned, _) = keymap::plan_for(&self.config.keybindings, self.config.new_tab);
        let own: Vec<&str> = self
            .config
            .keybindings
            .iter()
            .map(|binding| binding.command.as_str())
            .collect();
        let rows = commands::COMMANDS
            .iter()
            .filter_map(|command: &'static AppCommand| Some((command, command.title?)))
            .map(|(command, title)| {
                let keys: Vec<String> = planned
                    .iter()
                    .filter(|(_, bound)| bound.id == command.id)
                    .map(|(keys, _)| shown_keys(keys))
                    .collect();
                let recording = self.recording == Some(command.id);
                let shown = if recording {
                    "Press keys… (Esc cancels)".to_owned()
                } else if keys.is_empty() {
                    "—".to_owned()
                } else {
                    keys.join(", ")
                };
                let id = command.id;
                let control = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(200.)).text_sm().child(shown))
                    .child(
                        Button::new(("record", id.len() * 131 + title.len()))
                            .label("Record")
                            .xsmall()
                            .ghost()
                            // Where a test clicks; a no-op outside test builds.
                            .debug_selector(move || format!("record-{id}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.record(id, cx))),
                    )
                    .when(own.contains(&id), |row| {
                        row.child(
                            Button::new(("reset", id.len() * 131 + title.len()))
                                .label("Reset")
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set(
                                        Change::Bind {
                                            command: id,
                                            keys: None,
                                        },
                                        cx,
                                    );
                                })),
                        )
                    });
                Row {
                    page: Page::Keyboard,
                    group: "Keybindings",
                    label: title.into(),
                    keywords: format!("{id} {} shortcut hotkey key", keys.join(" ")),
                    control: control.into_any_element(),
                }
            });
        rows.collect()
    }
}

/// A key combination as `keybindings:` writes it (spec 0004), like
/// `ctrl+shift+k`; `None` for a modifier alone.
pub fn config_keys(keystroke: &Keystroke) -> Option<String> {
    if keystroke.is_modifier_key() || keystroke.key.is_empty() {
        return None;
    }
    let modifiers = keystroke.modifiers;
    let platform = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "super"
    };
    let mut parts: Vec<&str> = [
        (modifiers.control, "ctrl"),
        (modifiers.alt, "alt"),
        (modifiers.shift, "shift"),
        (modifiers.platform, platform),
    ]
    .into_iter()
    .filter_map(|(held, name)| held.then_some(name))
    .collect();
    parts.push(&keystroke.key);
    Some(parts.join("+"))
}

/// GPUI's spelling of keys, like `ctrl-shift-d`, as config writes them.
fn shown_keys(keys: &str) -> String {
    Keystroke::parse_with_os(keys, keymap::OS)
        .ok()
        .and_then(|keystroke| config_keys(&keystroke))
        .unwrap_or_else(|| keys.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::open_screen;
    use gpui::TestAppContext;

    fn keystroke(text: &str) -> Keystroke {
        Keystroke::parse_with_os(text, keymap::OS).unwrap()
    }

    /// Spec 0009, flow 3 and AC 3: a recorded combination is written in
    /// config's spelling.
    #[test]
    fn a_recorded_combination_is_written_in_config_spelling() {
        assert_eq!(
            config_keys(&keystroke("ctrl-shift-k")).as_deref(),
            Some("ctrl+shift+k")
        );
        assert_eq!(
            config_keys(&keystroke("alt-shift-d")).as_deref(),
            Some("alt+shift+d")
        );
    }

    /// Spec 0009, flow 3: a modifier held alone isn't a combination to
    /// record yet.
    #[test]
    fn a_lone_modifier_is_no_combination() {
        for modifier in ["shift", "control", "alt"] {
            assert_eq!(config_keys(&keystroke(modifier)), None, "{modifier}");
        }
    }

    /// Spec 0009, flow 3 and criterion 8 (T-058): while "Record" waits for
    /// keys, the first Esc cancels the recording and leaves the screen
    /// open; the next Esc leaves the screen.
    #[gpui::test]
    fn esc_cancels_a_recording_first_then_closes_the_screen(cx: &mut TestAppContext) {
        let mut screen = open_screen(cx);
        screen.click("settings-tab-Keyboard");
        screen.click("record-tab.new");
        let recording = screen.cx.update(|_, cx| screen.view.read(cx).recording);
        assert_eq!(recording, Some("tab.new"), "Record went through");

        screen.cx.simulate_keystrokes("escape");
        let recording = screen.cx.update(|_, cx| screen.view.read(cx).recording);
        assert_eq!(recording, None, "the first Esc cancels the recording");
        assert_eq!(screen.closed.get(), 0, "the first Esc doesn't close");

        screen.cx.simulate_keystrokes("escape");
        assert_eq!(screen.closed.get(), 1, "the second Esc closes");
    }
}
