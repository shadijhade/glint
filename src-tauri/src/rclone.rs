//! rclone (rclone.org), when it's installed (Settings → Tools): more than 60 cloud storage services
//! (Google Drive, OneDrive, Dropbox, Box, MEGA, S3…) as remote connections, signed in to from Glint,
//! and folders kept in sync with them (folder rules, rules.rs).
//!
//! Browsing goes through one `rclone rcd`, its remote control: started when first needed, on a port of
//! its own on 127.0.0.1 with a password made for that run, and ended with Glint. A file's bytes come
//! from its `--rc-serve`, ranges and all. Signing in is rclone's own `config` steps, asked one at a time
//! (config/create's non-interactive mode); a browser sign-in is `rclone authorize`'s, its page opened
//! by Glint. Remotes live in rclone's own configuration file, so the ones made in a terminal show here
//! too, and the other way round; their secrets (tokens, passwords) never leave rclone.

use crate::fsx::blocking;
use crate::proc;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tauri::{Emitter, Window};

/// The remote control's user name (its password is made at each start).
const USER: &str = "glint";

/// What an error says when rclone's configuration is encrypted and Glint hasn't its password: the
/// app asks for it (`rclone_unlock`).
pub const LOCKED: &str = "rclone's configuration is encrypted: its password unlocks it";

/// rclone's remote control, while it runs.
struct Daemon {
    port: u16,
    password: String,
    child: Mutex<Child>,
    /// Ends it with Glint, however Glint ends.
    _job: Mutex<proc::Job>,
}

