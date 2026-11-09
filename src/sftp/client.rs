//! A pane's SFTP connection (spec 0008, Model): `ssh … -s <host> sftp`
//! with the `russh-sftp` client on its stdio, on a thread of its own with a
//! current-thread tokio runtime. The browser sends [`Request`]s and draws
//! the [`Event`]s that come back; nothing here runs on the UI thread.
//! Dropping the [`Client`] ends the thread, which kills its ssh.

use super::transfer;
pub use crate::files::Event;
use crate::files::{Entry, Kind, Listing, MAX_ENTRIES};
use crate::terminal::clean_prompt;
use crate::terminal::pty::Command;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::join_all;
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error;
use russh_sftp::protocol::{File, FileAttributes, StatusCode};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// What the sidebar asks of its connection. Paths are absolute remote
/// paths; a transfer's ID is the sidebar's, for its events and its cancel.
enum Request {
    List(String),
    Download(Download),
    Upload(Upload),
    /// Stop a transfer and remove what it wrote (spec 0008, flows 4, 5).
    Cancel(u64),
    /// Rename `from` to `to`; an existing `to` only with `replace`, else
    /// it's reported (spec 0013, flow 5).
    Rename {
        from: String,
        to: String,
        replace: bool,
    },
    /// Remove a file, a link, or an empty folder (`folder`).
    Delete {
        path: String,
        folder: bool,
    },
    MakeFolder(String),
}

/// A download of remote file `remote` to local `target`, written to
/// `part` next to it until it's complete (spec 0008, flow 4).
#[derive(Debug)]
pub struct Download {
    pub id: u64,
    pub remote: String,
    pub target: PathBuf,
    /// `target` may be replaced: the user confirmed it.
    pub replace: bool,
    /// An edit copy: its folders are made private first, and it's only the
    /// user's to read (spec 0008, Model).
    pub private: bool,
}

/// An upload of local file `local` into remote `folder` as `name` (spec
/// 0008, flow 5).
#[derive(Debug)]
pub struct Upload {
    pub id: u64,
    pub local: PathBuf,
    pub folder: String,
    pub name: String,
    /// An existing remote file may be replaced: the user confirmed it.
    pub replace: bool,
    /// An edit's upload: the remote file must still have this size and
    /// time, else nothing is written and [`Event::Exists`] says so (flow 3).
    pub expect: Option<RemoteStat>,
}

/// A remote file's size and modification time, which tell an edit whether
/// someone changed the host's copy since it was fetched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteStat {
    pub size: Option<u64>,
    pub mtime: Option<u32>,
}

impl RemoteStat {
    pub(super) fn of(attrs: &FileAttributes) -> Self {
        Self {
            size: attrs.size,
            mtime: attrs.mtime,
        }
    }
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
        self.send(Request::List(path));
    }

    pub fn download(&self, download: Download) {
        self.send(Request::Download(download));
    }

    pub fn upload(&self, upload: Upload) {
        self.send(Request::Upload(upload));
    }

    pub fn cancel(&self, id: u64) {
        self.send(Request::Cancel(id));
    }

    /// Renames `from` to `to`. Unless `replace`, an existing `to` is left
    /// alone and [`Event::RenameExists`] says so.
    pub fn rename(&self, from: String, to: String, replace: bool) {
        self.send(Request::Rename { from, to, replace });
    }

    pub fn delete(&self, path: String, folder: bool) {
        self.send(Request::Delete { path, folder });
    }

    pub fn make_folder(&self, path: String) {
        self.send(Request::MakeFolder(path));
    }

    fn send(&self, request: Request) {
        let _ = self.requests.send(request);
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
    let sftp = Arc::new(RawSftpSession::new_with_config(
        tokio::io::join(stdout, stdin),
        config,
    ));
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
    ssh.creation_flags(crate::ssh::CREATE_NO_WINDOW);
    ssh.spawn()
}

