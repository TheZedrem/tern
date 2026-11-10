//! Keystrokes to the bytes a terminal program expects, following xterm's
//! conventions. `alacritty_terminal` has no key mapping of its own.
//!
//! Plain `Ctrl+<key>` and `Alt+<key>` always reach the shell. `Ctrl+Shift`
//! with a character key, `Ctrl+Tab` and Super/Cmd combinations send
//! nothing: they are reserved for app shortcuts (spec 0004), so an unbound
//! one never surprises the shell. Modified cursor and function keys still
//! send xterm's sequences.

use super::pty;
use super::{State, TerminalPane};
use alacritty_terminal::event::Notify;
use alacritty_terminal::grid::Scroll;
use gpui::Keystroke;
use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, px};

/// Terminal modes that change what a key sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM: arrows, Home and End send SS3 instead of CSI sequences.
    pub app_cursor: bool,
}

/// The bytes for `keystroke`, or `None` if the key sends nothing.
pub fn to_bytes(keystroke: &Keystroke, modes: KeyModes) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    // xterm's modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4).
    let param = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let key = keystroke.key.as_str();

    if let Some(bytes) = special_key(key, param, modes) {
        return Some(bytes);
    }
    match key {
        "enter" => return Some(with_alt(m.alt, b"\r")),
        "escape" => return Some(with_alt(m.alt, b"\x1b")),
        "tab" if m.control => return None, // Ctrl+Tab switches tabs (spec 0004)
        "tab" if m.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(with_alt(m.alt, b"\t")),
        "backspace" if m.control => return Some(with_alt(m.alt, b"\x08")),
        "backspace" => return Some(with_alt(m.alt, b"\x7f")),
        "space" if m.control => return Some(with_alt(m.alt, b"\0")),
        "space" => return Some(with_alt(m.alt, b" ")),
        _ => {}
    }

    // Windows reports AltGr as Ctrl+Alt plus the character it types (`@`
    // on a German AltGr+Q), so a Ctrl+Alt key with its own character is text.
    if m.control
        && m.alt
        && let Some(text) = keystroke.key_char.as_deref()
        && text != key
        && !text.chars().any(char::is_control)
    {
        return Some(text.as_bytes().to_vec());
    }
    if m.control && m.shift {
        return None;
    }
    if m.control {
        return control_byte(key).map(|byte| with_alt(m.alt, &[byte]));
    }

    // Text: what the key types with the current layout and Shift state.
    let text = keystroke
        .key_char
        .as_deref()
        .or_else(|| (key.chars().count() == 1).then_some(key))?;
    Some(with_alt(m.alt, text.as_bytes()))
}

/// Cursor, editing and function keys, with xterm's modifier encoding.
fn special_key(key: &str, param: u8, modes: KeyModes) -> Option<Vec<u8>> {
    let modified = param > 1;
    let letter = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(letter) = letter {
        let sequence = if modified {
            format!("\x1b[1;{param}{letter}")
        } else if modes.app_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        };
        return Some(sequence.into_bytes());
    }

    let tilde = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        _ => function_key_code(key),
    };
    if let Some(code) = tilde {
        let sequence = if modified {
            format!("\x1b[{code};{param}~")
        } else {
            format!("\x1b[{code}~")
        };
        return Some(sequence.into_bytes());
    }

    // F1–F4 use SS3 letters, or CSI 1;<mods> letters when modified.
    let f1_to_f4 = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    }?;
    let sequence = if modified {
        format!("\x1b[1;{param}{f1_to_f4}")
    } else {
        format!("\x1bO{f1_to_f4}")
    };
    Some(sequence.into_bytes())
}

/// xterm's `CSI <code> ~` numbers for F5–F12.
fn function_key_code(key: &str) -> Option<u8> {
    match key {
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    }
}

/// The C0 control byte `Ctrl+<key>` produces, for single-character keys.
fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Alt sends ESC before the key ("meta sends escape").
fn with_alt(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

impl TerminalPane {
    /// Writes what an input method committed to the program, as is.
    fn type_text(&mut self, text: &str) {
        // As for keys: nothing reaches a program waiting on a dialog.
        if self.exited || self.dialog.is_some() {
            return;
        }
        let State::Running(running) = &self.state else {
            return;
        };
        running.term.lock().scroll_display(Scroll::Bottom);
        running.notifier.notify(text.as_bytes().to_vec());
        self.keep_cursor_shown();
    }

    /// Where the cursor's cell is on screen, for the input method's
    /// candidate window.
    fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let (origin, cell) = self.geometry?;
        let State::Running(running) = &self.state else {
            return None;
        };
        let term = running.term.lock();
        let point = term.grid().cursor.point;
        let row = point.line.0 + term.grid().display_offset() as i32;
        let at = origin
            + gpui::point(
                px(pty::PADDING) + cell.width * point.column.0 as f32,
                px(pty::PADDING) + cell.height * row.max(0) as f32,
            );
        Some(Bounds::new(at, cell))
    }
}

