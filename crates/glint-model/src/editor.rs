//! How to start the editor Settings names so it opens a file at a line: a search's matching line, double-clicked.
//!
//! Editors take a line in their own ways (`code --goto file:12`, `notepad++ -n12 file`, `vim +12 file`), so the
//! ones that are known are told by the program's name. An editor not known is given the file alone, which opens
//! it at its top: a made-up "file:12" would be a file that isn't there.

use crate::paths::base_name;

/// The arguments that open `path` in the editor at `editor` (its program, as Settings has it), at `line`
/// (from 1). The file alone for a program whose way of taking a line isn't known, or when `line` is 0.
pub fn editor_args(editor: &str, path: &str, line: u64) -> Vec<String> {
    if line == 0 {
        return vec![path.to_string()];
    }
    let at = format!("{path}:{line}");
    let name = program_name(editor);
    let name = name.as_str();
    let is = |names: &[&str]| names.contains(&name);
    // The Visual Studio Code family and what is built on it.
    if is(&[
        "code",
        "code-insiders",
        "codium",
        "vscodium",
        "cursor",
        "windsurf",
        "antigravity",
        "trae",
        "kiro",
    ]) {
        vec!["--goto".to_string(), at]
    } else if name == "notepad++" {
        vec![format!("-n{line}"), path.to_string()]
    } else if is(&["sublime_text", "subl", "zed", "hx", "helix"]) {
        vec![at]
    } else if is(&["vim", "gvim", "nvim", "nvim-qt", "vi"]) {
        vec![format!("+{line}"), path.to_string()]
    } else if JETBRAINS
        .iter()
        .any(|ide| name.strip_suffix("64").unwrap_or(name) == *ide)
    {
        vec!["--line".to_string(), line.to_string(), path.to_string()]
    } else {
        vec![path.to_string()]
    }
}

/// The JetBrains IDEs' program names (their Windows launchers end in 64).
const JETBRAINS: [&str; 11] = [
    "idea",
    "pycharm",
    "webstorm",
    "clion",
    "rider",
    "goland",
    "rubymine",
    "phpstorm",
    "datagrip",
    "rustrover",
    "aqua",
];

/// A program's name as the table above has it: the file's name in lower case without ".exe", ".cmd" or ".bat".
fn program_name(editor: &str) -> String {
    let file = base_name(editor.trim().trim_matches('"')).to_lowercase();
    for extension in [".exe", ".cmd", ".bat"] {
        if let Some(name) = file.strip_suffix(extension) {
            return name.to_string();
        }
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(editor: &str, line: u64) -> Vec<String> {
        editor_args(editor, "C:\\src\\main.rs", line)
    }

    #[test]
    fn the_code_family_goes_to_a_line_with_goto() {
        let at = ["--goto", "C:\\src\\main.rs:42"];
        assert_eq!(
            args(
                "C:\\Users\\me\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
                42
            ),
            at
        );
        assert_eq!(args("cursor", 42), at);
        assert_eq!(args("code.cmd", 42), at);
        assert_eq!(args("\"C:\\Program Files\\VSCodium\\VSCodium.exe\"", 42), at);
    }

    #[test]
    fn other_editors_take_the_line_their_own_way() {
        assert_eq!(
            args("C:\\Program Files\\Notepad++\\notepad++.exe", 7),
            ["-n7", "C:\\src\\main.rs"]
        );
        assert_eq!(args("subl", 7), ["C:\\src\\main.rs:7"]);
        assert_eq!(args("zed.exe", 7), ["C:\\src\\main.rs:7"]);
        assert_eq!(args("nvim", 7), ["+7", "C:\\src\\main.rs"]);
        assert_eq!(
            args("C:\\JetBrains\\bin\\idea64.exe", 7),
            ["--line", "7", "C:\\src\\main.rs"]
        );
        assert_eq!(args("rustrover", 7), ["--line", "7", "C:\\src\\main.rs"]);
    }

    #[test]
    fn an_editor_not_known_is_given_the_file_alone() {
        assert_eq!(args("notepad", 7), ["C:\\src\\main.rs"]);
        assert_eq!(args("C:\\Tools\\mystery.exe", 7), ["C:\\src\\main.rs"]);
        assert_eq!(args("", 7), ["C:\\src\\main.rs"]);
        // No line (a document's passage has none): the file alone, whoever the editor is.
        assert_eq!(args("code", 0), ["C:\\src\\main.rs"]);
    }
}
