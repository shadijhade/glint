//! SFTP, through Windows' own OpenSSH client (sftp.exe, part of Windows since 2018): it signs in as ssh
//! does, with the person's keys, their agent and their ~/.ssh/config, or with a password (or a key's
//! passphrase) that Glint answers from Credential Manager as ssh's SSH_ASKPASS, so nothing secret goes
//! on a command line or to disk. Host keys are checked against ~/.ssh/known_hosts and Glint's own list,
//! which takes a server's keys only once the person trusts their fingerprint; ssh is told to write to
//! neither. Each thing asked is one sftp run (Windows' OpenSSH can't share a connection between runs),
//! a batch of downloads one run for all of them.

use super::s3::NOT_FOUND;
use super::Item;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a failed sign-in says when the server's host key isn't trusted yet (the connection's dialog,
/// which offers to trust it, knows it by its start).
pub const UNKNOWN_HOST: &str = "Its host key isn't known yet: Edit the connection (its right-click menu) and Test it to see its fingerprint and trust it";

/// How long a listing may take.
const LIST_TIME: Duration = Duration::from_secs(120);

pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The folder it starts at: below the login folder, or from / ("" for the login folder).
    pub root: String,
    /// A private key file to sign in with ("" for ssh's own choice).
    pub key: String,
    /// The credential holding its password (or key passphrase), when one is kept.
    pub askpass: Option<String>,
}

fn openssh(exe: &str) -> Result<PathBuf, String> {
    let windows = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let path = windows.join(r"System32\OpenSSH").join(exe);
    if path.is_file() {
        Ok(path)
    } else {
        Err("Windows' OpenSSH client isn't installed: add OpenSSH Client in Settings → System → Optional features".into())
    }
}

/// Glint's own list of trusted host keys, beside the person's ~/.ssh/known_hosts.
fn known_hosts() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("known_hosts"))
}

