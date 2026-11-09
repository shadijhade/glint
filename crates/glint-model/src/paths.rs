//! Ported from src/lib/paths.ts.
//!
//! Glint's path rules: roots, parents, crumbs, remote and archive paths, This PC, the Recycle Bin and
//! shadow copies. Paths are plain strings with Windows rules (either separator, case-insensitive).
//!
//! NOTE(port): the TypeScript used regular expressions. They are written out by hand here so that the
//! JavaScript details stay exact: `\d` is ASCII only, `i` folds ASCII only, and `.` does not match a
//! line break.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The "This PC" view (drives and known folders) has no real path.
pub const THIS_PC: &str = "";

/// The Recycle Bin, by File Explorer's name for it: a place, not a folder, whose items each have a path
/// of their own (recycle.rs).
pub const RECYCLE_BIN: &str = "shell:RecycleBinFolder";
/// Whether `path` is the Recycle Bin (any case).
pub fn is_recycle_bin(path: &str) -> bool {
    path.eq_ignore_ascii_case("shell:recyclebinfolder")
}

/// Recent, by File Explorer's name for Windows' Recent Items: the files opened lately, each at its own
/// path (recent.rs).
pub const RECENT: &str = "shell:Recent";
/// Whether `path` is Recent (any case).
pub fn is_recent(path: &str) -> bool {
    path.eq_ignore_ascii_case("shell:recent")
}

/// Gallery: the photos and videos in the gallery's folders, newest first by when each was taken
/// (gallery.rs).
pub const GALLERY: &str = "shell:Gallery";
/// Whether `path` is the Gallery (any case).
pub fn is_gallery(path: &str) -> bool {
    path.eq_ignore_ascii_case("shell:gallery")
}

/// A remote connection's folders (remote/): `remote:<id>` and below, read-only, their files downloaded
/// to be read.
pub const REMOTE: &str = "remote:";

/// What follows `REMOTE` at the start of `path` (any case), if it starts with it.
fn after_remote(path: &str) -> Option<&str> {
    let head = path.as_bytes().get(..REMOTE.len())?;
    if head.eq_ignore_ascii_case(REMOTE.as_bytes()) {
        // The first 7 bytes are ASCII, so this is a char boundary.
        path.get(REMOTE.len()..)
    } else {
        None
    }
}

/// Whether `path` is a remote connection's (starts with `remote:`, any case).
pub fn is_remote_path(path: &str) -> bool {
    after_remote(path).is_some()
}

/// The id of the connection a remote path is on ("" for any other path).
pub fn remote_id_of(path: &str) -> &str {
    after_remote(path)
        .and_then(|rest| rest.split(['\\', '/']).next())
        .unwrap_or("")
}

/// The start of a shadow copy's device name, as Windows spells it (compared in any case).
const SHADOW_PREFIX: &str = "\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy";

/// The byte length of `SHADOW_PREFIX` plus the digits after it, and those digits ("" when there are none).
fn shadow_digits(path: &str) -> Option<(usize, &str)> {
    let head = path.as_bytes().get(..SHADOW_PREFIX.len())?;
    if !head.eq_ignore_ascii_case(SHADOW_PREFIX.as_bytes()) {
        return None;
    }
    // The prefix is ASCII, so this is a char boundary.
    let rest = path.get(SHADOW_PREFIX.len()..)?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    Some((SHADOW_PREFIX.len() + digits, rest.get(..digits)?))
}

/// A shadow copy's name and number followed by a backslash or the end: the byte length of that top part,
/// and the number.
fn shadow_match(path: &str) -> Option<(usize, &str)> {
    let (end, digits) = shadow_digits(path)?;
    match path.as_bytes().get(end) {
        None | Some(b'\\') => Some((end, digits)),
        _ => None,
    }
}

/// A shadow copy's top, with or without its closing backslash and nothing after it.
fn is_shadow_root(path: &str) -> bool {
    shadow_match(path).is_some_and(|(end, _)| {
        let rest = path.get(end..).unwrap_or("x");
        rest.is_empty() || rest == "\\"
    })
}