impl Daemon {
    fn alive(&self) -> bool {
        lock(&self.child).try_wait().is_ok_and(|status| status.is_none())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let mut child = lock(&self.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

static DAEMON: Mutex<Option<Arc<Daemon>>> = Mutex::new(None);
/// The configuration's password, when it's encrypted and was given (for this run of Glint only).
static CONFIG_PASS: Mutex<Option<String>> = Mutex::new(None);
/// rclone's last words, for an error that needs them.
static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// A browser sign-in waiting for its page (`rclone authorize`).
static AUTHORIZING: Mutex<Option<Child>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn remember(line: String) {
    let mut said = lock(&SAID);
    said.push(line);
    let extra = said.len().saturating_sub(40);
    said.drain(..extra);
}

/// What rclone said last that reads as a reason (its ERROR or NOTICE lines), after ": ".
fn last_said() -> String {
    lock(&SAID)
        .iter()
        .rev()
        .find(|l| l.contains("ERROR") || l.contains("CRITICAL") || l.contains("Failed"))
        .map(|l| format!(": {}", l.split_once(" : ").map_or(l.as_str(), |(_, said)| said).trim()))
        .unwrap_or_default()
}

fn random_hex(bytes: usize) -> Result<String, String> {
    #[cfg(windows)]
    {
        use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        let mut buffer = vec![0u8; bytes];
        unsafe { BCryptGenRandom(None, &mut buffer, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(|e| e.message())?;
        Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(not(windows))]
    {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        Ok(format!("{:0width$x}", seed ^ std::process::id() as u128, width = bytes * 2))
    }
}

/// "… NOTICE: Serving remote control on http://127.0.0.1:55917/" → 55917.
fn served_on(line: &str) -> Option<u16> {
    const SERVING: &str = "Serving remote control on http://127.0.0.1:";
    let at = line.find(SERVING)? + SERVING.len();
    line[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Starts the remote control and waits for its address.
fn start() -> Result<Daemon, String> {
    let password = random_hex(24)?;
    let mut command = crate::tools::command("rclone")?;
    command
        .args(["rcd", "--rc-addr", "127.0.0.1:0", "--rc-serve", "--ask-password=false", "--rc-job-expire-duration", "1h"])
        .env("RCLONE_RC_USER", USER)
        .env("RCLONE_RC_PASS", &password)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(pass) = lock(&CONFIG_PASS).clone() {
        command.env("RCLONE_CONFIG_PASS", pass);
    }
    let mut child = command.spawn().map_err(|e| format!("rclone didn't start: {e}"))?;
    let job = proc::Job::ending()?;
    job.assign(&child)?;
    let stderr = child.stderr.take().ok_or("rclone didn't start")?;
    let (found, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(port) = served_on(&line) {
                let _ = found.send(port);
            }
            remember(line);
        }
    });
    match port.recv_timeout(Duration::from_secs(20)) {
        Ok(port) => Ok(Daemon { port, password, child: Mutex::new(child), _job: Mutex::new(job) }),
        Err(_) => {
            let _ = child.kill();
            Err(format!("rclone didn't start{}", last_said()))
        }
    }
}

/// The remote control, started if it isn't running (or has stopped).
fn daemon() -> Result<Arc<Daemon>, String> {
    let mut slot = lock(&DAEMON);
    if let Some(daemon) = slot.as_ref().filter(|d| d.alive()) {
        return Ok(daemon.clone());
    }
    let daemon = Arc::new(start()?);
    *slot = Some(daemon.clone());
    Ok(daemon)
}

/// Ends the remote control (it starts again when next needed): with another configuration password.
fn stop() {
    lock(&DAEMON).take();
}

static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    // reqwest's TLS (unused on 127.0.0.1, but made with every client) needs its crypto chosen first.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(5)).build().map_err(|e| e.to_string())
});

fn client() -> Result<&'static reqwest::Client, String> {
    CLIENT.as_ref().map_err(Clone::clone)
}

/// An error as rclone gave it, said plainly: its encrypted configuration's as LOCKED.
fn said(error: &str) -> String {
    let error = error.trim();
    if error.contains("unable to decrypt configuration") || error.contains("not allowed to ask for password") || error.contains("RCLONE_CONFIG_PASS") {
        return LOCKED.into();
    }
    let mut chars = error.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Calls the remote control: `method` with `input`, its answer.
pub async fn rc(method: &str, input: Value) -> Result<Value, String> {
    let daemon = blocking(daemon).await?;
    let response = client()?
        .post(format!("http://127.0.0.1:{}/{method}", daemon.port))
        .basic_auth(USER, Some(&daemon.password))
        .header("content-type", "application/json")
        .body(input.to_string())
        .send()
        .await
        .map_err(|e| format!("rclone didn't answer: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.is_success() {
        Ok(answer)
    } else {
        Err(said(answer.get("error").and_then(Value::as_str).unwrap_or(&text)))
    }
}

/// What a URL's path can't hold as it is (the remote's own ":" and "/" can).
const IN_URL: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'|')
    .add(b'\\');

/// rclone's name for a folder on a remote: "gdrive:" or "gdrive:Photos/2024".
pub fn fs(remote: &str, root: &str) -> String {
    format!("{remote}:{}", root.replace('\\', "/").trim_matches('/'))
}

/// A file's bytes from the remote control's `--rc-serve`: all of them, or the `first` ones (a range).
pub async fn serve(fs: &str, path: &str, first: Option<u64>) -> Result<reqwest::Response, String> {
    let daemon = blocking(daemon).await?;
    let path: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(|p| percent_encoding::utf8_percent_encode(p, IN_URL).to_string()).collect();
    let url = format!("http://127.0.0.1:{}/[{}]/{}", daemon.port, percent_encoding::utf8_percent_encode(fs, IN_URL), path.join("/"));
    let mut request = client()?.get(url).basic_auth(USER, Some(&daemon.password));
    if let Some(n) = first {
        request = request.header("range", format!("bytes=0-{}", n.saturating_sub(1)));
    }
    let response = request.send().await.map_err(|e| format!("rclone didn't answer: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(match status.as_u16() {
        404 => crate::remote::NOT_THERE.into(),
        416 => crate::remote::UNSATISFIABLE.into(),
        _ => {
            let text = response.text().await.unwrap_or_default();
            let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            said(answer.get("error").and_then(Value::as_str).unwrap_or(&text))
        }
    })
}

/// 2000-01-01T00:00:00Z, which rclone gives a folder that has no date of its own.
const UNDATED: i64 = 946_684_800_000;

/// An item as the remote control lists it: (name, folder, size, ms since 1970).
pub fn item_of(value: &Value) -> Option<(String, bool, u64, i64)> {
    let name = value.get("Name")?.as_str()?.to_string();
    let dir = value.get("IsDir").and_then(Value::as_bool).unwrap_or(false);
    // Google's documents have no size until they're exported (-1).
    let size = value.get("Size").and_then(Value::as_i64).unwrap_or(0).max(0) as u64;
    let modified = value.get("ModTime").and_then(Value::as_str).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map_or(0, |t| t.timestamp_millis().max(0));
    // A folder whose service gives it no date has 2000-01-01 from rclone: none.
    let modified = if dir && modified == UNDATED { 0 } else { modified };
    Some((name, dir, size, modified))
}

/// A question rclone asks while setting a remote up, or one of a service's settings.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub name: String,
    pub help: String,
    /// "bool", "string", "int", "SizeSuffix", "Duration", "CommaSepList"…
    pub kind: String,
    pub default: String,
    pub examples: Vec<Example>,
    /// Only one of the examples will do.
    pub exclusive: bool,
    pub required: bool,
    pub password: bool,
    /// The services (S3's) it's for, as rclone writes it ("AWS,Minio", "!AWS"): "" for all.
    pub provider: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub value: String,
    pub help: String,
    pub provider: String,
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn question(option: &Value) -> Question {
    let field = |name: &str| option.get(name);
    let flag = |name: &str| field(name).and_then(Value::as_bool).unwrap_or(false);
    let default = match field("DefaultStr").and_then(Value::as_str) {
        Some(text) => text.to_string(),
        None => text_of(field("Default")),
    };
    Question {
        name: text_of(field("Name")),
        help: text_of(field("Help")),
        kind: text_of(field("Type")),
        default,
        examples: field("Examples")
            .and_then(Value::as_array)
            .map(|all| all.iter().map(|e| Example { value: text_of(e.get("Value")), help: text_of(e.get("Help")), provider: text_of(e.get("Provider")) }).collect())
            .unwrap_or_default(),
        exclusive: flag("Exclusive"),
        required: flag("Required"),
        password: flag("IsPassword") || (flag("Sensitive") && text_of(field("Name")).contains("secret")),
        provider: text_of(field("Provider")),
    }
}

/// A kind of storage rclone reaches, and the settings it asks for first (the advanced ones aside).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// Its type in rclone: "drive", "onedrive", "s3"…
    pub name: String,
    pub description: String,
    /// It's signed in to in a browser.
    pub oauth: bool,
    pub options: Vec<Question>,
}

static PROVIDERS: Mutex<Option<Vec<Provider>>> = Mutex::new(None);

/// The kinds of storage rclone reaches (read once a run: its answer is large).
#[tauri::command]
pub async fn rclone_providers() -> Result<Vec<Provider>, String> {
    if let Some(known) = lock(&PROVIDERS).clone() {
        return Ok(known);
    }
    let answer = rc("config/providers", json!({})).await?;
    let providers: Vec<Provider> = answer
        .get("providers")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .map(|p| {
                    let options: Vec<&Value> = p.get("Options").and_then(Value::as_array).map(|o| o.iter().collect()).unwrap_or_default();
                    Provider {
                        name: text_of(p.get("Name")),
                        description: text_of(p.get("Description")),
                        oauth: options.iter().any(|o| o.get("Name").and_then(Value::as_str) == Some("token")),
                        options: options
                            .iter()
                            .filter(|o| !o.get("Advanced").and_then(Value::as_bool).unwrap_or(false) && o.get("Hide").and_then(Value::as_i64).unwrap_or(0) == 0)
                            .map(|o| question(o))
                            .collect(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    *lock(&PROVIDERS) = Some(providers.clone());
    Ok(providers)
}

/// A step of setting a remote up: rclone's next question, or (with no question and no state) done.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub state: String,
    pub question: Option<Question>,
    /// Why the last answer wasn't taken (rclone asks again).
    pub error: String,
}

fn step(answer: Value) -> Step {
    Step {
        state: text_of(answer.get("State")),
        question: answer.get("Option").filter(|o| !o.is_null()).map(question),
        error: text_of(answer.get("Error")),
    }
}

/// Starts setting up the remote `name` of the type `kind` with the settings asked first: rclone's
/// first question, or done. Passwords are obscured by rclone, as its own `config` does.
#[tauri::command]
pub async fn rclone_config_start(name: String, kind: String, parameters: Map<String, Value>) -> Result<Step, String> {
    let answer = rc("config/create", json!({ "name": name, "type": kind, "parameters": parameters, "opt": { "nonInteractive": true, "obscure": true } })).await?;
    Ok(step(answer))
}

/// Answers rclone's question at `state`: its next one, or done.
#[tauri::command]
pub async fn rclone_config_answer(name: String, state: String, answer: String) -> Result<Step, String> {
    let answer = rc("config/update", json!({ "name": name, "parameters": {}, "opt": { "nonInteractive": true, "obscure": true, "continue": true, "state": state, "result": answer } })).await?;
    Ok(step(answer))
}

/// The arguments rclone's own instructions give `rclone authorize` ("drive", and the settings it was
/// given, encoded), from its question's help: `rclone authorize "drive" "eyJzY29w…"`.
fn authorize_args(help: &str) -> Option<Vec<String>> {
    let rest = &help[help.find("rclone authorize")? + "rclone authorize".len()..];
    let line = rest.lines().next()?;
    let args: Vec<String> = line.split('"').skip(1).step_by(2).map(str::to_string).collect();
    (!args.is_empty()).then_some(args)
}

/// What `rclone authorize` gives to paste back: the text between its markers.
fn token_in(output: &str) -> Option<String> {
    let start = output.find("--->")? + 4;
    let end = start + output[start..].find("<---")?;
    let token = output[start..end].trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// A browser sign-in, as rclone's question at `help` says to do it: `rclone authorize` runs, its page's
/// address goes to the window ("rclone-sign-in", for it to open), and the sign-in it gives back is the
/// answer to the question. `rclone_authorize_cancel` stops it.
#[tauri::command]
pub async fn rclone_authorize(window: Window, help: String) -> Result<String, String> {
    let args = authorize_args(&help).ok_or("rclone didn't say how to sign in")?;
    blocking(move || {
        let mut command = crate::tools::command("rclone")?;
        command.arg("authorize").args(&args).arg("--auth-no-open-browser").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("rclone didn't start: {e}"))?;
        let job = proc::Job::ending()?;
        job.assign(&child)?;
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        if let Some(old) = lock(&AUTHORIZING).replace(child) {
            drop(old);
        }
        let (send, lines) = std::sync::mpsc::channel::<String>();
        let readers: Vec<_> = [stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)]
            .into_iter()
            .flatten()
            .map(|stream| {
                let send = send.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        let _ = send.send(line);
                    }
                })
            })
            .collect();
        drop(send);
        let mut output = String::new();
        let mut opened = false;
        for line in lines {
            if !opened {
                if let Some(at) = line.find("http://127.0.0.1:") {
                    let url: String = line[at..].chars().take_while(|c| !c.is_whitespace()).collect();
                    let _ = window.emit("rclone-sign-in", url);
                    opened = true;
                }
            }
            output.push_str(&line);
            output.push('\n');
        }
        for reader in readers {
            let _ = reader.join();
        }
        let ended = lock(&AUTHORIZING).take().map(|mut child| child.wait());
        drop(job);
        if let Some(token) = token_in(&output) {
            return Ok(token);
        }
        if ended.is_none() {
            return Err("cancelled".into());
        }
        let reason = output.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.split_once(" : ").map_or(l, |(_, said)| said).trim().to_string()).unwrap_or_default();
        Err(if reason.is_empty() { "The sign-in didn't finish".into() } else { format!("The sign-in didn't finish: {reason}") })
    })
    .await
}

