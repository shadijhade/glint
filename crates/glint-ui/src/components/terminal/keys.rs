//! Keys, paste and the mouse as the escape sequences a program reads.
//!
//! xterm.js (or ghostty-web) did this inside the TypeScript's `TermLike`; here it is plain functions of the key
//! name, the modifiers and the terminal's modes, so the whole table is tested without a window. Names are
//! GPUI's: `"enter"`, `"up"`, `"pageup"`, `"f5"`, `"a"`, `"space"`.
//!
//! What it follows is xterm's: application cursor mode (`ESC O A` for the arrows when a program asks, as
//! `less` and `vim` do), the modifier parameter (`ESC [ 1 ; 5 C` for Ctrl+Right), Ctrl+letter as a control
//! character, Alt as an `ESC` prefix, Backspace as DEL, and bracketed paste. The kitty keyboard protocol is not
//! spoken (a program that asks for it is told no by the terminal and gets the classic sequences).

use alacritty_terminal::term::TermMode;

/// The modifiers that change what a key sends. (The Windows key is never sent to a shell.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Mods {
    /// xterm's modifier parameter: 1 plus shift 1, alt 2, ctrl 4; none when no modifier is held.
    fn param(self) -> Option<u8> {
        let value = 1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl);
        (value > 1).then_some(value)
    }
}

/// Whether the key is typed text: the platform sends it as text (so an input method can take it) and the
/// terminal does not encode it itself. A key with Ctrl or Alt held is a shortcut, except AltGr (Windows
/// reports it as Ctrl+Alt), which makes characters.
pub fn is_text(key_char: Option<&str>, mods: Mods) -> bool {
    key_char.is_some_and(|text| !text.is_empty()) && (mods.ctrl == mods.alt)
}

/// The sequence for a key, or `None` when it sends nothing (a modifier alone, a key a shell has no use for).
pub fn encode_key(key: &str, key_char: Option<&str>, mods: Mods, mode: TermMode) -> Option<String> {
    let app = mode.contains(TermMode::APP_CURSOR);
    let param = mods.param();
    let csi = |last: char| match param {
        None => format!("\x1b[{last}"),
        Some(m) => format!("\x1b[1;{m}{last}"),
    };
    let tilde = |n: u8| match param {
        None => format!("\x1b[{n}~"),
        Some(m) => format!("\x1b[{n};{m}~"),
    };
    let ss3 = |last: char| match param {
        None => format!("\x1bO{last}"),
        Some(m) => format!("\x1b[1;{m}{last}"),
    };
    // The arrows and Home/End: application mode switches `ESC [` for `ESC O` while no modifier is held.
    let cursor = |last: char| {
        if app && param.is_none() {
            format!("\x1bO{last}")
        } else {
            csi(last)
        }
    };
    let sent = match key {
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "f13" => tilde(25),
        "f14" => tilde(26),
        "f15" => tilde(28),
        "f16" => tilde(29),
        "f17" => tilde(31),
        "f18" => tilde(32),
        "f19" => tilde(33),
        "f20" => tilde(34),
        "enter" => {
            if mods.alt {
                "\x1b\r".to_string()
            } else if mods.ctrl {
                "\n".to_string()
            } else {
                "\r".to_string()
            }
        }
        "tab" => {
            if mods.shift {
                "\x1b[Z".to_string()
            } else if mods.alt {
                "\x1b\t".to_string()
            } else {
                "\t".to_string()
            }
        }
        "escape" => {
            if mods.alt {
                "\x1b\x1b".to_string()
            } else {
                "\x1b".to_string()
            }
        }
        "backspace" => {
            if mods.ctrl {
                "\x08".to_string()
            } else if mods.alt {
                "\x1b\x7f".to_string()
            } else {
                "\x7f".to_string()
            }
        }
        "space" => {
            if mods.ctrl {
                "\0".to_string()
            } else if mods.alt {
                "\x1b ".to_string()
            } else {
                " ".to_string()
            }
        }
        other => return encode_character(other, key_char, mods),
    };
    Some(sent)
}

/// A letter, digit or punctuation key.
fn encode_character(key: &str, key_char: Option<&str>, mods: Mods) -> Option<String> {
    let mut chars = key.chars();
    let base = chars.next()?;
    if chars.next().is_some() {
        // A name this table doesn't know ("shift", "numlock", "f21"…).
        return None;
    }
    if mods.ctrl && !mods.alt {
        let control = control_char(base)?;
        return Some(control.to_string());
    }
    let typed = key_char.filter(|text| !text.is_empty()).unwrap_or(key);
    if mods.alt {
        return Some(format!("\x1b{typed}"));
    }
    Some(typed.to_string())
}

