//! Phones and computers nearby, through the LocalSend protocol (v2, an open protocol: github.com/localsend/
//! protocol): Glint finds the LocalSend apps on the same network (Android, iOS, macOS, Linux, Windows) by
//! their multicast announcements and by asking each address of the local network, and sends files to
//! them; while the Nearby dialog lets it, Glint shows up in their lists as "Glint on <PC>" and takes the
//! files they send into Downloads\LocalSend. No account and no internet: everything stays on the network.
//!
//! Glint's own side speaks plain HTTP (the protocol lets a device say so); it talks to the apps over the
//! HTTPS they announce, whose certificates are their own, made by each device (as LocalSend's apps do,
//! they're taken as they are: the network is the trust).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use crate::host::ipc::Channel;
use crate::host::{AppHandle, Event};

const MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);
const PORT: u16 = 53317;
const API: &str = "/api/localsend/v2";

/// What a device says of itself.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub alias: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub download: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
}

fn default_port() -> u16 {
    PORT
}

fn default_protocol() -> String {
    "https".into()
}

/// A device found: what it says, and where it is.
#[derive(Clone, Debug)]
pub struct Peer {
    pub info: Info,
    pub ip: String,
}

/// This PC's own fingerprint for a run (LocalSend's apps tell devices apart by it).
static FINGERPRINT: LazyLock<String> = LazyLock::new(|| {
    let mut bytes = [0u8; 32];
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()) ^ u128::from(std::process::id());
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (seed.wrapping_mul(6364136223846793005).wrapping_add(i as u128 * 1442695040888963407) >> (i % 16 * 8)) as u8;
    }
    bytes.iter().map(|b| format!("{b:02X}")).collect()
});

fn own_info(port: u16, announce: Option<bool>) -> Info {
    Info {
        alias: format!("Glint on {}", std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())),
        version: "2.1".into(),
        device_model: Some("Glint".into()),
        device_type: Some("desktop".into()),
        fingerprint: FINGERPRINT.clone(),
        port,
        protocol: "http".into(),
        download: false,
        announce,
    }
}

fn client() -> Result<reqwest::Client, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_millis(900))
        .build()
        .map_err(|e| e.to_string())
}

/// The local network's addresses to ask: each private IPv4 /24 this PC is on, but its own.
fn neighbors() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for own in local_ipv4() {
        let [a, b, c, me] = own.octets();
        for d in 1..=254u8 {
            if d != me {
                out.push(Ipv4Addr::new(a, b, c, d));
            }
        }
    }
    out
}

/// This PC's private IPv4 addresses (the one the default route goes out of first).
fn local_ipv4() -> Vec<Ipv4Addr> {
    let mut found = Vec::new();
    // The address a connection out would use (no packet is sent for a UDP "connect").
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("192.168.255.255:9").is_ok() || socket.connect("10.255.255.255:9").is_ok() {
            if let Ok(SocketAddr::V4(addr)) = socket.local_addr() {
                if addr.ip().is_private() {
                    found.push(*addr.ip());
                }
            }
        }
    }
    found
}