/// Stops a browser sign-in that's waiting.
#[tauri::command]
pub fn rclone_authorize_cancel() {
    if let Some(mut child) = lock(&AUTHORIZING).take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A remote in rclone's configuration: its name, type ("drive") and, for S3, its service ("AWS").
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub kind: String,
    pub provider: String,
}

/// The remotes in rclone's configuration (only their types: their settings hold their secrets).
#[tauri::command]
pub async fn rclone_remotes() -> Result<Vec<Remote>, String> {
    let dump = rc("config/dump", json!({})).await?;
    let mut remotes: Vec<Remote> = dump
        .as_object()
        .map(|all| all.iter().map(|(name, settings)| Remote { name: name.clone(), kind: text_of(settings.get("type")), provider: text_of(settings.get("provider")) }).collect())
        .unwrap_or_default();
    remotes.sort_by_key(|r| r.name.to_lowercase());
    Ok(remotes)
}

/// Takes a remote out of rclone's configuration: its sign-in goes with it.
#[tauri::command]
pub async fn rclone_delete(name: String) -> Result<(), String> {
    rc("config/delete", json!({ "name": name })).await.map(|_| ())
}

/// How full a remote is, where its service says.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Space {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub free: Option<u64>,
}

#[tauri::command]
pub async fn rclone_about(remote: String, root: String) -> Result<Option<Space>, String> {
    match rc("operations/about", json!({ "fs": fs(&remote, &root) })).await {
        Ok(answer) => {
            let n = |key: &str| answer.get(key).and_then(Value::as_u64);
            Ok(Some(Space { total: n("total"), used: n("used"), free: n("free") }))
        }
        // Not every service tells.
        Err(e) if e.contains("doesn't support about") || e.contains("not supported") => Ok(None),
        Err(e) => Err(e),
    }
}

