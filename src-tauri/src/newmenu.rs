//! Windows' New menu: the kinds of file File Explorer offers to make in a folder (Word, Excel and
//! PowerPoint documents, a zip, Google Drive's documents…), as the PC's apps registered them under a
//! `ShellNew` key, and making one as File Explorer does: a copy of the app's template, an empty file,
//! the bytes the app gave, or the app's own command. Kinds a COM handler makes (shortcuts, libraries)
//! are left out, and so are folders and text documents, which Glint makes itself.

use crate::fsx::blocking;
use serde::Serialize;

/// A kind of file the New menu makes.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewKind {
    /// Its extension with the dot (".docx"): what `new_file` is asked for.
    pub ext: String,
    /// What the menu says: "Microsoft Word Document".
    pub name: String,
    /// Its icon as File Explorer shows it, as a PNG's data URL.
    pub icon: Option<String>,
    /// Its app makes it (with a command of its own), and may open a window to: not renamed right away.
    pub by_app: bool,
}

/// The kinds File Explorer's New menu has on this PC, in its order.
#[tauri::command]
pub async fn new_kinds() -> Result<Vec<NewKind>, String> {
    blocking(|| crate::shell::in_sta(|| Ok(imp::kinds()))).await
}

/// Makes a file of the kind with extension `ext` in `folder`, named as File Explorer names it ("New
/// Microsoft Word Document.docx", numbered when that's taken): its path. One its app makes is waited
/// for a few seconds: None when it hasn't appeared by then.
#[tauri::command]
pub async fn new_file(folder: String, ext: String) -> Result<Option<String>, String> {
    blocking(move || imp::make(&folder, &ext)).await
}

/// "&Shortcut" → "Shortcut": a menu's text without its access key.
fn without_access_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                out.push('&');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `%NAME%` in `text` as this account has it (left as it is when it isn't set).
fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out + rest
}

/// A command with `%1` (or `%L`) as the new file's path, as File Explorer runs it.
fn with_path(command: &str, path: &str) -> String {
    let command = expand(command);
    if command.contains("%1") || command.contains("%L") || command.contains("%l") {
        command.replace("%1", path).replace("%L", path).replace("%l", path)
    } else {
        format!("{command} \"{path}\"")
    }
}

