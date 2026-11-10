//! The Terminal tab: scrollback, panes and the mouse.

use crate::config::{CloseOnExit, OpenLinks};
use crate::settings::change::Change;
use crate::settings::{Field, Page, Row, SettingsView, choice};
use gpui::{Context, IntoElement};

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
        ]
    }
}
