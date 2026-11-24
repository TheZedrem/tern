//! What never enters the command history, in memory or on disk (spec
//! 0021, never-stored rules and token patterns; TM-escape-16). The rules
//! err toward dropping: a missed secret lands in a plain-text file, while
//! a dropped harmless command only isn't suggested.

use regex::RegexSet;
use std::sync::LazyLock;

/// Longer commands aren't stored.
const MAX_LEN: usize = 4096;

/// Words that mark a command as holding a secret, matched ignoring case:
/// PSReadLine's rule for what it keeps out of its history file.
const KEYWORDS: &[&str] = &["password", "passwd", "secret", "token", "apikey", "api_key"];

/// Credentials by their shape. Adding one is a code change, never a
/// setting.
const PATTERNS: &[&str] = &[
    // A JWT: three base64url parts, the header's JSON starting `{"`.
    r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
    // An Authorization header, or a Bearer or Basic credential.
    r"(?i)\bauthorization\s*:",
    // Bearer in any case; Basic only capitalized as HTTP writes it, so
    // "basic settings" isn't one.
    r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,}",
    r"\bBasic\s+[A-Za-z0-9+/]{12,}",
    // Known token prefixes: GitHub, GitLab, Slack, AWS, OpenAI/Anthropic.
    r"\bgh[opusr]_[A-Za-z0-9]{20,}",
    r"\bgithub_pat_[A-Za-z0-9_]{20,}",
    r"\bglpat-[A-Za-z0-9_-]{20,}",
    r"\bxox[a-z]-[A-Za-z0-9-]{10,}",
    r"\bAKIA[0-9A-Z]{16}",
    r"\bsk-[A-Za-z0-9_-]{20,}",
    // A PEM key or certificate pasted inline.
    r"-----BEGIN",
    // A password in a URL: scheme://user:pass@.
    r"[A-Za-z][A-Za-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@",
];

/// The compiled patterns; `None` if they don't compile, which drops every
/// command rather than letting a secret through.
static SET: LazyLock<Option<RegexSet>> = LazyLock::new(|| RegexSet::new(PATTERNS).ok());

/// Whether `command` may enter the history.
pub fn storable(command: &str) -> bool {
    let Some(set) = SET.as_ref() else {
        return false;
    };
    let lower = command.to_lowercase();
    !command.trim().is_empty()
        && !command.starts_with(char::is_whitespace)
        && !command.contains(['\n', '\r'])
        && command.len() <= MAX_LEN
        && !KEYWORDS.iter().any(|word| lower.contains(word))
        && !set.is_match(command)
        && !command.split(separator).any(random_word)
}

/// What ends a word for [`random_word`]: whitespace, quotes, and what
/// joins a value to its name.
fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '=' | '\'' | '"' | ':' | ',' | ';' | '(' | ')')
}

