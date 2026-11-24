//! The Terminal tab: scrollback, panes and the mouse.

use crate::commands::ClearCommandHistory;
use crate::config::{CloseOnExit, CommandHistory, Config, OpenLinks};
use crate::settings::change::Change;
use crate::settings::{Field, Page, Row, SettingsView, choice};
use gpui::{Context, IntoElement, ParentElement, Styled, div, px};
use gpui_component::button::Button;
use gpui_component::{ActiveTheme, Sizable};

impl SettingsView {
    /// The Terminal tab's rows.
    pub(in crate::settings) fn terminal_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let config = &self.config;
        let close = match config.close_on_exit {
            CloseOnExit::Always => "always",
            CloseOnExit::Clean => "clean",
            CloseOnExit::Never => "never",
        };
        let links = match config.open_links {
            OpenLinks::Click => "click",
            OpenLinks::CtrlClick => "ctrl_click",
        };
        vec![
            Row::new(
                Page::Terminal,
                "Scrollback",
                "Scrollback lines",
                "history buffer lines scrollback",
                self.field(Field::Scrollback),
            ),
            Row::new(
                Page::Terminal,
                "Panes",
                "Close a pane on exit",
                "exit code quit close shell ends",
                self.options(
                    "close-on-exit",
                    &[
                        ("always", "Always"),
                        ("clean", "On exit code 0"),
                        ("never", "Never"),
                    ],
                    close,
                    |value| {
                        choice(vec!["close_on_exit"], value, |config| {
                            match config.close_on_exit {
                                CloseOnExit::Always => "always",
                                CloseOnExit::Clean => "clean",
                                CloseOnExit::Never => "never",
                            }
                            .to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::Terminal,
                "Mouse",
                "Open links with",
                "url hyperlink link click ctrl browser open",
                self.options(
                    "open-links",
                    &[
                        ("click", "A click"),
                        (
                            "ctrl_click",
                            if cfg!(target_os = "macos") {
                                "Cmd+click"
                            } else {
                                "Ctrl+click"
                            },
                        ),
                    ],
                    links,
                    |value| {
                        choice(vec!["open_links"], value, |config| {
                            match config.open_links {
                                OpenLinks::Click => "click",
                                OpenLinks::CtrlClick => "ctrl_click",
                            }
                            .to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::Terminal,
                "Mouse",
                "Copy on select",
                "clipboard selection copy mouse",
                self.switch(
                    "copy-on-select",
                    config.copy_on_select,
                    |on| Change::Set {
                        path: vec!["copy_on_select"],
                        value: on.to_string(),
                        reads: |config| config.copy_on_select.to_string(),
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::Terminal,
                "Command history",
                "Suggest past commands",
                "command history suggestions shell integration autosuggest save memory clear forget",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(self.options(
                        "command-history",
                        &[
                            ("save", "Kept in a file"),
                            ("memory", "Until Tern quits"),
                            ("off", "Off"),
                        ],
                        command_history_name(config),
                        |value| {
                            choice(vec!["command_history"], value, |config| {
                                command_history_name(config).to_owned()
                            })
                        },
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "The file, history.jsonl, is plain text. Needs the shell                                  integration; commands that look like they hold a secret                                  are never kept.",
                            ),
                    )
                    .into_any_element(),
            ),
            Row::new(
                Page::Terminal,
                "Command history",
                "Forget every command",
                "command history clear delete forget erase",
                Button::new("clear-command-history")
                    .label("Clear command history")
                    .small()
                    .outline()
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ClearCommandHistory), cx)
                    })
                    .into_any_element(),
            ),
        ]
    }
}

/// `command_history` as the file writes it.
fn command_history_name(config: &Config) -> &'static str {
    match config.command_history {
        CommandHistory::Save => "save",
        CommandHistory::Memory => "memory",
        CommandHistory::Off => "off",
    }
}
