//! The General tab: startup, updates and what double-clicks do.

use crate::config::{Config, FilesExecutable, NewTab, SftpClick, UpdateCheck, UploadMode};
use crate::settings::change::Change;
use crate::settings::{Page, Pick, Row, SettingsView, choice};
use gpui::prelude::FluentBuilder;
use gpui::{Context, IntoElement, ParentElement, Styled, div};
use gpui_component::ActiveTheme;

impl SettingsView {
    /// The General tab's rows.
    pub(in crate::settings) fn general_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let config = &self.config;
        let default_profile = config
            .default_profile
            .clone()
            .unwrap_or_else(|| "shell".to_owned());
        vec![
            Row::new(
                Page::General,
                "Startup",
                "New tabs open",
                "default profile shell startup",
                self.picker(
                    "pick-default-profile",
                    default_profile,
                    Pick::DefaultProfile,
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::General,
                "Startup",
                "Restore the last session",
                "restore session tabs splits reopen relaunch startup",
                self.switch(
                    "restore-session",
                    config.restore_session,
                    |on| Change::Set {
                        path: vec!["restore_session"],
                        value: on.to_string(),
                        reads: |config| config.restore_session.to_string(),
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::General,
                "Startup",
                if cfg!(target_os = "macos") {
                    "Cmd+T opens"
                } else {
                    "Ctrl+Shift+T opens"
                },
                "new tab key shortcut profile ask default picker swap",
                self.options(
                    "new-tab",
                    &[
                        ("ask", "A profile picker"),
                        ("default", "The default profile"),
                    ],
                    new_tab_name(config),
                    |value| {
                        choice(vec!["new_tab"], value, |config| {
                            new_tab_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::General,
                "Updates",
                "Check GitHub for new releases",
                "update upgrade new version release github check",
                self.options(
                    "update-check",
                    &[("daily", "Check daily"), ("off", "Don't check")],
                    update_check_name(config),
                    |value| {
                        choice(vec!["update_check"], value, |config| {
                            update_check_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .when(config.update_check.is_none(), |options| {
                    options.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Not chosen yet"),
                    )
                })
                .into_any_element(),
            ),
            Row::new(
                Page::General,
                "SFTP",
                "Double-clicking a remote file",
                "sftp remote file click open edit download double",
                self.options(
                    "sftp-click",
                    &[("edit", "Edits it"), ("download", "Downloads it")],
                    sftp_click_name(config),
                    |value| {
                        choice(vec!["sftp_click"], value, |config| {
                            sftp_click_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::General,
                "SFTP",
                "Edits upload",
                "sftp edit upload save focus manual remote",
                self.options(
                    "sftp-upload",
                    &[
                        ("manual", "On a click"),
                        ("save", "On save"),
                        ("focus", "On focus"),
                    ],
                    sftp_upload_name(config),
                    |value| {
                        choice(vec!["sftp_upload"], value, |config| {
                            sftp_upload_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::General,
                "Files",
                "Double-clicking a local program",
                "files pane local executable program script run edit ask double click",
                self.options(
                    "files-executable",
                    &[
                        ("ask", "Asks"),
                        ("run", "Runs it"),
                        ("edit", "Opens it as text"),
                    ],
                    files_executable_name(config),
                    |value| {
                        choice(vec!["files_executable"], value, |config| {
                            files_executable_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
        ]
    }
}

/// `new_tab` as the file writes it.
fn new_tab_name(config: &Config) -> &'static str {
    match config.new_tab {
        NewTab::Ask => "ask",
        NewTab::Default => "default",
    }
}

/// `update_check` as the file writes it; empty while it's not chosen,
/// which is never written (spec 0011).
pub fn update_check_name(config: &Config) -> &'static str {
    match config.update_check {
        Some(UpdateCheck::Daily) => "daily",
        Some(UpdateCheck::Off) => "off",
        None => "",
    }
}

/// `sftp_click` as the file writes it.
fn sftp_click_name(config: &Config) -> &'static str {
    match config.sftp_click {
        SftpClick::Edit => "edit",
        SftpClick::Download => "download",
    }
}

/// `sftp_upload` as the file writes it.
fn sftp_upload_name(config: &Config) -> &'static str {
    match config.sftp_upload {
        UploadMode::Manual => "manual",
        UploadMode::Save => "save",
        UploadMode::Focus => "focus",
    }
}

/// `files_executable` as the file writes it.
fn files_executable_name(config: &Config) -> &'static str {
    match config.files_executable {
        FilesExecutable::Ask => "ask",
        FilesExecutable::Run => "run",
        FilesExecutable::Edit => "edit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FIELDS;

    /// Spec 0011, AC10: the Updates field shows `daily` or `off` as set,
    /// and neither while the question is still open.
    #[test]
    fn the_updates_field_shows_neither_choice_while_unset() {
        let name = |update_check| {
            update_check_name(&Config {
                update_check,
                ..Config::default()
            })
        };
        assert_eq!(name(None), "");
        assert_eq!(name(Some(UpdateCheck::Daily)), "daily");
        assert_eq!(name(Some(UpdateCheck::Off)), "off");
        assert!(FIELDS.contains(&"update_check"));
    }
}
