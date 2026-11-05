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
    let authority = rest.split(['/', '?', '#']).next()?;
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
