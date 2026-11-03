//! App wiring: the boot path (spec 0001) and the Tauri commands.
//!
//! Boot order is load-bearing: read config → spawn the boot PTY → create the
//! window with `__BOOT__` injected. The shell starts while the webview loads,
//! and the frontend needs no `invoke` round trip before its first paint.
//! Commands stay thin; logic lives in `pty`, `output`, `config` and `theme`.

mod config;
mod output;
mod pty;
mod theme;
mod timing;

use config::Config;
use output::Sink;
use pty::{PaneId, PtyManager};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};
use timing::{Mark, Timing, TtfpReport};

/// Initial window size in logical pixels, until session restore (T-006)
/// remembers the last one.
const WINDOW_SIZE: (f64, f64) = (1000.0, 640.0);
/// Horizontal and vertical padding around a pane, matching the frontend.
const PANE_PADDING: f64 = 8.0;

/// Delivers a pane's output over its Tauri Channel: raw bytes, then a final
/// JSON `{ "exit": code | null }`.
struct ChannelSink(Channel);

impl Sink for ChannelSink {
    // Send errors mean the page is gone (dev reload); the pane re-attaches.
    fn bytes(&self, data: Vec<u8>) {
        let _ = self.0.send(InvokeResponseBody::Raw(data));
    }
    fn exit(&self, code: Option<u32>) {
        let msg = serde_json::json!({ "exit": code }).to_string();
        let _ = self.0.send(InvokeResponseBody::Json(msg));
    }
}

struct AppState {
    ptys: PtyManager<ChannelSink>,
    timing: Arc<Timing>,
    bench: bool,
}

/// What the frontend needs for its first paint, injected as `window.__BOOT__`.
#[derive(Serialize)]
struct Boot<'a> {
    config: &'a Config,
    theme: &'a theme::Theme,
    panes: Vec<BootPane>,
    notices: Vec<String>,
    bench: bool,
    /// `false` when `ZED_TERM_DISABLE_WEBGL` is set, to exercise the DOM renderer.
    webgl: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
enum BootPane {
    Ready { id: PaneId },
    Failed { error: String, command: String },
}

/// Estimates the boot pane's grid from the window and font size, so the
/// shell starts at roughly the right size. The frontend's fit corrects it.
fn estimate_grid(font: &config::Font) -> (u16, u16) {
    let cell_w = font.size * 0.6;
    let cell_h = (font.size * font.line_height).ceil();
    let cols = ((WINDOW_SIZE.0 - PANE_PADDING) / cell_w).floor();
    let rows = ((WINDOW_SIZE.1 - PANE_PADDING) / cell_h).floor();
    (cols.clamp(2.0, 1000.0) as u16, rows.clamp(1.0, 1000.0) as u16)
}

#[tauri::command]
async fn pty_attach(state: State<'_, AppState>, id: PaneId, on_output: Channel) -> Result<(), String> {
    state.ptys.attach(id, ChannelSink(on_output))
}

#[tauri::command]
async fn pty_write(state: State<'_, AppState>, id: PaneId, data: String) -> Result<(), String> {
    state.ptys.write(id, data.into_bytes())
}

#[tauri::command]
async fn pty_resize(state: State<'_, AppState>, id: PaneId, cols: u16, rows: u16) -> Result<(), String> {
    state.ptys.resize(id, cols, rows)
}

#[tauri::command]
async fn pty_ack(state: State<'_, AppState>, id: PaneId, bytes: usize) -> Result<(), String> {
    state.ptys.ack(id, bytes)
}

/// Called by the frontend once the first PTY output is on screen. The first
/// call prints the marks as one JSON line; in bench mode it also quits.
#[tauri::command]
async fn report_first_prompt(app: AppHandle, state: State<'_, AppState>) -> Result<TtfpReport, String> {
    let first = state.timing.mark(Mark::Rendered);
    let report = state.timing.report();
    if first {
        println!("{}", serde_json::to_string(&report).map_err(|e| e.to_string())?);
        if state.bench {
            app.exit(0);
        }
    }
    Ok(report)
}

#[tauri::command]
async fn app_quit(app: AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

/// Runs the app. `start` is the instant `main()` was entered; every TTFP
/// mark is measured from it.
pub fn run(start: Instant) {
    let timing = Arc::new(Timing::new(start));
    let bench = std::env::args().any(|a| a == "--bench-startup");

    let loaded = config::default_path()
        .map(|p| config::load(&p))
        .unwrap_or(config::Loaded {
            config: Config::default(),
            notices: vec![],
        });
    timing.mark(Mark::Config);

    let ptys = PtyManager::new(Arc::clone(&timing));
    let mut cmd = if bench {
        pty::bench_command()
    } else {
        pty::default_command()
    };
    pty::prepare(&mut cmd, None);
    let command = pty::describe(&cmd);
    let (cols, rows) = estimate_grid(&loaded.config.font);
    let boot_pane = match ptys.spawn(cmd, cols, rows) {
        Ok(id) => BootPane::Ready { id },
        Err(error) => BootPane::Failed { error, command },
    };
    timing.mark(Mark::Pty);

    let theme = &theme::DARK;
    let boot = Boot {
        config: &loaded.config,
        theme,
        panes: vec![boot_pane],
        notices: loaded.notices,
        bench,
        webgl: std::env::var_os("ZED_TERM_DISABLE_WEBGL").is_none(),
    };
    let init_script = format!(
        "window.__BOOT__ = {};",
        serde_json::to_string(&boot).expect("boot data serializes")
    );
    let (r, g, b) = theme::hex_rgb(theme.background).unwrap_or((0, 0, 0));

    let window_timing = Arc::clone(&timing);
    tauri::Builder::default()
        .manage(AppState {
            ptys,
            timing,
            bench,
        })
        .invoke_handler(tauri::generate_handler![
            pty_attach,
            pty_write,
            pty_resize,
            pty_ack,
            report_first_prompt,
            app_quit
        ])
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("zed-term")
                .inner_size(WINDOW_SIZE.0, WINDOW_SIZE.1)
                .background_color(tauri::window::Color(r, g, b, 255))
                .initialization_script(init_script.as_str())
                .build()?;
            window_timing.mark(Mark::Window);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<AppState>().ptys.kill_all();
            }
        });
}
