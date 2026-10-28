//! Folders handed to the app from outside: on its command line (File Explorer's "Open in Glint", a
//! double-click on a folder once folders open here by default, a terminal), and from later launches,
//! which give theirs to the copy that's already running instead of opening a second window.
//! Whatever the app can't open (This PC, the Recycle Bin, shell: and ::{CLSID} locations, a share
//! that keeps the user out, a drive with no disc) goes to File Explorer instead, so it opens as it
//! always did.

use crate::fsx;
use serde::Serialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, Manager, State};

/// A folder to open in a new tab, and the file to select in it when a file was given.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Target {
    pub path: String,
    pub select: Option<String>,
}

/// What a command line asks for: folders to open here, and what File Explorer should open instead.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub open: Vec<Target>,
    pub explorer: Vec<String>,
}

impl Request {
    /// This process's command line.
    pub fn from_env() -> Request {
        let cwd = std::env::current_dir().unwrap_or_default();
        Request::parse(std::env::args_os().skip(1).map(|arg| arg.to_string_lossy().into_owned()), &cwd)
    }

    /// Sorts the arguments (without the program's own path) out. Relative paths are relative to
    /// `cwd`; options (anything starting with "-") are left out, as the app has none.
    pub fn parse(args: impl IntoIterator<Item = String>, cwd: &Path) -> Request {
        let mut request = Request::default();
        for arg in args {
            let arg = arg.trim();
            if arg.is_empty() || arg.starts_with('-') {
                continue;
            }
            match target(arg, cwd) {
                Ok(target) => request.open.push(target),
                Err(other) => request.explorer.push(other),
            }
        }
        request
    }
}

/// What the app opens for one argument, or else what File Explorer should get.
fn target(arg: &str, cwd: &Path) -> Result<Target, String> {
    // File Explorer passes a drive as "C:\" in quotes, which the command line reads as C:" since a
    // backslash before a quote escapes it.
    let arg = arg.trim_end_matches('"');
    if is_shell_location(arg) {
        return Err(arg.to_string());
    }
    let text = if Path::new(arg).is_absolute() || arg.starts_with(['~', '%']) { arg.to_string() } else { cwd.join(arg).to_string_lossy().into_owned() };
    let Ok(path) = fsx::normalize(&text) else { return Err(arg.to_string()) };
    let Some(info) = fsx::stat(&path) else { return Err(path) };
    if info.dir {
        return if opens_here(&path) { Ok(Target { path, select: None }) } else { Err(path) };
    }
    // A zip or 7z archive opens like a folder (when it's opened with Glint).
    if crate::archive::is_archive_name(&path) {
        return Ok(Target { path, select: None });
    }
    // A file: its folder, with the file selected.
    let folder = Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    if !folder.is_empty() && opens_here(&folder) {
        Ok(Target { path: fsx::normalize(&folder).unwrap_or(folder), select: Some(path) })
    } else {
        Err(path)
    }
}

/// Whether the app opens a folder: one it can list, or one on this PC that Windows keeps this user
/// out of, which its tab says, offering to get access. A share that keeps them out is File Explorer's,
/// which can ask for another user name and password.
fn opens_here(dir: &str) -> bool {
    match fsx::probe(dir) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied && !fsx::on_other_computer(dir),
    }
}

/// shell:Downloads, ::{CLSID} (This PC, Control Panel…), search-ms:, file: and other URLs: File Explorer's.
fn is_shell_location(arg: &str) -> bool {
    if arg.starts_with("::") {
        return true;
    }
    // A scheme is two or more letters, digits, "+", "-" or "." before a colon; "C:" is a drive.
    arg.find(':').is_some_and(|colon| colon > 1 && arg[..colon].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
}

/// Opens what the app can't in File Explorer, with its own "open" (which stays File Explorer's even
/// when folders open here by default, so this can't come back here).
#[cfg(windows)]
pub fn hand_to_explorer(items: &[String]) {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // File Explorer's "open" is a COM verb.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    for item in items {
        let file = HSTRING::from(item.as_str());
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            // Finished before it returns, since this process may end right after.
            fMask: SEE_MASK_NOASYNC,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let _ = unsafe { ShellExecuteExW(&mut info) };
    }
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
}

#[cfg(not(windows))]
pub fn hand_to_explorer(_items: &[String]) {}

/// Folders waiting for the window to open them: the command line's, then each later launch's.
#[derive(Default)]
pub struct Pending(Mutex<Vec<Target>>);

