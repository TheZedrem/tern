//! Config reloads (spec 0005) and the settings screen (spec 0009): the
//! debounced reload off the UI thread, applying its result, and the
//! settings screen's edits.

use super::{Workspace, show_notice};
use crate::commands::palette::{Entry, Palette};
use crate::commands::{self, OpenConfig, OpenConfigFolder, OpenSettings, SelectTheme};
use crate::config::{self, Config};
use crate::files::browser::BrowserSettings;
use crate::settings::{self, Pick, SettingsEvent, SettingsView, change::Change};
use crate::terminal::FontSettings;
use crate::theme::{self, Theme};
use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Styled,
    Subscription, Window, WindowAppearance, div, px,
};
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

/// How long config changes must settle before a reload: editors save in
/// bursts (spec 0005, flow 3).
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);

/// The settings screen while it's open (spec 0009).
pub(super) struct SettingsScreen {
    pub(super) view: Entity<SettingsView>,
    pub(super) _events: Subscription,
}

/// The half of a reload that reads files, run on the background executor.
struct ReloadJob {
    /// The OS is in light mode, so `system` reads as `light`.
    light: bool,
    /// The theme names open panes use.
    pane_themes: Vec<String>,
}

/// A reloaded config with every theme it needs resolved.
struct Reloaded {
    config: Config,
    /// `theme.default`.
    theme: Theme,
    themes: HashMap<String, Theme>,
    notices: Vec<String>,
}

impl ReloadJob {
    fn run(self) -> Reloaded {
        let config::Loaded {
            config,
            mut notices,
        } = config::default_path()
            .map(|path| config::load(&path))
            .unwrap_or_else(|| config::Loaded {
                config: Config::default(),
                notices: vec![],
            });
        let (theme, themes) = {
            let dir = config::themes_dir();
            let library = theme::Library::new(&config.theme.custom, dir.as_deref());
            let mut resolve =
                |name: &str| library.resolve(theme::effective(name, self.light), &mut notices);
            let theme = resolve(&config.theme.default);
            let names: BTreeSet<String> = config
                .profiles
                .iter()
                .filter_map(|profile| profile.theme.clone())
                .chain(self.pane_themes)
                .collect();
            let themes = names
                .into_iter()
                .map(|name| {
                    let theme = resolve(&name);
                    (name, theme)
                })
                .collect();
            (theme, themes)
        };
        // Several names can lead to the same broken theme.
        let mut seen = std::collections::HashSet::new();
        notices.retain(|notice| seen.insert(notice.clone()));
        Reloaded {
            config,
            theme,
            themes,
            notices,
        }
    }
}

fn is_light(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Light | WindowAppearance::VibrantLight
    )
}

/// The change that sets `font.family`; `None` is the bundled font.
fn font_family(name: Option<&str>) -> Change {
    Change::Set {
        path: vec!["font", "family"],
        value: name.map_or_else(|| "null".to_owned(), settings::yaml::scalar),
        reads: |config| {
            config
                .font
                .family
                .as_deref()
                .map_or_else(|| "null".to_owned(), settings::yaml::scalar)
        },
    }
}

