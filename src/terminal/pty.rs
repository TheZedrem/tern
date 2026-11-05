//! Spawning a program in a PTY with `alacritty_terminal`, and bridging its
//! events to the UI thread. The event loop thread owns the PTY and the
//! parser; the UI only locks the grid to draw it (architecture §Key
//! decisions).

use crate::config::{Profile, ProfileKind};
use crate::ssh;
use crate::timing::{Mark, Timing};
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::collections::BTreeMap;
use std::path::Path;
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

/// What a pane runs: a program, its arguments and the environment it gets
/// on top of the inherited one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
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
                env: BTreeMap::new(),
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
                env: BTreeMap::new(),
            }
        }
    }

    /// What `profile` runs. A local profile: its command, or the login
    /// shell, with its env; on Windows the program resolves through `PATH`
    /// and `PATHEXT`. An SSH profile: [`ssh::command`].
    pub fn of_profile(profile: &Profile) -> Self {
        let local = match &profile.kind {
            ProfileKind::Local(local) => local,
            ProfileKind::Ssh(remote) => return ssh::command(remote),
        };
        let shell = match local.command.as_deref() {
            Some([program, args @ ..]) => Self {
                program: program.clone(),
                args: args.to_vec(),
                env: BTreeMap::new(),
            },
            _ => Self::default_shell(),
        };
        Self {
            env: local.env.clone(),
            ..shell
        }
    }

    /// Prints one line and exits, so `--bench-startup` measures Tern
    /// rather than the user's dotfiles (spec 0001).
    pub fn bench() -> Self {
        #[cfg(unix)]
        let (program, args) = ("sh", ["-c", "echo ready"]);
        #[cfg(windows)]
        let (program, args) = ("cmd.exe", ["/c", "echo ready"]);
        Self {
            program: program.into(),
            args: args.map(String::from).to_vec(),
            env: BTreeMap::new(),
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

/// Whether `program` is a file in one of `PATH`'s directories.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
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
    /// What feeds `events`, for a program started again on `term`.
    pub proxy: EventProxy,
    pub size: GridSize,
    /// The program's process ID, where the platform exposes it (Unix).
    pub child_pid: Option<u32>,
}

/// A program started on a terminal: the channel to it, and its process ID
/// where the platform exposes it (Unix).
pub type Started = (Notifier, Option<u32>);

/// Starts `command` in a new PTY of `size`, in `cwd` or the home directory.
/// Output is parsed on the event loop's own thread from the first byte,
/// even before any window exists.
pub fn spawn(
    command: &Command,
    size: GridSize,
    cwd: Option<&Path>,
    scrollback: usize,
    timing: Arc<Timing>,
) -> Result<Session, String> {
    let (tx, events) = unbounded();
    let proxy = EventProxy { tx, timing };
    let config = term::Config {
        scrolling_history: scrollback,
        // OSC 52 isn't even parsed into events: the terminal never writes
        // or reads the clipboard (TM-escape-04, -05).
        osc52: term::Osc52::Disabled,
        ..Default::default()
    };
    let term = Arc::new(FairMutex::new(Term::new(config, &size, proxy.clone())));
    let (notifier, child_pid) = rerun(command, size, cwd, &term, &proxy)?;
    Ok(Session {
        term,
        notifier,
        events,
        proxy,
        size,
        child_pid,
    })
}

/// Starts `command` in a new PTY on `term`, an existing session's
/// terminal, whose events keep going through `proxy`. The screen and
/// scrollback stay, so a reconnected SSH pane shows the old session above
/// the new one (spec 0006, flow 5).
pub fn rerun(
    command: &Command,
    size: GridSize,
    cwd: Option<&Path>,
    term: &Terminal,
    proxy: &EventProxy,
) -> Result<Started, String> {
    #[allow(
        clippy::needless_update,
        reason = "tty::Options has one more field on Windows"
    )]
    let options = tty::Options {
        shell: Some(tty::Shell::new(
            command.program.clone(),
            command.args.clone(),
        )),
        // A missing directory falls back to home: alacritty would otherwise
        // silently start in Tern's own working directory.
        working_directory: cwd
            .filter(|dir| dir.is_dir())
            .map(Path::to_path_buf)
            .or_else(dirs::home_dir),
        drain_on_exit: true,
        // The profile's env goes on top of ours: the user knows best.
        env: [
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
            ("TERM_PROGRAM", "tern"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .chain(command.env.clone())
        .collect(),
        ..Default::default()
    };
    let pty = tty::new(&options, size.window_size(), 0).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let child_pid = Some(pty.child().id());
    #[cfg(not(unix))]
    let child_pid = None;

    let event_loop = EventLoop::new(Arc::clone(term), proxy.clone(), pty, true, false)
        .map_err(|e| e.to_string())?;
    let notifier = Notifier(event_loop.channel());
    // Detached: the thread ends when the program exits and its output is drained.
    let _ = event_loop.spawn();
    Ok((notifier, child_pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::ssh_to;
    use crate::config::{LocalProfile, SshProfile};
    use std::num::NonZeroU16;
    use std::time::Instant;

    /// The `shell` profile, running `local` instead.
    fn local(local: LocalProfile) -> Profile {
        Profile {
            kind: ProfileKind::Local(local),
            ..Profile::shell()
        }
    }

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
            env: BTreeMap::new(),
        };
        assert_eq!(shell.describe(), "/usr/bin/zsh -l");
        let bare = Command {
            program: "cmd.exe".into(),
            args: vec![],
            env: BTreeMap::new(),
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
            env: BTreeMap::new(),
        };
        assert_eq!(Command::bench(), expected);
    }

    #[test]
    fn a_profile_without_a_command_runs_the_login_shell_with_its_env() {
        let env = BTreeMap::from([("EDITOR".to_owned(), "nvim".to_owned())]);
        let profile = local(LocalProfile {
            env: env.clone(),
            ..LocalProfile::default()
        });
        let expected = Command {
            env,
            ..Command::default_shell()
        };
        assert_eq!(Command::of_profile(&profile), expected);
    }

    #[test]
    fn an_ssh_profile_runs_ssh_to_its_host() {
        let remote = SshProfile {
            user: Some("alice".into()),
            port: NonZeroU16::new(2222),
            ..ssh_to("myserver.example.com")
        };
        let profile = Profile {
            kind: ProfileKind::Ssh(remote.clone()),
            ..Profile::shell()
        };
        assert_eq!(Command::of_profile(&profile), ssh::command(&remote));
    }

    /// What tells a pane that `ssh` is missing (spec 0006, flow 8).
    #[test]
    fn on_path_tells_whether_a_program_is_in_a_path_directory() {
        let everywhere = if cfg!(windows) { "cmd.exe" } else { "sh" };
        assert!(on_path(everywhere));
        assert!(!on_path("tern-test-no-such-program"));
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
            program: "/nonexistent/tern-test-shell".into(),
            args: vec![],
            env: BTreeMap::new(),
        };
        match spawn(&missing, SMALL, None, 100, timing()) {
            Ok(_) => panic!("spawned a program that doesn't exist"),
            Err(message) => assert!(!message.is_empty()),
        }
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        Command {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: BTreeMap::new(),
        }
    }

    /// Starts `command` in `cwd` (`None`: the home directory) and collects
    /// its events until it exited and the UI was woken after that, by which
    /// point all of its output is parsed.
    #[cfg(unix)]
    fn run_to_exit(
        command: &Command,
        size: GridSize,
        cwd: Option<&Path>,
        timing: Arc<Timing>,
    ) -> (Session, Vec<Event>) {
        let mut session = spawn(command, size, cwd, 100, timing).expect("spawn sh");
        let events = until_exit(&mut session.events);
        (session, events)
    }

    /// `events` as they arrive, until a program exited and the UI was woken
    /// after that.
    #[cfg(unix)]
    fn until_exit(events: &mut UnboundedReceiver<Event>) -> Vec<Event> {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        let mut received = Vec::new();
        loop {
            match events.try_recv() {
                Ok(event) => {
                    let exited = received.iter().any(|e| matches!(e, Event::ChildExit(_)));
                    let done = exited && matches!(event, Event::Wakeup);
                    received.push(event);
                    if done {
                        return received;
                    }
                }
                Err(_) => {
                    assert!(
                        Instant::now() < deadline,
                        "no exit within 10 s: {received:?}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }

    /// Every line `term` holds, scrollback first, without trailing blanks.
    #[cfg(unix)]
    fn all_lines<T>(term: &Term<T>) -> Vec<String> {
        use alacritty_terminal::index::{Column, Line};
        let grid = term.grid();
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .map(|line| {
                let row = &grid[Line(line)];
                let text: String = (0..grid.columns()).map(|col| row[Column(col)].c).collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    /// Spec 0006, flow 5: a reconnected pane shows the old session above
    /// the new one.
    #[cfg(unix)]
    #[test]
    fn a_program_rerun_on_a_finished_terminal_prints_below_what_was_there() {
        // Four lines on three rows: the first two go to the scrollback.
        let dropped = sh(r"printf 'one\ntwo\nthree\nfour\n'; exit 255");
        let (session, _) = run_to_exit(&dropped, SMALL, None, timing());
        rerun(
            &sh("echo again"),
            SMALL,
            None,
            &session.term,
            &session.proxy,
        )
        .expect("rerun sh");
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        let lines = loop {
            let lines = all_lines(&*session.term.lock());
            if lines.iter().any(|line| line == "again") {
                break lines;
            }
            assert!(
                Instant::now() < deadline,
                "no output within 10 s: {lines:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let printed: Vec<_> = lines
            .iter()
            .map(String::as_str)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(printed, ["one", "two", "three", "four", "again"]);
    }

    /// The pane hears of the rerun program's exit like of the first one's,
    /// so a connection that drops again offers to reconnect again.
    #[cfg(unix)]
    #[test]
    fn a_rerun_programs_exit_reaches_the_sessions_events() {
        let (mut session, _) = run_to_exit(&sh("exit 255"), SMALL, None, timing());
        rerun(&sh("exit 7"), SMALL, None, &session.term, &session.proxy).expect("rerun sh");
        let exit = until_exit(&mut session.events)
            .iter()
            .find_map(|event| match event {
                Event::ChildExit(status) => Some(status.code()),
                _ => None,
            });
        assert_eq!(exit, Some(Some(7)));
    }

    #[cfg(unix)]
    #[test]
    fn output_and_exit_code_reach_the_pane() {
        let timing = timing();
        // Returns only after a Wakeup, so the UI is told to redraw.
        let (session, events) =
            run_to_exit(&sh("printf hi; exit 3"), SMALL, None, Arc::clone(&timing));
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
        let (session, _) = run_to_exit(&sh(script), size, None, timing());
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        assert_eq!(screen[0], "xterm-256color truecolor tern");
        assert_eq!(screen[1], "4 100");
        let home = dirs::home_dir()
            .expect("home dir")
            .canonicalize()
            .expect("home exists");
        assert_eq!(std::path::Path::new(&screen[2]), home);
    }

    /// A split starts in its source pane's directory (spec 0003, flow 1).
    #[cfg(unix)]
    #[test]
    fn the_program_starts_in_the_directory_it_was_given() {
        let dir = std::env::temp_dir().join(format!("tern-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create a temp dir");
        let expected = dir.canonicalize().expect("the temp dir exists");
        // Wide enough that the path fits on one row.
        let wide = GridSize {
            cols: 400,
            rows: 2,
            ..SMALL
        };
        let (session, _) = run_to_exit(&sh("pwd -P"), wide, Some(&dir), timing());
        let _ = std::fs::remove_dir(&dir);
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        assert_eq!(Path::new(&screen[0]), expected);
    }

    /// The pane finds the program's cwd through this ID (spec 0002), so it
    /// must be the program's own process.
    #[cfg(unix)]
    #[test]
    fn child_pid_is_the_programs_process_id() {
        let (session, _) = run_to_exit(&sh("echo $$"), SMALL, None, timing());
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        let printed = screen[0].parse().expect("sh printed its PID");
        assert_eq!(session.child_pid, Some(printed));
    }

    /// A directory that is gone starts the program at home, not in
    /// Tern's own working directory, also when it is spelled the way
    /// `/proc` reports a removed one (spec 0003, flow 1).
    #[cfg(unix)]
    #[test]
    fn a_missing_directory_starts_the_program_at_home() {
        let removed = std::env::temp_dir().join(format!("tern-removed-{}", std::process::id()));
        std::fs::create_dir_all(&removed).expect("create a temp dir");
        std::fs::remove_dir(&removed).expect("remove it again");
        let mut deleted = removed.clone().into_os_string();
        deleted.push(" (deleted)");
        let home = dirs::home_dir()
            .expect("home dir")
            .canonicalize()
            .expect("home exists");
        let wide = GridSize {
            cols: 400,
            rows: 2,
            ..SMALL
        };
        for missing in [removed.as_path(), Path::new(&deleted)] {
            let (session, _) = run_to_exit(&sh("pwd -P"), wide, Some(missing), timing());
            let screen = crate::terminal::tests::screen(&*session.term.lock());
            assert_eq!(Path::new(&screen[0]), home, "started in {missing:?}");
        }
    }

    /// AC3 (spec 0005), the config's part: the profile's program gets its
    /// arguments, starts in its cwd, and sees its env on top of the
    /// inherited environment (spec 0005, flow 2).
    #[cfg(unix)]
    #[test]
    fn a_profiles_command_args_env_and_cwd_reach_the_program() {
        let dir = crate::config::tests::TempDir::new("profile-cwd");
        let cwd = dir.path().canonicalize().expect("the temp dir exists");
        // The inherited PATH comes back as `$3`, to compare it in the shell.
        let inherited = std::env::var("PATH").expect("PATH is set");
        let script = r#"echo "$0 $1 $2"; echo "$GREETING"; [ "$PATH" = "$3" ] && echo inherited || echo replaced; pwd -P"#;
        let command = ["sh", "-c", script, "probe", "first", "second"]
            .map(String::from)
            .into_iter()
            .chain([inherited])
            .collect();
        let profile = Profile {
            id: "probe".into(),
            ..local(LocalProfile {
                command: Some(command),
                cwd: Some(dir.path().to_path_buf()),
                env: BTreeMap::from([("GREETING".into(), "hello".into())]),
            })
        };
        let wide = GridSize {
            cols: 400,
            rows: 6,
            ..SMALL
        };
        let (session, _) = run_to_exit(
            &Command::of_profile(&profile),
            wide,
            profile.cwd(),
            timing(),
        );
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        let cwd = cwd.to_str().expect("a UTF-8 temp dir");
        assert_eq!(
            screen[..4],
            ["probe first second", "hello", "inherited", cwd]
        );
    }

    /// The user knows best: a profile may set TERM itself.
    #[cfg(unix)]
    #[test]
    fn a_profiles_env_wins_over_terns_own_terminal_variables() {
        let profile = local(LocalProfile {
            command: Some(
                ["sh", "-c", r#"echo "$TERM $COLORTERM $TERM_PROGRAM""#]
                    .map(String::from)
                    .to_vec(),
            ),
            env: BTreeMap::from([("TERM".into(), "dumb".into())]),
            ..LocalProfile::default()
        });
        let wide = GridSize { cols: 100, ..SMALL };
        let (session, _) = run_to_exit(&Command::of_profile(&profile), wide, None, timing());
        let screen = crate::terminal::tests::screen(&*session.term.lock());
        assert_eq!(screen[0], "dumb truecolor tern");
    }
}