/// rclone on this PC: whether it's there, its version, where its configuration is, and whether that's
/// encrypted and still locked.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub installed: bool,
    pub version: Option<String>,
    pub config: Option<String>,
    pub locked: bool,
}

#[tauri::command]
pub async fn rclone_status() -> Result<Status, String> {
    if proc::locate("rclone").is_none() {
        return Ok(Status::default());
    }
    let version = rc("core/version", json!({})).await?.get("version").and_then(Value::as_str).map(|v| v.trim_start_matches('v').to_string());
    let config = rc("config/paths", json!({})).await.ok().and_then(|p| p.get("config").and_then(Value::as_str).map(str::to_string));
    let locked = matches!(rc("config/listremotes", json!({})).await, Err(e) if e == LOCKED);
    Ok(Status { installed: true, version, config, locked })
}

/// Unlocks an encrypted configuration with its password, for this run of Glint.
#[tauri::command]
pub async fn rclone_unlock(password: String) -> Result<(), String> {
    *lock(&CONFIG_PASS) = Some(password);
    blocking(|| {
        stop();
        Ok(())
    })
    .await?;
    match rc("config/listremotes", json!({})).await {
        Err(e) if e == LOCKED => {
            *lock(&CONFIG_PASS) = None;
            blocking(|| {
                stop();
                Ok(())
            })
            .await?;
            Err("That isn't its password".into())
        }
        other => other.map(|_| ()),
    }
}

