//! The workspace's files panes (spec 0013): adding them and their events.
//! Also the views the sidebar draws for a terminal pane (spec 0018): its
//! own file browser (an SSH pane's host, spec 0008, or a local pane's
//! disk) and an SSH pane's port forwards (spec 0017, `super::forwards`),
//! kept per pane here and shown by `super::sidebar`.

use super::tree::{Axis, LeafId};
use super::{Workspace, show_notice};
use crate::commands::palette::{Entry, Palette};
use crate::commands::{OpenFiles, OpenFilesInNewTab, SwitchFilesTarget};
use crate::files::browser::{Browser, BrowserEvent, BrowserSettings, Source};
use crate::files::local;
use crate::files::pane::{FilesEvent, FilesPane, Target};
use crate::forwards::view::ForwardsView;
use crate::secrets;
use crate::ssh;
use crate::terminal::pty::GridSize;
use crate::terminal::{Launch, PaneProfile, Start, TerminalPane};
use gpui::{AppContext, Context, Entity, Subscription, Window};
use std::path::Path;
use std::sync::Arc;

/// What the sidebar draws for one terminal pane. It outlives the sidebar
/// showing another pane or closing: an SFTP session goes on until the
/// pane closes (spec 0008, Model). Each view is made the first time it's
/// shown, so showing forwards or a local disk connects nothing.
#[derive(Default)]
pub(super) struct PaneViews {
    pub(super) files: Option<SidebarView<Browser>>,
    pub(super) forwards: Option<SidebarView<ForwardsView>>,
}

/// One of a pane's views in the sidebar, and its events.
pub(super) struct SidebarView<V> {
    pub(super) view: Entity<V>,
    pub(super) _events: Subscription,
}

/// A files pane of the split tree (spec 0013), and its events.
pub(super) struct FilesLeaf {
    pub(super) pane: Entity<FilesPane>,
    pub(super) _events: Subscription,
}

impl Workspace {
    /// Puts a files pane on `target` at `folder` in `leaf` (spec 0013).
    /// Without `connect` it waits: see [`FilesPane::new`].
    pub(super) fn add_files(
        &mut self,
        leaf: LeafId,
        target: Target,
        folder: Option<String>,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config = &self.config;
        let pane = cx.new(|cx| FilesPane::new(target, folder, connect, config, window, cx));
        let events = cx.subscribe_in(&pane, window, move |this, _, event, window, cx| {
            this.files_event(leaf, event, window, cx);
        });
        self.files.insert(
            leaf,
            FilesLeaf {
                pane,
                _events: events,
            },
        );
    }

