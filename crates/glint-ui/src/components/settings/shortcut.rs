//! The quick search shortcut field: what is accepted, how it is written down, and what is said when it isn't.
//!
//! The TypeScript offered three presets and Off. Here a custom shortcut can be typed too, checked with the
//! tray's own parser (`platform::tray::parse`, the one `RegisterHotKey` is given), so a shortcut that is
//! accepted here is one the tray can register. Another app holding it is only known when registering, which
//! `startup.rs` does when the setting changes; [`hint`] says so once it is told.

use crate::platform::tray;
use crate::t;
use gpui_kit::SharedString;

/// The presets of the TypeScript's segmented choice, in its order.
pub const PRESETS: [&str; 3] = ["Ctrl+Alt+F", "Ctrl+Alt+Space", "Ctrl+Shift+Space"];

/// Why `typed` can't be the shortcut, or the shortcut written the way the settings keep it ("Ctrl+Alt+Space").
pub fn check(typed: &str) -> Result<String, SharedString> {
    let typed = typed.trim();
    match tray::parse(typed) {
        Some((modifiers, key)) => Ok(canonical(modifiers, key)),
        None => Err(t!("“{shortcut}” isn't a shortcut Glint knows", shortcut = typed)),
    }
}

/// `RegisterHotKey`'s modifiers and virtual key as text: Ctrl, Alt, Shift, Win, then the key.
fn canonical(modifiers: u32, key: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (bit, name) in [(0x2, "Ctrl"), (0x1, "Alt"), (0x4, "Shift"), (0x8, "Win")] {
        if modifiers & bit != 0 {
            parts.push(name.to_string());
        }
    }
    parts.push(match key {
        0x20 => "Space".to_string(),
        0x70..=0x7B => format!("F{}", key - 0x6F),
        other => char::from_u32(other).map(|c| c.to_string()).unwrap_or_default(),
    });
    parts.join("+")
}

/// What the Shortcut row says under its label: that another app holds the shortcut, when it does.
pub fn hint(taken: Option<&str>, current: Option<&str>) -> SharedString {
    match (taken, current) {
        (Some(taken), Some(current)) if taken == current => t!(
            "Another app already uses {shortcut}, so it doesn't open quick search: pick another",
            shortcut = taken
        ),
        _ => t!("From anywhere: a search box over every drive. Enter opens, Alt+Enter shows it in Glint"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_all_accepted_as_written() {
        let _guard = crate::testing::lock();
        for preset in PRESETS {
            assert_eq!(check(preset).ok().as_deref(), Some(preset));
        }
    }

    #[test]
    fn typed_shortcuts_are_written_in_one_form() {
        let _guard = crate::testing::lock();
        assert_eq!(check("  ctrl + alt + space ").ok().as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(check("win+shift+e").ok().as_deref(), Some("Shift+Win+E"));
        assert_eq!(check("alt+f9").ok().as_deref(), Some("Alt+F9"));
        assert_eq!(check("Ctrl+1").ok().as_deref(), Some("Ctrl+1"));
    }

    #[test]
    fn a_key_alone_or_nonsense_says_it_is_not_a_shortcut() {
        let _guard = crate::testing::lock();
        for bad in [
            "",
            "F",
            "Space",
            "Ctrl+",
            "Ctrl+Alt",
            "Ctrl+F13",
            "Ctrl+Enter",
            "banana",
        ] {
            let error = check(bad).expect_err(bad);
            assert_eq!(error, format!("“{bad}” isn't a shortcut Glint knows"), "{bad}");
        }
    }

    #[test]
    fn the_hint_names_a_shortcut_another_app_holds_only_while_it_is_the_one_chosen() {
        let _guard = crate::testing::lock();
        assert!(hint(Some("Ctrl+Alt+F"), Some("Ctrl+Alt+F")).starts_with("Another app already uses Ctrl+Alt+F"));
        assert!(hint(Some("Ctrl+Alt+F"), Some("Ctrl+Alt+Space")).starts_with("From anywhere"));
        assert!(hint(None, None).starts_with("From anywhere"));
    }
}
