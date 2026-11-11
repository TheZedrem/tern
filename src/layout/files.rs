//! The workspace's files panes (spec 0013) and the SFTP sidebar of SSH
//! panes (spec 0008): adding them, their events, and the sidebar's drawing
//! and resizing.

use super::splits::{DIVIDER, DividerGhost};
use super::tree::{Axis, LeafId};
use super::{Workspace, show_notice};
use crate::commands::ToggleSftp;
use crate::commands::palette::{Entry, Palette};
use crate::commands::{OpenFiles, OpenFilesInNewTab, SwitchFilesTarget};
use crate::config::SftpSide;
use crate::files::browser::{Browser, BrowserEvent, BrowserSettings, Source};
use crate::files::local;
use crate::files::pane::{FilesEvent, FilesPane, Target};
use crate::secrets;
use crate::ssh;
use crate::terminal::pty::GridSize;
use crate::terminal::{Launch, PaneProfile, Start};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AppContext, Context, DragMoveEvent, Entity, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};
use gpui_component::ActiveTheme;
use std::path::Path;
use std::sync::Arc;

/// The SFTP sidebar's width until it's dragged, and the least it can be
/// dragged to; the most is half the window (spec 0008, Model).
pub(super) const SFTP_WIDTH: f32 = 300.0;
const SFTP_MIN_WIDTH: f32 = 200.0;

/// Dragging the SFTP sidebar's inner edge to resize it (spec 0008).
#[derive(Clone)]
struct DraggedSftpEdge;

/// An SSH pane's SFTP sidebar, and whether it's open. It outlives being
/// closed: the session goes on until the pane closes (spec 0008, Model).
pub(super) struct Sidebar {
    pub(super) view: Entity<Browser>,
    pub(super) open: bool,
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

    /// Opens or closes the active pane's SFTP sidebar, starting its session
    /// the first time (spec 0008, flow 1). A local pane has none.
    pub(super) fn toggle_sftp(
        &mut self,
        _: &ToggleSftp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaf = self.tabs.active().focused;
        if let Some(sidebar) = self.sftp.get_mut(&leaf) {
            sidebar.open = !sidebar.open;
            if !sidebar.open {
                self.focus_active(window, cx);
            }
            cx.notify();
            return;
        }
        let pane = self.panes.get(&leaf).cloned();
        let Some(pane) = pane.filter(|pane| pane.read(cx).ssh_target().is_some()) else {
            show_notice("SFTP needs an SSH pane".into(), window, cx);
            return;
        };
        let settings = BrowserSettings::of(&self.config);
        let source = Source::Pane(pane.downgrade());
        // The folder the pane's shell last reported, read once (spec 0008).
        let start = pane.read(cx).remote_cwd();
        let view = cx.new(|cx| Browser::new(source, start, true, settings, window, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                BrowserEvent::Close => {
                    if let Some(sidebar) = this.sftp.get_mut(&leaf) {
                        sidebar.open = false;
                    }
                    this.focus_active(window, cx);
                    cx.notify();
                }
                BrowserEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
                BrowserEvent::Settled => this.finish_closing(window, cx),
                // A remote file never runs.
                BrowserEvent::Run(_) => {}
            },
        );
        self.sftp.insert(
            leaf,
            Sidebar {
                view,
                open: true,
                _events: events,
            },
        );
        cx.notify();
    }

    /// The active pane's SFTP sidebar if it's open, with the inner edge it
    /// is resized by (spec 0008, Model).
    pub(super) fn render_sftp(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sidebar = self
            .sftp
            .get(&self.tabs.active().focused)
            .filter(|sidebar| sidebar.open)?;
        let theme = cx.theme();
        let (background, border) = (theme.tab_bar, theme.border);
        let edge = div()
            .id("sftp-edge")
            .w(px(DIVIDER))
            .h_full()
            .flex_shrink_0()
            .bg(border)
            .cursor_col_resize()
            .on_drag(DraggedSftpEdge, |_, _, _, cx| cx.new(|_| DividerGhost));
        let body = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(sidebar.view.clone());
        let side = self.config.sftp_sidebar;
        Some(
            div()
                .id("sftp")
                .w(px(self.sftp_width))
                .h_full()
                .flex_shrink_0()
                .flex()
                .bg(background)
                .map(|sidebar| match side {
                    SftpSide::Left => sidebar.child(body).child(edge),
                    SftpSide::Right => sidebar.child(edge).child(body),
                })
                .on_drag_move(cx.listener(
                    |this, event: &DragMoveEvent<DraggedSftpEdge>, window, cx| {
                        this.drag_sftp_edge(event, window, cx);
                    },
                ))
                .into_any_element(),
        )
    }

    /// Sets the SFTP sidebar's width from the mouse: from its outer edge,
    /// which stays put, between [`SFTP_MIN_WIDTH`] and half the window.
    fn drag_sftp_edge(
        &mut self,
        event: &DragMoveEvent<DraggedSftpEdge>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (bounds, x) = (event.bounds, event.event.position.x);
        let width = match self.config.sftp_sidebar {
            SftpSide::Left => x - bounds.left(),
            SftpSide::Right => bounds.right() - x,
        };
        let most = (f32::from(window.viewport_size().width) / 2.0).max(SFTP_MIN_WIDTH);
        self.sftp_width = f32::from(width).clamp(SFTP_MIN_WIDTH, most);
        cx.notify();
    }
}