/// Devices nearby: those answering a multicast announcement and those answering at their address on the
/// local network, within `millis`.
pub async fn nearby_scan(millis: u64) -> Result<Vec<Peer>, String> {
    let found: Arc<Mutex<HashMap<String, Peer>>> = Arc::default();
    let millis = millis.clamp(500, 15_000);
    // Multicast: announce, and listen for answers on LocalSend's port (when nothing else holds it).
    let heard = found.clone();
    let listening = crate::host::async_runtime::spawn_blocking(move || multicast_round(&heard, Duration::from_millis(millis)));
    // Each address of the local network, asked as LocalSend's apps ask when multicast is blocked.
    let client = client()?;
    let body = serde_json::to_string(&own_info(receiving_port().unwrap_or(PORT), None)).map_err(|e| e.to_string())?;
    let asked = found.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(millis);
    let mut tasks = Vec::new();
    for ip in neighbors() {
        let (client, body, asked) = (client.clone(), body.clone(), asked.clone());
        tasks.push(crate::host::async_runtime::spawn(async move {
            for scheme in ["https", "http"] {
                let url = format!("{scheme}://{ip}:{PORT}{API}/register");
                let sent = tokio::time::timeout_at(deadline, client.post(&url).header("content-type", "application/json").body(body.clone()).send()).await;
                let Ok(Ok(response)) = sent else { continue };
                if let Ok(mut info) = response.json::<Info>().await {
                    info.protocol = scheme.into();
                    if info.fingerprint != *FINGERPRINT {
                        asked.lock().unwrap_or_else(|e| e.into_inner()).insert(info.fingerprint.clone(), Peer { info, ip: ip.to_string() });
                    }
                }
                break;
            }
        }));
    }
    for task in tasks {
        let _ = task.await;
    }
    let _ = listening.await;
    let mut peers: Vec<Peer> = found.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
    peers.sort_by_key(|p| p.info.alias.to_lowercase());
    Ok(peers)
}

/// One multicast announcement, then what's heard back for `wait`.
fn multicast_round(found: &Mutex<HashMap<String, Peer>>, wait: Duration) {
    let Ok(socket) = multicast_socket() else { return };
    let announcement = serde_json::to_vec(&own_info(receiving_port().unwrap_or(PORT), Some(true))).unwrap_or_default();
    let _ = socket.send_to(&announcement, (MULTICAST, PORT));
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let until = std::time::Instant::now() + wait;
    let mut buffer = [0u8; 4096];
    while std::time::Instant::now() < until {
        if let Ok((n, from)) = socket.recv_from(&mut buffer) {
            if let Ok(info) = serde_json::from_slice::<Info>(&buffer[..n]) {
                if info.fingerprint != *FINGERPRINT {
                    found.lock().unwrap_or_else(|e| e.into_inner()).insert(info.fingerprint.clone(), Peer { info, ip: from.ip().to_string() });
                }
            }
        }
    }
}

/// A UDP socket on LocalSend's port, in its multicast group (shared with a LocalSend app on this PC).
fn multicast_socket() -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    for own in local_ipv4() {
        let _ = socket.join_multicast_v4(&MULTICAST, &own);
    }
    let _ = socket.join_multicast_v4(&MULTICAST, &Ipv4Addr::UNSPECIFIED);
    socket.set_multicast_loop_v4(false)?;
    Ok(socket.into())
}

/// How a send is going.
#[derive(Clone)]
pub struct SendProgress {
    pub files: usize,
    pub total_files: usize,
    pub bytes: u64,
    pub total_bytes: u64,
    pub current: String,
}

static STOP: AtomicBool = AtomicBool::new(false);

