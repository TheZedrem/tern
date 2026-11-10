//! The search bar's query (spec 0002, flow 6): matches through the
//! scrollback with alacritty_terminal's regex search, the query taken as
//! plain text unless regex is on, and case-sensitive only when asked.

use super::{CloseSearch, SEARCH_CONTEXT, State, TerminalPane, grid, search};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Styled,
    Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Selectable, Sizable};

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

/// The search bar while it's open (spec 0002, flow 6).
pub(super) struct SearchBar {
    input: Entity<InputState>,
    case_sensitive: bool,
    regex: bool,
    /// The query, compiled; `None` while it's empty or not a valid regex.
    compiled: Option<RegexSearch>,
    /// The match the view is on.
    current: Option<Match>,
    _events: Subscription,
}

impl TerminalPane {
    /// Opens the search bar, or selects its query if it's open (spec 0002,
    /// flow 6).
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, State::Running(_)) {
            return;
        }
        if let Some(bar) = &self.search {
            bar.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let events = cx.subscribe_in(&input, window, |pane, _, event: &InputEvent, _, cx| {
            match event {
                InputEvent::Change => pane.search_changed(cx),
                // Enter goes up to older output, Shift+Enter back down.
                InputEvent::PressEnter { shift, .. } => pane.search_step(!shift, cx),
                _ => {}
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.search = Some(SearchBar {
            input,
            case_sensitive: false,
            regex: false,
            compiled: None,
            current: None,
            _events: events,
        });
        cx.notify();
    }

    /// Closes the search bar; the keyboard goes back to the program.
    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Searches anew for the query as it is now, from the newest output up.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let Some(bar) = &mut self.search else {
            return;
        };
        let query = bar.input.read(cx).value().to_string();
        bar.compiled = search::compile(&query, bar.case_sensitive, bar.regex);
        bar.current = None;
        self.search_step(true, cx);
    }

    /// Moves to the next match, up to older output or down to newer, and
    /// scrolls it into view.
    fn search_step(&mut self, up: bool, cx: &mut Context<Self>) {
        let (Some(bar), State::Running(running)) = (&mut self.search, &self.state) else {
            return;
        };
        let Some(compiled) = &mut bar.compiled else {
            cx.notify();
            return;
        };
        let mut term = running.term.lock();
        bar.current = search::step(&term, compiled, bar.current.as_ref(), up);
        if let Some(found) = &bar.current {
            term.scroll_to_point(*found.start());
        }
        cx.notify();
    }

    fn toggle_search_option(&mut self, regex: bool, cx: &mut Context<Self>) {
        if let Some(bar) = &mut self.search {
            if regex {
                bar.regex = !bar.regex;
            } else {
                bar.case_sensitive = !bar.case_sensitive;
            }
            self.search_changed(cx);
        }
    }

    /// The search highlights on screen now.
    pub(super) fn marks(&mut self) -> grid::Marks {
        let link = self.hovered_link.as_ref().map(|link| link.range.clone());
        let (Some(bar), State::Running(running)) = (&mut self.search, &self.state) else {
            return grid::Marks {
                link,
                ..Default::default()
            };
        };
        let Some(compiled) = &mut bar.compiled else {
            return grid::Marks {
                link,
                ..Default::default()
            };
        };
        let term = running.term.lock();
        grid::Marks {
            matches: search::on_screen(&term, compiled),
            current: bar.current.clone(),
            link,
            ..Default::default()
        }
    }

    /// The search bar over the pane's top right corner.
    pub(super) fn render_search(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bar = self.search.as_ref()?;
        let found = bar.current.is_some() || bar.compiled.is_none();
        let theme = cx.theme();
        let toggle = |id: &'static str, label: &'static str, on: bool, regex: bool| {
            Button::new(id)
                .label(label)
                .ghost()
                .xsmall()
                .selected(on)
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.toggle_search_option(regex, cx);
                }))
        };
        let step = |id: &'static str, label: &'static str, up: bool| {
            Button::new(id)
                .label(label)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |pane, _, _, cx| pane.search_step(up, cx)))
        };
        Some(
            div()
                .key_context(SEARCH_CONTEXT)
                .on_action(cx.listener(|pane, _: &CloseSearch, window, cx| {
                    pane.close_search(window, cx);
                }))
                .absolute()
                .top(px(6.))
                .right(px(6.))
                .w(px(340.))
                .p(px(4.))
                .flex()
                .items_center()
                .gap(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(if found { theme.border } else { theme.danger })
                .bg(theme.popover)
                .shadow_md()
                .child(div().flex_1().child(Input::new(&bar.input).xsmall()))
                .child(toggle("search-case", "Aa", bar.case_sensitive, false))
                .child(toggle("search-regex", ".*", bar.regex, true))
                .child(step("search-up", "↑", true))
                .child(step("search-down", "↓", false)),
        )
    }
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
