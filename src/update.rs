//! The opt-in check for a new release on GitHub (spec 0011). It only reads
//! the latest release's tag and page; installing is the package manager's.
//! Nothing here runs before the first frame, and nothing runs at all unless
//! `update_check: daily` is set or the user runs `app.checkForUpdates`.

use futures::channel::oneshot;
use serde::Deserialize;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// GitHub's latest release, which leaves out drafts and pre-releases.
const LATEST: &str = "https://api.github.com/repos/TheZedrem/tern/releases/latest";

/// The only release pages a notice opens [load-bearing].
const RELEASE_PAGES: &str = "https://github.com/TheZedrem/tern/releases/";

const TIMEOUT: Duration = Duration::from_secs(10);

/// Anything bigger isn't the small JSON object GitHub sends.
const MAX_BODY: u64 = 64 * 1024;

/// How often a `daily` check runs.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// A `vMAJOR.MINOR.PATCH` version, compared field by field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
    /// `MAJOR.MINOR.PATCH`, three plain integers and nothing else.
    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.').map(|part| {
            // `u32::from_str` takes a leading `+`; a version doesn't.
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        });
        let version = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// The version of this build, if Cargo.toml's is a plain one.
    pub fn running() -> Option<Self> {
        Self::parse(env!("CARGO_PKG_VERSION"))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The latest release, checked: a plain version and a page on the repo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub url: String,
}

/// The latest release in GitHub's response `body`. A tag other than
/// `vX.Y.Z` or a page outside [`RELEASE_PAGES`] fails the check.
pub fn parse_release(body: &[u8]) -> Result<Release, String> {
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
        html_url: String,
    }
    let latest: Latest =
        serde_json::from_slice(body).map_err(|_| "GitHub's answer isn't a release".to_owned())?;
    let version = latest
        .tag_name
        .strip_prefix('v')
        .and_then(Version::parse)
        .ok_or("the latest release's tag isn't a version like v1.2.3")?;
    if !latest.html_url.starts_with(RELEASE_PAGES) {
        return Err("the latest release's page isn't on Tern's GitHub".into());
    }
    Ok(Release {
        version,
        url: latest.html_url,
    })
}

/// The latest release if it's newer than `running`.
pub fn newer(release: Release, running: Version) -> Option<Release> {
    (release.version > running).then_some(release)
}

/// Asks GitHub for the latest release, blocking for up to [`TIMEOUT`]. The
/// request carries no header about the user or the machine beyond
/// `User-Agent: tern/<version>` [load-bearing]. TLS checks against the OS's
/// certificates, and `HTTPS_PROXY` is honored.
fn fetch() -> Result<Release, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("tern/", env!("CARGO_PKG_VERSION")))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .proxy(ureq::Proxy::try_from_env())
        .build()
        .into();
    let mut response = agent
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(code) => format!("GitHub answered with HTTP {code}"),
            ureq::Error::Timeout(_) => "GitHub didn't answer in time".to_owned(),
            error => format!("GitHub couldn't be reached: {error}"),
        })?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|_| "GitHub's answer couldn't be read, or was too big".to_owned())?;
    parse_release(&body)
}

/// Checks for a release newer than this build on a thread of its own, as
/// the request blocks. `Ok(None)`: Tern is up to date.
pub async fn check() -> Result<Option<Release>, String> {
    let (send, receive) = oneshot::channel();
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let checked = Version::running()
                .ok_or_else(|| "this build's version isn't like 1.2.3".to_owned())
                .and_then(|running| fetch().map(|release| newer(release, running)));
            let _ = send.send(checked);
        })
        .map_err(|error| format!("the update check couldn't start: {error}"))?;
    receive
        .await
        .unwrap_or_else(|_| Err("the update check stopped".into()))
}

/// `<data_local_dir>/tern/update.json`: when the last check ran.
pub fn state_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("tern").join("update.json"))
}

#[derive(serde::Serialize, Deserialize)]
struct State {
    /// Unix seconds.
    last_check: u64,
}

/// When the last check ran, in Unix seconds; `None` for a missing or
/// unreadable file, which counts as never.
pub fn last_check(path: &Path) -> Option<u64> {
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice::<State>(&text)
        .ok()
        .map(|state| state.last_check)
}

/// Records that a check ran at `now`, atomically through a temp file.
pub fn save_last_check(path: &Path, now: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(&serde_json::to_vec(&State { last_check: now })?)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
}

