//! The palette's matcher (spec 0004). A query matches a title when its
//! characters appear in it in order, ignoring case and spaces. Matches at
//! the start of a word and runs of consecutive characters rank higher, so
//! `splr` ranks "Split right" first. Of all the ways the query's characters
//! can line up with a title, the best one counts: `tab` finds the "tab" of
//! "Next tab", not its first `t`. The file browser's filter and path
//! suggestions use it too (spec 0013), over up to 50,000 names a
//! keystroke, so it costs time linear in the text per query character.

/// The step from one matched character to the next: a run scores, and a
/// gap costs its length, up to [`MAX_GAP`].
const RUN: i32 = 4;
const MAX_GAP: usize = 4;

/// A match: its score, and the indices of the characters it matched.
pub type Match = (i32, Vec<usize>);

/// How well `query` matches `text`, higher is better, or `None` if it
/// doesn't match. An empty query matches everything equally.
#[cfg(test)]
pub fn score(query: &str, text: &str) -> Option<i32> {
    matches(query, text).map(|(score, _)| score)
}

/// [`score`], and the indices of the characters of `text` the best
/// alignment matched, in order, for highlighting.
pub fn matches(query: &str, text: &str) -> Option<Match> {
    Matcher::new(query).matches(text)
}

/// One query matched against many texts: its buffers are reused, so the
/// file browser's filter allocates nothing for a name that doesn't match
/// and little for one that does.
pub struct Matcher {
    query: Vec<char>,
    text: Vec<char>,
    lower: Vec<char>,
    /// best[ix]: the best score of the query so far, its last character
    /// matched at text[ix].
    best: Vec<Option<i32>>,
    next: Vec<Option<i32>>,
    /// from[q * len + ix]: where character q of the query was when
    /// character q + 1 matched at ix.
    from: Vec<usize>,
}

impl Matcher {
    pub fn new(query: &str) -> Self {
        Self {
            query: query
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(lowercase)
                .collect(),
            text: Vec::new(),
            lower: Vec::new(),
            best: Vec::new(),
            next: Vec::new(),
            from: Vec::new(),
        }
    }

    /// [`matches`] for this matcher's query.
    pub fn matches(&mut self, text: &str) -> Option<Match> {
        let Some(&first) = self.query.first() else {
            return Some((0, Vec::new()));
        };
        // Most names don't hold the query in order: say so before the table.
        let mut wanted = self.query.iter().peekable();
        for c in text.chars().map(lowercase) {
            if wanted.peek() == Some(&&c) {
                wanted.next();
            }
        }
        if wanted.peek().is_some() {
            return None;
        }
        self.text.clear();
        self.text.extend(text.chars());
        self.lower.clear();
        self.lower.extend(self.text.iter().copied().map(lowercase));
        let (text, lower) = (&self.text, &self.lower);
        let len = text.len();
        let at = |ix: usize| {
            let word_start = ix == 0 || !text[ix - 1].is_alphanumeric();
            1 + if word_start { 8 } else { 0 }
        };
        self.best.clear();
        self.best
            .extend((0..len).map(|ix| (lower[ix] == first).then(|| at(ix))));
        self.from.clear();
        self.from.resize(len * (self.query.len() - 1), 0);
        for (q, &wanted) in self.query[1..].iter().enumerate() {
            self.next.clear();
            self.next.resize(len, None);
            let came = &mut self.from[q * len..(q + 1) * len];
            let best = &self.best;
            // The best score at least MAX_GAP characters back, and where.
            let mut far: Option<(i32, usize)> = None;
            for ix in 0..len {
                if ix > MAX_GAP
                    && let Some(score) = best[ix - MAX_GAP - 1]
                    && far.is_none_or(|(best, _)| score >= best)
                {
                    far = Some((score, ix - MAX_GAP - 1));
                }
                if lower[ix] != wanted {
                    continue;
                }
                let near = (1..=MAX_GAP.min(ix)).filter_map(|back| {
                    let step = if back == 1 { RUN } else { -(back as i32 - 1) };
                    Some((best[ix - back]? + step, ix - back))
                });
                let far = far.map(|(score, previous)| (score - MAX_GAP as i32, previous));
                if let Some((score, previous)) = near.chain(far).max_by_key(|&(score, _)| score) {
                    self.next[ix] = Some(score + at(ix));
                    came[ix] = previous;
                }
            }
            std::mem::swap(&mut self.best, &mut self.next);
        }
        let (mut ix, score) = self
            .best
            .iter()
            .enumerate()
            .filter_map(|(ix, score)| Some((ix, (*score)?)))
            .max_by_key(|&(_, score)| score)?;
        let mut hits = vec![0; self.query.len()];
        for q in (0..self.query.len()).rev() {
            hits[q] = ix;
            if q > 0 {
                ix = self.from[(q - 1) * len + ix];
            }
        }
        Some((score, hits))
    }
}

