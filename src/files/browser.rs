//! The file browser (spec 0013): one view over one target's session, drawn
//! in an SSH pane's SFTP sidebar (spec 0008) and in a files pane alike.
//! Its target in the header, the always-editable path field, the tree of
//! folders, the questions it asks before anything is replaced, deleted or
//! run, and on SFTP its [`edits`] and transfers. It draws what its session
//! reports and never waits on the network or the disk itself.

mod edits;

use super::local::{self, Opening};
use super::pane::FilesPane;
use super::tree::{Row, Tree};
use super::{Entry, Event, MAX_ENTRIES};
use crate::config::{Config, FilesExecutable, SftpClick, SshProfile, UploadMode};
use crate::sftp::client::{Client, Download, Upload};
use crate::terminal::{TerminalPane, clean_title};
use edits::Edit;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, DismissEvent, Entity, EventEmitter,
    ExternalPaths, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, ParentElement, PathPromptOptions, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, UniformListScrollHandle, WeakEntity,
    Window, actions, anchored, deferred, div, px, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, DropdownMenu, PopupMenu};
use gpui_component::progress::Progress;
use gpui_component::{ActiveTheme, Disableable, Icon, IconName, Sizable};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

/// The browser's key context, where its keys are bound.
const CONTEXT: &str = "FileBrowser";
/// The key context of the field a new name is typed into.
const NAME_CONTEXT: &str = "FileBrowserName";
/// The key context of the path field.
const PATH_CONTEXT: &str = "FileBrowserPath";
/// The key context of an open question's buttons.
const ASK_CONTEXT: &str = "FileBrowserAsk";
/// The height of a row, in logical pixels.
const ROW_HEIGHT: f32 = 24.0;
/// How far each level of the tree is indented, in logical pixels.
const INDENT: f32 = 14.0;

actions!(
    files,
    [
        SelectPrevious,
        SelectNext,
        OpenSelected,
        Expand,
        Collapse,
        GoUp,
        DeleteSelected,
        RenameSelected,
        EditSelected,
        DownloadSelected,
        DownloadSelectedTo,
        UploadFiles,
        NewFolder,
        Refresh,
        FocusPath,
        LeavePath,
        FocusNextControl,
        FocusPreviousControl,
        NextAnswer,
        PreviousAnswer,
        ShowMenu,
        CancelName,
        Dismiss
    ]
);

/// Binds the browser's keys (spec 0013, Keys): everything it does has
/// one; the mouse is a convenience. Arrows move, Right and Left expand and
/// collapse, Enter opens (or connects, while not connected), Ctrl+Enter
/// shows the context menu, Backspace goes up, F5 refreshes, Delete and F2
/// delete and rename, Ctrl+L goes to the path field and Esc back. While a question is open, it has the
/// keyboard. These are the browser's own: no terminal is under them.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("right", Expand, Some(CONTEXT)),
        KeyBinding::new("left", Collapse, Some(CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        KeyBinding::new("backspace", GoUp, Some(CONTEXT)),
        KeyBinding::new("delete", DeleteSelected, Some(CONTEXT)),
        KeyBinding::new("f2", RenameSelected, Some(CONTEXT)),
        KeyBinding::new("f5", Refresh, Some(CONTEXT)),
        KeyBinding::new("ctrl-e", EditSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-d", DownloadSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-d", DownloadSelectedTo, Some(CONTEXT)),
        KeyBinding::new("ctrl-u", UploadFiles, Some(CONTEXT)),
        KeyBinding::new("ctrl-n", NewFolder, Some(CONTEXT)),
        KeyBinding::new("ctrl-l", FocusPath, Some(CONTEXT)),
        KeyBinding::new("ctrl-enter", ShowMenu, Some(CONTEXT)),
        KeyBinding::new("escape", LeavePath, Some(PATH_CONTEXT)),
        // The widget library binds Tab nowhere: here it walks the tree, the
        // header's buttons, the path field, a question, edits and
        // transfers.
        KeyBinding::new("right", NextAnswer, Some(ASK_CONTEXT)),
        KeyBinding::new("left", PreviousAnswer, Some(ASK_CONTEXT)),
        KeyBinding::new("tab", FocusNextControl, Some(CONTEXT)),
        KeyBinding::new("shift-tab", FocusPreviousControl, Some(CONTEXT)),
        KeyBinding::new("escape", CancelName, Some(NAME_CONTEXT)),
    ]);
}

/// What the browser asks of whoever draws it.
pub enum BrowserEvent {
    /// The sidebar's close button: hide the sidebar, keep the session.
    Close,
    /// Something to say as a toast.
    Notice(String),
    /// An edit's upload ended and none is running, so a close waiting on
    /// them can go on, or give up (spec 0008, flow 3).
    Settled,
    /// Run this local file in a new pane beside the browser (spec 0013,
    /// flow 4): the user's named choice, or `files_executable: run`.
    Run(PathBuf),
}

/// The settings the browser follows; its owner passes them on reload.
#[derive(Clone, Copy)]
pub struct BrowserSettings {
    pub click: SftpClick,
    pub upload: UploadMode,
    pub executable: FilesExecutable,
}

impl BrowserSettings {
    pub fn of(config: &Config) -> Self {
        Self {
            click: config.sftp_click,
            upload: config.sftp_upload,
            executable: config.files_executable,
        }
    }
}

