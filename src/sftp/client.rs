//! A pane's SFTP connection (spec 0008, Model): `ssh … -s <host> sftp`
//! with the `russh-sftp` client on its stdio, on a thread of its own with a
//! current-thread tokio runtime. The sidebar sends [`Request`]s and draws
//! the [`Event`]s that come back; nothing here runs on the UI thread.
//! Dropping the [`Client`] ends the thread, which kills its ssh.

use super::{Entry, Kind, Listing, MAX_ENTRIES};
use crate::terminal::clean_prompt;
use crate::terminal::pty::Command;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::join_all;
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error;
use russh_sftp::protocol::{File, StatusCode};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::sync::mpsc;

/// A request with no reply in this long ends the connection as dropped
/// (spec 0008, Model; TM-sftp-17).
const REQUEST_TIMEOUT_SECS: u64 = 30;
/// How long a failed ssh gets to exit and finish saying why, before it's
/// killed.
const EXIT_WAIT: Duration = Duration::from_secs(2);
/// The most of ssh's stderr shown as its reason (TM-sftp-16).
const MAX_REASON_BYTES: usize = 1024;
/// How much of ssh's stderr is kept while it runs: its last words matter.
const STDERR_KEEP: usize = 4 * MAX_REASON_BYTES;
/// How many link targets are looked up at once.
const LINK_LOOKUPS: usize = 64;

/// What the sidebar asks of its connection.
pub enum Request {
    /// List the folder at this absolute path.
    List(String),
}

/// What the connection reports.
#[derive(Debug)]
pub enum Event {
    /// Logged in; the remote home folder, where relative paths start.
    Connected {
        home: String,
    },
    Listed(Listing),
    /// Listing `path` failed; the connection is still up.
    ListFailed {
        path: String,
        error: String,
    },
    /// The connection is over, and why, in ssh's words where it gave some.
    Ended(String),
}

/// The sidebar's end of a running connection.
pub struct Client {
    requests: mpsc::UnboundedSender<Request>,
}

impl Client {
    /// Starts `command` (ssh with the SFTP subsystem and its askpass
    /// environment) on a new thread. Events arrive on the receiver until
    /// the connection ends; [`Event::Ended`] is the last.
    pub fn connect(command: Command) -> (Self, UnboundedReceiver<Event>) {
        let (requests, requests_rx) = mpsc::unbounded_channel();
        let (events, events_rx) = unbounded();
        let thread_events = events.clone();
        let started = std::thread::Builder::new()
            .name("sftp".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let reason = format!("Couldn't start the SFTP connection: {error}");
                        let _ = thread_events.unbounded_send(Event::Ended(reason));
                        return;
                    }
                };
                runtime.block_on(run(command, requests_rx, thread_events));
            });
        if let Err(error) = started {
            let reason = format!("Couldn't start the SFTP connection: {error}");
            let _ = events.unbounded_send(Event::Ended(reason));
        }
        (Self { requests }, events_rx)
    }

    /// Lists the folder at absolute `path`; a [`Event::Listed`] or
    /// [`Event::ListFailed`] answers.
    pub fn list(&self, path: String) {
        let _ = self.requests.send(Request::List(path));
    }
}

/// Runs the connection until the sidebar drops its [`Client`] or the
/// connection ends, which it reports.
async fn run(
    command: Command,
    mut requests: mpsc::UnboundedReceiver<Request>,
    events: UnboundedSender<Event>,
) {
    let mut ssh = match spawn(&command) {
        Ok(ssh) => ssh,
        Err(error) => {
            let reason = format!("Couldn't start ssh: {error}");
            let _ = events.unbounded_send(Event::Ended(reason));
            return;
        }
    };
    let stderr = ssh
        .stderr
        .take()
        .map(|stderr| tokio::spawn(keep_tail(stderr)));
    let (Some(stdout), Some(stdin)) = (ssh.stdout.take(), ssh.stdin.take()) else {
        let _ = events.unbounded_send(Event::Ended("ssh started without its pipes".into()));
        return;
    };
    let config = russh_sftp::client::Config {
        request_timeout_secs: REQUEST_TIMEOUT_SECS,
        ..Default::default()
    };
    let sftp = RawSftpSession::new_with_config(tokio::io::join(stdout, stdin), config);
    let failure = match serve(&sftp, &mut ssh, &mut requests, &events).await {
        Ok(()) => return,
        Err(failure) => failure,
    };
    drop(sftp);
    let reason = reason(&mut ssh, stderr, failure).await;
    let _ = events.unbounded_send(Event::Ended(reason));
}

