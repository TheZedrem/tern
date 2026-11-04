//! Boot path (spec 0001), in this order: Wayland check, config, PTY spawn,
//! GPUI platform, window, first frame. Everything else waits for the first
//! frame. Anything added here needs a TTFP measurement in its commit.

mod bench;
mod commands;
mod config;
mod layout;
mod ssh;
mod terminal;
mod theme;
mod timing;

use bench::Mode;
use gpui::WindowDecorations;
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use terminal::FontSettings;
use terminal::pty::{self, Command, GridSize};
use timing::{Mark, Timing};

/// Initial window size in logical pixels, until session restore (T-006)
/// remembers the last one.
const WINDOW_SIZE: (f32, f32) = (1000.0, 640.0);

fn main() {
    // Taken first: TTFP is measured from here.
    let start = Instant::now();

    let mode = Mode::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("zed-term: {error}");
        std::process::exit(2);
    });
    if let Mode::Runner { runs } = mode {
        std::process::exit(bench::run(runs));
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
    // Resolved now, so the first frame has its colors. `system` is `dark`
    // until the OS is asked, after the first frame (spec 0005).
    let profile = config.default_profile();
    let (theme, pane_theme) = {
        let dir = config::themes_dir();
        let library = theme::Library::new(&config.theme.custom, dir.as_deref());
        let mut resolve =
            |name: &str| library.resolve(theme::effective(name, false), &mut boot_notices);
        let theme = resolve(&config.theme.default);
        let pane_theme = profile.theme.as_deref().map_or(theme, &mut resolve);
        (theme, pane_theme)
    };
    timing.mark(Mark::Config);

    // The shell starts now and boots while GPUI comes up. An SSH profile
    // waits for Enter instead: no network at startup (spec 0006, flow 6).
    let launch = terminal::Launch {
        command: if bench {
            Command::bench()
        } else {
            Command::of_profile(&profile)
        },
        cwd: profile.cwd().map(Path::to_path_buf),
        timing: Arc::clone(&timing),
    };
    let start = if !bench && matches!(profile.kind, config::ProfileKind::Ssh(_)) {
        terminal::Start::OnEnter(terminal::connect_prompt(profile.display_name()))
    } else {
        let font = FontSettings::from_config(&config.font);
        let grid = GridSize::fit(WINDOW_SIZE, font.estimated_cell());
        terminal::Start::Started(pty::spawn(
            &launch.command,
            grid,
            launch.cwd.as_deref(),
            config.scrollback as usize,
            Arc::clone(&timing),
        ))
    };
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
        gpui_component::init(cx);
        // Everything bound so far stays when a reload rebinds the commands.
        commands::keymap::keep_library(cx);
        boot_notices.extend(commands::keymap::bind(&config.keybindings, cx));
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
                size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("zed-term".into()),
                ..Default::default()
            }),
            app_id: Some("zed-term".into()),
            // Dev only: draw our own title bar even where the compositor
            // would draw one, to check the GNOME path (spec 0003, flow 8).
            window_decorations: std::env::var_os("ZED_TERM_CLIENT_DECORATIONS")
                .map(|_| WindowDecorations::Client),
            ..Default::default()
        };
        let boot = layout::BootPane {
            start,
            launch,
            profile: (&profile).into(),
            theme: pane_theme,
        };
        let opened = cx.open_window(options, |window, cx| {
            let workspace = cx.new(|cx| {
                layout::Workspace::new(
                    boot,
                    config,
                    font,
                    theme,
                    boot_notices,
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
            eprintln!("zed-term: could not open a window: {error:#}");
            std::process::exit(1);
        }
    });
}

/// On Linux, a Wayland session is required. Without one GPUI would start
/// headless and hang with no window (architecture §Key decisions).
fn check_display(wayland_display: Option<&OsStr>) -> Result<(), &'static str> {
    if cfg!(target_os = "linux") && wayland_display.is_none_or(OsStr::is_empty) {
        return Err("zed-term needs a Wayland session (X11 is not supported)");
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
