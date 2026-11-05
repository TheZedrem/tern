//! Links in the output (spec 0002, flow 7): OSC 8 hyperlinks and detected
//! URLs, underlined on hover and opened on Ctrl/Cmd+click. Only `http` and
//! `https` targets are links at all [load-bearing] (TM-escape-03), and none
//! longer than 2 KiB (TM-escape-10).

use super::clean_title;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};

/// The longest target zed-term opens.
const MAX_TARGET: usize = 2048;

/// What a detected URL looks like: no spaces, controls or the characters
/// that usually end one in text.
const URL: &str = r#"https?://[^\x00-\x1F\x7F-\x9F<>"\s{}^`\\⟨⟩]+"#;

/// A link in the grid: the cells it covers and where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub range: Match,
    pub target: String,
}

/// The search that finds URLs, built on first hover: link detection costs
/// nothing at startup (spec 0002, §Startup impact).
pub fn detector() -> RegexSearch {
    RegexSearch::new(URL).expect("the URL pattern is valid")
}

/// The link under `point`, if it's one zed-term opens: an OSC 8 hyperlink,
/// else a URL in the text of that row.
pub fn at<T>(term: &Term<T>, point: Point, urls: &mut RegexSearch) -> Option<Link> {
    hyperlink_at(term, point)
        .or_else(|| url_at(term, point, urls))
        .filter(|link| openable(&link.target))
}

/// The OSC 8 hyperlink under `point`, across the cells of its row that
/// carry it.
fn hyperlink_at<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let link = grid[point].hyperlink()?;
    let row = &grid[point.line];
    let carries = |col: usize| row[Column(col)].hyperlink().as_ref() == Some(&link);
    let (mut start, mut end) = (point.column.0, point.column.0);
    while start > 0 && carries(start - 1) {
        start -= 1;
    }
    while end + 1 < term.columns() && carries(end + 1) {
        end += 1;
    }
    Some(Link {
        range: Point::new(point.line, Column(start))..=Point::new(point.line, Column(end)),
        target: link.uri().to_owned(),
    })
}

/// The URL in `point`'s row that covers it, without the punctuation prose
/// puts after one.
fn url_at<T>(term: &Term<T>, point: Point, urls: &mut RegexSearch) -> Option<Link> {
    let row_start = Point::new(point.line, Column(0));
    let row_end = Point::new(point.line, Column(term.columns() - 1));
    let found = RegexIter::new(row_start, row_end, Direction::Right, term, urls)
        .find(|found| found.contains(&point))?;
    let text = term.bounds_to_string(*found.start(), *found.end());
    let target = text.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
    let trimmed = text.chars().count() - target.chars().count();
    let end = Point::new(
        found.end().line,
        Column(found.end().column.0.saturating_sub(trimmed)),
    );
    let range = *found.start()..=end;
    range.contains(&point).then(|| Link {
        range,
        target: target.to_owned(),
    })
}

/// Whether zed-term opens `target`: an `http` or `https` URL with a host,
/// at most 2 KiB.
pub fn openable(target: &str) -> bool {
    target.len() <= MAX_TARGET && host(target).is_some_and(|host| !host.is_empty())
}

/// The host of an `http` or `https` URL; `None` for any other scheme.
pub fn host(target: &str) -> Option<&str> {
    let (scheme, rest) = target.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    // Browsers end an http(s) host at `\` too, so a target can't hide its
    // real host behind one (TM-escape-02).
    let authority = rest.split(['/', '?', '#', '\\']).next()?;
    Some(
        authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host),
    )
}

