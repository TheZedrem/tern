//! Permissions in the file browser (spec 0013, Permissions and flow 10):
//! what a listing knows of an entry's mode, owner and group, or on a
//! Windows disk its read-only flag, and how a change reaches the target.
//! SFTP names come from the long names servers send; a Linux disk asks
//! the system's user database, on the files thread only.

/// An entry's permissions, as far as its target has them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Perms {
    /// The mode's permission bits (`0o7777`), on SFTP and a Unix disk.
    pub mode: Option<u32>,
    pub owner: Option<Id>,
    pub group: Option<Id>,
    /// The read-only attribute, on a Windows disk.
    pub readonly: Option<bool>,
}

/// A user or group: its number, and its name where it's known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Id {
    pub id: u32,
    pub name: Option<String>,
}

impl Id {
    /// As the panel shows it: the name, else the number.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.to_string())
    }
}

/// A user or group as typed: a number, or a name to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    Number(u32),
    Name(String),
}

impl Who {
    pub fn parse(typed: &str) -> Option<Self> {
        let typed = typed.trim();
        if typed.is_empty() {
            return None;
        }
        Some(match typed.parse() {
            Ok(number) => Who::Number(number),
            Err(_) => Who::Name(typed.to_owned()),
        })
    }
}

/// What the panel asks a Local target to change; `None` stays as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub mode: Option<u32>,
    pub owner: Option<Who>,
    pub group: Option<Who>,
    pub readonly: Option<bool>,
}

/// A mode typed in octal: three or four digits, like `755` or `4755`.
pub fn parse_mode(typed: &str) -> Option<u32> {
    let typed = typed.trim();
    if !(3..=4).contains(&typed.len()) || !typed.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return None;
    }
    u32::from_str_radix(typed, 8).ok()
}

/// A mode as `ls -l` shows its permissions, like `rwxr-xr-x`, with the
/// setuid, setgid and sticky bits as `s`, `s` and `t` (`S`, `T` without
/// the execute bit under them).
pub fn mode_text(mode: u32) -> String {
    let special = [(0o4000, 's'), (0o2000, 's'), (0o1000, 't')];
    (0..3)
        .flat_map(|class| {
            let bits = (mode >> (6 - 3 * class)) & 0o7;
            let (flag, letter) = special[class];
            let execute = match (bits & 1 != 0, mode & flag != 0) {
                (true, true) => letter,
                (false, true) => letter.to_ascii_uppercase(),
                (true, false) => 'x',
                (false, false) => '-',
            };
            [
                if bits & 4 != 0 { 'r' } else { '-' },
                if bits & 2 != 0 { 'w' } else { '-' },
                execute,
            ]
        })
        .collect()
}

/// The owner and group names of an SFTP long name, the `ls -l` line a
/// server sends with each entry: its third and fourth columns, when the
/// first is a mode. Servers that send something else name nobody.
pub fn long_name_owners(long_name: &str) -> (Option<String>, Option<String>) {
    let mut columns = long_name.split_whitespace();
    let mode = columns.next().unwrap_or_default();
    // Bytes, not a str slice: a server's long name may be any text.
    let looks_like_mode = mode.as_bytes().get(1..10).is_some_and(|bits| {
        bits.iter()
            .all(|b| matches!(b, b'r' | b'w' | b'x' | b's' | b'S' | b't' | b'T' | b'-'))
    });
    if !looks_like_mode {
        return (None, None);
    }
    let owner = columns.nth(1).map(str::to_owned);
    let group = columns.next().map(str::to_owned);
    (owner, group)
}

/// The system's user database, through `libc` (spec 0013, Platform notes).
/// It may ask a directory server, so only the files thread calls it.
#[cfg(unix)]
pub mod users {
    use std::ffi::{CStr, CString};

    /// Enough for any passwd or group entry but a group with thousands
    /// of members, which then has no name here.
    const BUFFER: usize = 64 * 1024;

