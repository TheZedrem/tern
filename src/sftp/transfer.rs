//! Downloads and uploads (spec 0008, flows 4 and 5), each a task on its
//! connection's thread. Reads and writes go out [`IN_FLIGHT`] at a time in
//! [`CHUNK`]s, so a long link stays busy and at most a mebibyte is held.
//! Both write to a `.tern-part` file next to the target and rename it into
//! place when complete: a cancel or a failure removes the part and never
//! touches a file the user confirmed replacing.

use super::client::{Download, Event, Upload, describe};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::FuturesOrdered;
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The bytes one read or write request asks for. OpenSSH answers a read of
/// at most 255 KiB, and a short answer stalls the pipeline.
const CHUNK: u32 = 64 * 1024;
/// Requests a transfer keeps in flight.
const IN_FLIGHT: usize = 16;
/// Progress reaches the sidebar at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// What a file being written is called until it's complete.
const PART: &str = ".tern-part";

/// Why a transfer stopped short.
enum Stop {
    Cancelled,
    Failed(String),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Stop::Failed(describe(&error))
    }
}

impl From<std::io::Error> for Stop {
    fn from(error: std::io::Error) -> Self {
        Stop::Failed(error.to_string())
    }
}

/// Runs `download` and reports how it ended.
pub(super) async fn download(
    sftp: Arc<RawSftpSession>,
    download: Download,
    cancel: Arc<AtomicBool>,
    events: UnboundedSender<Event>,
) {
    let id = download.id;
    let part = part_path(&download.target);
    // Removes the part however this ends short, even if the connection's
    // thread stops mid-way.
    let mut partial = Partial(Some(part.clone()));
    let mut progress = Progress::new(id, &events);
    let event = match fetch(&sftp, &download, &part, &cancel, &mut progress).await {
        Ok(Sent::Done) => {
            partial.0 = None;
            Event::TransferDone { id }
        }
        Ok(Sent::Exists) => {
            partial.0 = None;
            Event::Exists { id }
        }
        Err(Stop::Cancelled) => return,
        Err(Stop::Failed(error)) => Event::TransferFailed { id, error },
    };
    // The part is gone before anyone hears how it ended.
    drop(partial);
    let _ = events.unbounded_send(event);
}