/// What hovering a link shows: its target as plain text by the title
/// rules, marked when the host has non-ASCII letters, which can pass for
/// others (TM-escape-02).
pub fn hint(target: &str) -> String {
    let shown = clean_title(target);
    if host(target).is_some_and(|host| !host.is_ascii()) {
        format!("{shown}  (the host has non-ASCII letters)")
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::tests::term_with;
    use alacritty_terminal::index::Line;

    /// Cell `col` of the top row.
    fn top(col: usize) -> Point {
        Point::new(Line(0), Column(col))
    }

    /// The link under cell `col` of `term`'s top row, as hovering finds it.
    fn link_at<T>(term: &Term<T>, col: usize) -> Option<Link> {
        at(term, top(col), &mut detector())
    }

    /// TM-escape-03, -10 [load-bearing]: only `http` and `https` targets,
    /// in any case, are links; `javascript:`, `file:`, `ms-msdt:`, an
    /// app's own scheme and the rest never reach the OS handler. Neither
    /// does a target over 2 KiB.
    #[test]
    fn only_http_and_https_targets_up_to_2_kib_are_links() {
        let links = [
            "http://example.com",
            "https://example.com/docs?page=2#top",
            "HTTPS://EXAMPLE.COM/",
            "hTtP://example.com/",
        ];
        for target in links {
            assert!(openable(target), "{target}");
        }
        let refused = [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "javascript://example.com/%0Aalert(1)",
            "file:///etc/passwd",
            "FILE://localhost/etc/passwd",
            "ms-msdt:/id PCWDiagnostic /skip force",
            "vscode://file/etc/passwd",
            "ftp://example.com/",
            "data:text/html,<script>alert(1)</script>",
            "mailto:alice@example.com",
            "example.com",
            "https://",
        ];
        for target in refused {
            assert!(!openable(target), "{target}");
        }
        let base = "https://example.com/";
        let long = |bytes: usize| format!("{base}{}", "a".repeat(bytes - base.len()));
        assert!(openable(&long(2048)));
        assert!(!openable(&long(2049)));
    }

    /// TM-escape-02: hovering shows the real target as plain text, by the
    /// title rules, and marks a host with non-ASCII letters, which can pass
    /// for ASCII ones; the userinfo before an `@` is no host, however much
    /// it looks like one.
    #[test]
    fn the_hint_is_the_target_as_plain_text_marked_when_its_host_has_non_ascii_letters() {
        let plain = "https://example.com/docs?page=2";
        assert_eq!(hint(plain), plain);
        let shown = hint("https://example.com/\u{202E}gpj.exe\x1b[8m\n");
        assert!(shown.starts_with("https://example.com/"), "{shown:?}");
        assert!(!shown.contains(['\u{202E}', '\x1b', '\n']), "{shown:?}");
        // "exаmple" with a Cyrillic а, alone and behind an ASCII userinfo.
        let lookalikes = [
            "https://ex\u{0430}mple.com/login",
            "https://example.com@ex\u{0430}mple.org/login",
        ];
        for target in lookalikes {
            let marked = hint(target);
            assert!(
                marked.starts_with(target) && marked != target,
                "{target}: {marked:?}"
            );
        }
    }

    /// TM-escape-02, on the parsed URL: browsers end the host at a
    /// backslash as at a slash (WHATWG URL Standard), so this target opens
    /// the look-alike `exаmple.com`, not `example.org`, and its hint must
    /// be marked.
    #[test]
    fn a_non_ascii_host_that_a_backslash_ends_is_marked() {
        let target = "https://ex\u{0430}mple.com\\@example.org/";
        let marked = hint(target);
        assert!(marked.starts_with(target) && marked != target, "{marked:?}");
    }

    /// Spec 0002, flow 7: an OSC 8 hyperlink is a link over exactly the
    /// cells of its text, even right next to another one; the cells beside
    /// it are no link.
    #[test]
    fn an_osc_8_hyperlink_covers_exactly_its_own_cells() {
        // "see onetwo end", with "one" and "two" linked to different targets.
        let term = term_with(
            20,
            1,
            b"see \x1b]8;;https://one.example/\x1b\\one\
              \x1b]8;;https://two.example/\x1b\\two\x1b]8;;\x1b\\ end",
        );
        let one = Link {
            range: top(4)..=top(6),
            target: "https://one.example/".into(),
        };
        let two = Link {
            range: top(7)..=top(9),
            target: "https://two.example/".into(),
        };
        for col in 4..=6 {
            assert_eq!(link_at(&term, col), Some(one.clone()), "column {col}");
        }
        for col in 7..=9 {
            assert_eq!(link_at(&term, col), Some(two.clone()), "column {col}");
        }
        for col in [0, 3, 10, 12] {
            assert_eq!(link_at(&term, col), None, "column {col}");
        }
    }

    /// AC8 (spec 0002), TM-escape-03 [load-bearing]: an OSC 8 link to
    /// `javascript:alert(1)`, `file:///etc/passwd` or any other non-web
    /// target is no link at all, so Ctrl+click has nothing to open, while
    /// an `https://` one is.
    #[test]
    fn an_osc_8_link_to_anything_but_http_or_https_is_no_link() {
        let printed = |target: &str| {
            let bytes = format!("\x1b]8;;{target}\x1b\\click me\x1b]8;;\x1b\\");
            term_with(20, 1, bytes.as_bytes())
        };
        let refused = [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ms-msdt:/id PCWDiagnostic",
            "vscode://file/etc/passwd",
        ];
        for target in refused {
            let term = printed(target);
            for col in 0..8 {
                assert_eq!(link_at(&term, col), None, "{target} at column {col}");
            }
        }
        let term = printed("https://example.com/");
        let opened = link_at(&term, 2).map(|link| link.target);
        assert_eq!(opened.as_deref(), Some("https://example.com/"));
    }

    /// Spec 0002, flow 7: a URL in the text is a link up to its last
    /// character, without the punctuation a sentence puts after it. That
    /// punctuation and the words around the URL are no link.
    #[test]
    fn a_url_in_the_text_is_a_link_without_the_punctuation_after_it() {
        let term = term_with(
            80,
            1,
            b"Docs: https://example.com/guide. Or https://example.org/a, b",
        );
        let guide = Link {
            range: top(6)..=top(30),
            target: "https://example.com/guide".into(),
        };
        for col in [6, 20, 30] {
            assert_eq!(link_at(&term, col), Some(guide.clone()), "column {col}");
        }
        let a = Link {
            range: top(36)..=top(56),
            target: "https://example.org/a".into(),
        };
        assert_eq!(link_at(&term, 40), Some(a));
        // "Docs", the full stop, "Or", the comma and "b".
        for col in [0, 31, 33, 57, 59] {
            assert_eq!(link_at(&term, col), None, "column {col}");
        }
    }
}
