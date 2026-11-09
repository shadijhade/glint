//! Ported from src/lib/cd.ts.
//!
//! The address bar's cd command line: what each shell is typed to run to go to a folder, what it is typed
//! to run a script with, and the check that tells the person typing apart from the terminal's own reports.
//!
//! Every shell gets its text quoted for that shell, so a folder or file name with a quote or a space still
//! reaches the program as it was. The quoting only swaps ASCII characters, so it is safe on any UTF-8 name.

/// What a shell is told to go to `path` (a Windows path), as typed at its prompt, Enter included.
///
/// `shell` is the name the terminal gives its tab (`pwsh`, `powershell`, `cmd`, `bash`, `wsl`, `nu`); any
/// other name gets the plain `cd "..."` that the TypeScript used as its fallback.
pub fn cd_command(shell: &str, path: &str) -> String {
    match shell {
        // Inside single quotes: PowerShell doubles a quote, bash closes, escapes and reopens one.
        "pwsh" | "powershell" => format!("Set-Location -LiteralPath '{}'\r", single_quoted_pwsh(path)),
        // A Windows path can't hold a double quote.
        "cmd" => format!("cd /d \"{path}\"\r"),
        "bash" => format!("cd \"$(cygpath -u {})\"\r", single_quoted_bash(path)),
        "wsl" => format!("cd \"$(wslpath {})\"\r", single_quoted_bash(path)),
        "nu" => format!("cd \"{}\"\r", nu_inside_quotes(path)),
        _ => format!("cd \"{path}\"\r"),
    }
}

/// What a shell is told to run `program` (a Windows path) with `args`, as typed at its prompt, Enter
/// included: each quoted as the shell needs, so spaces and quotes in paths stay as they are.
///
/// `program` comes first in the list the quoting is applied to, as in the TypeScript. The fallback for
/// other shell names does no escaping at all, so it is only for shells the app doesn't know.
pub fn run_line(shell: &str, program: &str, args: &[&str]) -> String {
    let mut words = Vec::with_capacity(args.len() + 1);
    words.push(program);
    words.extend_from_slice(args);
    match shell {
        "pwsh" | "powershell" => {
            let quoted: Vec<String> = words.iter().map(|w| format!("'{}'", single_quoted_pwsh(w))).collect();
            format!("& {}\r", quoted.join(" "))
        }
        "cmd" => {
            // A Windows path can't hold a double quote; neither may what's passed with it here.
            let quoted: Vec<String> = words.iter().map(|w| format!("\"{}\"", w.replace('"', ""))).collect();
            format!("{}\r", quoted.join(" "))
        }
        "bash" => {
            // The argument list is joined even when it is empty, so an empty list leaves a trailing space, as before.
            let rest: Vec<String> = args.iter().map(|a| single_quoted_bash(a)).collect();
            format!("\"$(cygpath -u {})\" {}\r", single_quoted_bash(program), rest.join(" "))
        }
        "nu" => {
            let quoted: Vec<String> = words.iter().map(|w| format!("\"{}\"", nu_inside_quotes(w))).collect();
            format!("^{}\r", quoted.join(" "))
        }
        _ => {
            let rest: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
            format!("\"{program}\" {}\r", rest.join(" "))
        }
    }
}

/// Whether what a terminal sends is the person typing, rather than its own reports: focus in and out
/// (ESC [I, ESC [O) and the cursor's position (ESC [row;colR).
///
/// Matches the TypeScript's regex exactly: the report must be the whole string, and the digits are ASCII.
pub fn is_typing(data: &str) -> bool {
    !is_terminal_report(data)
}

/// The text a PowerShell single-quoted string needs: a quote is doubled.
fn single_quoted_pwsh(text: &str) -> String {
    text.replace('\'', "''")
}

