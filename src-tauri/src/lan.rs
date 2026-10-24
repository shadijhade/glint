//! Sharing a folder (or a file) with a phone or another computer on this network, with dufs: a link,
//! and its QR code, that opens it in a browser there. It's served at one of this PC's addresses on the
//! network (never every address), at a port nothing's listening on, picked at random, and under a random
//! path that's the key to it (without the link, nothing's there). It's read-only unless uploads are
//! allowed, and then what's there can't be replaced or deleted. It ends when its time is up, when it's
//! stopped, or with Glint (share.rs holds it as it holds a send). The first time, Windows asks whether
//! dufs may accept connections from the network.

use crate::fsx::blocking;
use crate::share::{listen, running, start, why, Running, NEXT};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::net::Ipv4Addr;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;

/// Kept out of its listings: dot files, and what Windows keeps in folders for itself.
const HIDDEN: &str = ".*,desktop.ini,Thumbs.db,$RECYCLE.BIN,System Volume Information";

/// One of this PC's addresses on a network, and that network adapter's name ("Wi-Fi").
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub address: String,
    pub name: String,
}

/// This PC's IPv4 addresses on its networks, the one its traffic goes out by first: those other devices
/// on the same network reach it at. Networks without a way out (a virtual machine's) come last.
#[tauri::command]
pub fn lan_addresses() -> Vec<Address> {
    imp::addresses()
}

/// `n` random bytes from Windows' own generator, as base32 letters and digits.
fn random_name(n: usize) -> Result<String, String> {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let bytes = imp::random(n)?;
    let mut name = String::new();
    let (mut bits, mut held) = (0u32, 0u32);
    for b in bytes {
        bits = (bits << 8) | b as u32;
        held += 8;
        while held >= 5 {
            held -= 5;
            name.push(ALPHABET[((bits >> held) & 31) as usize] as char);
        }
    }
    Ok(name)
}

