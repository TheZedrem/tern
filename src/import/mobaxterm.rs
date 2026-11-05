//! MobaXterm's `MobaXterm.ini` and `.mxtsessions` exports (spec 0010).
//! Sessions are the keys of `[Bookmarks]` and `[Bookmarks_N]` sections, a
//! section's folder its `SubRep` (`Folder\Subfolder`). A session's value
//! is `#`-separated; its third part holds the connection, `%`-separated.
//! Field positions follow the reverse-engineered format of MobaXterm 23
//! (no official documentation exists). Only `[Bookmarks…]` sections are
//! read, so `[Passwords]` and `[Credentials]` never are.

use super::{Host, Parsed, Skip};

/// Session type 0, the first connection field.
const SSH: &str = "0";

/// Connection fields read, by position.
const HOST: usize = 1;
const PORT: usize = 2;
const USER: usize = 3;
const GATEWAY: usize = 8;
const KEY: usize = 14;
const PROXY_TYPE: usize = 19;

/// Whether `text` has a `[Bookmarks…]` section.
pub fn detect(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("[Bookmarks"))
}

pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed {
        format: "MobaXterm",
        ..Parsed::default()
    };
    let mut in_bookmarks = false;
    let mut group = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_bookmarks = line.starts_with("[Bookmarks");
            group = None;
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_bookmarks) else {
            continue;
        };
        match key {
            "SubRep" => group = folder(value),
            "ImgNum" => {}
            name => parsed.push(session(name, value, group.as_deref())),
        }
    }
    parsed
}

/// A `SubRep` as a host group: its folders joined with ` / `; `None` at
/// the top.
fn folder(sub_rep: &str) -> Option<String> {
    let folders: Vec<&str> = sub_rep
        .split('\\')
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
        .collect();
    (!folders.is_empty()).then(|| unescape(&folders.join(" / ")))
}

fn session(name: &str, value: &str, group: Option<&str>) -> Result<Host, Skip> {
    let fields: Vec<String> = value
        .split('#')
        .nth(2)
        .unwrap_or_default()
        .split('%')
        .map(unescape)
        .collect();
    let field = |at: usize| fields.get(at).map_or("", String::as_str);
    if field(0) != SSH {
        return Err(Skip::NotSsh);
    }
    if !field(GATEWAY).is_empty() {
        return Err(Skip::JumpHost);
    }
    if !matches!(field(PROXY_TYPE), "" | "0") {
        return Err(Skip::Proxy);
    }
    let port = match field(PORT) {
        "" => None,
        port => Some(port.parse().map_err(|_| Skip::BadPort)?),
    };
    Host::new(
        &unescape(name),
        field(HOST),
        port,
        field(USER),
        &key_path(field(KEY)),
        group,
    )
}

/// A key path as written, with MobaXterm's placeholders resolved:
/// `_ProfileDir_` is the home directory, `_CurrentDrive_` drive `C`.
fn key_path(path: &str) -> String {
    match path.strip_prefix("_ProfileDir_") {
        Some(rest) => format!("~{rest}"),
        None => path.replace("_CurrentDrive_", "C"),
    }
}

/// The characters MobaXterm writes as placeholders, back.
fn unescape(text: &str) -> String {
    text.replace("__DIEZE__", "#")
        .replace("__PERCENT__", "%")
        .replace("__PTVIRG__", ";")
        .replace("__DBLQUO__", "\"")
        .replace("__PIPE__", "|")
}
