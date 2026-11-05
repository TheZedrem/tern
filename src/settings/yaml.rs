//! In-place edits of `config.yaml` for the settings screen (spec 0009): an
//! entry is found by its path of keys, block style, and only its own lines
//! change; every other byte of the file stays, comments included. YAML an
//! edit can't change safely, like a flow mapping `{ … }` where a block is
//! needed, an anchor or a multi-line scalar, is left alone: `None`.

/// The lines of `text`, its line break, and whether it ends with one.
fn split(text: &str) -> (Vec<&str>, &'static str, bool) {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing = text.is_empty() || text.ends_with('\n');
    let lines = text
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    (lines, newline, trailing)
}

fn join(lines: &[String], newline: &str, trailing: bool) -> String {
    let mut text = lines.join(newline);
    if trailing && !lines.is_empty() {
        text.push_str(newline);
    }
    text
}

/// A blank or comment-only line, which neither starts nor ends an entry.
fn skip(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// A mapping line's key and what follows its colon; `None` for anything
/// else (a sequence item, a continuation, a comment).
fn key_of(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    let (key, after) = if let Some(quoted) = trimmed.strip_prefix('"') {
        let mut key = String::new();
        let mut chars = quoted.char_indices();
        loop {
            match chars.next()? {
                (_, '\\') => key.push(chars.next()?.1),
                (end, '"') => break (key, &quoted[end + 1..]),
                (_, c) => key.push(c),
            }
        }
    } else if let Some(quoted) = trimmed.strip_prefix('\'') {
        let end = quoted.find('\'')?;
        (quoted[..end].to_owned(), &quoted[end + 1..])
    } else {
        if trimmed.starts_with(['-', '{', '[', '&', '*', '!', '?']) {
            return None;
        }
        let end = trimmed.match_indices(':').map(|(at, _)| at).find(|&at| {
            trimmed[at + 1..].is_empty() || trimmed[at + 1..].starts_with([' ', '\t'])
        })?;
        (trimmed[..end].trim_end().to_owned(), &trimmed[end..])
    };
    let rest = after.trim_start().strip_prefix(':')?;
    Some((key, rest))
}

/// Where the comment after a value starts in `rest`, outside quotes.
fn comment_at(rest: &str) -> Option<usize> {
    let mut quote = None;
    let mut last = ' ';
    for (at, c) in rest.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '#') if last.is_whitespace() => return Some(at),
            _ => {}
        }
        last = c;
    }
    None
}

/// The value after a key's colon, without its comment.
fn value_of(rest: &str) -> &str {
    comment_at(rest).map_or(rest, |at| &rest[..at]).trim()
}

/// The index after entry `start`'s last line: its key line and every
/// deeper line below it.
fn span(lines: &[&str], start: usize) -> usize {
    let depth = indent(lines[start]);
    let mut end = start + 1;
    for (at, line) in lines.iter().enumerate().skip(start + 1) {
        if skip(line) {
            continue;
        }
        if indent(line) <= depth {
            break;
        }
        end = at + 1;
    }
    end
}

enum Spot {
    /// The entry's lines and indent.
    Found {
        start: usize,
        end: usize,
        indent: usize,
    },
    /// Path keys from `depth` on are missing; they go before line `at`, the
    /// first at `indent`.
    Missing {
        at: usize,
        indent: usize,
        depth: usize,
    },
}

/// Where `path` is. `None` if an entry on the way holds a value of its own
/// where a block is needed.
fn locate(lines: &[&str], path: &[&str]) -> Option<Spot> {
    let (mut start, mut end, mut parent) = (0, lines.len(), None::<usize>);
    for (depth, key) in path.iter().enumerate() {
        let child = match parent {
            None => 0,
            Some(parent) => (start..end)
                .find(|&at| !skip(lines[at]))
                .map_or(parent + 2, |at| indent(lines[at])),
        };
        let found = (start..end).find(|&at| {
            !skip(lines[at])
                && indent(lines[at]) == child
                && key_of(lines[at]).is_some_and(|(found, _)| found == *key)
        });
        let Some(at) = found else {
            return Some(Spot::Missing {
                at: end,
                indent: child,
                depth,
            });
        };
        let entry_end = span(lines, at);
        if depth + 1 == path.len() {
            return Some(Spot::Found {
                start: at,
                end: entry_end,
                indent: child,
            });
        }
        let (_, rest) = key_of(lines[at])?;
        if !value_of(rest).is_empty() {
            return None;
        }
        (start, end, parent) = (at + 1, entry_end, Some(child));
    }
    None
}