/// A folder kept in sync with a place on a remote ("gdrive:Backups"), by a folder rule.
pub struct Sync<'a> {
    pub folder: &'a str,
    pub target: &'a str,
    /// "copy": what's new or changed here goes there, and nothing there is deleted; "mirror": there
    /// becomes what's here, what's gone here deleted there; "both": changes go both ways (bisync).
    pub mode: &'a str,
    /// rclone's filter flags for the files it takes (names, sizes, age, depth).
    pub filters: Vec<String>,
    /// Only says what it would do.
    pub dry: bool,
}

/// What a sync did (or, a dry run, would do).
#[derive(Default, Debug, PartialEq)]
pub struct Synced {
    pub up: usize,
    pub down: usize,
    pub deleted_there: usize,
    pub deleted_here: usize,
    /// Each file, with what happens to it ("Copy up", "Delete there"…), up to 500.
    pub items: Vec<(String, String)>,
    pub errors: Vec<String>,
}

impl Synced {
    /// In a few words: "3 up, 1 down, 2 deleted there"; "Nothing to do".
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [(self.up, "up"), (self.down, "down"), (self.deleted_there, "deleted there"), (self.deleted_here, "deleted here")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
        if parts.is_empty() {
            "Nothing to do".into()
        } else {
            parts.join(", ")
        }
    }
}

/// Which side what rclone does next is on, in a two-way sync's log.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    There,
    Here,
}

