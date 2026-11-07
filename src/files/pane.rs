//! The files pane (spec 0013): a leaf of a tab's split tree that draws the
//! file browser instead of a terminal. It holds one session per target it
//! has shown (Local, or a saved SSH profile with its own ssh), switched in
//! the header's dropdown; every session ends when the pane closes. The
//! layout moves, zooms and restores it like a terminal pane.

use super::browser::{Browser, BrowserEvent, BrowserSettings, Source, TitleMenu};
use crate::config::{Config, ProfileKind, SshProfile};
use crate::ssh::askpass::{self, Prompt};
use crate::terminal::prompt::{Answer, PromptDialog, Remember};
use crate::terminal::pty::Command;
use crate::terminal::{PromptHost, PromptNotice, answer_prompts};
use futures::channel::oneshot;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, SharedString, Styled, Subscription, Task,
    WeakEntity, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{ActiveTheme, Sizable, WindowExt};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

/// What the pane tells the workspace.
pub enum FilesEvent {
    /// Its first frame is on screen: the TTFP moment of a tab without a
    /// terminal (spec 0001).
    FirstFrame,
    /// The user clicked into the pane.
    Focused,
    /// The target shown changed, and with it the pane's title.
    TitleChanged,
    /// A prompt dialog opened or closed, which moves the keyboard focus.
    DialogChanged,
    Notice(String),
    /// There is no keychain; the workspace says so once (spec 0006, flow 8).
    NoKeychain,
    /// An edit's upload ended (spec 0008, flow 3).
    Settled,
    /// Run this local file in a new pane beside this one (flow 4).
    Run(PathBuf),
}

/// What a files pane shows: the local disk, or the SSH profile with this
/// ID (spec 0013, Model).
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Local,
    Ssh(String),
}

/// A target's session in the pane.
struct Session {
    /// Ties its prompts and its connections to it.
    id: u64,
    target: Target,
    /// The profile's name when it started, kept if the profile leaves the
    /// config (Model).
    name: SharedString,
    browser: Entity<Browser>,
    _events: Subscription,
    /// Answers its ssh's prompts; a new connection replaces it.
    askpass: Option<Task<()>>,
}

pub struct FilesPane {
    sessions: Vec<Session>,
    /// The session shown, an index into `sessions`.
    shown: usize,
    next_id: u64,
    /// For the profiles the header lists, and the browsers' settings.
    config: Config,
    /// The dialog over the pane while an ssh waits on a prompt, and whose.
    dialog: Option<(u64, Entity<PromptDialog>)>,
    /// The sessions' prompts take turns at the one dialog.
    dialog_turn: Rc<futures::lock::Mutex<()>>,
    first_frame_scheduled: bool,
}

impl EventEmitter<FilesEvent> for FilesPane {}

