//! Where SFTP edit copies live (spec 0008, Model; TM-sftp-13):
//! `<data_local_dir>/tern/edit/<run>/<session>/<edit>/<name>`, folders
//! `0700`, files `0600`. Each running Tern holds a lock on `<run>.lock`
//! for as long as it lives; the sweep after the first frame removes only
//! runs whose lock is free, so copies of another open Tern are safe.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// This run: its folder's name, and its lock once taken.
struct Run {
    id: String,
    lock: Mutex<Option<File>>,
}

static RUN: OnceLock<Run> = OnceLock::new();
static SESSIONS: AtomicU64 = AtomicU64::new(0);

fn run() -> &'static Run {
    RUN.get_or_init(|| {
        let mut bytes = [0u8; 8];
        // A clash with a live run would need 64 equal random bits.
        let id = match getrandom::fill(&mut bytes) {
            Ok(()) => u64::from_le_bytes(bytes),
            Err(_) => u64::from(std::process::id()),
        };
        Run {
            id: format!("{id:016x}"),
            lock: Mutex::new(None),
        }
    })
}

/// `<data_local_dir>/tern/edit`, where every run's copies are.
fn root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("tern").join("edit"))
}

/// A new folder for one SFTP session's copies. Nothing is created yet:
/// that waits for its first edit ([`prepare`]).
pub fn session_dir() -> Option<PathBuf> {
    let session = SESSIONS.fetch_add(1, Ordering::Relaxed);
    Some(root()?.join(&run().id).join(session.to_string()))
}

/// Makes the folders above `copy`, private to the user, and takes this
/// run's lock first, so a sweep never takes a run that is writing. Runs
/// on the connection's thread.
pub fn prepare(copy: &Path) -> io::Result<()> {
    let root = root().ok_or_else(|| io::Error::other("there's no data folder"))?;
    private_dirs(&root)?;
    {
        let run = run();
        let mut lock = run
            .lock
            .lock()
            .map_err(|_| io::Error::other("lock poisoned"))?;
        if lock.is_none() {
            let file = File::create(root.join(format!("{}.lock", run.id)))?;
            file.try_lock().map_err(|error| match error {
                TryLockError::Error(error) => error,
                TryLockError::WouldBlock => io::Error::other("another Tern holds this run"),
            })?;
            *lock = Some(file);
        }
    }
    match copy.parent() {
        Some(parent) => private_dirs(parent),
        None => Ok(()),
    }
}

/// Removes a session's or an edit's copies, off the UI thread. Best
/// effort: a copy an app holds open on Windows stays until the next
/// sweep.
pub fn remove_dir(dir: PathBuf) {
    std::thread::spawn(move || {
        let _ = fs::remove_dir_all(dir);
    });
}

/// Removes the copies of runs that ended without cleaning up: a crash, a
/// kill. A run whose lock can be taken has no Tern left holding it.
/// Blocks on the file system; call it off the UI thread.
pub fn sweep() {
    if let Some(root) = root() {
        sweep_in(&root, &run().id);
    }
}

/// [`sweep`] of the runs in `root`, but never run `own`.
pub fn sweep_in(root: &Path, own: &str) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Only a run's lock says it's over: a folder without one is left
        // alone, in case its run is between taking the lock and writing.
        let Some(id) = name.strip_suffix(".lock").filter(|id| *id != own) else {
            continue;
        };
        let free = File::options()
            .write(true)
            .open(&path)
            .is_ok_and(|file| file.try_lock().is_ok());
        if free {
            let _ = fs::remove_dir_all(root.join(id));
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(unix)]
fn private_dirs(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn private_dirs(dir: &Path) -> io::Result<()> {
    // `%LOCALAPPDATA%` is the user's alone by its ACL.
    fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;

    /// A run `id` in `root` that ended: its copies and its lock file.
    fn run_in(root: &Path, id: &str) {
        let copy = root.join(id).join("0").join("1");
        fs::create_dir_all(&copy).expect("create the run's folders");
        fs::write(copy.join(".env"), b"SECRET=1").expect("write a copy");
        File::create(root.join(format!("{id}.lock"))).expect("create the lock file");
    }

    /// A live Tern's hold on run `id`'s lock, as `prepare` takes it.
    fn hold(root: &Path, id: &str) -> File {
        let file = File::options()
            .write(true)
            .open(root.join(format!("{id}.lock")))
            .expect("open the lock file");
        file.try_lock().expect("take the lock");
        file
    }

    /// TM-sftp-13: copies of a run that ended without cleaning up (its
    /// lock is free) go, with their lock file.
    #[test]
    fn a_run_whose_lock_is_free_is_swept_with_its_lock_file() {
        let root = TempDir::new("copies-sweep-free");
        run_in(root.path(), "dead");
        sweep_in(root.path(), "own");
        assert!(!root.path().join("dead").exists());
        assert!(!root.path().join("dead.lock").exists());
    }

    /// Spec 0008, Model: another open Tern's copies are safe while it
    /// holds its lock.
    #[test]
    fn a_run_whose_lock_is_held_keeps_its_copies() {
        let root = TempDir::new("copies-sweep-held");
        run_in(root.path(), "live");
        let _held = hold(root.path(), "live");
        // The sweep's own open must see the hold, as another process's
        // would: std's locks are per open file, not per process.
        let other = File::options()
            .write(true)
            .open(root.path().join("live.lock"))
            .expect("open the lock file again");
        assert!(other.try_lock().is_err(), "a second open took a held lock");
        drop(other);
        sweep_in(root.path(), "own");
        assert!(root.path().join("live/0/1/.env").exists());
        assert!(root.path().join("live.lock").exists());
    }

    /// The running Tern's own copies are never swept, even before it took
    /// its lock.
    #[test]
    fn the_own_run_is_never_swept_even_with_a_free_lock() {
        let root = TempDir::new("copies-sweep-own");
        run_in(root.path(), "own");
        sweep_in(root.path(), "own");
        assert!(root.path().join("own/0/1/.env").exists());
        assert!(root.path().join("own.lock").exists());
    }

    /// A folder with no lock file is left alone: its run may be between
    /// making it and taking its lock.
    #[test]
    fn a_folder_without_a_lock_file_is_left_alone() {
        let root = TempDir::new("copies-sweep-nolock");
        run_in(root.path(), "unlocked");
        fs::remove_file(root.path().join("unlocked.lock")).expect("remove the lock file");
        sweep_in(root.path(), "own");
        assert!(root.path().join("unlocked/0/1/.env").exists());
    }

    /// A first launch has no edit folder yet; the sweep has nothing to do.
    #[test]
    fn sweeping_a_missing_root_does_nothing() {
        let root = TempDir::new("copies-sweep-missing");
        let missing = root.path().join("edit");
        sweep_in(&missing, "own");
        assert!(!missing.exists());
    }
}