async fn fetch(
    sftp: &RawSftpSession,
    download: &Download,
    part: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<Sent, Stop> {
    if !download.replace && download.target.exists() {
        return Ok(Sent::Exists);
    }
    let remote = download.remote.as_str();
    let handle = sftp
        .open(remote, OpenFlags::READ, FileAttributes::empty())
        .await?
        .handle;
    let read = read_into(sftp, &handle, part, cancel, progress).await;
    let _ = sftp.close(handle.as_str()).await;
    read?;
    let target = &download.target;
    // A file that turned up since the user was asked isn't theirs to lose.
    if !download.replace && target.exists() {
        return Err(Stop::Failed(format!("{} exists now", target.display())));
    }
    fs::rename(part, target)?;
    mark_from_internet(target);
    Ok(Sent::Done)
}

/// Reads the remote file `handle` into `part`, from its start. A short
/// answer asks again from where it ended; answers already on their way for
/// later offsets are dropped.
async fn read_into(
    sftp: &RawSftpSession,
    handle: &str,
    part: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), Stop> {
    progress.total = sftp
        .fstat(handle)
        .await
        .ok()
        .and_then(|attrs| attrs.attrs.size);
    let mut file = fs::File::create(part)?;
    let mut reads = FuturesOrdered::new();
    let (mut next, mut done, mut eof) = (0u64, 0u64, false);
    loop {
        while !eof && reads.len() < IN_FLIGHT {
            let offset = next;
            reads.push_back(async move { (offset, sftp.read(handle, offset, CHUNK).await) });
            next += u64::from(CHUNK);
        }
        let Some((offset, read)) = reads.next().await else {
            break;
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        if offset != done {
            continue;
        }
        match read {
            Ok(data) if !data.data.is_empty() => {
                file.write_all(&data.data)?;
                done += data.data.len() as u64;
                if data.data.len() < CHUNK as usize {
                    next = done;
                }
                progress.update(done);
            }
            Ok(_) => eof = true,
            Err(Error::Status(status)) if status.status_code == StatusCode::Eof => eof = true,
            Err(error) => return Err(error.into()),
        }
    }
    file.sync_all()?;
    progress.finish(done);
    Ok(())
}

/// Runs `upload` and reports how it ended; a done one changed its folder.
pub(super) async fn upload(
    sftp: Arc<RawSftpSession>,
    upload: Upload,
    cancel: Arc<AtomicBool>,
    events: UnboundedSender<Event>,
) {
    let id = upload.id;
    let mut progress = Progress::new(id, &events);
    let event = match send(&sftp, &upload, &cancel, &mut progress).await {
        Ok(Sent::Done) => {
            let _ = events.unbounded_send(Event::TransferDone { id });
            Event::Changed {
                folder: upload.folder,
            }
        }
        Ok(Sent::Exists) => Event::Exists { id },
        Err(Stop::Cancelled) => return,
        Err(Stop::Failed(error)) => Event::TransferFailed { id, error },
    };
    let _ = events.unbounded_send(event);
}

/// How a transfer that didn't fail ended.
enum Sent {
    Done,
    /// The target exists and wasn't to be replaced; nothing was written.
    Exists,
}

async fn send(
    sftp: &RawSftpSession,
    upload: &Upload,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<Sent, Stop> {
    let mut local = fs::File::open(&upload.local)?;
    let metadata = local.metadata()?;
    if metadata.is_dir() {
        return Err(Stop::Failed(
            "It's a folder; uploading folders isn't supported yet".into(),
        ));
    }
    progress.total = Some(metadata.len());
    let target = super::join(&upload.folder, &upload.name);
    let existing = sftp
        .stat(target.as_str())
        .await
        .ok()
        .map(|attrs| attrs.attrs);
    if existing.is_some() && !upload.replace {
        return Ok(Sent::Exists);
    }
    let part = super::join(&upload.folder, &format!(".{}{PART}", upload.name));
    let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE;
    let handle = sftp
        .open(part.as_str(), flags, FileAttributes::empty())
        .await?
        .handle;
    let written = write_from(sftp, &handle, &mut local, cancel, progress).await;
    let closed = sftp.close(handle.as_str()).await;
    let placed = match (written, closed) {
        (Err(stop), _) => Err(stop),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(_)) => place(sftp, &part, &target, existing).await,
    };
    if placed.is_err() {
        let _ = sftp.remove(part.as_str()).await;
    }
    placed.map(|()| Sent::Done)
}

/// Renames a complete `part` to `target`. A file it replaces lends it its
/// permissions, so an edited script stays executable.
async fn place(
    sftp: &RawSftpSession,
    part: &str,
    target: &str,
    existing: Option<FileAttributes>,
) -> Result<(), Stop> {
    if let Some(existing) = existing {
        if let Some(permissions) = existing.permissions {
            let attrs = FileAttributes {
                permissions: Some(permissions),
                ..FileAttributes::empty()
            };
            let _ = sftp.setstat(part, attrs).await;
        }
        // SFTP's rename won't replace a file: the old one goes first.
        sftp.remove(target).await?;
    }
    sftp.rename(part, target).await?;
    Ok(())
}

/// Writes `local` from its start to the remote file `handle`.
async fn write_from(
    sftp: &RawSftpSession,
    handle: &str,
    local: &mut fs::File,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), Stop> {
    let mut writes = FuturesOrdered::new();
    let (mut offset, mut eof) = (0u64, false);
    loop {
        while !eof && writes.len() < IN_FLIGHT {
            let chunk = read_chunk(local)?;
            if chunk.is_empty() {
                eof = true;
                break;
            }
            let (at, len) = (offset, chunk.len() as u64);
            offset += len;
            writes.push_back(async move { (at + len, sftp.write(handle, at, chunk).await) });
        }
        let Some((end, written)) = writes.next().await else {
            break;
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        written?;
        progress.update(end);
    }
    progress.finish(offset);
    Ok(())
}

/// The next [`CHUNK`] of `local`, shorter only at its end.
fn read_chunk(local: &mut fs::File) -> std::io::Result<Vec<u8>> {
    let mut chunk = vec![0; CHUNK as usize];
    let mut filled = 0;
    while filled < chunk.len() {
        match local.read(&mut chunk[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    chunk.truncate(filled);
    Ok(chunk)
}

/// `target` while it's being written: `<name>.tern-part` beside it.
fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(PART);
    target.with_file_name(name)
}

/// A local part file, removed when dropped unless it was finished.
struct Partial(Option<PathBuf>);

impl Drop for Partial {
    fn drop(&mut self) {
        if let Some(part) = &self.0 {
            let _ = fs::remove_file(part);
        }
    }
}

/// Marks a download as from the internet, so SmartScreen and Office's
/// Protected View treat it as any other download (TM-sftp-07). Best effort:
/// a file system without alternate streams can't hold the mark.
fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        let _ = fs::write(stream, b"[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// A transfer's progress events, at most one per [`PROGRESS_EVERY`].
struct Progress<'a> {
    id: u64,
    total: Option<u64>,
    events: &'a UnboundedSender<Event>,
    sent: Option<Instant>,
}

impl<'a> Progress<'a> {
    fn new(id: u64, events: &'a UnboundedSender<Event>) -> Self {
        Self {
            id,
            total: None,
            events,
            sent: None,
        }
    }

    fn update(&mut self, done: u64) {
        if self
            .sent
            .is_none_or(|sent| sent.elapsed() >= PROGRESS_EVERY)
        {
            self.finish(done);
        }
    }

    fn finish(&mut self, done: u64) {
        self.sent = Some(Instant::now());
        let _ = self.events.unbounded_send(Event::Progress {
            id: self.id,
            done,
            total: self.total,
        });
    }
}
