//! The sidebar of one SSH pane (spec 0008): its host in the header, the
//! path field, the flat listing, the questions it asks before anything is
//! replaced or deleted, its [`edits`] and its transfers. It draws what its
//! [`Client`] reports and never waits on the network or the disk itself.

mod edits;

use super::client::{Client, Download, Event, Upload};
use super::{Entry, Listing, MAX_ENTRIES};
use crate::config::{Config, SftpClick, UploadMode};
use crate::terminal::{TerminalPane, clean_title};
use edits::Edit;
use futures::StreamExt;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, EventEmitter, ExternalPaths,
    FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, PathPromptOptions, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, UniformListScrollHandle, WeakEntity,
    Window, actions, div, px, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_component::progress::Progress;
use gpui_component::{ActiveTheme, Disableable, Icon, IconName, Sizable};
use std::collections::VecDeque;
use std::path::PathBuf;

/// The sidebar's key context, where its keys are bound.
const CONTEXT: &str = "SftpSidebar";
/// The key context of the field a new name is typed into.
const NAME_CONTEXT: &str = "SftpName";
/// The height of a listing row, in logical pixels.
const ROW_HEIGHT: f32 = 24.0;

actions!(
    sftp,
    [
        SelectPrevious,
        SelectNext,
        OpenSelected,
        GoUp,
        DeleteSelected,
        RenameSelected,
        CancelName,
        Dismiss
    ]
);

/// Binds the listing's keys: arrows move, Enter opens, Backspace goes up
/// (spec 0008, flow 2), Delete and F2 delete and rename. While a question
/// is open, Enter answers it and Esc cancels it. The path field's own
/// bindings win while it has the keyboard.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        KeyBinding::new("backspace", GoUp, Some(CONTEXT)),
        KeyBinding::new("delete", DeleteSelected, Some(CONTEXT)),
        KeyBinding::new("f2", RenameSelected, Some(CONTEXT)),
        KeyBinding::new("escape", CancelName, Some(NAME_CONTEXT)),
    ]);
}

/// What the sidebar asks of the workspace.
pub enum SftpEvent {
    /// Its close button: hide the sidebar, keep the session.
    Close,
    /// Something to say as a toast.
    Notice(String),
    /// An edit's upload ended and none is running, so a close waiting on
    /// them can go on, or give up (spec 0008, flow 3).
    Settled,
}

/// The settings the sidebar follows; the workspace passes them on reload.
#[derive(Clone, Copy)]
pub struct SftpSettings {
    pub click: SftpClick,
    pub upload: UploadMode,
}

impl SftpSettings {
    pub fn of(config: &Config) -> Self {
        Self {
            click: config.sftp_click,
            upload: config.sftp_upload,
        }
    }
}

/// Where the connection is.
enum Connection {
    Connecting,
    Connected {
        home: String,
    },
    /// Over, and why.
    Ended(SharedString),
}

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

