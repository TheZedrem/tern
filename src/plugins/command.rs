//! What a plugin wants typed, run or opened, as the user is shown it
//! before it happens (spec 0016, Untrusted text): on a command button and
//! on a confirmation bar. The text is shown exactly as it would
//! run, so text that could hide what it runs is refused instead of cleaned
//! (TM-plug-04, -05).

use super::manifest::Right;
use gpui::{
    App, FontWeight, HighlightStyle, IntoElement, ParentElement, SharedString, Styled, StyledText,
    div, px,
};
use gpui_component::ActiveTheme;
use serde::Deserialize;
use std::ops::Range;

/// The most text a command may carry, over all of its parts.
const MAX_BYTES: usize = 4 * 1024;

/// A command a panel button carries. `read_output` is never one: what it
/// would send isn't known before the click (TM-plug-23).
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Command {
    #[serde(rename = "fill_command")]
    Fill {
        text: String,
    },
    #[serde(rename = "run_command")]
    Run {
        text: String,
    },
    OpenTab {
        argv: Vec<String>,
    },
}

impl Command {
    /// Its name on the wire, for the plugin's log.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Fill { .. } => "fill_command",
            Command::Run { .. } => "run_command",
            Command::OpenTab { .. } => "open_tab",
        }
    }

    /// The right it needs.
    pub fn right(&self) -> Right {
        match self {
            Command::Fill { .. } | Command::Run { .. } => Right::TerminalWrite,
            Command::OpenTab { .. } => Right::OpenTab,
        }
    }

    /// What it would type, or each element of the argv it would run.
    pub fn parts(&self) -> &[String] {
        match self {
            Command::Fill { text } | Command::Run { text } => std::slice::from_ref(text),
            Command::OpenTab { argv } => argv,
        }
    }

    /// Why Tern won't show or run it, if it won't: a control or format
    /// character, more than 4 KiB, nothing to run, or on Windows a program
    /// that `cmd.exe` would parse again.
    pub fn refusal(&self) -> Option<&'static str> {
        let parts = self.parts();
        if parts.iter().map(String::len).sum::<usize>() > MAX_BYTES {
            return Some("a command over 4 KiB");
        }
        if parts.iter().any(|part| part.chars().any(hides)) {
            return Some("a command with a control or invisible character");
        }
        match self {
            Command::OpenTab { argv } => match argv.first() {
                None => Some("an `open_tab` without a program"),
                Some(program) if program.is_empty() => Some("an `open_tab` without a program"),
                Some(program) if cfg!(windows) && reparsed_on_windows(program) => {
                    Some("an `open_tab` of a .bat or .cmd file, or a program with a quote")
                }
                Some(_) => None,
            },
            Command::Fill { text } | Command::Run { text } if text.is_empty() => {
                Some("a command without text")
            }
            _ => None,
        }
    }
}

/// A program Windows would hand to `cmd.exe`, which parses its arguments
/// again (`&`, `|`, quotes), or whose name breaks the command line
/// (TM-plug-05).
fn reparsed_on_windows(program: &str) -> bool {
    let lower = program.to_lowercase();
    program.contains('"') || lower.ends_with(".bat") || lower.ends_with(".cmd")
}

/// A character that could hide what a command runs: a control character
/// (a newline or ESC turns a fill into a run) or a format character
/// (Unicode `Cf`: direction overrides, zero-width and tag characters).
fn hides(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        )
}

/// The byte ranges of `text` that hold non-ASCII characters, neighbours
/// joined: what is drawn highlighted, so a look-alike letter stands out.
/// A plain ASCII check, with no locale and no script table (OQ-035).
pub fn non_ascii(text: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (at, c) in text.char_indices().filter(|(_, c)| !c.is_ascii()) {
        let end = at + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == at => last.end = end,
            _ => ranges.push(at..end),
        }
    }
    ranges
}

/// The note above a command that holds non-ASCII characters.
pub const NON_ASCII_NOTE: &str = "Contains non-ASCII characters (highlighted)";