/// The text a bash single-quoted string needs, already wrapped in single quotes: a quote closes the
/// string, adds an escaped quote, and reopens it.
fn single_quoted_bash(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Escapes for a nushell double-quoted string: backslashes and double quotes, in that order.
fn nu_inside_quotes(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Focus in (ESC [I), focus out (ESC [O), or a cursor position report (ESC [row;colR), and nothing else.
fn is_terminal_report(data: &str) -> bool {
    if data == "\x1b[I" || data == "\x1b[O" {
        return true;
    }
    let Some(body) = data.strip_prefix("\x1b[").and_then(|rest| rest.strip_suffix('R')) else {
        return false;
    };
    let Some((row, col)) = body.split_once(';') else {
        return false;
    };
    is_ascii_digits(row) && is_ascii_digits(col)
}

/// One or more ASCII digits, like the TypeScript's `\d+`.
fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cd_command_quotes_the_path_for_each_shell() {
        // PowerShell doubles the apostrophe inside its single quotes.
        assert_eq!(
            cd_command("pwsh", "C:\\It's mine"),
            "Set-Location -LiteralPath 'C:\\It''s mine'\r"
        );
        assert_eq!(cd_command("cmd", "D:\\Work Files"), "cd /d \"D:\\Work Files\"\r");
        // bash closes its quotes, escapes the apostrophe, and reopens them.
        assert_eq!(cd_command("bash", "C:\\it's"), "cd \"$(cygpath -u 'C:\\it'\\''s')\"\r");
        // nushell doubles each backslash inside its double quotes.
        assert_eq!(cd_command("nu", "C:\\a b"), "cd \"C:\\\\a b\"\r");
    }

    #[test]
    fn is_typing_ignores_focus_and_cursor_reports() {
        assert!(is_typing("ls\r"));
        assert!(is_typing("\x1b[A"));
        assert!(!is_typing("\x1b[I"));
        assert!(!is_typing("\x1b[O"));
        assert!(!is_typing("\x1b[12;40R"));
    }

    #[test]
    fn is_typing_counts_anything_that_is_not_exactly_a_report_as_typing() {
        // The report has to be the whole input, with digits only on both sides of the semicolon.
        assert!(is_typing("\x1b[12;40Rx"));
        assert!(is_typing("\x1b[;40R"));
        assert!(is_typing("\x1b[12;R"));
        assert!(is_typing("\x1b[1;2;3R"));
        assert!(is_typing(""));
    }

    #[test]
    fn run_line_quotes_a_script_and_its_paths_for_each_shell() {
        assert_eq!(
            run_line("pwsh", "C:\\Scripts\\tidy.ps1", &["C:\\It's here", "D:\\b"]),
            "& 'C:\\Scripts\\tidy.ps1' 'C:\\It''s here' 'D:\\b'\r"
        );
        assert_eq!(
            run_line("cmd", "C:\\Scripts\\go.cmd", &["C:\\a b"]),
            "\"C:\\Scripts\\go.cmd\" \"C:\\a b\"\r"
        );
        assert_eq!(
            run_line("bash", "C:\\Scripts\\x.exe", &["C:\\it's"]),
            "\"$(cygpath -u 'C:\\Scripts\\x.exe')\" 'C:\\it'\\''s'\r"
        );
        assert_eq!(
            run_line("nu", "C:\\Scripts\\x.exe", &["C:\\a \"b\""]),
            "^\"C:\\\\Scripts\\\\x.exe\" \"C:\\\\a \\\"b\\\"\"\r"
        );
    }

    #[test]
    fn run_line_keeps_the_trailing_space_bash_and_fallback_leave_without_arguments() {
        // Ported as the TypeScript builds it: the argument list is joined even when empty.
        assert_eq!(run_line("bash", "C:\\x.exe", &[]), "\"$(cygpath -u 'C:\\x.exe')\" \r");
        assert_eq!(run_line("zsh", "C:\\x.exe", &[]), "\"C:\\x.exe\" \r");
    }

    #[test]
    fn cd_command_fallback_and_wsl_use_their_own_quoting() {
        // A shell the app doesn't know gets the plain form, with no escaping.
        assert_eq!(cd_command("zsh", "C:\\a b"), "cd \"C:\\a b\"\r");
        assert_eq!(cd_command("wsl", "C:\\it's"), "cd \"$(wslpath 'C:\\it'\\''s')\"\r");
    }
}