    pub fn user_name(uid: u32) -> Option<String> {
        let mut buffer = vec![0; BUFFER];
        // SAFETY: zeroed is a valid passwd; getpwuid_r writes into it and
        // into `buffer`, whose length it's given, and `found` points at
        // `entry` or is null.
        unsafe {
            let mut entry: libc::passwd = std::mem::zeroed();
            let mut found = std::ptr::null_mut();
            let rc = libc::getpwuid_r(uid, &mut entry, buffer.as_mut_ptr(), BUFFER, &mut found);
            (rc == 0 && !found.is_null()).then(|| name(entry.pw_name))?
        }
    }

    pub fn group_name(gid: u32) -> Option<String> {
        let mut buffer = vec![0; BUFFER];
        // SAFETY: as in `user_name`, for getgrgid_r.
        unsafe {
            let mut entry: libc::group = std::mem::zeroed();
            let mut found = std::ptr::null_mut();
            let rc = libc::getgrgid_r(gid, &mut entry, buffer.as_mut_ptr(), BUFFER, &mut found);
            (rc == 0 && !found.is_null()).then(|| name(entry.gr_name))?
        }
    }

    pub fn user_id(user: &str) -> Option<u32> {
        let user = CString::new(user).ok()?;
        let mut buffer = vec![0; BUFFER];
        // SAFETY: as in `user_name`, for getpwnam_r with a C string.
        unsafe {
            let mut entry: libc::passwd = std::mem::zeroed();
            let mut found = std::ptr::null_mut();
            let rc = libc::getpwnam_r(
                user.as_ptr(),
                &mut entry,
                buffer.as_mut_ptr(),
                BUFFER,
                &mut found,
            );
            (rc == 0 && !found.is_null()).then_some(entry.pw_uid)
        }
    }

    pub fn group_id(group: &str) -> Option<u32> {
        let group = CString::new(group).ok()?;
        let mut buffer = vec![0; BUFFER];
        // SAFETY: as in `user_name`, for getgrnam_r with a C string.
        unsafe {
            let mut entry: libc::group = std::mem::zeroed();
            let mut found = std::ptr::null_mut();
            let rc = libc::getgrnam_r(
                group.as_ptr(),
                &mut entry,
                buffer.as_mut_ptr(),
                BUFFER,
                &mut found,
            );
            (rc == 0 && !found.is_null()).then_some(entry.gr_gid)
        }
    }

