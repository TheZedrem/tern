//! What a pane does when its program exits (spec 0002, flow 8; spec
//! 0006, flow 5), and the text rules for what a program or ssh says:
//! titles (spec 0003) and prompts (spec 0006, flow 4).

use super::{PaneEvent, State, TerminalPane};
use crate::config::CloseOnExit;
use crate::ssh;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use gpui::Context;

/// Longest tab title taken from a program (spec 0003).
const MAX_TITLE_CHARS: usize = 200;
/// The most of a prompt a dialog shows (TM-askpass-08).
const MAX_PROMPT_BYTES: usize = 4096;

/// The notice written into the grid when an SSH connection drops.
const RECONNECT_NOTICE: &str = "\r\n\x1b[2m[connection closed: press Enter to reconnect]\x1b[0m";

/// A program's title as a tab shows it: control characters dropped, runs
/// of whitespace collapsed, at most 200 characters (spec 0003).
pub fn clean_title(title: &str) -> String {
    title
        .split(|c: char| c.is_whitespace() || c.is_control() || is_invisible_format(c))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

/// ssh's prompt as a dialog shows it (spec 0006, flow 4): the title rules,
/// but with line breaks kept and tabs as spaces, capped at 4 KiB.
/// Host-key questions span several lines.
pub fn clean_prompt(prompt: &str) -> String {
    let mut cleaned = String::new();
    for c in prompt.chars() {
        let c = match c {
            '\n' => '\n',
            '\t' => ' ',
            c if c.is_control() || is_invisible_format(c) => continue,
            c => c,
        };
        if cleaned.len() + c.len_utf8() > MAX_PROMPT_BYTES {
            break;
        }
        cleaned.push(c);
    }
    cleaned
}

/// Invisible characters that could hide text or reorder what a tab shows:
/// zero-width space, direction marks and overrides, invisible operators,
/// byte-order mark, tag characters. They are treated like controls. The
/// zero-width joiner and non-joiner stay: emoji sequences and several
/// scripts need them.
pub(super) fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// Whether a pane closes after its program exited with `code` (`None`:
/// killed by a signal) under `policy` (spec 0002, flow 8).
pub fn closes_on_exit(policy: CloseOnExit, code: Option<i32>) -> bool {
    match policy {
        CloseOnExit::Always => true,
        CloseOnExit::Clean => code == Some(0),
        CloseOnExit::Never => false,
    }
}

/// What a pane does once its program exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterExit {
    Close,
    /// Shows the exit code and stays.
    Stay,
    /// Says the connection closed and waits for Enter to reconnect.
    Reconnect,
}

/// What a pane does after its program exited with `code`. A dropped SSH
/// connection (exit 255) stays open whatever `policy` says (spec 0006,
/// flow 5); everything else follows `policy`.
pub fn after_exit(reconnects: bool, policy: CloseOnExit, code: Option<i32>) -> AfterExit {
    if reconnects && code == Some(ssh::CONNECTION_LOST) {
        AfterExit::Reconnect
    } else if closes_on_exit(policy, code) {
        AfterExit::Close
    } else {
        AfterExit::Stay
    }
}

/// The notice written into the grid when the program exits.
pub fn exit_notice(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("\r\n\x1b[2m[process exited with code {code}]\x1b[0m"),
        None => "\r\n\x1b[2m[process was killed by a signal]\x1b[0m".to_owned(),
    }
}

