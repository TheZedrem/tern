//! Spawning a program in a PTY with `alacritty_terminal`, and bridging its
//! events to the UI thread. The event loop thread owns the PTY and the
//! parser; the UI only locks the grid to draw it (architecture §Key
//! decisions).

use crate::timing::{Mark, Timing};
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::sync::Arc;

/// Padding around the grid inside its pane, in logical pixels.
pub const PADDING: f32 = 6.0;

/// A grid size in cells, plus the cell size in whole pixels (PTYs report
/// both to programs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl GridSize {
    /// The largest grid that fits `area` (logical pixels, padding included)
    /// with cells of `cell` size; at least 2×1.
    pub fn fit(area: (f32, f32), cell: (f32, f32)) -> Self {
        let cells = |length: f32, cell: f32| ((length - 2.0 * PADDING) / cell).floor().max(0.0);
        Self {
            cols: (cells(area.0, cell.0) as u16).max(2),
            rows: (cells(area.1, cell.1) as u16).max(1),
            cell_width: cell.0.round() as u16,
            cell_height: cell.1.round() as u16,
        }
    }

    pub fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.cols)
    }
}

/// What a pane runs: a program and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

impl Command {
    /// The user's login shell on Unix. On Windows, the first of `pwsh`,
    /// `powershell` and `cmd` found on `PATH` (OQ-003).
    pub fn default_shell() -> Self {
        #[cfg(unix)]
        {
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/bin/sh".into());
            Self {
                program: shell,
                args: vec!["-l".into()],
            }
        }
        #[cfg(windows)]
        {
            let found = ["pwsh.exe", "powershell.exe"]
                .into_iter()
                .find(|exe| on_path(exe));
            Self {
                program: found.unwrap_or("cmd.exe").into(),
                args: vec![],
            }
        }
    }

    /// Prints one line and exits, so `--bench-startup` measures zed-term
    /// rather than the user's dotfiles (spec 0001).
    pub fn bench() -> Self {
        #[cfg(unix)]
        let (program, args) = ("sh", ["-c", "echo ready"]);
        #[cfg(windows)]
        let (program, args) = ("cmd.exe", ["/c", "echo ready"]);
        Self {
            program: program.into(),
            args: args.map(String::from).to_vec(),
        }
    }

    /// The command line as the user would type it, for error messages.
    pub fn describe(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(windows)]
fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(exe).is_file()))
}

/// Forwards terminal events to the UI thread and records the first output.
#[derive(Clone)]
pub struct EventProxy {
    tx: UnboundedSender<Event>,
    timing: Arc<Timing>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        if matches!(event, Event::Wakeup) {
            self.timing.mark(Mark::FirstByte);
        }
        // An error only means the UI is gone; there is nobody left to tell.
        let _ = self.tx.unbounded_send(event);
    }
}

pub type Terminal = Arc<FairMutex<Term<EventProxy>>>;

/// A running program: its terminal state, the channel for input and
/// resizes, and the events the UI has to handle.
pub struct Session {
    pub term: Terminal,
    pub notifier: Notifier,
    pub events: UnboundedReceiver<Event>,
    pub size: GridSize,
}