/// Now, in Unix seconds.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// How long until the next check is due, given the last one. A last check
/// in the future (a clock set back) counts as due now.
pub fn due_in(last_check: Option<u64>, now: u64) -> Duration {
    match last_check {
        Some(last) if last <= now => INTERVAL.saturating_sub(Duration::from_secs(now - last)),
        _ => Duration::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;

    const PAGE: &str = "https://github.com/TheZedrem/tern/releases/tag/v1.2.3";
    const DAY: u64 = 24 * 60 * 60;

    /// GitHub's answer with `tag` and `url`, plus fields Tern doesn't read.
    fn body(tag: &str, url: &str) -> Vec<u8> {
        serde_json::json!({
            "id": 1,
            "tag_name": tag,
            "html_url": url,
            "name": "Tern",
            "prerelease": false,
            "assets": [],
        })
        .to_string()
        .into_bytes()
    }

    fn release(major: u32, minor: u32, patch: u32) -> Release {
        Release {
            version: Version(major, minor, patch),
            url: PAGE.into(),
        }
    }

    /// Spec 0011 §Model, AC4: a `vX.Y.Z` tag on Tern's release pages is
    /// read as that version and page, whatever else GitHub sends.
    #[test]
    fn a_plain_tag_on_terns_release_page_is_a_release() {
        let parsed = parse_release(&body("v1.2.3", PAGE)).unwrap();
        assert_eq!(parsed, release(1, 2, 3));
        assert_eq!(parsed.version.to_string(), "1.2.3");
    }

    /// Spec 0011 §Model, AC7: a tag other than exactly `vMAJOR.MINOR.PATCH`
    /// fails the check, pre-releases included.
    #[test]
    fn a_tag_that_isnt_exactly_v_major_minor_patch_fails() {
        for tag in [
            "1.2.3",
            "V1.2.3",
            " v1.2.3",
            "v1.2.3 ",
            "v1.2",
            "v1",
            "v",
            "",
            "v1.2.3.4",
            "v1.2.3-rc.1",
            "v1.2.3-beta",
            "v1.2.3+build",
            "v+1.2.3",
            "v1.+2.3",
            "v1..3",
            "v1.2.",
            "v.1.2",
            "v-1.2.3",
            "v1.x.3",
            "nightly",
        ] {
            assert!(parse_release(&body(tag, PAGE)).is_err(), "{tag:?}");
        }
    }

    /// Spec 0011 §Model, AC7: a page outside
    /// `https://github.com/TheZedrem/tern/releases/` fails the check.
    #[test]
    fn a_page_outside_terns_releases_fails() {
        for url in [
            "http://github.com/TheZedrem/tern/releases/tag/v1.2.3",
            "https://github.com/Someone/tern/releases/tag/v1.2.3",
            "https://github.com/TheZedrem/tern-fork/releases/tag/v1.2.3",
            "https://github.com/TheZedrem/tern/issues/1",
            "https://github.com/TheZedrem/tern/releases",
            "https://github.com.evil.example/TheZedrem/tern/releases/tag/v1.2.3",
            "https://evil.example/https://github.com/TheZedrem/tern/releases/",
            "https://api.github.com/repos/TheZedrem/tern/releases/1",
            "",
        ] {
            assert!(parse_release(&body("v1.2.3", url)).is_err(), "{url:?}");
        }
    }

    /// Spec 0011 flow 5, AC7: a body that isn't a release object with both
    /// fields as text fails the check.
    #[test]
    fn a_malformed_body_fails() {
        for text in [
            "",
            "not json",
            "<html>rate limited</html>",
            "null",
            "[]",
            "{}",
            r#"{"tag_name": "v1.2.3"}"#,
            &format!(r#"{{"html_url": "{PAGE}"}}"#),
            &format!(r#"{{"tag_name": 123, "html_url": "{PAGE}"}}"#),
            &format!(r#"{{"tag_name": "v1.2.3", "html_url": "{PAGE}""#),
        ] {
            assert!(parse_release(text.as_bytes()).is_err(), "{text:?}");
        }
    }

    /// Spec 0011 §Model, flow 4: only a strictly higher version is newer,
    /// compared as three integers.
    #[test]
    fn only_a_strictly_higher_version_is_newer() {
        let running = Version(0, 9, 5);
        for (higher, (major, minor, patch)) in [
            (true, (0, 9, 6)),
            (true, (0, 10, 0)),
            (true, (1, 0, 0)),
            (false, (0, 9, 5)),
            (false, (0, 9, 4)),
            (false, (0, 8, 99)),
            (false, (0, 0, 0)),
        ] {
            let latest = release(major, minor, patch);
            let expected = higher.then(|| latest.clone());
            assert_eq!(newer(latest, running), expected, "{major}.{minor}.{patch}");
        }
    }

    /// Spec 0011 §Model: versions compare numerically, not as text.
    #[test]
    fn versions_compare_as_numbers_not_text() {
        let ten = parse_release(&body("v0.10.0", PAGE)).unwrap();
        assert!(newer(ten, Version(0, 9, 0)).is_some());
        let nine = parse_release(&body("v0.9.0", PAGE)).unwrap();
        assert!(newer(nine, Version(0, 10, 0)).is_none());
        let patch = parse_release(&body("v2.0.10", PAGE)).unwrap();
        assert!(newer(patch, Version(2, 0, 9)).is_some());
    }

    /// Spec 0011 §Model: the running version is this build's
    /// `CARGO_PKG_VERSION`, so the command has something to compare.
    #[test]
    fn the_running_version_is_the_packages() {
        let running = Version::running().expect("Cargo.toml's version is plain");
        assert_eq!(running.to_string(), env!("CARGO_PKG_VERSION"));
    }

    /// Spec 0011 flow 3, AC6: a check is due at once if none ever ran, and
    /// otherwise 24 h after the last one.
    #[test]
    fn a_check_is_due_a_day_after_the_last() {
        let now = 1_800_000_000;
        assert_eq!(INTERVAL, Duration::from_secs(DAY));
        for (last, due) in [
            (None, 0),
            (Some(now), DAY),
            (Some(now - 1), DAY - 1),
            (Some(now - 6 * 60 * 60), 18 * 60 * 60),
            (Some(now - DAY + 1), 1),
            (Some(now - DAY), 0),
            (Some(now - DAY - 1), 0),
            (Some(now - 30 * DAY), 0),
            (Some(0), 0),
        ] {
            assert_eq!(due_in(last, now), Duration::from_secs(due), "{last:?}");
        }
    }

    /// Spec 0011 flow 3: a last check in the future, from a clock set back,
    /// doesn't hold the next one off; it's due now.
    #[test]
    fn a_last_check_in_the_future_is_due_now() {
        let now = 1_800_000_000;
        for last in [now + 1, now + DAY, now + 365 * DAY] {
            assert_eq!(due_in(Some(last), now), Duration::ZERO, "{last}");
        }
    }

    /// Spec 0011 §Model: the state file reads back the last check saved,
    /// creating its folder, and a newer save replaces the older one.
    #[test]
    fn the_last_check_saved_reads_back() {
        let root = TempDir::new("update-state-round-trip");
        let path = root.path().join("tern").join("update.json");
        save_last_check(&path, 1_700_000_000).unwrap();
        assert_eq!(last_check(&path), Some(1_700_000_000));
        save_last_check(&path, 1_700_086_400).unwrap();
        assert_eq!(last_check(&path), Some(1_700_086_400));
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["update.json"], "temp file left behind");
    }

    /// Spec 0011 §Security & privacy: the state file holds `last_check`
    /// and nothing else.
    #[test]
    fn the_state_file_holds_only_the_last_check() {
        let root = TempDir::new("update-state-minimal");
        let path = root.path().join("update.json");
        save_last_check(&path, 42).unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(state, serde_json::json!({ "last_check": 42 }));
    }

    /// Spec 0011 §Model: a missing state file, or one that doesn't parse,
    /// counts as never checked.
    #[test]
    fn a_missing_or_unreadable_state_file_counts_as_never() {
        let root = TempDir::new("update-state-bad");
        let path = root.path().join("update.json");
        assert_eq!(last_check(&path), None);
        for text in [
            "",
            "garbage",
            "{}",
            "null",
            r#"{"last_check": "yesterday"}"#,
            r#"{"last_check": -5}"#,
            r#"{"last_check": 12"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(last_check(&path), None, "{text:?}");
        }
    }

    /// Spec 0011 §Model: the state lives at
    /// `<data_local_dir>/tern/update.json`.
    #[test]
    fn the_state_file_is_terns_update_json_in_local_data() {
        let path = state_path().expect("a local data dir");
        assert_eq!(
            path,
            dirs::data_local_dir().unwrap().join("tern/update.json")
        );
    }
}