/// Logs in, reports the home folder, then answers requests until the
/// sidebar goes away (`Ok`) or the connection fails (`Err` with what
/// failed, for when ssh says nothing). Transfers run as tasks of their
/// own, so listing goes on while they do.
async fn serve(
    sftp: &Arc<RawSftpSession>,
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
    let mut cancels: HashMap<u64, Arc<AtomicBool>> = HashMap::new();
    loop {
        let request = tokio::select! {
            request = requests.recv() => request,
            exit = ssh.wait() => return Err(exited(exit)),
        };
        let Some(request) = request else {
            return Ok(());
        };
        let event = match request {
            Request::List(path) => match list(sftp, &path).await {
                Ok(listing) => Event::Listed(listing),
                Err(Error::Status(status)) if status.status_code != StatusCode::ConnectionLost => {
                    Event::ListFailed {
                        path,
                        error: status_message(&status.error_message, status.status_code),
                    }
                }
                Err(error) => return Err(describe(&error)),
            },
            Request::Download(download) => {
                let cancel = cancels.entry(download.id).or_default().clone();
                let task = transfer::download(Arc::clone(sftp), download, cancel, events.clone());
                tokio::spawn(task);
                continue;
            }
            Request::Upload(upload) => {
                let cancel = cancels.entry(upload.id).or_default().clone();
                let task = transfer::upload(Arc::clone(sftp), upload, cancel, events.clone());
                tokio::spawn(task);
                continue;
            }
            Request::Cancel(id) => {
                if let Some(cancel) = cancels.remove(&id) {
                    cancel.store(true, Ordering::Relaxed);
                }
                continue;
            }
            Request::Rename { from, to, replace } => {
                // The look and the rename are two requests; a name taken in
                // between is replaced or refused as the server does.
                let taken = sftp.lstat(to.as_str()).await.is_ok();
                if taken && !replace {
                    Event::RenameExists { from, to }
                } else {
                    changed(rename(sftp, &from, &to, taken).await, &super::parent(&to))
                }
            }
            Request::Delete { path, folder } => {
                let done = if folder {
                    sftp.rmdir(path.as_str()).await
                } else {
                    sftp.remove(path.as_str()).await
                };
                changed(done.map(drop), &super::parent(&path))
            }
            Request::MakeFolder(path) => {
                let done = sftp.mkdir(path.as_str(), FileAttributes::empty()).await;
                changed(done.map(drop), &super::parent(&path))
            }
        };
        if events.unbounded_send(event).is_err() {
            return Ok(());
        }
    }
}

/// Renames `from` to `to`. SFTP's rename won't replace a file, so a
/// `taken` one the user agreed to replace goes first.
async fn rename(sftp: &RawSftpSession, from: &str, to: &str, taken: bool) -> Result<(), Error> {
    if taken {
        sftp.remove(to).await?;
    }
    sftp.rename(from, to).await.map(drop)
}

