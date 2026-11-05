//! The search bar's query (spec 0002, flow 6): matches through the
//! scrollback with alacritty_terminal's regex search, the query taken as
//! plain text unless regex is on, and case-sensitive only when asked.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};

/// The most matches highlighted on screen at once.
const MAX_SHOWN: usize = 1000;

/// The search for `query`, or `None` for an empty query or an invalid
/// regex.
pub fn compile(query: &str, case_sensitive: bool, regex: bool) -> Option<RegexSearch> {
    if query.is_empty() {
        return None;
    }
    let body = if regex {
        query.to_owned()
    } else {
        escape(query)
    };
    // The inline flag outranks alacritty's own smart case.
    let flags = if case_sensitive { "(?-i)" } else { "(?i)" };
    RegexSearch::new(&format!("{flags}{body}")).ok()
}

/// `text` with every regex metacharacter escaped, so it matches itself.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// The match after `from`: older, above it, when `up`, else newer,
/// wrapping around the scrollback. Without `from`, the newest match, from
/// the bottom of the screen up.
pub fn step<T>(
    term: &Term<T>,
    search: &mut RegexSearch,
    from: Option<&Match>,
    up: bool,
) -> Option<Match> {
    let (origin, direction) = match (from, up) {
        (Some(current), true) => (
            current.start().sub(term, Boundary::None, 1),
            Direction::Left,
        ),
        (Some(current), false) => (current.end().add(term, Boundary::None, 1), Direction::Right),
        (None, _) => {
            let bottom = Line(term.screen_lines() as i32 - 1);
            (
                Point::new(bottom, Column(term.columns() - 1)),
                Direction::Left,
            )
        }
    };
    let side = match direction {
        Direction::Right => Side::Left,
        Direction::Left => Side::Right,
    };
    term.search_next(search, origin, direction, side, None)
}

