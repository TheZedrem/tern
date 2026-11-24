//! The workspace's side of the command history (spec 0021): starting it
//! after the first frame, following `command_history`, and the clear
//! command. The history itself is `crate::history`.

use super::Workspace;
use crate::commands::{AcceptSuggestion, ClearCommandHistory};
use crate::history::{Recorder, file};
use futures::StreamExt;
use gpui::{Context, Global, Window};

impl Global for Recorder {}

impl Workspace {
    /// Starts the history thread, which loads the file (spec 0021, flow
    /// 2). The first failed file operation of a run shows a notice; the
    /// history carries on in memory.
    pub(super) fn start_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (recorder, mut failed) =
            Recorder::start(self.config.command_history, file::default_path());
        cx.set_global(recorder);
        cx.spawn_in(window, async move |_, cx| {
            let Some(error) = failed.next().await else {
                return;
            };
            let _ = cx.update(|window, cx| {
                super::show_notice(
                    format!("Command history is kept only until Tern quits: {error}"),
                    window,
                    cx,
                );
            });
            // Later failures of this run stay quiet.
            while failed.next().await.is_some() {}
        })
        .detach();
    }

    /// Applies a reloaded `command_history`.
    pub(super) fn history_reloaded(&mut self, cx: &mut Context<Self>) {
        let mode = self.config.command_history;
        if cx.has_global::<Recorder>() {
            cx.global_mut::<Recorder>().set_mode(mode);
        }
    }

    pub(super) fn clear_command_history(
        &mut self,
        _: &ClearCommandHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(recorder) = cx.try_global::<Recorder>() {
            recorder.clear();
        }
        // Every open pane drops the suggestion it shows.
        cx.refresh_windows();
        super::show_notice("Command history cleared.".into(), window, cx);
    }

    /// Accepts the focused pane's suggestion (spec 0021, flow 4). With
    /// none showing the key isn't ours [load-bearing]: it goes on to
    /// whatever has the keyboard, so Right still moves the shell's cursor.
    pub(super) fn accept_suggestion(
        &mut self,
        _: &AcceptSuggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let accepted = self
            .focused_pane()
            .is_some_and(|pane| pane.update(cx, |pane, cx| pane.accept_suggestion(window, cx)));
        if !accepted {
            cx.propagate();
        }
    }
}