    fn files_event(
        &mut self,
        leaf: LeafId,
        event: &FilesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A tab with a terminal waits for its output (spec 0001).
            FilesEvent::FirstFrame => {
                if self.panes.is_empty() {
                    self.first_frame(window, cx);
                }
            }
            FilesEvent::Focused => {
                if self.tabs.focus(leaf) {
                    cx.notify();
                }
            }
            FilesEvent::TitleChanged => {
                self.save_soon(window, cx);
                cx.notify();
            }
            FilesEvent::DialogChanged => {
                if leaf == self.tabs.active().focused
                    && self.palette.is_none()
                    && self.rename.is_none()
                    && self.settings.is_none()
                {
                    self.focus_active(window, cx);
                }
                cx.notify();
            }
            FilesEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
            FilesEvent::NoKeychain => {
                if !std::mem::replace(&mut self.keychain_missing, true) {
                    show_notice(secrets::UNAVAILABLE.into(), window, cx);
                }
            }
            FilesEvent::Settled => self.finish_closing(window, cx),
            FilesEvent::Run(file) => self.run_file(leaf, file, window, cx),
        }
    }

    /// Splits files pane `leaf` to the right with a local pane running
    /// `file` in its folder, so its output and exit show (spec 0013, flow
    /// 4). Without room, it says so and runs nothing.
    fn run_file(&mut self, leaf: LeafId, file: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.active().root.contains(leaf) {
            return;
        }
        self.tabs.focus(leaf);
        if !self.has_room(Axis::Row, window) {
            show_notice(
                "There's no room beside the files pane to run it".into(),
                window,
                cx,
            );
            return;
        }
        let name = file.file_name().map_or_else(
            || file.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let launch = Launch {
            command: local::run_command(file),
            cwd: file.parent().map(Path::to_path_buf),
            timing: Arc::clone(&self.timing),
        };
        let theme = self.theme;
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.font.estimated_cell(),
        );
        // Saved as the default profile in the file's folder: a restore
        // opens a shell there, never runs the file again.
        let profile = PaneProfile {
            id: self.config.default_profile().id,
            name,
            theme: None,
            ssh: false,
            stays: true,
        };
        let new = self.tabs.split(Axis::Row);
        self.add_pane(new, Start::Now(grid), launch, profile, theme, window, cx);
        self.show_active(window, cx);
    }

    /// Opens a files pane (spec 0013, flow 1): the tab's own if it has one;
    /// else beside the focused pane, on its target: an SSH pane's host, a
    /// local pane's folder. Without room, it says so.
    pub(super) fn open_files(
        &mut self,
        _: &OpenFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tabs.active();
        if let Some(leaf) = tab
            .root
            .leaves()
            .into_iter()
            .find(|leaf| self.files.contains_key(leaf))
        {
            self.tabs.focus(leaf);
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let focused = tab.focused;
        let (target, folder) = match self.panes.get(&focused) {
            Some(pane) => {
                let pane = pane.read(cx);
                let profile = pane.profile();
                if profile.ssh {
                    (Target::Ssh(profile.id.clone()), None)
                } else {
                    let folder = pane.cwd().and_then(|cwd| cwd.to_str().map(str::to_owned));
                    (Target::Local, folder)
                }
            }
            None => return self.open_files_in_new_tab(&OpenFilesInNewTab, window, cx),
        };
        if !self.has_room(Axis::Row, window) {
            show_notice("There's no room for a files pane here".into(), window, cx);
            return;
        }
        let leaf = self.tabs.split(Axis::Row);
        self.add_files(leaf, target, folder, true, window, cx);
        self.show_active(window, cx);
    }

    /// Lists the focused files pane's targets in the palette, so the
    /// keyboard switches them as the header's dropdown does (spec 0013,
    /// flow 2).
    pub(super) fn switch_files_target(
        &mut self,
        _: &SwitchFilesTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(files) = self.files.get(&self.tabs.active().focused) else {
            return;
        };
        let entries = files
            .pane
            .read(cx)
            .choices()
            .into_iter()
            .map(|choice| Entry::Files {
                host: ssh::Host {
                    id: String::new(),
                    label: choice.label.to_string(),
                    group: choice.group,
                    destination: String::new(),
                },
                pick: choice.pick,
                current: choice.current,
            })
            .collect();
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, "Show Local or a host", None, window, cx)
        });
    }

    /// A tab holding one files pane on Local at home (spec 0013, flow 1).
    pub(super) fn open_files_in_new_tab(
        &mut self,
        _: &OpenFilesInNewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaf = self.tabs.new_tab();
        self.add_files(leaf, Target::Local, None, true, window, cx);
        self.show_active(window, cx);
    }

    /// The sidebar's file browser for pane `leaf` (spec 0018, Files
    /// panel): an SSH pane's host, which connects now, or the Local
    /// target for a local pane, which connects nothing.
    pub(super) fn sidebar_files(
        &mut self,
        leaf: LeafId,
        pane: &Entity<TerminalPane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SidebarView<Browser> {
        let settings = BrowserSettings::of(&self.config);
        // The folder the pane's shell last reported, read once (spec
        // 0008): it doesn't follow later `cd`s.
        let (source, start) = if pane.read(cx).ssh_target().is_some() {
            (Source::Pane(pane.downgrade()), pane.read(cx).remote_cwd())
        } else {
            let folder = pane.read(cx).local_folder();
            let folder = folder.and_then(|cwd| cwd.to_str().map(str::to_owned));
            (Source::Local, folder)
        };
        let view = cx.new(|cx| Browser::new(source, start, true, settings, window, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                BrowserEvent::Leave => this.focus_active(window, cx),
                BrowserEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
                BrowserEvent::Settled => this.finish_closing(window, cx),
                // Only a local file runs, in a pane beside this one.
                BrowserEvent::Run(file) => this.run_file(leaf, file, window, cx),
            },
        );
        SidebarView {
            view,
            _events: events,
        }
    }
}