/// Whether `path` is in a shadow copy (Windows' earlier state of a drive: previous versions, versions.rs),
/// read-only.
pub fn is_shadow_path(path: &str) -> bool {
    shadow_match(path).is_some()
}

/// A shadow copy's top, with its closing backslash ("" for any other path).
pub fn shadow_root_of(path: &str) -> String {
    match shadow_match(path) {
        Some((end, _)) => format!("{}\\", path.get(..end).unwrap_or("")),
        None => String::new(),
    }
}

/// What shadow copies' tops are called ("C: on 2 Nov 2026, 14:20"), as Glint learned them. Keys are a
/// shadow top in lower case with its closing backslash.
fn shadow_names_map() -> &'static Mutex<HashMap<String, String>> {
    static NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    NAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Teaches Glint what a shadow copy's top is called: the TypeScript `shadowNames.set(top.toLowerCase(), name)`.
/// `top` is lower-cased here.
pub fn set_shadow_name(top: &str, name: &str) {
    let mut names = shadow_names_map().lock().unwrap_or_else(|e| e.into_inner());
    names.insert(top.to_lowercase(), name.to_string());
}

/// Forgets every learned shadow copy name (the TypeScript `shadowNames.clear()`).
pub fn clear_shadow_names() {
    shadow_names_map().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The name a shadow copy's top goes by: the one Glint learned, or "Shadow copy 7".
pub fn shadow_name(path: &str) -> String {
    let key = shadow_root_of(path).to_lowercase();
    let learned = shadow_names_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    learned.unwrap_or_else(|| {
        let number = shadow_match(path).map_or("", |(_, digits)| digits);
        format!("Shadow copy {number}").trim().to_string()
    })
}

/// A folder on this PC (on disk, or in an archive) rather than This PC, the Recycle Bin or a remote:
/// what's in it has its path, for other tools too.
pub fn is_folder_path(path: &str) -> bool {
    path != THIS_PC && !is_recycle_bin(path) && !is_recent(path) && !is_gallery(path) && !is_remote_path(path)
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// `path` without the separators at its end (the TypeScript `replace(/[\\/]+$/, "")`).
fn trim_separators(path: &str) -> &str {
    path.trim_end_matches(is_separator)
}

/// In an archive (or the archive itself), by its names: read-only, and not on disk for other tools.
pub fn in_archive_path(path: &str) -> bool {
    crate::archives::archive_in_path(path)
}

/// `X:` with one ASCII letter.
fn is_bare_drive(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// "C:\", "C:", "\\server\share", "\\server" (its shares) and "/" are roots, and so are the Recycle Bin,
/// Recent and a remote's top (This PC is above them).
pub fn is_root(path: &str) -> bool {
    // C:, C:\ or C:/
    let drive = path.strip_suffix(is_separator).unwrap_or(path);
    if is_bare_drive(drive) {
        return true;
    }
    if is_shadow_root(path) {
        return true;
    }
    // \\server, \\server\ , \\server\share and \\server\share\
    if let Some(rest) = path.strip_prefix("\\\\") {
        let parts: Vec<&str> = rest.split('\\').collect();
        let named = |s: &&str| !s.is_empty();
        let unc_root = match parts.as_slice() {
            [a] => named(a),
            [a, last] => named(a) && (last.is_empty() || named(last)),
            [a, b, ""] => named(a) && named(b),
            _ => false,
        };
        if unc_root {
            return true;
        }
    }
    if path == "/" || is_recycle_bin(path) || is_recent(path) || is_gallery(path) {
        return true;
    }
    // remote:<id> or remote:<id>\
    after_remote(path).is_some_and(|rest| {
        let id = rest.strip_suffix(is_separator).unwrap_or(rest);
        !id.is_empty() && !id.contains(is_separator)
    })
}

/// Joins a folder and a name with the folder's own kind of separator (a `/` folder keeps `/`).
pub fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with(is_separator) {
        return format!("{dir}{name}");
    }
    let sep = if dir.contains('/') && !dir.contains('\\') {
        "/"
    } else {
        "\\"
    };
    format!("{dir}{sep}{name}")
}

/// The last name in a path (the whole path when there is none).
pub fn base_name(path: &str) -> &str {
    let trimmed = trim_separators(path);
    // A separator is one byte, so the name starts one byte after it.
    let name = trimmed
        .rfind(is_separator)
        .map_or(trimmed, |cut| trimmed.get(cut + 1..).unwrap_or(""));
    if name.is_empty() {
        path
    } else {
        name
    }
}

/// `\\server\share` (with an optional closing backslash) gives `\\server`.
fn share_server(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("\\\\")?;
    let parts: Vec<&str> = rest.split('\\').collect();
    match parts.as_slice() {
        [a, b] | [a, b, ""] if !a.is_empty() && !b.is_empty() => path.get(..2 + a.len()),
        _ => None,
    }
}

/// "C:\a\b" gives "C:\a", "C:\a" gives "C:\", a share gives its server, and a root gives This PC.
pub fn parent_path(path: &str) -> String {
    if let Some(server) = share_server(path) {
        return server.to_string();
    }
    if path == THIS_PC || is_root(path) {
        return THIS_PC.to_string();
    }
    let trimmed = trim_separators(path);
    let Some(cut) = trimmed.rfind(is_separator) else {
        return THIS_PC.to_string();
    };
    let parent = trimmed.get(..cut).unwrap_or("");
    if is_bare_drive(parent) || is_shadow_root(parent) {
        return format!("{parent}\\");
    }
    if parent.is_empty() {
        "/".to_string()
    } else {
        parent.to_string()
    }
}

/// Same location, ignoring case and a trailing separator (Windows paths are case-insensitive).
pub fn same_path(a: &str, b: &str) -> bool {
    trim_separators(a).to_lowercase() == trim_separators(b).to_lowercase()
}

/// The drive ("C:") or share ("\\server\share") a path is on ("" for any other path).
pub fn volume_of(path: &str) -> &str {
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return path.get(..2).unwrap_or("");
    }
    if let Some((end, _)) = shadow_digits(path) {
        return path.get(..end).unwrap_or("");
    }
    if let Some(rest) = path.strip_prefix("\\\\") {
        let mut parts = rest.splitn(3, '\\');
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            if !server.is_empty() && !share.is_empty() {
                return path.get(..2 + server.len() + 1 + share.len()).unwrap_or("");
            }
        }
    }
    ""
}