/// A remote path for sftp's command line, in double quotes: what sftp would otherwise read (\ and ",
/// and the * ? [ ] it would match names with) escaped.
fn remote_arg(path: &str) -> String {
    let mut out = String::from("\"");
    for c in path.chars() {
        if matches!(c, '\\' | '"' | '*' | '?' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A path on this PC for sftp's command line (Glint's own temp folders: no quotes in them).
fn local_arg(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('\\', "/"))
}

impl Server {
    /// The server's path for `path` (below the folder it starts at).
    fn full(&self, path: &str) -> String {
        let path = path.trim_matches('/');
        let root = if self.root == "/" { "/" } else { self.root.trim_end_matches('/') };
        match (root, path) {
            ("", "") => ".".into(),
            ("", path) => path.into(),
            ("/", path) => format!("/{path}"),
            (root, "") => root.into(),
            (root, path) => format!("{root}/{path}"),
        }
    }

    fn destination(&self) -> String {
        if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    fn command(&self) -> Result<Command, String> {
        let mut command = Command::new(openssh("sftp.exe")?);
        crate::proc::no_window(&mut command);
        let mut hosts = String::from("\"~/.ssh/known_hosts\"");
        if let Some(own) = known_hosts() {
            hosts.push_str(&format!(" \"{}\"", own.to_string_lossy().replace('\\', "/")));
        }
        command.args(["-o", "LogLevel=ERROR", "-o", "StrictHostKeyChecking=yes", "-o", "UpdateHostKeys=no", "-o", "ConnectTimeout=15", "-o", "ServerAliveInterval=15", "-o", "NumberOfPasswordPrompts=1"]);
        command.arg("-o").arg(format!("UserKnownHostsFile={hosts}"));
        if self.port != 0 && self.port != 22 {
            command.arg("-P").arg(self.port.to_string());
        }
        if !self.key.trim().is_empty() {
            command.arg("-i").arg(self.key.trim());
        }
        match (&self.askpass, std::env::current_exe()) {
            // ssh asks Glint itself for the password, which reads it from Credential Manager.
            (Some(target), Ok(glint)) => {
                command.args(["-o", "BatchMode=no"]).env("SSH_ASKPASS", glint).env("SSH_ASKPASS_REQUIRE", "force").env("DISPLAY", "glint").env(ASKPASS, target);
            }
            _ => {
                command.args(["-o", "BatchMode=yes"]);
            }
        }
        command.args(["-b", "-"]).arg(self.destination());
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(command)
    }

    /// Runs `script` (sftp's commands, a line each) in one sftp run, ended after `limit` (or when
    /// `stop` says so): its output's lines (the commands it echoes left out), and its error lines.
    fn run(&self, script: &str, limit: Duration, stop: &dyn Fn() -> bool) -> Result<(i32, Vec<String>, Vec<String>), String> {
        let job = crate::proc::Job::ending()?;
        let mut child = self.command()?.spawn().map_err(|e| format!("Couldn't start sftp: {e}"))?;
        let _ = job.assign(&child);
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(script.as_bytes());
        }
        let lines = |stream: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || stream.map_or_else(Vec::new, |s| BufReader::new(s).lines().map_while(Result::ok).collect::<Vec<String>>()))
        };
        let out = lines(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let err = lines(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let started = Instant::now();
        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code().unwrap_or(-1),
                Ok(None) if stop() => {
                    let _ = child.kill();
                    return Err("cancelled".into());
                }
                Ok(None) if started.elapsed() > limit => {
                    let _ = child.kill();
                    return Err("It didn't answer in time".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(30)),
                Err(e) => return Err(e.to_string()),
            }
        };
        let out: Vec<String> = out.join().unwrap_or_default().into_iter().filter(|l| !l.starts_with("sftp> ")).collect();
        let err = err.join().unwrap_or_default().into_iter().filter(|l| !l.trim().is_empty()).collect();
        Ok((code, out, err))
    }

    /// One sftp run of `script` that must succeed: its output, or why it didn't.
    async fn ask(&self, script: String, limit: Duration) -> Result<Vec<String>, String> {
        let me = self.settings();
        crate::fsx::blocking(move || {
            let (code, out, err) = me.run(&script, limit, &|| false)?;
            if code == 0 {
                Ok(out)
            } else {
                Err(failure(&err, &me.host))
            }
        })
        .await
    }

    /// A copy, to take to a thread of its own.
    pub fn settings(&self) -> Server {
        Server { host: self.host.clone(), port: self.port, user: self.user.clone(), root: self.root.clone(), key: self.key.clone(), askpass: self.askpass.clone() }
    }

    /// What's in the folder `path` ("" for the folder it starts at).
    pub async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        let full = self.full(path);
        let out = self.ask(format!("ls -lan {}\n", remote_arg(&full)), LIST_TIME).await?;
        Ok(entries(&out, &full).into_iter().filter(|i| i.name != "." && i.name != "..").collect())
    }

    /// The file or folder at `path`; None when there's nothing there.
    pub async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        let full = self.full(path);
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        if path.trim_matches('/').is_empty() {
            return Ok(Some(Item { name, dir: true, size: 0, modified: 0 }));
        }
        let out = match self.ask(format!("ls -lan {}\n", remote_arg(&full)), LIST_TIME).await {
            Err(e) if e == NOT_FOUND => return Ok(None),
            other => other?,
        };
        let found = entries(&out, &full);
        // A folder lists its own "." (ls -a); a file lists itself, by its own path.
        if found.iter().any(|i| i.name == ".") {
            return Ok(Some(Item { name, dir: true, size: 0, modified: 0 }));
        }
        Ok(found.into_iter().find(|i| i.name.is_empty()).map(|i| Item { name, ..i }))
    }

    /// Downloads each (server path, file on this PC, its size) in one sftp run, telling `on_progress`
    /// how many bytes have come in all (now and then; it stops them by returning false, "cancelled"):
    /// each one's outcome.
    pub fn download(&self, files: &[(String, PathBuf, u64)], on_progress: &dyn Fn(u64) -> bool) -> Result<Vec<Result<u64, String>>, String> {
        for (_, dest, _) in files {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let script: String = files.iter().map(|(from, to, _)| format!("-get {} {}\n", remote_arg(&self.full(from)), local_arg(to))).collect();
        let sizes = || files.iter().map(|(_, to, _)| std::fs::metadata(to).map_or(0, |m| m.len())).sum::<u64>();
        let last = std::cell::Cell::new(Instant::now());
        let going = std::cell::Cell::new(true);
        let stop = || {
            if last.get().elapsed() >= Duration::from_millis(250) {
                last.set(Instant::now());
                going.set(on_progress(sizes()));
            }
            !going.get()
        };
        let (code, _, err) = self.run(&script, Duration::from_secs(24 * 3600), &stop)?;
        on_progress(sizes());
        if code == 255 {
            return Err(failure(&err, &self.host));
        }
        Ok(files
            .iter()
            .map(|(from, to, size)| match std::fs::metadata(to) {
                Ok(m) if m.len() == *size => Ok(m.len()),
                Ok(m) => {
                    let _ = std::fs::remove_file(to);
                    Err(format!("It ended after {} of its {size} bytes", m.len()))
                }
                Err(_) => Err(err.iter().find(|l| l.contains(from.rsplit('/').next().unwrap_or(from))).map_or_else(|| "It didn't come".to_string(), |l| failure(std::slice::from_ref(l), &self.host))),
            })
            .collect())
    }
}