/// The indices of `titles` that match `query`, best first. Equal scores
/// keep their order.
pub fn rank<'a>(query: &str, titles: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut matcher = Matcher::new(query);
    let mut matches: Vec<(usize, i32)> = titles
        .into_iter()
        .enumerate()
        .filter_map(|(ix, title)| Some((ix, matcher.matches(title)?.0)))
        .collect();
    matches.sort_by_key(|&(ix, score)| (std::cmp::Reverse(score), ix));
    matches.into_iter().map(|(ix, _)| ix).collect()
}

fn lowercase(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::COMMANDS;

    /// The command titles, in the order the palette lists them.
    fn palette_titles() -> Vec<&'static str> {
        COMMANDS
            .iter()
            .filter_map(|command| command.title)
            .collect()
    }

    /// AC1 (spec 0004).
    #[test]
    fn typing_splr_ranks_split_right_first() {
        let titles = palette_titles();
        let ranked = rank("splr", titles.iter().copied());
        assert_eq!(ranked.first().map(|&ix| titles[ix]), Some("Split right"));
    }

    #[test]
    fn a_match_at_the_start_of_words_beats_one_inside_a_word() {
        // `n` starts "New" but is inside "Rename".
        assert_eq!(rank("nt", ["Rename tab", "New tab"]), [1, 0]);
    }

    /// Of all the ways a query lines up with a title, the best counts: the
    /// `t` of "Next" doesn't hide the whole word "tab" after it.
    #[test]
    fn typing_tab_scores_next_tab_no_lower_than_go_to_tab() {
        let next = score("tab", "Next tab").expect("`tab` matches Next tab");
        let go_to = score("tab", "Go to tab 2").expect("`tab` matches Go to tab 2");
        assert!(next >= go_to, "Next tab {next}, Go to tab 2 {go_to}");
    }

    #[test]
    fn titles_without_the_query_in_order_are_left_out() {
        // "Split down" has a `d` and an `s`, but not in that order.
        let titles = ["Split down", "Rename tab", "Decrease font size"];
        assert_eq!(rank("ds", titles), [2]);
    }

    #[test]
    fn the_query_ignores_case_and_spaces() {
        for query in ["SPLR", "split right", "Sp Lr"] {
            assert_eq!(rank(query, ["Split down", "Split right"]), [1], "{query}");
        }
    }

    /// The matched characters are the best alignment's, for highlighting.
    #[test]
    fn matches_name_the_characters_of_the_best_alignment() {
        assert_eq!(
            matches("rdme", "README.md"),
            Some((score("rdme", "README.md").unwrap(), vec![0, 3, 4, 5]))
        );
        assert_eq!(
            matches("tab", "Next tab").map(|(_, hits)| hits),
            Some(vec![5, 6, 7])
        );
        assert_eq!(matches("", "x"), Some((0, vec![])));
        assert_eq!(matches("xz", "zx"), None);
    }

    /// The linear table scores exactly as trying every earlier character
    /// does, the palette's first matcher.
    #[test]
    fn the_linear_matcher_scores_as_the_exhaustive_one() {
        fn exhaustive(query: &str, text: &str) -> Option<i32> {
            let query: Vec<char> = query.chars().map(lowercase).collect();
            let text: Vec<char> = text.chars().collect();
            let lower: Vec<char> = text.iter().copied().map(lowercase).collect();
            let at = |ix: usize| {
                1 + if ix == 0 || !text[ix - 1].is_alphanumeric() {
                    8
                } else {
                    0
                }
            };
            let mut best: Vec<Option<i32>> = (0..text.len())
                .map(|ix| (lower[ix] == query[0]).then(|| at(ix)))
                .collect();
            for &wanted in &query[1..] {
                best = (0..text.len())
                    .map(|ix| {
                        if lower[ix] != wanted {
                            return None;
                        }
                        let before = (0..ix).filter_map(|previous| {
                            let step = match ix - previous - 1 {
                                0 => 4,
                                gap => -(gap.min(4) as i32),
                            };
                            Some(best[previous]? + step)
                        });
                        Some(before.max()? + at(ix))
                    })
                    .collect();
            }
            best.into_iter().flatten().max()
        }
        let texts = [
            "README.md",
            "a_very_long_file_name_with_many_parts.txt",
            "aaaaaaaaaaaaab",
            "Split right",
            "x-y-z-x-y-z-x-y-z",
            "Grüße aus Köln",
        ];
        for text in texts {
            for query in [
                "a", "ab", "rdme", "xyz", "xz", "sr", "fnp", "aab", "gk", "e",
            ] {
                assert_eq!(
                    score(query, text),
                    exhaustive(query, text),
                    "{query} in {text}"
                );
            }
        }
    }

    /// An empty query shows the whole palette as it is ordered.
    #[test]
    fn an_empty_query_keeps_every_title_in_palette_order() {
        let titles = palette_titles();
        let all: Vec<usize> = (0..titles.len()).collect();
        assert_eq!(rank("", titles), all);
    }
}
