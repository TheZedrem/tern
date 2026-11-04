//! The hosts of OpenSSH's own config, `~/.ssh/config` (spec 0006): every
//! `Host` pattern that names exactly one host, following `Include`. Only
//! the aliases are read; ssh applies the rest of the file itself.

use std::path::{Path, PathBuf};

/// How many `Include`s may nest below the top config, as in OpenSSH.
const MAX_INCLUDE_DEPTH: usize = 16;

/// The aliases `path` and the files it includes define, each once, in file
/// order, and a notice per file that couldn't be read. A missing `path` is
/// no hosts and no notice: plenty of users have no ssh config.
pub fn aliases(path: &Path) -> (Vec<String>, Vec<String>) {
    let mut found = Found::default();
    // Relative `Include` paths are relative to `~/.ssh`.
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    found.read(path, &base, 0);
    (found.aliases, found.notices)
}

#[derive(Default)]
struct Found {
    aliases: Vec<String>,
    notices: Vec<String>,
}

impl Found {
    fn read(&mut self, path: &Path, base: &Path, depth: usize) {
        // A directory an `Include` glob matched is skipped, as ssh does.
        if path.is_dir() {
            return;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            // A missing file, at the top or as an `Include` without a
            // match, is nothing to read, as ssh sees it.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.notices.push(format!(
                    "{} could not be read ({error}); its hosts are missing.",
                    path.display()
                ));
                return;
            }
        };
        // ssh reads bytes: a stray Latin-1 comment mustn't hide the hosts.
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let Some((keyword, args)) = keyword_and_args(line) else {
                continue;
            };
            match keyword.to_ascii_lowercase().as_str() {
                "host" => {
                    for alias in args.into_iter().filter(|arg| is_alias(arg)) {
                        if !self.aliases.contains(&alias) {
                            self.aliases.push(alias);
                        }
                    }
                }
                "include" if depth >= MAX_INCLUDE_DEPTH => self.notices.push(format!(
                    "{}: `Include` nests deeper than {MAX_INCLUDE_DEPTH} files; the rest was skipped.",
                    path.display()
                )),
                "include" => {
                    for pattern in args {
                        for file in expand(&pattern, base) {
                            self.read(&file, base, depth + 1);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// A config line's keyword and arguments, or `None` for a blank line or a
/// comment. The keyword ends at whitespace or `=`; arguments are separated
/// by whitespace, double quotes keep spaces in one, and an argument that
/// starts with `#` begins a comment to the end of the line.
fn keyword_and_args(line: &str) -> Option<(&str, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    Some((keyword, split_args(rest)))
}

fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut in_arg = false;
    for c in text.chars() {
        match c {
            '#' if !quoted && !in_arg => break,
            '"' => {
                quoted = !quoted;
                in_arg = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            c => {
                current.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(current);
    }
    args
}

/// Whether a `Host` pattern names one host: no wildcards, no negation,
/// and nothing ssh would read as an option.
fn is_alias(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.contains(['*', '?']) && !pattern.starts_with(['!', '-'])
}

/// The files an `Include` pattern names, sorted, as `glob` would: `~` is
/// the home directory, relative paths start in `base`, and the last
/// component may hold `*` and `?`.
fn expand(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => match dirs::home_dir() {
            Some(home) => home.join(rest.trim_start_matches('/')),
            None => return Vec::new(),
        },
        _ => base.join(pattern),
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return vec![path];
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|candidate| wildcard_match(name, candidate))
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

/// Whether `text` matches `pattern`, where `*` is any run of characters and
/// `?` any one.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    // matches[j]: pattern[..i] matches text[..j], row by row.
    let mut matches = vec![false; text.len() + 1];
    matches[0] = true;
    for &p in &pattern {
        let mut next = vec![false; text.len() + 1];
        for j in 0..=text.len() {
            next[j] = match p {
                '*' => matches[j] || (j > 0 && next[j - 1]),
                '?' => j > 0 && matches[j - 1],
                c => j > 0 && matches[j - 1] && text[j - 1] == c,
            };
        }
        matches = next;
    }
    matches[text.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use std::fs;

    /// Writes `text` to `name` in `dir`, creating the directories it's in,
    /// and returns its path.
    fn write(dir: &TempDir, name: &str, text: &str) -> PathBuf {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create a directory");
        }
        fs::write(&path, text).expect("write a config file");
        path
    }

    /// The aliases of an ssh config holding `text`, written as `config` in
    /// `dir`. It must give no notice.
    fn aliases_of(dir: &TempDir, text: &str) -> Vec<String> {
        let (found, notices) = aliases(&write(dir, "config", text));
        assert!(notices.is_empty(), "{notices:?}");
        found
    }

    /// An ssh config with `depth` files included one inside the other:
    /// `config` includes `1.conf`, which includes `2.conf`, and so on.
    /// File n names `Host hn`.
    fn include_chain(dir: &TempDir, depth: usize) -> PathBuf {
        for n in 0..=depth {
            let name = if n == 0 {
                "config".to_owned()
            } else {
                format!("{n}.conf")
            };
            let include = if n < depth {
                format!("Include {}.conf\n", n + 1)
            } else {
                String::new()
            };
            write(dir, &name, &format!("Host h{n}\n{include}"));
        }
        dir.path().join("config")
    }

    /// Takes every permission away from `file`. `false` if it can still be
    /// read anyway, as it can by root.
    #[cfg(unix)]
    fn lock(file: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o000)).expect("chmod 000");
        fs::read(file).is_err()
    }

    /// `file` as an `Include` path through the home directory: `~/`, as
    /// many `..` as it takes to climb from there to the root, then the
    /// absolute path. Nothing in the home directory is read or written.
    #[cfg(unix)]
    fn through_home(file: &Path) -> String {
        // `..` climbs the real path, so count the real path's components.
        let home = dirs::home_dir()
            .and_then(|home| home.canonicalize().ok())
            .expect("a home directory");
        let up = "../".repeat(home.components().count() - 1);
        let below_root = file.strip_prefix("/").expect("an absolute path");
        format!("~/{up}{}", below_root.display())
    }

    #[test]
    fn keywords_ignore_case_and_may_be_followed_by_whitespace_or_one_equals_sign() {
        let dir = TempDir::new("ssh-config-keywords");
        write(&dir, "extra.conf", "Host included\n");
        let found = aliases_of(
            &dir,
            concat!(
                "HOST upper\n",
                "host=equals\n",
                "Host = spaced\n",
                "hOsT\ttabbed\n",
                "INCLUDE=extra.conf\n",
            ),
        );
        assert_eq!(found, ["upper", "equals", "spaced", "tabbed", "included"]);
    }

    /// Only `Host` names hosts; `HostName` and the rest set options.
    #[test]
    fn the_other_keywords_name_no_hosts() {
        let dir = TempDir::new("ssh-config-other-keywords");
        let found = aliases_of(
            &dir,
            concat!(
                "Host web\n",
                "  HostName web.internal.example.com\n",
                "  HostKeyAlias web-key\n",
                "  ProxyJump bastion\n",
                "  User alice\n",
            ),
        );
        assert_eq!(found, ["web"]);
    }

    #[test]
    fn arguments_split_on_whitespace_and_double_quotes_keep_spaces() {
        let dir = TempDir::new("ssh-config-arguments");
        write(&dir, "my hosts/extra.conf", "Host in-a-spaced-dir\n");
        let found = aliases_of(
            &dir,
            concat!(
                "Host one  two\tthree\n",
                "Host \"quoted\"\n",
                "Include \"my hosts/extra.conf\"\n",
            ),
        );
        assert_eq!(found, ["one", "two", "three", "quoted", "in-a-spaced-dir"]);
    }

    #[test]
    fn blank_lines_and_comments_are_skipped() {
        let dir = TempDir::new("ssh-config-comments");
        write(&dir, "extra.conf", "Host commented-include\n");
        let found = aliases_of(
            &dir,
            concat!(
                "# Host commented\n",
                "\n",
                "   \n",
                "  # Host indented-comment\n",
                "#Host no-space\n",
                "# Include extra.conf\n",
                "  Host indented\n",
            ),
        );
        assert_eq!(found, ["indented"]);
    }

    /// OpenSSH reads a `#` that starts an argument as a comment to the end
    /// of the line.
    #[test]
    fn a_comment_after_the_aliases_names_no_hosts() {
        let dir = TempDir::new("ssh-config-trailing-comment");
        let found = aliases_of(&dir, "Host web # the web server\n");
        assert_eq!(found, ["web"]);
    }

    /// An included file's hosts come where its `Include` stands.
    #[test]
    fn every_alias_is_listed_once_in_file_order() {
        let dir = TempDir::new("ssh-config-order");
        write(&dir, "middle.conf", "Host middle first\n");
        let found = aliases_of(&dir, "Host first\nInclude middle.conf\nHost last first\n");
        assert_eq!(found, ["first", "middle", "last"]);
    }

    /// Spec 0006 §Model: a pattern that matches many hosts, or none, is no
    /// host. ssh would read one that starts with `-` as an option.
    #[test]
    fn wildcard_negated_and_dash_patterns_are_not_hosts() {
        let dir = TempDir::new("ssh-config-patterns");
        let found = aliases_of(
            &dir,
            concat!(
                "Host *\n",
                "Host *.example.com web-?\n",
                "Host real !negated -oProxyCommand=evil also-real\n",
            ),
        );
        assert_eq!(found, ["real", "also-real"]);
    }

    /// `Match` applies settings by condition; it names no host, and the
    /// `Host` after it still does.
    #[test]
    fn match_blocks_add_no_hosts() {
        let dir = TempDir::new("ssh-config-match");
        let found = aliases_of(
            &dir,
            concat!(
                "Match host matched-only,other-matched\n",
                "  User alice\n",
                "Match originalhost original-only\n",
                "  User bob\n",
                "Host listed\n",
            ),
        );
        assert_eq!(found, ["listed"]);
    }

    /// One `Include` may name several files, relative or absolute.
    #[test]
    fn an_include_reads_each_path_it_names_in_the_order_named() {
        let dir = TempDir::new("ssh-config-include-paths");
        write(&dir, "zulu.conf", "Host zulu\n");
        let alpha = write(&dir, "elsewhere/alpha.conf", "Host alpha\n");
        let found = aliases_of(
            &dir,
            &format!("Include zulu.conf \"{}\"\n", alpha.display()),
        );
        assert_eq!(found, ["zulu", "alpha"]);
    }

    #[cfg(unix)]
    #[test]
    fn an_include_starting_with_a_tilde_starts_in_the_home_directory() {
        let dir = TempDir::new("ssh-config-include-tilde");
        let included = write(&dir, "included.conf", "Host through-home\n");
        let found = aliases_of(&dir, &format!("Include \"{}\"\n", through_home(&included)));
        assert_eq!(found, ["through-home"]);
    }

    /// As in ssh: relative to `~/.ssh`, even in a file that is itself
    /// included from a subdirectory.
    #[test]
    fn a_relative_include_starts_in_the_top_configs_directory_even_when_nested() {
        let dir = TempDir::new("ssh-config-include-relative");
        write(&dir, "conf.d/nested.conf", "Include leaf.conf\n");
        write(&dir, "leaf.conf", "Host from-the-top-directory\n");
        write(&dir, "conf.d/leaf.conf", "Host from-the-nested-directory\n");
        let found = aliases_of(&dir, "Include conf.d/nested.conf\n");
        assert_eq!(found, ["from-the-top-directory"]);
    }

    #[test]
    fn an_include_wildcard_reads_the_matching_files_in_sorted_order() {
        let dir = TempDir::new("ssh-config-include-wildcard");
        // Written out of order, to be read in order.
        write(&dir, "conf.d/c.conf", "Host charlie\n");
        write(&dir, "conf.d/a.conf", "Host alpha\n");
        write(&dir, "conf.d/b.conf", "Host bravo\n");
        write(&dir, "conf.d/notes.txt", "Host not-a-conf\n");
        write(&dir, "conf.d/web2.cfg", "Host web-two\n");
        write(&dir, "conf.d/web10.cfg", "Host web-ten\n");
        write(&dir, "conf.d/web1.cfg", "Host web-one\n");
        let found = aliases_of(&dir, "Include conf.d/*.conf\nInclude conf.d/web?.cfg\n");
        assert_eq!(found, ["alpha", "bravo", "charlie", "web-one", "web-two"]);
    }

    /// As in ssh, a directory an `Include` glob matches is skipped: the
    /// files around it are still read, and it gives no notice.
    #[test]
    fn a_directory_an_include_glob_matches_is_skipped_without_a_notice() {
        let dir = TempDir::new("ssh-config-include-glob-dir");
        // `old` sorts between the two files.
        write(&dir, "conf.d/alpha.conf", "Host alpha\n");
        write(&dir, "conf.d/old/stale.conf", "Host stale\n");
        write(&dir, "conf.d/zulu.conf", "Host zulu\n");
        let found = aliases_of(&dir, "Include conf.d/*\n");
        assert_eq!(found, ["alpha", "zulu"]);
    }

    /// As in ssh, an `Include` that names no file is no mistake.
    #[test]
    fn an_include_that_matches_nothing_is_silently_nothing() {
        let dir = TempDir::new("ssh-config-include-nothing");
        fs::create_dir(dir.path().join("empty")).expect("create a directory");
        let found = aliases_of(
            &dir,
            concat!(
                "Include missing.conf\n",
                "Include no-such-dir/*.conf\n",
                "Include empty/*\n",
                "Host kept\n",
            ),
        );
        assert_eq!(found, ["kept"]);
    }

    /// Spec 0006, flow 8.
    #[cfg(unix)]
    #[test]
    fn an_include_that_cant_be_read_gives_a_notice_naming_it_and_only_its_hosts_are_missing() {
        let dir = TempDir::new("ssh-config-include-unreadable");
        let locked = write(&dir, "locked.conf", "Host locked-away\n");
        if !lock(&locked) {
            return; // Root reads any file: nothing to test.
        }
        let config = write(
            &dir,
            "config",
            "Host before\nInclude locked.conf\nHost after\n",
        );
        let (found, notices) = aliases(&config);
        assert_eq!(found, ["before", "after"]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("locked.conf"), "{}", notices[0]);
    }

    /// Spec 0006, flow 8.
    #[cfg(unix)]
    #[test]
    fn an_ssh_config_that_cant_be_read_gives_a_notice_naming_it() {
        let dir = TempDir::new("ssh-config-unreadable");
        let config = write(&dir, "my-ssh-config", "Host locked-away\n");
        if !lock(&config) {
            return; // Root reads any file: nothing to test.
        }
        let (found, notices) = aliases(&config);
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("my-ssh-config"), "{}", notices[0]);
    }

    /// As in OpenSSH, whose limit is 16 below the top config.
    #[test]
    fn includes_16_deep_are_all_read() {
        let dir = TempDir::new("ssh-config-include-16-deep");
        let (found, notices) = aliases(&include_chain(&dir, 16));
        let all: Vec<String> = (0..=16).map(|n| format!("h{n}")).collect();
        assert_eq!(found, all);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// The limit also ends an `Include` loop.
    #[test]
    fn an_include_17_deep_gives_a_notice_instead_of_being_read() {
        let dir = TempDir::new("ssh-config-include-17-deep");
        let (found, notices) = aliases(&include_chain(&dir, 17));
        assert_eq!(found.first().map(String::as_str), Some("h0"), "{found:?}");
        assert!(!found.iter().any(|host| host == "h17"), "{found:?}");
        assert_eq!(notices.len(), 1, "{notices:?}");
    }

    /// Plenty of users have no ssh config, or no `~/.ssh` at all.
    #[test]
    fn a_missing_ssh_config_is_no_hosts_and_no_notice() {
        let dir = TempDir::new("ssh-config-missing");
        let (found, notices) = aliases(&dir.path().join(".ssh").join("config"));
        assert!(found.is_empty(), "{found:?}");
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Windows editors end lines in CRLF.
    #[test]
    fn a_config_with_crlf_line_endings_lists_the_same_hosts() {
        let dir = TempDir::new("ssh-config-crlf");
        write(&dir, "extra.conf", "Host included\r\n");
        let found = aliases_of(&dir, "Host web db\r\nInclude extra.conf\r\n");
        assert_eq!(found, ["web", "db", "included"]);
    }

    /// ssh reads its config as bytes: a comment in Latin-1 is no reason to
    /// miss a file's hosts.
    #[test]
    fn a_config_that_isnt_utf_8_still_lists_its_hosts() {
        let dir = TempDir::new("ssh-config-latin-1");
        let config = dir.path().join("config");
        fs::write(&config, b"# Server im B\xfcro\nHost office\n").expect("write the config");
        let (found, notices) = aliases(&config);
        assert_eq!(found, ["office"]);
        assert!(notices.is_empty(), "{notices:?}");
    }
}
