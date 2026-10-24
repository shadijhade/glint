//! Remote connections: an S3-compatible bucket (Amazon S3, Cloudflare R2, Backblaze B2, MinIO…) or a
//! WebDAV server (Nextcloud, a NAS, dufs), browsed as a read-only folder at `remote:<id>\…` (the id is
//! made once from the connection's name, and kept through renames). Connections come from the app's
//! settings without their secrets, which Windows Credential Manager keeps ("<app>/remote/<id>").
//!
//! A remote's names are anyone's: each is shown as Windows would take it (archive::tree::clean_name,
//! made unique), and remembered against the remote's own where the two differ. What's previewed,
//! opened, copied or dragged out is downloaded first, into the folders archives' files are extracted
//! to (archive/extract.rs): a preview up to 100 MB. The clients are Glint's own, on the reqwest and
//! rustls already built for updates (OpenDAL would have added 5.7 MB).

mod dav;
mod s3;
mod time;

use crate::archive::{extract, Skipped, StageProgress, Staged};
use crate::fsx::{Entry, Resolved};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tauri::ipc::Channel;

/// In Glint's paths: `remote:<id>\folder\…`.
pub const PREFIX: &str = "remote:";

/// What a remote's items say about themselves (entries.ts's ATTR): read through copies, as an
/// archive's are (0x40000000), and on a remote (0x04000000).
const ATTRS: u32 = 0x4000_0000 | 0x0400_0000;

/// The most a preview downloads.
const PREVIEW_MOST: u64 = 100 << 20;

/// What a server says to a range it won't give (past a small file's end, at some; of an empty one, at S3).
pub(crate) const UNSATISFIABLE: &str = "The part asked for isn't there";

/// The pictures a thumbnail is made of (downloaded, where the connection allows), up to 16 MB.
const PICTURES: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "heic", "avif"];
const THUMBNAIL_MOST: u64 = 16 << 20;

pub fn is_remote(path: &str) -> bool {
    path.get(..PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(PREFIX))
}

/// A connection, as the app's settings have it (no secret).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Connection {
    pub id: String,
    pub name: String,
    /// "s3" or "webdav".
    pub kind: String,
    /// S3: the service's address (Amazon's, from the region, when it's empty); WebDAV: the server's URL.
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// S3's access key ID; WebDAV's user name.
    pub user: String,
    /// The folder on it to start at.
    pub root: String,
    /// Pictures shown in the grid get thumbnails, which downloads each one (up to 16 MB).
    pub thumbnails: bool,
}

/// A file or folder on a remote, by its own name.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Milliseconds since 1970, 0 when unknown.
    pub modified: i64,
}

enum Backend {
    S3(s3::Bucket),
    Dav(dav::Server),
}

impl Backend {
    async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        match self {
            Backend::S3(b) => b.list(path).await,
            Backend::Dav(d) => d.list(path).await,
        }
    }

    async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        match self {
            Backend::S3(b) => b.stat(path).await,
            Backend::Dav(d) => d.stat(path).await,
        }
    }

    async fn get(&self, path: &str, first: Option<u64>) -> Result<reqwest::Response, String> {
        match self {
            Backend::S3(b) => b.get(path, first).await,
            Backend::Dav(d) => d.get(path, first).await,
        }
    }
}

