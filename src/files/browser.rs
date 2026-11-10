//! The file browser (spec 0013): one view over one target's session, drawn
//! in an SSH pane's SFTP sidebar (spec 0008) and in a files pane alike.
//! Its target in the header, the always-editable path field, the tree of
//! folders, the questions it asks before anything is replaced, deleted or
//! run, and on SFTP its [`edits`] and transfers. It draws what its session
//! reports and never waits on the network or the disk itself.

mod changes;
mod edits;
mod menu;
mod navigate;
mod render;
mod transfers;

use super::local;
use super::pane::FilesPane;
use super::tree::Tree;
use crate::config::{Config, FilesExecutable, SftpClick, SshProfile, UploadMode};
use crate::sftp::client::Client;
use crate::terminal::TerminalPane;
use changes::Naming;
use edits::Edit;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding,
    SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, actions,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::menu::PopupMenu;
use menu::KeyMenu;
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
}

impl Focusable for Browser {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}
