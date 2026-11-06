//! The SFTP sidebar (spec 0008): each SSH pane can open one, showing the
//! files of its host. A pane's [`client`] runs `ssh -s <host> sftp` and
//! the SFTP client on a thread of its own; its [`view`] draws what that
//! thread reports. This module is the plain data both share: listings and
//! remote paths, testable without a window or a host.

pub mod client;
pub mod view;

use std::cmp::Ordering;

pub use view::{SftpEvent, SftpView};

/// The most entries a listing keeps; the rest are left out with a note
/// (TM-sftp-17).
pub const MAX_ENTRIES: usize = 50_000;

/// One folder's listing, folders first, then files, by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// The folder, as an absolute remote path.
    pub path: String,
    pub entries: Vec<Entry>,
    /// The folder held more than [`MAX_ENTRIES`].
    pub truncated: bool,
}

/// A file or folder in a listing. The name is the server's, untrusted:
/// it's shown by the title rules and used only to name the remote file.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// A symbolic link; `kind` is what it points to.
    pub link: bool,
    /// In bytes, if the server said.
    pub size: Option<u64>,
    /// Seconds since the Unix epoch, if the server said.
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

/// Where the path field's `input` goes: `~` and paths under it, and
/// relative paths, resolve against `home`; an absolute path is used as it
/// is. The result is normalized without asking the server, so the field
/// shows what the user typed, not where links lead.
pub fn resolve(home: &str, input: &str) -> String {
    let input = input.trim();
    let joined = match input.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ if input.starts_with('/') => input.to_owned(),
        _ => format!("{home}/{input}"),
    };
    normalize(&joined)
}

/// `name` inside folder `path`.
pub fn join(path: &str, name: &str) -> String {
    normalize(&format!("{path}/{name}"))
}

/// The folder above `path`; `/` stays `/`.
pub fn parent(path: &str) -> String {
    normalize(&format!("{path}/.."))
}

/// An absolute path with `.`, `..` and repeated slashes taken out; `..`
/// at the root stays at the root.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
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

    /// Spec 0008, flow 2: `~`, paths under it and relative paths resolve
    /// against the home folder; an absolute path is used as it is.
    #[test]
    fn the_path_field_resolves_tilde_and_relative_paths_against_home() {
        let home = "/home/alice";
        for (input, expected) in [
            ("~", "/home/alice"),
            ("~/", "/home/alice"),
            ("~/logs", "/home/alice/logs"),
            ("logs/today", "/home/alice/logs/today"),
            ("./logs", "/home/alice/logs"),
            ("/var/log", "/var/log"),
            ("  /var/log  ", "/var/log"),
            ("", "/home/alice"),
        ] {
            assert_eq!(resolve(home, input), expected, "{input:?}");
        }
    }

    /// `~bob` is a folder named `~bob` under home, not bob's home: only
    /// `~` alone or followed by `/` means the home folder.
    #[test]
    fn tilde_followed_by_a_name_is_a_relative_folder() {
        assert_eq!(resolve("/home/alice", "~bob"), "/home/alice/~bob");
    }

    /// Spec 0008, Model: the path field shows the folder listed, so what
    /// was typed is normalized: `.`, `..` and repeated slashes go, and
    /// `..` at the root stays at the root.
    #[test]
    fn a_typed_path_is_normalized_and_dot_dot_never_climbs_above_the_root() {
        for (input, expected) in [
            ("/var//log/./nginx/", "/var/log/nginx"),
            ("/var/log/../tmp", "/var/tmp"),
            ("/..", "/"),
            ("/../../etc", "/etc"),
            ("../../../..", "/"),
            ("~/../../..", "/"),
        ] {
            assert_eq!(resolve("/home/alice", input), expected, "{input:?}");
        }
    }

    /// Entering a folder joins its name to the path shown; at the root no
    /// double slash appears.
    #[test]
    fn entering_a_folder_joins_its_name_to_the_path() {
        assert_eq!(join("/home/alice", "logs"), "/home/alice/logs");
        assert_eq!(join("/", "etc"), "/etc");
    }

    /// Spec 0008, flow 2: Backspace goes up a folder; at `/` it stays.
    #[test]
    fn going_up_from_the_root_stays_at_the_root() {
        assert_eq!(parent("/home/alice/logs"), "/home/alice");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("/"), "/");
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
}