/// Line breaks, which JavaScript's `.` does not match.
fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// A URL without its query or fragment (the TypeScript `replace(/[?#].*$/, "")`): the cut is the first `?`
/// or `#` after the last line break, since `.*` cannot cross one.
fn without_query(url: &str) -> &str {
    let from = url
        .char_indices()
        .rfind(|(_, c)| is_line_break(*c))
        .map_or(0, |(at, c)| at + c.len_utf8());
    match url.get(from..).and_then(|tail| tail.find(['?', '#'])) {
        Some(at) => url.get(..from + at).unwrap_or(url),
        None => url,
    }
}

fn hex_value(b: u8) -> Option<u8> {
    char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

/// The byte a `%XX` at `bytes[at..]` stands for.
fn percent_byte(bytes: &[u8], at: usize) -> Option<u8> {
    if bytes.get(at) != Some(&b'%') {
        return None;
    }
    let high = hex_value(*bytes.get(at + 1)?)?;
    let low = hex_value(*bytes.get(at + 2)?)?;
    Some(high * 16 + low)
}

/// JavaScript's `decodeURIComponent`: `None` where it would throw (a stray `%`, or bytes that are not UTF-8).
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let first = percent_byte(bytes, i)?;
        let count: usize = match first {
            0x00..=0x7F => 1,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return None,
        };
        let mut sequence = vec![first];
        for k in 1..count {
            let next = percent_byte(bytes, i + 3 * k)?;
            if next & 0xC0 != 0x80 {
                return None;
            }
            sequence.push(next);
        }
        // Rejects overlong forms, surrogates and values past U+10FFFF, as JavaScript does.
        std::str::from_utf8(&sequence).ok()?;
        out.extend_from_slice(&sequence);
        i += 3 * count;
    }
    String::from_utf8(out).ok()
}