/// What Ctrl+`key` sends: letters give 1-26, and the punctuation xterm maps to the rest of the C0 set.
fn control_char(key: char) -> Option<char> {
    let code = match key {
        'a'..='z' => key as u32 - 'a' as u32 + 1,
        'A'..='Z' => key as u32 - 'A' as u32 + 1,
        '@' | '2' | ' ' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    };
    char::from_u32(code)
}

/// What pasting `text` sends. With bracketed paste on, the text is wrapped so a shell knows it was pasted (and
/// won't run it line by line); an Escape or Ctrl+C inside it is dropped so the paste can't end the bracket or
/// interrupt the program. Without it, line breaks are Enter.
pub fn paste_text(text: &str, mode: TermMode) -> String {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean: String = text.chars().filter(|c| *c != '\x1b' && *c != '\x03').collect();
        format!("\x1b[200~{clean}\x1b[201~")
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r")
    }
}

/// What a wheel turn sends to a full-screen program that has no mouse mode on (`less`, `man`, `vim` in the
/// alternate screen): the arrow key, `lines` times, as if pressed. `up` scrolls towards older text.
pub fn wheel_as_arrows(up: bool, lines: usize, mode: TermMode) -> String {
    let last = if up { 'A' } else { 'B' };
    let one = if mode.contains(TermMode::APP_CURSOR) {
        format!("\x1bO{last}")
    } else {
        format!("\x1b[{last}")
    };
    one.repeat(lines)
}

/// A mouse button a program can be told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
        }
    }
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Press(Button),
    Release(Button),
    /// Moved, with a button held or not.
    Move(Option<Button>),
    /// A wheel notch: up is towards older text.
    Wheel {
        up: bool,
    },
}

