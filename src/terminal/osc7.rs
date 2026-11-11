//! The working directory a shell reports with OSC 7 (spec 0002, cwd;
//! TM-escape-09). vte doesn't parse OSC 7, so [`Osc7Pty`] wraps a PTY and
//! scans the bytes it reads, passing them on untouched. Only the report's
//! path is kept, never the output around it.

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, PollMode, Poller};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Reports longer than this are ignored (TM-escape-10).
const MAX_REPORT: usize = 4096;

/// The last cwd a pane's program reported, shared between the PTY's reader
/// thread and the pane.
pub struct Reported {
    /// An SSH pane's reports name a folder on the remote host.
    remote: bool,
    /// A local pane's checked local directory, or an SSH pane's remote path.
    path: Mutex<Option<String>>,
    /// A local pane's cwd changed since the pane last looked.
    changed: AtomicBool,
}

impl Reported {
    pub fn new(remote: bool) -> Arc<Self> {
        Arc::new(Self {
            remote,
            path: Mutex::new(None),
            changed: AtomicBool::new(false),
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

    fn path(&self) -> Option<String> {
        self.path.lock().ok()?.clone()
    }

    /// Takes the payload of one OSC 7: a local pane keeps it only if it
    /// names an existing directory on this machine.
    fn report(&self, payload: &[u8]) {
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

/// Where the scanner is in the byte stream. Reads split sequences
/// anywhere, so the state carries over between them.
#[derive(Debug, PartialEq)]
enum State {
    Ground,
    Escape,
    /// After `ESC ]`, matched this many bytes of `7;`.
    Introducer(usize),
    /// Collecting the payload, up to [`MAX_REPORT`] bytes.
    Payload,
    /// An `ESC` inside the payload: `\` ends it (ST).
    PayloadEscape,
}

/// Finds `ESC ] 7 ; <payload> (BEL | ESC \)` in a byte stream.
struct Scanner {
    state: State,
    payload: Vec<u8>,
    too_long: bool,
}

impl Scanner {
    fn new() -> Self {
        Self {
            state: State::Ground,
            payload: Vec::new(),
            too_long: false,
        }
    }

    /// Feeds `bytes`, calling `found` with each complete payload.
    fn feed(&mut self, bytes: &[u8], found: &mut impl FnMut(&[u8])) {
        for &byte in bytes {
            self.step(byte, found);
        }
    }

    fn step(&mut self, byte: u8, found: &mut impl FnMut(&[u8])) {
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
            (State::Escape, b']') => State::Introducer(0),
            (State::Escape, ESC) => State::Escape,
            (State::Escape, _) => State::Ground,
            (State::Introducer(0), b'7') => State::Introducer(1),
            (State::Introducer(1), b';') => {
                self.payload.clear();
                self.too_long = false;
                State::Payload
            }
            (State::Introducer(_), ESC) => State::Escape,
            (State::Introducer(_), _) => State::Ground,
            (State::Payload, BEL) => {
                self.finish(found);
                State::Ground
            }
            (State::Payload, ESC) => State::PayloadEscape,
            (State::Payload, _) => {
                if self.payload.len() < MAX_REPORT {
                    self.payload.push(byte);
                } else {
                    self.too_long = true;
                }
                State::Payload
            }
            (State::PayloadEscape, b'\\') => {
                self.finish(found);
                State::Ground
            }
            // Any other escape cancels the report and starts a sequence.
            (State::PayloadEscape, _) => {
                self.state = State::Escape;
                return self.step(byte, found);
            }
        };
    }

    fn finish(&mut self, found: &mut impl FnMut(&[u8])) {
        if !self.too_long {
            found(&self.payload);
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

/// A PTY whose output is scanned for OSC 7 on its way to the parser. The
/// event loop reads through it as through the PTY itself; everything else
/// is the PTY's.
pub struct Osc7Pty<P> {
    pty: P,
    scanner: Scanner,
    reported: Arc<Reported>,
}

impl<P> Osc7Pty<P> {
    pub fn new(pty: P, reported: Arc<Reported>) -> Self {
        Self {
            pty,
            scanner: Scanner::new(),
            reported,
        }
    }
}

impl<P: EventedReadWrite> Read for Osc7Pty<P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.pty.reader().read(buf)?;
        let reported = &self.reported;
        self.scanner
            .feed(&buf[..read], &mut |payload| reported.report(payload));
        Ok(read)
    }
}

impl<P: EventedReadWrite> EventedReadWrite for Osc7Pty<P> {
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

impl<P: EventedPty> EventedPty for Osc7Pty<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl<P: OnResize> OnResize for Osc7Pty<P> {
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
            scanner.feed(chunk, &mut |payload| {
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
        remote.report(b"file://prod/srv/app");
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
        local.report(url.as_bytes());
        assert!(local.local().is_some_and(|path| path.is_dir()));
        assert_eq!(local.remote(), None);
        assert!(local.take_changed());
        local.report(url.as_bytes());
        assert!(!local.take_changed(), "the same directory again");
        local.report(b"file://elsewhere.example/tmp");
        assert!(!local.take_changed(), "another machine's report is ignored");
    }
}