pub fn nearby_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Sends `paths` (files, and folders with everything in them) to `peer`; the device asks its person
/// first, as LocalSend's apps do. How many files went.
pub async fn nearby_send(peer: Peer, paths: Vec<String>, on_progress: Channel<SendProgress>) -> Result<usize, String> {
    STOP.store(false, Ordering::SeqCst);
    let files = crate::fsx::blocking(move || Ok(gather(&paths))).await?;
    if files.is_empty() {
        return Err("Nothing to send".into());
    }
    let base = format!("{}://{}:{}{API}", peer.info.protocol, peer.ip, peer.info.port);
    let client = client()?;
    let metadata: serde_json::Map<String, serde_json::Value> = files
        .iter()
        .enumerate()
        .map(|(i, (_, name, size))| (i.to_string(), serde_json::json!({ "id": i.to_string(), "fileName": name, "size": size, "fileType": mime(name) })))
        .collect();
    let prepare = serde_json::json!({ "info": own_info(receiving_port().unwrap_or(PORT), None), "files": metadata });
    // The person on the other side has a while to say yes.
    let response = client.post(format!("{base}/prepare-upload")).timeout(Duration::from_secs(120)).json(&prepare).send().await.map_err(|e| format!("Couldn't reach {}: {e}", peer.info.alias))?;
    match response.status().as_u16() {
        200 => {}
        204 => return Ok(0),
        403 => return Err(format!("{} said no", peer.info.alias)),
        409 => return Err(format!("{} is busy with another transfer", peer.info.alias)),
        429 => return Err(format!("{} asks to wait a moment", peer.info.alias)),
        code => return Err(format!("{} answered {code}", peer.info.alias)),
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Prepared {
        session_id: String,
        files: HashMap<String, String>,
    }
    let prepared: Prepared = response.json().await.map_err(|e| e.to_string())?;
    let total_bytes: u64 = files.iter().map(|(_, _, s)| s).sum();
    let mut progress = SendProgress { files: 0, total_files: files.len(), bytes: 0, total_bytes, current: String::new() };
    for (i, (path, name, size)) in files.iter().enumerate() {
        let Some(token) = prepared.files.get(&i.to_string()) else { continue };
        if STOP.load(Ordering::SeqCst) {
            let _ = client.post(format!("{base}/cancel?sessionId={}", prepared.session_id)).send().await;
            return Err("cancelled".into());
        }
        progress.current = name.clone();
        let _ = on_progress.send(progress.clone());
        let file = tokio::fs::File::open(path).await.map_err(|e| format!("{}: {e}", path.display()))?;
        let sent_before = progress.bytes;
        let channel = on_progress.clone();
        let mut shown = progress.clone();
        let stream = futures_util::StreamExt::map(tokio_util::io::ReaderStream::with_capacity(file, 256 << 10), move |chunk| {
            if let Ok(bytes) = &chunk {
                shown.bytes += bytes.len() as u64;
                let _ = channel.send(shown.clone());
            }
            chunk
        });
        let url = format!("{base}/upload?sessionId={}&fileId={i}&token={}", prepared.session_id, token);
        let answer = client
            .post(url)
            .header("content-length", size.to_string())
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| format!("The transfer broke off: {e}"))?;
        if !answer.status().is_success() {
            return Err(format!("{} refused “{name}” ({})", peer.info.alias, answer.status().as_u16()));
        }
        progress.bytes = sent_before + size;
        progress.files += 1;
        let _ = on_progress.send(progress.clone());
    }
    Ok(progress.files)
}

/// A file type for the other app's icon.
fn mime(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "zip" => "application/zip",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

/// Each file in `paths`, folders walked: (where it is, its name as sent, with its folders, its size).
fn gather(paths: &[String]) -> Vec<(PathBuf, String, u64)> {
    fn walk(path: &Path, relative: &str, out: &mut Vec<(PathBuf, String, u64)>) {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        if meta.is_dir() {
            for item in std::fs::read_dir(path).into_iter().flatten().flatten() {
                walk(&item.path(), &format!("{relative}/{}", item.file_name().to_string_lossy()), out);
            }
        } else if meta.is_file() {
            out.push((path.to_path_buf(), relative.to_string(), meta.len()));
        }
    }
    let mut out = Vec::new();
    for path in paths {
        let path = Path::new(path);
        walk(path, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &mut out);
    }
    out
}

// Receiving.

/// The receiver while it runs: its port, and what stops it.
struct Receiver {
    port: u16,
    stop: Arc<AtomicBool>,
}

static RECEIVER: Mutex<Option<Receiver>> = Mutex::new(None);

fn receiving_port() -> Option<u16> {
    RECEIVER.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.port)
}

/// An upload session: the files it may bring, by id, with their tokens and names.
struct Session {
    from: String,
    files: HashMap<String, (String, String, u64)>,
}

static SESSIONS: LazyLock<Mutex<HashMap<String, Session>>> = LazyLock::new(Default::default);

/// Where received files go: Downloads\LocalSend.
fn inbox() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads").join("LocalSend"))
}