/// What a rename, delete or new folder in `folder` reports.
fn changed(done: Result<(), Error>, folder: &str) -> Event {
    match done {
        Ok(()) => Event::Changed {
            folder: folder.to_owned(),
        },
        Err(error) => Event::Failed(describe(&error)),
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
                    .filter(|file| !crate::files::hidden(&file.filename));
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
    crate::files::sort(&mut entries);
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
pub(super) fn describe(error: &Error) -> String {
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
                // Port 1: no SSH server answers there, even on a CI runner
                // that runs one on 22.
                .chain(["-p", "1", "-s", "127.0.0.1", "sftp"])
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

        /// `len` bytes that differ from offset to offset, so a chunk
        /// written at the wrong place shows.
        fn pattern(len: usize) -> Vec<u8> {
            (0..len).map(|ix| (ix % 251) as u8).collect()
        }

        /// The events up to and including the first one `last` accepts,
        /// within `seconds` each.
        fn events_until(
            events: &mut UnboundedReceiver<Event>,
            seconds: u64,
            last: impl Fn(&Event) -> bool,
        ) -> Vec<Event> {
            let mut seen = Vec::new();
            loop {
                let event = next_within(events, seconds).expect("the connection ended");
                let done = last(&event);
                seen.push(event);
                if done {
                    return seen;
                }
            }
        }

        /// The event that ends transfer `id`, with the events before it.
        fn transfer_end(events: &mut UnboundedReceiver<Event>, id: u64) -> Vec<Event> {
            events_until(events, 30, |event| match event {
                Event::TransferDone { id: done, .. }
                | Event::TransferFailed { id: done, .. }
                | Event::Exists { id: done } => *done == id,
                _ => false,
            })
        }

        /// Waits up to `seconds` for `gone` to hold.
        fn eventually(seconds: u64, gone: impl Fn() -> bool) -> bool {
            let until = std::time::Instant::now() + Duration::from_secs(seconds);
            while std::time::Instant::now() < until {
                if gone() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            gone()
        }

        /// Names of the files in `dir` ending in `.tern-part`.
        fn parts(dir: &Path) -> Vec<String> {
            fs::read_dir(dir)
                .expect("read the dir")
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| name.ends_with(".tern-part"))
                .collect()
        }

        /// The one event a rename, delete or new folder answers with.
        fn change(events: &mut UnboundedReceiver<Event>) -> Event {
            next_within(events, 10).expect("the connection ended")
        }

        /// Spec 0008, flow 4 and AC4: a download arrives byte-identical,
        /// with progress ending at the file's size, and leaves no part.
        #[test]
        fn a_download_arrives_byte_identical_with_progress_and_no_part_left() {
            let remote = TempDir::new("sftp-dl-remote");
            let local = TempDir::new("sftp-dl-local");
            // Not a multiple of the 64 KiB request size.
            let data = pattern(3 * 1024 * 1024 + 12_345);
            fs::write(remote.path().join("big.bin"), &data).expect("write big.bin");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let target = local.path().join("big.bin");
            client.download(Download {
                id: 1,
                remote: format!("{}/big.bin", absolute(&remote)),
                target: target.clone(),
                replace: false,
                private: false,
            });
            let seen = transfer_end(&mut events, 1);
            assert!(
                matches!(seen.last(), Some(Event::TransferDone { id: 1, .. })),
                "{:?}",
                seen.last()
            );
            let progress: Vec<(u64, Option<u64>)> = seen
                .iter()
                .filter_map(|event| match event {
                    Event::Progress { id: 1, done, total } => Some((*done, *total)),
                    _ => None,
                })
                .collect();
            let size = data.len() as u64;
            assert_eq!(progress.last(), Some(&(size, Some(size))), "{progress:?}");
            assert!(fs::read(&target).expect("read the download") == data);
            assert_eq!(parts(local.path()), Vec::<String>::new());
        }

        /// Spec 0008, flow 4 and TM-sftp-08: an existing local file isn't
        /// replaced without the confirmation; it's reported instead.
        #[test]
        fn a_download_onto_an_existing_file_reports_exists_and_leaves_it() {
            let remote = TempDir::new("sftp-dl-exists-remote");
            let local = TempDir::new("sftp-dl-exists-local");
            fs::write(remote.path().join("a.txt"), b"remote").expect("write remote");
            let target = local.path().join("a.txt");
            fs::write(&target, b"mine").expect("write local");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.download(Download {
                id: 2,
                remote: format!("{}/a.txt", absolute(&remote)),
                target: target.clone(),
                replace: false,
                private: false,
            });
            let seen = transfer_end(&mut events, 2);
            assert!(
                matches!(seen.last(), Some(Event::Exists { id: 2 })),
                "{seen:?}"
            );
            assert_eq!(fs::read(&target).expect("read local"), b"mine");
            assert_eq!(parts(local.path()), Vec::<String>::new());
        }

        /// Spec 0008, flow 4: once confirmed, the download replaces it.
        #[test]
        fn a_confirmed_download_replaces_the_existing_file() {
            let remote = TempDir::new("sftp-dl-replace-remote");
            let local = TempDir::new("sftp-dl-replace-local");
            fs::write(remote.path().join("a.txt"), b"remote").expect("write remote");
            let target = local.path().join("a.txt");
            fs::write(&target, b"mine, and longer than remote").expect("write local");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.download(Download {
                id: 3,
                remote: format!("{}/a.txt", absolute(&remote)),
                target: target.clone(),
                replace: true,
                private: false,
            });
            let seen = transfer_end(&mut events, 3);
            assert!(
                matches!(seen.last(), Some(Event::TransferDone { id: 3, .. })),
                "{seen:?}"
            );
            assert_eq!(fs::read(&target).expect("read local"), b"remote");
        }

        /// Spec 0008, flow 4 and TM-sftp-18: a download that fails says
        /// why and leaves no part file (here: the remote is a folder).
        #[test]
        fn a_failed_download_says_why_and_leaves_no_part() {
            let remote = TempDir::new("sftp-dl-fail-remote");
            let local = TempDir::new("sftp-dl-fail-local");
            fs::create_dir(remote.path().join("folder")).expect("create folder");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let target = local.path().join("folder");
            client.download(Download {
                id: 4,
                remote: format!("{}/folder", absolute(&remote)),
                target: target.clone(),
                replace: false,
                private: false,
            });
            let seen = transfer_end(&mut events, 4);
            match seen.last() {
                Some(Event::TransferFailed { error, .. }) => assert!(!error.is_empty()),
                other => panic!("expected TransferFailed, got {other:?}"),
            }
            assert!(!target.exists());
            // The part goes as the task ends, which can be just after its
            // event.
            assert!(
                eventually(5, || parts(local.path()).is_empty()),
                "{:?}",
                parts(local.path())
            );
        }

        /// Spec 0008, flow 4 and AC4: a cancelled download reports no
        /// end, and its part file goes away; the target never appears.
        #[test]
        fn a_cancelled_download_removes_its_part_and_reports_no_end() {
            let remote = TempDir::new("sftp-dl-cancel-remote");
            let local = TempDir::new("sftp-dl-cancel-local");
            fs::write(remote.path().join("huge.bin"), vec![0u8; 64 * 1024 * 1024])
                .expect("write huge.bin");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let target = local.path().join("huge.bin");
            client.download(Download {
                id: 5,
                remote: format!("{}/huge.bin", absolute(&remote)),
                target: target.clone(),
                replace: false,
                private: false,
            });
            events_until(&mut events, 10, |event| {
                matches!(event, Event::Progress { id: 5, .. })
            });
            client.cancel(5);
            let part = local.path().join("huge.bin.tern-part");
            assert!(
                eventually(5, || !part.exists() && !target.exists()),
                "part or target still there"
            );
            // A list round-trip: anything the transfer sent is ahead of it.
            client.list(absolute(&remote));
            let seen = events_until(&mut events, 10, |event| matches!(event, Event::Listed(_)));
            assert!(
                !seen.iter().any(|event| matches!(
                    event,
                    Event::TransferDone { id: 5, .. } | Event::TransferFailed { id: 5, .. }
                )),
                "{seen:?}"
            );
            assert!(!target.exists());
        }

        /// Spec 0008, flow 5: an upload arrives byte-identical, without a
        /// part file left, and its folder is reported changed after it.
        #[test]
        fn an_upload_arrives_byte_identical_and_its_folder_changes() {
            let remote = TempDir::new("sftp-up-remote");
            let local = TempDir::new("sftp-up-local");
            let data = pattern(2 * 1024 * 1024 + 777);
            let source = local.path().join("up.bin");
            fs::write(&source, &data).expect("write up.bin");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.upload(Upload {
                id: 6,
                local: source,
                folder: folder.clone(),
                name: "up.bin".into(),
                replace: false,
                expect: None,
            });
            let seen = transfer_end(&mut events, 6);
            assert!(
                matches!(seen.last(), Some(Event::TransferDone { id: 6, .. })),
                "{:?}",
                seen.last()
            );
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(fs::read(remote.path().join("up.bin")).expect("read upload") == data);
            assert_eq!(parts(remote.path()), Vec::<String>::new());
        }

        /// Spec 0008, flow 5 and AC9: an existing remote file isn't
        /// replaced without the confirmation.
        #[test]
        fn an_upload_onto_an_existing_remote_file_reports_exists_and_leaves_it() {
            let remote = TempDir::new("sftp-up-exists-remote");
            let local = TempDir::new("sftp-up-exists-local");
            fs::write(remote.path().join("a.txt"), b"theirs").expect("write remote");
            let source = local.path().join("a.txt");
            fs::write(&source, b"mine").expect("write local");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.upload(Upload {
                id: 7,
                local: source,
                folder: absolute(&remote),
                name: "a.txt".into(),
                replace: false,
                expect: None,
            });
            let seen = transfer_end(&mut events, 7);
            assert!(
                matches!(seen.last(), Some(Event::Exists { id: 7 })),
                "{seen:?}"
            );
            assert_eq!(
                fs::read(remote.path().join("a.txt")).expect("read"),
                b"theirs"
            );
            assert_eq!(parts(remote.path()), Vec::<String>::new());
        }

        /// Spec 0008, flow 5: a confirmed upload replaces the remote file
        /// and keeps its permissions, so a script stays executable.
        #[test]
        fn a_confirmed_upload_replaces_the_file_and_keeps_its_permissions() {
            use std::os::unix::fs::PermissionsExt;
            let remote = TempDir::new("sftp-up-replace-remote");
            let local = TempDir::new("sftp-up-replace-local");
            let script = remote.path().join("run.sh");
            fs::write(&script, b"#!/bin/sh\necho old\n").expect("write remote");
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
            let source = local.path().join("run.sh");
            fs::write(&source, b"#!/bin/sh\necho new\n").expect("write local");
            fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).expect("chmod");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.upload(Upload {
                id: 8,
                local: source,
                folder: absolute(&remote),
                name: "run.sh".into(),
                replace: true,
                expect: None,
            });
            let seen = transfer_end(&mut events, 8);
            assert!(
                matches!(seen.last(), Some(Event::TransferDone { id: 8, .. })),
                "{seen:?}"
            );
            assert_eq!(fs::read(&script).expect("read"), b"#!/bin/sh\necho new\n");
            let mode = fs::metadata(&script).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
            assert_eq!(parts(remote.path()), Vec::<String>::new());
        }

        /// Spec 0008, flow 5, AC9 and TM-sftp-11: a folder isn't uploaded;
        /// the transfer fails saying why and nothing is written.
        #[test]
        fn uploading_a_folder_fails_saying_why_and_writes_nothing() {
            let remote = TempDir::new("sftp-up-folder-remote");
            let local = TempDir::new("sftp-up-folder-local");
            let source = local.path().join("docs");
            fs::create_dir(&source).expect("create docs");
            fs::write(source.join("inside.txt"), b"x").expect("write inside");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.upload(Upload {
                id: 9,
                local: source,
                folder: absolute(&remote),
                name: "docs".into(),
                replace: false,
                expect: None,
            });
            let seen = transfer_end(&mut events, 9);
            match seen.last() {
                Some(Event::TransferFailed { error, .. }) => {
                    assert!(error.contains("folder"), "{error:?}")
                }
                other => panic!("expected TransferFailed, got {other:?}"),
            }
            let written: Vec<_> = fs::read_dir(remote.path()).expect("read").collect();
            assert!(written.is_empty(), "{written:?}");
        }

        /// TM-sftp-11: a symlink to a file uploads its target's contents.
        #[test]
        fn uploading_a_symlink_sends_its_targets_contents() {
            let remote = TempDir::new("sftp-up-link-remote");
            let local = TempDir::new("sftp-up-link-local");
            fs::write(local.path().join("real.txt"), b"the contents").expect("write real");
            let link = local.path().join("link.txt");
            symlink("real.txt", &link).expect("link");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.upload(Upload {
                id: 10,
                local: link,
                folder: absolute(&remote),
                name: "link.txt".into(),
                replace: false,
                expect: None,
            });
            let seen = transfer_end(&mut events, 10);
            assert!(
                matches!(seen.last(), Some(Event::TransferDone { id: 10, .. })),
                "{seen:?}"
            );
            let uploaded = remote.path().join("link.txt");
            assert!(!fs::symlink_metadata(&uploaded).expect("stat").is_symlink());
            assert_eq!(fs::read(&uploaded).expect("read"), b"the contents");
        }

        /// Spec 0008, flow 5: a cancelled upload removes its remote part,
        /// and the target never appears.
        #[test]
        fn a_cancelled_upload_removes_its_remote_part() {
            let remote = TempDir::new("sftp-up-cancel-remote");
            let local = TempDir::new("sftp-up-cancel-local");
            let source = local.path().join("huge.bin");
            fs::write(&source, vec![0u8; 64 * 1024 * 1024]).expect("write huge.bin");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.upload(Upload {
                id: 11,
                local: source,
                folder: absolute(&remote),
                name: "huge.bin".into(),
                replace: false,
                expect: None,
            });
            events_until(&mut events, 10, |event| {
                matches!(event, Event::Progress { id: 11, .. })
            });
            client.cancel(11);
            let part = remote.path().join(".huge.bin.tern-part");
            let target = remote.path().join("huge.bin");
            assert!(
                eventually(5, || !part.exists() && !target.exists()),
                "part or target still there"
            );
            client.list(absolute(&remote));
            let seen = events_until(&mut events, 10, |event| matches!(event, Event::Listed(_)));
            assert!(
                !seen.iter().any(|event| matches!(
                    event,
                    Event::TransferDone { id: 11, .. } | Event::TransferFailed { id: 11, .. }
                )),
                "{seen:?}"
            );
            assert!(!target.exists());
        }

        /// Spec 0008, flow 6: a rename reports its folder changed.
        #[test]
        fn a_rename_changes_its_folder() {
            let remote = TempDir::new("sftp-rename");
            fs::write(remote.path().join("old.txt"), b"x").expect("write old");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.rename(
                format!("{folder}/old.txt"),
                format!("{folder}/new.txt"),
                false,
            );
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(!remote.path().join("old.txt").exists());
            assert_eq!(fs::read(remote.path().join("new.txt")).expect("read"), b"x");
        }

        /// Spec 0013, flow 5 and AC7: on an SFTP target, renaming onto an
        /// existing name is reported, not done, so the browser can ask.
        #[test]
        fn a_rename_onto_an_existing_name_reports_it_and_changes_nothing() {
            let remote = TempDir::new("sftp-rename-exists");
            fs::write(remote.path().join("a.txt"), b"a").expect("write a");
            fs::write(remote.path().join("b.txt"), b"b").expect("write b");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            let (from, to) = (format!("{folder}/a.txt"), format!("{folder}/b.txt"));
            client.rename(from.clone(), to.clone(), false);
            match change(&mut events) {
                Event::RenameExists {
                    from: reported_from,
                    to: reported_to,
                } => {
                    assert_eq!(reported_from, from);
                    assert_eq!(reported_to, to);
                }
                other => panic!("expected RenameExists, got {other:?}"),
            }
            assert_eq!(fs::read(remote.path().join("a.txt")).expect("read a"), b"a");
            assert_eq!(fs::read(remote.path().join("b.txt")).expect("read b"), b"b");
        }

        /// Spec 0013, flow 5: once confirmed, the rename replaces the
        /// existing remote file and its folder changes.
        #[test]
        fn a_confirmed_rename_replaces_the_existing_file() {
            let remote = TempDir::new("sftp-rename-replace");
            fs::write(remote.path().join("a.txt"), b"a").expect("write a");
            fs::write(remote.path().join("b.txt"), b"b").expect("write b");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.rename(format!("{folder}/a.txt"), format!("{folder}/b.txt"), true);
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(!remote.path().join("a.txt").exists());
            assert_eq!(fs::read(remote.path().join("b.txt")).expect("read b"), b"a");
        }

        /// Spec 0008, flow 6: a new folder reports its parent changed.
        #[test]
        fn a_new_folder_changes_its_parent() {
            let remote = TempDir::new("sftp-mkdir");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.make_folder(format!("{folder}/fresh"));
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(remote.path().join("fresh").is_dir());
        }

        /// Spec 0008, flow 6: deleting a file reports its folder changed.
        #[test]
        fn deleting_a_file_changes_its_folder() {
            let remote = TempDir::new("sftp-rm-file");
            fs::write(remote.path().join("gone.txt"), b"x").expect("write");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.delete(format!("{folder}/gone.txt"), false);
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(!remote.path().join("gone.txt").exists());
        }

        /// Spec 0008, flow 6: deleting an empty folder reports its parent
        /// changed.
        #[test]
        fn deleting_an_empty_folder_changes_its_parent() {
            let remote = TempDir::new("sftp-rm-empty");
            fs::create_dir(remote.path().join("empty")).expect("create");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.delete(format!("{folder}/empty"), true);
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(!remote.path().join("empty").exists());
        }

        /// Spec 0008, flow 6: a folder with anything in it isn't deleted;
        /// it fails with the server's reason and its contents stay.
        #[test]
        fn deleting_a_folder_with_contents_fails_and_keeps_them() {
            let remote = TempDir::new("sftp-rm-full");
            fs::create_dir(remote.path().join("full")).expect("create");
            fs::write(remote.path().join("full/kept.txt"), b"x").expect("write");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            client.delete(format!("{}/full", absolute(&remote)), true);
            match change(&mut events) {
                Event::Failed(reason) => assert!(!reason.is_empty()),
                other => panic!("expected Failed, got {other:?}"),
            }
            assert!(remote.path().join("full/kept.txt").exists());
        }

        /// TM-sftp-10: deleting a link to a folder removes the link, never
        /// what it points to.
        #[test]
        fn deleting_a_link_to_a_folder_removes_only_the_link() {
            let remote = TempDir::new("sftp-rm-link");
            fs::create_dir(remote.path().join("target")).expect("create");
            fs::write(remote.path().join("target/kept.txt"), b"x").expect("write");
            symlink("target", remote.path().join("link")).expect("link");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let folder = absolute(&remote);
            client.delete(format!("{folder}/link"), false);
            match change(&mut events) {
                Event::Changed { folder: changed } => assert_eq!(changed, folder),
                other => panic!("expected Changed, got {other:?}"),
            }
            assert!(fs::symlink_metadata(remote.path().join("link")).is_err());
            assert!(remote.path().join("target/kept.txt").exists());
        }

        /// Fetches `name` from `remote` to `target` as edit `id` does, and
        /// returns the stat its `TransferDone` carries.
        fn fetch(
            client: &Client,
            events: &mut UnboundedReceiver<Event>,
            id: u64,
            remote: &TempDir,
            target: &Path,
            private: bool,
        ) -> RemoteStat {
            client.download(Download {
                id,
                remote: format!("{}/{}", absolute(remote), ".env"),
                target: target.to_path_buf(),
                replace: true,
                private,
            });
            match transfer_end(events, id).pop() {
                Some(Event::TransferDone {
                    stat: Some(stat), ..
                }) => stat,
                other => panic!("expected TransferDone with a stat, got {other:?}"),
            }
        }

        /// The edit upload of `local` over `.env` in `remote`, expecting
        /// `expect`; the event that ends it.
        fn upload_edit(
            client: &Client,
            events: &mut UnboundedReceiver<Event>,
            id: u64,
            remote: &TempDir,
            local: &Path,
            expect: Option<RemoteStat>,
        ) -> Event {
            client.upload(Upload {
                id,
                local: local.to_path_buf(),
                folder: absolute(remote),
                name: ".env".into(),
                replace: true,
                expect,
            });
            transfer_end(events, id).pop().expect("an end")
        }

        /// Spec 0008, Model and TM-sftp-13: an edit's fetch carries the
        /// remote's size and time, and its copy is the user's alone: the
        /// file `0600` (no execute bit), the folders it made `0700`.
        #[test]
        fn an_edit_fetch_carries_the_remote_stat_and_its_copy_is_private() {
            use std::os::unix::fs::PermissionsExt;
            let remote = TempDir::new("sftp-edit-fetch-remote");
            let local = TempDir::new("sftp-edit-fetch-local");
            fs::write(remote.path().join(".env"), b"SECRET=hunter2\n").expect("write remote");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let copy = local.path().join("session/7/.env");
            let stat = fetch(&client, &mut events, 20, &remote, &copy, true);
            assert_eq!(stat.size, Some(b"SECRET=hunter2\n".len() as u64));
            assert!(stat.mtime.is_some(), "{stat:?}");
            assert_eq!(fs::read(&copy).expect("read the copy"), b"SECRET=hunter2\n");
            let mode = |path: &Path| fs::metadata(path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode(&copy), 0o600);
            assert_eq!(mode(&local.path().join("session/7")), 0o700);
            assert_eq!(mode(&local.path().join("session")), 0o700);
        }

        /// Spec 0008, flow 3: an edit's upload replaces the host's file
        /// still as fetched, and carries its new stat for the next upload.
        #[test]
        fn an_edit_upload_replaces_the_unchanged_remote_and_carries_its_new_stat() {
            let remote = TempDir::new("sftp-edit-up-remote");
            let local = TempDir::new("sftp-edit-up-local");
            fs::write(remote.path().join(".env"), b"A=1\n").expect("write remote");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let copy = local.path().join(".env");
            let fetched = fetch(&client, &mut events, 21, &remote, &copy, false);
            fs::write(&copy, b"A=1\nB=22\n").expect("save the copy");
            match upload_edit(&client, &mut events, 22, &remote, &copy, Some(fetched)) {
                Event::TransferDone {
                    stat: Some(stat), ..
                } => assert_eq!(stat.size, Some(b"A=1\nB=22\n".len() as u64)),
                other => panic!("expected TransferDone with a stat, got {other:?}"),
            }
            assert_eq!(
                fs::read(remote.path().join(".env")).expect("read remote"),
                b"A=1\nB=22\n"
            );
        }

        /// Spec 0008 AC6 and TM-sftp-09: when the host's file changed since
        /// the fetch, the edit's upload writes nothing and says so; once
        /// the user confirms (no `expect`), it overwrites.
        #[test]
        fn an_edit_upload_over_a_changed_remote_reports_exists_until_confirmed() {
            let remote = TempDir::new("sftp-edit-conflict-remote");
            let local = TempDir::new("sftp-edit-conflict-local");
            let theirs = remote.path().join(".env");
            fs::write(&theirs, b"A=1\n").expect("write remote");
            let Some((client, mut events, _)) = connect(remote.path()) else {
                return;
            };
            let copy = local.path().join(".env");
            let fetched = fetch(&client, &mut events, 23, &remote, &copy, false);
            fs::write(&theirs, b"A=1\nTHEIRS=yes\n").expect("change the remote");
            fs::write(&copy, b"A=1\nMINE=yes\n").expect("save the copy");
            let ended = upload_edit(&client, &mut events, 24, &remote, &copy, Some(fetched));
            assert!(matches!(ended, Event::Exists { id: 24 }), "{ended:?}");
            assert_eq!(
                fs::read(&theirs).expect("read remote"),
                b"A=1\nTHEIRS=yes\n"
            );
            assert_eq!(parts(remote.path()), Vec::<String>::new());
            let ended = upload_edit(&client, &mut events, 25, &remote, &copy, None);
            assert!(
                matches!(ended, Event::TransferDone { id: 25, .. }),
                "{ended:?}"
            );
            assert_eq!(fs::read(&theirs).expect("read remote"), b"A=1\nMINE=yes\n");
        }
    }
}
