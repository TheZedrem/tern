//! SFTP sessions (spec 0008): a [`client`] runs `ssh -s <host> sftp` and
//! the SFTP client on a thread of its own, with its [`transfer`]s and the
//! edit [`copies`]; the file browser of `crate::files` draws what it
//! reports, in an SSH pane's sidebar or a files pane. This module is the
//! plain data about remote files: remote paths and the local names of
//! remote files, testable without a window or a host.

pub mod client;
pub mod copies;
mod transfer;

use std::fmt::Write;
use std::path::{Path, PathBuf};

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

/// The local file name for the remote name `name` (spec 0008 §Security,
/// TM-sftp-03, -04, -06): never refused, but every character that could
/// make it a path or disguise it escaped as `%XX` per UTF-8 byte. `windows`
/// adds what Windows can't hold; callers pass `cfg!(windows)`. `%` itself
/// stays: the escape is for safety, not to be undone.
pub fn local_name(name: &str, windows: bool) -> String {
    if matches!(name, "." | "..") {
        return name.replace('.', "%2E");
    }
    let last = name.chars().count().saturating_sub(1);
    let reserved = windows && is_reserved_device(name);
    let mut local = String::with_capacity(name.len());
    for (ix, c) in name.chars().enumerate() {
        let escape = c == '/'
            || c == '\\'
            || c.is_control()
            || is_format(c)
            || (windows && matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            || (reserved && ix == 0)
            || (windows && ix == last && matches!(c, '.' | ' '));
        if escape {
            let mut bytes = [0; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                let _ = write!(local, "%{byte:02X}");
            }
        } else {
            local.push(c);
        }
    }
    local
}

/// Where a remote file named `name` lands in `folder`: its [`local_name`],
/// which must come out as a direct child of `folder`. `None` only for an
/// empty name, which no listing shows, or if escaping ever let a path
/// through: the backstop of TM-sftp-03.
pub fn local_path(folder: &Path, name: &str, windows: bool) -> Option<PathBuf> {
    let local = local_name(name, windows);
    let path = folder.join(&local);
    let direct = path.parent() == Some(folder)
        && path.file_name().and_then(|file| file.to_str()) == Some(local.as_str());
    (!local.is_empty() && direct).then_some(path)
}

/// Types Windows runs rather than opens, beyond what `PATHEXT` names.
const WINDOWS_RUNS: &[&str] = &[
    ".lnk",
    ".url",
    ".scr",
    ".pif",
    ".msi",
    ".msp",
    ".cpl",
    ".hta",
    ".reg",
    ".appref-ms",
    ".application",
    ".settingcontent-ms",
    ".library-ms",
    ".search-ms",
    ".scf",
    ".wsh",
    ".xll",
    ".jar",
];
/// `PATHEXT` when it isn't set: Windows 11's default.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC";
/// Types a Linux desktop may run rather than open: launchers, packaged
/// apps, Java, and Windows programs, which Wine runs where it's installed.
const LINUX_RUNS: &[&str] = &[
    ".desktop",
    ".appimage",
    ".jar",
    ".exe",
    ".com",
    ".bat",
    ".cmd",
    ".msi",
    ".scr",
    ".lnk",
];

/// Whether the OS would run a file named `name` (as written to disk)
/// rather than open it in an app (spec 0008 §Security, TM-sftp-05). Such a
/// file is never opened. `pathext` is Windows' `PATHEXT`, if set; scripts
/// like `.sh` and `.js` stay editable on Linux, where an edit copy has no
/// execute bit.
pub fn runs_when_opened(name: &str, windows: bool, pathext: Option<&str>) -> bool {
    if windows {
        in_pathext(name, pathext) || WINDOWS_RUNS.iter().any(|ext| has_extension(name, ext))
    } else {
        LINUX_RUNS.iter().any(|ext| has_extension(name, ext))
    }
}

/// Whether `name` ends in an extension of Windows' `PATHEXT` (its
/// default when `None`): what Windows runs as a program.
pub fn in_pathext(name: &str, pathext: Option<&str>) -> bool {
    pathext
        .unwrap_or(DEFAULT_PATHEXT)
        .split(';')
        .any(|ext| has_extension(name, ext))
}

/// Whether `name` ends in `extension` (`.exe`), ignoring case and the
/// spaces `PATHEXT` may hold.
fn has_extension(name: &str, extension: &str) -> bool {
    let extension = extension.trim().to_lowercase();
    extension.starts_with('.') && name.to_lowercase().ends_with(&extension)
}

/// Windows' device names, whatever their extension or case: `CON`,
/// `aux.txt`, `com1.log`.
fn is_reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            // By character, not byte: a stem may end in any letter.
            let mut chars = upper.chars();
            let digit = chars.next_back();
            matches!(chars.as_str(), "COM" | "LPT") && digit.is_some_and(|c| c.is_ascii_digit())
        }
    }
}