impl FilesPane {
    /// A pane showing `target` at `folder` (home if `None`). Without
    /// `connect` its session waits: a restored SSH target for Connect, a
    /// restored Local one for [`Self::start`] after the first frame (spec
    /// 0013, Saved). A `target` that's no SSH profile shows Local.
    pub fn new(
        target: Target,
        folder: Option<String>,
        connect: bool,
        config: &Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            sessions: Vec::new(),
            shown: 0,
            next_id: 0,
            config: config.clone(),
            dialog: None,
            dialog_turn: Rc::default(),
            first_frame_scheduled: false,
        };
        let shown = pane
            .add_session(target, folder.clone(), connect, window, cx)
            .or_else(|| pane.add_session(Target::Local, None, connect, window, cx));
        pane.shown = shown.unwrap_or_default();
        pane
    }

    /// Starts the shown session if it waits and it's Local: a restored
    /// pane's listing, after the first frame (spec 0013, Startup impact).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.get(self.shown) {
            let browser = session.browser.read(cx);
            if browser.is_local() && browser.is_idle() {
                session
                    .browser
                    .update(cx, |browser, cx| browser.connect(window, cx));
            }
        }
    }

    /// Takes a reloaded config: the header's profiles, and the settings
    /// every session follows.
    pub fn set_config(&mut self, config: &Config, cx: &mut Context<Self>) {
        self.config = config.clone();
        let settings = BrowserSettings::of(config);
        for session in &self.sessions {
            session
                .browser
                .update(cx, |browser, _| browser.set_settings(settings));
        }
        cx.notify();
    }

    /// Adds a session for `target`; `None` if it's no SSH profile.
    fn add_session(
        &mut self,
        target: Target,
        folder: Option<String>,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let (source, name) = match &target {
            Target::Local => (Source::Local, SharedString::from("Local")),
            Target::Ssh(profile) => {
                let profile = self.config.profile(profile)?;
                let ProfileKind::Ssh(ssh) = &profile.kind else {
                    return None;
                };
                let source = Source::Files {
                    pane: cx.entity().downgrade(),
                    id,
                    profile: ssh.clone(),
                };
                (source, profile.display_name().to_owned().into())
            }
        };
        let settings = BrowserSettings::of(&self.config);
        let menu = title_menu(cx.entity().downgrade());
        let browser = cx.new(|cx| {
            let mut browser = Browser::new(source, folder, false, settings, window, cx);
            browser.set_title_menu(menu);
            browser
        });
        let events = cx.subscribe_in(&browser, window, |_, _, event, _, cx| match event {
            BrowserEvent::Notice(notice) => cx.emit(FilesEvent::Notice(notice.clone())),
            BrowserEvent::Settled => cx.emit(FilesEvent::Settled),
            BrowserEvent::Run(file) => cx.emit(FilesEvent::Run(file.clone())),
            BrowserEvent::Close => {}
        });
        if connect {
            // Its SFTP connection asks this pane for its command, which
            // can't happen while the pane is being updated.
            let weak = browser.downgrade();
            cx.defer_in(window, move |_, window, cx| {
                let _ = weak.update(cx, |browser, cx| browser.connect(window, cx));
            });
        }
        self.sessions.push(Session {
            id,
            target,
            name,
            browser,
            _events: events,
            askpass: None,
        });
        Some(self.sessions.len() - 1)
    }

    /// Shows `target`'s session, or a new one at its home folder; the
    /// one shown before keeps its folder, tree, transfers and edits
    /// (spec 0013, flow 2).
    pub fn show(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self
            .sessions
            .iter()
            .position(|session| session.target == target);
        let Some(ix) = existing.or_else(|| self.add_session(target, None, true, window, cx)) else {
            return;
        };
        self.shown = ix;
        let focus = self.sessions[ix].browser.focus_handle(cx);
        window.focus(&focus, cx);
        cx.emit(FilesEvent::TitleChanged);
        cx.notify();
    }

    /// Ends session `id`'s ssh and forgets it, asking first if it has edits
    /// whose changes aren't on the host (spec 0013, flow 2). Local stays.
    fn disconnect(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.iter().find(|session| session.id == id) else {
            return;
        };
        let unsent: Vec<SharedString> = session
            .browser
            .read(cx)
            .open_edits()
            .into_iter()
            .filter(|(_, unsent)| *unsent)
            .map(|(name, _)| name)
            .collect();
        if unsent.is_empty() {
            self.end_session(id, window, cx);
            return;
        }
        let target = session.browser.read(cx).target();
        let pane = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let pane = pane.clone();
            dialog
                .title(format!("Disconnect from {target}?"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .text_sm()
                        .child("These edits have changes that aren't on the host, and are lost:")
                        .children(
                            unsent
                                .iter()
                                .map(|name| div().pl(px(8.)).child(name.clone())),
                        ),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.))
                        .child(
                            Button::new("disconnect-cancel")
                                .label("Cancel")
                                .small()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("disconnect-anyway")
                                .label("Disconnect")
                                .small()
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = pane.update(cx, |pane, cx| {
                                        pane.end_session(id, window, cx);
                                    });
                                }),
                        ),
                )
        });
    }

    /// Drops session `id`: its ssh ends and its edit copies go. The pane
    /// shows Local after.
    fn end_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.sessions.iter().position(|session| session.id == id) else {
            return;
        };
        if self.sessions[ix].target == Target::Local {
            return;
        }
        let shown = self.sessions[self.shown].id;
        self.sessions.remove(ix);
        if self.dialog.as_ref().is_some_and(|(asker, _)| *asker == id) {
            self.close_prompt(id, cx);
        }
        match self.sessions.iter().position(|session| session.id == shown) {
            Some(still) => self.shown = still,
            None => self.show(Target::Local, window, cx),
        }
        cx.emit(FilesEvent::TitleChanged);
        cx.notify();
    }

    /// What starts session `id`'s SFTP connection to `profile`: its own
    /// ssh, with a new askpass token whose prompts open this pane's dialog
    /// (spec 0013, flow 1). Without an askpass server ssh can't ask, so it
    /// runs in batch mode and fails rather than wait.
    pub fn sftp_command(
        &mut self,
        id: u64,
        profile: &SshProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Command {
        let ssh = crate::ssh::command(profile);
        let mut args = crate::ssh::sftp_args(&ssh.args).unwrap_or_default();
        let mut env = ssh.env.clone();
        self.close_prompt(id, cx);
        let task = match askpass::connect(cx) {
            Ok(connection) => {
                env.extend(connection.env);
                let turn = Rc::clone(&self.dialog_turn);
                let prompts = connection.prompts;
                Some(answer_prompts(prompts, ssh.args, id, turn, window, cx))
            }
            Err(_) => {
                args.splice(0..0, ["-o".to_owned(), "BatchMode=yes".to_owned()]);
                None
            }
        };
        if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) {
            session.askpass = task;
        }
        Command {
            program: ssh.program,
            args,
            env,
        }
    }

    fn shown(&self) -> Option<&Session> {
        self.sessions.get(self.shown)
    }

    /// The pane's title: what it shows (spec 0003's title rules apply to
    /// a host's name too).
    pub fn title(&self, cx: &App) -> String {
        let target = self
            .shown()
            .map(|session| session.browser.read(cx).target())
            .unwrap_or_default();
        format!("Files: {target}")
    }

    /// What the session saves of it (spec 0013, Saved): the target shown,
    /// as its profile ID (`None` for Local), and its folder.
    pub fn saved(&self, cx: &App) -> (Option<String>, Option<String>) {
        let Some(session) = self.shown() else {
            return (None, None);
        };
        let target = match &session.target {
            Target::Local => None,
            Target::Ssh(id) => Some(id.clone()),
        };
        let folder = session.browser.read(cx).folder().map(str::to_owned);
        (target, folder)
    }

    /// The local folder shown, where a split beside the pane starts.
    pub fn local_folder(&self, cx: &App) -> Option<PathBuf> {
        let browser = self.shown()?.browser.read(cx);
        browser
            .is_local()
            .then(|| browser.folder().map(PathBuf::from))
            .flatten()
    }

    /// Every session's edits a close would delete: each one's name, and
    /// whether its changes aren't on the host (spec 0013, flow 6).
    pub fn open_edits(&self, cx: &App) -> Vec<(SharedString, bool)> {
        self.sessions
            .iter()
            .flat_map(|session| session.browser.read(cx).open_edits())
            .collect()
    }

    /// Uploads every session's unsent edits, for a close that waits on
    /// them.
    pub fn upload_unsent(&self, cx: &mut Context<Self>) {
        for session in &self.sessions {
            session
                .browser
                .update(cx, |browser, cx| browser.upload_unsent(cx));
        }
    }

    /// Whether a close waiting on uploads can go on, as
    /// [`Browser::settled`] says, over every session.
    pub fn settled(&self, cx: &App) -> Option<bool> {
        let mut sent = true;
        for session in &self.sessions {
            sent &= session.browser.read(cx).settled()?;
        }
        Some(sent)
    }

    /// What takes the keyboard when the pane is focused: its prompt dialog
    /// while one is open, else the shown browser.
    pub fn focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        match &self.dialog {
            Some((_, dialog)) => Some(dialog.read(cx).focus_target(cx)),
            None => self.shown().map(|session| session.browser.focus_handle(cx)),
        }
    }

    /// The target switcher's lines (Model): Local, the saved SSH profiles
    /// by group as the host picker groups them, then sessions whose
    /// profile left the config, and Disconnect for a shown SSH session.
    /// The header's dropdown and the palette's `files.switchTarget` both
    /// list these.
    pub fn choices(&self) -> Vec<Choice> {
        let shown = self.shown().map(|session| session.target.clone());
        let show = |label: SharedString, group: &str, target: Target| Choice {
            current: shown.as_ref() == Some(&target),
            label,
            group: group.to_owned(),
            pick: Pick::Show(target),
        };
        let mut choices = vec![show("Local".into(), "Local", Target::Local)];
        let mut groups: BTreeMap<String, Vec<(SharedString, String)>> = BTreeMap::new();
        for profile in &self.config.profiles {
            if let ProfileKind::Ssh(ssh) = &profile.kind {
                let group = ssh.group.clone().unwrap_or_else(|| "Hosts".into());
                let label = profile.display_name().to_owned().into();
                groups
                    .entry(group)
                    .or_default()
                    .push((label, profile.id.clone()));
            }
        }
        let listed = |id: &str| groups.values().flatten().any(|(_, listed)| listed == id);
        let others: Vec<(SharedString, String)> = self
            .sessions
            .iter()
            .filter_map(|session| match &session.target {
                Target::Ssh(id) if !listed(id) => Some((session.name.clone(), id.clone())),
                _ => None,
            })
            .collect();
        for (group, hosts) in groups
            .into_iter()
            .chain((!others.is_empty()).then(|| ("Other".to_owned(), others)))
        {
            for (label, id) in hosts {
                choices.push(show(label, &group, Target::Ssh(id)));
            }
        }
        if let Some(session) = self
            .shown()
            .filter(|session| session.target != Target::Local)
        {
            choices.push(Choice {
                label: format!("Disconnect from {}", session.name).into(),
                group: "Disconnect".into(),
                pick: Pick::Disconnect(session.id),
                current: false,
            });
        }
        choices
    }

    /// Does what a target switcher line says.
    pub fn pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        match pick {
            Pick::Show(target) => self.show(target, window, cx),
            Pick::Disconnect(id) => self.disconnect(id, window, cx),
        }
    }

    /// The header's dropdown over [`Self::choices`], a separator and the
    /// group's name between groups.
    fn menu(&self, pane: &WeakEntity<Self>, mut menu: PopupMenu) -> PopupMenu {
        let mut group = None;
        for choice in self.choices() {
            if group.as_ref() != Some(&choice.group) {
                if group.is_some() {
                    menu = menu.separator();
                    if choice.group != "Disconnect" {
                        menu = menu.label(choice.group.clone());
                    }
                }
                group = Some(choice.group.clone());
            }
            let (pane, pick) = (pane.clone(), choice.pick);
            menu = menu.item(
                PopupMenuItem::new(choice.label)
                    .checked(choice.current)
                    .on_click(move |_, window, cx| {
                        let pick = pick.clone();
                        let _ = pane.update(cx, |pane, cx| pane.pick(pick, window, cx));
                    }),
            );
        }
        menu
    }
}