impl Workspace {
    /// Reloads the config whenever `config.yaml` or a theme file changes,
    /// and whenever the OS switches between light and dark while a theme in
    /// use is `system` (spec 0005). Starts after the first frame.
    pub(super) fn start_reloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (reload, mut reloads) = unbounded();
        if let Some(dir) = config::default_dir() {
            match config::Watcher::start(dir, reload.clone()) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(error) => show_notice(
                    format!("Config changes apply after a restart: {error}"),
                    window,
                    cx,
                ),
            }
        }
        self.light = is_light(window.appearance());
        if self.light && self.uses_system() {
            let _ = reload.unbounded_send(());
        }
        cx.observe_window_appearance(window, move |this, window, _| {
            let light = is_light(window.appearance());
            if light != this.light {
                this.light = light;
                if this.uses_system() {
                    let _ = reload.unbounded_send(());
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            while reloads.next().await.is_some() {
                cx.background_executor().timer(RELOAD_DEBOUNCE).await;
                while reloads.try_recv().is_ok() {}
                let Ok(job) = this.update(cx, |this, _| this.reload_job()) else {
                    break;
                };
                let reloaded = cx.background_spawn(async move { job.run() }).await;
                let applied = this.update_in(cx, |this, window, cx| {
                    this.apply_reload(reloaded, window, cx);
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Whether `system` decides any theme on screen.
    fn uses_system(&self) -> bool {
        self.config.theme.default == "system" || self.themes.contains_key("system")
    }

    /// What a reload reads: the config file, then every theme the new
    /// config's profiles and the open panes name.
    fn reload_job(&self) -> ReloadJob {
        ReloadJob {
            light: self.light,
            pane_themes: self.themes.keys().cloned().collect(),
        }
    }

    /// Swaps in a reloaded config: font, themes, scrollback and
    /// `close_on_exit` change in every open pane, while profiles apply to
    /// panes opened from now on (spec 0005, flow 3).
    fn apply_reload(&mut self, reloaded: Reloaded, window: &mut Window, cx: &mut Context<Self>) {
        let Reloaded {
            config,
            theme,
            themes,
            mut notices,
        } = reloaded;
        if let Some(watcher) = &mut self.watcher {
            watcher.arm();
        }
        let (font, font_notice) = FontSettings::checked(&config.font, cx);
        notices.extend(font_notice);
        notices.extend(commands::keymap::bind(
            &config.keybindings,
            config.new_tab,
            cx,
        ));
        self.config = config;
        self.configured_font = font;
        self.font = self.configured_font.zoomed(self.zoom);
        self.themes = themes;
        // While the theme picker previews, the new theme waits for its Esc.
        match self
            .palette
            .as_mut()
            .and_then(|open| open.saved_theme.as_mut())
        {
            Some(saved) => *saved = theme,
            None => self.theme = theme,
        }
        self.apply_settings(cx);
        self.plugins_reloaded(cx);
        if let Some(screen) = &self.settings {
            let config = self.config.clone();
            screen
                .view
                .update(cx, |view, cx| view.set_config(config, window, cx));
        }
        for notice in notices {
            show_notice(notice, window, cx);
        }
        self.follow_update_check(window, cx);
    }

    /// Hands the current font, themes and config to the widgets and every
    /// pane.
    pub(super) fn apply_settings(&mut self, cx: &mut Context<Self>) {
        theme::apply_to_widgets(&self.theme, cx);
        let settings = BrowserSettings::of(&self.config);
        for files in self
            .sftp
            .values()
            .filter_map(|sidebar| sidebar.files.as_ref())
        {
            files.view.update(cx, |view, _| view.set_settings(settings));
        }
        for files in self.files.values() {
            let config = &self.config;
            files
                .pane
                .update(cx, |pane, cx| pane.set_config(config, cx));
        }
        for pane in self.panes.values() {
            let name = pane.read(cx).profile().theme.clone();
            let theme = name
                .and_then(|name| self.themes.get(&name).copied())
                .unwrap_or(self.theme);
            let settings = self.pane_settings(theme);
            pane.update(cx, |pane, cx| pane.apply(settings, cx));
        }
        cx.notify();
    }

    /// Opens `config.yaml` with the OS's default handler, first writing the
    /// commented defaults if there is no config yet (spec 0005).
    pub(super) fn open_config(
        &mut self,
        _: &OpenConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_config_file(App::open_with_system, window, cx);
    }

    /// Shows `config.yaml` in the OS's file manager, creating it first like
    /// "Open config file" does (spec 0009).
    pub(super) fn open_config_folder(
        &mut self,
        _: &OpenConfigFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_config_file(App::reveal_path, window, cx);
    }

    /// Makes sure `config.yaml` exists, off the UI thread, then hands its
    /// path to `open`.
    fn with_config_file(
        &mut self,
        open: fn(&App, &std::path::Path),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = config::default_path() else {
            show_notice("This system has no config directory.".into(), window, cx);
            return;
        };
        let created = cx.background_spawn({
            let path = path.clone();
            async move { config::ensure_exists(&path) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = created.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    if let Some(watcher) = &mut this.watcher {
                        watcher.arm();
                    }
                    open(cx, &path);
                }
                Err(error) => show_notice(
                    format!("config.yaml could not be created: {error}"),
                    window,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Opens the settings screen over the window (spec 0009, flow 1).
    pub(super) fn open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_none() {
            let config = self.config.clone();
            let view = cx.new(|cx| SettingsView::new(config, window, cx));
            let events = cx.subscribe_in(&view, window, Self::settings_event);
            self.settings = Some(SettingsScreen {
                view,
                _events: events,
            });
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn settings_event(
        &mut self,
        _: &Entity<SettingsView>,
        event: &SettingsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsEvent::Change(change) => self.write_change(change.clone(), window, cx),
            SettingsEvent::Pick(pick) => self.pick_setting(*pick, window, cx),
            // An import may have created the config directory.
            SettingsEvent::Written => {
                if let Some(watcher) = &mut self.watcher {
                    watcher.arm();
                }
            }
            SettingsEvent::Close => {
                self.settings = None;
                self.focus_active(window, cx);
                cx.notify();
            }
        }
    }

    /// Writes `change` into `config.yaml` off the UI thread; the watcher's
    /// reload applies it (spec 0009, flow 2). A failure says why.
    pub(super) fn write_change(
        &mut self,
        change: Change,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = config::default_path() else {
            show_notice("This system has no config directory.".into(), window, cx);
            return;
        };
        let saved = cx
            .background_spawn(async move { config::save_edit(&path, |text| change.edited(text)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = saved.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Saving may have created the config directory.
                if let Some(watcher) = &mut this.watcher {
                    watcher.arm();
                }
                if let Err(error) = result {
                    show_notice(error, window, cx);
                }
            });
        })
        .detach();
    }

    /// Opens the palette as a picker for a setting the screen lists
    /// (spec 0009): fonts and default profiles here, themes as the theme
    /// picker does.
    fn pick_setting(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let (entries, placeholder): (Vec<Entry>, &str) = match pick {
            Pick::Theme => return self.select_theme(&SelectTheme, window, cx),
            Pick::Font => {
                let current = self.config.font.family.clone();
                let mut names = cx.text_system().all_font_names();
                names.sort();
                names.dedup();
                let bundled = Entry::Setting {
                    title: format!("{} (bundled)", crate::terminal::DEFAULT_FONT_FAMILY),
                    current: current.is_none(),
                    change: font_family(None),
                };
                let installed = names.into_iter().map(|name| Entry::Setting {
                    current: current.as_deref() == Some(name.as_str()),
                    change: font_family(Some(&name)),
                    title: name,
                });
                (
                    std::iter::once(bundled).chain(installed).collect(),
                    "Pick a font",
                )
            }
            Pick::DefaultProfile => {
                let current = self.config.default_profile().id;
                let builtin_shell = !self.config.configures(config::SHELL);
                let entries = self
                    .profiles()
                    .into_iter()
                    .map(|profile| Entry::Setting {
                        title: if builtin_shell && profile.id == config::SHELL {
                            format!("{} (built-in)", profile.display_name())
                        } else {
                            profile.display_name().to_owned()
                        },
                        current: profile.id == current,
                        change: Change::Set {
                            path: vec!["default_profile"],
                            value: settings::yaml::scalar(&profile.id),
                            reads: |config| settings::yaml::scalar(&config.default_profile().id),
                        },
                    })
                    .collect();
                (entries, "Pick the profile new tabs open")
            }
        };
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, placeholder, None, window, cx)
        });
    }

    /// The settings screen, centered over the tab content.
    pub(super) fn render_settings(&self) -> Option<impl IntoElement> {
        let screen = self.settings.as_ref()?;
        Some(
            div()
                .id("settings-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.3))
                // The panes below get no clicks, which would take the
                // keyboard from the screen's fields.
                .occlude()
                .child(
                    div()
                        .w(gpui::relative(0.9))
                        .max_w(px(760.))
                        .h(gpui::relative(0.9))
                        .child(screen.view.clone()),
                ),
        )
    }
}