/// Input methods (spec 0002, flow 2): what they compose shows at the
/// cursor, and what they commit goes to the program as is. The grid's text
/// isn't theirs to read, so there is nothing to select or replace.
impl EntityInputHandler for TerminalPane {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let preedit = self.preedit.as_ref()?;
        Some(0..preedit.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.preedit.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = None;
        self.type_text(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = Some(text.to_owned()).filter(|text| !text.is_empty());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds()
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    const APP_CURSOR: KeyModes = KeyModes { app_cursor: true };

    /// What a key sends. Keystrokes are shaped the way GPUI's Linux backend
    /// reports them: `key` names the key, `key_char` is the text it types
    /// (none for named keys and Ctrl combinations).
    fn send(
        modifiers: Modifiers,
        key: &str,
        key_char: Option<&str>,
        modes: KeyModes,
    ) -> Option<String> {
        let keystroke = Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(Into::into),
        };
        to_bytes(&keystroke, modes).map(|bytes| String::from_utf8(bytes).expect("UTF-8"))
    }

    /// A key that types no text, in the default modes.
    fn press(modifiers: Modifiers, key: &str) -> Option<String> {
        send(modifiers, key, None, KeyModes::default())
    }

    #[test]
    fn printable_keys_send_the_text_they_type() {
        let none = Modifiers::none();
        let cases = [
            (none, "a", "a"),
            (Modifiers::shift(), "a", "A"),
            // GPUI drops Shift for symbols: Shift+1 arrives as "!".
            (none, "!", "!"),
            // A German "ö" sits on the US ";" key: the typed text wins.
            (none, ";", "ö"),
            (none, "space", " "),
        ];
        for (modifiers, key, text) in cases {
            let sent = send(modifiers, key, Some(text), KeyModes::default());
            assert_eq!(sent.as_deref(), Some(text), "{modifiers:?} {key}");
        }
    }

    #[test]
    fn editing_keys_send_their_xterm_bytes() {
        let cases = [
            (Modifiers::none(), "enter", "\r"),
            (Modifiers::none(), "tab", "\t"),
            (Modifiers::shift(), "tab", "\x1b[Z"),
            (Modifiers::none(), "backspace", "\x7f"),
            (Modifiers::none(), "escape", "\x1b"),
            (Modifiers::none(), "insert", "\x1b[2~"),
            (Modifiers::none(), "delete", "\x1b[3~"),
            (Modifiers::none(), "pageup", "\x1b[5~"),
            (Modifiers::none(), "pagedown", "\x1b[6~"),
            (Modifiers::control(), "delete", "\x1b[3;5~"),
            (Modifiers::alt(), "pagedown", "\x1b[6;3~"),
        ];
        for (modifiers, key, expected) in cases {
            let sent = press(modifiers, key);
            assert_eq!(sent.as_deref(), Some(expected), "{modifiers:?} {key}");
        }
    }

    #[test]
    fn cursor_keys_follow_the_application_cursor_mode() {
        let cases = [
            ("up", "\x1b[A", "\x1bOA"),
            ("down", "\x1b[B", "\x1bOB"),
            ("right", "\x1b[C", "\x1bOC"),
            ("left", "\x1b[D", "\x1bOD"),
            ("home", "\x1b[H", "\x1bOH"),
            ("end", "\x1b[F", "\x1bOF"),
        ];
        for (key, normal, app) in cases {
            let sent = press(Modifiers::none(), key);
            assert_eq!(sent.as_deref(), Some(normal), "{key}");
            let sent = send(Modifiers::none(), key, None, APP_CURSOR);
            assert_eq!(sent.as_deref(), Some(app), "{key}, app cursor mode");
        }
    }

    #[test]
    fn modified_cursor_keys_send_csi_1_with_the_modifier_in_either_mode() {
        // The parameter is 1 + Shift 1 + Alt 2 + Ctrl 4.
        let cases = [
            (Modifiers::shift(), "up", "\x1b[1;2A"),
            (Modifiers::alt(), "right", "\x1b[1;3C"),
            (Modifiers::control(), "left", "\x1b[1;5D"),
            (Modifiers::control_shift(), "right", "\x1b[1;6C"),
            (Modifiers::control() | Modifiers::alt(), "down", "\x1b[1;7B"),
            (Modifiers::control(), "home", "\x1b[1;5H"),
            (Modifiers::shift(), "end", "\x1b[1;2F"),
        ];
        for (modifiers, key, expected) in cases {
            for modes in [KeyModes::default(), APP_CURSOR] {
                let sent = send(modifiers, key, None, modes);
                assert_eq!(
                    sent.as_deref(),
                    Some(expected),
                    "{modifiers:?} {key} {modes:?}"
                );
            }
        }
    }

    #[test]
    fn function_keys_send_xterm_sequences() {
        let cases = [
            (Modifiers::none(), "f1", "\x1bOP"),
            (Modifiers::none(), "f2", "\x1bOQ"),
            (Modifiers::none(), "f3", "\x1bOR"),
            (Modifiers::none(), "f4", "\x1bOS"),
            (Modifiers::none(), "f5", "\x1b[15~"),
            (Modifiers::none(), "f6", "\x1b[17~"),
            (Modifiers::none(), "f7", "\x1b[18~"),
            (Modifiers::none(), "f8", "\x1b[19~"),
            (Modifiers::none(), "f9", "\x1b[20~"),
            (Modifiers::none(), "f10", "\x1b[21~"),
            (Modifiers::none(), "f11", "\x1b[23~"),
            (Modifiers::none(), "f12", "\x1b[24~"),
            (Modifiers::shift(), "f1", "\x1b[1;2P"),
            (Modifiers::control(), "f4", "\x1b[1;5S"),
            (Modifiers::shift(), "f5", "\x1b[15;2~"),
            (Modifiers::control(), "f12", "\x1b[24;5~"),
        ];
        for (modifiers, key, expected) in cases {
            let sent = press(modifiers, key);
            assert_eq!(sent.as_deref(), Some(expected), "{modifiers:?} {key}");
        }
    }

    #[test]
    fn ctrl_keys_send_c0_control_bytes() {
        let cases = [
            ("a", "\x01"),
            ("c", "\x03"),
            ("d", "\x04"),
            ("r", "\x12"),
            ("z", "\x1a"),
            ("space", "\0"),
            ("[", "\x1b"),
            ("\\", "\x1c"),
            ("]", "\x1d"),
            // Readline's undo (spec 0004).
            ("-", "\x1f"),
        ];
        for (key, expected) in cases {
            let sent = press(Modifiers::control(), key);
            assert_eq!(sent.as_deref(), Some(expected), "ctrl-{key}");
        }
    }

    #[test]
    fn alt_sends_esc_before_the_key() {
        let alt = Modifiers::alt();
        let cases = [
            // Readline: back a word, digit argument (spec 0004).
            (alt, "b", Some("b"), "\x1bb"),
            (alt, "3", Some("3"), "\x1b3"),
            (alt | Modifiers::shift(), "b", Some("B"), "\x1bB"),
            (alt | Modifiers::control(), "c", None, "\x1b\x03"),
            (alt, "backspace", None, "\x1b\x7f"),
            (alt, "enter", None, "\x1b\r"),
        ];
        for (modifiers, key, key_char, expected) in cases {
            let sent = send(modifiers, key, key_char, KeyModes::default());
            assert_eq!(sent.as_deref(), Some(expected), "{modifiers:?} {key}");
        }
    }

    #[test]
    fn app_shortcut_combinations_send_nothing() {
        let ctrl_shift = Modifiers::control_shift();
        // Super on Linux, Win on Windows, Cmd on macOS.
        let platform = Modifiers::super_key();
        let cases = [
            // Copy, paste, search, palette, next and previous tab (spec 0004).
            (ctrl_shift, "c", None),
            (ctrl_shift, "v", None),
            (ctrl_shift, "f", None),
            (ctrl_shift, "p", None),
            (Modifiers::control(), "tab", None),
            (ctrl_shift, "tab", None),
            (platform, "a", Some("a")),
            (platform, "c", Some("c")),
            (platform, "left", None),
            (platform, "enter", None),
        ];
        for (modifiers, key, key_char) in cases {
            let sent = send(modifiers, key, key_char, KeyModes::default());
            assert_eq!(sent, None, "{modifiers:?} {key}");
        }
    }

    /// Windows reports AltGr as Ctrl+Alt plus the character it types (as in
    /// GPUI's own AltGr tests). Every European layout types `@`, `{`, `[`,
    /// `\`, `|` and `~` this way (spec 0002, Platform notes).
    #[test]
    fn altgr_on_windows_types_its_character() {
        let altgr = Modifiers::control() | Modifiers::alt();
        let cases = [("q", "@"), ("7", "{"), ("e", "€")];
        for (key, text) in cases {
            let sent = send(altgr, key, Some(text), KeyModes::default());
            assert_eq!(sent.as_deref(), Some(text), "AltGr+{key}");
        }
    }
}
