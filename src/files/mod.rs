//! The file browser (spec 0013): one view, [`browser::Browser`], drawn
//! over one target's session, in an SSH pane's SFTP sidebar (spec 0008)
//! or in a [`pane::FilesPane`] of the split tree. A target is the local
//! disk ([`local`]) or an SFTP host (`crate::sftp`); both report the same
//! [`Event`]s, and the browser shows their folders as a [`tree`]. This
//! module is the plain data they share, testable without a window.

pub mod browser;
pub mod local;
pub mod pane;
pub mod tree;

use std::cmp::Ordering;

/// The most entries a listing keeps; the rest are left out with a note
/// (TM-sftp-17).
pub const MAX_ENTRIES: usize = 50_000;

/// One folder's listing, folders first, then files, by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// The folder, as an absolute path of its target.
    pub path: String,
    pub entries: Vec<Entry>,
    /// The folder held more than [`MAX_ENTRIES`].
    pub truncated: bool,
}

/// A file or folder in a listing. A remote name is the server's,
/// untrusted: it's shown by the title rules and used only to name the
/// remote file; a local one is shown by the same rules.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// A symbolic link; `kind` is what it points to.
    pub link: bool,
    /// In bytes, if known.
    pub size: Option<u64>,
    /// Seconds since the Unix epoch, if known.
    pub modified: Option<u32>,
}

/// What an entry is, or what its link points to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    File,
    /// A device, socket or pipe, or a link whose target is gone.
    Other,
}

impl Entry {
    /// Enter and double-click go into it.
    pub fn is_folder(&self) -> bool {
        self.kind == Kind::Folder
    }
}

/// Sorts `entries` as a listing shows them: folders first, then by name,
/// ignoring case, with ties in byte order so the order is stable.
pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_folder()
            .cmp(&a.is_folder())
            .then_with(|| compare_names(&a.name, &b.name))
    });
}