/// Why sftp failed, in words, from what it said.
fn failure(err: &[String], host: &str) -> String {
    let said = err.join("\n");
    if said.contains("IDENTIFICATION HAS CHANGED") {
        "Its host key has changed since it was trusted, which can mean someone is in the way: Glint won't connect. Check with whoever runs the server".into()
    } else if said.contains("host key is known") || said.contains("Host key verification failed") {
        UNKNOWN_HOST.into()
    } else if said.contains("Permission denied (") || said.contains("Too many authentication failures") {
        "The server didn't take the key or the password".into()
    } else if said.contains("Could not resolve hostname") {
        format!("There's no server called “{host}”")
    } else if said.contains("Connection refused") {
        "The server refused the connection: is SFTP on that port?".into()
    } else if said.contains("timed out") {
        "It didn't answer in time".into()
    } else if said.contains("not found") || said.contains("No such file") {
        NOT_FOUND.into()
    } else if said.contains("subsystem request failed") {
        "That server doesn't offer SFTP".into()
    } else {
        err.iter().rev().find(|l| !l.contains("Connection closed")).cloned().unwrap_or_else(|| "sftp ended without saying why".into())
    }
}

/// Lines of `ls -lan <folder>` (OpenSSH's own layout: permissions, links, owner, group, size, date,
/// path) as items, named below `folder` ("." for the folder itself; "" for `folder` when it's a file).
fn entries(lines: &[String], folder: &str) -> Vec<Item> {
    let base = format!("{}/", folder.trim_end_matches('/'));
    lines
        .iter()
        .filter_map(|line| {
            let mut rest = line.as_str();
            let mut fields = Vec::new();
            for _ in 0..8 {
                rest = rest.trim_start();
                let end = rest.find(' ')?;
                fields.push(&rest[..end]);
                rest = &rest[end..];
            }
            let path = rest.strip_prefix(' ')?;
            let name = if path == folder { "" } else { path.strip_prefix(&base).unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path)) };
            let dir = fields[0].starts_with('d');
            Some(Item { name: name.to_string(), dir, size: if dir { 0 } else { fields[4].parse().unwrap_or(0) }, modified: date(fields[5], fields[6], fields[7]).unwrap_or(0) })
        })
        .collect()
}

