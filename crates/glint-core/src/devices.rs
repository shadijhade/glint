//! Devices: Glint on your other PCs, over iroh (QUIC dialed by public key, through NATs and firewalls,
//! relayed when no direct path can be made, end-to-end encrypted either way). A PC shares the folders
//! chosen in Settings → Devices with the PCs paired with it, read-only; a paired PC browses them as a
//! remote (`remote:<id>`, of the kind "device": remote/device.rs), previewing and copying out as from
//! any remote. Pairing: the sharing PC shows a code (its key and a one-time secret, good for ten
//! minutes), the other PC enters it, and from then on is let in by its own key alone.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What Glint's devices speak (the QUIC handshake's ALPN).
pub const ALPN: &[u8] = b"glint/devices/1";
/// A request's most (an answer's JSON part can be bigger: a folder's whole listing).
const MAX_REQUEST: usize = 64 << 10;
const MAX_ANSWER: usize = 64 << 20;
const PAIRING_LIFE: Duration = Duration::from_secs(600);

/// A folder shared with paired PCs: the name they see, and where it is here.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Share {
    pub name: String,
    pub path: String,
}

/// A PC let in: its key, its name, and when it was paired (ms).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub paired: i64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct Saved {
    /// This PC's secret key, in hex: its identity to other PCs.
    secret: String,
    sharing: bool,
    shares: Vec<Share>,
    allowed: Vec<Peer>,
}

fn saved_path() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("devices.json"))
}

static SAVED: Mutex<Option<Saved>> = Mutex::new(None);

fn saved() -> Saved {
    let mut cached = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if cached.is_none() {
        *cached = Some(saved_path().and_then(|p| fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    }
    cached.clone().unwrap_or_default()
}

fn save(change: impl FnOnce(&mut Saved)) -> Result<Saved, String> {
    let mut now = saved();
    change(&mut now);
    let path = saved_path().ok_or("Glint's data folder isn't available")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&now).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(now.clone());
    Ok(now)
}

/// This PC's name, as others see it.
pub fn this_pc() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let bytes: Vec<u8> = (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect::<Option<_>>()?;
    bytes.try_into().ok()
}

/// This PC's key, made the first time.
fn secret_key() -> Result<iroh::SecretKey, String> {
    if let Some(bytes) = unhex(&saved().secret) {
        return Ok(iroh::SecretKey::from_bytes(&bytes));
    }
    let key = iroh::SecretKey::generate();
    save(|s| s.secret = hex(&key.to_bytes()))?;
    Ok(key)
}

static ENDPOINT: tokio::sync::OnceCell<iroh::Endpoint> = tokio::sync::OnceCell::const_new();

/// Glint's iroh endpoint, started the first time it's wanted (and when Glint starts, with sharing on),
/// answering paired PCs from then on.
pub async fn endpoint() -> Result<&'static iroh::Endpoint, String> {
    ENDPOINT
        .get_or_try_init(|| async {
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0).secret_key(secret_key()?).alpns(vec![ALPN.to_vec()]).bind().await.map_err(|e| format!("Devices couldn't start: {e}"))?;
            let serving = endpoint.clone();
            crate::host::async_runtime::spawn(serve(serving));
            Ok::<_, String>(endpoint)
        })
        .await
}

/// Where files sent here are announced (the app's windows).
static APP: std::sync::OnceLock<crate::host::AppHandle> = std::sync::OnceLock::new();

/// With sharing on, or a PC paired (it may send files here), the endpoint starts with Glint, so paired
/// PCs find it.
pub fn start(app: &crate::host::AppHandle) {
    let _ = APP.set(app.clone());
    let now = saved();
    if now.sharing || !now.allowed.is_empty() {
        crate::host::async_runtime::spawn(async {
            let _ = endpoint().await;
        });
    }
}

/// What a PC asks another.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    /// The pairing code's secret, and the asking PC's name.
    Pair { secret: String, name: String },
    List { path: String },
    Stat { path: String },
    /// A file's bytes from `offset`, after the answer.
    Read { path: String, offset: u64 },
    /// A file sent to this PC (Send to…): its path in what's sent ("Trip/day 1.jpg"), and its size; its
    /// bytes follow the request. It lands in Downloads, in a folder named after the PC it came from.
    Push { path: String, size: u64 },
}