/// `scheme:` at the start (a letter, then letters, digits, `+`, `.` or `-`).
fn has_scheme(path: &str) -> bool {
    let b = path.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let run = b
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
        .count();
    b.get(run) == Some(&b':')
}

/// The file a link in a document names (a Markdown link or picture): its URL's path, relative to the
/// document's `folder`, or to `root` (its repository's, or its drive's) when it starts with / as on
/// GitHub; or a path like C:\... None for a web address or another scheme.
pub fn linked_path(url: &str, folder: &str, root: &str) -> Option<String> {
    let mut path = without_query(url).to_string();
    if path.is_empty() {
        return None;
    }
    // A stray % is the name itself.
    if let Some(decoded) = decode_uri_component(&path) {
        path = decoded;
    }
    let pb = path.as_bytes();
    let drive_start = pb.len() >= 2 && pb[0].is_ascii_alphabetic() && pb[1] == b':';
    let drive_path = drive_start && matches!(pb.get(2), Some(b'\\' | b'/'));
    if has_scheme(&path) && !drive_path {
        return None;
    }
    // A leading / or \ is the root's, never a share's \\server.
    let full = if drive_start {
        path
    } else {
        let base = if path.starts_with(is_separator) { root } else { folder };
        format!("{base}\\{path}")
    };
    let volume = volume_of(&full);
    if volume.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in full.get(volume.len()..).unwrap_or("").split(is_separator) {
        if part == ".." {
            parts.pop();
        } else if !part.is_empty() && part != "." {
            parts.push(part);
        }
    }
    Some(format!("{volume}\\{}", parts.join("\\")))
}

/// One step of the breadcrumb trail: what to show, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub name: String,
    pub path: String,
}

fn crumb(name: &str, path: &str) -> Crumb {
    Crumb {
        name: name.to_string(),
        path: path.to_string(),
    }
}

/// Adds each name of `parts` below the last crumb.
fn extend_trail(trail: &mut Vec<Crumb>, parts: Vec<String>) {
    for part in parts {
        let below = trail.last().map_or_else(String::new, |c| join_path(&c.path, &part));
        trail.push(Crumb {
            name: part,
            path: below,
        });
    }
}

/// The names in `text` between separators, skipping empty ones.
fn names_in(text: &str) -> Vec<String> {
    text.split(is_separator)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// The breadcrumb trail: root first, then each folder down to `path`. The names of shadow copies' tops
/// come from `set_shadow_name`.
pub fn crumbs(path: &str) -> Vec<Crumb> {
    crumbs_with(path, shadow_name)
}

/// `crumbs`, with the shadow copy naming given by `name_of_shadow` (called with a path in the shadow copy).
pub fn crumbs_with(path: &str, name_of_shadow: impl Fn(&str) -> String) -> Vec<Crumb> {
    if path == THIS_PC {
        return Vec::new();
    }
    if is_recycle_bin(path) {
        return vec![crumb("Recycle Bin", RECYCLE_BIN)];
    }
    if is_recent(path) {
        return vec![crumb("Recent", RECENT)];
    }
    if is_gallery(path) {
        return vec![crumb("Gallery", GALLERY)];
    }
    // A remote's top is named for its connection's id (the address bar shows the connection's name).
    if let Some(rest) = after_remote(path) {
        let mut parts = names_in(rest).into_iter();
        let id = parts.next().unwrap_or_default();
        let mut trail = vec![Crumb {
            name: id.clone(),
            path: format!("{REMOTE}{id}"),
        }];
        extend_trail(&mut trail, parts.collect());
        return trail;
    }
    // A shadow copy's top is named for its drive and when it was taken.
    let shadow = shadow_root_of(path);
    if !shadow.is_empty() {
        let mut trail = vec![Crumb {
            name: name_of_shadow(path),
            path: shadow.clone(),
        }];
        let rest = path.get(shadow.len()..).unwrap_or("");
        extend_trail(&mut trail, names_in(rest));
        return trail;
    }
    // A share comes after its server, which lists its shares.
    let b = path.as_bytes();
    let (root, rest): (String, &str) = if let Some((server, rest)) = split_server(path) {
        (server.to_string(), rest)
    } else if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        (
            format!("{}\\", path.get(..2).unwrap_or("")),
            path.get(2..).unwrap_or(""),
        )
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/".to_string(), rest)
    } else {
        return vec![crumb(path, path)];
    };
    let mut trail = vec![Crumb {
        name: root.clone(),
        path: root,
    }];
    extend_trail(&mut trail, names_in(rest));
    trail
}