fn compare_names(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Whether a listing leaves out a name the server sent: `.` and `..`,
/// which Backspace and the path field cover, and an empty name.
pub fn hidden(name: &str) -> bool {
    matches!(name, "" | "." | "..")
}

/// A size as the listing shows it: bytes up to 1 KiB, then one decimal in
/// the largest binary unit below it.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A modification time as the listing shows it: the UTC date, as
/// `YYYY-MM-DD`. Days from the epoch to a civil date by Howard Hinnant's
/// algorithm, so no date crate is needed for one column.
pub fn date(seconds: u32) -> String {
    let days = i64::from(seconds) / 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// What a target's session reports to its browser: the local disk's
/// thread and an SFTP connection alike. Transfers are SFTP's only.
#[derive(Debug)]
pub enum Event {
    /// Ready; the home folder, where relative paths start.
    Connected {
        home: String,
    },
    Listed(Listing),
    /// Listing `path` failed; the session is still up.
    ListFailed {
        path: String,
        error: String,
    },
    /// A transfer moved on: `done` bytes of `total`, if the size is known.
    Progress {
        id: u64,
        done: u64,
        total: Option<u64>,
    },
    /// A transfer is complete; `stat` is the remote file's size and time,
    /// as fetched or as uploaded, when the server said.
    TransferDone {
        id: u64,
        stat: Option<crate::sftp::client::RemoteStat>,
    },
    /// A transfer failed, and removed what it wrote. A cancelled one
    /// reports nothing.
    TransferFailed {
        id: u64,
        error: String,
    },
    /// A transfer's target exists and wasn't to be replaced, or an edit's
    /// remote file changed since it was fetched: nothing was written.
    /// Asking and starting again with `replace` (or no `expect`) is the
    /// browser's.
    Exists {
        id: u64,
    },
    /// A rename's new name exists and wasn't to be replaced: nothing was
    /// renamed (spec 0013, flow 5). Asking is the browser's.
    RenameExists {
        from: String,
        to: String,
    },
    /// Something in this folder changed: an upload, rename, delete or new
    /// folder.
    Changed {
        folder: String,
    },
    /// A rename, delete or new folder failed.
    Failed(String),
    /// The session is over, and why.
    Ended(String),
}

/// Whether `name` can be a new name typed for a rename or a new folder
/// (spec 0013, flow 5): one path component, so not empty, `.` or `..`,
/// and without `/`, nor `\` where `windows` (the target's platform).
/// Anything else the target refuses comes back with its reason.
pub fn valid_name(name: &str, windows: bool) -> Result<(), &'static str> {
    if matches!(name, "" | "." | "..") {
        return Err("A name can't be empty, . or ..");
    }
    if name.contains('/') || (windows && name.contains('\\')) {
        return Err(if windows {
            "A name can't hold / or \\"
        } else {
            "A name can't hold /"
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry named `name` of `kind`, not a link, with no size or date.
    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            link: false,
            size: None,
            modified: None,
        }
    }

    /// Spec 0008, flow 2: folders first, then files, by name ignoring
    /// case; a link to a folder sorts with the folders.
    #[test]
    fn a_listing_shows_folders_first_then_files_by_name_ignoring_case() {
        let mut entries = vec![
            entry("b.txt", Kind::File),
            entry("zeta", Kind::Folder),
            entry("Beta", Kind::File),
            entry("dev-null", Kind::Other),
            Entry {
                link: true,
                ..entry("Alpha", Kind::Folder)
            },
            entry("apple", Kind::File),
        ];
        sort(&mut entries);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            ["Alpha", "zeta", "apple", "b.txt", "Beta", "dev-null"]
        );
    }

    /// Names equal but for case keep one order whatever the server sent,
    /// so a refresh doesn't shuffle them.
    #[test]
    fn names_that_differ_only_in_case_always_sort_the_same_way() {
        let names = |order: [&str; 3]| {
            let mut entries: Vec<Entry> = order
                .into_iter()
                .map(|name| entry(name, Kind::File))
                .collect();
            sort(&mut entries);
            entries
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>()
        };
        let sorted = names(["readme", "README", "ReadMe"]);
        assert_eq!(sorted, names(["README", "ReadMe", "readme"]));
        assert_eq!(sorted, names(["ReadMe", "readme", "README"]));
    }

    /// Spec 0008 §Security: the listing hides `.`, `..` and empty names,
    /// and nothing else, dotfiles included.
    #[test]
    fn the_listing_hides_dot_dot_dot_and_empty_names_only() {
        for name in ["", ".", ".."] {
            assert!(hidden(name), "{name:?} is shown");
        }
        for name in [".bashrc", "...", ". ", "..x", "a"] {
            assert!(!hidden(name), "{name:?} is hidden");
        }
    }

    /// Sizes read as bytes below 1 KiB, then one decimal in binary units.
    #[test]
    fn sizes_read_as_bytes_then_one_decimal_in_binary_units() {
        for (bytes, expected) in [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.0 KiB"),
            (1536, "1.5 KiB"),
            (1024 * 1024, "1.0 MiB"),
            (5 * 1024 * 1024 * 1024, "5.0 GiB"),
        ] {
            assert_eq!(human_size(bytes), expected, "{bytes}");
        }
    }

    /// The modified column is the UTC date: the epoch, a leap day, the
    /// last second of a day and of a year, and the largest time SFTP v3
    /// can send.
    #[test]
    fn modification_times_show_as_the_utc_date() {
        for (seconds, expected) in [
            (0, "1970-01-01"),
            (86_399, "1970-01-01"),
            (951_782_400, "2000-02-29"),
            (1_709_164_800, "2024-02-29"),
            (1_735_689_599, "2024-12-31"),
            (u32::MAX, "2106-02-07"),
        ] {
            assert_eq!(date(seconds), expected, "{seconds}");
        }
    }

    /// Spec 0013, flow 5 and AC7: a new name is one path component; `/`
    /// everywhere and `\\` on Windows are refused, as are `.`, `..` and
    /// an empty name.
    #[test]
    fn a_new_name_must_be_one_path_component() {
        for windows in [false, true] {
            for name in ["", ".", "..", "a/b", "/x", "x/"] {
                assert!(valid_name(name, windows).is_err(), "{name:?}, {windows}");
            }
            for name in ["notes.txt", ".env", "...", "a b", "Grüße"] {
                assert_eq!(valid_name(name, windows), Ok(()), "{name:?}, {windows}");
            }
        }
        assert!(valid_name("a\\b", true).is_err());
        assert_eq!(valid_name("a\\b", false), Ok(()));
    }
}