/// The matches on screen, in reading order, at most [`MAX_SHOWN`].
pub fn on_screen<T>(term: &Term<T>, search: &mut RegexSearch) -> Vec<Match> {
    let offset = term.grid().display_offset() as i32;
    let top = Point::new(Line(-offset), Column(0));
    let bottom = Point::new(
        Line(term.screen_lines() as i32 - 1 - offset),
        Column(term.columns() - 1),
    );
    RegexIter::new(top, bottom, Direction::Right, term, search)
        .take(MAX_SHOWN)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::tests::term_with;
    use alacritty_terminal::grid::Scroll;

    /// What the matches of `query` on `term`'s screen read, in order.
    fn found<T>(term: &Term<T>, query: &str, case_sensitive: bool, regex: bool) -> Vec<String> {
        let mut search = compile(query, case_sensitive, regex).expect("a search");
        on_screen(term, &mut search)
            .iter()
            .map(|found| term.bounds_to_string(*found.start(), *found.end()))
            .collect()
    }

    /// Where `found` is: its line (negative in the scrollback), its first
    /// and its last column.
    fn place(found: &Match) -> (i32, usize, usize) {
        let (start, end) = (found.start(), found.end());
        (start.line.0, start.column.0, end.column.0)
    }

    /// Spec 0002, flow 6: there is nothing to search for while the query
    /// is empty, or, with regex on, while it isn't a valid regex. As plain
    /// text the same query is fine.
    #[test]
    fn an_empty_query_or_an_invalid_regex_searches_nothing() {
        let toggles = [(false, false), (true, false), (false, true), (true, true)];
        for (case_sensitive, regex) in toggles {
            assert!(compile("", case_sensitive, regex).is_none());
        }
        for invalid in ["(", "[a-", "*"] {
            assert!(compile(invalid, false, true).is_none(), "{invalid}");
            let text = compile(invalid, false, false);
            assert!(text.is_some(), "{invalid} as text");
        }
    }

    /// Spec 0002, flow 6: with the regex toggle off the query is plain
    /// text, so `a.b` finds only itself and a command finds itself as
    /// typed; with it on, the query is a regex.
    #[test]
    fn a_query_is_literal_text_unless_regex_is_on() {
        let term = term_with(40, 2, b"axb a.b (1+1)*2\r\nrm -rf ~/tmp x12 y345");
        assert_eq!(found(&term, "a.b", false, false), ["a.b"]);
        assert_eq!(found(&term, "(1+1)*2", false, false), ["(1+1)*2"]);
        assert_eq!(found(&term, "rm -rf ~/tmp", false, false), ["rm -rf ~/tmp"]);
        assert_eq!(found(&term, "a.b", false, true), ["axb", "a.b"]);
        assert_eq!(found(&term, r"[xy]\d+", false, true), ["x12", "y345"]);
        assert!(found(&term, r"[xy]\d+", false, false).is_empty());
    }

    /// Spec 0002, flow 6: search ignores case until the case-sensitive
    /// toggle is on, also for a query with capitals, which
    /// alacritty_terminal's own smart case would take as case-sensitive.
    #[test]
    fn search_ignores_case_unless_case_sensitive_is_on() {
        let term = term_with(20, 1, b"Foo foo FOO");
        for query in ["foo", "Foo"] {
            assert_eq!(
                found(&term, query, false, false),
                ["Foo", "foo", "FOO"],
                "{query}"
            );
        }
        assert_eq!(found(&term, "Foo", true, false), ["Foo"]);
        assert_eq!(found(&term, "foo", true, false), ["foo"]);
    }

    /// Spec 0002, flow 6: a search starts at the newest match, at the
    /// bottom. Next goes up to older output and previous down to newer,
    /// both wrapping around the scrollback.
    #[test]
    fn stepping_starts_at_the_newest_match_and_wraps_around_the_scrollback() {
        // Six lines on three rows: the first three are in the scrollback.
        let term = term_with(20, 3, b"match 1\r\nx\r\nmatch 2\r\nx\r\nmatch 3\r\nx");
        let mut search = compile("match", false, false).expect("a search");
        // The lines of four steps `up` (or down), from no match on.
        let mut walk = |up: bool| {
            let mut current = None;
            let mut lines = Vec::new();
            for _ in 0..4 {
                current = step(&term, &mut search, current.as_ref(), up);
                lines.push(current.as_ref().expect("a match").start().line.0);
            }
            lines
        };
        assert_eq!(walk(true), [1, -1, -3, 1]);
        assert_eq!(walk(false), [1, -3, -1, 1]);
    }

    /// Spec 0002, flow 6: the matches highlighted are those on screen, in
    /// reading order, and only those, also while scrolled back.
    #[test]
    fn only_the_matches_on_screen_are_highlighted_in_reading_order() {
        // Five lines on three rows: the first two are in the scrollback.
        let mut term = term_with(20, 3, b"foo old\r\nx\r\nfoo bar foo\r\nx\r\nfoo");
        let mut search = compile("foo", false, false).expect("a search");
        let shown: Vec<_> = on_screen(&term, &mut search).iter().map(place).collect();
        assert_eq!(shown, [(0, 0, 2), (0, 8, 10), (2, 0, 2)]);
        term.scroll_display(Scroll::Delta(2));
        let shown: Vec<_> = on_screen(&term, &mut search).iter().map(place).collect();
        assert_eq!(shown, [(-2, 0, 2), (0, 0, 2), (0, 8, 10)]);
    }

    /// AC5 (spec 0002): search finds a term 5 000 lines up in the
    /// scrollback, and once the view scrolls to it, as the pane does, it's
    /// among the matches on screen to highlight.
    #[test]
    fn search_finds_a_term_5000_lines_up_and_brings_it_on_screen() {
        let mut output = b"needle\r\n".to_vec();
        output.extend(b"hay\r\n".repeat(5000));
        let mut term = term_with(80, 24, &output);
        let mut search = compile("needle", false, false).expect("a search");
        let found = step(&term, &mut search, None, true).expect("the needle");
        // 5 002 lines on 24 rows: the first is 4 978 lines up.
        assert_eq!(place(&found), (-4978, 0, 5));
        term.scroll_to_point(*found.start());
        assert!(on_screen(&term, &mut search).contains(&found));
    }
}