/// `text` with the entry at `path` replaced by `entry`, or added in block
/// style where it's missing. `entry`'s lines are relative: its first is
/// `key: value` or `key:`, and deeper lines carry their own extra indent.
pub fn upsert(text: &str, path: &[&str], entry: &[String]) -> Option<String> {
    let (lines, newline, trailing) = split(text);
    let at_indent = |indent: usize, line: &String| format!("{}{line}", " ".repeat(indent));
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    match locate(&lines, path)? {
        Spot::Found { start, end, indent } => {
            out.splice(start..end, entry.iter().map(|line| at_indent(indent, line)));
        }
        Spot::Missing { at, indent, depth } => {
            let mut added = Vec::new();
            for (step, key) in path[depth..path.len() - 1].iter().enumerate() {
                added.push(format!(
                    "{}{}:",
                    " ".repeat(indent + 2 * step),
                    key_text(key)
                ));
            }
            let deepest = indent + 2 * (path.len() - 1 - depth);
            added.extend(entry.iter().map(|line| at_indent(deepest, line)));
            out.splice(at..at, added);
        }
    }
    Some(join(&out, newline, trailing))
}

/// `text` with the scalar at `path` set to `value` (already YAML), its
/// comment kept. `None` where `path` holds a block or a value this can't
/// rewrite on one line.
pub fn set_scalar(text: &str, path: &[&str], value: &str) -> Option<String> {
    let (lines, newline, trailing) = split(text);
    let key = path.last()?;
    match locate(&lines, path)? {
        Spot::Found { start, end, .. } => {
            if end != start + 1 {
                return None;
            }
            let line = lines[start];
            let (_, rest) = key_of(line)?;
            if value_of(rest).starts_with(['{', '[', '|', '>', '&', '*', '!']) {
                return None;
            }
            let head = &line[..line.len() - rest.len()];
            // The comment keeps the gap it had after the value.
            let comment = comment_at(rest).map_or(String::new(), |at| {
                let before = &rest[..at];
                let gap = before.len() - before.trim_end().len();
                format!("{}{}", " ".repeat(gap.max(1)), &rest[at..])
            });
            let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
            out[start] = format!("{head} {value}{comment}");
            Some(join(&out, newline, trailing))
        }
        Spot::Missing { .. } => upsert(text, path, &[format!("{}: {value}", key_text(key))]),
    }
}

/// `text` without the entry at `path`; unchanged if it isn't there.
pub fn remove(text: &str, path: &[&str]) -> Option<String> {
    let (lines, newline, trailing) = split(text);
    match locate(&lines, path)? {
        Spot::Found { start, end, .. } => {
            let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
            out.drain(start..end);
            Some(join(&out, newline, trailing))
        }
        Spot::Missing { .. } => Some(text.to_owned()),
    }
}