/// Starts (or stops) taking files from LocalSend's apps nearby: this PC answers their announcements and
/// shows up in their lists. The port it listens on.
pub async fn nearby_receive(app: AppHandle, on: bool) -> Result<Option<u16>, String> {
    let mut slot = RECEIVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = slot.take() {
        old.stop.store(true, Ordering::SeqCst);
    }
    if !on {
        return Ok(None);
    }
    // LocalSend's port, or the next ones when a LocalSend app on this PC has it.
    let listener = (PORT..PORT + 10).find_map(|p| std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).ok()).ok_or("No port to listen on")?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    *slot = Some(Receiver { port, stop: stop.clone() });
    drop(slot);
    let listener = tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
    let (serving, answering) = (stop.clone(), stop.clone());
    let app_for_server = app.clone();
    crate::host::async_runtime::spawn(async move { serve(listener, serving, app_for_server).await });
    std::thread::spawn(move || answer_announcements(port, &answering));
    Ok(Some(port))
}

/// Answers devices' multicast announcements with this PC's own, so they list it.
fn answer_announcements(port: u16, stop: &AtomicBool) {
    let Ok(socket) = multicast_socket() else { return };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = socket.send_to(&serde_json::to_vec(&own_info(port, Some(true))).unwrap_or_default(), (MULTICAST, PORT));
    let mut buffer = [0u8; 4096];
    while !stop.load(Ordering::SeqCst) {
        if let Ok((n, _)) = socket.recv_from(&mut buffer) {
            if let Ok(info) = serde_json::from_slice::<Info>(&buffer[..n]) {
                if info.announce == Some(true) && info.fingerprint != *FINGERPRINT {
                    let _ = socket.send_to(&serde_json::to_vec(&own_info(port, Some(false))).unwrap_or_default(), (MULTICAST, PORT));
                }
            }
        }
    }
}

async fn serve(listener: tokio::net::TcpListener, stop: Arc<AtomicBool>, app: AppHandle) {
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(PORT);
    while !stop.load(Ordering::SeqCst) {
        let Ok(Ok((stream, from))) = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await else { continue };
        let app = app.clone();
        crate::host::async_runtime::spawn(async move {
            let service = service_fn(move |request| route(request, from.ip(), port, app.clone()));
            let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
        });
    }
}

type Reply = hyper::Response<http_body_util::Full<bytes::Bytes>>;

fn reply(status: u16, body: impl Into<bytes::Bytes>) -> Reply {
    hyper::Response::builder().status(status).header("content-type", "application/json").body(http_body_util::Full::new(body.into())).unwrap_or_default()
}

