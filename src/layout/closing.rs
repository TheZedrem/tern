//! Closing panes, tabs and the window, and asking first when SFTP edits
//! would go (spec 0008, flow 3).

use super::tabs::{Closed, TabId};
use super::tree::LeafId;
use super::{Workspace, answers, show_notice};
use crate::commands::ClosePane;
use crate::session::Session;
use gpui::{App, Context, ParentElement, SharedString, Styled, Window, div, px};
use gpui_component::WindowExt;
use gpui_component::button::{Button, ButtonVariants};
use std::rc::Rc;

/// What a close is about to remove, kept while it waits on SFTP uploads
/// (spec 0008, flow 3).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Closing {
    Pane(LeafId),
    Tab(TabId),
    Window,
}

impl Workspace {
    /// The panes `closing` would close.
    fn closing_leaves(&self, closing: Closing) -> Vec<LeafId> {
        match closing {
            Closing::Pane(leaf) => vec![leaf],
            Closing::Tab(id) => self
                .tabs
                .tabs()
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.root.leaves())
                .unwrap_or_default(),
            Closing::Window => self
                .tabs
                .tabs()
                .iter()
                .flat_map(|tab| tab.root.leaves())
                .collect(),
        }
    }

    /// The SFTP edits `closing` would delete: each one's name, and whether
    /// its changes aren't on the host.
    pub(super) fn edits_of(&self, closing: Closing, cx: &App) -> Vec<(SharedString, bool)> {
        let leaves = self.closing_leaves(closing);
        let sidebars = leaves
            .iter()
            .filter_map(|leaf| self.sftp.get(leaf)?.files.as_ref())
            .flat_map(|files| files.view.read(cx).open_edits());
        let files = leaves
            .iter()
            .filter_map(|leaf| self.files.get(leaf))
            .flat_map(|files| files.pane.read(cx).open_edits(cx));
        sidebars.chain(files).collect()
    }

    /// Closes a pane, a tab or the window, asking first when that would
    /// delete SFTP edits (spec 0008, flow 3).
    pub(super) fn close(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        let edits = self.edits_of(closing, cx);
        if edits.is_empty() {
            self.close_now(closing, window, cx);
        } else {
            self.ask_close(closing, edits, window, cx);
        }
    }

    fn close_now(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        match closing {
            Closing::Pane(leaf) => self.close_pane(leaf, window, cx),
            Closing::Tab(id) => {
                if let Some(ix) = self.tabs.tabs().iter().position(|tab| tab.id == id) {
                    self.close_tab(ix, window, cx);
                }
            }
            Closing::Window => {
                self.window_may_close = true;
                window.remove_window();
            }
        }
    }

    /// The question before a close deletes SFTP edits: it names them, calls
    /// out the ones whose changes aren't on the host, and offers to upload
    /// those first.
    pub(super) fn ask_close(
        &mut self,
        closing: Closing,
        edits: Vec<(SharedString, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = cx.entity().downgrade();
        let unsent = edits.iter().any(|(_, unsent)| *unsent);
        let what = match closing {
            Closing::Pane(_) => "this pane",
            Closing::Tab(_) => "this tab",
            Closing::Window => "this window",
        };
        let lines: Vec<SharedString> = edits
            .iter()
            .map(|(name, unsent)| match unsent {
                true => format!("{name}: changed, not uploaded").into(),
                false => name.clone(),
            })
            .collect();
        let answer = |label: &'static str, style: fn(Button) -> Button, upload: Option<bool>| {
            let workspace = workspace.clone();
            answers::Answer {
                label: label.into(),
                style,
                act: Rc::new(move |window: &mut Window, cx: &mut App| {
                    let _ = workspace.update(cx, |workspace, cx| {
                        workspace.answer_close(closing, upload, window, cx);
                    });
                }),
            }
        };
        let mut choices = vec![
            answer("Cancel", |button| button, None),
            answer("Close", |button| button.danger(), Some(false)),
        ];
        if unsent {
            choices.push(answer(
                "Upload and close",
                |button| button.primary(),
                Some(true),
            ));
        }
        // Enter with no button chosen is Cancel: nothing is lost by it.
        let choices = answers::Answers::new(choices, 0, cx);
        choices.focus_safe(window);
        window.open_dialog(cx, move |dialog, _, _| {
            choices.apply(dialog).title(format!("Close {what}?")).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_sm()
                    .child("Its SFTP edit copies are deleted when it closes:")
                    .children(
                        lines
                            .iter()
                            .map(|line| div().pl(px(8.)).child(line.clone())),
                    ),
            )
        });
    }

    /// The answer to [`Self::ask_close`]: `None` cancels, `Some(true)`
    /// uploads the unsent edits and closes once they're on the host.
    fn answer_close(
        &mut self,
        closing: Closing,
        upload: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match upload {
            None => {}
            Some(false) => self.close_now(closing, window, cx),
            Some(true) => {
                self.closing = Some(closing);
                for leaf in self.closing_leaves(closing) {
                    if let Some(files) = self.sftp.get(&leaf).and_then(|s| s.files.as_ref()) {
                        files.view.update(cx, |view, cx| view.upload_unsent(cx));
                    }
                    if let Some(files) = self.files.get(&leaf) {
                        files.pane.update(cx, |pane, cx| pane.upload_unsent(cx));
                    }
                }
                self.finish_closing(window, cx);
            }
        }
    }

    /// Goes on with a close that waited on uploads once none is running:
    /// it closes if the host has every change, else it stays open and says
    /// so, and the sidebar says why.
    pub(super) fn finish_closing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closing) = self.closing else {
            return;
        };
        let mut sent = true;
        for leaf in self.closing_leaves(closing) {
            let sidebar = self
                .sftp
                .get(&leaf)
                .and_then(|sidebar| sidebar.files.as_ref());
            let settled = match (sidebar, self.files.get(&leaf)) {
                (Some(sidebar), _) => sidebar.view.read(cx).settled(),
                (_, Some(files)) => files.pane.read(cx).settled(cx),
                (None, None) => continue,
            };
            match settled {
                None => return,
                Some(all) => sent &= all,
            }
        }
        self.closing = None;
        if sent {
            self.close_now(closing, window, cx);
        } else {
            let notice = "Not closed: an SFTP edit didn't upload; its file browser says why";
            show_notice(notice.into(), window, cx);
        }
    }

    /// Stops a pane's program and forgets the pane, or a restored pane that
    /// never started.
    fn stop_pane(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        self.pending.remove(&leaf);
        // Its SFTP session ends with it, and its ssh with the session; a
        // files pane's sessions end the same way. Its port forwards don't:
        // they belong to the host (spec 0017, flow 7).
        self.sftp.remove(&leaf);
        self.files.remove(&leaf);
        self.pane_subscriptions.remove(&leaf);
        if let Some(pane) = self.panes.remove(&leaf) {
            pane.update(cx, |pane, _| pane.shutdown());
            self.plugins_pane_closed(leaf, cx);
        }
    }

    fn after_close(&mut self, closed: Closed, window: &mut Window, cx: &mut Context<Self>) {
        if closed == Closed::Last {
            // Nothing is left to restore: the normal case, not a corrupt
            // file (spec 0003, flow 7).
            self.save_last(Session::new(self.window_size, Vec::new(), 0));
            cx.quit();
            return;
        }
        self.show_active(window, cx);
    }

    fn close_pane(&mut self, leaf: LeafId, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_pane(leaf, cx);
        let closed = self.tabs.close_pane(leaf);
        self.after_close(closed, window, cx);
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (leaves, closed) = self.tabs.close_tab(ix);
        for leaf in leaves {
            self.stop_pane(leaf, cx);
        }
        self.after_close(closed, window, cx);
    }

    pub(super) fn close_focused_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaf = self.tabs.active().focused;
        self.close(Closing::Pane(leaf), window, cx);
    }

    /// Closes the tab at `ix`, asking first if SFTP edits would go.
    pub(super) fn ask_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.tabs().get(ix).map(|tab| tab.id) {
            self.close(Closing::Tab(id), window, cx);
        }
    }
}