/// Starts ssh with its stdio piped. On Windows it gets no console window
/// of its own.
fn spawn(command: &Command) -> std::io::Result<Child> {
    let mut ssh = tokio::process::Command::new(&command.program);
    ssh.args(&command.args)
        .envs(&command.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        ssh.creation_flags(CREATE_NO_WINDOW);
    }
    ssh.spawn()
}

/// Logs in, reports the home folder, then answers requests until the
/// sidebar goes away (`Ok`) or the connection fails (`Err` with what
/// failed, for when ssh says nothing).
async fn serve(
    sftp: &RawSftpSession,
    ssh: &mut Child,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    events: &UnboundedSender<Event>,
) -> Result<(), String> {
    sftp.init().await.map_err(|error| describe(&error))?;
    let home = sftp
        .realpath(".")
        .await
        .map_err(|error| describe(&error))?
        .files
        .into_iter()
        .next()
        .map(|file| file.filename)
        .ok_or("The server didn't say where the home folder is")?;
    let _ = events.unbounded_send(Event::Connected { home });
    loop {
        let request = tokio::select! {
            request = requests.recv() => request,
            exit = ssh.wait() => return Err(exited(exit)),
        };
        let Some(Request::List(path)) = request else {
            return Ok(());
        };
        let event = match list(sftp, &path).await {
            Ok(listing) => Event::Listed(listing),
            Err(Error::Status(status)) if status.status_code != StatusCode::ConnectionLost => {
                Event::ListFailed {
                    path,
                    error: status_message(&status.error_message, status.status_code),
                }
            }
            Err(error) => return Err(describe(&error)),
        };
        if events.unbounded_send(event).is_err() {
            return Ok(());
        }
    }
}

