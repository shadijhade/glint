//! The terminal pane: shells in Windows' pseudo console (ConPTY, through WezTerm's portable-pty), their
//! output sent to the page as it comes, and the folder each is in read from what its prompt says
//! (OSC 7 or OSC 9;9; Glint sets the prompt up to say it for PowerShell, cmd and Git Bash, and Nushell
//! says it on its own).

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tauri::ipc::{Channel, InvokeResponseBody};

/// A shell the pane can run.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Shell {
    /// "pwsh", "powershell", "cmd", "nu", "bash" or "wsl".
    pub id: &'static str,
    pub name: &'static str,
    pub path: String,
}

fn env_path(var: &str, rest: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(|v| PathBuf::from(v).join(rest)).filter(|p| p.is_file())
}

/// The shells on this PC, in the order the pane prefers them.
pub fn shells() -> Vec<Shell> {
    let mut found = Vec::new();
    let mut add = |id, name, path: Option<PathBuf>| {
        if let Some(path) = path {
            found.push(Shell { id, name, path: path.to_string_lossy().into_owned() });
        }
    };
    add(
        "pwsh",
        "PowerShell",
        crate::proc::locate("pwsh").map(|(p, _)| p).or_else(|| env_path("ProgramFiles", r"PowerShell\7\pwsh.exe")).or_else(|| env_path("LOCALAPPDATA", r"Microsoft\WindowsApps\pwsh.exe")),
    );
    add("powershell", "Windows PowerShell", env_path("SystemRoot", r"System32\WindowsPowerShell\v1.0\powershell.exe"));
    add("cmd", "Command Prompt", env_path("SystemRoot", r"System32\cmd.exe"));
    add("nu", "Nushell", crate::proc::locate("nu").map(|(p, _)| p));
    add("bash", "Git Bash", env_path("ProgramFiles", r"Git\bin\bash.exe").or_else(|| env_path("LOCALAPPDATA", r"Programs\Git\bin\bash.exe")));
    add("wsl", "WSL", env_path("SystemRoot", r"System32\wsl.exe"));
    found
}

#[tauri::command]
pub fn term_shells() -> Vec<Shell> {
    shells()
}

/// PowerShell's prompt, wrapped to say where it is first (OSC 9;9, as Windows Terminal reads it), in
/// both Windows PowerShell and PowerShell 7: the profile's own prompt is what's wrapped.
const PWSH_PROMPT: &str = r#"$global:__glintPrompt = $function:prompt; function global:prompt { $at = $executionContext.SessionState.Path.CurrentLocation; $shown = & $global:__glintPrompt; if ($at.Provider.Name -eq 'FileSystem') { "$([char]27)]9;9;`"$($at.ProviderPath)`"$([char]7)$shown" } else { $shown } }"#;

/// How a shell is started so its prompt says where it is.
fn command_for(shell: &Shell, cwd: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new(&shell.path);
    match shell.id {
        "pwsh" | "powershell" => command.args(["-NoLogo", "-NoExit", "-Command", PWSH_PROMPT]),
        // cmd prints its PROMPT variable: where it is (OSC 9;9), then its usual "C:\path>".
        "cmd" => command.env("PROMPT", "$e]9;9;$P$e\\$P$G"),
        "bash" => {
            command.args(["--login", "-i"]);
            command.env("PROMPT_COMMAND", r#"printf '\e]9;9;%s\a' "$(cygpath -w "$PWD")""#);
        }
        _ => {}
    }
    if std::path::Path::new(cwd).is_dir() {
        command.cwd(cwd);
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "Glint");
    command
}

/// Picks the folder out of what a shell prints: OSC 7 (`file://host/C:/path`) and OSC 9;9
/// (`"C:\path"`), ended by BEL or ESC \, even split across reads.
#[derive(Default)]
struct CwdScanner {
    /// An OSC being read: what's after "ESC ]" so far.
    osc: Option<Vec<u8>>,
    escape: bool,
}

/// An OSC that long isn't one of these.
const MAX_OSC: usize = 4096;

impl CwdScanner {
    /// The folders `bytes` reported, in order.
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut found = Vec::new();
        for &b in bytes {
            if let Some(osc) = self.osc.as_mut() {
                let ended = b == 0x07 || (self.escape && b == b'\\');
                if ended {
                    if self.escape {
                        osc.pop();
                    }
                    if let Some(path) = cwd_of(osc) {
                        found.push(path);
                    }
                    self.osc = None;
                } else if osc.len() > MAX_OSC {
                    self.osc = None;
                } else {
                    osc.push(b);
                }
                self.escape = b == 0x1b;
                continue;
            }
            if self.escape && b == b']' {
                self.osc = Some(Vec::new());
            }
            self.escape = b == 0x1b;
        }
        found
    }
}

