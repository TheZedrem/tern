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