/// One line of rclone's JSON log, counted into `synced`.
fn read_line(line: &str, synced: &mut Synced, side: &mut Side) {
    let Ok(entry) = serde_json::from_str::<Value>(line) else { return };
    let message = entry.get("msg").and_then(Value::as_str).unwrap_or("").trim();
    let object = entry.get("object").and_then(Value::as_str).unwrap_or("").to_string();
    let skipped = entry.get("skipped").and_then(Value::as_str).unwrap_or("");
    // bisync says where it goes before it does it: "- Path2  Do queued copies to  - Path1".
    if let Some(at) = message.find("Do queued ") {
        *side = if message[at..].contains("Path1") { Side::Here } else { Side::There };
        return;
    }
    let copied = message.starts_with("Copied") || skipped == "copy";
    let deleted = message == "Deleted" || skipped == "delete";
    let mut note = |what: &str| {
        if synced.items.len() < 500 {
            synced.items.push((what.to_string(), object.clone()));
        }
    };
    if copied {
        if *side == Side::Here {
            synced.down += 1;
            note("Copy down");
        } else {
            synced.up += 1;
            note("Copy up");
        }
    } else if deleted {
        if *side == Side::Here {
            synced.deleted_here += 1;
            note("Delete here");
        } else {
            synced.deleted_there += 1;
            note("Delete there");
        }
    } else if matches!(entry.get("level").and_then(Value::as_str), Some("error" | "critical")) && synced.errors.len() < 5 {
        synced.errors.push(message.split('\n').next().unwrap_or(message).trim().to_string());
    }
}

fn sync_args(sync: &Sync, resync: bool) -> Vec<String> {
    let verb = match sync.mode {
        "mirror" => "sync",
        "both" => "bisync",
        _ => "copy",
    };
    let mut args: Vec<String> = [verb, sync.folder, sync.target, "--use-json-log", "-v", "--stats", "0", "--color", "NEVER", "--ask-password=false"].map(String::from).to_vec();
    if sync.mode == "both" {
        // The newer of a file changed on both sides wins, the other kept as "name.conflict1"; a run that
        // stopped half-way picks up where it was.
        args.extend(["--conflict-resolve", "newer", "--conflict-loser", "num", "--resilient", "--recover", "--max-lock", "2m"].map(String::from));
        if resync {
            args.extend(["--resync", "--resync-mode", "newer"].map(String::from));
        }
    }
    args.extend(sync.filters.iter().cloned());
    if sync.dry {
        args.push("--dry-run".into());
    }
    args
}

/// Runs a sync and waits for it. A two-way one with no record of a last run (its first) makes one: each
/// file goes where it's missing, the newer of two winning.
pub fn sync(sync: &Sync) -> Result<Synced, String> {
    let first = run_sync(sync, false)?;
    let needs_resync = sync.mode == "both" && first.errors.iter().any(|e| e.contains("cannot find prior") || e.contains("--resync"));
    if needs_resync {
        return run_sync(sync, true);
    }
    Ok(first)
}