    /// SAFETY: `name` is a NUL-terminated string the lookup just wrote.
    unsafe fn name(name: *const libc::c_char) -> Option<String> {
        if name.is_null() {
            return None;
        }
        // SAFETY: see above.
        unsafe { CStr::from_ptr(name) }
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

/// Opens Explorer's properties dialog for `path`, where a Windows file's
/// ACLs and owner live (spec 0013, flow 10). The dialog runs on a thread
/// of the shell's own, so this returns at once.
#[cfg(windows)]
pub fn show_properties(path: &str) -> Result<(), String> {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::{SHOP_FILEPATH, SHObjectProperties};
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    // SAFETY: COM is set up once per thread, which keeps it; `wide` is a
    // NUL-terminated path that outlives the call, and no page is named.
    let shown = unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        SHObjectProperties(
            std::ptr::null_mut(),
            SHOP_FILEPATH as u32,
            wide.as_ptr(),
            std::ptr::null(),
        )
    };
    if shown != 0 {
        Ok(())
    } else {
        Err(format!("Windows couldn't show the properties of {path}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_is_three_or_four_octal_digits() {
        assert_eq!(parse_mode("755"), Some(0o755));
        assert_eq!(parse_mode(" 0644 "), Some(0o644));
        assert_eq!(parse_mode("4755"), Some(0o4755));
        for typed in ["", "75", "75555", "789", "rwx", "-755"] {
            assert_eq!(parse_mode(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn a_mode_reads_as_ls_shows_it() {
        assert_eq!(mode_text(0o755), "rwxr-xr-x");
        assert_eq!(mode_text(0o644), "rw-r--r--");
        assert_eq!(mode_text(0o4755), "rwsr-xr-x");
        assert_eq!(mode_text(0o2644), "rw-r-Sr--");
        assert_eq!(mode_text(0o1777), "rwxrwxrwt");
        assert_eq!(mode_text(0), "---------");
    }

    #[test]
    fn a_long_name_names_its_owner_and_group() {
        let line = "-rwxr-xr-x    1 alice    staff        1234 Jan  1 12:00 run.sh";
        assert_eq!(
            long_name_owners(line),
            (Some("alice".into()), Some("staff".into()))
        );
        let folder = "drwxr-sr-x+  12 1001 www-data 4096 Nov 16 09:00 site";
        assert_eq!(
            long_name_owners(folder),
            (Some("1001".into()), Some("www-data".into()))
        );
        assert_eq!(long_name_owners("run.sh"), (None, None));
        assert_eq!(long_name_owners(""), (None, None));
    }

    #[test]
    fn a_typed_owner_is_a_number_or_a_name() {
        assert_eq!(Who::parse(" 1000 "), Some(Who::Number(1000)));
        assert_eq!(Who::parse("alice"), Some(Who::Name("alice".into())));
        assert_eq!(Who::parse("  "), None);
    }

    /// Spec 0013 flow 10: the octal field takes exactly 3 or 4 octal
    /// digits, all special bits included; nothing else reads as a mode.
    #[test]
    fn a_mode_takes_every_bit_and_nothing_but_octal_digits() {
        assert_eq!(parse_mode("000"), Some(0));
        assert_eq!(parse_mode("0000"), Some(0));
        assert_eq!(parse_mode("7777"), Some(0o7777));
        for typed in ["0o755", "7 55", "７５５", "+755", "8000"] {
            assert_eq!(parse_mode(typed), None, "{typed:?}");
        }
    }

    /// Spec 0013 flow 10: the `rwx` form beside the octal mode shows every
    /// special bit, upper case where no execute bit is under it.
    #[test]
    fn every_special_bit_shows_in_the_ls_form() {
        assert_eq!(mode_text(0o7777), "rwsrwsrwt");
        assert_eq!(mode_text(0o7000), "--S--S--T");
        assert_eq!(mode_text(0o1776), "rwxrwxrwT");
        assert_eq!(mode_text(0o4644), "rwSr--r--");
        assert_eq!(mode_text(0o2755), "rwxr-sr-x");
        // Only the permission bits count: a file type above them doesn't.
        assert_eq!(mode_text(0o100644 & 0o7777), "rw-r--r--");
    }

    /// Spec 0013 §Permissions: owners come from the long name only when it
    /// is an `ls -l` line; anything else, however odd, names nobody and
    /// never panics.
    #[test]
    fn an_odd_long_name_names_nobody_and_a_short_one_whoever_it_has() {
        for line in [
            "total 12",
            "-rwx",
            "-rwzr-xr-x 1 alice staff 0 Jan 1 00:00 x",
            "é-rwxr-xr-x 1 alice staff 0 Jan 1 00:00 x",
            "-rwxr-xr-é 1 alice staff 0 Jan 1 00:00 x",
            "   ",
        ] {
            assert_eq!(long_name_owners(line), (None, None), "{line:?}");
        }
        assert_eq!(
            long_name_owners("-rw-r--r-- 1 alice"),
            (Some("alice".into()), None)
        );
        assert_eq!(long_name_owners("-rw-r--r-- 1"), (None, None));
        // macOS marks extended attributes after the mode.
        assert_eq!(
            long_name_owners("-rw-r--r--@ 1 bob wheel 0 Jan 1 00:00 x"),
            (Some("bob".into()), Some("wheel".into()))
        );
    }

    /// Spec 0013 §Permissions: an ID without a name shows as its number.
    #[test]
    fn an_id_shows_its_name_else_its_number() {
        let named = Id {
            id: 1000,
            name: Some("alice".into()),
        };
        assert_eq!(named.label(), "alice");
        assert_eq!(
            Id {
                id: 1001,
                name: None
            }
            .label(),
            "1001"
        );
    }
}