/// An item as it goes between PCs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Wire {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct Answer {
    pub error: String,
    pub name: String,
    pub items: Vec<Wire>,
    pub item: Option<Wire>,
    pub size: u64,
}

impl Answer {
    fn error(text: impl Into<String>) -> Answer {
        Answer { error: text.into(), ..Default::default() }
    }
}

/// A frame: its length (4 bytes, little-endian), then its JSON.
pub async fn write_frame<T: Serialize>(send: &mut iroh::endpoint::SendStream, value: &T) -> Result<(), String> {
    let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    send.write_all(&(json.len() as u32).to_le_bytes()).await.map_err(|e| e.to_string())?;
    send.write_all(&json).await.map_err(|e| e.to_string())
}

pub async fn read_frame<T: serde::de::DeserializeOwned>(recv: &mut iroh::endpoint::RecvStream, max: usize) -> Result<T, String> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.map_err(|e| format!("The other PC hung up: {e}"))?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err("The other PC sent too much".into());
    }
    let mut json = vec![0u8; len];
    recv.read_exact(&mut json).await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&json).map_err(|e| e.to_string())
}

async fn serve(endpoint: iroh::Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        crate::host::async_runtime::spawn(async move {
            let Ok(connection) = incoming.await else { return };
            let peer = connection.remote_id().to_string();
            while let Ok((send, recv)) = connection.accept_bi().await {
                let peer = peer.clone();
                crate::host::async_runtime::spawn(async move {
                    let _ = answer(&peer, send, recv).await;
                });
            }
        });
    }
}

/// The pairing code shown now: its secret, and until when it works.
static PAIRING: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Whether `secret` is the code's (once: a code pairs one PC).
fn take_pairing(secret: &str) -> bool {
    let mut pairing = PAIRING.lock().unwrap_or_else(|e| e.into_inner());
    let good = pairing.as_ref().is_some_and(|(want, until)| Instant::now() < *until && want.len() == secret.len() && want.bytes().zip(secret.bytes()).fold(0, |d, (a, b)| d | (a ^ b)) == 0);
    if good {
        *pairing = None;
    }
    good
}

async fn answer(peer: &str, mut send: iroh::endpoint::SendStream, mut recv: iroh::endpoint::RecvStream) -> Result<(), String> {
    let request: Request = read_frame(&mut recv, MAX_REQUEST).await?;
    match request {
        Request::Pair { secret, name } => {
            let reply = if take_pairing(&secret) {
                let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
                let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                save(|s| {
                    s.allowed.retain(|p| p.id != peer);
                    s.allowed.push(Peer { id: peer.to_string(), name, paired: at });
                })?;
                Answer { name: this_pc(), ..Default::default() }
            } else {
                Answer::error("That code isn't right, or its ten minutes are up: show a new one")
            };
            write_frame(&mut send, &reply).await?;
        }
        Request::Push { path, size } => {
            let now = saved();
            let from = now.allowed.iter().find(|p| p.id == peer).map(|p| p.name.clone());
            let reply = match from {
                None => Answer::error("That PC isn't paired with this one any more: pair them again in Settings → Devices"),
                Some(from) => match receive(&from, &path, size, &mut recv).await {
                    Ok(to) => {
                        announce(&from, &to);
                        Answer { name: to.to_string_lossy().into_owned(), ..Default::default() }
                    }
                    Err(e) => Answer::error(e),
                },
            };
            write_frame(&mut send, &reply).await?;
        }
        request => {
            let now = saved();
            if !now.allowed.iter().any(|p| p.id == peer) {
                write_frame(&mut send, &Answer::error("That PC isn't paired with this one any more: pair them again in Settings → Devices")).await?;
            } else if !now.sharing {
                write_frame(&mut send, &Answer::error("That PC has stopped sharing (Settings → Devices)")).await?;
            } else {
                handle(request, &now.shares, &mut send).await?;
            }
        }
    }
    let _ = send.finish();
    Ok(())
}

