//! `history.jsonl` (spec 0021): one [`Entry`] per line as JSON, oldest
//! first, only ever touched on the history thread. Owner-only on Unix;
//! `%LOCALAPPDATA%` on Windows doesn't roam.

use super::Entry;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Where the history is kept: `<data_local_dir>/tern/history.jsonl`.
pub fn default_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("tern").join("history.jsonl"))
}

/// What [`read`] found.
pub struct Read {
    /// The lines that parsed, oldest first.
    pub entries: Vec<Entry>,
    /// Every non-empty line, read or not.
    pub lines: usize,
}

/// Reads the file. Lines that don't parse are skipped, and a missing file
/// is an empty history.
pub fn read(path: &Path) -> Result<Read, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    Ok(Read {
        entries: lines
            .iter()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect(),
        lines: lines.len(),
    })
}

/// Adds one entry at the end.
pub fn append(path: &Path, entry: &Entry) -> Result<(), String> {
    let write = || -> io::Result<()> {
        let mut file = open(path, OpenOptions::new().append(true))?;
        file.write_all(line(entry)?.as_bytes())
    };
    write().map_err(|error| format!("{}: {error}", path.display()))
}

/// Replaces the file with `entries`, through a temp file and a rename so
/// a crash never leaves half a history.
pub fn rewrite(path: &Path, entries: &[Entry]) -> Result<(), String> {
    let temp = path.with_extension("jsonl.tmp");
    let write = || -> io::Result<()> {
        let mut file = open(&temp, OpenOptions::new().write(true).truncate(true))?;
        let mut text = String::new();
        for entry in entries {
            text.push_str(&line(entry)?);
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)
    };
    write().map_err(|error| format!("{}: {error}", path.display()))
}

/// Deletes the file; a missing one is fine.
pub fn delete(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(format!("{}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

fn line(entry: &Entry) -> io::Result<String> {
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    Ok(line)
}

/// Opens `path` for writing, creating it and its folder, owner-only on
/// Unix.
fn open(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    options.create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(options, 0o600);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use crate::history::ShellKind;

    fn entry(shell: ShellKind, command: &str) -> Entry {
        Entry {
            shell,
            command: command.into(),
        }
    }

    #[test]
    fn appended_entries_read_back_oldest_first() {
        let dir = TempDir::new("history-file-append");
        let path = dir.path().join("tern/history.jsonl");
        let first = entry(ShellKind::Bash, "git status");
        let second = entry(ShellKind::Pwsh, "Get-ChildItem 'C:\\a b' | % { \"äöü\" }");
        append(&path, &first).expect("append");
        append(&path, &second).expect("append");
        let found = read(&path).expect("read");
        assert_eq!(found.entries, [first, second]);
        assert_eq!(found.lines, 2);
    }

    /// Spec 0021, flow 2: a missing file is the normal empty case.
    #[test]
    fn a_missing_file_is_an_empty_history() {
        let dir = TempDir::new("history-file-missing");
        let found = read(&dir.path().join("history.jsonl")).expect("read");
        assert!(found.entries.is_empty());
        assert_eq!(found.lines, 0);
    }

    /// Spec 0021, flow 2: lines that can't be read are skipped, but still
    /// count as lines; blank lines don't.
    #[test]
    fn lines_that_dont_parse_are_skipped_but_counted() {
        let dir = TempDir::new("history-file-corrupt");
        let path = dir.path().join("history.jsonl");
        let text = concat!(
            "{\"shell\":\"bash\",\"command\":\"ls\"}\n",
            "{\"shell\":\"bash\",\"comm\n",
            "\n",
            "not json at all\n",
            "{\"shell\":\"fish\",\"command\":\"ls\"}\n",
            "{\"shell\":\"zsh\",\"command\":\"make\"}",
        );
        fs::write(&path, text).expect("write");
        let found = read(&path).expect("read");
        assert_eq!(
            found.entries,
            [entry(ShellKind::Bash, "ls"), entry(ShellKind::Zsh, "make")]
        );
        assert_eq!(found.lines, 5);
    }

    #[test]
    fn rewrite_replaces_the_whole_file_and_leaves_no_temp_file() {
        let dir = TempDir::new("history-file-rewrite");
        let path = dir.path().join("history.jsonl");
        for n in 0..5 {
            append(&path, &entry(ShellKind::Bash, &format!("echo {n}"))).expect("append");
        }
        let kept = [entry(ShellKind::Zsh, "ls"), entry(ShellKind::Bash, "make")];
        rewrite(&path, &kept).expect("rewrite");
        let found = read(&path).expect("read");
        assert_eq!(found.entries, kept);
        assert_eq!(found.lines, 2);
        let names: Vec<_> = fs::read_dir(dir.path())
            .expect("list")
            .map(|item| item.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["history.jsonl"], "a temp file was left");

        rewrite(&path, &[]).expect("rewrite to empty");
        assert!(read(&path).expect("read").entries.is_empty());
    }

    #[test]
    fn delete_removes_the_file_and_a_missing_one_is_fine() {
        let dir = TempDir::new("history-file-delete");
        let path = dir.path().join("history.jsonl");
        append(&path, &entry(ShellKind::Bash, "ls")).expect("append");
        delete(&path).expect("delete");
        assert!(!path.exists());
        delete(&path).expect("delete a missing file");
        delete(&dir.path().join("no/such/folder/history.jsonl")).expect("missing folder");
    }

    #[test]
    fn the_file_lives_in_the_local_data_directory() {
        if let (Some(path), Some(data)) = (default_path(), dirs::data_local_dir()) {
            assert_eq!(path, data.join("tern").join("history.jsonl"));
        }
    }

    /// Spec 0021, Platform notes: on Linux the file is owner-only, whether
    /// appended to or rewritten.
    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("history-file-mode");
        let path = dir.path().join("history.jsonl");
        let mode = |path: &Path| fs::metadata(path).expect("stat").permissions().mode() & 0o777;
        append(&path, &entry(ShellKind::Bash, "ls")).expect("append");
        assert_eq!(mode(&path), 0o600, "appended");
        rewrite(&path, &[entry(ShellKind::Bash, "make")]).expect("rewrite");
        assert_eq!(mode(&path), 0o600, "rewritten");
    }
}