/// A word that looks like a random key: 32 or more hex digits (a full git
/// SHA counts, by design), or 32 or more base64 characters mixing digits,
/// upper and lower case with few separators. A path (`/`, `~` or `.`
/// first) or a long kebab-case name isn't one.
fn random_word(word: &str) -> bool {
    let word = word.trim_end_matches('=');
    if word.len() < 32 {
        return false;
    }
    if word.bytes().all(|b| b.is_ascii_hexdigit()) {
        return true;
    }
    let base64 = word
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_'));
    let separators = word
        .bytes()
        .filter(|b| matches!(b, b'+' | b'/' | b'-' | b'_'))
        .count();
    base64
        && !word.starts_with(['/', '~', '.'])
        && word.bytes().any(|b| b.is_ascii_digit())
        && word.bytes().any(|b| b.is_ascii_uppercase())
        && word.bytes().any(|b| b.is_ascii_lowercase())
        && separators * 8 <= word.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_dropped(commands: &[&str]) {
        for command in commands {
            assert!(!storable(command), "stored {command:?}");
        }
    }

    /// Spec 0021, never stored: a leading space or tab is the user's way
    /// to keep a command out, as with `HISTCONTROL=ignorespace`.
    #[test]
    fn a_command_starting_with_whitespace_is_dropped() {
        assert_dropped(&[" echo hi", "\techo hi", "  git status"]);
        assert!(storable("echo hi"));
    }

    /// Spec 0021, never stored, and TM-escape-17: a multi-line command
    /// could run without Enter when accepted, so it never enters the
    /// history.
    #[test]
    fn a_command_spanning_more_than_one_line_is_dropped() {
        assert_dropped(&[
            "echo a\necho b",
            "echo a\r\necho b",
            "echo a\recho b",
            "for f in *; do\n  echo $f\ndone",
        ]);
    }

    /// Spec 0021, never stored, and TM-escape-19: longer than 4 KiB is
    /// dropped, 4 KiB itself is kept.
    #[test]
    fn a_command_over_4_kib_is_dropped() {
        let at_limit = format!("echo {}", "x".repeat(4096 - "echo ".len()));
        assert_eq!(at_limit.len(), 4096);
        assert!(storable(&at_limit), "4 KiB exactly is kept");
        let over = format!("{at_limit}x");
        assert!(!storable(&over), "4 KiB and a byte is stored");
    }

    /// Spec 0021, never stored: PSReadLine's keywords, ignoring case,
    /// anywhere in the line (acceptance criterion 4's `mysql`).
    #[test]
    fn a_command_with_a_secret_keyword_is_dropped_ignoring_case() {
        assert_dropped(&[
            "mysql --password=hunter2",
            "mysql -u root --PASSWORD hunter2",
            "echo $DB_PASSWD",
            "export AWS_SECRET_ACCESS_KEY=abc",
            "vault kv get mySecret",
            "export TOKEN=abc",
            "gh auth login --with-token",
            "curl -H 'X-ApiKey: abc' https://example.com",
            "export OPENAI_API_KEY=abc",
            "echo api_key",
        ]);
    }

    /// Spec 0021, token patterns: a JWT anywhere in the line (acceptance
    /// criterion 4), even with parts too short to look random.
    #[test]
    fn a_jwt_is_dropped() {
        assert_dropped(&[
            "echo eyJhbGciOi.eyJzdWIi.sig",
            "http get https://api.example.com x-jwt:eyJhbGciOi.eyJzdWIi.sig",
            "curl -b session=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln https://example.com",
        ]);
    }

    /// Spec 0021, token patterns: an `Authorization:` header, or a
    /// `Bearer` or `Basic` value (acceptance criterion 4's `curl`).
    #[test]
    fn an_authorization_header_or_bearer_or_basic_value_is_dropped() {
        assert_dropped(&[
            r#"curl -H "Authorization: Bearer abc" https://example.com"#,
            "curl -H 'authorization:abc' https://example.com",
            "http https://example.com 'Bearer abcdefgh12345'",
            "curl -H 'X-Auth: Basic dXNlcjpodW50ZXIy' https://example.com",
        ]);
    }

    /// Spec 0021, token patterns: each known token prefix, one by one,
    /// each value too short to be caught as a random word instead.
    #[test]
    fn each_known_token_prefix_is_dropped() {
        for (name, command) in [
            ("GitHub ghp_", "export GH=ghp_aBcD1234eFgH5678iJkL"),
            ("GitHub gho_", "export GH=gho_aBcD1234eFgH5678iJkL"),
            (
                "GitHub github_pat_",
                "export GH=github_pat_11ABCDEFG0aBcD12eFgH",
            ),
            ("GitLab glpat-", "export GL=glpat-aBcD1234eFgH5678iJkL"),
            ("Slack xoxb-", "export SLACK=xoxb-1234567890-abcdef"),
            ("Slack xoxp-", "export SLACK=xoxp-1234567890-abcdef"),
            ("AWS AKIA", "export AWS=AKIAIOSFODNN7EXAMPLE"),
            ("OpenAI/Anthropic sk-", "export KEY=sk-ant-aBcD1234eFgH5678"),
            ("PEM", "echo '-----BEGIN RSA PRIVATE KEY-----' > id_rsa"),
        ] {
            assert!(!storable(command), "{name}: stored {command:?}");
        }
    }

    /// Acceptance criterion 4: `export GH=ghp_<36 chars>`.
    #[test]
    fn a_full_github_token_is_dropped() {
        let token = format!("ghp_{}", "aB3dE5gH7jK9mN1pQ3sT5vX7zA9cE1gI2kM4");
        assert_eq!(token.len(), 40);
        assert!(!storable(&format!("export GH={token}")));
    }

    /// Spec 0021, token patterns: a credential in a URL.
    #[test]
    fn a_url_with_credentials_is_dropped() {
        assert_dropped(&[
            "git clone https://me:hunter2@example.com/repo.git",
            "psql postgres://app:s3cr3t@db.internal:5432/app",
            "redis-cli -u redis://default:pass@localhost:6379",
        ]);
        assert!(storable("git clone https://example.com/repo.git"));
        assert!(storable("git clone git@example.com:group/repo.git"));
    }

    /// Spec 0021, token patterns: 32 or more hex characters as a word of
    /// their own, which drops a full git SHA by design (acceptance
    /// criterion 4).
    #[test]
    fn a_run_of_32_or_more_hex_characters_is_dropped() {
        assert_dropped(&[
            "git show 3f786850e387550fdab836ed7e6dc881de23001b",
            "export KEY=0123456789abcdef0123456789abcdef",
            "curl -H 'X-Key: 0123456789ABCDEF0123456789ABCDEF' https://example.com",
        ]);
        assert!(storable("git show 3f78685"), "a short SHA is kept");
    }

    /// Spec 0021, token patterns: 32 or more random base64 characters as a
    /// word of their own.
    #[test]
    fn a_random_base64_word_is_dropped() {
        assert_dropped(&[
            "export KEY=aB3dE5gH7jK9mN1pQ3sT5vX7zA9cE1gI",
            "some-cli --key aB3dE5gH7jK9mN1pQ3sT5vX7zA9cE1gI2kM4oQ6s==",
            "echo 'Zm9vYmFyQmF6UXV4MTIzNDU2Nzg5MGFiY2RlZg=='",
            "deploy --key=k8+Zq/3Lm9Xw2Rt7Yv4Np1Bc6Hs0Gd5Fj8",
        ]);
    }

    /// Spec 0021: the rules err toward dropping, but ordinary commands
    /// (paths, long kebab-case names, plain words such as "basic") are
    /// kept, or the history would be useless.
    #[test]
    fn ordinary_commands_are_kept() {
        for command in [
            "git status",
            "cargo build --release",
            "cargo test -- --nocapture",
            "cd /home/me/projects/some-long-folder-name-2024",
            "cd ~/projects/SomeVeryLongProjectFolderName2024/src/history",
            "npm run build-and-deploy-the-whole-staging-cluster-now",
            "git checkout feature/T-106-record-command-history",
            "kubectl get pods -n kube-system -o wide",
            "docker run --rm -it -e FOO=bar ubuntu:24.04 bash",
            "ssh deploy@prod.example.com",
            "echo basic settings",
            "echo Basic settings",
            r"Set-Location 'C:\Users\me\OneDrive - Firma\Documents'",
            "ls -la | grep äöü",
        ] {
            assert!(storable(command), "dropped {command:?}");
        }
    }

    #[test]
    fn an_empty_or_blank_command_is_dropped() {
        assert_dropped(&["", " ", "\t"]);
    }
}