/// Where a browser's session comes from: its target, and who builds and
/// logs in each SFTP connection.
pub enum Source {
    /// This machine's disk.
    Local,
    /// An SSH pane's sidebar: the pane builds each connection, and its
    /// prompts open the pane's dialog (spec 0008).
    Pane(WeakEntity<TerminalPane>),
    /// A files pane's own session on an SSH profile, its session `id`
    /// there: the pane builds each connection, and its prompts open the
    /// pane's dialog (spec 0013, Model).
    Files {
        pane: WeakEntity<FilesPane>,
        id: u64,
        profile: SshProfile,
    },
}

/// The running session: the disk's thread or an SFTP connection.
enum Link {
    Local(local::Worker),
    Sftp(Client),
}

impl Link {
    fn list(&self, path: String) {
        match self {
            Link::Local(worker) => worker.list(path),
            Link::Sftp(client) => client.list(path),
        }
    }

    fn rename(&self, from: String, to: String, replace: bool) {
        match self {
            Link::Local(worker) => worker.rename(from, to, replace),
            Link::Sftp(client) => client.rename(from, to, replace),
        }
    }

    /// Removes `path`; on SFTP a `folder` must be empty, and the disk
    /// looks for itself.
    fn delete(&self, path: String, folder: bool) {
        match self {
            Link::Local(worker) => worker.delete(path),
            Link::Sftp(client) => client.delete(path, folder),
        }
    }

    fn make_folder(&self, path: String) {
        match self {
            Link::Local(worker) => worker.make_folder(path),
            Link::Sftp(client) => client.make_folder(path),
        }
    }
}

/// Where the session is.
enum Connection {
    /// Not started: a restored SSH target waits for Connect (spec 0013,
    /// Saved), a restored Local one for the first frame.
    Idle,
    Connecting,
    Connected {
        home: String,
    },
    /// Over, and why.
    Ended(SharedString),
}

/// The header title's dropdown in a files pane: the targets it switches
/// between (spec 0013, Model).
pub type TitleMenu = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// What a transfer does; enough to start it again with `replace` once the
/// user says so.
#[derive(Clone)]
enum Job {
    Download {
        remote: String,
        target: PathBuf,
    },
    Upload {
        local: PathBuf,
        folder: String,
        name: String,
    },
}

/// A running or failed transfer, as its row shows it.
struct Transfer {
    id: u64,
    job: Job,
    /// Its name, by the title rules.
    name: SharedString,
    done: u64,
    total: Option<u64>,
    /// Why it failed; a failed transfer stays until dismissed.
    error: Option<SharedString>,
}

/// A question the browser asks before it writes over, removes or runs
/// anything (spec 0008 and 0013 §Security: writes are the user's, a click
/// never runs a program). One at a time, in order.
enum Ask {
    /// The transfer's target exists.
    Replace(Job),
    /// Remove the file or link at `path`; a `folder` must be empty.
    Delete { path: String, folder: bool },
    /// A rename's new name exists: replace it?
    Rename { from: String, to: String },
    /// A runnable local file: run it, open it as text, or neither.
    Run(PathBuf),
    /// The host's copy of an edit changed since it was fetched: upload
    /// over it anyway?
    Overwrite(u64),
    /// Close an edit whose changes aren't on the host.
    Discard(u64),
}

