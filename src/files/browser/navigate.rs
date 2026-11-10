//! Getting around: the session's connection and its events, the tree,
//! the selection, the path field and opening what's on a row.

use super::{Ask, Browser, BrowserEvent, Connection, Job, Link, Source};
use crate::config::{FilesExecutable, SftpClick};
use crate::files::local::{self, Opening};
use crate::files::tree::Row;
use crate::files::{Entry, Event};
use crate::sftp::client::Client;
use crate::terminal::clean_title;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{AppContext, Context, ScrollStrategy, Window};
use std::path::PathBuf;

impl Browser {
    /// Starts the session; a running one ends, its transfers with it.
    /// Edits stay, and upload over the new connection (spec 0008, flow 7).
    pub fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.link = None;
        self.loading = None;
        self.error = None;
        self.transfers.clear();
        self.asks
            .retain(|ask| matches!(ask, Ask::Overwrite(_) | Ask::Discard(_)));
        let started = match &self.source {
            Source::Local => {
                let (worker, events) = local::Worker::start();
                Some((Link::Local(worker), events))
            }
            Source::Pane(pane) => {
                let pane = pane.clone();
                pane.update(cx, |pane, cx| pane.sftp_command(window, cx))
                    .ok()
                    .flatten()
                    .map(Self::sftp_link)
            }
            Source::Files { pane, id, profile } => {
                let (pane, id, profile) = (pane.clone(), *id, profile.clone());
                pane.update(cx, |pane, cx| pane.sftp_command(id, &profile, window, cx))
                    .ok()
                    .map(Self::sftp_link)
            }
        };
        let Some((link, mut events)) = started else {
            self.connection = Connection::Ended("This pane has no SSH connection".into());
            cx.notify();
            return;
        };
        self.connection = Connection::Connecting;
        self.link = Some(link);
        self._events = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.next().await {
                let handled = this.update_in(cx, |view, window, cx| view.event(event, window, cx));
                if handled.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn sftp_link(command: crate::terminal::pty::Command) -> (Link, UnboundedReceiver<Event>) {
        let (client, events) = Client::connect(command);
        (Link::Sftp(client), events)
    }

    fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            // TODO(T-022): an SSH target starts in the folder the pane's
            // shell reported.
            Event::Connected { home } => {
                self.connection = Connection::Connected { home: home.clone() };
                // A reconnect shows the folder it showed.
                let first = self
                    .tree
                    .root()
                    .map(str::to_owned)
                    .or_else(|| self.start.clone())
                    .unwrap_or(home);
                self.list_root(first);
            }
            Event::Listed(listing) => {
                let keep = self.selected_path();
                if self.loading.as_deref() == Some(listing.path.as_str()) {
                    self.loading = None;
                    self.error = None;
                    self.start = None;
                    self.path.update(cx, |input, cx| {
                        input.set_value(listing.path.clone(), window, cx)
                    });
                    let new_root = self.tree.root() != Some(listing.path.as_str());
                    if new_root {
                        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    }
                    self.tree.set_root(listing);
                    self.reselect(keep.filter(|_| !new_root));
                } else if self.tree.listed(listing) {
                    self.reselect(keep);
                }
            }
            Event::ListFailed { path, error } => {
                if self.loading.as_deref() == Some(path.as_str()) {
                    self.loading = None;
                    // A start folder that's gone shows home instead (spec
                    // 0008, Model; spec 0013, Saved).
                    if let Connection::Connected { home } = &self.connection
                        && self.tree.root().is_none()
                        && *home != path
                    {
                        let home = home.clone();
                        self.list_root(home);
                    } else {
                        let error = format!("Couldn't open {}: {error}", clean_title(&path));
                        self.error = Some(error.into());
                    }
                } else {
                    let keep = self.selected_path();
                    if self.tree.failed(&path, clean_title(&error)) {
                        self.reselect(keep);
                    }
                }
            }
            Event::Progress { id, done, total } => {
                if let Some(edit) = self.edit_mut(id) {
                    edit.done = done;
                    edit.total = total;
                } else if let Some(transfer) = self.transfer(id) {
                    transfer.done = done;
                    transfer.total = total;
                }
            }
            Event::TransferDone { id, stat } if self.edit_mut(id).is_some() => {
                self.edit_done(id, stat, cx);
            }
            Event::TransferFailed { id, error } if self.edit_mut(id).is_some() => {
                self.edit_failed(id, error, cx);
            }
            Event::Exists { id } if self.edit_mut(id).is_some() => self.edit_conflict(id, cx),
            Event::TransferDone { id, .. } => {
                if let Some(ix) = self.transfers.iter().position(|transfer| transfer.id == id) {
                    let transfer = self.transfers.remove(ix);
                    if let Job::Download { target, .. } = &transfer.job {
                        let notice = format!("Downloaded {}", target.display());
                        cx.emit(BrowserEvent::Notice(notice));
                    }
                }
            }
            Event::TransferFailed { id, error } => {
                if let Some(transfer) = self.transfer(id) {
                    transfer.error = Some(error.into());
                }
            }
            Event::Exists { id } => {
                if let Some(ix) = self.transfers.iter().position(|transfer| transfer.id == id) {
                    let transfer = self.transfers.remove(ix);
                    self.asks.push_back(Ask::Replace(transfer.job));
                }
            }
            Event::RenameExists { from, to } => self.asks.push_back(Ask::Rename { from, to }),
            Event::Changed { folder } => {
                if self.tree.root() == Some(folder.as_str()) {
                    self.list_root(folder);
                } else if self.tree.shows(&folder)
                    && let Some(link) = &self.link
                {
                    link.list(folder);
                }
            }
            Event::Failed(error) => {
                cx.emit(BrowserEvent::Notice(format!("{}: {error}", self.target)));
            }
            Event::Ended(reason) => {
                self.link = None;
                self.loading = None;
                self.transfers.retain(|transfer| transfer.error.is_some());
                self.connection = Connection::Ended(reason.into());
                self.edits_disconnected(cx);
            }
        }
        cx.notify();
    }

    /// Asks for the folder at absolute `path`, to show as the root.
    fn list_root(&mut self, path: String) {
        if let Some(link) = &self.link {
            self.loading = Some(path.clone());
            link.list(path);
        }
    }

    /// The path of the entry on the selected row.
    fn selected_path(&self) -> Option<String> {
        self.tree.entry(self.selected?).map(|(_, path)| path)
    }

    /// Selects the row of `path` after the rows changed, else keeps the
    /// row number if it's still there, else the first row.
    fn reselect(&mut self, path: Option<String>) {
        let count = self.tree.rows().len();
        self.selected = path
            .and_then(|path| self.tree.row_of(&path))
            .or_else(|| self.selected.filter(|&ix| ix < count))
            .or((count > 0).then_some(0));
    }

    /// Where `typed` in the path field goes on this target.
    fn resolve(&self, home: &str, typed: &str) -> String {
        if self.is_local() {
            local::resolve(home, typed)
        } else {
            crate::sftp::resolve(home, typed)
        }
    }

    /// Shows what was typed into the path field (spec 0008, Model).
    pub(super) fn go(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Connection::Connected { home } = &self.connection {
            let path = self.resolve(home, typed);
            self.list_root(path);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// The entry on row `ix` and its absolute path.
    pub(super) fn entry(&self, ix: usize) -> Option<(&Entry, String)> {
        self.tree.entry(ix)
    }

    /// Enter or a double click on row `ix` (spec 0013, flows 3 and 4): a
    /// folder becomes the root; a remote file is edited or downloaded as
    /// `sftp_click` says, a local one opened.
    pub(super) fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((entry, path)) = self.entry(ix) else {
            return;
        };
        if entry.is_folder() {
            self.list_root(path);
        } else if self.is_local() {
            self.open_local(PathBuf::from(path), cx);
        } else {
            match self.settings.click {
                SftpClick::Edit => self.edit(ix, cx),
                SftpClick::Download => self.download(ix, None, cx),
            }
        }
        cx.notify();
    }

    /// Opens a local file itself (spec 0013, flow 4, §Security): with its
    /// default app, unless that would run it. A runnable file does what
    /// `files_executable` says; an edit-only one opens as plain text.
    fn open_local(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        let checked = cx.background_spawn({
            let file = file.clone();
            async move { local::opening_of(&file) }
        });
        cx.spawn(async move |this, cx| {
            let opening = checked.await;
            let _ = this.update(cx, |this, cx| match opening {
                Ok(Opening::Open) => cx.open_with_system(&file),
                Ok(Opening::EditOnly) => this.edit_local(file, cx),
                Ok(Opening::Runnable) => match this.settings.executable {
                    FilesExecutable::Ask => {
                        this.asks.push_back(Ask::Run(file));
                        cx.notify();
                    }
                    FilesExecutable::Run => cx.emit(BrowserEvent::Run(file)),
                    FilesExecutable::Edit => this.edit_local(file, cx),
                },
                Err(error) => {
                    let notice = format!("Couldn't open {}: {error}", file.display());
                    cx.emit(BrowserEvent::Notice(notice));
                }
            });
        })
        .detach();
    }

    /// Opens a local file with the plain-text app, off the UI thread.
    pub(super) fn edit_local(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        let edited = cx.background_spawn(async move { local::edit(&file) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = edited.await {
                let _ = this.update(cx, |_, cx| cx.emit(BrowserEvent::Notice(error)));
            }
        })
        .detach();
    }

    /// Right or a click on a chevron: opens the folder on row `ix`,
    /// listing it the first time; on an open one, `step_in` moves to its
    /// first entry.
    pub(super) fn expand(&mut self, ix: usize, step_in: bool, cx: &mut Context<Self>) {
        let Some((entry, path)) = self.entry(ix) else {
            return;
        };
        if !entry.is_folder() {
            return;
        }
        if self.tree.is_expanded(&path) {
            if step_in && self.tree.rows().get(ix + 1).map(Row::depth) > Some(self.depth(ix)) {
                self.select_row(ix + 1);
            }
        } else if self.tree.expand(&path)
            && let Some(link) = &self.link
        {
            link.list(path);
        }
        cx.notify();
    }

    /// Left: closes the folder on row `ix` if it's open, else moves to the
    /// folder it's in.
    pub(super) fn collapse(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((_, path)) = self.entry(ix) else {
            return;
        };
        if !self.tree.collapse(&path)
            && let Some(parent) = self.tree.parent_row(ix)
        {
            self.select_row(parent);
        }
        cx.notify();
    }

    fn depth(&self, ix: usize) -> usize {
        self.tree.rows().get(ix).map_or(0, Row::depth)
    }

    /// Backspace: the root's parent becomes the root.
    pub(super) fn go_up(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.tree.root().map(str::to_owned) else {
            return;
        };
        let parent = if self.is_local() {
            local::parent(&root)
        } else {
            crate::sftp::parent(&root)
        };
        if parent != root {
            self.list_root(parent);
            cx.notify();
        }
    }

    /// F5: lists the root and every folder open in the tree again.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut folders = self.tree.refresh().into_iter();
        if let Some(root) = folders.next() {
            self.list_root(root);
        }
        if let Some(link) = &self.link {
            for folder in folders {
                link.list(folder);
            }
        }
        cx.notify();
    }

    pub(super) fn select(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.tree.rows().len();
        if count == 0 {
            return;
        }
        let next = match self.selected {
            Some(ix) => ix.saturating_add_signed(step).min(count - 1),
            None => 0,
        };
        self.select_row(next);
        cx.notify();
    }

    fn select_row(&mut self, ix: usize) {
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
    }
}