/// What picking a line of the target switcher does.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    Show(Target),
    /// End session `id`'s ssh.
    Disconnect(u64),
}

/// One line of the target switcher.
pub struct Choice {
    pub label: SharedString,
    pub group: String,
    pub pick: Pick,
    /// The target shown now.
    pub current: bool,
}

/// The title dropdown of every browser in the pane at `pane`.
fn title_menu(pane: WeakEntity<FilesPane>) -> TitleMenu {
    Rc::new(move |menu, _, cx| match pane.upgrade() {
        Some(entity) => entity.read(cx).menu(&pane, menu),
        None => menu,
    })
}

impl PromptHost for FilesPane {
    /// The session whose ssh asks.
    type Asker = u64;

    fn open_prompt(
        &mut self,
        prompt: &Prompt,
        remember: Option<Remember>,
        asker: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Answer> {
        let (answer, answered) = oneshot::channel();
        let name = self
            .sessions
            .iter()
            .find(|session| session.id == asker)
            .map(|session| session.name.clone())
            .unwrap_or_default();
        let connection = SharedString::from(format!("{name} (Files)"));
        let dialog = cx.new(|cx| {
            PromptDialog::new(
                connection,
                &prompt.text,
                prompt.kind,
                remember,
                answer,
                window,
                cx,
            )
        });
        self.dialog = Some((asker, dialog));
        cx.emit(FilesEvent::DialogChanged);
        cx.notify();
        answered
    }

    fn close_prompt(&mut self, asker: u64, cx: &mut Context<Self>) {
        if self
            .dialog
            .as_ref()
            .is_some_and(|(owner, _)| *owner == asker)
        {
            self.dialog = None;
            cx.emit(FilesEvent::DialogChanged);
            cx.notify();
        }
    }

    fn prompt_notice(&mut self, notice: PromptNotice, cx: &mut Context<Self>) {
        cx.emit(match notice {
            PromptNotice::Say(notice) => FilesEvent::Notice(notice),
            PromptNotice::NoKeychain => FilesEvent::NoKeychain,
        });
    }
}

impl Render for FilesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_scheduled {
            self.first_frame_scheduled = true;
            let pane = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = pane.update(cx, |_, cx| cx.emit(FilesEvent::FirstFrame));
            });
        }
        let background = cx.theme().tab_bar;
        div()
            .size_full()
            .relative()
            .bg(background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(FilesEvent::Focused)),
            )
            .when_some(self.shown(), |pane, session| {
                pane.child(session.browser.clone())
            })
            .children(self.dialog.as_ref().map(|(_, dialog)| dialog.clone()))
    }
}