/// A question the sidebar asks before it writes over or removes anything
/// (spec 0008 §Security, writes are the user's). One at a time, in order.
enum Ask {
    /// The transfer's target exists.
    Replace(Job),
    /// Remove the file or link at `path`; a `folder` must be empty.
    Delete { path: String, folder: bool },
    /// The host's copy of an edit changed since it was fetched: upload
    /// over it anyway?
    Overwrite(u64),
    /// Close an edit whose changes aren't on the host.
    Discard(u64),
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

pub struct SftpView {
    focus: FocusHandle,
    /// The pane whose host this is, which builds each connection's command.
    pane: WeakEntity<TerminalPane>,
    /// `user@host`, always in the header and in every question
    /// (TM-sftp-01).
    target: SharedString,
    connection: Connection,
    client: Option<Client>,
    _events: Option<Task<()>>,
    /// What's listed; kept when a later listing fails.
    listing: Option<Listing>,
    /// The folder being listed now.
    loading: Option<String>,
    /// Why the last listing failed, shown above the one kept.
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
    naming: Option<Naming>,
    settings: SftpSettings,
    /// Where this session's edit copies go; made on its first edit.
    copies: Option<PathBuf>,
    edits: Vec<Edit>,
    /// Looks at the edit copies every second while there are any.
    poll: Option<Task<()>>,
    _activation: Subscription,
}

impl EventEmitter<SftpEvent> for SftpView {}

impl Drop for SftpView {
    /// The session is over: its edit copies go (spec 0008, Model).
    fn drop(&mut self) {
        if let Some(copies) = self.copies.take() {
            super::copies::remove_dir(copies);
        }
    }
}

impl SftpView {
    /// The sidebar of `pane`, which starts connecting right away (spec
    /// 0008, flow 1).
    pub fn new(
        pane: &Entity<TerminalPane>,
        settings: SftpSettings,
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
        let target = pane.read(cx).ssh_target().unwrap_or_default();
        // Tern's window gaining focus uploads `focus` edits (flow 3).
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.window_focused(cx);
            }
        });
        let mut view = Self {
            focus: cx.focus_handle(),
            pane: pane.downgrade(),
            target: target.into(),
            connection: Connection::Connecting,
            client: None,
            _events: None,
            listing: None,
            loading: None,
            error: None,
            selected: None,
            path,
            scroll: UniformListScrollHandle::new(),
            _path_events: path_events,
            transfers: Vec::new(),
            next_transfer: 0,
            asks: VecDeque::new(),
            naming: None,
            settings,
            copies: super::copies::session_dir(),
            edits: Vec::new(),
            poll: None,
            _activation: activation,
        };
        view.connect(window, cx);
        view
    }

    /// Follows a reload's settings; edits keep the modes they have.
    pub fn set_settings(&mut self, settings: SftpSettings) {
        self.settings = settings;
    }

    /// The next ID for a transfer or an edit.
    fn next_id(&mut self) -> u64 {
        let id = self.next_transfer;
        self.next_transfer += 1;
        id
    }

    /// Starts a connection with a new askpass token from the pane; the
    /// last one, if any, ends, and its transfers with it. Edits stay, and
    /// upload over the new one (flow 7).
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.client = None;
        self.loading = None;
        self.error = None;
        self.transfers.clear();
        self.asks
            .retain(|ask| matches!(ask, Ask::Overwrite(_) | Ask::Discard(_)));
        let command = self
            .pane
            .update(cx, |pane, cx| pane.sftp_command(window, cx))
            .ok()
            .flatten();
        let Some(command) = command else {
            self.connection = Connection::Ended("This pane has no SSH connection".into());
            cx.notify();
            return;
        };
        self.connection = Connection::Connecting;
        let (client, mut events) = Client::connect(command);
        self.client = Some(client);
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

    fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            // TODO(T-022): start in the folder the pane's shell reported.
            Event::Connected { home } => {
                self.connection = Connection::Connected { home: home.clone() };
                self.list(home);
            }
            Event::Listed(listing) => {
                self.loading = None;
                self.error = None;
                self.path.update(cx, |input, cx| {
                    input.set_value(listing.path.clone(), window, cx)
                });
                let same = self.listing.as_ref().map(|shown| &shown.path) == Some(&listing.path);
                if !same {
                    self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                let last = listing.entries.len().checked_sub(1);
                self.selected = match (same, self.selected) {
                    (true, Some(ix)) => last.map(|last| ix.min(last)),
                    _ => last.map(|_| 0),
                };
                self.listing = Some(listing);
            }
            Event::ListFailed { path, error } => {
                self.loading = None;
                self.error = Some(format!("Couldn't open {}: {error}", clean_title(&path)).into());
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
                        cx.emit(SftpEvent::Notice(notice));
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
            Event::Changed { folder } => {
                if self.listing.as_ref().map(|listing| &listing.path) == Some(&folder) {
                    self.list(folder);
                }
            }
            Event::Failed(error) => cx.emit(SftpEvent::Notice(format!("{}: {error}", self.target))),
            Event::Ended(reason) => {
                self.client = None;
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

    /// Asks for the folder at absolute `path`.
    fn list(&mut self, path: String) {
        if let Some(client) = &self.client {
            self.loading = Some(path.clone());
            client.list(path);
        }
    }

    /// Lists what was typed into the path field (spec 0008, Model).
    fn go(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Connection::Connected { home } = &self.connection {
            let path = super::resolve(home, typed);
            self.list(path);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// The entry at `ix` and its absolute path.
    fn entry(&self, ix: usize) -> Option<(&Entry, String)> {
        let listing = self.listing.as_ref()?;
        let entry = listing.entries.get(ix)?;
        Some((entry, super::join(&listing.path, &entry.name)))
    }

    /// Enters the folder at `ix`; a file is edited or downloaded to the
    /// Downloads folder, as `sftp_click` says (flow 2).
    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((entry, path)) = self.entry(ix) else {
            return;
        };
        if entry.is_folder() {
            self.list(path);
        } else {
            match self.settings.click {
                SftpClick::Edit => self.edit(ix, cx),
                SftpClick::Download => self.download(ix, None, cx),
            }
        }
        cx.notify();
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(listing) = &self.listing {
            let parent = super::parent(&listing.path);
            if parent != listing.path {
                self.list(parent);
                cx.notify();
            }
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.listing.as_ref().map(|listing| listing.path.clone()) {
            self.list(path);
            cx.notify();
        }
    }

    fn select(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self
            .listing
            .as_ref()
            .map_or(0, |listing| listing.entries.len());
        if count == 0 {
            return;
        }
        let next = match self.selected {
            Some(ix) => ix.saturating_add_signed(step).min(count - 1),
            None => 0,
        };
        self.selected = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
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
            cx.emit(SftpEvent::Notice("There's no Downloads folder".into()));
            return;
        };
        let Some(target) = super::local_path(&folder, &name, cfg!(windows)) else {
            let notice = format!("{} can't be saved under its name", clean_title(&name));
            cx.emit(SftpEvent::Notice(notice));
            return;
        };
        self.start(Job::Download { remote, target }, false, cx);
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
                    let _ = this.update(cx, |_, cx| cx.emit(SftpEvent::Notice(notice)));
                    None
                }
                _ => None,
            };
            let Some(folder) = folder else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // The listing may have changed while the dialog was open.
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

    /// Asks the OS for files, then uploads them into the folder shown.
    fn pick_uploads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.listing.as_ref().map(|listing| listing.path.clone()) else {
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
                    let _ = this.update(cx, |_, cx| cx.emit(SftpEvent::Notice(notice)));
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
            self.start(job, false, cx);
        }
    }

    /// Starts `job`; with `replace`, the user confirmed writing over what's
    /// there.
    fn start(&mut self, job: Job, replace: bool, cx: &mut Context<Self>) {
        if self.client.is_none() {
            return;
        }
        let id = self.next_id();
        let Some(client) = &self.client else {
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
        if let Some(client) = &self.client {
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
        match (ask, yes) {
            (Ask::Replace(job), true) => self.start(job, true, cx),
            (Ask::Delete { path, folder }, true) => {
                if let Some(client) = &self.client {
                    client.delete(path, folder);
                }
            }
            (Ask::Overwrite(id), true) => self.upload_edit(id, true, cx),
            (Ask::Discard(id), true) => self.remove_edit(id, cx),
            // A declined overwrite leaves the edit changed; a close that
            // waits on it gives up.
            (Ask::Overwrite(_), false) => cx.emit(SftpEvent::Settled),
            (_, false) => {}
        }
        cx.notify();
    }

    /// Opens the name field to rename the entry at `ix`.
    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(listing) = &self.listing else {
            return;
        };
        let Some(entry) = listing.entries.get(ix) else {
            return;
        };
        let kind = NameKind::Rename {
            folder: listing.path.clone(),
            from: entry.name.clone(),
        };
        let name = entry.name.clone();
        self.start_naming(kind, &name, window, cx);
    }

    fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.listing.as_ref().map(|listing| listing.path.clone()) else {
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

    /// Ends the name field; `Some` non-empty name goes ahead.
    fn finish_naming(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = self.naming.take() else {
            return;
        };
        window.focus(&self.focus, cx);
        cx.notify();
        let (Some(name), Some(client)) = (name.filter(|name| !name.is_empty()), &self.client)
        else {
            return;
        };
        match naming.kind {
            NameKind::Rename { folder, from } => {
                if name != from {
                    client.rename(super::join(&folder, &from), super::join(&folder, &name));
                }
            }
            NameKind::NewFolder { folder } => client.make_folder(super::join(&folder, &name)),
        }
    }

    /// The entry's menu: what can be done with it.
    fn entry_menu(view: &WeakEntity<Self>, ix: usize, folder: bool) -> Vec<PopupMenuItem> {
        let item = |label: &'static str,
                    act: fn(&mut Self, usize, &mut Window, &mut Context<Self>)| {
            let view = view.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| act(this, ix, window, cx));
            })
        };
        let mut items = Vec::new();
        if folder {
            items.push(item("Open", |this, ix, _, cx| this.open(ix, cx)));
        } else {
            items.push(item("Edit", |this, ix, _, cx| this.edit(ix, cx)));
            items.push(item("Download", |this, ix, _, cx| {
                this.download(ix, None, cx)
            }));
            items.push(item("Download to…", |this, ix, window, cx| {
                this.download_to(ix, window, cx)
            }));
        }
        items.push(item("Rename", |this, ix, window, cx| {
            this.start_rename(ix, window, cx)
        }));
        items.push(item("Delete", |this, ix, _, cx| this.ask_delete(ix, cx)));
        items
    }

    fn render_row(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, selected_bg, hover, drop) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.drag_border,
        );
        let folder = entry.is_folder();
        let icon = if folder {
            IconName::Folder
        } else {
            IconName::File
        };
        let size = entry
            .size
            .filter(|_| !folder)
            .map(super::human_size)
            .unwrap_or_default();
        let modified = entry.modified.map(super::date).unwrap_or_default();
        let view = cx.entity().downgrade();
        let path = self
            .listing
            .as_ref()
            .map(|listing| super::join(&listing.path, &entry.name))
            .unwrap_or_default();
        div()
            .id(("sftp-entry", ix))
            .h(px(ROW_HEIGHT))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .when(self.selected == Some(ix), |row| row.bg(selected_bg))
            .hover(|row| row.bg(hover))
            .child(Icon::new(icon).xsmall().text_color(muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(entry.link, |name| name.italic())
                    .child(clean_title(&entry.name)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(muted)
                    .child(size),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(muted)
                    .child(modified),
            )
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
                cx.listener(move |this, _, _, cx| {
                    this.selected = Some(ix);
                    cx.notify();
                }),
            )
            // Files dropped on a folder row go into that folder (flow 5).
            .when(folder, |row| {
                row.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        cx.stop_propagation();
                        this.upload(paths.paths().to_vec(), path.clone(), cx);
                    }))
            })
            .context_menu(move |menu, _, _| {
                Self::entry_menu(&view, ix, folder)
                    .into_iter()
                    .fold(menu, |menu, item| menu.item(item))
            })
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let line = |text: SharedString| div().px(px(8.)).py(px(4.)).text_xs().child(text);
        match &self.connection {
            Connection::Connecting => Some(
                line("Connecting…".into())
                    .text_color(muted)
                    .into_any_element(),
            ),
            Connection::Ended(reason) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(line("ssh said:".into()).text_color(muted))
                    .child(line(reason.clone()).text_color(danger).whitespace_normal())
                    .child(
                        div().px(px(8.)).child(
                            Button::new("sftp-reconnect")
                                .label("Connect again")
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.connect(window, cx)),
                                ),
                        ),
                    )
                    .into_any_element(),
            ),
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
        let (question, yes) = match ask {
            Ask::Replace(Job::Download { target, .. }) => {
                (format!("Replace {}?", target.display()), "Replace")
            }
            Ask::Replace(Job::Upload { folder, name, .. }) => (
                format!(
                    "Replace {} on {}?",
                    clean_title(&super::join(folder, name)),
                    self.target
                ),
                "Replace",
            ),
            Ask::Delete { path, folder } => {
                let what = clean_title(path);
                let question = if *folder {
                    format!(
                        "Delete the folder {what} on {}? Only an empty folder can be deleted.",
                        self.target
                    )
                } else {
                    format!("Delete {what} on {}?", self.target)
                };
                (question, "Delete")
            }
            Ask::Overwrite(id) => {
                let name = self.edit_label(*id);
                let question = format!(
                    "{name} changed on {} since you opened it. Upload your copy over it?",
                    self.target
                );
                (question, "Upload anyway")
            }
            Ask::Discard(id) => {
                let name = self.edit_label(*id);
                let question = format!(
                    "Close {name}? Your changes aren't on {} and will be lost.",
                    self.target
                );
                (question, "Discard")
            }
        };
        Some(
            div()
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
                            Button::new("sftp-ask-yes")
                                .label(yes)
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| this.answer(true, cx))),
                        )
                        .child(
                            Button::new("sftp-ask-no")
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
                None => Progress::new(("sftp-progress", id))
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
                            Button::new(("sftp-cancel", id))
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
}