impl Pending {
    pub fn new(targets: Vec<Target>) -> Pending {
        Pending(Mutex::new(targets))
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Target>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One more folder to open, in a new tab (the UI takes it on the next "open-requested").
    pub fn push(&self, target: Target) {
        self.lock().push(target);
    }
}

/// The folders waiting to open, each in a new tab. The UI asks at start, and again on every
/// "open-requested" event.
#[tauri::command]
pub fn take_launch_targets(pending: State<Pending>) -> Vec<Target> {
    std::mem::take(&mut *pending.lock())
}

/// A later launch handed over its command line (the single-instance plugin calls this on the UI
/// thread): the window comes to the front and opens its folders.
#[cfg_attr(debug_assertions, allow(dead_code))]
pub fn second_launch(app: &AppHandle, args: Vec<String>, cwd: String) {
    bring_to_front(app);
    let app = app.clone();
    // Off the UI thread: looking at a folder on a sleeping network drive can take seconds.
    std::thread::spawn(move || {
        // That launch already gave File Explorer what the app can't open.
        let open = Request::parse(args.into_iter().skip(1), Path::new(&cwd)).open;
        if !open.is_empty() {
            app.state::<Pending>().lock().extend(open);
            let _ = app.emit("open-requested", ());
        }
    });
}

pub(crate) fn bring_to_front(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let _ = window.unminimize();
    crate::placement::show(&window);
    bring_forward(&window);
}

/// SetForegroundWindow rather than window.set_focus(), which presses Alt when Windows says no. What
/// asked for it (a launch handing over its folders, the shortcut, the tray) allowed this process to come forward.
pub(crate) fn bring_forward(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        let _ = unsafe { SetForegroundWindow(HWND(hwnd.0 as _)) };
    }
    #[cfg(not(windows))]
    let _ = window.set_focus();
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    /// A folder of the test's own: tests run in parallel, and each deletes its folder when done.
    fn scratch(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-launch-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub folder")).unwrap();
        fs::write(dir.join("sub folder").join("notes.txt"), "hi").unwrap();
        dir
    }

    fn open(path: &Path) -> Target {
        Target { path: path.to_string_lossy().into_owned(), select: None }
    }

    #[test]
    fn folders_files_and_drives() {
        let dir = scratch("folders");
        let sub = dir.join("sub folder");
        let file = sub.join("notes.txt");
        let parse = |args: &[&str]| Request::parse(args.iter().map(|a| a.to_string()), &dir);

        assert_eq!(parse(&[&sub.to_string_lossy()]).open, [open(&sub)]);
        // Relative to the folder the launch ran in, with any trailing separator dropped.
        assert_eq!(parse(&["sub folder\\"]).open, [open(&sub)]);
        assert_eq!(parse(&["."]).open, [open(&dir)]);
        // A file opens its folder with it selected.
        let request = parse(&[&file.to_string_lossy()]);
        assert_eq!(request.open, [Target { path: sub.to_string_lossy().into_owned(), select: Some(file.to_string_lossy().into_owned()) }]);
        // "C:\" in quotes arrives as C:" (and "C:" alone means the drive).
        let drive = std::env::var("SystemDrive").unwrap();
        assert_eq!(parse(&[&format!("{drive}\"")]).open, [Target { path: format!("{drive}\\"), select: None }]);
        assert_eq!(parse(&[&drive]).open, [Target { path: format!("{drive}\\"), select: None }]);
        // Options are left out.
        assert_eq!(parse(&["--flag", "-x", ""]), Request::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_app_cannot_open_goes_to_file_explorer() {
        let dir = scratch("explorer");
        let parse = |arg: &str| Request::parse([arg.to_string()], &dir);
        for place in [
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "shell:RecycleBinFolder",
            "shell:::{26EE0668-A00A-44D7-9371-BEB064C98683}",
            "SHELL:Libraries",
            "search-ms:query=report",
            "file:///C:/Windows",
        ] {
            assert_eq!(parse(place), Request { open: vec![], explorer: vec![place.to_string()] }, "{place}");
        }
        // Missing: File Explorer says so, as it would have.
        let missing = dir.join("missing");
        assert_eq!(parse("missing").explorer, [missing.to_string_lossy().into_owned()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_windows_keeps_the_user_out_of_open_here() {
        // Where Windows has one: its tab says why it can't be listed, and offers to get access.
        let denied = r"C:\System Volume Information";
        if Path::new(denied).exists() && fs::read_dir(denied).is_err() {
            assert_eq!(Request::parse([denied.to_string()], Path::new("C:\\")).open, [Target { path: denied.into(), select: None }]);
        }
    }

    #[test]
    fn shell_locations() {
        assert!(is_shell_location("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
        assert!(is_shell_location("shell:Downloads"));
        assert!(is_shell_location("ms-settings:storagesense"));
        assert!(!is_shell_location(r"C:\Windows"));
        assert!(!is_shell_location("C:"));
        assert!(!is_shell_location(r"\\server\share"));
        assert!(!is_shell_location(r"\\?\C:\Windows"));
        assert!(!is_shell_location(r"relative\path"));
    }
}
