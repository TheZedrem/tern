//! The palette's matcher (spec 0004). A query matches a title when its
//! characters appear in it in order, ignoring case and spaces. Matches at
//! the start of a word and runs of consecutive characters rank higher, so
//! `splr` ranks "Split right" first. Of all the ways the query's characters
//! can line up with a title, the best one counts: `tab` finds the "tab" of
//! "Next tab", not its first `t`.

/// How well `query` matches `text`, higher is better, or `None` if it
/// doesn't match. An empty query matches everything equally.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(lowercase)
        .collect();
    let Some((&first, rest)) = query.split_first() else {
        return Some(0);
    };
    let text: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.iter().copied().map(lowercase).collect();
    let at = |ix: usize| {
        let word_start = ix == 0 || !text[ix - 1].is_alphanumeric();
        1 + if word_start { 8 } else { 0 }
    };
    // best[ix]: the best score of the query so far, its last character
    // matched at text[ix].
    let mut best: Vec<Option<i32>> = (0..text.len())
        .map(|ix| (lower[ix] == first).then(|| at(ix)))
        .collect();
    for &wanted in rest {
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

/// The indices of `titles` that match `query`, best first. Equal scores
/// keep their order.
pub fn rank<'a>(query: &str, titles: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut matches: Vec<(usize, i32)> = titles
        .into_iter()
        .enumerate()
        .filter_map(|(ix, title)| score(query, title).map(|score| (ix, score)))
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

    /// An empty query shows the whole palette as it is ordered.
    #[test]
    fn an_empty_query_keeps_every_title_in_palette_order() {
        let titles = palette_titles();
        let all: Vec<usize> = (0..titles.len()).collect();
        assert_eq!(rank("", titles), all);
    }
}