/// The report of a mouse event for a program that asked for them (DECSET 1000, 1002, 1003; encoded as SGR
/// 1006, UTF-8 1005 or the classic X10 way). `col` and `row` count from 0. `None` when the program's mode
/// doesn't want this event, or the position can't be written in the encoding it chose.
pub fn mouse_report(event: Mouse, col: usize, row: usize, mods: Mods, mode: TermMode) -> Option<String> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let wanted = match event {
        Mouse::Press(_) | Mouse::Release(_) | Mouse::Wheel { .. } => true,
        Mouse::Move(Some(_)) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
        Mouse::Move(None) => mode.contains(TermMode::MOUSE_MOTION),
    };
    if !wanted {
        return None;
    }
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code = match event {
        Mouse::Press(button) => button.code(),
        // The classic encodings can't say which button let go.
        Mouse::Release(button) => {
            if sgr {
                button.code()
            } else {
                3
            }
        }
        Mouse::Move(button) => 32 + button.map_or(3, Button::code),
        Mouse::Wheel { up } => 64 + u32::from(!up),
    };
    code += 4 * u32::from(mods.shift) + 8 * u32::from(mods.alt) + 16 * u32::from(mods.ctrl);
    let (x, y) = (col + 1, row + 1);
    if sgr {
        let last = if matches!(event, Mouse::Release(_)) { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{last}"));
    }
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    // Each value is sent as one character: 32 more than the number. Past 127 that needs the UTF-8 form.
    let limit = if utf8 { 2015 } else { 95 };
    if x > limit || y > limit {
        return None;
    }
    let ch = |n: usize| char::from_u32(n as u32 + 32);
    Some(format!("\x1b[M{}{}{}", ch(code as usize)?, ch(x)?, ch(y)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
    };
    const CTRL: Mods = Mods {
        shift: false,
        ctrl: true,
        alt: false,
    };
    const ALT: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: true,
    };
    const SHIFT: Mods = Mods {
        shift: true,
        ctrl: false,
        alt: false,
    };
    const CTRL_SHIFT: Mods = Mods {
        shift: true,
        ctrl: true,
        alt: false,
    };

    fn key(name: &str, mods: Mods) -> Option<String> {
        encode_key(name, None, mods, TermMode::empty())
    }

    #[test]
    fn named_keys_send_xterms_sequences() {
        assert_eq!(key("enter", NONE).as_deref(), Some("\r"));
        assert_eq!(key("tab", NONE).as_deref(), Some("\t"));
        assert_eq!(key("tab", SHIFT).as_deref(), Some("\x1b[Z"));
        assert_eq!(key("escape", NONE).as_deref(), Some("\x1b"));
        assert_eq!(key("backspace", NONE).as_deref(), Some("\x7f"));
        assert_eq!(key("backspace", CTRL).as_deref(), Some("\x08"));
        assert_eq!(key("backspace", ALT).as_deref(), Some("\x1b\x7f"));
        assert_eq!(key("delete", NONE).as_deref(), Some("\x1b[3~"));
        assert_eq!(key("insert", NONE).as_deref(), Some("\x1b[2~"));
        assert_eq!(key("pageup", NONE).as_deref(), Some("\x1b[5~"));
        assert_eq!(key("pagedown", NONE).as_deref(), Some("\x1b[6~"));
        assert_eq!(key("home", NONE).as_deref(), Some("\x1b[H"));
        assert_eq!(key("end", NONE).as_deref(), Some("\x1b[F"));
        assert_eq!(key("f1", NONE).as_deref(), Some("\x1bOP"));
        assert_eq!(key("f4", NONE).as_deref(), Some("\x1bOS"));
        assert_eq!(key("f5", NONE).as_deref(), Some("\x1b[15~"));
        assert_eq!(key("f12", NONE).as_deref(), Some("\x1b[24~"));
        assert_eq!(key("space", CTRL).as_deref(), Some("\0"));
        assert_eq!(key("alt", NONE), None);
        assert_eq!(key("shift", SHIFT), None);
    }

    #[test]
    fn application_cursor_mode_switches_the_arrows_to_ss3() {
        let app = TermMode::APP_CURSOR;
        for (name, normal, application) in [
            ("up", "\x1b[A", "\x1bOA"),
            ("down", "\x1b[B", "\x1bOB"),
            ("right", "\x1b[C", "\x1bOC"),
            ("left", "\x1b[D", "\x1bOD"),
            ("home", "\x1b[H", "\x1bOH"),
            ("end", "\x1b[F", "\x1bOF"),
        ] {
            assert_eq!(
                encode_key(name, None, NONE, TermMode::empty()).as_deref(),
                Some(normal),
                "{name}"
            );
            assert_eq!(
                encode_key(name, None, NONE, app).as_deref(),
                Some(application),
                "{name} in application mode"
            );
        }
        // With a modifier it is always the CSI form with the parameter.
        assert_eq!(encode_key("right", None, CTRL, app).as_deref(), Some("\x1b[1;5C"));
    }

    #[test]
    fn modifiers_are_xterms_parameter() {
        assert_eq!(key("right", CTRL).as_deref(), Some("\x1b[1;5C"));
        assert_eq!(key("left", SHIFT).as_deref(), Some("\x1b[1;2D"));
        assert_eq!(key("up", ALT).as_deref(), Some("\x1b[1;3A"));
        assert_eq!(key("up", CTRL_SHIFT).as_deref(), Some("\x1b[1;6A"));
        assert_eq!(key("delete", CTRL).as_deref(), Some("\x1b[3;5~"));
        assert_eq!(key("pageup", SHIFT).as_deref(), Some("\x1b[5;2~"));
        assert_eq!(key("f5", CTRL).as_deref(), Some("\x1b[15;5~"));
        assert_eq!(key("f1", SHIFT).as_deref(), Some("\x1b[1;2P"));
        assert_eq!(key("enter", ALT).as_deref(), Some("\x1b\r"));
    }

    #[test]
    fn ctrl_letters_are_control_characters() {
        assert_eq!(encode_key("c", None, CTRL, TermMode::empty()).as_deref(), Some("\x03"));
        assert_eq!(encode_key("a", None, CTRL, TermMode::empty()).as_deref(), Some("\x01"));
        assert_eq!(encode_key("z", None, CTRL, TermMode::empty()).as_deref(), Some("\x1a"));
        assert_eq!(
            encode_key("l", None, CTRL_SHIFT, TermMode::empty()).as_deref(),
            Some("\x0c")
        );
        assert_eq!(encode_key("[", None, CTRL, TermMode::empty()).as_deref(), Some("\x1b"));
        assert_eq!(encode_key("\\", None, CTRL, TermMode::empty()).as_deref(), Some("\x1c"));
        assert_eq!(encode_key("]", None, CTRL, TermMode::empty()).as_deref(), Some("\x1d"));
        assert_eq!(encode_key("/", None, CTRL, TermMode::empty()).as_deref(), Some("\x1f"));
        assert_eq!(encode_key("2", None, CTRL, TermMode::empty()).as_deref(), Some("\0"));
        assert_eq!(encode_key("8", None, CTRL, TermMode::empty()).as_deref(), Some("\x7f"));
        // Ctrl with a key that has no control character sends nothing.
        assert_eq!(encode_key("1", None, CTRL, TermMode::empty()), None);
    }

    #[test]
    fn alt_prefixes_escape_and_plain_keys_are_their_text() {
        assert_eq!(
            encode_key("b", Some("b"), ALT, TermMode::empty()).as_deref(),
            Some("\x1bb")
        );
        // Alt+Shift+B: the shifted character.
        let alt_shift = Mods {
            shift: true,
            ctrl: false,
            alt: true,
        };
        assert_eq!(
            encode_key("b", Some("B"), alt_shift, TermMode::empty()).as_deref(),
            Some("\x1bB")
        );
        assert_eq!(
            encode_key("a", Some("a"), NONE, TermMode::empty()).as_deref(),
            Some("a")
        );
        assert_eq!(
            encode_key("1", Some("!"), SHIFT, TermMode::empty()).as_deref(),
            Some("!")
        );
        assert_eq!(
            encode_key("space", Some(" "), NONE, TermMode::empty()).as_deref(),
            Some(" ")
        );
    }

    #[test]
    fn typed_text_is_left_to_the_input_method_but_shortcuts_are_not() {
        assert!(is_text(Some("a"), NONE));
        assert!(is_text(Some("A"), SHIFT));
        assert!(is_text(Some("é"), NONE));
        // AltGr is Ctrl+Alt on Windows and makes characters.
        let altgr = Mods {
            shift: false,
            ctrl: true,
            alt: true,
        };
        assert!(is_text(Some("@"), altgr));
        assert!(!is_text(Some("c"), CTRL));
        assert!(!is_text(Some("b"), ALT));
        assert!(!is_text(None, NONE));
        assert!(!is_text(Some(""), NONE));
    }

    #[test]
    fn paste_is_bracketed_when_the_program_asked_and_has_escapes_removed() {
        let plain = paste_text("one\ntwo\r\nthree", TermMode::empty());
        assert_eq!(plain, "one\rtwo\rthree");
        let bracketed = paste_text("a\nb\x1b[201~c\x03", TermMode::BRACKETED_PASTE);
        assert_eq!(bracketed, "\x1b[200~a\nb[201~c\x1b[201~");
        // Arabic text goes through untouched.
        assert_eq!(paste_text("مرحبا", TermMode::empty()), "مرحبا");
    }

    #[test]
    fn the_wheel_in_a_full_screen_program_is_arrow_keys() {
        assert_eq!(wheel_as_arrows(true, 3, TermMode::empty()), "\x1b[A\x1b[A\x1b[A");
        assert_eq!(wheel_as_arrows(false, 2, TermMode::APP_CURSOR), "\x1bOB\x1bOB");
    }

    #[test]
    fn mouse_reports_follow_the_encoding_the_program_chose() {
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            mouse_report(Mouse::Press(Button::Left), 0, 0, NONE, sgr).as_deref(),
            Some("\x1b[<0;1;1M")
        );
        assert_eq!(
            mouse_report(Mouse::Release(Button::Left), 9, 4, NONE, sgr).as_deref(),
            Some("\x1b[<0;10;5m")
        );
        assert_eq!(
            mouse_report(Mouse::Press(Button::Right), 2, 3, CTRL, sgr).as_deref(),
            Some("\x1b[<18;3;4M")
        );
        assert_eq!(
            mouse_report(Mouse::Wheel { up: true }, 1, 1, NONE, sgr).as_deref(),
            Some("\x1b[<64;2;2M")
        );
        assert_eq!(
            mouse_report(Mouse::Wheel { up: false }, 1, 1, SHIFT, sgr).as_deref(),
            Some("\x1b[<69;2;2M")
        );
        // The classic encoding: 32 more than each number; a release doesn't say which button.
        let x10 = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            mouse_report(Mouse::Press(Button::Left), 0, 0, NONE, x10).as_deref(),
            Some("\x1b[M !!")
        );
        assert_eq!(
            mouse_report(Mouse::Release(Button::Middle), 0, 0, NONE, x10).as_deref(),
            Some("\x1b[M#!!")
        );
        // Too far right for it: nothing is sent rather than something wrong.
        assert_eq!(mouse_report(Mouse::Press(Button::Left), 200, 0, NONE, x10), None);
        assert!(mouse_report(Mouse::Press(Button::Left), 200, 0, NONE, x10 | TermMode::UTF8_MOUSE).is_some());
    }

    #[test]
    fn motion_is_reported_only_in_the_modes_that_ask_for_it() {
        let click = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let motion = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(mouse_report(Mouse::Move(Some(Button::Left)), 3, 3, NONE, click), None);
        assert_eq!(
            mouse_report(Mouse::Move(Some(Button::Left)), 3, 3, NONE, drag).as_deref(),
            Some("\x1b[<32;4;4M")
        );
        assert_eq!(mouse_report(Mouse::Move(None), 3, 3, NONE, drag), None);
        assert_eq!(
            mouse_report(Mouse::Move(None), 3, 3, NONE, motion).as_deref(),
            Some("\x1b[<35;4;4M")
        );
        // No mouse mode: nothing, so the terminal selects text itself.
        assert_eq!(
            mouse_report(Mouse::Press(Button::Left), 0, 0, NONE, TermMode::SGR_MOUSE),
            None
        );
    }
}