/// `text` without the entries under `parent` whose key and value `drop`
/// picks, values unquoted.
pub fn remove_where(text: &str, parent: &str, drop: impl Fn(&str, &str) -> bool) -> Option<String> {
    let (lines, newline, trailing) = split(text);
    let Spot::Found { start, end, .. } = locate(&lines, &[parent])? else {
        return Some(text.to_owned());
    };
    let (_, rest) = key_of(lines[start])?;
    if !value_of(rest).is_empty() {
        return None;
    }
    let child = (start + 1..end)
        .find(|&at| !skip(lines[at]))
        .map(|at| indent(lines[at]));
    let mut gone = Vec::new();
    let mut at = start + 1;
    while at < end {
        if !skip(lines[at])
            && Some(indent(lines[at])) == child
            && let Some((key, rest)) = key_of(lines[at])
        {
            let entry_end = span(&lines, at);
            if drop(&key, unquote(value_of(rest))) {
                gone.push(at..entry_end);
            }
            at = entry_end;
            continue;
        }
        at += 1;
    }
    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    for range in gone.into_iter().rev() {
        out.drain(range);
    }
    Some(join(&out, newline, trailing))
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

/// A key as YAML: plain where it reads back as itself, quoted otherwise.
pub fn key_text(key: &str) -> String {
    scalar(key)
}

/// `text` as a YAML string: plain if it reads back as the same string,
/// double-quoted otherwise.
pub fn scalar(text: &str) -> String {
    let plain = text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '~')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '~'))
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n" | "~"
        );
    if plain {
        text.to_owned()
    } else {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TEMPLATE, parse};

    /// The lines of `before` and `after` that differ, by line number; both
    /// must have as many lines.
    fn differing(before: &str, after: &str) -> Vec<(String, String)> {
        assert_eq!(before.lines().count(), after.lines().count());
        before
            .lines()
            .zip(after.lines())
            .filter(|(old, new)| old != new)
            .map(|(old, new)| (old.to_owned(), new.to_owned()))
            .collect()
    }

    /// Spec 0009, AC 1: a font size change touches only the `size:` line of
    /// the real template; every other byte, comments included, stays.
    #[test]
    fn setting_font_size_in_the_template_changes_only_its_line() {
        let edited = set_scalar(TEMPLATE, &["font", "size"], "14").unwrap();
        assert_eq!(
            differing(TEMPLATE, &edited),
            [("  size: 13".to_owned(), "  size: 14".to_owned())]
        );
        assert!(edited.ends_with('\n'));
    }

    /// Spec 0009, in-place edits: a value's trailing comment is kept.
    #[test]
    fn setting_a_commented_value_keeps_its_comment() {
        let edited = set_scalar(TEMPLATE, &["font", "line_height"], "1.5").unwrap();
        assert_eq!(
            differing(TEMPLATE, &edited),
            [(
                "  line_height: 1.2  # a multiple of the font size".to_owned(),
                "  line_height: 1.5  # a multiple of the font size".to_owned()
            )]
        );
    }

    /// Spec 0009, in-place edits: a missing key is added in block style
    /// under its existing parent.
    #[test]
    fn a_missing_key_is_added_under_its_existing_block() {
        let text = "font:\n  size: 13\nscrollback: 100\n";
        let edited = set_scalar(text, &["font", "line_height"], "1.5").unwrap();
        assert_eq!(
            edited,
            "font:\n  size: 13\n  line_height: 1.5\nscrollback: 100\n"
        );
        assert_eq!(parse(&edited).config.font.line_height, 1.5);
    }

    /// Spec 0009, in-place edits: a missing key whose parent is missing too
    /// is added with its parent block.
    #[test]
    fn a_missing_key_is_added_with_its_missing_parent() {
        let text = "scrollback: 100 # lines\n";
        let edited = set_scalar(text, &["cursor", "blink"], "false").unwrap();
        assert!(edited.starts_with(text));
        let loaded = parse(&edited);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert!(!loaded.config.cursor.blink);
        assert_eq!(loaded.config.scrollback, 100);
    }

    /// Spec 0009, in-place edits and AC 6: what can't be rewritten on its
    /// own line is refused: a key in flow style, or a block where a scalar
    /// is asked.
    #[test]
    fn what_cant_be_rewritten_on_its_line_is_refused() {
        assert_eq!(
            set_scalar("font: { size: 13 }\n", &["font", "size"], "14"),
            None
        );
        assert_eq!(
            set_scalar("font: { size: 13 }\n", &["font", "family"], "Lilex"),
            None
        );
        assert_eq!(set_scalar("font:\n  size: 13\n", &["font"], "big"), None);
    }

    /// Spec 0009, in-place edits: every other byte stays, so line breaks
    /// stay as they were: CRLF stays CRLF, and a file without a final
    /// newline doesn't gain one, for a changed line and an added one.
    #[test]
    fn line_endings_stay_as_they_were() {
        let crlf = "font:\r\n  size: 13\r\nscrollback: 100\r\n";
        assert_eq!(
            set_scalar(crlf, &["font", "size"], "14").unwrap(),
            "font:\r\n  size: 14\r\nscrollback: 100\r\n"
        );
        assert_eq!(
            set_scalar(crlf, &["copy_on_select"], "true").unwrap(),
            "font:\r\n  size: 13\r\nscrollback: 100\r\ncopy_on_select: true\r\n"
        );
        assert_eq!(
            set_scalar("scrollback: 100", &["scrollback"], "200").unwrap(),
            "scrollback: 200"
        );
        assert_eq!(
            set_scalar("scrollback: 100", &["copy_on_select"], "true").unwrap(),
            "scrollback: 100\ncopy_on_select: true"
        );
    }

    /// Spec 0009, flow 3: resetting what isn't there leaves the file as is.
    #[test]
    fn removing_a_missing_path_leaves_the_text_unchanged() {
        let text = "# keys\nfont:\n  size: 13 # pt\n";
        assert_eq!(
            remove(text, &["keybindings", "ctrl+shift+k"]).unwrap(),
            text
        );
        assert_eq!(remove(text, &["font", "family"]).unwrap(), text);
    }

    /// Spec 0009, flow 3: only the matching entries under the parent go,
    /// whether their value is quoted or not.
    #[test]
    fn remove_where_drops_only_matching_entries_under_its_parent() {
        let text = "keybindings:\n  \"ctrl+shift+d\": pane.splitRight\n  ctrl+shift+n: \"tab.new\"\n  alt+shift+x: 'pane.splitRight'\nother: pane.splitRight\n";
        let edited = remove_where(text, "keybindings", |_, command| {
            command == "pane.splitRight"
        });
        assert_eq!(
            edited.unwrap(),
            "keybindings:\n  ctrl+shift+n: \"tab.new\"\nother: pane.splitRight\n"
        );
    }

    /// Spec 0009, flow 2: a string is written so YAML reads it back as that
    /// same string, and plain names stay plain.
    #[test]
    fn scalar_quotes_what_yaml_would_read_as_something_else() {
        for text in [
            "yes",
            "No",
            "null",
            "~",
            "13",
            "1.5",
            "a: b",
            "a #b",
            "Fira Code",
            "say \"hi\"",
        ] {
            let written = scalar(text);
            assert!(written.starts_with('"'), "{text} written as {written}");
            let loaded = parse(&format!("font:\n  family: {written}\n"));
            assert_eq!(
                loaded.config.font.family.as_deref(),
                Some(text),
                "{written}"
            );
        }
        for text in ["Lilex", "nvim", "~/projects", "web-1.example.com"] {
            assert_eq!(scalar(text), text);
        }
    }
}