/// ls's date ("Oct 24 04:23" this year, "Oct 24  2023" further back), in this PC's time, as ms since 1970.
fn date(month: &str, day: &str, time_or_year: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(month))? as u16 + 1;
    let day: u16 = day.parse().ok()?;
    let now = local_now();
    let (year, hour, minute) = match time_or_year.split_once(':') {
        Some((h, m)) => (now.0, h.parse().ok()?, m.parse().ok()?),
        None => (time_or_year.parse().ok()?, 0, 0),
    };
    let ms = local_to_utc(year, month, day, hour, minute)?;
    // A time without a year is within the last half year: one that would be ahead is last year's.
    if !time_or_year.contains(':') || ms <= utc_now() + 86_400_000 {
        Some(ms)
    } else {
        local_to_utc(year - 1, month, day, hour, minute)
    }
}

fn utc_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// This PC's local year.
fn local_now() -> (u16,) {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    (unsafe { GetLocalTime() }.wYear,)
}

/// A date in this PC's time zone, as UTC ms since 1970.
fn local_to_utc(year: u16, month: u16, day: u16, hour: u16, minute: u16) -> Option<i64> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
    let local = SYSTEMTIME { wYear: year, wMonth: month, wDay: day, wHour: hour, wMinute: minute, ..Default::default() };
    let mut utc = SYSTEMTIME::default();
    let mut file = FILETIME::default();
    unsafe {
        TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).ok()?;
        SystemTimeToFileTime(&utc, &mut file).ok()?;
    }
    let ticks = (u64::from(file.dwHighDateTime) << 32) | u64::from(file.dwLowDateTime);
    Some((ticks / 10_000) as i64 - 11_644_473_600_000)
}

/// The environment variable naming the credential Glint answers ssh's password prompt from.
pub const ASKPASS: &str = "GLINT_ASKPASS";

/// Glint run by ssh as its SSH_ASKPASS: a password (or a key's passphrase) prompt gets the secret kept
/// for the connection, and nothing else does. None when Glint wasn't run for that.
pub fn askpass() -> Option<i32> {
    let target = std::env::var(ASKPASS).ok()?;
    let prompt = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    if !target.contains("/remote/") || !(prompt.contains("password") || prompt.contains("passphrase")) {
        return Some(1);
    }
    match super::secrets::read_target(&target) {
        Some(secret) => {
            println!("{secret}");
            Some(0)
        }
        None => Some(1),
    }
}

/// A server's host keys, as ssh-keyscan reads them, and their fingerprints, for the person to trust.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HostKeys {
    pub fingerprints: Vec<String>,
    pub keys: String,
}