#[derive(Default)]
struct State {
    connections: Vec<Connection>,
    /// Each connection's client once made, by id: made again when it or its secret changes.
    made: HashMap<String, Arc<Backend>>,
    /// Names shown other than the remote's own, by folder ("<id>\a\b", lower case): shown (lower case) → the remote's.
    names: HashMap<String, HashMap<String, String>>,
    /// Copies downloaded for previews, by (path in lower case, size, date).
    copies: HashMap<(String, u64, i64), PathBuf>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn client() -> Result<reqwest::Client, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("Glint/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn with_scheme(address: &str) -> String {
    if address.contains("://") {
        address.to_string()
    } else {
        format!("https://{address}")
    }
}

/// A client for `connection`, with its secret.
fn make(connection: &Connection, secret: &str) -> Result<Backend, String> {
    let root = connection.root.trim().replace('\\', "/").trim_matches('/').to_string();
    match connection.kind.as_str() {
        "s3" => {
            let bucket = connection.bucket.trim();
            if bucket.is_empty() {
                return Err("Which bucket? Its name goes in the connection".into());
            }
            let region = match connection.region.trim() {
                "" => "us-east-1",
                region => region,
            };
            let endpoint = match connection.endpoint.trim() {
                "" => format!("https://s3.{region}.amazonaws.com"),
                endpoint => with_scheme(endpoint),
            };
            let endpoint = url::Url::parse(&endpoint).map_err(|_| format!("“{endpoint}” isn't an address"))?;
            Ok(Backend::S3(s3::Bucket { client: client()?, endpoint, bucket: bucket.into(), region: region.into(), access_key: connection.user.trim().into(), secret: secret.into(), root }))
        }
        "webdav" => {
            let address = with_scheme(connection.endpoint.trim());
            let mut base = url::Url::parse(&address).map_err(|_| format!("“{}” isn't an address", connection.endpoint.trim()))?;
            if !base.path().ends_with('/') {
                base.set_path(&format!("{}/", base.path()));
            }
            Ok(Backend::Dav(dav::Server { client: client()?, base, user: connection.user.trim().into(), password: secret.into(), root }))
        }
        other => Err(format!("Glint doesn't know connections of the kind “{other}”")),
    }
}

/// The connection `id` and its client.
fn backend(id: &str) -> Result<(Connection, Arc<Backend>), String> {
    let mut state = state();
    let connection = state.connections.iter().find(|c| c.id.eq_ignore_ascii_case(id)).cloned().ok_or("That connection isn't there any more: add it again in the sidebar's Remote section")?;
    if let Some(made) = state.made.get(&connection.id) {
        return Ok((connection, made.clone()));
    }
    let made = Arc::new(make(&connection, &secrets::read(&connection.id).unwrap_or_default())?);
    state.made.insert(connection.id.clone(), made.clone());
    Ok((connection, made))
}

/// A remote path's connection id, and the names below it as they're shown.
fn parts(path: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = path.get(PREFIX.len()..)?.split(['\\', '/']).filter(|p| !p.is_empty());
    let id = parts.next()?;
    Some((id, parts.collect()))
}

/// Glint's path for the names `shown` below the connection `id`.
fn shown_path(id: &str, shown: &[&str]) -> String {
    std::iter::once(format!("{PREFIX}{id}")).chain(shown.iter().map(|s| s.to_string())).collect::<Vec<_>>().join("\\")
}

/// The remote's own path ("a/b/c") for names as they're shown.
fn raw(id: &str, shown: &[&str]) -> String {
    let state = state();
    let mut folder = id.to_lowercase();
    let mut out = Vec::new();
    for name in shown {
        let own = state.names.get(&folder).and_then(|m| m.get(&name.to_lowercase())).cloned();
        out.push(own.unwrap_or_else(|| name.to_string()));
        folder.push('\\');
        folder.push_str(&name.to_lowercase());
    }
    out.join("/")
}

/// Items as entries Windows would take (each name made safe, then unique), with the names that changed
/// (shown, lower case → the remote's).
fn entries_of(items: Vec<Item>) -> (Vec<Entry>, HashMap<String, String>) {
    let mut seen = HashSet::new();
    let mut renamed = HashMap::new();
    let mut entries = Vec::new();
    for item in items {
        let Some(clean) = crate::archive::tree::clean_name(&item.name) else { continue };
        let mut shown = clean.clone();
        let mut n = 2;
        while !seen.insert(shown.to_lowercase()) {
            shown = crate::fsx::numbered(&clean, item.dir, n);
            n += 1;
        }
        if shown != item.name {
            renamed.insert(shown.to_lowercase(), item.name.clone());
        }
        entries.push(Entry { name: shown, path: None, dir: item.dir, size: item.size, modified: item.modified, attrs: ATTRS });
    }
    crate::fsx::sort_entries(&mut entries);
    (entries, renamed)
}

/// A folder on a remote, as fsx::list_dir answers: its listing, marked as a remote's.
pub async fn listing(path: &str, started: Instant) -> Result<Vec<u8>, String> {
    let (id, shown) = parts(path).ok_or("That isn't a folder on a remote")?;
    let (connection, backend) = backend(id)?;
    let items = backend.list(&raw(id, &shown)).await?;
    let location = shown_path(&connection.id, &shown);
    let (entries, renamed) = entries_of(items);
    let key = location[PREFIX.len()..].to_lowercase();
    {
        let mut state = state();
        if renamed.is_empty() {
            state.names.remove(&key);
        } else {
            state.names.insert(key, renamed);
        }
    }
    let mut bytes = crate::fsx::encode_listing(&location, &entries, started.elapsed().as_secs_f64() * 1000.0, None, false);
    crate::fsx::mark_remote(&mut bytes);
    Ok(bytes)
}

/// Whether a path on a remote is a folder or a file (fsx::resolve_path's answer).
pub async fn resolve(path: &str) -> Result<Resolved, String> {
    let (id, shown) = parts(path).ok_or("That isn't on a remote")?;
    let (connection, backend) = backend(id)?;
    let location = shown_path(&connection.id, &shown);
    if shown.is_empty() {
        return Ok(Resolved { path: location, dir: true });
    }
    match backend.stat(&raw(id, &shown)).await? {
        Some(item) => Ok(Resolved { path: location, dir: item.dir }),
        None => Err(format!("Can't find “{}” on {}", shown.join("\\"), connection.name)),
    }
}

/// Downloads `raw` to a new file at `dest`, never more than the `expected` size and failing if fewer
/// come; `tick` gets the bytes as they come and stops it by returning false ("cancelled"). The file
/// gets `modified` as its date, and is made read-only for opening. On any failure it's deleted.
async fn download(backend: &Backend, raw: &str, dest: &Path, expected: Option<u64>, modified: i64, read_only: bool, tick: &mut (dyn FnMut(u64) -> bool + Send)) -> Result<u64, String> {
    use std::io::Write;
    let result = async {
        let mut response = backend.get(raw, None).await?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = std::fs::File::create(dest).map_err(crate::fsx::file_error)?;
        let mut written = 0u64;
        while let Some(chunk) = response.chunk().await.map_err(|e| reach_failure(&e))? {
            written += chunk.len() as u64;
            if expected.is_some_and(|n| written > n) {
                return Err("It sent more than its size".to_string());
            }
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            if !tick(chunk.len() as u64) {
                return Err("cancelled".to_string());
            }
        }
        if let Some(n) = expected.filter(|&n| n != written) {
            return Err(format!("It ended after {written} of its {n} bytes"));
        }
        if modified > 0 {
            let _ = file.set_modified(UNIX_EPOCH + Duration::from_millis(modified as u64));
        }
        drop(file);
        if read_only {
            let mut permissions = std::fs::metadata(dest).map_err(|e| e.to_string())?.permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(dest, permissions).map_err(|e| e.to_string())?;
        }
        Ok(written)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

/// A file on a remote downloaded for a preview (within its budget, kept while Glint runs) or to open
/// (read-only): archive_extract's answer for a remote's path.
pub async fn copy_of(path: &str, purpose: &str) -> Result<String, String> {
    let (id, shown) = parts(path).ok_or("That isn't on a remote")?;
    let (_, backend) = backend(id)?;
    let raw = raw(id, &shown);
    let item = backend.stat(&raw).await?.ok_or(s3::NOT_FOUND)?;
    if item.dir {
        return Err("That's a folder".into());
    }
    let preview = purpose != "open";
    if preview && item.size > PREVIEW_MOST {
        return Err(format!("At {} MB it's too big to preview from a remote: copy it out to open it", item.size >> 20));
    }
    let key = (path.to_lowercase(), item.size, item.modified);
    if let Some(copy) = state().copies.get(&key).filter(|p| p.is_file()) {
        return Ok(copy.to_string_lossy().into_owned());
    }
    let folder = extract::new_folder(if preview { "preview" } else { "open" }).map_err(|e| e.message())?;
    let dest = extract::inside(&folder, &[shown.last().copied().unwrap_or("file")]).map_err(|e| e.message())?;
    download(&backend, &raw, &dest, Some(item.size), item.modified, !preview, &mut |_| true).await?;
    if preview {
        state().copies.insert(key, dest.clone());
    }
    Ok(dest.to_string_lossy().into_owned())
}

/// The first `limit` bytes of a file on a remote (asked for as a range), and whether there's more:
/// fsx::read_text_preview's answer for a remote's path.
pub async fn read_prefix(path: &str, limit: usize) -> Result<(Vec<u8>, bool), String> {
    let (id, shown) = parts(path).ok_or("That isn't on a remote")?;
    let (_, backend) = backend(id)?;
    let raw = raw(id, &shown);
    let mut response = match backend.get(&raw, Some(limit as u64 + 1)).await {
        // A range the server won't give: the whole file, read only as far as needed.
        Err(e) if e == UNSATISFIABLE => backend.get(&raw, None).await?,
        other => other?,
    };
    let mut bytes = Vec::new();
    // A server that sends it all anyway is stopped once there's enough (dropping the answer).
    while let Some(chunk) = response.chunk().await.map_err(|e| reach_failure(&e))? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > limit {
            break;
        }
    }
    let more = bytes.len() > limit;
    bytes.truncate(limit);
    Ok((bytes, more))
}

/// Whether the connection a path is on gives its pictures thumbnails (a download each).
pub fn thumbnails_allowed(path: &str) -> bool {
    parts(path).is_some_and(|(id, _)| state().connections.iter().any(|c| c.id.eq_ignore_ascii_case(id) && c.thumbnails))
}

/// A picture on a remote (up to 16 MB) downloaded for its thumbnail: what thumbs.rs makes it of. None
/// for anything else.
pub async fn thumbnail_source(path: &str, size: u64) -> Option<String> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    if !PICTURES.contains(&ext.as_str()) || size > THUMBNAIL_MOST {
        return None;
    }
    copy_of(path, "preview").await.ok()
}

/// What staging downloads: from where, to where, how big, and its date.
struct Job {
    backend: Arc<Backend>,
    raw: String,
    shown: String,
    dest: PathBuf,
    size: u64,
    modified: i64,
}

/// Downloads `paths` (files and folders, with all that's in them) for Windows' file engine to copy
/// ("copy"), to hand to another app ("out"), or to open, read-only ("open"): archive_stage's answer
/// for a remote's paths.
pub async fn stage(paths: &[String], purpose: &str, cancel: &AtomicBool, progress: &Channel<StageProgress>) -> Result<Staged, String> {
    let out = purpose != "copy";
    if out {
        extract::sweep_out(&crate::shell::clipboard_paths(), Duration::from_secs(3600));
    }
    let folder = extract::new_folder(if out { "out" } else { "copy" }).map_err(|e| e.message())?;
    let result = stage_into(&folder, paths, purpose == "open", cancel, progress).await;
    if !matches!(&result, Ok(staged) if !staged.paths.is_empty()) {
        extract::remove_tree(&folder);
    }
    match result {
        Ok(staged) if staged.paths.is_empty() => Err(staged.skipped.into_iter().next().map_or_else(|| s3::NOT_FOUND.to_string(), |s| s.reason)),
        other => other,
    }
}

async fn stage_into(folder: &Path, paths: &[String], read_only: bool, cancel: &AtomicBool, progress: &Channel<StageProgress>) -> Result<Staged, String> {
    let failure = |e: crate::archive::Failure| e.message();
    let mut jobs = Vec::new();
    let mut tops = Vec::new();
    let mut names = HashSet::new();
    let mut skipped = Vec::new();
    for (k, path) in paths.iter().enumerate() {
        let (id, shown) = parts(path).ok_or("That isn't on a remote")?;
        let (_, backend) = backend(id)?;
        let top = *shown.last().ok_or("A remote's top can't be copied as a whole: open it and choose what's in it")?;
        let raw = raw(id, &shown);
        // Two of the same name (from different folders) each go in a folder of their own.
        let dest = if names.insert(top.to_lowercase()) { extract::inside(folder, &[top]) } else { extract::inside(folder, &[&k.to_string(), top]) }.map_err(failure)?;
        let item = backend.stat(&raw).await?.ok_or(s3::NOT_FOUND)?;
        if !item.dir {
            jobs.push(Job { backend: backend.clone(), raw, shown: path.clone(), dest: dest.clone(), size: item.size, modified: item.modified });
            tops.push(dest);
            continue;
        }
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        let mut stack = vec![(raw, path.clone(), dest.clone())];
        while let Some((raw, shown, dest)) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let (entries, renamed) = entries_of(backend.list(&raw).await?);
            for entry in entries {
                let own = renamed.get(&entry.name.to_lowercase()).cloned().unwrap_or_else(|| entry.name.clone());
                let (raw, shown, dest) = (format!("{}/{own}", raw.trim_end_matches('/')).trim_start_matches('/').to_string(), format!("{shown}\\{}", entry.name), dest.join(&entry.name));
                if entry.dir {
                    if let Err(e) = std::fs::create_dir_all(&dest) {
                        skipped.push(Skipped { path: shown, reason: e.to_string() });
                        continue;
                    }
                    stack.push((raw, shown, dest));
                } else {
                    jobs.push(Job { backend: backend.clone(), raw, shown, dest, size: entry.size, modified: entry.modified });
                }
            }
        }
        tops.push(dest);
    }
    let total: u64 = jobs.iter().map(|j| j.size).sum();
    extract::check_space(folder, total).map_err(failure)?;
    let _ = progress.send(StageProgress { done: 0, total });
    let mut done = 0u64;
    let mut last = Instant::now();
    for job in jobs {
        let mut tick = |n: u64| {
            done += n;
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                let _ = progress.send(StageProgress { done, total });
            }
            !cancel.load(Ordering::Relaxed)
        };
        match download(&job.backend, &job.raw, &job.dest, Some(job.size), job.modified, read_only, &mut tick).await {
            Ok(_) => {}
            Err(e) if e == "cancelled" => return Err(e),
            Err(reason) => skipped.push(Skipped { path: job.shown, reason }),
        }
    }
    let _ = progress.send(StageProgress { done: total, total });
    tops.retain(|p| p.exists());
    Ok(Staged { folder: folder.to_string_lossy().into_owned(), paths: tops.iter().map(|p| p.to_string_lossy().into_owned()).collect(), skipped, bytes: total })
}