/// The context menu Ctrl+Enter opens on row `ix`, as a right click
/// would (spec 0013, Keys).
struct KeyMenu {
    ix: usize,
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

/// The field a new name is typed into, and what it names.
struct Naming {
    kind: NameKind,
    input: Entity<InputState>,
    _events: Subscription,
}

enum NameKind {
    Rename { folder: String, from: String },
    NewFolder { folder: String },
}

pub struct Browser {
    focus: FocusHandle,
    source: Source,
    /// `Local` or `user@host`, always in the header and in every question
    /// that writes (TM-sftp-01).
    target: SharedString,
    connection: Connection,
    link: Option<Link>,
    _events: Option<Task<()>>,
    tree: Tree,
    /// The folder to show first once connected; home if `None`.
    start: Option<String>,
    /// The folder being listed as the root now.
    loading: Option<String>,
    /// Why the last root listing failed, shown above the tree kept.
    error: Option<SharedString>,
    /// The row Enter acts on.
    selected: Option<usize>,
    path: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _path_events: Subscription,
    transfers: Vec<Transfer>,
    /// Transfers and edits share these IDs, so an event finds its owner.
    next_transfer: u64,
    asks: VecDeque<Ask>,
    /// The first question's buttons: yes, Edit (a Run question's), Cancel.
    /// It takes the keyboard when it opens, on its safe answer, and Tab
    /// moves between them.
    ask_buttons: [FocusHandle; 3],
    /// Bumped as each question is answered, so the next one is focused.
    asks_answered: u64,
    /// The `asks_answered` whose question has the keyboard.
    ask_focused: Option<u64>,
    naming: Option<Naming>,
    settings: BrowserSettings,
    /// Where this session's edit copies go; made on its first edit. SFTP
    /// only.
    copies: Option<PathBuf>,
    edits: Vec<Edit>,
    /// Looks at the edit copies every second while there are any.
    poll: Option<Task<()>>,
    /// The title's dropdown, in a files pane.
    title_menu: Option<TitleMenu>,
    /// The selected entry's menu, opened by Ctrl+Enter: the row it's on.
    key_menu: Option<KeyMenu>,
    _activation: Subscription,
}

impl EventEmitter<BrowserEvent> for Browser {}

impl Drop for Browser {
    /// The session is over: its edit copies go (spec 0008, Model).
    fn drop(&mut self) {
        if let Some(copies) = self.copies.take() {
            crate::sftp::copies::remove_dir(copies);
        }
    }
}

impl Browser {
    /// A browser over `source`'s target, showing `start` first (else the
    /// home folder). With `connect` it starts its session right away;
    /// otherwise it waits for [`Self::connect`].
    pub fn new(
        source: Source,
        start: Option<String>,
        connect: bool,
        settings: BrowserSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("Path"));
        let path_events = cx.subscribe_in(&path, window, |this, input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let typed = input.read(cx).value().to_string();
                this.go(&typed, window, cx);
            }
        });
        let (target, join, copies): (String, fn(&str, &str) -> String, _) = match &source {
            Source::Local => ("Local".into(), local::join, None),
            Source::Pane(pane) => {
                let target = pane
                    .upgrade()
                    .and_then(|pane| pane.read(cx).ssh_target())
                    .unwrap_or_default();
                (
                    target,
                    crate::sftp::join,
                    crate::sftp::copies::session_dir(),
                )
            }
            Source::Files { profile, .. } => {
                let target = crate::ssh::target_label(&crate::ssh::command(profile).args);
                (
                    target,
                    crate::sftp::join,
                    crate::sftp::copies::session_dir(),
                )
            }
        };
        // Tern's window gaining focus uploads `focus` edits (spec 0008,
        // flow 3).
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.window_focused(cx);
            }
        });
        let mut browser = Self {
            focus: cx.focus_handle(),
            source,
            target: target.into(),
            connection: Connection::Idle,
            link: None,
            _events: None,
            tree: Tree::new(join),
            start,
            loading: None,
            error: None,
            selected: None,
            path,
            scroll: UniformListScrollHandle::new(),
            _path_events: path_events,
            transfers: Vec::new(),
            next_transfer: 0,
            asks: VecDeque::new(),
            ask_buttons: [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()],
            asks_answered: 0,
            ask_focused: None,
            naming: None,
            settings,
            copies,
            edits: Vec::new(),
            poll: None,
            title_menu: None,
            key_menu: None,
            _activation: activation,
        };
        if connect {
            browser.connect(window, cx);
        }
        browser
    }

    /// Follows a reload's settings; edits keep the modes they have.
    pub fn set_settings(&mut self, settings: BrowserSettings) {
        self.settings = settings;
    }

    /// Gives the header's title a dropdown, as a files pane does.
    pub fn set_title_menu(&mut self, menu: TitleMenu) {
        self.title_menu = Some(menu);
    }

    /// `Local` or `user@host`.
    pub fn target(&self) -> SharedString {
        self.target.clone()
    }

    pub fn is_local(&self) -> bool {
        matches!(self.source, Source::Local)
    }

    /// The folder shown, or the one it will show once connected: what a
    /// files pane saves (spec 0013, Saved).
    pub fn folder(&self) -> Option<&str> {
        self.tree.root().or(self.start.as_deref())
    }

    /// Whether the session hasn't started yet.
    pub fn is_idle(&self) -> bool {
        matches!(self.connection, Connection::Idle)
    }

    /// The SFTP connection, if that's the target and it's up.
    fn sftp(&self) -> Option<&Client> {
        match &self.link {
            Some(Link::Sftp(client)) => Some(client),
            _ => None,
        }
    }

    /// The next ID for a transfer or an edit.
    fn next_id(&mut self) -> u64 {
        let id = self.next_transfer;
        self.next_transfer += 1;
        id
    }

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

    fn transfer(&mut self, id: u64) -> Option<&mut Transfer> {
        self.transfers.iter_mut().find(|transfer| transfer.id == id)
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
    fn go(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Connection::Connected { home } = &self.connection {
            let path = self.resolve(home, typed);
            self.list_root(path);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// The entry on row `ix` and its absolute path.
    fn entry(&self, ix: usize) -> Option<(&Entry, String)> {
        self.tree.entry(ix)
    }

    /// Enter or a double click on row `ix` (spec 0013, flows 3 and 4): a
    /// folder becomes the root; a remote file is edited or downloaded as
    /// `sftp_click` says, a local one opened.
    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
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
    fn edit_local(&mut self, file: PathBuf, cx: &mut Context<Self>) {
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
    fn expand(&mut self, ix: usize, step_in: bool, cx: &mut Context<Self>) {
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
    fn collapse(&mut self, ix: usize, cx: &mut Context<Self>) {
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
    fn go_up(&mut self, cx: &mut Context<Self>) {
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
    fn refresh(&mut self, cx: &mut Context<Self>) {
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

    fn select(&mut self, step: isize, cx: &mut Context<Self>) {
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

    /// Downloads the file at `ix` into `folder`, the Downloads folder if
    /// `None` (spec 0008, flow 4). Its local name escapes whatever could
    /// make it a path (TM-sftp-03).
    fn download(&mut self, ix: usize, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some((entry, remote)) = self.entry(ix).filter(|(entry, _)| !entry.is_folder()) else {
            return;
        };
        let name = entry.name.clone();
        let Some(folder) = folder.or_else(dirs::download_dir).or_else(dirs::home_dir) else {
            cx.emit(BrowserEvent::Notice("There's no Downloads folder".into()));
            return;
        };
        let Some(target) = crate::sftp::local_path(&folder, &name, cfg!(windows)) else {
            let notice = format!("{} can't be saved under its name", clean_title(&name));
            cx.emit(BrowserEvent::Notice(notice));
            return;
        };
        self.start_job(Job::Download { remote, target }, false, cx);
    }

    /// Asks the OS for a folder, then downloads the file at `ix` there.
    fn download_to(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, remote)) = self.entry(ix) else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Download here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let folder = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(error)) => {
                    let notice = format!("Couldn't open the file dialog: {error}");
                    let _ = this.update(cx, |_, cx| cx.emit(BrowserEvent::Notice(notice)));
                    None
                }
                _ => None,
            };
            let Some(folder) = folder else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // The tree may have changed while the dialog was open.
                let ix = this
                    .entry(ix)
                    .filter(|(_, path)| *path == remote)
                    .map(|_| ix);
                if let Some(ix) = ix {
                    this.download(ix, Some(folder), cx);
                }
            });
        })
        .detach();
    }

    /// Asks the OS for files, then uploads them into the root folder.
    fn pick_uploads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.tree.root().map(str::to_owned) else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Upload".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match picked.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Err(error)) => {
                    let notice = format!("Couldn't open the file dialog: {error}");
                    let _ = this.update(cx, |_, cx| cx.emit(BrowserEvent::Notice(notice)));
                    return;
                }
                _ => return,
            };
            let _ = this.update(cx, |this, cx| this.upload(paths, folder, cx));
        })
        .detach();
    }

    /// Uploads local files into remote `folder` (spec 0008, flow 5). A
    /// folder among them fails with a reason.
    fn upload(&mut self, paths: Vec<PathBuf>, folder: String, cx: &mut Context<Self>) {
        for local in paths {
            let Some(name) = local
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let job = Job::Upload {
                local,
                folder: folder.clone(),
                name,
            };
            self.start_job(job, false, cx);
        }
    }

    /// Starts `job`; with `replace`, the user confirmed writing over what's
    /// there.
    fn start_job(&mut self, job: Job, replace: bool, cx: &mut Context<Self>) {
        if self.sftp().is_none() {
            return;
        }
        let id = self.next_id();
        let Some(client) = self.sftp() else {
            return;
        };
        let name = match &job {
            Job::Download { remote, .. } => remote.rsplit('/').next().unwrap_or(remote),
            Job::Upload { name, .. } => name,
        };
        let name = clean_title(name).into();
        match job.clone() {
            Job::Download { remote, target } => client.download(Download {
                id,
                remote,
                target,
                replace,
                private: false,
            }),
            Job::Upload {
                local,
                folder,
                name,
            } => client.upload(Upload {
                id,
                local,
                folder,
                name,
                replace,
                expect: None,
            }),
        }
        self.transfers.push(Transfer {
            id,
            job,
            name,
            done: 0,
            total: None,
            error: None,
        });
        cx.notify();
    }

    /// Stops a running transfer, or dismisses a failed one.
    fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(client) = self.sftp() {
            client.cancel(id);
        }
        self.transfers.retain(|transfer| transfer.id != id);
        cx.notify();
    }

    /// Asks before removing the entry at `ix`.
    fn ask_delete(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((entry, path)) = self.entry(ix) else {
            return;
        };
        // A link to a folder is removed as a link (TM-sftp-10).
        let folder = entry.is_folder() && !entry.link;
        self.asks.push_back(Ask::Delete { path, folder });
        cx.notify();
    }

    /// Answers the first question: `yes` goes ahead.
    fn answer(&mut self, yes: bool, cx: &mut Context<Self>) {
        let Some(ask) = self.asks.pop_front() else {
            return;
        };
        self.asks_answered += 1;
        match (ask, yes) {
            (Ask::Replace(job), true) => self.start_job(job, true, cx),
            (Ask::Delete { path, folder }, true) => {
                if let Some(link) = &self.link {
                    link.delete(path, folder);
                }
            }
            (Ask::Rename { from, to }, true) => {
                if let Some(link) = &self.link {
                    link.rename(from, to, true);
                }
            }
            (Ask::Run(file), true) => cx.emit(BrowserEvent::Run(file)),
            (Ask::Overwrite(id), true) => self.upload_edit(id, true, cx),
            (Ask::Discard(id), true) => self.remove_edit(id, cx),
            // A declined overwrite leaves the edit changed; a close that
            // waits on it gives up.
            (Ask::Overwrite(_), false) => cx.emit(BrowserEvent::Settled),
            (_, false) => {}
        }
        cx.notify();
    }

    /// The Edit answer to a Run question: the file opens as plain text.
    fn answer_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(Ask::Run(file)) = self.asks.pop_front() {
            self.asks_answered += 1;
            self.edit_local(file, cx);
        }
        cx.notify();
    }

    /// Enter while a question is open. A Run question opens the file as
    /// text: running takes the button, never a key pressed in passing.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        match self.asks.front() {
            Some(Ask::Run(_)) => self.answer_edit(cx),
            Some(_) => self.answer(true, cx),
            None => {}
        }
    }

    /// Opens the name field to rename the entry at `ix`.
    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((entry, _)), Some(folder)) = (self.entry(ix), self.tree.folder_of(ix)) else {
            return;
        };
        let kind = NameKind::Rename {
            folder: folder.to_owned(),
            from: entry.name.clone(),
        };
        let name = entry.name.clone();
        self.start_naming(kind, &name, window, cx);
    }

    fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.tree.root().map(str::to_owned) else {
            return;
        };
        self.start_naming(NameKind::NewFolder { folder }, "", window, cx);
    }

    fn start_naming(
        &mut self,
        kind: NameKind,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            NameKind::Rename { .. } => "New name",
            NameKind::NewFolder { .. } => "Folder name",
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(clean_title(value))
        });
        let events = cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = input.read(cx).value().trim().to_owned();
                    this.finish_naming(Some(name), window, cx);
                }
                InputEvent::Blur => this.finish_naming(None, window, cx),
                _ => {}
            },
        );
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.naming = Some(Naming {
            kind,
            input,
            _events: events,
        });
        cx.notify();
    }

    /// Ends the name field; `Some` name goes ahead if it's one path
    /// component (spec 0013, flow 5).
    fn finish_naming(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = self.naming.take() else {
            return;
        };
        window.focus(&self.focus, cx);
        cx.notify();
        let Some(name) = name else {
            return;
        };
        let windows = self.is_local() && cfg!(windows);
        if let Err(why) = super::valid_name(&name, windows) {
            cx.emit(BrowserEvent::Notice(why.into()));
            return;
        }
        let Some(link) = &self.link else {
            return;
        };
        let join = |folder: &str, name: &str| {
            if self.is_local() {
                local::join(folder, name)
            } else {
                crate::sftp::join(folder, name)
            }
        };
        match naming.kind {
            NameKind::Rename { folder, from } => {
                if name != from {
                    link.rename(join(&folder, &from), join(&folder, &name), false);
                }
            }
            NameKind::NewFolder { folder } => link.make_folder(join(&folder, &name)),
        }
    }

    /// The selected entry's menu: what can be done with it on this
    /// target, each as the action its key runs, so the menu shows the key.
    fn entry_menu(menu: PopupMenu, focus: FocusHandle, folder: bool, local: bool) -> PopupMenu {
        let menu = menu.action_context(focus);
        let menu = if folder || local {
            menu.menu("Open", Box::new(OpenSelected))
        } else {
            menu.menu("Edit", Box::new(EditSelected))
                .menu("Download", Box::new(DownloadSelected))
                .menu("Download to…", Box::new(DownloadSelectedTo))
        };
        menu.menu("Rename", Box::new(RenameSelected))
            .menu("Delete", Box::new(DeleteSelected))
    }

    fn render_row(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, selected_bg, hover, drop) = (
            theme.muted_foreground,
            theme.danger,
            theme.list_active,
            theme.list_hover,
            theme.drag_border,
        );
        let indent = px(8. + INDENT * row.depth() as f32);
        let line = |id: &'static str| {
            div()
                .id((id, ix))
                .h(px(ROW_HEIGHT))
                .pl(indent)
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_xs()
                .text_color(muted)
        };
        let entry = match row {
            Row::Loading { .. } => {
                return line("files-loading").child("Loading…").into_any_element();
            }
            Row::Truncated { .. } => {
                let note = format!("Showing the first {MAX_ENTRIES} entries");
                return line("files-truncated").child(note).into_any_element();
            }
            Row::Entry { .. } => self.entry(ix),
        };
        let Some((entry, path)) = entry else {
            return line("files-gone").into_any_element();
        };
        let folder = entry.is_folder();
        let expanded = folder && self.tree.is_expanded(&path);
        let failed = folder.then(|| self.tree.error(&path)).flatten();
        let icon = if folder {
            IconName::Folder
        } else {
            IconName::File
        };
        let chevron = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        let size = entry
            .size
            .filter(|_| !folder)
            .map(super::human_size)
            .unwrap_or_default();
        let modified = entry.modified.map(super::date).unwrap_or_default();
        let detail = match failed {
            Some(error) => div()
                .flex_shrink_0()
                .max_w(px(220.))
                .truncate()
                .text_xs()
                .text_color(danger)
                .child(error.to_owned())
                .into_any_element(),
            None => div()
                .flex()
                .flex_shrink_0()
                .gap(px(6.))
                .text_xs()
                .text_color(muted)
                .child(size)
                .child(modified)
                .into_any_element(),
        };
        let focus = self.focus.clone();
        let (local, sftp) = (self.is_local(), !self.is_local());
        div()
            .id(("files-entry", ix))
            .h(px(ROW_HEIGHT))
            .pl(indent)
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(4.))
            .when(self.selected == Some(ix), |row| row.bg(selected_bg))
            .hover(|row| row.bg(hover))
            // First, so it opens at the row's start.
            .children(
                self.key_menu
                    .as_ref()
                    .filter(|open| open.ix == ix)
                    .map(|open| {
                        deferred(
                            anchored()
                                .snap_to_window_with_margin(px(8.))
                                .child(div().mt(px(ROW_HEIGHT)).child(open.menu.clone())),
                        )
                        .with_priority(1)
                    }),
            )
            .child(
                div()
                    .id(("files-chevron", ix))
                    .w(px(14.))
                    .flex_shrink_0()
                    .when(folder, |slot| {
                        slot.child(Icon::new(chevron).xsmall().text_color(muted))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                window.focus(&this.focus, cx);
                                this.selected = Some(ix);
                                if expanded {
                                    this.collapse(ix, cx);
                                } else {
                                    this.expand(ix, false, cx);
                                }
                            }))
                    }),
            )
            .child(Icon::new(icon).xsmall().text_color(muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(entry.link, |name| name.italic())
                    .child(clean_title(&entry.name)),
            )
            .child(detail)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.selected = Some(ix);
                if event.click_count() >= 2 {
                    this.open(ix, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selected = Some(ix);
                    cx.notify();
                }),
            )
            // Files dropped on a remote folder row go into that folder
            // (spec 0008, flow 5).
            .when(folder && sftp, |row| {
                row.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        cx.stop_propagation();
                        this.upload(paths.paths().to_vec(), path.clone(), cx);
                    }))
            })
            .context_menu(move |menu, _, _| Self::entry_menu(menu, focus.clone(), folder, local))
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let line = |text: SharedString| div().px(px(8.)).py(px(4.)).text_xs().child(text);
        let connect = |id: &'static str, label: &'static str, cx: &mut Context<Self>| {
            div().px(px(8.)).child(
                Button::new(id)
                    .label(label)
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            )
        };
        match &self.connection {
            Connection::Idle if self.is_local() => None,
            Connection::Idle => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(line("Not connected".into()).text_color(muted))
                    .child(connect("files-connect", "Connect", cx))
                    .into_any_element(),
            ),
            Connection::Connecting => Some(
                line("Connecting…".into())
                    .text_color(muted)
                    .into_any_element(),
            ),
            Connection::Ended(reason) => {
                let said = if self.is_local() { "" } else { "ssh said:" };
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .when(!said.is_empty(), |status| {
                            status.child(line(said.into()).text_color(muted))
                        })
                        .child(line(reason.clone()).text_color(danger).whitespace_normal())
                        .child(connect("files-reconnect", "Connect again", cx))
                        .into_any_element(),
                )
            }
            Connection::Connected { .. } => match (&self.error, &self.loading) {
                (Some(error), _) => Some(line(error.clone()).text_color(danger).into_any_element()),
                (None, Some(_)) => {
                    Some(line("Loading…".into()).text_color(muted).into_any_element())
                }
                (None, None) => None,
            },
        }
    }

    /// The first open question, naming what goes and where (TM-sftp-01).
    fn render_ask(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let ask = self.asks.front()?;
        let theme = cx.theme();
        let (border, background) = (theme.border, theme.popover);
        let target = &self.target;
        let (question, yes) = match ask {
            Ask::Replace(Job::Download { target, .. }) => {
                (format!("Replace {}?", target.display()), "Replace")
            }
            Ask::Replace(Job::Upload { folder, name, .. }) => (
                format!(
                    "Replace {} on {target}?",
                    clean_title(&crate::sftp::join(folder, name))
                ),
                "Replace",
            ),
            Ask::Delete { path, folder } => {
                let what = clean_title(path);
                let question = if *folder {
                    format!(
                        "Delete the folder {what} on {target}? Only an empty folder can be \
                         deleted. This can't be undone."
                    )
                } else {
                    format!("Delete {what} on {target}? This can't be undone.")
                };
                (question, "Delete")
            }
            Ask::Rename { to, .. } => (
                format!("{} exists on {target}. Replace it?", clean_title(to)),
                "Replace",
            ),
            Ask::Run(file) => (
                format!(
                    "{} is a program. Run it, or open it as text?",
                    clean_title(&file.to_string_lossy())
                ),
                "Run",
            ),
            Ask::Overwrite(id) => {
                let name = self.edit_label(*id);
                let question = format!(
                    "{name} changed on {target} since you opened it. Upload your copy over it?"
                );
                (question, "Upload anyway")
            }
            Ask::Discard(id) => {
                let name = self.edit_label(*id);
                let question =
                    format!("Close {name}? Your changes aren't on {target} and will be lost.");
                (question, "Discard")
            }
        };
        let run = matches!(ask, Ask::Run(_));
        Some(
            div()
                .key_context(ASK_CONTEXT)
                .on_action(cx.listener(|this, _: &NextAnswer, window, cx| {
                    this.step_answer(1, window, cx);
                }))
                .on_action(cx.listener(|this, _: &PreviousAnswer, window, cx| {
                    this.step_answer(-1, window, cx);
                }))
                .m(px(6.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(px(4.))
                .border_1()
                .border_color(border)
                .bg(background)
                .text_xs()
                .child(div().whitespace_normal().child(question))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            Button::new("files-ask-yes")
                                .track_focus(&self.ask_buttons[0])
                                .label(yes)
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| this.answer(true, cx))),
                        )
                        .when(run, |buttons| {
                            buttons.child(
                                Button::new("files-ask-edit")
                                    .track_focus(&self.ask_buttons[1])
                                    .label("Edit")
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, _, cx| this.answer_edit(cx))),
                            )
                        })
                        .child(
                            Button::new("files-ask-no")
                                .track_focus(&self.ask_buttons[2])
                                .label("Cancel")
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| this.answer(false, cx))),
                        ),
                ),
        )
    }

    fn render_naming(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let naming = self.naming.as_ref()?;
        Some(
            div()
                .key_context(NAME_CONTEXT)
                .on_action(cx.listener(|this, _: &CancelName, window, cx| {
                    this.finish_naming(None, window, cx);
                }))
                .px(px(6.))
                .py(px(4.))
                .child(Input::new(&naming.input).xsmall()),
        )
    }

    fn render_transfers(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.transfers.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (border, muted, danger) = (theme.border, theme.muted_foreground, theme.danger);
        let rows = self.transfers.iter().map(|transfer| {
            let id = transfer.id;
            let icon = match transfer.job {
                Job::Download { .. } => IconName::ArrowDown,
                Job::Upload { .. } => IconName::ArrowUp,
            };
            let percent = transfer
                .total
                .filter(|&total| total > 0)
                .map(|total| transfer.done as f32 * 100.0 / total as f32);
            let amount = match transfer.total {
                Some(total) => format!(
                    "{} / {}",
                    super::human_size(transfer.done),
                    super::human_size(total)
                ),
                None => super::human_size(transfer.done),
            };
            let status = match &transfer.error {
                Some(error) => div()
                    .text_color(danger)
                    .whitespace_normal()
                    .child(error.clone())
                    .into_any_element(),
                None => Progress::new(("files-progress", id))
                    .value(percent.unwrap_or_default())
                    .loading(percent.is_none() && transfer.done == 0)
                    .into_any_element(),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(8.))
                .py(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(Icon::new(icon).xsmall().text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(transfer.name.clone()),
                        )
                        .child(div().flex_shrink_0().text_color(muted).child(amount))
                        .child(
                            Button::new(("files-cancel", id))
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| this.cancel(id, cx))),
                        ),
                )
                .child(status)
        });
        Some(
            div()
                .flex_shrink_0()
                .max_h(px(200.))
                .border_t_1()
                .border_color(border)
                .text_xs()
                .children(rows),
        )
    }

    /// Ctrl+Enter: the selected entry's context menu, at its row, with the
    /// keyboard in it. Its actions go to the browser, as a right click's.
    fn show_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ix, folder)) = self
            .selected
            .and_then(|ix| Some((ix, self.entry(ix)?.0.is_folder())))
        else {
            return;
        };
        let (focus, local) = (self.focus.clone(), self.is_local());
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            Self::entry_menu(menu, focus, folder, local)
        });
        let dismissed =
            cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
                // An action may have moved the keyboard, like Rename into its
                // field; only a menu left without one gives it back.
                if menu.focus_handle(cx).is_focused(window) || window.focused(cx).is_none() {
                    window.focus(&this.focus, cx);
                }
                this.key_menu = None;
                cx.notify();
            });
        menu.focus_handle(cx).focus(window, cx);
        self.key_menu = Some(KeyMenu {
            ix,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    /// Which of the open question's buttons has the keyboard: 0 yes, 1
    /// Edit, 2 Cancel.
    fn focused_answer(&self, window: &Window) -> Option<usize> {
        self.asks.front()?;
        self.ask_buttons
            .iter()
            .position(|button| button.is_focused(window))
    }

    /// Left and Right between the open question's buttons, stopping at
    /// either end.
    fn step_answer(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let run = matches!(self.asks.front(), Some(Ask::Run(_)));
        let shown: Vec<&FocusHandle> = self
            .ask_buttons
            .iter()
            .enumerate()
            .filter(|(ix, _)| run || *ix != 1)
            .map(|(_, button)| button)
            .collect();
        let Some(at) = shown.iter().position(|button| button.is_focused(window)) else {
            return;
        };
        let next = at.saturating_add_signed(step).min(shown.len() - 1);
        let target = shown[next].clone();
        window.focus(&target, cx);
    }

    /// Moves the keyboard to a question as it opens, onto its safe answer:
    /// Edit for a Run question, else the one Enter has always meant (spec
    /// 0008, flow 6). Once the questions are answered, it goes back to the
    /// tree if nothing else took it. Only while the browser has the
    /// keyboard.
    fn follow_asks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.asks.front() {
            Some(_) if self.ask_focused == Some(self.asks_answered) => return,
            Some(ask) => {
                if !self.focus.contains_focused(window, cx) {
                    return;
                }
                self.ask_focused = Some(self.asks_answered);
                let button = if matches!(ask, Ask::Run(_)) { 1 } else { 0 };
                self.ask_buttons[button].clone()
            }
            None if self.ask_focused.take().is_some() => {
                // Back to the tree, unless the answer moved the keyboard
                // elsewhere, like Run into its new pane.
                cx.defer_in(window, |this, window, cx| {
                    let in_question = this.ask_buttons.iter().any(|b| b.is_focused(window));
                    if in_question || window.focused(cx).is_none() {
                        window.focus(&this.focus, cx);
                    }
                });
                return;
            }
            None => return,
        };
        cx.defer_in(window, move |_, window, cx| window.focus(&target, cx));
    }

    /// The header's title: the target, as a dropdown in a files pane.
    fn render_title(&self) -> AnyElement {
        let label = clean_title(&self.target);
        match &self.title_menu {
            Some(menu) => {
                let menu = Rc::clone(menu);
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Button::new("files-target")
                            .label(label)
                            .icon(IconName::ChevronDown)
                            .ghost()
                            .small()
                            .tooltip("Switch between Local and your hosts")
                            .dropdown_menu(move |popup, window, cx| menu(popup, window, cx)),
                    )
                    .into_any_element()
            }
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .child(label)
                .into_any_element(),
        }
    }
}