/// Lists the folder at `path`: at most [`MAX_ENTRIES`] entries, links
/// resolved to what they point to, sorted. A server that sends more stops
/// being read (TM-sftp-17).
async fn list(sftp: &RawSftpSession, path: &str) -> Result<Listing, Error> {
    let handle = sftp.opendir(path).await?.handle;
    let mut files: Vec<File> = Vec::new();
    let mut truncated = false;
    let read = loop {
        match sftp.readdir(handle.as_str()).await {
            Ok(name) => {
                let shown = name
                    .files
                    .into_iter()
                    .filter(|file| !super::hidden(&file.filename));
                for file in shown {
                    if files.len() == MAX_ENTRIES {
                        truncated = true;
                        break;
                    }
                    files.push(file);
                }
                if truncated {
                    break Ok(());
                }
            }
            Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    let _ = sftp.close(handle).await;
    read?;
    let mut entries: Vec<Entry> = files.into_iter().map(entry).collect();
    resolve_links(sftp, path, &mut entries).await;
    super::sort(&mut entries);
    Ok(Listing {
        path: path.to_owned(),
        entries,
        truncated,
    })
}

/// An entry as `readdir` describes it, which for a link is the link.
fn entry(file: File) -> Entry {
    let kind = file.attrs.file_type();
    Entry {
        kind: if kind.is_dir() {
            Kind::Folder
        } else if kind.is_file() || kind.is_symlink() {
            Kind::File
        } else {
            Kind::Other
        },
        link: kind.is_symlink(),
        size: file.attrs.size,
        modified: file.attrs.mtime,
        name: file.filename,
    }
}

/// Looks up what each link in `entries` points to, a batch at a time, so
/// a link to a folder enters like one. A broken link is `Other`.
async fn resolve_links(sftp: &RawSftpSession, path: &str, entries: &mut [Entry]) {
    let links: Vec<usize> = (0..entries.len()).filter(|&ix| entries[ix].link).collect();
    for batch in links.chunks(LINK_LOOKUPS) {
        let targets = join_all(
            batch
                .iter()
                .map(|&ix| sftp.stat(super::join(path, &entries[ix].name))),
        )
        .await;
        for (&ix, target) in batch.iter().zip(targets) {
            let entry = &mut entries[ix];
            match target {
                Ok(target) => {
                    let attrs = target.attrs;
                    let kind = attrs.file_type();
                    entry.kind = if kind.is_dir() {
                        Kind::Folder
                    } else if kind.is_file() {
                        Kind::File
                    } else {
                        Kind::Other
                    };
                    entry.size = attrs.size;
                }
                Err(_) => entry.kind = Kind::Other,
            }
        }
    }
}

/// Keeps reading ssh's stderr until it closes, holding on to its last
/// [`STDERR_KEEP`] bytes, so ssh never blocks on a full pipe.
async fn keep_tail(mut stderr: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = [0; 1024];
    while let Ok(read @ 1..) = stderr.read(&mut buffer).await {
        kept.extend_from_slice(&buffer[..read]);
        if kept.len() > STDERR_KEEP {
            kept.drain(..kept.len() - STDERR_KEEP);
        }
    }
    kept
}

/// Why the connection ended: ssh's own words when it said any, else
/// `failure`. ssh gets [`EXIT_WAIT`] to finish, then it's killed.
async fn reason(
    ssh: &mut Child,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
    failure: String,
) -> String {
    if tokio::time::timeout(EXIT_WAIT, ssh.wait()).await.is_err() {
        let _ = ssh.kill().await;
    }
    let said = match stderr {
        Some(stderr) => stderr.await.unwrap_or_default(),
        None => Vec::new(),
    };
    let said = clean_reason(&String::from_utf8_lossy(&said));
    if said.is_empty() { failure } else { said }
}

/// ssh's stderr as the sidebar shows it: plain text without control or
/// invisible characters, its last [`MAX_REASON_BYTES`] at most, cut at a
/// line break where one is near (TM-sftp-16).
pub fn clean_reason(said: &str) -> String {
    // Cut first: cleaning keeps a text's start, and ssh's reason is its end.
    let cleaned = clean_prompt(tail(said, STDERR_KEEP));
    let cleaned = cleaned.trim();
    if cleaned.len() <= MAX_REASON_BYTES {
        return cleaned.to_owned();
    }
    let tail = tail(cleaned, MAX_REASON_BYTES);
    match tail.find('\n') {
        Some(line) => tail[line + 1..].to_owned(),
        None => tail.to_owned(),
    }
}

/// The last `bytes` of `text` at most, starting on a whole character.
fn tail(text: &str, bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// What a failed request says, in the sidebar's words.
fn describe(error: &Error) -> String {
    match error {
        Error::Timeout => "The server stopped answering".into(),
        Error::Status(status) => status_message(&status.error_message, status.status_code),
        Error::IO(_) | Error::UnexpectedBehavior(_) => "The connection closed".into(),
        Error::Limited(_) | Error::UnexpectedPacket => {
            "The server sent something that isn't SFTP".into()
        }
    }
}

/// A server's status message, which it wrote: as plain text, a line at
/// most. An empty one is named by its code.
fn status_message(message: &str, code: StatusCode) -> String {
    let message = crate::terminal::clean_title(message);
    if message.is_empty() {
        format!("{code:?}")
    } else {
        message
    }
}

/// The reason for an ssh that exited while the connection was up.
fn exited(exit: std::io::Result<ExitStatus>) -> String {
    match exit.ok().and_then(|status| status.code()) {
        Some(code) => format!("ssh exited with code {code}"),
        None => "The connection closed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TM-sftp-16: what ssh said is plain text: no escape sequences'
    /// control bytes, no bell, no direction overrides or zero-width
    /// characters that could disguise it; its words and lines stay.
    #[test]
    fn a_reason_loses_control_and_invisible_characters_but_keeps_its_words() {
        let said = "\x1b[31mssh: connect\x07 to host\u{202E}x\u{200B} port 22:\r\nConnection refused\x1b[0m\n";
        let cleaned = clean_reason(said);
        assert!(
            !cleaned
                .chars()
                .any(|c| (c.is_control() && c != '\n') || matches!(c, '\u{202E}' | '\u{200B}')),
            "{cleaned:?}"
        );
        assert!(cleaned.contains("ssh: connect to host"), "{cleaned:?}");
        assert!(
            cleaned.ends_with("port 22:\nConnection refused[0m"),
            "{cleaned:?}"
        );
    }

    /// TM-sftp-16: at most 1 KiB is shown, and it's the end, where ssh
    /// says why it gave up; a cut never splits a character.
    #[test]
    fn a_long_reason_is_cut_to_its_last_kib() {
        let mut said = String::new();
        for line in 0..60 {
            said.push_str(&format!("debug1: line {line} ééééé\n"));
        }
        said.push_str("ssh: Could not resolve hostname nowhere");
        assert!(said.len() > 1024 && said.len() < 4096);
        let cleaned = clean_reason(&said);
        assert!(cleaned.len() <= 1024, "{} bytes", cleaned.len());
        assert!(
            cleaned.ends_with("ssh: Could not resolve hostname nowhere"),
            "{cleaned:?}"
        );
    }

    /// TM-sftp-16: the end is kept however long ssh's stderr was. The
    /// 4 KiB kept of stderr can exceed 4 KiB after lossy UTF-8 decoding
    /// (each invalid byte becomes a 3-byte U+FFFD).
    #[test]
    fn a_reason_longer_than_4_kib_still_keeps_its_end() {
        let said = format!("{}\nssh: the last words", "x".repeat(10 * 1024));
        let cleaned = clean_reason(&said);
        assert!(cleaned.len() <= 1024, "{} bytes", cleaned.len());
        assert!(cleaned.ends_with("ssh: the last words"), "{cleaned:?}");
    }

    /// Runs against OpenSSH's `sftp-server` speaking SFTP on its stdio, as
    /// `ssh -s <host> sftp` would, so no host or ssh login is needed.
    #[cfg(unix)]
    mod server {
        use super::super::*;
        use crate::config::tests::TempDir;
        use futures::StreamExt;
        use futures::executor::block_on;
        use futures::future::{Either, select};
        use std::fs;
        use std::os::unix::fs::symlink;
        use std::path::Path;

        const SFTP_SERVER: &str = "/usr/libexec/openssh/sftp-server";

        /// The next event within `seconds`; `None` once the stream ended.
        fn next_within(events: &mut UnboundedReceiver<Event>, seconds: u64) -> Option<Event> {
            let (timer, expired) = futures::channel::oneshot::channel::<()>();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(seconds));
                let _ = timer.send(());
            });
            match block_on(select(events.next(), expired)) {
                Either::Left((event, _)) => event,
                Either::Right(_) => panic!("no event within {seconds} s"),
            }
        }

        /// A connection to `sftp-server` starting in `dir`, with its
        /// `Connected` event taken off; `None` (the test skips) where
        /// OpenSSH's server isn't installed there.
        fn connect(dir: &Path) -> Option<(Client, UnboundedReceiver<Event>, String)> {
            if !Path::new(SFTP_SERVER).exists() {
                eprintln!("skipped: no {SFTP_SERVER}");
                return None;
            }
            let (client, mut events) = Client::connect(Command {
                program: SFTP_SERVER.into(),
                args: vec!["-d".into(), dir.display().to_string()],
                env: Default::default(),
            });
            match next_within(&mut events, 10) {
                Some(Event::Connected { home }) => Some((client, events, home)),
                other => panic!("expected Connected first, got {other:?}"),
            }
        }

        /// `dir` as the server names it.
        fn absolute(dir: &TempDir) -> String {
            fs::canonicalize(dir.path())
                .expect("canonicalize the temp dir")
                .display()
                .to_string()
        }

        /// The listing of `path`, which must succeed.
        fn listed(client: &Client, events: &mut UnboundedReceiver<Event>, path: &str) -> Listing {
            client.list(path.into());
            match next_within(events, 30) {
                Some(Event::Listed(listing)) => listing,
                other => panic!("expected Listed for {path}, got {other:?}"),
            }
        }

        /// Spec 0008, Model: the first event names the remote home folder,
        /// where relative paths start.
        #[test]
        fn the_first_event_is_connected_with_the_remote_home_folder() {
            let dir = TempDir::new("sftp-home");
            let Some((_client, _events, home)) = connect(dir.path()) else {
                return;
            };
            assert_eq!(home, absolute(&dir));
        }

        /// Spec 0008, flow 2 and Model: folders first, a symlink to a
        /// folder counts as one (and says it's a link), a broken link is
        /// `Other`, files have their size.
        #[test]
        fn a_listing_has_folders_first_links_resolved_and_file_sizes() {
            let dir = TempDir::new("sftp-listing");
            fs::create_dir(dir.path().join("docs")).expect("create docs");
            fs::write(dir.path().join("a.txt"), b"hello").expect("write a.txt");
            symlink("docs", dir.path().join("link-to-docs")).expect("link to docs");
            symlink("missing", dir.path().join("dangling")).expect("dangling link");
            let Some((client, mut events, _)) = connect(dir.path()) else {
                return;
            };
            let path = absolute(&dir);
            let listing = listed(&client, &mut events, &path);
            assert_eq!(listing.path, path);
            assert!(!listing.truncated);
            let shown: Vec<(&str, Kind, bool)> = listing
                .entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.kind, entry.link))
                .collect();
            assert_eq!(
                shown,
                [
                    ("docs", Kind::Folder, false),
                    ("link-to-docs", Kind::Folder, true),
                    ("a.txt", Kind::File, false),
                    ("dangling", Kind::Other, true),
                ]
            );
            assert_eq!(listing.entries[2].size, Some(5));
            assert!(listing.entries[2].modified.is_some());
        }

        /// Spec 0008 §Security: a name with a newline or control
        /// character is listed, its raw name kept for naming the remote
        /// file.
        #[test]
        fn a_name_with_control_characters_is_listed_as_it_is() {
            let dir = TempDir::new("sftp-names");
            let name = "bad\nname\x07\x1b[31m";
            fs::write(dir.path().join(name), b"").expect("write the odd file");
            let Some((client, mut events, _)) = connect(dir.path()) else {
                return;
            };
            let listing = listed(&client, &mut events, &absolute(&dir));
            let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, [name]);
        }

        /// Spec 0008, flow 2: a listing that fails says why, and the
        /// connection stays up for the next one.
        #[test]
        fn a_missing_folder_fails_its_listing_and_the_connection_keeps_working() {
            let dir = TempDir::new("sftp-missing");
            fs::write(dir.path().join("kept"), b"").expect("write kept");
            let Some((client, mut events, _)) = connect(dir.path()) else {
                return;
            };
            let missing = format!("{}/not-there", absolute(&dir));
            client.list(missing.clone());
            match next_within(&mut events, 10) {
                Some(Event::ListFailed { path, error }) => {
                    assert_eq!(path, missing);
                    assert!(!error.is_empty());
                }
                other => panic!("expected ListFailed, got {other:?}"),
            }
            let listing = listed(&client, &mut events, &absolute(&dir));
            assert_eq!(listing.entries.len(), 1);
            assert_eq!(listing.entries[0].name, "kept");
        }

        /// TM-sftp-17: a listing stops at 50,000 entries and says the rest
        /// was left out.
        #[test]
        fn a_folder_with_more_than_max_entries_lists_max_entries_and_says_so() {
            let dir = TempDir::new("sftp-cap");
            for ix in 0..=MAX_ENTRIES {
                fs::File::create(dir.path().join(ix.to_string())).expect("create a file");
            }
            let Some((client, mut events, _)) = connect(dir.path()) else {
                return;
            };
            let listing = listed(&client, &mut events, &absolute(&dir));
            assert!(listing.truncated);
            assert_eq!(listing.entries.len(), MAX_ENTRIES);
        }

        /// Spec 0008, flow 7 and AC10: an ssh that can't connect ends the
        /// connection with ssh's own reason, without hanging.
        #[test]
        fn an_ssh_that_cant_connect_ends_with_its_own_reason() {
            if !crate::ssh::installed() {
                eprintln!("skipped: no ssh");
                return;
            }
            let args = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=3"]
                .into_iter()
                .chain(["-s", "127.0.0.1", "sftp"])
                .map(String::from)
                .collect();
            let (_client, mut events) = Client::connect(Command {
                program: "ssh".into(),
                args,
                env: Default::default(),
            });
            match next_within(&mut events, 20) {
                Some(Event::Ended(reason)) => assert!(reason.contains("ssh"), "{reason:?}"),
                Some(Event::Connected { .. }) => {
                    eprintln!("skipped: something answers SFTP on 127.0.0.1");
                }
                other => panic!("expected Ended, got {other:?}"),
            }
        }
    }
}