/// Unicode format characters (category `Cf`): invisible, and some reorder
/// the text around them, so a name can't hide its real end.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Remote names that a hostile server could send, on every platform.
    const HOSTILE: &[&str] = &[
        "../../.bashrc",
        "/etc/passwd",
        "a/b",
        "a\\b",
        "C:\\x",
        "..\\..\\x",
        ".",
        "..",
        "/",
        "bad\nname\x07\x1b[31m",
        "txt\u{202E}gpj.exe",
        "CON",
        "aux.txt",
        "x.txt:evil",
        "trailing. ",
        "\u{200B}",
    ];

    /// Spec 0008 AC5 and TM-sftp-03: path separators and `.`/`..` are
    /// escaped per UTF-8 byte, so no name is a path, on every platform.
    #[test]
    fn separators_and_dot_names_are_escaped_so_no_name_is_a_path() {
        for windows in [false, true] {
            for (name, expected) in [
                ("../../.bashrc", "..%2F..%2F.bashrc"),
                ("a/b", "a%2Fb"),
                ("a\\b", "a%5Cb"),
                ("/etc/passwd", "%2Fetc%2Fpasswd"),
                (".", "%2E"),
                ("..", "%2E%2E"),
            ] {
                assert_eq!(local_name(name, windows), expected, "{name:?}, {windows}");
            }
        }
    }

    /// Spec 0008 §Security and TM-sftp-06: control characters and Unicode
    /// format characters (direction overrides, zero-width) are escaped as
    /// their UTF-8 bytes, so a name can't disguise its real end.
    #[test]
    fn control_and_invisible_characters_are_escaped_as_their_utf8_bytes() {
        for (name, expected) in [
            ("bad\nname\x07", "bad%0Aname%07"),
            ("\x1b[31m", "%1B[31m"),
            ("del\x7f", "del%7F"),
            ("c1\u{0085}", "c1%C2%85"),
            ("photo\u{202E}gpj.exe", "photo%E2%80%AEgpj.exe"),
            ("a\u{200B}b", "a%E2%80%8Bb"),
            ("\u{2066}x\u{2069}", "%E2%81%A6x%E2%81%A9"),
            ("\u{FEFF}bom", "%EF%BB%BFbom"),
        ] {
            assert_eq!(local_name(name, false), expected, "{name:?}");
        }
    }

    /// Spec 0008 §Security: ordinary names, `%` included, are kept as
    /// they are, on every platform.
    #[test]
    fn ordinary_names_and_percent_signs_are_unchanged() {
        for windows in [false, true] {
            for name in [
                "report.pdf",
                ".bashrc",
                "my file (1).txt",
                "Grüße – 東京.md",
                "100%.txt",
                "a%2Fb",
                "CONSOLE.log",
                "com10",
                "lpt",
            ] {
                assert_eq!(local_name(name, windows), name, "{name:?}, {windows}");
            }
        }
    }

    /// Spec 0008 AC5 and TM-sftp-04: on Windows, the characters it can't
    /// hold, a reserved device name's first character (any case, any
    /// extension) and a trailing dot or space are escaped.
    #[test]
    fn on_windows_reserved_characters_devices_and_trailing_dots_are_escaped() {
        for (name, expected) in [
            ("CON", "%43ON"),
            ("x.txt:evil", "x.txt%3Aevil"),
            ("aux.txt", "%61ux.txt"),
            ("Nul", "%4Eul"),
            ("prn.tar.gz", "%70rn.tar.gz"),
            ("COM0", "%43OM0"),
            ("com9.log", "%63om9.log"),
            ("LPT1", "%4CPT1"),
            ("a<b>c\"d|e?f*g", "a%3Cb%3Ec%22d%7Ce%3Ff%2Ag"),
            ("name.", "name%2E"),
            ("name ", "name%20"),
        ] {
            assert_eq!(local_name(name, true), expected, "{name:?}");
        }
    }

    /// TM-sftp-04: the Windows rules stay off elsewhere, where those names
    /// are ordinary files.
    #[test]
    fn the_windows_rules_dont_apply_elsewhere() {
        for name in [
            "CON",
            "aux.txt",
            "x.txt:evil",
            "a<b>?*|\"",
            "name.",
            "name ",
            "...",
        ] {
            assert_eq!(local_name(name, false), name, "{name:?}");
        }
    }

    /// Spec 0008 AC5 and TM-sftp-03: every hostile name lands directly in
    /// its folder, under its escaped name.
    #[test]
    fn every_hostile_name_lands_directly_inside_its_folder() {
        let folder = Path::new("/home/alice/Downloads");
        for windows in [false, true] {
            for name in HOSTILE {
                let path = local_path(folder, name, windows)
                    .unwrap_or_else(|| panic!("{name:?} refused ({windows})"));
                assert_eq!(path.parent(), Some(folder), "{name:?}, {windows}");
                assert_eq!(
                    path.file_name().and_then(|file| file.to_str()),
                    Some(local_name(name, windows).as_str()),
                    "{name:?}, {windows}"
                );
            }
        }
    }

    /// Spec 0008 §Security: an empty name has no local file.
    #[test]
    fn an_empty_name_has_no_local_path() {
        assert_eq!(local_path(Path::new("/tmp"), "", false), None);
        assert_eq!(local_path(Path::new("/tmp"), "", true), None);
    }

    /// Spec 0008 §Security and TM-sftp-05: on Windows, every type in
    /// Windows 11's default `PATHEXT` and every type of the fixed list is
    /// run rather than opened, whatever its case.
    #[test]
    fn on_windows_pathext_and_the_fixed_list_run_whatever_their_case() {
        for name in [
            "run.exe",
            "a.com",
            "b.bat",
            "c.cmd",
            "d.vbs",
            "e.vbe",
            "f.js",
            "g.jse",
            "h.wsf",
            "i.wsh",
            "j.msc",
            "a.LNK",
            "k.url",
            "l.scr",
            "m.pif",
            "n.msi",
            "o.msp",
            "p.cpl",
            "q.hta",
            "r.reg",
            "s.appref-ms",
            "t.application",
            "u.settingcontent-ms",
            "v.library-ms",
            "w.search-ms",
            "x.scf",
            "y.xll",
            "z.jar",
            "RUN.EXE",
            "Setup.Msi",
        ] {
            assert!(runs_when_opened(name, true, None), "{name:?}");
        }
    }

    /// Spec 0008 §Security: a `PATHEXT` that is set replaces the default
    /// (case and spaces as Windows allows); the fixed list still applies.
    #[test]
    fn a_set_pathext_replaces_the_default_but_not_the_fixed_list() {
        let pathext = Some(".EXE; .Py");
        assert!(runs_when_opened("tool.py", true, pathext));
        assert!(runs_when_opened("run.exe", true, pathext));
        assert!(!runs_when_opened("script.js", true, pathext));
        assert!(runs_when_opened("a.lnk", true, pathext));
        assert!(!runs_when_opened("tool.py", true, None));
    }

    /// Spec 0008 §Security: on Linux, launchers, packaged apps, Java and
    /// what Wine runs are never opened, whatever their case.
    #[test]
    fn on_linux_launchers_java_and_wine_types_run() {
        for name in [
            "x.desktop",
            "App.AppImage",
            "tool.jar",
            "run.exe",
            "a.com",
            "b.bat",
            "c.cmd",
            "d.msi",
            "e.scr",
            "a.LNK",
        ] {
            assert!(runs_when_opened(name, false, None), "{name:?}");
        }
    }

    /// Spec 0008 §Security: on Linux, scripts and text stay editable; an
    /// edit copy has no execute bit, so opening them doesn't run them.
    #[test]
    fn on_linux_scripts_and_text_are_opened() {
        for name in [
            "deploy.sh",
            "app.js",
            "tool.py",
            "notes.txt",
            "nginx.conf",
            "a.vbs",
        ] {
            assert!(!runs_when_opened(name, false, None), "{name:?}");
        }
    }

    /// TM-sftp-05: only the real end of the name counts; an extension in
    /// the middle, or a name that only ends in the letters, isn't one.
    #[test]
    fn an_extension_only_counts_at_the_end_of_the_name() {
        for windows in [false, true] {
            for name in [
                "exe.txt",
                "notes.desktop.txt",
                "run.exe.md",
                "readme",
                "jar",
            ] {
                assert!(
                    !runs_when_opened(name, windows, None),
                    "{name:?}, {windows}"
                );
            }
        }
    }

    /// TM-sftp-06 with -05: a direction override that shows `exe.txt` for
    /// a `.exe` is escaped on disk, and the name on disk is still caught.
    #[test]
    fn a_disguised_exe_is_still_caught_by_its_name_on_disk() {
        for windows in [false, true] {
            let disk = local_name("photo\u{202E}gpj.exe", windows);
            assert!(
                runs_when_opened(&disk, windows, None),
                "{disk:?}, {windows}"
            );
        }
    }
}