impl Focusable for Browser {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_asks(window, cx);
        let theme = cx.theme();
        let (border, muted, drop) = (theme.border, theme.muted_foreground, theme.drag_border);
        let shown = self.link.is_some() && self.tree.root().is_some();
        let sftp = !self.is_local();
        let closable = matches!(self.source, Source::Pane(_));
        let header = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .py(px(4.))
            // In a files pane, the split's grab handle sits in the top
            // right corner (spec 0007): the buttons stay clear of it.
            .when(self.title_menu.is_some(), |header| header.pr(px(32.)))
            .border_b_1()
            .border_color(border)
            .child(self.render_title())
            .when(sftp, |header| {
                header.child(
                    Button::new("files-upload")
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .tooltip("Upload files… (Ctrl+U)")
                        .disabled(!shown)
                        .on_click(cx.listener(|this, _, window, cx| this.pick_uploads(window, cx))),
                )
            })
            .child(
                Button::new("files-new-folder")
                    .icon(IconName::FolderClosed)
                    .ghost()
                    .xsmall()
                    .tooltip("New folder (Ctrl+N)")
                    .disabled(!shown)
                    .on_click(cx.listener(|this, _, window, cx| this.start_new_folder(window, cx))),
            )
            .child(
                Button::new("files-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Refresh (F5)")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .when(closable, |header| {
                header.child(
                    Button::new("files-close")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Close sidebar")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserEvent::Close))),
                )
            });
        let path_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .py(px(4.))
            .child(
                Button::new("files-up")
                    .icon(IconName::ArrowUp)
                    .ghost()
                    .xsmall()
                    .tooltip("Up (Backspace)")
                    .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .key_context(PATH_CONTEXT)
                    .on_action(cx.listener(|this, _: &LeavePath, window, cx| {
                        window.focus(&this.focus, cx);
                    }))
                    .child(Input::new(&self.path).xsmall()),
            );
        let rows = self.tree.rows().len();
        let list = match self.tree.root_listing() {
            Some(listing) if listing.entries.is_empty() => div()
                .px(px(8.))
                .py(px(4.))
                .text_xs()
                .text_color(muted)
                .child("Empty folder")
                .into_any_element(),
            Some(_) => uniform_list(
                "files-rows",
                rows,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .filter_map(|ix| {
                            let row = this.tree.rows().get(ix)?.clone();
                            Some(this.render_row(ix, &row, cx))
                        })
                        .collect::<Vec<AnyElement>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
            None => div().into_any_element(),
        };
        // Files dropped anywhere else on a remote tree go into its root
        // folder (spec 0008, flow 5).
        let list = div()
            .id("files-drop")
            .flex_1()
            .min_h_0()
            .when(sftp, |list| {
                list.drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.border_1().border_color(drop)
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    if let Some(folder) = this.tree.root().map(str::to_owned) {
                        this.upload(paths.paths().to_vec(), folder, cx);
                    }
                }))
            })
            .child(list);
        div()
            .id("file-browser")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // The tree is the first tab stop; the controls follow in order.
            .tab_group()
            .tab_index(0)
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| {
                window.focus_next(cx);
            }))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| {
                window.focus_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &Expand, _, cx| {
                if let Some(ix) = this.selected {
                    this.expand(ix, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Collapse, _, cx| {
                if let Some(ix) = this.selected {
                    this.collapse(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                // Enter on a question's button presses that button.
                if let Some(button) = this.focused_answer(window) {
                    match button {
                        0 => this.answer(true, cx),
                        1 => this.answer_edit(cx),
                        _ => this.answer(false, cx),
                    }
                } else if !this.focus.is_focused(window) {
                    // Another control has the keyboard: Enter is its click.
                    cx.propagate();
                } else if !this.asks.is_empty() {
                    this.confirm(cx);
                } else if matches!(this.connection, Connection::Idle | Connection::Ended(_)) {
                    this.connect(window, cx);
                } else if let Some(ix) = this.selected {
                    this.open(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditSelected, _, cx| {
                if let Some(ix) = this.selected.filter(|_| !this.is_local()) {
                    this.edit(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DownloadSelected, _, cx| {
                if let Some(ix) = this.selected.filter(|_| !this.is_local()) {
                    this.download(ix, None, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DownloadSelectedTo, window, cx| {
                if let Some(ix) = this.selected.filter(|_| !this.is_local()) {
                    this.download_to(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UploadFiles, window, cx| {
                if !this.is_local() {
                    this.pick_uploads(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                this.start_new_folder(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowMenu, window, cx| {
                this.show_menu(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPath, window, cx| {
                this.path.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.answer(false, cx)))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.go_up(cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &DeleteSelected, _, cx| {
                // One question at a time: a second Delete doesn't queue another.
                if let Some(ix) = this.selected.filter(|_| this.asks.is_empty()) {
                    this.ask_delete(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSelected, window, cx| {
                if let Some(ix) = this.selected {
                    this.start_rename(ix, window, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .text_sm()
            .child(header)
            .child(path_row)
            .children(self.render_status(cx))
            .children(self.render_ask(cx))
            .children(self.render_naming(cx))
            .child(list)
            .children(self.render_edits(cx))
            .children(self.render_transfers(cx))
    }
}