/// Why a request didn't get an answer, in words.
pub(crate) fn reach_failure(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "It didn't answer in time".into()
    } else if e.is_connect() {
        format!("Couldn't connect to it ({})", std::error::Error::source(e).map_or_else(|| e.to_string(), |s| s.to_string()))
    } else {
        e.to_string()
    }
}

/// An HTTP answer that isn't what was asked for, in words.
pub(crate) fn http_failure(status: reqwest::StatusCode, said: &str) -> String {
    let what = format!("{} {}", status.as_u16(), status.canonical_reason().unwrap_or(""));
    if said.trim().is_empty() {
        format!("The server answered {}", what.trim())
    } else {
        format!("The server answered {}: {}", what.trim(), said.trim())
    }
}

/// An XML element's fields, found by `xml_items`: each one's text, and whether it was there at all.
pub(crate) struct Fields(Vec<Option<String>>);

impl Fields {
    pub fn text(&self, i: usize) -> &str {
        self.0[i].as_deref().unwrap_or("")
    }
    pub fn has(&self, i: usize) -> bool {
        self.0[i].is_some()
    }
}

impl std::ops::Index<usize> for Fields {
    type Output = str;
    fn index(&self, i: usize) -> &str {
        self.text(i)
    }
}

/// Each `item` element in `xml` (by its local name, whatever its namespace), with the text of the first
/// element below it with each of `fields`' names.
pub(crate) fn xml_items(xml: &str, item: &str, fields: &[&str]) -> Vec<Fields> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let mut out = Vec::new();
    // The item being read and how deep it started; the field whose text this is.
    let mut current: Option<(Vec<Option<String>>, usize)> = None;
    let mut field: Option<usize> = None;
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                let name = e.local_name().into_inner();
                match &mut current {
                    None if name == item => current = Some((vec![None; fields.len()], depth)),
                    Some((values, _)) => {
                        field = fields.iter().position(|f| *f == name).filter(|&i| values[i].is_none());
                        if let Some(i) = field {
                            values[i] = Some(String::new());
                        }
                    }
                    None => {}
                }
            }
            Ok(Event::Empty(e)) => {
                if let (Some((values, _)), Some(i)) = (&mut current, fields.iter().position(|f| *f == e.local_name().into_inner())) {
                    values[i].get_or_insert_with(String::new);
                }
            }
            Ok(Event::Text(t)) => {
                if let (Some((values, _)), Some(i)) = (&mut current, field) {
                    values[i].get_or_insert_with(String::new).push_str(&t.into_inner());
                }
            }
            Ok(Event::CData(t)) => {
                if let (Some((values, _)), Some(i)) = (&mut current, field) {
                    values[i].get_or_insert_with(String::new).push_str(&t.into_inner());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let (Some((values, _)), Some(i)) = (&mut current, field) {
                    let name = r.into_inner();
                    let text = match quick_xml::events::BytesRef::new(name.as_ref()).resolve_char_ref() {
                        Ok(Some(c)) => c.to_string(),
                        _ => match name.as_ref() {
                            "amp" => "&".into(),
                            "lt" => "<".into(),
                            "gt" => ">".into(),
                            "quot" => "\"".into(),
                            "apos" => "'".into(),
                            _ => String::new(),
                        },
                    };
                    values[i].get_or_insert_with(String::new).push_str(&text);
                }
            }
            Ok(Event::End(_)) => {
                field = None;
                if current.as_ref().is_some_and(|(_, at)| *at == depth) {
                    out.push(Fields(current.take().map(|(v, _)| v).unwrap_or_default()));
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// The connections the app has (it sends them at start and whenever they change): one that changed is
/// made again, one that went is forgotten.
#[tauri::command]
pub fn remote_configure(connections: Vec<Connection>) {
    let mut state = state();
    let kept: Vec<String> = connections.iter().filter(|c| state.connections.contains(c)).map(|c| c.id.clone()).collect();
    state.made.retain(|id, _| kept.contains(id));
    state.connections = connections;
}

/// Keeps a connection's secret (an S3 secret access key, a WebDAV password) in Credential Manager.
#[tauri::command]
pub fn remote_secret_set(id: String, secret: String) -> Result<(), String> {
    secrets::write(&id, &secret)?;
    state().made.remove(&id);
    Ok(())
}

#[tauri::command]
pub fn remote_secret_delete(id: String) {
    let _ = secrets::delete(&id);
    state().made.remove(&id);
}

/// Whether a connection has a secret kept.
#[tauri::command]
pub fn remote_secret_saved(id: String) -> bool {
    secrets::read(&id).is_some()
}

/// Tries a connection as it's typed (with `secret`, or the one kept when there's none): says what's
/// at its top, or why it didn't connect.
#[tauri::command]
pub async fn remote_test(connection: Connection, secret: Option<String>) -> Result<String, String> {
    let secret = secret.unwrap_or_else(|| secrets::read(&connection.id).unwrap_or_default());
    let items = make(&connection, &secret)?.list("").await?;
    Ok(match items.len() {
        0 => "Connected: there's nothing there yet".into(),
        1 => "Connected: 1 item at its top".into(),
        n => format!("Connected: {n} items at its top"),
    })
}

/// The app's name, for its secrets' names in Credential Manager (a test copy's stay apart).
static APP: OnceLock<String> = OnceLock::new();

pub fn init(app: &str) {
    let _ = APP.set(app.to_string());
}

/// Connections' secrets, in Windows Credential Manager as generic credentials "<app>/remote/<id>".
mod secrets {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Security::Credentials::{CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};

    fn target(id: &str) -> HSTRING {
        HSTRING::from(format!("{}/remote/{id}", super::APP.get().map_or("Glint", String::as_str)))
    }

    pub fn write(id: &str, secret: &str) -> Result<(), String> {
        let target = target(id);
        let user = HSTRING::from(id);
        let blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_ptr() as *mut u16),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_ptr() as *mut u16),
            ..Default::default()
        };
        unsafe { CredWriteW(&credential, 0) }.map_err(|e| e.message())
    }

    pub fn read(id: &str) -> Option<String> {
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe { CredReadW(&target(id), CRED_TYPE_GENERIC, None, &mut found) }.ok()?;
        let credential = unsafe { &*found };
        let bytes = unsafe { std::slice::from_raw_parts(credential.CredentialBlob, credential.CredentialBlobSize as usize) }.to_vec();
        unsafe { CredFree(found as *const std::ffi::c_void) };
        String::from_utf8(bytes).ok()
    }

    pub fn delete(id: &str) -> Result<(), String> {
        unsafe { CredDeleteW(&target(id), CRED_TYPE_GENERIC, None) }.map_err(|e| e.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_paths() {
        assert!(is_remote("remote:photos") && is_remote(r"REMOTE:photos\2024") && !is_remote(r"C:\remote:x") && !is_remote("rem"));
        assert_eq!(parts(r"remote:photos\2024\beach.jpg"), Some(("photos", vec!["2024", "beach.jpg"])));
        assert_eq!(parts("remote:photos"), Some(("photos", vec![])));
        assert_eq!(parts("remote:"), None);
        assert_eq!(shown_path("photos", &["2024", "a.jpg"]), r"remote:photos\2024\a.jpg");
    }

    #[test]
    fn shows_names_windows_takes_and_finds_them_again() {
        let items = vec![
            Item { name: "logs".into(), dir: true, size: 0, modified: 0 },
            Item { name: "2024-07-14T10:30:05.log".into(), dir: false, size: 3, modified: 0 },
            Item { name: "Notes.txt".into(), dir: false, size: 1, modified: 0 },
            Item { name: "notes.txt".into(), dir: false, size: 2, modified: 0 },
            Item { name: "..".into(), dir: true, size: 0, modified: 0 },
        ];
        let (entries, renamed) = entries_of(items);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["logs", "2024-07-14T10_30_05.log", "notes (2).txt", "Notes.txt"]);
        assert!(entries.iter().all(|e| e.attrs == ATTRS));
        assert_eq!(renamed.get("2024-07-14t10_30_05.log").map(String::as_str), Some("2024-07-14T10:30:05.log"));
        assert_eq!(renamed.get("notes (2).txt").map(String::as_str), Some("notes.txt"));
        state().names.insert("test-names\\logs".into(), renamed);
        assert_eq!(raw("test-names", &["logs", "notes (2).txt"]), "logs/notes.txt");
        assert_eq!(raw("test-names", &["logs", "Notes.txt"]), "logs/Notes.txt");
    }

    #[test]
    fn makes_clients_from_what_is_typed() {
        let s3 = Connection { id: "b".into(), name: "B".into(), kind: "s3".into(), bucket: "photos".into(), region: "eu-west-1".into(), ..Default::default() };
        let Ok(Backend::S3(bucket)) = make(&s3, "x") else { panic!() };
        assert_eq!(bucket.endpoint.as_str(), "https://s3.eu-west-1.amazonaws.com/");
        assert!(make(&Connection { bucket: String::new(), ..s3.clone() }, "").is_err());
        let dav = Connection { id: "d".into(), name: "D".into(), kind: "webdav".into(), endpoint: "nas.local:5006/dav".into(), root: r"\photos\".into(), ..Default::default() };
        let Ok(Backend::Dav(server)) = make(&dav, "pw") else { panic!() };
        assert_eq!((server.base.as_str(), server.root.as_str()), ("https://nas.local:5006/dav/", "photos"));
    }
}