impl TerminalPane {
    pub(super) fn child_exited(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        self.exited = true;
        // The connection's token dies with its ssh (TM-askpass-03).
        self.end_prompts(cx);
        let policy = match self.profile.stays {
            true => CloseOnExit::Never,
            false => self.settings.close_on_exit,
        };
        let after = after_exit(self.profile.ssh, policy, code);
        self.reconnect = after == AfterExit::Reconnect;
        let notice = if self.reconnect {
            RECONNECT_NOTICE.to_owned()
        } else {
            exit_notice(code)
        };
        if let State::Running(running) = &self.state {
            let mut parser = Processor::<StdSyncHandler>::new();
            parser.advance(&mut *running.term.lock(), notice.as_bytes());
        }
        cx.notify();
        if !self.bench && after == AfterExit::Close {
            cx.emit(PaneEvent::Exited);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::terminal::tests::{screen, term_with};

    #[test]
    fn close_on_exit_policies() {
        // Exit codes 0 and 3, then killed by a signal.
        let cases = [
            (CloseOnExit::Always, [true, true, true]),
            (CloseOnExit::Clean, [true, false, false]),
            (CloseOnExit::Never, [false, false, false]),
        ];
        for (policy, expected) in cases {
            for (code, closes) in [Some(0), Some(3), None].into_iter().zip(expected) {
                assert_eq!(closes_on_exit(policy, code), closes, "{policy:?} {code:?}");
            }
        }
    }

    #[test]
    fn default_config_closes_on_exit_0_and_stays_open_on_exit_3() {
        let policy = Config::default().close_on_exit;
        assert!(closes_on_exit(policy, Some(0)));
        assert!(!closes_on_exit(policy, Some(3)));
    }

    #[test]
    fn exit_notice_shows_the_code_on_the_line_after_the_output() {
        let mut bytes = b"hi".to_vec();
        bytes.extend_from_slice(exit_notice(Some(3)).as_bytes());
        let term = term_with(40, 3, &bytes);
        assert_eq!(screen(&term)[..2], ["hi", "[process exited with code 3]"]);
    }

    /// Spec 0006, flow 5: ssh exits 255 when the connection failed or
    /// dropped.
    #[test]
    fn an_ssh_exit_255_reconnects_whatever_close_on_exit_says() {
        for policy in [CloseOnExit::Always, CloseOnExit::Clean, CloseOnExit::Never] {
            let after = after_exit(true, policy, Some(255));
            assert_eq!(after, AfterExit::Reconnect, "{policy:?}");
        }
    }

    /// The remote shell's `exit`, a failed remote command, a signal: the
    /// pane does what `close_on_exit` says, as for any program.
    #[test]
    fn every_other_ssh_exit_follows_close_on_exit() {
        use AfterExit::{Close, Stay};
        // Exit codes 0 and 1, then killed by a signal.
        let cases = [
            (CloseOnExit::Always, [Close, Close, Close]),
            (CloseOnExit::Clean, [Close, Stay, Stay]),
            (CloseOnExit::Never, [Stay, Stay, Stay]),
        ];
        for (policy, expected) in cases {
            for (code, after) in [Some(0), Some(1), None].into_iter().zip(expected) {
                assert_eq!(after_exit(true, policy, code), after, "{policy:?} {code:?}");
            }
        }
    }

    /// Only ssh's 255 means a dropped connection; for a local program it's
    /// an exit code like any other.
    #[test]
    fn a_local_pane_follows_close_on_exit_even_on_exit_255() {
        use AfterExit::{Close, Stay};
        // Exit codes 0 and 255, then killed by a signal.
        let cases = [
            (CloseOnExit::Always, [Close, Close, Close]),
            (CloseOnExit::Clean, [Close, Stay, Stay]),
            (CloseOnExit::Never, [Stay, Stay, Stay]),
        ];
        for (policy, expected) in cases {
            for (code, after) in [Some(0), Some(255), None].into_iter().zip(expected) {
                assert_eq!(
                    after_exit(false, policy, code),
                    after,
                    "{policy:?} {code:?}"
                );
            }
        }
    }

    /// Spec 0006, flow 5.
    #[test]
    fn a_dropped_connection_shows_the_reconnect_hint_on_the_line_after_the_output() {
        let mut bytes = b"Connection to prod closed.".to_vec();
        bytes.extend_from_slice(RECONNECT_NOTICE.as_bytes());
        let term = term_with(60, 3, &bytes);
        let expected = [
            "Connection to prod closed.",
            "[connection closed: press Enter to reconnect]",
        ];
        assert_eq!(screen(&term)[..2], expected);
    }

    /// `raw` cleans to `words`, joined by single spaces or by nothing: a
    /// control character may be dropped or become a space, but never stays.
    fn assert_title(raw: &str, words: &[&str]) {
        let title = clean_title(raw);
        assert!(
            title == words.join(" ") || title == words.concat(),
            "{raw:?} became {title:?}"
        );
    }

    #[test]
    fn titles_lose_their_control_characters() {
        assert_title("bell\x07here", &["bell", "here"]);
        assert_title("esc\x1b[31mred", &["esc", "[31mred"]);
        assert_title("one\ntwo\r\nthree", &["one", "two", "three"]);
        assert_title("tab\there", &["tab", "here"]);
        // NUL, DEL and the C1 control CSI.
        assert_title("nul\0del\x7fcsi\u{9b}end", &["nul", "del", "csi", "end"]);
    }

    #[test]
    fn title_whitespace_runs_collapse_to_one_space() {
        assert_eq!(
            clean_title("cargo   build \t\n  --release"),
            "cargo build --release"
        );
        assert_eq!(clean_title("vim ~/notes.md"), "vim ~/notes.md");
    }

    /// AC9 (spec 0003): a 5 000-character title is capped at 200
    /// characters, on one line.
    #[test]
    fn a_5000_character_title_is_capped_at_200_characters_on_one_line() {
        assert_eq!(clean_title(&"x".repeat(5000)).chars().count(), 200);
        // Characters, not bytes: two- and four-byte characters stay whole.
        assert_eq!(clean_title(&"ä🦀".repeat(2500)), "ä🦀".repeat(100));
        let lines = clean_title(&"line\n".repeat(1000));
        assert_eq!(lines.chars().count(), 200);
        assert!(!lines.contains(['\n', '\r']), "{lines:?}");
        // The other Unicode line breaks: CR, VT, FF, NEL, LS and PS.
        for separator in ["\r", "\u{0b}", "\u{0c}", "\u{85}", "\u{2028}", "\u{2029}"] {
            assert_title(&format!("first{separator}second"), &["first", "second"]);
        }
    }

    /// The characters that hide or reorder text: Arabic letter mark,
    /// zero-width space, direction marks, embeddings, overrides and
    /// isolates, invisible operators, byte-order mark, and the first tag
    /// character, tag "A" and the closing cancel tag.
    fn invisible_characters() -> impl Iterator<Item = char> {
        ['\u{061C}', '\u{200B}', '\u{200E}', '\u{200F}', '\u{FEFF}']
            .into_iter()
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2060}'..='\u{2064}')
            .chain('\u{2066}'..='\u{2069}')
            .chain(['\u{E0000}', '\u{E0041}', '\u{E007F}'])
    }

    /// 👨‍💻 (man, zero-width joiner, laptop), and Persian "mikhaham" (I
    /// want), with a zero-width non-joiner after "mi".
    const JOINED: [&str; 2] = [
        "\u{1F468}\u{200D}\u{1F4BB}",
        "\u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0647}\u{0645}",
    ];

    /// A title can't reorder or hide text: a right-to-left override would
    /// show `invoice\u{202E}fdp.exe` as "invoiceexe.pdf", zero-width
    /// characters make different text look the same, and tag characters
    /// carry text nobody sees.
    #[test]
    fn titles_lose_characters_that_hide_or_reorder_text() {
        assert_title("invoice\u{202E}fdp.exe", &["invoice", "fdp.exe"]);
        assert_title("ad\u{200B}min\u{FEFF}", &["ad", "min"]);
        // "rm" spelled in tag characters.
        assert_title("build\u{E0072}\u{E006D}", &["build"]);
        for c in invisible_characters() {
            assert_title(&format!("before{c}after"), &["before", "after"]);
        }
    }

    /// The zero-width joiner and non-joiner are part of the text: emoji
    /// sequences and scripts like Persian need them.
    #[test]
    fn titles_keep_the_joiners_emoji_and_scripts_need() {
        let [technologist, persian] = JOINED;
        let both = format!("{technologist} {persian}");
        for title in [technologist, persian, both.as_str()] {
            assert_eq!(clean_title(title), title);
        }
    }

    /// Spec 0006, flow 4: ssh's host-key question spans several lines, and
    /// its dialog shows them as lines. A tab is a space.
    #[test]
    fn prompts_keep_their_line_breaks_and_tabs_become_spaces() {
        let question = "The authenticity of host 'prod (192.0.2.10)' can't be established.\n\
                        ED25519 key fingerprint is: SHA256:ufr9ck+x5zXLjNm4HvDvXiRs6L/6qjQoTXCJCukwfy8\n\
                        This key is not known by any other names.\n\
                        Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(clean_prompt(question), question);
        assert_eq!(clean_prompt("first\r\nsecond"), "first\nsecond");
        assert_eq!(clean_prompt("Verification\tcode: "), "Verification code: ");
    }

    /// TM-askpass-08: a server's prompt text can't steer its dialog: the
    /// control characters are dropped, line breaks aside.
    #[test]
    fn prompts_lose_their_control_characters() {
        let cases = [
            ("bell\x07here", "bellhere"),
            ("esc\x1b[31mred", "esc[31mred"),
            ("back\x08space", "backspace"),
            // NUL, DEL and the C1 control CSI.
            ("nul\0del\x7fcsi\u{9b}end", "nuldelcsiend"),
        ];
        for (raw, cleaned) in cases {
            assert_eq!(clean_prompt(raw), cleaned, "{raw:?}");
        }
    }

    /// TM-askpass-08, by the title rules of spec 0003: a prompt can't hide
    /// or reorder what it says, say to make its dialog name another host
    /// than the one asking. The joiners emoji and scripts need stay.
    #[test]
    fn prompts_lose_the_characters_that_hide_or_reorder_text() {
        for c in invisible_characters() {
            let prompt = format!("alice@prod{c}'s password: ");
            assert_eq!(
                clean_prompt(&prompt),
                "alice@prod's password: ",
                "{prompt:?}"
            );
        }
        for text in JOINED {
            assert_eq!(clean_prompt(text), text);
        }
    }

    /// TM-askpass-08: a dialog shows 4 KiB of a prompt at most, cut between
    /// two characters, never through one. What's dropped doesn't count.
    #[test]
    fn a_prompt_is_capped_at_4_kib_between_characters() {
        assert_eq!(clean_prompt(&"x".repeat(5000)), "x".repeat(4096));
        // 4 KiB are 1 024 four-byte crabs.
        assert_eq!(clean_prompt(&"🦀".repeat(2000)), "🦀".repeat(1024));
        // One byte ahead, the crab that would cross 4 KiB is left out whole.
        assert_eq!(
            clean_prompt(&format!("x{}", "🦀".repeat(2000))),
            format!("x{}", "🦀".repeat(1023))
        );
        assert_eq!(
            clean_prompt(&format!("{}ok", "\u{200B}".repeat(5000))),
            "ok"
        );
    }
}