impl Focusable for SftpView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SftpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, drop) = (theme.border, theme.muted_foreground, theme.drag_border);
        let connected = self.client.is_some() && self.listing.is_some();
        let header = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .py(px(4.))
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(clean_title(&self.target)),
            )
            .child(
                Button::new("sftp-upload")
                    .icon(IconName::Plus)
                    .ghost()
                    .xsmall()
                    .tooltip("Upload files…")
                    .disabled(!connected)
                    .on_click(cx.listener(|this, _, window, cx| this.pick_uploads(window, cx))),
            )
            .child(
                Button::new("sftp-new-folder")
                    .icon(IconName::FolderClosed)
                    .ghost()
                    .xsmall()
                    .tooltip("New folder")
                    .disabled(!connected)
                    .on_click(cx.listener(|this, _, window, cx| this.start_new_folder(window, cx))),
            )
            .child(
                Button::new("sftp-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                Button::new("sftp-close")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tooltip("Close sidebar")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SftpEvent::Close))),
            );
        let path_row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .py(px(4.))
            .child(
                Button::new("sftp-up")
                    .icon(IconName::ArrowUp)
                    .ghost()
                    .xsmall()
                    .tooltip("Up")
                    .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.path).xsmall()),
            );
        let entries = self
            .listing
            .as_ref()
            .map_or(0, |listing| listing.entries.len());
        let list = match &self.listing {
            Some(listing) if listing.entries.is_empty() => div()
                .px(px(8.))
                .py(px(4.))
                .text_xs()
                .text_color(muted)
                .child("Empty folder")
                .into_any_element(),
            Some(_) => uniform_list(
                "sftp-entries",
                entries,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    let Some(listing) = this.listing.take() else {
                        return Vec::new();
                    };
                    let rows: Vec<AnyElement> = range
                        .filter_map(|ix| Some(this.render_row(ix, listing.entries.get(ix)?, cx)))
                        .collect();
                    this.listing = Some(listing);
                    rows
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
            None => div().into_any_element(),
        };
        // Files dropped anywhere else on the listing go into the folder
        // shown (flow 5).
        let list = div()
            .id("sftp-drop")
            .flex_1()
            .min_h_0()
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_1().border_color(drop))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(folder) = this.listing.as_ref().map(|listing| listing.path.clone()) {
                    this.upload(paths.paths().to_vec(), folder, cx);
                }
            }))
            .child(list);
        let truncated = self
            .listing
            .as_ref()
            .filter(|listing| listing.truncated)
            .map(|_| {
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_xs()
                    .text_color(muted)
                    .child(format!("Showing the first {MAX_ENTRIES} entries"))
            });
        div()
            .id("sftp-sidebar")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &OpenSelected, _, cx| {
                if !this.asks.is_empty() {
                    this.answer(true, cx);
                } else if let Some(ix) = this.selected {
                    this.open(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.answer(false, cx)))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.go_up(cx)))
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
            .children(truncated)
            .children(self.render_edits(cx))
            .children(self.render_transfers(cx))
    }
}