fn query_of(uri: &hyper::Uri) -> HashMap<String, String> {
    uri.query().unwrap_or_default().split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

async fn route(request: hyper::Request<hyper::body::Incoming>, from: IpAddr, port: u16, app: AppHandle) -> Result<Reply, std::convert::Infallible> {
    use http_body_util::BodyExt;
    let path = request.uri().path().to_string();
    let query = query_of(request.uri());
    Ok(match path.strip_prefix(API).unwrap_or("") {
        "/register" | "/info" => reply(200, serde_json::to_vec(&own_info(port, None)).unwrap_or_default()),
        "/prepare-upload" => {
            let Ok(body) = http_body_util::Limited::new(request.into_body(), 4 << 20).collect().await else { return Ok(reply(400, "{}")) };
            #[derive(Deserialize)]
            struct File {
                id: String,
                #[serde(rename = "fileName")]
                file_name: String,
                size: u64,
            }
            #[derive(Deserialize)]
            struct Prepare {
                info: Info,
                files: HashMap<String, File>,
            }
            let Ok(prepare) = serde_json::from_slice::<Prepare>(&body.to_bytes()) else { return Ok(reply(400, "{}")) };
            let session_id = token();
            let mut tokens = serde_json::Map::new();
            let mut files = HashMap::new();
            for (_, f) in prepare.files {
                let t = token();
                tokens.insert(f.id.clone(), serde_json::Value::String(t.clone()));
                files.insert(f.id, (t, f.file_name, f.size));
            }
            let from_name = format!("{} ({from})", prepare.info.alias);
            SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id.clone(), Session { from: from_name, files });
            reply(200, serde_json::to_vec(&serde_json::json!({ "sessionId": session_id, "files": tokens })).unwrap_or_default())
        }
        "/upload" => {
            let (session_id, file_id, token_given) = (query.get("sessionId").cloned().unwrap_or_default(), query.get("fileId").cloned().unwrap_or_default(), query.get("token").cloned().unwrap_or_default());
            let found = SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).get(&session_id).and_then(|s| s.files.get(&file_id).map(|f| (s.from.clone(), f.clone())));
            let Some((from_name, (token_kept, name, size))) = found else { return Ok(reply(403, "{}")) };
            if token_kept != token_given {
                return Ok(reply(403, "{}"));
            }
            match save(request.into_body(), &name, size).await {
                Ok(to) => {
                    app.emit(Event::NearbyReceived { from: from_name, path: to.to_string_lossy().into_owned() });
                    reply(200, "{}")
                }
                Err(e) => reply(500, serde_json::to_vec(&serde_json::json!({ "message": e })).unwrap_or_default()),
            }
        }
        "/cancel" => {
            if let Some(id) = query.get("sessionId") {
                SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            }
            reply(200, "{}")
        }
        _ => reply(404, "{}"),
    })
}

/// A file's bytes off the request into the inbox (a free name, its folders kept inside it).
async fn save(mut body: hyper::body::Incoming, name: &str, size: u64) -> Result<PathBuf, String> {
    use http_body_util::BodyExt;
    use tokio::io::AsyncWriteExt;
    let parts: Vec<String> = name
        .split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(|p| p.chars().map(|c| if r#":*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect::<String>().trim_end_matches(['.', ' ']).to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let Some((leaf, folders)) = parts.split_last() else { return Err("No name".into()) };
    let mut folder = inbox().ok_or("Downloads isn't known")?;
    for f in folders {
        folder.push(f);
    }
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let target = crate::fsx::create_numbered(&folder.to_string_lossy(), leaf, false)?;
    let partial = PathBuf::from(format!("{target}.part"));
    let mut file = tokio::fs::File::create(&partial).await.map_err(|e| e.to_string())?;
    let mut got = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| e.to_string())?;
        if let Some(data) = frame.data_ref() {
            got += data.len() as u64;
            if got > size.saturating_add(1 << 20) {
                drop(file);
                let _ = std::fs::remove_file(&partial);
                let _ = std::fs::remove_file(&target);
                return Err("More came than was said".into());
            }
            file.write_all(data).await.map_err(|e| e.to_string())?;
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    Ok(PathBuf::from(target))
}

/// A random token (sessions, files).
fn token() -> String {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mixed = (t ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    format!("{mixed:016x}{:08x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_devices_say() {
        let info: Info = serde_json::from_str(r#"{"alias":"Pixel 9","version":"2.1","deviceModel":"Google Pixel","deviceType":"mobile","fingerprint":"AB12","port":53317,"protocol":"https","download":false,"announce":true}"#).unwrap();
        assert_eq!((info.alias.as_str(), info.port, info.protocol.as_str(), info.announce), ("Pixel 9", 53317, "https", Some(true)));
        // An older app's answer, without the newer fields.
        let old: Info = serde_json::from_str(r#"{"alias":"Mac","fingerprint":"CD"}"#).unwrap();
        assert_eq!((old.port, old.protocol.as_str()), (53317, "https"));
        let own = serde_json::to_value(own_info(53317, None)).unwrap();
        assert_eq!(own["protocol"], "http");
        assert!(own.get("announce").is_none());
        assert_eq!(mime("IMG_1.JPG"), "image/jpeg");
        assert_ne!(token(), token());
    }
}