/// The TypeScript `^(\\\\[^\\]+)(.*)$`: the server (with its `\\`) and what follows it. `.` cannot cross a
/// line break, so one after the server's name spoils the match.
fn split_server(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("\\\\")?;
    let name_len = rest.find('\\').unwrap_or(rest.len());
    let after = rest.get(name_len..)?;
    if name_len == 0 || after.contains(is_line_break) {
        return None;
    }
    Some((path.get(..2 + name_len)?, after))
}

/// Of `paths`, those a drop on `dest` would do something with: not already in it, nor it or a folder
/// above it.
pub fn droppable(paths: &[String], dest: &str) -> Vec<String> {
    let dest_lower = dest.to_lowercase();
    let below = |p: &str| dest_lower.starts_with(&format!("{}\\", trim_separators(p).to_lowercase()));
    paths
        .iter()
        .filter(|p| !same_path(&parent_path(p), dest) && !same_path(p, dest) && !below(p))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADOW_TOP: &str = "\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy7\\";

    fn paths_of(trail: &[Crumb]) -> Vec<&str> {
        trail.iter().map(|c| c.path.as_str()).collect()
    }

    fn names_of(trail: &[Crumb]) -> Vec<&str> {
        trail.iter().map(|c| c.name.as_str()).collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn join_and_parent() {
        assert_eq!(join_path("C:\\", "Users"), "C:\\Users");
        assert_eq!(join_path("C:\\Users", "me"), "C:\\Users\\me");
        assert_eq!(join_path("/home", "me"), "/home/me");
        assert_eq!(parent_path("C:\\Users\\me"), "C:\\Users");
        assert_eq!(parent_path("C:\\Users"), "C:\\");
        assert_eq!(parent_path("C:\\"), THIS_PC);
        assert_eq!(parent_path("\\\\server\\share\\docs"), "\\\\server\\share");
        // A share's parent is its server, which lists its shares.
        assert_eq!(parent_path("\\\\server\\share"), "\\\\server");
        assert_eq!(parent_path("\\\\server"), THIS_PC);
    }

    #[test]
    fn roots_samepath_and_crumbs() {
        assert!(
            is_root("D:")
                && is_root("D:\\")
                && is_root("\\\\nas\\media")
                && is_root("\\\\nas")
                && !is_root("C:\\Users")
        );
        assert!(same_path("c:\\users\\", "C:\\Users"));
        assert_eq!(
            paths_of(&crumbs("C:\\Users\\me")),
            ["C:\\", "C:\\Users", "C:\\Users\\me"]
        );
        assert_eq!(
            paths_of(&crumbs("\\\\nas\\media\\tv")),
            ["\\\\nas", "\\\\nas\\media", "\\\\nas\\media\\tv"]
        );
        assert_eq!(crumbs(THIS_PC), Vec::<Crumb>::new());
    }

    #[test]
    fn links_in_markdown() {
        let repo = "C:\\src\\app";
        let docs = "C:\\src\\app\\docs";
        let link = |url: &str| linked_path(url, docs, repo);
        assert_eq!(link("guide.md").as_deref(), Some("C:\\src\\app\\docs\\guide.md"));
        assert_eq!(
            link("./img/a%20b.png?raw=true#x").as_deref(),
            Some("C:\\src\\app\\docs\\img\\a b.png")
        );
        assert_eq!(link("../../../../x.md").as_deref(), Some("C:\\x.md"));
        assert_eq!(link("/LICENSE").as_deref(), Some("C:\\src\\app\\LICENSE"));
        assert_eq!(
            link("\\\\evil\\share\\pic.png").as_deref(),
            Some("C:\\src\\app\\evil\\share\\pic.png")
        );
        assert_eq!(link("D:/notes/todo.md").as_deref(), Some("D:\\notes\\todo.md"));
        assert_eq!(link("https://example.com/a.png"), None);
        assert_eq!(link("mailto:me@example.com"), None);
        assert_eq!(link("#heading"), None);
        assert_eq!(link("100%.md").as_deref(), Some("C:\\src\\app\\docs\\100%.md"));
        assert_eq!(
            linked_path("pic.png", "\\\\nas\\media\\docs", "\\\\nas\\media\\").as_deref(),
            Some("\\\\nas\\media\\docs\\pic.png")
        );
    }

    #[test]
    fn linked_path_decoding_edge_cases() {
        let link = |url: &str| linked_path(url, "C:\\d", "C:\\r");
        assert_eq!(link("caf%C3%A9.md").as_deref(), Some("C:\\d\\caf\u{e9}.md"));
        // Bytes that are not UTF-8 leave the whole name as it was.
        assert_eq!(link("a%FF%20b.md").as_deref(), Some("C:\\d\\a%FF%20b.md"));
        assert_eq!(link(""), None);
        assert_eq!(link("C:foo"), None);
    }

    #[test]
    fn volumes() {
        assert_eq!(volume_of("\\\\nas\\media\\docs\\a"), "\\\\nas\\media");
        assert_eq!(volume_of("c:\\x"), "c:");
        assert_eq!(volume_of("relative\\x"), "");
        assert_eq!(volume_of("\\\\server"), "");
    }

    // The shadow names are shared by every test, so the one that teaches a name also checks the default.
    #[test]
    fn shadow_copies_are_drives_of_their_own() {
        let on_other_device = "\\\\?\\GLOBALROOT\\Device\\HarddiskVolume7\\Users";
        assert!(is_shadow_path(&format!("{SHADOW_TOP}Users")) && !is_shadow_path(on_other_device));
        let without_slash = &SHADOW_TOP[..SHADOW_TOP.len() - 1];
        assert!(is_root(SHADOW_TOP) && is_root(without_slash) && !is_root(&format!("{SHADOW_TOP}Users")));
        assert_eq!(parent_path(&format!("{SHADOW_TOP}Users")), SHADOW_TOP);
        assert_eq!(
            parent_path(&format!("{SHADOW_TOP}Users\\me")),
            format!("{SHADOW_TOP}Users")
        );
        assert_eq!(parent_path(SHADOW_TOP), THIS_PC);
        assert_eq!(volume_of(&format!("{SHADOW_TOP}Users\\a.txt")), without_slash);
        assert_eq!(
            names_of(&crumbs(&format!("{SHADOW_TOP}Users\\me"))),
            ["Shadow copy 7", "Users", "me"]
        );
        set_shadow_name(SHADOW_TOP, "C: on 2 Nov");
        assert_eq!(
            crumbs(&format!("{SHADOW_TOP}Users")),
            vec![
                Crumb {
                    name: "C: on 2 Nov".into(),
                    path: SHADOW_TOP.into()
                },
                Crumb {
                    name: "Users".into(),
                    path: format!("{SHADOW_TOP}Users")
                },
            ]
        );
        clear_shadow_names();
    }

    #[test]
    fn archive_paths() {
        assert!(in_archive_path("C:\\a\\report.docx\\word") && in_archive_path("C:\\a\\b.7z"));
        assert!(!in_archive_path("C:\\a\\docx\\b") && !in_archive_path("C:\\a\\b.zipper"));
        // The archive's own top folder, and the crumbs into it: an archive's path is a folder path like any other.
        let inside = "C:\\D\u{f6}\\photos.zip\\2024\\old.7z\\summer";
        assert_eq!(
            names_of(&crumbs(inside)),
            ["C:\\", "D\u{f6}", "photos.zip", "2024", "old.7z", "summer"]
        );
        assert_eq!(parent_path("C:\\D\u{f6}\\photos.zip"), "C:\\D\u{f6}");
    }

    #[test]
    fn recycle_bin_is_a_place() {
        assert!(is_recycle_bin(RECYCLE_BIN) && is_recycle_bin("SHELL:recyclebinfolder"));
        assert!(!is_recycle_bin("C:\\$Recycle.Bin"));
        assert!(is_root(RECYCLE_BIN) && parent_path(RECYCLE_BIN) == THIS_PC);
        assert!(!is_folder_path(RECYCLE_BIN) && !is_folder_path(THIS_PC));
        assert!(is_folder_path("C:\\") && is_folder_path("C:\\a.zip\\b"));
        assert_eq!(
            crumbs(RECYCLE_BIN),
            vec![Crumb {
                name: "Recycle Bin".into(),
                path: RECYCLE_BIN.into()
            }]
        );
    }

    #[test]
    fn remote_connections() {
        assert!(is_remote_path("remote:photos") && is_remote_path("REMOTE:photos\\2024"));
        assert!(!is_remote_path("C:\\remote:x") && !is_remote_path("rem"));
        assert_eq!(remote_id_of("remote:photos\\2024\\a.jpg"), "photos");
        assert_eq!(remote_id_of("C:\\photos"), "");
        assert!(is_root("remote:photos") && is_root("remote:photos\\") && !is_root("remote:photos\\2024"));
        assert_eq!(parent_path("remote:photos\\2024\\beach"), "remote:photos\\2024");
        assert_eq!(parent_path("remote:photos\\2024"), "remote:photos");
        assert_eq!(parent_path("remote:photos"), THIS_PC);
        assert_eq!(join_path("remote:photos", "2024"), "remote:photos\\2024");
        assert!(!is_folder_path("remote:photos") && !is_folder_path("remote:photos\\2024"));
        assert_eq!(
            crumbs("remote:photos\\2024\\beach"),
            vec![
                Crumb {
                    name: "photos".into(),
                    path: "remote:photos".into()
                },
                Crumb {
                    name: "2024".into(),
                    path: "remote:photos\\2024".into()
                },
                Crumb {
                    name: "beach".into(),
                    path: "remote:photos\\2024\\beach".into()
                },
            ]
        );
    }

    #[test]
    fn drops() {
        assert_eq!(
            droppable(&strings(&["C:\\a\\x.txt", "C:\\b\\y.txt"]), "C:\\a"),
            strings(&["C:\\b\\y.txt"])
        );
        assert_eq!(droppable(&strings(&["C:\\a"]), "C:\\a"), Vec::<String>::new());
        assert_eq!(droppable(&strings(&["C:\\a"]), "C:\\a\\sub"), Vec::<String>::new());
        assert_eq!(droppable(&strings(&["C:\\ab"]), "C:\\a\\sub"), strings(&["C:\\ab"]));
        assert_eq!(droppable(&strings(&["D:\\photos"]), "C:\\"), strings(&["D:\\photos"]));
    }

    #[test]
    fn small_places_and_names() {
        assert!(is_recent("shell:RECENT") && is_gallery("Shell:gallery") && !is_recent(GALLERY));
        assert_eq!(names_of(&crumbs(RECENT)), ["Recent"]);
        assert_eq!(names_of(&crumbs(GALLERY)), ["Gallery"]);
        assert_eq!(base_name("C:\\a\\b\\"), "b");
        assert_eq!(base_name("C:\\"), "C:");
        assert_eq!(base_name("\\"), "\\");
        assert_eq!(parent_path("relative"), THIS_PC);
        assert_eq!(parent_path("/a"), "/");
        assert_eq!(names_of(&crumbs("relative")), ["relative"]);
        assert_eq!(names_of(&crumbs("/a/b")), ["/", "a", "b"]);
        assert!(same_path("\u{c4}\\", "\u{e4}"));
    }
}
