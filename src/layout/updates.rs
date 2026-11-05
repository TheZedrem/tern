//! The workspace's side of the update check (spec 0011): the one-time
//! question, the daily check while `update_check: daily`, and the notices.
//! All of it starts after the first frame.

use super::{Workspace, show_notice};
use crate::commands::CheckForUpdates;
use crate::config::UpdateCheck;
use crate::settings::{change::Change, update_check_name};
use crate::update::{self, Release};
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AppContext, Context, IntoElement, ParentElement, Styled, Task, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::notification::Notification;
use gpui_component::{Sizable, WindowExt};

/// Identifies the question's notice, so an answer elsewhere can close it.
struct UpdateQuestion;
/// Identifies the update notice, so a newer one replaces it.
struct UpdateAvailable;

impl Workspace {
    /// Follows `update_check`: asks once per run while it's unset, and runs
    /// the daily check exactly while it's `daily`. Called after the first
    /// frame and on every reload.
    pub(super) fn follow_update_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.config.update_check {
            None => {
                self.update_timer = None;
                if !std::mem::replace(&mut self.update_asked, true) {
                    self.ask_about_updates(window, cx);
                }
            }
            Some(answer) => {
                // Answered, maybe on the settings screen or in the file.
                window.remove_notification::<UpdateQuestion>(cx);
                match answer {
                    UpdateCheck::Daily if self.update_timer.is_none() => {
                        self.update_timer = Some(self.daily_checks(window, cx));
                    }
                    UpdateCheck::Daily => {}
                    UpdateCheck::Off => self.update_timer = None,
                }
            }
        }
    }

    /// The question (spec 0011, flow 2). A button writes its answer, and
    /// the reload that follows starts or skips the check; closing the
    /// notice writes nothing, so the next run asks again.
    fn ask_about_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx.entity().downgrade();
        let notice = Notification::new()
            .id::<UpdateQuestion>()
            .placement(Anchor::BottomRight)
            .autohide(false)
            .message(
                "Check GitHub for new Tern releases once a day? \
                 This sends your IP address to GitHub.",
            )
            .content(move |_, _, cx| {
                let answer = |id: &'static str, label: &'static str, value: UpdateCheck| {
                    let workspace = workspace.clone();
                    Button::new(id)
                        .label(label)
                        .small()
                        .when(value == UpdateCheck::Daily, |button| button.primary())
                        .on_click(cx.listener(move |note, _, window, cx| {
                            let _ = workspace.update(cx, |workspace, cx| {
                                workspace.write_change(answer_change(value), window, cx);
                            });
                            note.dismiss(window, cx);
                        }))
                };
                div()
                    .flex()
                    .gap(px(6.))
                    .pt(px(8.))
                    .child(answer("update-daily", "Check daily", UpdateCheck::Daily))
                    .child(answer("update-off", "Don't check", UpdateCheck::Off))
                    .into_any_element()
            });
        window.push_notification(notice, cx);
    }

    /// The daily check (spec 0011, flow 3): due a day after the last one,
    /// which may have been in an earlier run, then every day. Dropping the
    /// task stops it. A failed check is silent and retried a day later.
    fn daily_checks(&self, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            let path = update::state_path();
            let read = path.clone();
            let last = cx
                .background_spawn(async move { read.and_then(|path| update::last_check(&path)) })
                .await;
            let mut wait = update::due_in(last, update::now());
            loop {
                cx.background_executor().timer(wait).await;
                let checked = update::check().await;
                if let Some(path) = path.clone() {
                    let now = update::now();
                    let saved = cx
                        .background_spawn(async move { update::save_last_check(&path, now) })
                        .await;
                    if let Err(error) = saved {
                        eprintln!("tern: the time of the update check couldn't be saved: {error}");
                    }
                }
                if let Ok(Some(release)) = checked {
                    let shown =
                        this.update_in(cx, |_, window, cx| show_release(release, window, cx));
                    if shown.is_err() {
                        break;
                    }
                }
                // Not read back from the file: a save that failed must not
                // make the next check due at once.
                wait = update::INTERVAL;
            }
        })
    }

    /// `app.checkForUpdates` (spec 0011, flow 6): checks now under any
    /// setting and says how it went, also when Tern is up to date.
    pub(super) fn check_for_updates(
        &mut self,
        _: &CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |_, cx| {
            let checked = update::check().await;
            let _ = cx.update(|window, cx| match checked {
                Ok(Some(release)) => show_release(release, window, cx),
                Ok(None) => show_notice(
                    format!("Tern {} is up to date.", env!("CARGO_PKG_VERSION")),
                    window,
                    cx,
                ),
                Err(error) => {
                    let notice =
                        Notification::error(format!("Couldn't check for updates: {error}"))
                            .placement(Anchor::BottomRight);
                    window.push_notification(notice, cx);
                }
            });
        })
        .detach();
    }
}

/// What a button of the question writes: `daily` or `off`, never anything
/// meaning "not chosen".
fn answer_change(value: UpdateCheck) -> Change {
    Change::Set {
        path: vec!["update_check"],
        value: match value {
            UpdateCheck::Daily => "daily",
            UpdateCheck::Off => "off",
        }
        .to_owned(),
        reads: |config| update_check_name(config).to_owned(),
    }
}

/// "Tern X.Y.Z is available", until it's closed or a newer check replaces
/// it (spec 0011, flow 4). Its button opens the release page, which
/// [`update::parse_release`] checked is on Tern's GitHub.
fn show_release(release: Release, window: &mut Window, cx: &mut gpui::App) {
    window.remove_notification::<UpdateAvailable>(cx);
    let Release { version, url } = release;
    let notice = Notification::new()
        .id::<UpdateAvailable>()
        .placement(Anchor::BottomRight)
        .message(format!("Tern {version} is available."))
        .action(move |_, _, _| {
            let url = url.clone();
            Button::new("open-release")
                .label("Open release page")
                .on_click(move |_, _, cx| cx.open_url(&url))
        });
    window.push_notification(notice, cx);
}
