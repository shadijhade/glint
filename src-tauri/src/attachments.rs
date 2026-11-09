//! Opening an email's attachment: the preview reads .eml and .msg files in the page, and an attachment
//! asked for comes here as raw bytes, saved into a folder of its own under the temp folder, marked as
//! from the internet (as Outlook and browsers mark theirs, so Windows warns before a program or a
//! document's macros run), for open_item to open with its app.

use std::fs;
use std::path::PathBuf;

#[tauri::command]
pub async fn attachment_save(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return Err("No attachment came".into()) };
    let name = request.headers().get("name").and_then(|v| v.to_str().ok()).map(percent_decoded).unwrap_or_default();
    let path = save(&safe_name(&name), bytes)?;
    Ok(path.to_string_lossy().into_owned())
}

fn save(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join("Glint attachments").join(format!("{stamp:x}"));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    // The internet zone's mark (Zone.Identifier), as an attachment saved by a mail app gets.
    let mut marked = path.clone().into_os_string();
    marked.push(":Zone.Identifier");
    let _ = fs::write(PathBuf::from(marked), "[ZoneTransfer]\r\nZoneId=3\r\n");
    Ok(path)
}

/// "%D8%B1%D8%AD%D9%84%D8%A9.pdf" as the name it was (the page sends names percent-encoded: headers are ASCII).
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A name Windows takes: no folders in it, no characters it refuses, not empty.
fn safe_name(name: &str) -> String {
    let leaf = name.rsplit(['\\', '/']).next().unwrap_or_default();
    let cleaned: String = leaf.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "attachment".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_names() {
        assert_eq!(percent_decoded("%D8%B1%D8%AD%D9%84%D8%A9.pdf"), "رحلة.pdf");
        assert_eq!(percent_decoded("report%20(final).docx"), "report (final).docx");
        assert_eq!(percent_decoded("100%"), "100%");
    }

    #[test]
    fn keeps_names_inside_their_folder() {
        assert_eq!(safe_name(r"..\..\Windows\evil.exe"), "evil.exe");
        assert_eq!(safe_name("a/b/c:d?.txt"), "c_d_.txt");
        assert_eq!(safe_name(" . "), "attachment");
        assert_eq!(safe_name("notes.txt."), "notes.txt");
    }

    #[test]
    fn saves_and_marks() {
        let path = save("hello.txt", b"hi").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hi");
        let mut zone = path.clone().into_os_string();
        zone.push(":Zone.Identifier");
        assert!(fs::read_to_string(PathBuf::from(zone)).unwrap().contains("ZoneId=3"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