fn run_sync(sync: &Sync, resync: bool) -> Result<Synced, String> {
    let mut command = crate::tools::command("rclone")?;
    command.args(sync_args(sync, resync)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    if let Some(pass) = lock(&CONFIG_PASS).clone() {
        command.env("RCLONE_CONFIG_PASS", pass);
    }
    let mut child = command.spawn().map_err(|e| format!("rclone didn't start: {e}"))?;
    let job = proc::Job::ending()?;
    job.assign(&child)?;
    let mut synced = Synced::default();
    let mut side = Side::There;
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            read_line(&line, &mut synced, &mut side);
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() && synced.errors.is_empty() {
        synced.errors.push(format!("rclone stopped with code {}", status.code().unwrap_or(-1)));
    }
    Ok(synced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_rclone_says() {
        assert_eq!(served_on("2026/10/30 11:19:46 NOTICE: Serving remote control on http://127.0.0.1:55917/"), Some(55917));
        assert_eq!(served_on("2026/10/30 11:19:46 INFO  : Using --user glint"), None);
        let help = "Execute the following on the machine with the web browser (same rclone\nversion recommended):\n\n\trclone authorize \"drive\" \"eyJzY29wZSI6ImRyaXZlIn0\"\n\nThen paste the result.\n";
        assert_eq!(authorize_args(help), Some(vec!["drive".to_string(), "eyJzY29wZSI6ImRyaXZlIn0".to_string()]));
        assert_eq!(authorize_args("\trclone authorize \"onedrive\"\n"), Some(vec!["onedrive".to_string()]));
        assert_eq!(authorize_args("nothing to run"), None);
        let output = "2026/10/30 NOTICE: Got code\nPaste the following into your remote machine --->\n{\"access_token\":\"x\"}\n<---End paste\n";
        assert_eq!(token_in(output).as_deref(), Some("{\"access_token\":\"x\"}"));
        assert_eq!(said("unable to decrypt configuration and not allowed to ask for password: set RCLONE_CONFIG_PASS"), LOCKED);
        assert_eq!(said("failed to configure: empty token"), "Failed to configure: empty token");
        assert_eq!(fs("gdrive", r"\Photos\2024\"), "gdrive:Photos/2024");
        assert_eq!(fs("gdrive", ""), "gdrive:");
        let item = item_of(&json!({ "Name": "a.txt", "Size": 30, "ModTime": "2026-10-30T11:21:35.2305299+03:00", "IsDir": false })).unwrap();
        assert_eq!(item, ("a.txt".to_string(), false, 30, 1_793_348_495_230));
        assert_eq!(item_of(&json!({ "Name": "Doc.docx", "Size": -1, "IsDir": false })).unwrap().2, 0);
        assert_eq!(item_of(&json!({ "Name": "v1.0", "Size": -1, "ModTime": "2000-01-01T00:00:00Z", "IsDir": true })).unwrap().3, 0);
    }

    #[test]
    fn counts_what_a_sync_did() {
        let mut synced = Synced::default();
        let mut side = Side::There;
        for line in [
            r#"{"level":"info","msg":"Copied (new)","size":3,"object":"a.txt"}"#,
            r#"{"level":"info","msg":"Deleted","object":"gone.txt"}"#,
            r#"{"level":"notice","msg":"- Path2    Do queued copies to                - Path1"}"#,
            r#"{"level":"notice","msg":"Skipped copy as --dry-run is set (size 9)","skipped":"copy","size":9,"object":"there.txt"}"#,
            r#"{"level":"notice","msg":"- Path1    Do queued copies to                - Path2"}"#,
            r#"{"level":"info","msg":"Copied (replaced existing)","object":"b.txt"}"#,
            r#"{"level":"error","msg":"Bisync critical error: cannot find prior Path1 or Path2 listings\nTip: …"}"#,
            "not json",
        ] {
            read_line(line, &mut synced, &mut side);
        }
        assert_eq!((synced.up, synced.down, synced.deleted_there, synced.deleted_here), (2, 1, 1, 0));
        assert_eq!(synced.summary(), "2 up, 1 down, 1 deleted there");
        assert_eq!(synced.items[2], ("Copy down".to_string(), "there.txt".to_string()));
        assert_eq!(synced.errors, vec!["Bisync critical error: cannot find prior Path1 or Path2 listings".to_string()]);
        assert_eq!(Synced::default().summary(), "Nothing to do");
        let two_way = Sync { folder: r"C:\Docs", target: "gd:Docs", mode: "both", filters: vec!["--max-depth".into(), "1".into()], dry: true };
        let args = sync_args(&two_way, true);
        assert_eq!(&args[..3], ["bisync", r"C:\Docs", "gd:Docs"]);
        assert!(args.contains(&"--resync".to_string()) && args.ends_with(&["--max-depth".to_string(), "1".to_string(), "--dry-run".to_string()]));
        assert_eq!(sync_args(&Sync { mode: "mirror", dry: false, ..two_way }, false)[0], "sync");
    }

    #[test]
    fn asks_rclones_questions_as_it_gives_them() {
        let q = question(&json!({
            "Name": "config_is_local", "Help": "Use web browser?", "Type": "bool", "Default": true, "DefaultStr": "true",
            "Examples": [{ "Value": "true", "Help": "Yes" }, { "Value": "false", "Help": "No" }], "Exclusive": true, "Required": false, "IsPassword": false
        }));
        assert_eq!((q.name.as_str(), q.kind.as_str(), q.default.as_str(), q.examples.len(), q.exclusive), ("config_is_local", "bool", "true", 2, true));
        let done = step(json!({ "State": "", "Option": null, "Error": "", "Result": "" }));
        assert!(done.state.is_empty() && done.question.is_none());
    }
}
