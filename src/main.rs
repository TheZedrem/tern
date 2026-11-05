//! Boot path (spec 0001), in this order: Wayland check, config and session,
//! PTY spawns for the shown tab, GPUI platform, window, first frame.
//! Everything else waits for the first frame. Anything added here needs a
//! TTFP measurement in its commit.

mod bench;
mod commands;
mod config;
mod import;
mod layout;
mod secrets;
mod session;
mod settings;
mod ssh;
mod terminal;
mod theme;
mod timing;

use bench::Mode;
use gpui::WindowDecorations;
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use terminal::FontSettings;
use terminal::pty::{self, Command, GridSize};
use timing::{Mark, Timing};

/// The window's size in logical pixels when no session remembers one.
const WINDOW_SIZE: (f32, f32) = (1000.0, 640.0);

/// A session the bench restores instead of none, to measure a restored
/// layout (spec 0003, AC7). The bench never reads or writes the user's.
const BENCH_SESSION_VAR: &str = "TERN_BENCH_SESSION";

fn main() {
    // ssh runs Tern as its askpass helper (spec 0006, flow 3), which
    // asks the running app and exits before any of the app's own code runs.
    if let Some(code) = ssh::askpass::helper() {
        std::process::exit(code);
    }
    // TTFP is measured from here: the check above reads two variables.
    let start = Instant::now();

    let mode = Mode::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("tern: {error}");
        std::process::exit(2);
    });
    match &mode {
        Mode::Runner { runs } => std::process::exit(bench::run(*runs)),
        Mode::Import { file, dry_run } => std::process::exit(import::run(file, *dry_run)),
        Mode::Normal | Mode::Once => {}
    }
    let bench = mode == Mode::Once;

    if let Err(message) = check_display(std::env::var_os("WAYLAND_DISPLAY").as_deref()) {
        eprintln!("{message}");
        std::process::exit(1);
    }

    let timing = Arc::new(Timing::new(start));
    let config::Loaded {
        config,
        notices: mut boot_notices,
    } = config::default_path()
        .map(|path| config::load(&path))
        .unwrap_or_else(|| config::Loaded {
            config: config::Config::default(),
            notices: vec![],
        });
    // The last session (spec 0003), so its shown tab starts with the window.
    // One that can't be restored is set aside as session.json.bad.
    let session_path = if bench { None } else { session::path() };
    let restore_from = if bench {
        std::env::var_os(BENCH_SESSION_VAR).map(PathBuf::from)
    } else {
        session_path.clone().filter(|_| config.restore_session)
    };
    // Set aside after the first frame: the boot path writes no files.
    let mut bad_session = None;
    let restored = restore_from.and_then(|path| match session::load(&path) {
        Ok(session) => session,
        Err(why) => {
            boot_notices.push(format!(
                "The last session couldn't be restored ({why}); it's kept as session.json.bad."
            ));
            bad_session = (!bench).then_some(path);
            None
        }
    });
    let area = restored
        .as_ref()
        .and_then(|session| session.window)
        .map_or(WINDOW_SIZE, |window| (window.width, window.height));
    // Resolved now, so the first frame has its colors. `system` is `dark`
    // until the OS is asked, after the first frame (spec 0005).
    let dir = config::themes_dir();
    let library = theme::Library::new(&config.theme.custom, dir.as_deref());
    let mut theme_notices = Vec::new();
    let mut resolve =
        |name: &str| library.resolve(theme::effective(name, false), &mut theme_notices);
    let theme = resolve(&config.theme.default);
    timing.mark(Mark::Config);

    // Each shown pane's program starts now and boots while GPUI comes up.
    // An SSH profile waits for Enter instead: no network at startup (spec
    // 0006, flow 6). The bench runs its own command in every pane.
    let cell = FontSettings::from_config(&config.font).estimated_cell();
    let mut start_pane = |profile: &config::Profile,
                          cwd: Option<PathBuf>,
                          area: (f32, f32),
                          title: Option<String>| {
        let launch = terminal::Launch {
            command: if bench {
                Command::bench()
            } else {
                Command::of_profile(profile)
            },
            cwd,
            timing: Arc::clone(&timing),
        };
        let start = if !bench && matches!(profile.kind, config::ProfileKind::Ssh(_)) {
            terminal::Start::OnEnter(terminal::connect_prompt(profile.display_name()))
        } else {
            terminal::Start::Started(pty::spawn(
                &launch.command,
                GridSize::fit(area, cell),
                launch.cwd.as_deref(),
                config.scrollback as usize,
                Arc::clone(&timing),
            ))
        };
        layout::BootPane {
            start,
            launch,
            profile: profile.into(),
            theme: profile.theme.as_deref().map_or(theme, &mut resolve),
            title,
        }
    };
    let mut restore_notices = Vec::new();
    let boot = restored.as_ref().and_then(|session| {
        layout::Boot::restore(session, area, |leaf, area| {
            let profile = layout::restored_profile(&config, &leaf.profile, &mut restore_notices);
            let cwd = leaf
                .cwd
                .clone()
                .or_else(|| profile.cwd().map(Path::to_path_buf));
            start_pane(&profile, cwd, area, leaf.title.clone())
        })
    });
    let boot = match boot {
        Some(boot) => boot,
        None => {
            let profile = config.default_profile();
            let cwd = profile.cwd().map(Path::to_path_buf);
            layout::Boot::fresh(start_pane(&profile, cwd, area, None))
        }
    }
    .setting_aside(bad_session);
    boot_notices.extend(theme_notices);
    boot_notices.extend(restore_notices);
    timing.mark(Mark::Pty);

    let app = gpui_platform::application().with_assets(gpui_kit_assets::Assets);
    timing.mark(Mark::Platform);

    app.run(move |cx: &mut App| {
        if let Err(error) = terminal::init(cx) {
            boot_notices.push(format!("The default font could not be loaded: {error}"));
        }
        let (font, font_notice) = FontSettings::checked(&config.font, cx);
        boot_notices.extend(font_notice);
        layout::init(cx);
        settings::init(cx);
        gpui_component::init(cx);
        // Everything bound so far stays when a reload rebinds the commands.
        commands::keymap::keep_library(cx);
        boot_notices.extend(commands::keymap::bind(
            &config.keybindings,
            config.new_tab,
            cx,
        ));
        theme::apply_to_widgets(&theme, cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(area.0), px(area.1)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Tern".into()),
                ..Default::default()
            }),
            app_id: Some("de.zedrem.tern".into()),
            // Dev only: draw our own title bar even where the compositor
            // would draw one, to check the GNOME path (spec 0003, flow 8).
            window_decorations: std::env::var_os("TERN_CLIENT_DECORATIONS")
                .map(|_| WindowDecorations::Client),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let workspace = cx.new(|cx| {
                layout::Workspace::new(
                    boot,
                    config,
                    font,
                    theme,
                    boot_notices,
                    session_path,
                    Arc::clone(&timing),
                    bench,
                    window,
                    cx,
                )
            });
            timing.mark(Mark::Window);
            cx.new(|cx| gpui_component::Root::new(workspace, window, cx))
        });
        if let Err(error) = opened {
            eprintln!("tern: could not open a window: {error:#}");
            std::process::exit(1);
        }
    });
}

/// On Linux, a Wayland session is required. Without one GPUI would start
/// headless and hang with no window (architecture §Key decisions).
fn check_display(wayland_display: Option<&OsStr>) -> Result<(), &'static str> {
    if cfg!(target_os = "linux") && wayland_display.is_none_or(OsStr::is_empty) {
        return Err("Tern needs a Wayland session (X11 is not supported)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_without_a_wayland_display_is_refused() {
        for display in [None, Some(OsStr::new(""))] {
            let refused = check_display(display).expect_err("started without Wayland");
            assert!(refused.contains("Wayland session"), "message: {refused}");
        }
    }

    #[test]
    fn a_wayland_display_is_accepted() {
        assert_eq!(check_display(Some(OsStr::new("wayland-1"))), Ok(()));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_platforms_need_no_wayland_display() {
        assert_eq!(check_display(None), Ok(()));
    }
}