#[cfg(windows)]
mod imp {
    use super::{with_path, without_access_key, NewKind};
    use base64::Engine;
    use std::collections::HashSet;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ};
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, REG_BINARY, REG_VALUE_TYPE, RRF_RT_ANY};
    use windows::Win32::System::Threading::{CreateProcessW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW};
    use windows::Win32::UI::Shell::{AssocQueryStringW, SHGetFileInfoW, SHGetKnownFolderPath, SHLoadIndirectString, ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME, FOLDERID_CommonTemplates, FOLDERID_Templates, KF_FLAG_DEFAULT, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    /// What File Explorer keeps of its New menu after reading every file type, in its order.
    const EXPLORER_LIST: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Discardable\PostSetup\ShellNew";

    /// A value as it's stored (strings with their environment variables filled in): its type and bytes.
    fn value(root: HKEY, key: &str, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        if unsafe { RegGetValueW(root, &key, &name, RRF_RT_ANY, Some(&mut kind), None, Some(&mut size)) } != ERROR_SUCCESS {
            return None;
        }
        for _ in 0..3 {
            let mut data = vec![0u8; size as usize + 2];
            size = data.len() as u32;
            match unsafe { RegGetValueW(root, &key, &name, RRF_RT_ANY, Some(&mut kind), Some(data.as_mut_ptr().cast()), Some(&mut size)) } {
                ERROR_SUCCESS => {
                    data.truncate(size as usize);
                    return Some((kind, data));
                }
                ERROR_MORE_DATA => continue,
                _ => return None,
            }
        }
        None
    }

    fn utf16(data: &[u8]) -> Vec<u16> {
        data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect()
    }

    fn text(data: &[u8]) -> String {
        let units = utf16(data);
        String::from_utf16_lossy(&units[..units.iter().position(|&u| u == 0).unwrap_or(units.len())])
    }

    fn string(root: HKEY, key: &str, name: &str) -> Option<String> {
        value(root, key, name).map(|(_, data)| text(&data))
    }

    fn exists(root: HKEY, key: &str) -> bool {
        let mut opened = HKEY::default();
        let found = unsafe { RegOpenKeyExW(root, &HSTRING::from(key), None, KEY_READ, &mut opened) } == ERROR_SUCCESS;
        if found {
            let _ = unsafe { RegCloseKey(opened) };
        }
        found
    }

    /// A string such as "@shell32.dll,-30397" as it reads ("New shortcut"); plain text as it is.
    fn indirect(text: &str) -> Option<String> {
        if !text.starts_with('@') {
            return Some(text.to_string());
        }
        let mut out = [0u16; 512];
        unsafe { SHLoadIndirectString(&HSTRING::from(text), &mut out, None) }.ok()?;
        Some(String::from_utf16_lossy(&out[..out.iter().position(|&u| u == 0).unwrap_or(0)]))
    }

    /// A file type's name, as File Explorer's Type column has it ("Microsoft Word Document").
    fn friendly(ext: &str) -> Option<String> {
        let mut out = [0u16; 512];
        let mut len = out.len() as u32;
        unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME, &HSTRING::from(ext), PCWSTR::null(), Some(PWSTR(out.as_mut_ptr())), &mut len) }.ok().ok()?;
        let name = String::from_utf16_lossy(&out[..out.iter().position(|&u| u == 0).unwrap_or(0)]);
        (!name.is_empty()).then_some(name)
    }

    /// Where a file type's New menu entry is: under the type its extension names (`.docx\Word.Document.12\ShellNew`), or the extension's own.
    fn shell_new(ext: &str) -> Option<String> {
        let progid = string(HKEY_CLASSES_ROOT, ext, "").filter(|p| !p.is_empty() && !p.contains('\\'));
        progid.map(|p| format!(r"{ext}\{p}\ShellNew")).into_iter().chain([format!(r"{ext}\ShellNew")]).find(|key| exists(HKEY_CLASSES_ROOT, key))
    }

    /// The extensions File Explorer's New menu lists: its own list, or (before File Explorer has made
    /// one) every file type with a ShellNew key.
    fn listed() -> Vec<String> {
        if let Some((_, data)) = value(HKEY_CURRENT_USER, EXPLORER_LIST, "Classes") {
            let names: Vec<String> = utf16(&data).split(|&u| u == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
            if !names.is_empty() {
                return names;
            }
        }
        let mut found = Vec::new();
        let mut name = [0u16; 256];
        for i in 0.. {
            let mut len = name.len() as u32;
            match unsafe { RegEnumKeyExW(HKEY_CLASSES_ROOT, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) } {
                ERROR_SUCCESS => {
                    let ext = String::from_utf16_lossy(&name[..len as usize]);
                    if ext.starts_with('.') && shell_new(&ext).is_some() {
                        found.push(ext);
                    }
                }
                ERROR_MORE_DATA => continue,
                _ => break,
            }
        }
        found
    }

    /// How a kind is made.
    enum How {
        Empty,
        Copy(PathBuf),
        Bytes(Vec<u8>),
        Run(String),
    }

    struct Entry {
        name: String,
        /// The new file's name, without its extension: "New Microsoft Word Document".
        item: String,
        how: How,
    }

    /// A template named in a ShellNew key: as it is when it's a full path, else in this account's
    /// Templates folder, all accounts', or Windows' ShellNew folder.
    fn template(file: &str) -> Option<PathBuf> {
        let named = Path::new(file);
        if named.is_absolute() {
            return named.is_file().then(|| named.to_path_buf());
        }
        let known = |id| unsafe {
            let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let text = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as *const _));
            text.map(PathBuf::from)
        };
        let windows = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("ShellNew"));
        [known(&FOLDERID_Templates), known(&FOLDERID_CommonTemplates), windows].into_iter().flatten().map(|dir| dir.join(named)).find(|p| p.is_file())
    }

    /// What a file type's ShellNew key says; None for a kind a COM handler makes, or one whose template is gone.
    fn entry(ext: &str) -> Option<Entry> {
        let key = shell_new(ext)?;
        let get = |name: &str| value(HKEY_CLASSES_ROOT, &key, name);
        if get("Handler").is_some() {
            return None;
        }
        let how = if let Some(command) = string(HKEY_CLASSES_ROOT, &key, "Command").filter(|c| !c.trim().is_empty()) {
            How::Run(command)
        } else if let Some((kind, data)) = get("Data") {
            How::Bytes(if kind == REG_BINARY { data } else { text(&data).into_bytes() })
        } else if let Some(file) = string(HKEY_CLASSES_ROOT, &key, "FileName").filter(|f| !f.is_empty()) {
            How::Copy(template(&file)?)
        } else if get("NullFile").is_some() {
            How::Empty
        } else {
            return None;
        };
        let read = |name: &str| string(HKEY_CLASSES_ROOT, &key, name).and_then(|t| indirect(&t)).map(|t| without_access_key(&t)).filter(|t| !t.trim().is_empty());
        let friendly = friendly(ext);
        let name = read("MenuText").or_else(|| friendly.clone())?;
        let item = read("ItemName").or_else(|| friendly.map(|f| format!("New {f}")))?;
        Some(Entry { name, item, how })
    }

    /// A file type's icon, as File Explorer shows it, as a PNG's data URL.
    fn icon(ext: &str) -> Option<String> {
        let mut info = SHFILEINFOW::default();
        let found = unsafe { SHGetFileInfoW(&HSTRING::from(ext), FILE_ATTRIBUTE_NORMAL, Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES) };
        if found == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let png = unsafe { icon_png(info.hIcon) };
        let _ = unsafe { DestroyIcon(info.hIcon) };
        png.map(|png| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
    }

    /// A bitmap's pixels, top-down 32-bit BGRA.
    unsafe fn pixels(bitmap: HBITMAP, w: usize, h: usize) -> Option<Vec<u8>> {
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; w * h * 4];
        let dc = CreateCompatibleDC(None);
        let got = GetDIBits(dc, bitmap, 0, h as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        (got != 0).then_some(bgra)
    }

    /// An icon as a PNG: its colors with their alpha, or (an old icon without one) its mask's.
    unsafe fn icon_png(icon: HICON) -> Option<Vec<u8>> {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let png = (|| {
            let mut bm = BITMAP::default();
            if GetObjectW(HGDIOBJ(info.hbmColor.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
                return None;
            }
            let (w, h) = (bm.bmWidth.unsigned_abs() as usize, bm.bmHeight.unsigned_abs() as usize);
            if w == 0 || h == 0 || w > 256 || h > 256 {
                return None;
            }
            let bgra = pixels(info.hbmColor, w, h)?;
            let mask = bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0).then(|| pixels(info.hbmMask, w, h)).flatten();
            let mut rgba = Vec::with_capacity(w * h * 4);
            for (i, p) in bgra.as_chunks::<4>().0.iter().enumerate() {
                let alpha = match &mask {
                    Some(mask) => if mask[i * 4] == 0 { 255 } else { 0 },
                    None => p[3],
                };
                rgba.extend([p[2], p[1], p[0], alpha]);
            }
            let mut png = Vec::new();
            let mut encoder = png::Encoder::new(&mut png, w as u32, h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().ok()?.write_image_data(&rgba).ok()?;
            Some(png)
        })();
        let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        png
    }

    pub fn kinds() -> Vec<NewKind> {
        let mut names = HashSet::new();
        listed()
            .into_iter()
            .filter(|ext| ext.starts_with('.') && !ext.eq_ignore_ascii_case(".txt"))
            .filter_map(|ext| {
                let entry = entry(&ext)?;
                // Two types of one app's (Access's .accdb and .mdb) can share a name: the first is kept.
                names.insert(entry.name.to_lowercase()).then(|| NewKind { icon: icon(&ext), by_app: matches!(entry.how, How::Run(_)), ext, name: entry.name })
            })
            .collect()
    }

    pub fn make(folder: &str, ext: &str) -> Result<Option<String>, String> {
        let entry = entry(ext).ok_or("Windows doesn't make this kind of file any more")?;
        if crate::archive::is_read_only(folder) {
            return Err(crate::fsx::READ_ONLY.into());
        }
        let name = format!("{}{ext}", entry.item.trim());
        if name.contains(['\\', '/', ':']) {
            return Err("That name isn't allowed".into());
        }
        match entry.how {
            How::Empty => crate::fsx::create_numbered(folder, &name, false).map(Some),
            How::Copy(template) => written(folder, &name, |file| std::io::copy(&mut std::fs::File::open(&template)?, file).map(|_| ())),
            How::Bytes(bytes) => written(folder, &name, |file| file.write_all(&bytes)),
            How::Run(command) => {
                let path = (1..10_000).map(|n| Path::new(folder).join(crate::fsx::numbered(&name, false, n))).find(|p| !p.exists()).ok_or("Couldn't find a free name")?;
                let path = path.to_string_lossy().into_owned();
                run(&with_path(&command, &path), folder)?;
                // Its app makes it, in its own time.
                let until = Instant::now() + Duration::from_secs(4);
                while Instant::now() < until {
                    if Path::new(&path).exists() {
                        return Ok(Some(path));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(None)
            }
        }
    }

    /// A new file named `name` in `folder` (numbered when that's taken), filled by `fill`: its path. It
    /// goes again if filling it fails.
    fn written(folder: &str, name: &str, fill: impl Fn(&mut std::fs::File) -> std::io::Result<()>) -> Result<Option<String>, String> {
        for n in 1..10_000 {
            let path = Path::new(folder).join(crate::fsx::numbered(name, false, n));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let filled = fill(&mut file);
                    drop(file);
                    return match filled {
                        Ok(()) => Ok(Some(path.to_string_lossy().into_owned())),
                        Err(e) => {
                            let _ = std::fs::remove_file(&path);
                            Err(e.to_string())
                        }
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Couldn't find a free name".into())
    }

    /// Runs a command line as File Explorer would, in `folder`, without waiting for it.
    fn run(command: &str, folder: &str) -> Result<(), String> {
        let mut line: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
        let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut process = PROCESS_INFORMATION::default();
        unsafe { CreateProcessW(PCWSTR::null(), Some(PWSTR(line.as_mut_ptr())), None, None, false, CREATE_UNICODE_ENVIRONMENT, None, &HSTRING::from(folder), &startup, &mut process) }
            .map_err(|e| format!("Its app didn't start: {}", e.message()))?;
        unsafe {
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::NewKind;

    pub fn kinds() -> Vec<NewKind> {
        Vec::new()
    }

    pub fn make(_folder: &str, _ext: &str) -> Result<Option<String>, String> {
        Err("Only on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_menus_and_commands_as_explorer_does() {
        assert_eq!(without_access_key("&Shortcut"), "Shortcut");
        assert_eq!(without_access_key("Fish && &Chips"), "Fish & Chips");
        std::env::set_var("GLINT_NEWMENU_TEST", r"C:\Apps");
        assert_eq!(expand(r"%GLINT_NEWMENU_TEST%\app.exe %1 %NOT_SET_HERE%"), r"C:\Apps\app.exe %1 %NOT_SET_HERE%");
        assert_eq!(with_path(r#""%GLINT_NEWMENU_TEST%\a.exe" --new="%1""#, r"D:\x\New.gdoc"), r#""C:\Apps\a.exe" --new="D:\x\New.gdoc""#);
        assert_eq!(with_path("app.exe /new", r"D:\x\New.db"), r#"app.exe /new "D:\x\New.db""#);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads this PC's registry"]
    fn lists_this_pcs_kinds() {
        let kinds = crate::shell::in_sta(|| Ok(imp::kinds())).unwrap();
        for kind in &kinds {
            println!("{} {} {} icon {}", kind.ext, kind.name, kind.by_app, kind.icon.as_ref().map_or(0, |i| i.len()));
        }
        assert!(kinds.iter().all(|k| k.ext.starts_with('.') && !k.name.is_empty()));
    }
}