/// `command` as the user reads it before it runs: all of its text,
/// wrapped and never cut, in the terminal's `font`, every non-ASCII
/// character highlighted under a line saying so. An argv's elements each
/// get a box of their own, so where one ends is never a guess.
pub fn shown(command: &Command, font: SharedString, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (warning, muted, boxed) = (theme.warning, theme.muted_foreground, theme.secondary);
    let highlight = HighlightStyle {
        background_color: Some(warning),
        color: Some(theme.warning_foreground),
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let parts = command.parts();
    let marked = parts.iter().any(|part| !part.is_ascii());
    let separate = matches!(command, Command::OpenTab { .. });
    let lines = parts.iter().map(move |part| {
        let ranges = non_ascii(part).into_iter().map(|range| (range, highlight));
        let text = StyledText::new(part.clone()).with_highlights(ranges);
        let line = div().min_w_0().child(text);
        if separate {
            line.px(px(4.)).rounded(px(3.)).bg(boxed)
        } else {
            line
        }
    });
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .min_w_0()
        .text_xs()
        .children(marked.then(|| div().text_color(muted).child(NON_ASCII_NOTE)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .min_w_0()
                .font_family(font)
                .children(lines),
        )
}

/// Spec 0016 §Model (Untrusted text) and AC7; threat model 0005,
/// TM-plug-04 and -05: what a command may hold, and what is highlighted.
#[cfg(test)]
mod tests {
    use super::*;

    fn fill(text: &str) -> Command {
        Command::Fill { text: text.into() }
    }

    fn run(text: &str) -> Command {
        Command::Run { text: text.into() }
    }

    fn open(argv: &[&str]) -> Command {
        Command::OpenTab {
            argv: argv.iter().map(|part| (*part).to_owned()).collect(),
        }
    }

    /// Untrusted text, AC7, TM-plug-04: text "containing a control
    /// character (newline, ESC, …) or a format character (Unicode `Cf`:
    /// direction overrides, zero-width and tag characters) is refused",
    /// whether it would be typed, run, or is an element of an argv.
    #[test]
    fn a_command_with_a_control_or_format_character_is_refused() {
        let hiding = [
            ("a newline", "ls\nrm -rf ~"),
            ("a carriage return", "ls\rrm -rf ~"),
            ("ESC", "ls \u{1b}[2K"),
            ("NUL", "ls\u{0}"),
            ("a C1 control", "ls\u{9b}2K"),
            ("a right-to-left override", "ls \u{202E}fdp.exe"),
            ("a direction isolate", "ls \u{2066}x\u{2069}"),
            ("a zero-width space", "cu\u{200B}rl example.com"),
            ("a zero-width joiner", "cu\u{200D}rl example.com"),
            ("a word joiner", "cu\u{2060}rl example.com"),
            ("a byte-order mark", "\u{FEFF}curl example.com"),
            ("a soft hyphen", "cu\u{AD}rl example.com"),
            ("a tag character", "curl\u{E0041} example.com"),
        ];
        for (what, text) in hiding {
            assert!(fill(text).refusal().is_some(), "fill_command with {what}");
            assert!(run(text).refusal().is_some(), "run_command with {what}");
            let argv = open(&["docker", text]);
            assert!(argv.refusal().is_some(), "open_tab with {what}");
        }
    }

    /// Untrusted text: "and so is text over 4 KiB", over all of an argv;
    /// 4 KiB itself is still shown.
    #[test]
    fn a_command_over_4_kib_is_refused() {
        let most = "x".repeat(4096);
        let over = "x".repeat(4097);
        assert_eq!(fill(&most).refusal(), None);
        assert_eq!(run(&most).refusal(), None);
        assert!(fill(&over).refusal().is_some());
        assert!(run(&over).refusal().is_some());
        // Bytes, not characters: 2,049 two-byte letters are over 4 KiB.
        assert!(run(&"é".repeat(2049)).refusal().is_some());

        let half = "x".repeat(2048);
        assert_eq!(open(&[&half, &half]).refusal(), None);
        assert!(open(&[&half, &half, "x"]).refusal().is_some());
    }

    /// An `open_tab` needs a program to open a tab with.
    #[test]
    fn an_open_tab_without_a_program_is_refused() {
        assert!(open(&[]).refusal().is_some());
        assert!(open(&[""]).refusal().is_some());
        assert!(open(&["", "-it"]).refusal().is_some());
        assert_eq!(
            open(&["docker", "exec", "-it", "web", "sh"]).refusal(),
            None
        );
    }

    /// Untrusted text, TM-plug-05: "On Windows, an `open_tab` whose
    /// program contains `"` or ends in `.bat` or `.cmd` is refused"
    /// (ignoring case); only the program counts, and only on Windows.
    #[test]
    fn an_open_tab_that_cmd_exe_would_parse_again_is_refused_on_windows() {
        for program in ["build.bat", "BUILD.CMD", "Build.Bat", "my\"tool"] {
            let refused = open(&[program, "--all"]).refusal().is_some();
            assert_eq!(refused, cfg!(windows), "{program}");
        }
        assert_eq!(open(&["tool", "build.bat", "a\"b"]).refusal(), None);
    }

    /// Untrusted text, AC7, TM-plug-04 (owner, OQ-035): non-ASCII text is
    /// "a plain ASCII check: no locale, no script table, and nothing is
    /// refused for it".
    #[test]
    fn non_ascii_text_is_not_refused() {
        // The first letter is a Cyrillic `с`.
        assert_eq!(run("сurl example.com").refusal(), None);
        assert_eq!(fill("сurl example.com").refusal(), None);
        assert_eq!(fill("cat ~/Übersicht/größe.txt").refusal(), None);
        assert_eq!(open(&["cat", "日本語.txt", "📦"]).refusal(), None);
    }

    /// AC7: `сurl example.com` (Cyrillic `с`) is shown "with only the `с`
    /// highlighted"; `curl example.com` shows no highlight.
    #[test]
    fn exactly_the_look_alike_letter_is_highlighted() {
        let text = "сurl example.com";
        let ranges = non_ascii(text);
        assert_eq!(ranges.len(), 1, "{ranges:?}");
        assert_eq!(ranges[0], 0..'с'.len_utf8());
        assert_eq!(&text[ranges[0].clone()], "с");
        assert!(non_ascii("curl example.com").is_empty());
        assert!(non_ascii("").is_empty());
    }

    /// Untrusted text: "Every non-ASCII character in that text is drawn
    /// highlighted", and nothing else is: the ranges lie on character
    /// boundaries, in order, and hold every non-ASCII character and no
    /// ASCII one.
    #[test]
    fn every_non_ascii_character_and_no_other_is_highlighted() {
        let texts = [
            "echo naïve café",
            "cat 日本語.txt | grep é",
            "é",
            "aé",
            "éa",
            "tar -xf 📦.tar && ls ~/Übersicht",
            "ｃｕｒｌ example.com",
        ];
        for text in texts {
            let ranges = non_ascii(text);
            let mut highlighted = String::new();
            let mut plain = String::new();
            let mut at = 0;
            for range in &ranges {
                assert!(range.start >= at && range.start < range.end, "{text}");
                plain.push_str(&text[at..range.start]);
                highlighted.push_str(&text[range.clone()]);
                at = range.end;
            }
            plain.push_str(&text[at..]);
            let expected: String = text.chars().filter(|c| !c.is_ascii()).collect();
            assert_eq!(highlighted, expected, "{text}");
            assert!(plain.is_ascii(), "{text}: {plain:?} isn't highlighted");
        }
    }

    /// Command buttons: a button "needs the same right as the action":
    /// `terminal_write` to type or run, `open_tab` to open a tab.
    #[test]
    fn a_command_needs_the_right_of_its_action() {
        assert_eq!(fill("ls").right(), Right::TerminalWrite);
        assert_eq!(run("ls").right(), Right::TerminalWrite);
        assert_eq!(open(&["htop"]).right(), Right::OpenTab);
    }
}