/// The folder an OSC's body says: "7;file://host/C:/path" or "9;9;\"C:\\path\"".
fn cwd_of(osc: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(osc);
    if let Some(rest) = text.strip_prefix("9;9;") {
        let path = rest.trim().trim_matches('"');
        return (!path.is_empty()).then(|| path.to_string());
    }
    let url = text.strip_prefix("7;")?;
    let after = url.strip_prefix("file://")?;
    // file://host/C:/path or file:///C:/path: from the path's first slash.
    windows_path(&percent_decode(&after[after.find('/')?..]))
}

/// An OSC 7 path as Windows names it: /C:/a is C:\a, and so is WSL's /mnt/c/a. Any other Linux
/// path isn't one the pane can open.
fn windows_path(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix('/').filter(|p| p.as_bytes().get(1) == Some(&b':')) {
        return Some(rest.replace('/', "\\"));
    }
    let (drive, rest) = path.strip_prefix("/mnt/")?.split_at_checked(1)?;
    (drive.as_bytes()[0].is_ascii_alphabetic() && (rest.is_empty() || rest.starts_with('/')))
        .then(|| format!("{}:\\{}", drive.to_ascii_uppercase(), rest.trim_start_matches('/').replace('/', "\\")))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(byte) = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What happens in a session besides its output (which comes as raw bytes on the same channel).
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum TermEvent {
    /// Its prompt said it's in this folder.
    Cwd { path: String },
    /// The shell ended.
    Exit { code: u32 },
}

/// ConPTY's first words: where's the cursor? It waits for the answer before the shell starts.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
/// Top left, where a new pane's shell starts.
const CURSOR_ANSWER: &[u8] = b"\x1b[1;1R";

/// Answers ConPTY's opening question (when `first` says this is the start) so a shell starts
/// whatever draws it, and returns what's left to show.
fn answer_opening<'a>(first: bool, data: &'a [u8], writer: &mut dyn Write) -> &'a [u8] {
    match data.strip_prefix(CURSOR_QUERY) {
        Some(rest) if first => {
            let _ = writer.write_all(CURSOR_ANSWER).and_then(|_| writer.flush());
            rest
        }
        _ => data,
    }
}