/// Starts `command` in a new PTY of `size`. Output is parsed on the event
/// loop's own thread from the first byte, even before any window exists.
pub fn spawn(
    command: &Command,
    size: GridSize,
    scrollback: usize,
    timing: Arc<Timing>,
) -> Result<Session, String> {
    #[allow(
        clippy::needless_update,
        reason = "tty::Options has one more field on Windows"
    )]
    let options = tty::Options {
        shell: Some(tty::Shell::new(
            command.program.clone(),
            command.args.clone(),
        )),
        working_directory: dirs::home_dir(),
        drain_on_exit: true,
        env: [
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
            ("TERM_PROGRAM", "zed-term"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect(),
        ..Default::default()
    };
    let pty = tty::new(&options, size.window_size(), 0).map_err(|e| e.to_string())?;

    let (tx, events) = unbounded();
    let proxy = EventProxy { tx, timing };
    let config = term::Config {
        scrolling_history: scrollback,
        ..Default::default()
    };
    let term = Arc::new(FairMutex::new(Term::new(config, &size, proxy.clone())));
    let event_loop =
        EventLoop::new(Arc::clone(&term), proxy, pty, true, false).map_err(|e| e.to_string())?;
    let notifier = Notifier(event_loop.channel());
    // Detached: the thread ends when the program exits and its output is drained.
    let _ = event_loop.spawn();

    Ok(Session {
        term,
        notifier,
        events,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const SMALL: GridSize = GridSize {
        cols: 20,
        rows: 3,
        cell_width: 8,
        cell_height: 16,
    };

    fn timing() -> Arc<Timing> {
        Arc::new(Timing::new(Instant::now()))
    }

    #[test]
    fn fit_takes_off_the_padding_and_counts_whole_cells() {
        // 80×24 cells of 8×16 px, plus the padding on both sides.
        let area = (80.0 * 8.0 + 2.0 * PADDING, 24.0 * 16.0 + 2.0 * PADDING);
        let exact = GridSize::fit(area, (8.0, 16.0));
        assert_eq!((exact.cols, exact.rows), (80, 24));
        // One pixel less loses a whole column and row.
        let short = GridSize::fit((area.0 - 1.0, area.1 - 1.0), (8.0, 16.0));
        assert_eq!((short.cols, short.rows), (79, 23));
    }

    #[test]
    fn fit_reports_the_cell_size_in_whole_pixels() {
        let size = GridSize::fit((1000.0, 640.0), (7.8, 15.6));
        assert_eq!((size.cell_width, size.cell_height), (8, 16));
    }

    #[test]
    fn fit_keeps_at_least_two_columns_and_one_row() {
        for area in [(0.0, 0.0), (2.0 * PADDING, 2.0 * PADDING), (20.0, 20.0)] {
            let size = GridSize::fit(area, (8.0, 16.0));
            assert_eq!((size.cols, size.rows), (2, 1), "area {area:?}");
        }
    }

    #[test]
    fn describe_is_the_command_line() {
        let shell = Command {
            program: "/usr/bin/zsh".into(),
            args: vec!["-l".into()],
        };
        assert_eq!(shell.describe(), "/usr/bin/zsh -l");
        let bare = Command {
            program: "cmd.exe".into(),
            args: vec![],
        };
        assert_eq!(bare.describe(), "cmd.exe");
    }

    #[test]
    fn bench_runs_echo_ready_instead_of_the_users_shell() {
        let (program, flag) = if cfg!(windows) {
            ("cmd.exe", "/c")
        } else {
            ("sh", "-c")
        };
        let expected = Command {
            program: program.into(),
            args: vec![flag.into(), "echo ready".into()],
        };
        assert_eq!(Command::bench(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn default_shell_is_shell_started_as_a_login_shell() {
        let shell = Command::default_shell();
        assert_eq!(shell.args, ["-l"]);
        if let Some(expected) = std::env::var("SHELL").ok().filter(|s| !s.is_empty()) {
            assert_eq!(shell.program, expected);
        }
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let missing = Command {
            program: "/nonexistent/zed-term-test-shell".into(),
            args: vec![],
        };
        match spawn(&missing, SMALL, 100, timing()) {
            Ok(_) => panic!("spawned a program that doesn't exist"),
            Err(message) => assert!(!message.is_empty()),
        }
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        Command {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
        }
    }

    /// Starts `command` and collects its events until it exited and the UI
    /// was woken after that, by which point all of its output is parsed.
    #[cfg(unix)]
    fn run_to_exit(
        command: &Command,
        size: GridSize,
        timing: Arc<Timing>,
    ) -> (Session, Vec<Event>) {
        let mut session = spawn(command, size, 100, timing).expect("spawn sh");
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        let mut events = Vec::new();
        loop {
            match session.events.try_recv() {
                Ok(event) => {
                    let exited = events.iter().any(|e| matches!(e, Event::ChildExit(_)));
                    let done = exited && matches!(event, Event::Wakeup);
                    events.push(event);
                    if done {
                        return (session, events);
                    }
                }
                Err(_) => {
                    assert!(Instant::now() < deadline, "no exit within 10 s: {events:?}");
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_and_exit_code_reach_the_pane() {
        let timing = timing();
        // Returns only after a Wakeup, so the UI is told to redraw.
        let (session, events) = run_to_exit(&sh("printf hi; exit 3"), SMALL, Arc::clone(&timing));
        let exit = events.iter().find_map(|event| match event {
            Event::ChildExit(status) => Some(status.code()),
            _ => None,
        });
        assert_eq!(exit, Some(Some(3)));
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        assert_eq!(screen[0], "hi");
        assert!(timing.report().first_byte.is_some(), "no first_byte mark");
    }

    #[cfg(unix)]
    #[test]
    fn the_program_gets_the_terminal_env_the_grid_size_and_the_home_directory() {
        let size = GridSize {
            cols: 100,
            rows: 4,
            ..SMALL
        };
        let script =
            r#"printf '%s %s %s\n' "$TERM" "$COLORTERM" "$TERM_PROGRAM"; stty size; pwd -P"#;
        let (session, _) = run_to_exit(&sh(script), size, timing());
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        assert_eq!(screen[0], "xterm-256color truecolor zed-term");
        assert_eq!(screen[1], "4 100");
        let home = dirs::home_dir()
            .expect("home dir")
            .canonicalize()
            .expect("home exists");
        assert_eq!(std::path::Path::new(&screen[2]), home);
    }
}
