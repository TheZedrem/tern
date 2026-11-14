//! The palette (spec 0004) and what it picks: profiles and hosts to open
//! or split with (spec 0007, flow 6), hosts to forget, and themes.

use super::tree::{Axis, Rect};
use super::{Workspace, show_notice};
use crate::commands::palette::{Entry, Palette, PaletteEvent};
use crate::commands::{
    self, ConnectToHost, ForgetPassword, NewTabWithProfile, OpenPalette, SelectTheme,
};
use crate::config::{self, Profile, ProfileKind};
use crate::secrets;
use crate::ssh;
use crate::theme::{self, Theme};
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Styled,
    Subscription, Window, div, px,
};
use gpui_component::ActiveTheme;

/// The split picker's group of local profiles (spec 0007, flow 6).
const LOCAL_GROUP: &str = "Local";

/// The palette while it's open.
pub(super) struct OpenPaletteView {
    pub(super) view: Entity<Palette>,
    pub(super) _events: Subscription,
    /// The default theme as saved, while the theme picker previews others.
    pub(super) saved_theme: Option<Theme>,
}

/// The widest the palette gets, in logical pixels.
const PALETTE_WIDTH: f32 = 560.0;

impl Workspace {
    /// Opens the command palette over the focused pane, or closes it if
    /// it's open (spec 0004).
    pub(super) fn open_palette(
        &mut self,
        _: &OpenPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.close_palette(false, window, cx);
            return;
        }
        let commands = commands::COMMANDS.iter().filter_map(|command| {
            Some(Entry::Command {
                command,
                title: command.title?,
                enabled: self.can_run(command.id, window, cx),
            })
        });
        let profiles = self.profiles().into_iter().map(|profile| Entry::Profile {
            title: format!("Open profile: {}", profile.display_name()),
            id: profile.id,
        });
        // The enabled plugins' commands (spec 0016, Commands).
        let plugin_commands = self
            .plugin_host()
            .map(|host| host.read(cx).commands())
            .unwrap_or_default()
            .into_iter()
            .map(|listed| Entry::Plugin {
                title: listed.title,
                command: commands::PluginCommand {
                    plugin: listed.plugin,
                    command: listed.command,
                },
            });
        let entries = commands.chain(plugin_commands).chain(profiles).collect();
        let quick = self.forward_shortcut(cx);
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, "Run a command or open a profile", None, window, cx)
                .with_quick(quick)
        });
    }

    /// Opens the host picker to connect to a host (spec 0006, flow 1).
    pub(super) fn connect_to_host(
        &mut self,
        _: &ConnectToHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_host(Vec::new(), Entry::Host, "Connect to a host", window, cx);
    }

    /// Opens the host picker to forget what's saved for a host (spec 0006,
    /// flow 7).
    pub(super) fn forget_password(
        &mut self,
        _: &ForgetPassword,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_host(
            Vec::new(),
            Entry::ForgetHost,
            "Forget the password saved for a host",
            window,
            cx,
        );
    }

    /// Opens the host picker, with the local profiles first, to split the
    /// focused pane along `axis` with the one picked (spec 0007, flow 6).
    /// Without room for a split it doesn't open.
    pub(super) fn pick_split_profile(
        &mut self,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_room(axis, window) {
            return;
        }
        let placeholder = match axis {
            Axis::Row => "Split right with a profile or host",
            Axis::Column => "Split down with a profile or host",
        };
        let entry = move |host| Entry::SplitWith { host, axis };
        self.pick_host(self.local_hosts(), entry, placeholder, window, cx);
    }

    /// Asks which profile or host a new tab opens, the default profile
    /// first so Enter opens it (spec 0004, `Ctrl+Shift+T`).
    pub(super) fn new_tab_with_profile(
        &mut self,
        _: &NewTabWithProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.local_hosts();
        self.pick_host(
            local,
            Entry::Host,
            "Open a profile or host in a new tab",
            window,
            cx,
        );
    }

    /// The local profiles as picker lines in group "Local", the default
    /// profile first.
    fn local_hosts(&self) -> Vec<ssh::Host> {
        let default = self.config.default_profile().id;
        let mut local: Vec<ssh::Host> = self
            .profiles()
            .into_iter()
            .filter(|profile| matches!(profile.kind, ProfileKind::Local(_)))
            .map(|profile| ssh::Host {
                label: profile.display_name().to_owned(),
                group: LOCAL_GROUP.into(),
                destination: String::new(),
                id: profile.id,
            })
            .collect();
        local.sort_by_key(|host| host.id != default);
        local
    }

    /// Opens profile or host `id` in a new tab.
    pub(super) fn open_profile_tab(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = self.config.profile_or_default(id);
        let leaf = self.tabs.new_tab();
        self.spawn_pane(leaf, profile, None, window, cx);
        self.save_soon(window, cx);
    }

    /// Splits the focused pane along `axis` with profile or host `id`.
    fn split_with(&mut self, id: &str, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.config.profile_or_default(id);
        self.split(axis, Some(profile), window, cx);
    }

    /// Opens the host picker over `first`, then the SSH profiles and the
    /// hosts of `~/.ssh/config`, which is read each time, off the UI thread,
    /// so its edits show. `entry` says what picking a host does. A picker
    /// over local profiles that would list only the default profile runs it
    /// without asking (spec 0004).
    fn pick_host(
        &mut self,
        first: Vec<ssh::Host>,
        entry: impl Fn(ssh::Host) -> Entry + 'static,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profiles = self.config.profiles.clone();
        let only_default = first.len() == 1;
        let found = cx.background_spawn(async move {
            let path = ssh::config_path().unwrap_or_default();
            ssh::hosts(&profiles, &path)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (hosts, notices) = found.await;
            let _ = this.update_in(cx, |this, window, cx| {
                for notice in notices {
                    show_notice(notice, window, cx);
                }
                let mut entries: Vec<Entry> = first.into_iter().chain(hosts).map(entry).collect();
                if only_default && entries.len() == 1 {
                    match entries.pop() {
                        Some(Entry::Host(host)) => this.open_profile_tab(&host.id, window, cx),
                        Some(Entry::SplitWith { host, axis }) => {
                            this.split_with(&host.id, axis, window, cx)
                        }
                        _ => {}
                    }
                    return;
                }
                this.show_palette(None, window, cx, |window, cx| {
                    Palette::new(entries, placeholder, None, window, cx).with_empty(
                        "No hosts yet: add `type: ssh` profiles to config.yaml, \
                         or hosts to ~/.ssh/config",
                    )
                });
            });
        })
        .detach();
    }

    /// Forgets the password and key passphrases saved for host `id`, with
    /// a notice saying what was forgotten. Only this asks ssh where the host
    /// connects, besides connecting (TM-askpass-20), and it runs off the UI
    /// thread.
    fn forget_host(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.config.profile(id) else {
            return;
        };
        let ProfileKind::Ssh(remote) = &profile.kind else {
            return;
        };
        let args = ssh::command(remote).args;
        let label = profile.display_name().to_owned();
        let forgotten = secrets::off_thread(move || ssh::forget(&args));
        cx.spawn_in(window, async move |this, cx| {
            let notice = match forgotten.await {
                Ok(Ok(keys)) if keys.is_empty() => format!("Nothing was saved for {label}."),
                Ok(Ok(keys)) => {
                    let described: Vec<String> = keys.iter().map(secrets::Key::describe).collect();
                    format!("Forgot {}.", described.join(", "))
                }
                Ok(Err(error)) => format!("Nothing was forgotten for {label}: {error}"),
                Err(_) => format!("Nothing was forgotten for {label}."),
            };
            let _ = this.update_in(cx, |_, window, cx| show_notice(notice, window, cx));
        })
        .detach();
    }

    /// The profiles the palette offers: `shell` and the configured ones
    /// (spec 0005).
    pub(super) fn profiles(&self) -> Vec<Profile> {
        self.config.all_profiles()
    }

    /// Opens the theme picker: moving through it previews each theme, Enter
    /// saves one as `theme.default`, Esc goes back (spec 0005, flow 4).
    pub(super) fn select_theme(
        &mut self,
        _: &SelectTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = config::themes_dir();
        let mut notices = Vec::new();
        let names = theme::list(&self.config.theme.custom, dir.as_deref(), &mut notices);
        for notice in notices {
            show_notice(notice, window, cx);
        }
        let current = self.config.theme.default.clone();
        let highlight = names.iter().position(|name| *name == current);
        let entries = names
            .into_iter()
            .map(|name| Entry::Theme {
                current: name == current,
                name,
            })
            .collect();
        self.show_palette(Some(self.theme), window, cx, |window, cx| {
            Palette::new(entries, "Select a theme", highlight, window, cx)
        });
    }

    /// Shows a palette, replacing one that's open: a command in the
    /// palette can open the theme picker.
    /// `saved_theme` is the theme to go back to when the palette previews
    /// themes.
    pub(super) fn show_palette(
        &mut self,
        saved_theme: Option<Theme>,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(&mut Window, &mut Context<Palette>) -> Palette,
    ) {
        if let Some(saved) = self.palette.take().and_then(|open| open.saved_theme) {
            self.theme = saved;
            self.apply_settings(cx);
        }
        let view = cx.new(|cx| build(window, cx));
        let events = cx.subscribe_in(&view, window, Self::palette_event);
        self.palette = Some(OpenPaletteView {
            view,
            _events: events,
            saved_theme,
        });
        cx.notify();
    }

    fn palette_event(
        &mut self,
        palette: &Entity<Palette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .palette
            .as_ref()
            .is_none_or(|open| open.view != *palette)
        {
            return;
        }
        match event {
            PaletteEvent::OpenProfile(id) => self.open_profile_tab(id, window, cx),
            PaletteEvent::ForgetHost(id) => self.forget_host(id, window, cx),
            PaletteEvent::SplitWith { id, axis } => self.split_with(id, *axis, window, cx),
            PaletteEvent::PreviewTheme(name) => {
                self.theme = self.theme_for(Some(name), window, cx);
                self.apply_settings(cx);
            }
            PaletteEvent::Change(change) => self.write_change(change.clone(), window, cx),
            PaletteEvent::Files(pick) => {
                let focused = self.tabs.active().focused;
                if let Some(files) = self.files.get(&focused) {
                    let pick = pick.clone();
                    files
                        .pane
                        .update(cx, |pane, cx| pane.pick(pick, window, cx));
                }
            }
            PaletteEvent::AddForward(forward) => self.add_forward(forward.clone(), window, cx),
            PaletteEvent::StopForward(id) => {
                if let Some(forwards) = &self.forwards {
                    forwards.update(cx, |forwards, cx| forwards.remove(*id, cx));
                }
            }
            PaletteEvent::ChooseTheme(name) => {
                if let Some(open) = &mut self.palette {
                    open.saved_theme = None;
                }
                self.save_theme(name.clone(), window, cx);
            }
            PaletteEvent::Closed { cancelled } => self.close_palette(*cancelled, window, cx),
        }
    }

    /// Closes the palette and gives the focus back to the active pane.
    /// Leaving the theme picker without choosing restores the saved theme.
    fn close_palette(&mut self, cancelled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(saved) = self.palette.take().and_then(|open| open.saved_theme)
            && cancelled
        {
            self.theme = saved;
            self.apply_settings(cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Writes `name` as `theme.default` off the UI thread. The watcher
    /// then reloads the config, which applies it for good.
    fn save_theme(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = config::default_path() else {
            show_notice("This system has no config directory.".into(), window, cx);
            return;
        };
        let saved = cx.background_spawn(async move { config::save_theme_default(&path, &name) });
        cx.spawn_in(window, async move |this, cx| {
            let result = saved.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Saving may have created the config directory.
                if let Some(watcher) = &mut this.watcher {
                    watcher.arm();
                }
                if let Err(error) = result {
                    show_notice(format!("The theme wasn't saved: {error}"), window, cx);
                }
            });
        })
        .detach();
    }

    /// The open palette, centered over the focused pane near its top (spec
    /// 0004, flow 2). A click outside it leaves it, like Esc.
    pub(super) fn render_palette(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let open = self.palette.as_ref()?;
        let size = self.content_size.get()?;
        let tab = self.tabs.active();
        let pane = match tab.zoomed {
            Some(_) => Rect::UNIT,
            None => tab
                .root
                .rects(Rect::UNIT)
                .into_iter()
                .find(|(leaf, _)| *leaf == tab.focused)
                .map_or(Rect::UNIT, |(_, rect)| rect),
        };
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let pane_width = pane.width * width;
        let palette_width = PALETTE_WIDTH.min(pane_width - 16.0).max(240.0);
        let left = (pane.x * width + (pane_width - palette_width) / 2.0).max(0.0);
        let top = pane.y * height + (pane.height * height * 0.12).min(80.0);
        let (border, background) = (cx.theme().border, cx.theme().popover);
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(palette_width))
                .rounded(px(8.))
                .border_1()
                .border_color(border)
                .bg(background)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.close_palette(true, window, cx);
                }))
                .child(open.view.clone()),
        )
    }
}