struct Session {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

static SESSIONS: LazyLock<Mutex<HashMap<u32, Arc<Session>>>> = LazyLock::new(Default::default);
static NEXT: AtomicU32 = AtomicU32::new(1);

fn session(id: u32) -> Result<Arc<Session>, String> {
    SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().ok_or_else(|| "That terminal has closed".into())
}

/// Starts `shell` (an id from [`shells`]) in `cwd`, `cols` × `rows`; its output, folder reports and
/// end come on `on_output`. Returns the session's id.
#[tauri::command]
pub fn term_spawn(shell: String, cwd: String, cols: u16, rows: u16, on_output: Channel<InvokeResponseBody>) -> Result<u32, String> {
    let shell = shells().into_iter().find(|s| s.id == shell).ok_or_else(|| format!("{shell} isn't installed"))?;
    let pty = native_pty_system().openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut child = pty.slave.spawn_command(command_for(&shell, &cwd)).map_err(|e| format!("Couldn't start {}: {e}", shell.name))?;
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pty.master.take_writer().map_err(|e| e.to_string())?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let opened = Arc::new(Session { writer: Mutex::new(writer), master: Mutex::new(pty.master), killer: Mutex::new(child.clone_killer()) });
    SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(id, opened);

    let output = on_output.clone();
    std::thread::spawn(move || {
        let mut scanner = CwdScanner::default();
        let mut buf = vec![0u8; 64 * 1024];
        let mut first = true;
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let data = match session(id) {
                        Ok(session) => answer_opening(first, &buf[..n], &mut **session.writer.lock().unwrap_or_else(|e| e.into_inner())),
                        Err(_) => &buf[..n],
                    };
                    first = false;
                    for path in scanner.feed(data) {
                        let _ = output.send(InvokeResponseBody::Json(serde_json::to_string(&TermEvent::Cwd { path }).unwrap_or_default()));
                    }
                    if !data.is_empty() && output.send(InvokeResponseBody::Raw(data.to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
    std::thread::spawn(move || {
        let code = child.wait().map(|status| status.exit_code()).unwrap_or(1);
        // Closing the pseudo console ends the reader too.
        SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        let _ = on_output.send(InvokeResponseBody::Json(serde_json::to_string(&TermEvent::Exit { code }).unwrap_or_default()));
    });
    Ok(id)
}

/// What was typed (or pasted) into a session.
#[tauri::command]
pub fn term_write(id: u32, data: String) -> Result<(), String> {
    let session = session(id)?;
    let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());
    writer.write_all(data.as_bytes()).and_then(|_| writer.flush()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn term_resize(id: u32, cols: u16, rows: u16) -> Result<(), String> {
    let session = session(id)?;
    let master = session.master.lock().unwrap_or_else(|e| e.into_inner());
    master.resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())
}

/// Ends a session's shell (and what it runs).
#[tauri::command]
pub fn term_close(id: u32) {
    if let Some(session) = SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        let _ = session.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

/// Ends every session: as the app closes.
pub fn close_all() {
    let sessions: Vec<_> = SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, s)| s).collect();
    for session in sessions {
        let _ = session.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_where_a_prompt_says_it_is() {
        let mut scanner = CwdScanner::default();
        // Windows Terminal's OSC 9;9 with BEL, from PowerShell; cmd's ends with ESC \.
        assert_eq!(scanner.feed(b"before\x1b]9;9;\"C:\\Users\\me\"\x07PS C:\\Users\\me> "), ["C:\\Users\\me"]);
        assert_eq!(scanner.feed(b"\x1b]9;9;D:\\Work\x1b\\D:\\Work>"), ["D:\\Work"]);
        // OSC 7, as Nushell says it, split across two reads, with an escaped space.
        assert!(scanner.feed(b"\x1b]7;file://PC/C:/My%20Files").is_empty());
        assert_eq!(scanner.feed(b"/x\x1b\\"), ["C:\\My Files\\x"]);
        // A title (OSC 0) and colors aren't folders.
        assert!(scanner.feed(b"\x1b]0;Windows PowerShell\x07\x1b[32mgreen\x1b[0m").is_empty());
        assert_eq!(cwd_of(b"7;file:///C:/a/b"), Some("C:\\a\\b".into()));
        // WSL's: the drives it mounts are Windows folders; its own aren't.
        assert_eq!(cwd_of(b"7;file://ubuntu/mnt/c/Users/me"), Some("C:\\Users\\me".into()));
        assert_eq!(cwd_of(b"7;file://ubuntu/mnt/d"), Some("D:\\".into()));
        assert_eq!(cwd_of(b"7;file://ubuntu/home/me"), None);
        assert_eq!(cwd_of(b"7;file://ubuntu/mnt/wsl"), None);
    }

    #[test]
    fn finds_windows_own_shells() {
        let found = shells();
        assert!(found.iter().any(|s| s.id == "cmd"), "{found:?}");
        assert!(found.iter().any(|s| s.id == "powershell"), "{found:?}");
    }

    #[test]
    fn runs_a_shell_in_a_pseudo_console() {
        let cmd = shells().into_iter().find(|s| s.id == "cmd").unwrap();
        let dir = std::env::temp_dir();
        let pty = native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut child = pty.slave.spawn_command(command_for(&cmd, &dir.to_string_lossy())).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let mut writer = pty.master.take_writer().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        // Answered as the pane answers it; then a command once cmd is at its prompt.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let (mut all, mut first, mut typed) = (Vec::new(), true, false);
        while !String::from_utf8_lossy(&all).contains("glint-works") {
            let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(100)) else {
                assert!(std::time::Instant::now() < deadline, "cmd didn't answer in 20 s: {:?}", String::from_utf8_lossy(&all));
                continue;
            };
            all.extend_from_slice(answer_opening(first, &chunk, &mut writer));
            first = false;
            if !typed && String::from_utf8_lossy(&all).contains('>') {
                writer.write_all(b"echo glint-%COMPUTERNAME:~0,0%works\r\n").unwrap();
                typed = true;
            }
        }
        let _ = child.kill();
        let text = String::from_utf8_lossy(&all);
        assert!(!text.contains("\x1b[6n"), "the opening question isn't shown");
        assert!(text.contains("glint-works"), "{text}");
        // Its prompt said where it is.
        let mut scanner = CwdScanner::default();
        let said = scanner.feed(&all);
        assert!(said.iter().any(|p| p.trim_end_matches('\\').eq_ignore_ascii_case(dir.to_string_lossy().trim_end_matches('\\'))), "{said:?} in {text}");
    }
}