async fn handle(request: Request, shares: &[Share], send: &mut iroh::endpoint::SendStream) -> Result<(), String> {
    match request {
        Request::List { path } => {
            let answer = match local(&path, shares) {
                Ok(None) => Answer { items: shares.iter().map(|s| Wire { name: s.name.clone(), dir: true, size: 0, modified: 0 }).collect(), ..Default::default() },
                Ok(Some(dir)) => match fs::read_dir(&dir) {
                    Ok(items) => Answer { items: items.flatten().filter_map(|item| wire(&item.path(), item.file_name().to_string_lossy().into_owned())).collect(), ..Default::default() },
                    Err(e) => Answer::error(e.to_string()),
                },
                Err(e) => Answer::error(e),
            };
            write_frame(send, &answer).await
        }
        Request::Stat { path } => {
            let answer = match local(&path, shares) {
                Ok(None) => Answer { item: Some(Wire { name: String::new(), dir: true, size: 0, modified: 0 }), ..Default::default() },
                Ok(Some(full)) => Answer { item: wire(&full, full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), ..Default::default() },
                Err(e) => Answer::error(e),
            };
            write_frame(send, &answer).await
        }
        Request::Read { path, offset } => {
            use std::io::{Read, Seek};
            let file = match local(&path, shares).and_then(|p| p.ok_or_else(|| "That's a folder".to_string())).and_then(|p| fs::File::open(p).map_err(|e| e.to_string())) {
                Ok(file) => file,
                Err(e) => return write_frame(send, &Answer::error(e)).await,
            };
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            write_frame(send, &Answer { size: size.saturating_sub(offset), ..Default::default() }).await?;
            let mut file = file;
            file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            let mut buffer = vec![0u8; 256 << 10];
            loop {
                let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                // The other PC stopping (a preview has enough) ends this.
                send.write_all(&buffer[..n]).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Request::Pair { .. } | Request::Push { .. } => Ok(()),
    }
}

/// Where files from `from` land: Downloads\From <PC>.
fn inbox(from: &str) -> Result<PathBuf, String> {
    let downloads = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")).ok_or("This PC's Downloads folder isn't known")?;
    let clean: String = from.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    Ok(downloads.join(format!("From {}", clean.trim().trim_end_matches('.'))))
}

/// A sent path made safe as a path below the inbox: its parts, each a name Windows takes, none leading up.
fn safe_parts(path: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = path
        .split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != ".")
        .map(|p| p.chars().map(|c| if r#":*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect::<String>().trim_end_matches(['.', ' ']).to_string())
        .collect();
    if parts.is_empty() || parts.iter().any(|p| p == ".." || p.is_empty()) {
        return Err("That name isn't one this PC takes".into());
    }
    Ok(parts)
}

/// Takes a file's `size` bytes off `recv` into the inbox (a free name: " (2)" and so on): where it went.
async fn receive(from: &str, path: &str, size: u64, recv: &mut iroh::endpoint::RecvStream) -> Result<PathBuf, String> {
    use tokio::io::AsyncWriteExt;
    let parts = safe_parts(path)?;
    let mut folder = inbox(from)?;
    for part in &parts[..parts.len() - 1] {
        folder.push(part);
    }
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let target = crate::fsx::create_numbered(&folder.to_string_lossy(), &parts[parts.len() - 1], false)?;
    let partial = PathBuf::from(format!("{target}.part"));
    let mut file = tokio::fs::File::create(&partial).await.map_err(|e| e.to_string())?;
    let mut left = size;
    let mut buffer = vec![0u8; 256 << 10];
    while left > 0 {
        let want = buffer.len().min(left as usize);
        let n = match recv.read(&mut buffer[..want]).await {
            Ok(Some(n)) if n > 0 => n,
            _ => {
                drop(file);
                let _ = fs::remove_file(&partial);
                let _ = fs::remove_file(&target);
                return Err("The other PC stopped sending".into());
            }
        };
        file.write_all(&buffer[..n]).await.map_err(|e| e.to_string())?;
        left -= n as u64;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    let _ = fs::remove_file(&target);
    fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    Ok(PathBuf::from(target))
}

/// Tells the app's windows a file came (they show it, with Show).
fn announce(from: &str, to: &Path) {
    use crate::host::Emitter;
    if let Some(app) = APP.get() {
        let _ = app.emit("device-received", serde_json::json!({ "from": from, "path": to.to_string_lossy() }));
    }
}

/// How a send is going: files and bytes sent of all, and the one under way.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SendProgress {
    pub files: usize,
    pub total_files: usize,
    pub bytes: u64,
    pub total_bytes: u64,
    pub current: String,
}

/// Sends `paths` (files, and folders with everything in them) to the paired PC `id`: what it says each landed as.
pub async fn devices_send(id: String, paths: Vec<String>, on_progress: crate::host::ipc::Channel<SendProgress>) -> Result<Vec<String>, String> {
    let id: iroh::EndpointId = id.parse().map_err(|_| "That isn't a paired PC")?;
    // Every file to send, with its path in what's sent.
    let files = crate::fsx::blocking(move || Ok(gather(&paths))).await?;
    let total_bytes: u64 = files.iter().map(|(_, _, size)| size).sum();
    let mut progress = SendProgress { files: 0, total_files: files.len(), bytes: 0, total_bytes, current: String::new() };
    let connection = connect(id).await?;
    let mut landed = Vec::new();
    for (full, relative, size) in files {
        use tokio::io::AsyncReadExt;
        progress.current = relative.clone();
        let _ = on_progress.send(progress.clone());
        let (mut send, mut recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
        write_frame(&mut send, &Request::Push { path: relative.clone(), size }).await?;
        let mut file = tokio::fs::File::open(&full).await.map_err(|e| format!("{}: {e}", full.display()))?;
        let mut buffer = vec![0u8; 256 << 10];
        let mut left = size;
        while left > 0 {
            let cap = buffer.len().min(left as usize);
            let n = file.read(&mut buffer[..cap]).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            send.write_all(&buffer[..n]).await.map_err(|e| e.to_string())?;
            left -= n as u64;
            progress.bytes += n as u64;
            let _ = on_progress.send(progress.clone());
        }
        let _ = send.finish();
        let answer: Answer = read_frame(&mut recv, MAX_REQUEST).await.map_err(|_| "That PC didn't take it: update Glint there to receive files".to_string())?;
        if !answer.error.is_empty() {
            return Err(answer.error);
        }
        landed.push(answer.name);
        progress.files += 1;
    }
    let _ = on_progress.send(progress);
    Ok(landed)
}

/// Each file in `paths`, folders walked: (where it is, its path as sent, its size). Links aren't followed.
fn gather(paths: &[String]) -> Vec<(PathBuf, String, u64)> {
    let mut out = Vec::new();
    for path in paths {
        let path = Path::new(path);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        walk(path, &name, &mut out);
    }
    out
}

fn walk(path: &Path, relative: &str, out: &mut Vec<(PathBuf, String, u64)>) {
    let Ok(meta) = fs::symlink_metadata(path) else { return };
    if meta.is_dir() {
        let Ok(items) = fs::read_dir(path) else { return };
        for item in items.flatten() {
            walk(&item.path(), &format!("{relative}/{}", item.file_name().to_string_lossy()), out);
        }
    } else if meta.is_file() {
        out.push((path.to_path_buf(), relative.to_string(), meta.len()));
    }
}

fn wire(path: &Path, name: String) -> Option<Wire> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    Some(Wire { name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, modified })
}

/// A shared path ("Pictures/2024/a.jpg") here: None for the top (the shares), Err for anything that
/// isn't in a share (.., a drive, or a link out of it).
fn local(path: &str, shares: &[Share]) -> Result<Option<PathBuf>, String> {
    let mut parts = path.split(['\\', '/']).filter(|p| !p.is_empty());
    let Some(first) = parts.next() else { return Ok(None) };
    let share = shares.iter().find(|s| s.name.eq_ignore_ascii_case(first)).ok_or("That folder isn't shared")?;
    let mut full = PathBuf::from(&share.path);
    for part in parts {
        if part == ".." || part == "." || part.contains(':') {
            return Err("That isn't in a shared folder".into());
        }
        full.push(part);
    }
    let root = fs::canonicalize(&share.path).map_err(|e| e.to_string())?;
    let real = fs::canonicalize(&full).map_err(|_| "That isn't there".to_string())?;
    if !real.starts_with(&root) {
        return Err("That isn't in a shared folder".into());
    }
    Ok(Some(full))
}

/// Settings → Devices.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub name: String,
    pub sharing: bool,
    pub shares: Vec<Share>,
    pub allowed: Vec<Peer>,
}

pub fn devices_status() -> Status {
    let s = saved();
    Status { name: this_pc(), sharing: s.sharing, shares: s.shares, allowed: s.allowed }
}

pub async fn devices_configure(sharing: bool, shares: Vec<Share>) -> Result<Status, String> {
    let mut named: Vec<Share> = Vec::new();
    for share in shares {
        // Each share's name once: a second "Pictures" is "Pictures (2)".
        let base = if share.name.trim().is_empty() { Path::new(&share.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| share.path.replace([':', '\\'], "")) } else { share.name.trim().to_string() };
        let mut name = base.clone();
        let mut n = 2;
        while named.iter().any(|s| s.name.eq_ignore_ascii_case(&name)) {
            name = format!("{base} ({n})");
            n += 1;
        }
        named.push(Share { name, path: share.path });
    }
    save(|s| {
        s.sharing = sharing;
        s.shares = named;
    })?;
    if sharing {
        endpoint().await?;
    }
    Ok(devices_status())
}

pub fn devices_forget(id: String) -> Result<Status, String> {
    save(|s| s.allowed.retain(|p| p.id != id))?;
    Ok(devices_status())
}

/// A pairing code for another PC to enter: this PC's key and a one-time secret, good for ten minutes.
pub async fn devices_pair_code() -> Result<String, String> {
    let endpoint = endpoint().await?;
    // Ten letters and digits from fresh randomness (a new key's bytes are just that).
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let secret: String = iroh::SecretKey::generate().to_bytes().iter().take(10).map(|b| LETTERS[(*b as usize) % LETTERS.len()] as char).collect();
    *PAIRING.lock().unwrap_or_else(|e| e.into_inner()) = Some((secret.clone(), Instant::now() + PAIRING_LIFE));
    Ok(format!("{}-{secret}", endpoint.id()))
}

/// The PC a pairing code is from, as the browsing PC adds it: its key and its name.
#[derive(Serialize)]
pub struct Paired {
    pub id: String,
    pub name: String,
}

/// Enters another PC's pairing code: from then on this PC browses what it shares.
pub async fn devices_pair(code: String) -> Result<Paired, String> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let (id, secret) = code.rsplit_once('-').ok_or("That isn't a pairing code: it's shown in Settings → Devices on the other PC")?;
    let id: iroh::EndpointId = id.parse().map_err(|_| "That isn't a pairing code: it's shown in Settings → Devices on the other PC".to_string())?;
    let endpoint = endpoint().await?;
    if id == endpoint.id() {
        return Err("That's this PC's own code: enter it on the other PC".into());
    }
    let connection = tokio::time::timeout(Duration::from_secs(30), endpoint.connect(id, ALPN)).await.map_err(|_| "The other PC didn't answer: is Glint open there?".to_string())?.map_err(|e| format!("Couldn't reach the other PC: {e}"))?;
    let (mut send, mut recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
    write_frame(&mut send, &Request::Pair { secret: secret.to_uppercase(), name: this_pc() }).await?;
    let _ = send.finish();
    let answer: Answer = read_frame(&mut recv, MAX_ANSWER).await?;
    if !answer.error.is_empty() {
        return Err(answer.error);
    }
    crate::remote::device_connected(id, connection);
    Ok(Paired { id: id.to_string(), name: answer.name })
}

/// A connection to a paired PC: the one open, or a new one.
pub async fn connect(id: iroh::EndpointId) -> Result<iroh::endpoint::Connection, String> {
    let endpoint = endpoint().await?;
    tokio::time::timeout(Duration::from_secs(30), endpoint.connect(id, ALPN)).await.map_err(|_| "That PC didn't answer: is it on, with Glint open?".to_string())?.map_err(|e| format!("Couldn't reach that PC: {e}"))
}

pub const ANSWER_MOST: usize = MAX_ANSWER;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_to_the_shared_folders() {
        let dir = std::env::temp_dir().join(format!("glint-devices-{}", std::process::id()));
        fs::create_dir_all(dir.join("shared").join("inner")).unwrap();
        fs::write(dir.join("secret.txt"), "no").unwrap();
        let shares = vec![Share { name: "Pictures".into(), path: dir.join("shared").to_string_lossy().into_owned() }];
        assert_eq!(local("", &shares).unwrap(), None);
        assert_eq!(local("pictures/inner", &shares).unwrap(), Some(dir.join("shared").join("inner")));
        assert!(local("Pictures/../secret.txt", &shares).is_err());
        assert!(local("Pictures/C:/Windows", &shares).is_err());
        assert!(local("Other", &shares).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_sent_files_inside_the_inbox() {
        assert_eq!(safe_parts("Trip/day 1.jpg").unwrap(), ["Trip", "day 1.jpg"]);
        assert_eq!(safe_parts("a\\b:c.txt").unwrap(), ["a", "b_c.txt"]);
        assert!(safe_parts("../x.txt").is_err());
        assert!(safe_parts("").is_err());
        assert!(inbox("Desk/PC").unwrap().ends_with("From Desk_PC"));
    }

    #[test]
    fn a_code_pairs_once_in_its_time() {
        *PAIRING.lock().unwrap() = Some(("ABCDEFGHJK".into(), Instant::now() + PAIRING_LIFE));
        assert!(!take_pairing("ABCDEFGHJX"));
        assert!(take_pairing("ABCDEFGHJK"));
        assert!(!take_pairing("ABCDEFGHJK"), "once");
        *PAIRING.lock().unwrap() = Some(("ABCDEFGHJK".into(), Instant::now() - Duration::from_secs(1)));
        assert!(!take_pairing("ABCDEFGHJK"), "past its time");
    }

    /// Two endpoints on this PC, over iroh: pairing with a code, then a shared folder listed, read and
    /// previewed through remote/device.rs, then the same remote in File Explorer on demand (ondemand.rs),
    /// a placeholder read from disk. Needs the network (n0's relays and address lookup):
    /// `cargo test --lib -- --ignored over_iroh`.
    #[test]
    #[ignore]
    fn pairs_and_reads_over_iroh() {
        let dir = std::env::temp_dir().join(format!("glint-devices-e2e-{}", std::process::id()));
        let shared = dir.join("shared");
        fs::create_dir_all(shared.join("inner")).unwrap();
        fs::write(shared.join("hello.txt"), "hello over iroh").unwrap();
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(shared.join("inner").join("big.bin"), &big).unwrap();
        crate::tools::set_dir(dir.join("tools"));
        let server_id = crate::host::async_runtime::block_on(async {
            // The sharing PC: an endpoint of its own, answering as Glint does.
            let server = iroh::Endpoint::builder(iroh::endpoint::presets::N0).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
            let _ = tokio::time::timeout(Duration::from_secs(20), server.online()).await;
            crate::host::async_runtime::spawn(serve(server.clone()));
            save(|s| {
                s.sharing = true;
                s.shares = vec![Share { name: "Pictures".into(), path: shared.to_string_lossy().into_owned() }];
            })
            .unwrap();
            *PAIRING.lock().unwrap() = Some(("ABCDEFGHJK".into(), Instant::now() + PAIRING_LIFE));
            // The browsing PC: Glint's own endpoint, pairing with the code.
            let client = endpoint().await.unwrap();
            let connection = client.connect(server.addr(), ALPN).await.unwrap();
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            write_frame(&mut send, &Request::Pair { secret: "ABCDEFGHJK".into(), name: "Test PC".into() }).await.unwrap();
            let _ = send.finish();
            let answer: Answer = read_frame(&mut recv, MAX_ANSWER).await.unwrap();
            assert!(answer.error.is_empty(), "{}", answer.error);
            assert_eq!(answer.name, this_pc());
            assert!(saved().allowed.iter().any(|p| p.id == client.id().to_string() && p.name == "Test PC"), "the browsing PC is let in");
            crate::remote::device_connected(server.id(), connection);
            let peer = crate::remote::device::Peer { id: server.id() };
            let names = |items: Vec<crate::remote::Item>| {
                let mut n: Vec<String> = items.into_iter().map(|i| format!("{}{}", i.name, if i.dir { "\\" } else { "" })).collect();
                n.sort();
                n
            };
            assert_eq!(names(peer.list("").await.unwrap()), ["Pictures\\"]);
            assert_eq!(names(peer.list("Pictures").await.unwrap()), ["hello.txt", "inner\\"]);
            assert_eq!(peer.stat("Pictures/hello.txt").await.unwrap().map(|i| i.size), Some(15));
            assert_eq!(peer.stat("Pictures/missing.txt").await.unwrap(), None);
            assert!(peer.list("Pictures/../..").await.is_err(), "nothing outside a share");
            let copy = dir.join("big copy.bin");
            let mut told = 0u64;
            let mut tick = |n: u64| {
                told += n;
                true
            };
            assert_eq!(peer.fetch("Pictures/inner/big.bin", &copy, Some(big.len() as u64), &mut tick).await.unwrap(), big.len() as u64);
            assert_eq!(told, big.len() as u64);
            assert_eq!(fs::read(&copy).unwrap(), big);
            assert_eq!(peer.prefix("Pictures/hello.txt", 5).await.unwrap(), (b"hello".to_vec(), true));
            assert_eq!(peer.prefix("Pictures/inner/big.bin", 10).await.unwrap().0, big[..10].to_vec());
            server.id()
        });
        // The same PC in File Explorer, on demand: a placeholder's bytes come when it's read.
        crate::remote::remote_configure(vec![crate::remote::Connection { id: "testpc".into(), name: "Test PC".into(), kind: "device".into(), host: server_id.to_string(), ..Default::default() }]);
        let folder = dir.join("On demand");
        let mounted = crate::ondemand::mount_folder("testpc", &folder);
        if let Err(e) = &mounted {
            eprintln!("Files On-Demand couldn't be tried here: {e}");
        } else {
            // Read by another process, as File Explorer and apps do: Windows asks the provider (this one).
            let outside = |args: &[&std::ffi::OsStr]| std::process::Command::new("cmd").arg("/c").args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
            let listed = outside(&["dir".as_ref(), "/b".as_ref(), folder.join("Pictures").as_os_str()]);
            let read = outside(&["type".as_ref(), folder.join("Pictures").join("hello.txt").as_os_str()]);
            let copied = dir.join("copied.bin");
            let _ = outside(&["copy".as_ref(), "/b".as_ref(), folder.join("Pictures").join("inner").join("big.bin").as_os_str(), copied.as_os_str()]);
            let big_read = fs::read(&copied);
            crate::ondemand::unmount_folder("testpc", &folder).unwrap();
            let mut names: Vec<String> = listed.unwrap().lines().map(str::to_string).collect();
            names.sort();
            assert_eq!(names, ["hello.txt", "inner"]);
            assert_eq!(read.unwrap(), "hello over iroh");
            assert_eq!(big_read.unwrap(), big);
        }
        // Forgotten: let in no more.
        save(|s| s.allowed.clear()).unwrap();
        let refused = crate::host::async_runtime::block_on(crate::remote::device::Peer { id: server_id }.list(""));
        assert!(refused.is_err_and(|e| e.contains("isn't paired")));
        let _ = fs::remove_dir_all(&dir);
        mounted.unwrap();
    }

    #[test]
    fn keys_go_to_hex_and_back() {
        let key = iroh::SecretKey::generate();
        assert_eq!(unhex(&hex(&key.to_bytes())), Some(key.to_bytes()));
        assert_eq!(unhex("zz"), None);
    }
}