pub async fn host_keys(host: String, port: u16) -> Result<HostKeys, String> {
    crate::fsx::blocking(move || {
        let mut scan = Command::new(openssh("ssh-keyscan.exe")?);
        crate::proc::no_window(&mut scan);
        scan.args(["-T", "10", "-t", "ed25519,ecdsa,rsa", "-p", &(if port == 0 { 22 } else { port }).to_string(), "--", &host]);
        let out = scan.stderr(Stdio::null()).output().map_err(|e| e.to_string())?;
        let keys: String = String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).map(|l| format!("{l}\n")).collect();
        if keys.is_empty() {
            return Err(format!("Couldn't read {host}'s host keys: is it an SSH server, and on that port?"));
        }
        let mut keygen = Command::new(openssh("ssh-keygen.exe")?);
        crate::proc::no_window(&mut keygen);
        let mut child = keygen.args(["-lf", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
        let _ = child.stdin.take().map(|mut s| s.write_all(keys.as_bytes()));
        let listed = child.wait_with_output().map_err(|e| e.to_string())?;
        // "256 SHA256:LFy7… [127.0.0.1]:52222 (ED25519)" → "SHA256:LFy7… (ED25519)".
        let fingerprints = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter_map(|l| {
                let mut words = l.split_whitespace();
                let fingerprint = words.nth(1)?;
                Some(format!("{fingerprint} {}", words.last().unwrap_or("")))
            })
            .collect();
        Ok(HostKeys { fingerprints, keys })
    })
    .await
}

/// Adds keys `host_keys` read to Glint's own list: only known_hosts lines of plain keys.
pub fn trust(keys: &str) -> Result<(), String> {
    let lines: Vec<&str> = keys.lines().filter(|l| !l.trim().is_empty()).collect();
    let plain = |l: &&str| {
        let fields: Vec<&str> = l.split_whitespace().collect();
        fields.len() == 3 && !l.starts_with('@') && (fields[1].starts_with("ssh-") || fields[1].starts_with("ecdsa-")) && fields[2].bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    };
    if lines.is_empty() || !lines.iter().all(plain) {
        return Err("Those aren't host keys".into());
    }
    let path = known_hosts().ok_or("Glint's data folder isn't available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
    for line in lines {
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ls() {
        let out: Vec<String> = [
            "drwx******    ? 0        0               0 Oct 24 04:23 /srv/files/.",
            "drwx******    ? 0        0               0 Oct 24 04:23 /srv/files/..",
            "drwxr-xr-x    2 1000     1000         4096 Oct 24  2023 /srv/files/empty dir",
            "-rw-r--r--    1 1000     1000        20480 Jan  5 09:07 /srv/files/with spaces & 'quotes'.txt",
            "lrwxrwxrwx    1 1000     1000            7 Jan  5 09:07 /srv/files/link",
        ]
        .map(String::from)
        .to_vec();
        let found = entries(&out, "/srv/files");
        let names: Vec<(&str, bool, u64)> = found.iter().map(|i| (i.name.as_str(), i.dir, i.size)).collect();
        assert_eq!(names, [(".", true, 0), ("..", true, 0), ("empty dir", true, 0), ("with spaces & 'quotes'.txt", false, 20480), ("link", false, 7)]);
        assert!(found[2].modified > 0 && found[3].modified > 0);
        // A file listed by its own path.
        assert_eq!(entries(&["-rw-r--r--    1 0 0 15 Oct 24 04:23 notes.txt".to_string()], "notes.txt")[0].name, "");
        assert_eq!(remote_arg(r#"a "b" [1]*?\c"#), r#""a \"b\" \[1\]\*\?\\c""#);
        assert_eq!(local_arg(Path::new(r"C:\Temp\a b.txt")), "\"C:/Temp/a b.txt\"");
        let server = Server { host: "h".into(), port: 22, user: String::new(), root: String::new(), key: String::new(), askpass: None };
        assert_eq!((server.full(""), server.full("a/b")), (".".to_string(), "a/b".to_string()));
        let server = Server { root: "/srv/files".into(), ..server };
        assert_eq!((server.full(""), server.full("/a/")), ("/srv/files".to_string(), "/srv/files/a".to_string()));
    }

    #[test]
    fn says_why_it_failed() {
        let say = |l: &str| failure(&[l.to_string(), "Connection closed".to_string()], "nas");
        assert_eq!(say("No ED25519 host key is known for [127.0.0.1]:52222 and you have requested strict checking."), UNKNOWN_HOST);
        assert_eq!(say("glint@127.0.0.1: Permission denied (publickey,password)."), "The server didn't take the key or the password");
        assert_eq!(say("ssh: Could not resolve hostname nas: No such host is known."), "There's no server called “nas”");
        assert_eq!(say("Can't ls: \"/nope\" not found"), NOT_FOUND);
        assert!(trust("[127.0.0.1]:52222 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFuN\n").is_ok() || known_hosts().is_none());
        assert!(trust("@cert-authority * ssh-ed25519 AAAA").is_err());
        assert!(trust("not a key").is_err());
    }
}