/// A port nothing on this PC listens on, at random, away from the ones Windows hands out for
/// connections going out.
fn free_port() -> Result<u16, String> {
    let taken = imp::listening();
    for _ in 0..64 {
        let [a, b] = imp::random(2)?[..] else { unreachable!() };
        let port = 20_000 + u16::from_be_bytes([a, b]) % 25_000;
        if !taken.contains(&port) {
            return Ok(port);
        }
    }
    Err("Couldn't find a free port".into())
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LanOptions {
    /// What's sent there can be saved into it (nothing there can be replaced or deleted).
    pub uploads: bool,
    /// When it ends by itself, in minutes from its start: 0, when it's stopped.
    pub minutes: u32,
    /// Which of this PC's addresses (`lan_addresses`'s); the first otherwise.
    pub address: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LanEvent {
    /// It's being served: its link, that as a QR code (an SVG), and when it ends by itself (ms since 1970).
    Ready { url: String, qr: String, ends: Option<u64> },
    /// Something was asked for: from which address, what (dufs's request line), and its answer's status.
    Visit { from: String, request: String, status: u16 },
    /// It ended by itself: its time was up, or why dufs ended.
    Ended { message: String },
}

/// dufs's arguments for serving `path` at `address`:`port` under `prefix`.
fn arguments(path: &str, address: Ipv4Addr, port: u16, prefix: &str, uploads: bool) -> Vec<String> {
    let mut args: Vec<String> = ["--bind", &address.to_string(), "--port", &port.to_string(), "--path-prefix", prefix, "--allow-archive", "--allow-search", "--hidden", HIDDEN]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if uploads {
        args.push("--allow-upload".into());
    }
    args.extend(["--".into(), path.into()]);
    args
}

/// A request in dufs's log: `2026-10-24T03:10:35+03:00 INFO - 192.168.1.40 "GET /k3j9/a.jpg" 200`.
fn visit(line: &str) -> Option<LanEvent> {
    let (_, rest) = line.split_once(" - ")?;
    let (from, rest) = rest.split_once(' ')?;
    let request = rest.strip_prefix('"')?;
    let (request, status) = request.rsplit_once("\" ")?;
    Some(LanEvent::Visit { from: from.into(), request: request.into(), status: status.trim().parse().ok()? })
}

/// Serves `path` on this network as `options` say, until it's stopped (share_stop), its time is up or
/// Glint ends: its link comes to `on_event` once dufs is listening. Resolves to its id.
#[tauri::command]
pub async fn lan_share_start(path: String, options: LanOptions, on_event: Channel<LanEvent>) -> Result<u64, String> {
    blocking(move || {
        if !Path::new(&path).exists() {
            return Err(format!("“{path}” isn't there any more"));
        }
        let addresses = imp::addresses();
        let address = match &options.address {
            Some(chosen) => addresses.iter().find(|a| &a.address == chosen).ok_or("That address isn't one of this PC's on a network any more")?,
            None => addresses.first().ok_or("This PC isn't on a network")?,
        };
        let address: Ipv4Addr = address.address.parse().map_err(|_| "That isn't an address")?;
        let prefix = random_name(10)?;
        let port = free_port()?;
        let mut command = crate::tools::command("dufs")?;
        command.args(arguments(&path, address, port, &prefix, options.uploads)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let (job, mut child) = start(&mut command, "dufs")?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let stopped = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        running().push(Running { id, _job: job, stopped: stopped.clone(), work: None, _lock: None });
        let ends = (options.minutes > 0).then(|| Duration::from_secs(options.minutes as u64 * 60));
        if let Some(after) = ends {
            let (stopped, timed_out) = (stopped.clone(), timed_out.clone());
            std::thread::spawn(move || {
                let started = Instant::now();
                while started.elapsed() < after {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                // Ended as Stop would, but said as its time being up.
                timed_out.store(true, Ordering::Relaxed);
                running().retain(|r| r.id != id);
            });
        }
        let ends_at = ends.map(|d| (SystemTime::now() + d).duration_since(UNIX_EPOCH).map_or(0, |t| t.as_millis() as u64));
        let said = listen(child.stderr.take(), |_| {});
        let stdout = child.stdout.take();
        std::thread::spawn(move || {
            for line in stdout.into_iter().flat_map(|s| BufReader::new(s).lines().map_while(Result::ok)) {
                let event = match line.strip_prefix("Listening on ") {
                    Some(url) => Some(LanEvent::Ready { url: url.trim().to_string(), qr: crate::share::qr_svg(url.trim()), ends: ends_at }),
                    None => visit(&line),
                };
                if let Some(event) = event {
                    let _ = on_event.send(event);
                }
            }
            let code = child.wait().ok().and_then(|s| s.code());
            let last = said.join().unwrap_or_default();
            running().retain(|r| r.id != id);
            if timed_out.load(Ordering::Relaxed) {
                let _ = on_event.send(LanEvent::Ended { message: "Its time was up".into() });
            } else if !stopped.load(Ordering::Relaxed) {
                let _ = on_event.send(LanEvent::Ended { message: why(&last, code, "dufs") });
            }
        });
        Ok(id)
    })
    .await
}

#[cfg(windows)]
mod imp {
    use super::Address;
    use std::net::Ipv4Addr;

    pub fn random(n: usize) -> Result<Vec<u8>, String> {
        use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        let mut bytes = vec![0u8; n];
        unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(|e| e.message())?;
        Ok(bytes)
    }

    /// The ports TCP listeners hold on this PC (IPv4's).
    pub fn listening() -> Vec<u16> {
        use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
        use windows::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER};
        use windows::Win32::Networking::WinSock::AF_INET;
        // Asked again with the size it says it needs, while it grows.
        let mut size = 0u32;
        for _ in 0..4 {
            let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
            let got = unsafe { GetExtendedTcpTable(Some(buf.as_mut_ptr().cast()), &mut size, false, AF_INET.0 as u32, TCP_TABLE_OWNER_PID_LISTENER, 0) };
            if got == 0 {
                let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
                let rows = unsafe { std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize) };
                return rows.iter().map(|r| u16::from_be((r.dwLocalPort & 0xFFFF) as u16)).collect();
            }
            if got != ERROR_INSUFFICIENT_BUFFER.0 {
                break;
            }
        }
        Vec::new()
    }

    pub fn addresses() -> Vec<Address> {
        use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
        use windows::Win32::NetworkManagement::IpHelper::{
            GetAdaptersAddresses, GetBestInterface, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK,
            IP_ADAPTER_ADDRESSES_LH,
        };
        use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
        use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
        // Asked again with the size it says it needs (u64s, for the records' alignment).
        let mut size = 16 * 1024u32;
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut asked = 0;
        let buf = loop {
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            match unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(buf.as_mut_ptr().cast()), &mut size) } {
                0 => break buf,
                got if got == ERROR_BUFFER_OVERFLOW.0 && asked < 4 => asked += 1,
                _ => return Vec::new(),
            }
        };
        // The adapter traffic for the internet goes out by.
        let mut best = 0u32;
        let out = unsafe { GetBestInterface(u32::from_ne_bytes([8, 8, 8, 8]), &mut best) } == 0;
        // (address, name, out by it, has a gateway, metric)
        let mut found: Vec<(Ipv4Addr, String, bool, bool, u32)> = Vec::new();
        let mut at = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(adapter) = unsafe { at.as_ref() } {
            if adapter.OperStatus == IfOperStatusUp && adapter.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let name = unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default();
                let index = unsafe { adapter.Anonymous1.Anonymous.IfIndex };
                let gateway = !adapter.FirstGatewayAddress.is_null();
                let mut unicast = adapter.FirstUnicastAddress;
                while let Some(one) = unsafe { unicast.as_ref() } {
                    let socket = one.Address.lpSockaddr;
                    if !socket.is_null() && unsafe { (*socket).sa_family } == AF_INET {
                        let ip = Ipv4Addr::from(u32::from_be(unsafe { (*(socket as *const SOCKADDR_IN)).sin_addr.S_un.S_addr }));
                        if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
                            found.push((ip, name.clone(), out && index == best, gateway, adapter.Ipv4Metric));
                        }
                    }
                    unicast = one.Next;
                }
            }
            at = adapter.Next;
        }
        found.sort_by_key(|(ip, _, out, gateway, metric)| (!out, !gateway, !ip.is_private(), *metric));
        found.into_iter().map(|(ip, name, ..)| Address { address: ip.to_string(), name }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_where_and_as_asked() {
        let args = arguments(r"C:\Photos", Ipv4Addr::new(192, 168, 1, 23), 31337, "abcd", false);
        assert_eq!(&args[..6], ["--bind", "192.168.1.23", "--port", "31337", "--path-prefix", "abcd"]);
        assert!(!args.iter().any(|a| a == "--allow-upload" || a == "--allow-delete" || a == "--allow-all" || a == "--allow-symlink"));
        assert_eq!(&args[args.len() - 2..], ["--", r"C:\Photos"]);
        assert!(arguments(r"C:\Photos", Ipv4Addr::new(10, 0, 0, 2), 20000, "abcd", true).iter().any(|a| a == "--allow-upload"));
    }

    #[test]
    fn reads_its_log() {
        assert_eq!(
            visit(r#"2026-10-24T03:10:35+03:00 INFO - 192.168.1.40 "GET /k3j9xq/a b.jpg" 200"#),
            Some(LanEvent::Visit { from: "192.168.1.40".into(), request: "GET /k3j9xq/a b.jpg".into(), status: 200 })
        );
        assert_eq!(visit("Listening on http://192.168.1.23:31337/k3j9xq/"), None);
    }

    #[test]
    fn picks_keys_and_ports_at_random() {
        let (a, b) = (random_name(10).unwrap(), random_name(10).unwrap());
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c)));
        let port = free_port().unwrap();
        assert!((20_000..45_000).contains(&port) && !imp::listening().contains(&port));
        // This PC's own: none of them loopback.
        assert!(lan_addresses().iter().all(|a| !a.address.starts_with("127.")));
    }
}
