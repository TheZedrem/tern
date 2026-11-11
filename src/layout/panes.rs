//! Terminal panes: starting their programs, the settings and theme they
//! draw with, and their events.

use super::closing::Closing;
use super::tree::LeafId;
use super::{Workspace, show_notice};
use crate::config::{self, Profile};
use crate::secrets;
use crate::terminal::pty::{Command, GridSize};
use crate::terminal::{Launch, PaneEvent, PaneProfile, PaneSettings, Start, TerminalPane};
use crate::theme::{self, Theme};
use gpui::{AppContext, Context, Window};
use std::path::{Path, PathBuf};
use std::sync::Arc;

impl Workspace {
    #[allow(clippy::too_many_arguments, reason = "a pane's parts, used once each")]
    pub(super) fn add_pane(
        &mut self,
        leaf: LeafId,
        start: Start,
        launch: Launch,
        profile: PaneProfile,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (settings, bench) = (self.pane_settings(theme), self.bench);
        let pane =
            cx.new(|cx| TerminalPane::new(start, launch, profile, settings, bench, window, cx));
        let subscription = cx.subscribe_in(&pane, window, move |this, _, event, window, cx| {
            this.pane_event(leaf, event, window, cx);
        });
        self.panes.insert(leaf, pane);
        self.pane_subscriptions.insert(leaf, subscription);
    }

    /// What a pane with `theme` takes from the current config.
    pub(super) fn pane_settings(&self, theme: Theme) -> PaneSettings {
        PaneSettings {
            font: self.font.clone(),
            theme,
            close_on_exit: self.config.close_on_exit,
            scrollback: self.config.scrollback as usize,
            copy_on_select: self.config.copy_on_select,
            cursor: self.config.cursor,
            open_links: self.config.open_links,
        }
    }

    /// Starts `profile` for `leaf` in `cwd`, else the profile's own cwd,
    /// else the home directory (spec 0005, flow 2).
    pub(super) fn spawn_pane(
        &mut self,
        leaf: LeafId,
        profile: Profile,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launch = Launch {
            command: Command::of_profile(&profile),
            cwd: cwd.or_else(|| profile.cwd().map(Path::to_path_buf)),
            timing: Arc::clone(&self.timing),
        };
        let theme = self.theme_for(profile.theme.as_deref(), window, cx);
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.font.estimated_cell(),
        );
        let profile = PaneProfile::from(&profile);
        self.add_pane(leaf, Start::Now(grid), launch, profile, theme, window, cx);
    }

    /// The palette of a pane whose profile names theme `name` (`None`:
    /// `theme.default`). A name met for the first time is resolved now,
    /// which reads its theme file if it has one: a small read, next to the
    /// fork of the pane's program.
    pub(super) fn theme_for(
        &mut self,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Theme {
        let Some(name) = name else {
            return self.theme;
        };
        if let Some(theme) = self.themes.get(name) {
            return *theme;
        }
        let mut notices = Vec::new();
        let dir = config::themes_dir();
        let library = theme::Library::new(&self.config.theme.custom, dir.as_deref());
        let theme = library.resolve(theme::effective(name, self.light), &mut notices);
        self.themes.insert(name.to_owned(), theme);
        for notice in notices {
            show_notice(notice, window, cx);
        }
        theme
    }

    fn pane_event(
        &mut self,
        leaf: LeafId,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::FirstFrame => self.first_frame(window, cx),
            PaneEvent::Focused => {
                if self.tabs.focus(leaf) {
                    cx.notify();
                }
            }
            PaneEvent::TitleChanged => {
                self.save_soon(window, cx);
                cx.notify();
            }
            PaneEvent::CwdChanged => self.save_soon(window, cx),
            PaneEvent::Exited => self.close(Closing::Pane(leaf), window, cx),
            // The keyboard follows a prompt dialog of the focused pane in and
            // out, unless the palette or a rename has it.
            PaneEvent::DialogChanged => {
                if leaf == self.tabs.active().focused
                    && self.palette.is_none()
                    && self.rename.is_none()
                    && self.settings.is_none()
                {
                    self.focus_active(window, cx);
                }
                cx.notify();
            }
            PaneEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
            PaneEvent::NoKeychain => {
                if !std::mem::replace(&mut self.keychain_missing, true) {
                    show_notice(secrets::UNAVAILABLE.into(), window, cx);
                }
            }
            PaneEvent::ForwardPrompt { id, open } => {
                if let Some(forwards) = &self.forwards {
                    forwards.update(cx, |forwards, cx| forwards.prompting(*id, *open, cx));
                }
            }
            PaneEvent::ForwardDeclined(id) => {
                if let Some(forwards) = &self.forwards {
                    forwards.update(cx, |forwards, cx| forwards.cancelled(*id, cx));
                }
            }
        }
    }
}
