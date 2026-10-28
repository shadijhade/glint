//! Sharing a folder (or a file) with a phone or another computer on this network, with dufs: a link,
//! and its QR code, that opens it in a browser there. It's served at one of this PC's addresses on the
//! network (never every address), at a port nothing's listening on, picked at random, and under a random
//! path that's the key to it (without the link, nothing's there). It's read-only unless uploads are
//! allowed, and then what's there can't be replaced or deleted. It ends when its time is up, when it's
//! stopped, or with Glint (share.rs holds it as it holds a send). The first time, Windows asks whether
//! dufs may accept connections from the network. Whether other devices can reach it is Windows' to say
//! (the kind of network it has the address on, and its firewall's rules for dufs there): Glint reads
//! that, to say what keeps them out and where it's changed, and never changes it.

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
pub async fn lan_addresses() -> Result<Vec<Address>, String> {
    crate::fsx::blocking(|| Ok(imp::addresses())).await
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

/// What may keep other devices from reaching a share at one of this PC's addresses: the kind of network
/// Windows has it on, and Windows Firewall's say on dufs there.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    /// "public", "private" or "domain", as Windows has the network; None when it can't say.
    pub network: Option<&'static str>,
    pub firewall: Firewall,
}

/// What Windows Firewall does with connections coming in to dufs on a network.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Firewall {
    /// It's off there (another firewall may stand in for it).
    Off,
    /// A rule lets them in.
    Allowed,
    /// A rule keeps them out (dismissing Windows' prompt makes one), or it keeps everything out there.
    Blocked,
    /// No rule says, so they're kept out: Windows asks the first time dufs listens, unless it's set not to.
    Unset,
    /// It couldn't be told.
    Unknown,
}

/// What of one of Windows Firewall's rules bears on connections coming in to a program.
#[derive(Clone, Debug, Default)]
struct Rule {
    /// The program it's for, %variables% and all ("" for any).
    program: String,
    inbound: bool,
    enabled: bool,
    allow: bool,
    /// The kinds of network it's for: NET_FW_PROFILE2_* bits (1 domain, 2 private, 4 public).
    profiles: i32,
    /// 6 is TCP, 17 UDP, 256 any.
    protocol: i32,
}

const TCP: i32 = 6;
const ANY_PROTOCOL: i32 = 256;

/// The firewall's profile bit for a kind of network.
fn profile_of(network: &str) -> i32 {
    match network {
        "domain" => 1,
        "private" => 2,
        _ => 4,
    }
}

/// What Windows Firewall does with TCP connections coming in to `program` on a network of `profile`:
/// nothing when it's off there, and everything kept out when it's set to; otherwise the program's own
/// rules say, one that blocks winning over one that allows, as in Windows, and with neither they're
/// kept out. (Rules for any program, by port, aren't looked at: dufs's port is new each time.)
fn verdict(on: bool, block_all: bool, profile: i32, rules: &[Rule], program: &str) -> Firewall {
    if !on {
        return Firewall::Off;
    }
    if block_all {
        return Firewall::Blocked;
    }
    let program = program.to_lowercase();
    let mut allowed = false;
    for rule in rules {
        let bears = rule.enabled && rule.inbound && rule.profiles & profile != 0 && (rule.protocol == TCP || rule.protocol == ANY_PROTOCOL);
        if !bears || rule.program.is_empty() || crate::fsx::expand_vars(&rule.program).to_lowercase() != program {
            continue;
        }
        if !rule.allow {
            return Firewall::Blocked;
        }
        allowed = true;
    }
    if allowed {
        Firewall::Allowed
    } else {
        Firewall::Unset
    }
}

/// What may keep other devices from a share at `address` (one of `lan_addresses`'s): asked once it's
/// ready, and again when Glint's window comes back to the front, after Windows' prompt or Settings.
#[tauri::command]
pub async fn lan_reach(address: String) -> Result<Reach, String> {
    let address: Ipv4Addr = address.parse().map_err(|_| "That isn't an address")?;
    // The firewall knows a program by where it really is: winget's link to dufs leads into its package.
    let program = crate::proc::locate("dufs").and_then(|(path, _)| std::fs::canonicalize(path).ok()).map(|path| {
        let path = path.to_string_lossy().into_owned();
        path.strip_prefix(r"\\?\").map_or(path.clone(), str::to_string)
    });
    // On a thread of its own, which can set COM up as it needs.
    blocking(move || std::thread::spawn(move || imp::reach(address, program.as_deref())).join().map_err(|_| "Couldn't tell".into())).await
}

/// Opens Windows' page for what keeps other devices from a share at `address`: "network", Settings for
/// the network it's on (where it's set as public or private), or "firewall", Windows Firewall's
/// allowed apps. Only opens it: what to change there is the person's.
#[tauri::command]
pub async fn lan_open_settings(page: String, address: String) -> Result<(), String> {
    let address: Ipv4Addr = address.parse().map_err(|_| "That isn't an address")?;
    blocking(move || match page.as_str() {
        "network" => std::thread::spawn(move || imp::open_network_settings(address)).join().map_err(|_| "Couldn't open Settings".to_string())?,
        "firewall" => imp::open_allowed_apps(),
        _ => Err(format!("No page is called {page}")),
    })
    .await
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
    use super::{profile_of, verdict, Address, Firewall, Reach, Rule};
    use std::net::Ipv4Addr;
    use std::path::Path;

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

    /// A network adapter that's up, with its IPv4 addresses (but loopback, link-local and unset ones).
    pub struct Adapter {
        pub name: String,
        /// Its GUID, as Windows names adapters: "{9195C9CA-…}".
        pub id: String,
        /// IF_TYPE_*: 6 is Ethernet, 71 Wi-Fi.
        pub kind: u32,
        pub ips: Vec<Ipv4Addr>,
        /// Traffic for the internet goes out by it.
        pub out: bool,
        pub gateway: bool,
        pub metric: u32,
    }

    pub fn adapters() -> Vec<Adapter> {
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
        let mut found = Vec::new();
        let mut at = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(adapter) = unsafe { at.as_ref() } {
            if adapter.OperStatus == IfOperStatusUp && adapter.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let index = unsafe { adapter.Anonymous1.Anonymous.IfIndex };
                let mut ips = Vec::new();
                let mut unicast = adapter.FirstUnicastAddress;
                while let Some(one) = unsafe { unicast.as_ref() } {
                    let socket = one.Address.lpSockaddr;
                    if !socket.is_null() && unsafe { (*socket).sa_family } == AF_INET {
                        let ip = Ipv4Addr::from(u32::from_be(unsafe { (*(socket as *const SOCKADDR_IN)).sin_addr.S_un.S_addr }));
                        if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
                            ips.push(ip);
                        }
                    }
                    unicast = one.Next;
                }
                found.push(Adapter {
                    name: unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default(),
                    id: unsafe { adapter.AdapterName.to_string() }.unwrap_or_default(),
                    kind: adapter.IfType,
                    ips,
                    out: out && index == best,
                    gateway: !adapter.FirstGatewayAddress.is_null(),
                    metric: adapter.Ipv4Metric,
                });
            }
            at = adapter.Next;
        }
        found
    }

    pub fn addresses() -> Vec<Address> {
        // (address, name, out by it, has a gateway, metric)
        let mut found: Vec<(Ipv4Addr, String, bool, bool, u32)> = adapters().into_iter().flat_map(|a| a.ips.into_iter().map(move |ip| (ip, a.name.clone(), a.out, a.gateway, a.metric))).collect();
        found.sort_by_key(|(ip, _, out, gateway, metric)| (!out, !gateway, !ip.is_private(), *metric));
        found.into_iter().map(|(ip, name, ..)| Address { address: ip.to_string(), name }).collect()
    }

    /// What may keep other devices from a share at `address` served by `program` (lan_reach). On a
    /// thread of its own: it sets COM up for itself.
    pub fn reach(address: Ipv4Addr, program: Option<&str>) -> Reach {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
        let Some(adapter) = adapters().into_iter().find(|a| a.ips.contains(&address)) else {
            return Reach { network: None, firewall: Firewall::Unknown };
        };
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let network = network_of(&adapter.id);
        let firewall = match (network, program) {
            (Some(network), Some(program)) => firewall(profile_of(network), program),
            _ => Firewall::Unknown,
        };
        if com.is_ok() {
            unsafe { CoUninitialize() };
        }
        Reach { network, firewall }
    }

    /// The kind of network Windows has the adapter `id` on ("public", "private" or "domain"), from its
    /// list of networks and their connections.
    fn network_of(id: &str) -> Option<&'static str> {
        use windows::Win32::Networking::NetworkListManager::{
            INetworkListManager, NetworkListManager, NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED, NLM_NETWORK_CATEGORY_PRIVATE, NLM_NETWORK_CATEGORY_PUBLIC,
        };
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
        let list: INetworkListManager = unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL) }.ok()?;
        let connections = unsafe { list.GetNetworkConnections() }.ok()?;
        let id = id.trim_matches(['{', '}']);
        loop {
            let mut one = [None];
            unsafe { connections.Next(&mut one, None) }.ok()?;
            // None once they've all been seen.
            let connection = one[0].take()?;
            if !format!("{:?}", unsafe { connection.GetAdapterId() }.ok()?).eq_ignore_ascii_case(id) {
                continue;
            }
            return match unsafe { connection.GetNetwork().and_then(|network| network.GetCategory()) }.ok()? {
                NLM_NETWORK_CATEGORY_PUBLIC => Some("public"),
                NLM_NETWORK_CATEGORY_PRIVATE => Some("private"),
                NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED => Some("domain"),
                _ => None,
            };
        }
    }

    /// What Windows Firewall does with connections coming in to `program` on a network of `profile`
    /// (a NET_FW_PROFILE2_* bit), from its settings there and the rules for a program of that name.
    fn firewall(profile: i32, program: &str) -> Firewall {
        use windows::core::Interface;
        use windows::Win32::NetworkManagement::WindowsFirewall::{INetFwPolicy2, INetFwRule, NetFwPolicy2, NET_FW_ACTION_ALLOW, NET_FW_PROFILE_TYPE2, NET_FW_RULE_DIR_IN};
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::System::Ole::IEnumVARIANT;
        use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_DISPATCH};
        let read = || -> windows::core::Result<Firewall> {
            let policy: INetFwPolicy2 = unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)? };
            let kind = NET_FW_PROFILE_TYPE2(profile);
            let on = unsafe { policy.get_FirewallEnabled(kind)? }.as_bool();
            let block_all = unsafe { policy.get_BlockAllInboundTraffic(kind)? }.as_bool();
            // Of the hundreds of rules, only those for a program of this one's name are read through.
            let name = Path::new(program).file_name().map_or(String::new(), |n| n.to_string_lossy().to_lowercase());
            let items: IEnumVARIANT = unsafe { policy.Rules()?._NewEnum()? }.cast()?;
            let mut rules = Vec::new();
            loop {
                let mut one = [VARIANT::default()];
                let mut fetched = 0u32;
                if unsafe { items.Next(&mut one, &mut fetched) }.is_err() || fetched == 0 {
                    break;
                }
                let rule = unsafe {
                    let value = &one[0].Anonymous.Anonymous;
                    if value.vt == VT_DISPATCH {
                        value.Anonymous.pdispVal.as_ref().and_then(|rule| rule.cast::<INetFwRule>().ok())
                    } else {
                        None
                    }
                };
                let _ = unsafe { VariantClear(&mut one[0]) };
                let Some(rule) = rule else { continue };
                let app = unsafe { rule.ApplicationName() }.map(|a| a.to_string()).unwrap_or_default();
                if !app.to_lowercase().ends_with(&name) {
                    continue;
                }
                rules.push(Rule {
                    program: app,
                    inbound: unsafe { rule.Direction() }.is_ok_and(|d| d == NET_FW_RULE_DIR_IN),
                    enabled: unsafe { rule.Enabled() }.is_ok_and(|e| e.as_bool()),
                    allow: unsafe { rule.Action() }.is_ok_and(|a| a == NET_FW_ACTION_ALLOW),
                    profiles: unsafe { rule.Profiles() }.unwrap_or(0),
                    protocol: unsafe { rule.Protocol() }.unwrap_or(0),
                });
            }
            Ok(verdict(on, block_all, profile, &rules, program))
        };
        read().unwrap_or(Firewall::Unknown)
    }

    /// Opens Settings on the network the adapter with `address` is on, where it's set as public or
    /// private: Ethernet's page or Wi-Fi's, else Network & internet. On a thread of its own.
    pub fn open_network_settings(address: Ipv4Addr) -> Result<(), String> {
        use windows::core::{w, HSTRING, PCWSTR};
        use windows::Win32::NetworkManagement::IpHelper::{IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211};
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let page = match adapters().into_iter().find(|a| a.ips.contains(&address)).map(|a| a.kind) {
            Some(IF_TYPE_ETHERNET_CSMACD) => "ms-settings:network-ethernet",
            Some(IF_TYPE_IEEE80211) => "ms-settings:network-wifi",
            _ => "ms-settings:network",
        };
        // Opened as the Start menu opens it: with the shell's "open", a COM verb.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        let file = HSTRING::from(page);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let opened = unsafe { ShellExecuteExW(&mut info) };
        if com.is_ok() {
            unsafe { CoUninitialize() };
        }
        opened.map_err(|e| format!("Couldn't open Settings: {}", e.message()))
    }

    /// Opens Windows Firewall's allowed apps, where an app is let in on private or public networks
    /// (changing them there asks for administrator approval). The page's name is the firewall control
    /// panel's own (FirewallControlPanel.dll).
    pub fn open_allowed_apps() -> Result<(), String> {
        let windows = std::env::var_os("SystemRoot").map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from);
        std::process::Command::new(windows.join(r"System32\control.exe"))
            .args(["/name", "Microsoft.WindowsFirewall", "/page", "PageConfigureApps"])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open Windows Firewall's settings: {e}"))
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
    fn tells_what_windows_firewall_does_with_dufs() {
        let dufs = r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\sigoden.Dufs_x\dufs.exe";
        // As Windows' prompt writes them: the path in lowercase but the drive, TCP and UDP apart.
        let rule = |allow, profiles, protocol| Rule { program: format!("C:{}", dufs[2..].to_lowercase()), inbound: true, enabled: true, allow, profiles, protocol };
        // Off on that kind of network, or keeping everything out there, whatever the rules say.
        assert_eq!(verdict(false, false, 4, &[rule(false, 4, TCP)], dufs), Firewall::Off);
        assert_eq!(verdict(true, true, 4, &[rule(true, 4, TCP)], dufs), Firewall::Blocked);
        // Its prompt dismissed on a public network: blocked there, nothing said for private ones.
        let dismissed = [rule(false, 4, TCP), rule(false, 4, 17)];
        assert_eq!(verdict(true, false, 4, &dismissed, dufs), Firewall::Blocked);
        assert_eq!(verdict(true, false, 2, &dismissed, dufs), Firewall::Unset);
        // Allowed there; and a rule that blocks wins over one that allows, as in Windows.
        assert_eq!(verdict(true, false, 4, &[rule(true, 6, TCP)], dufs), Firewall::Allowed);
        assert_eq!(verdict(true, false, 2, &[rule(true, 6, ANY_PROTOCOL), rule(false, 7, TCP)], dufs), Firewall::Blocked);
        // What doesn't bear on TCP coming in to this dufs on a public network.
        let others = [
            Rule { enabled: false, ..rule(false, 4, TCP) },
            Rule { inbound: false, ..rule(false, 4, TCP) },
            rule(false, 4, 17),
            rule(false, 3, TCP),
            Rule { program: r"C:\Tools\dufs.exe".into(), ..rule(false, 4, TCP) },
            Rule { program: String::new(), ..rule(false, 4, TCP) },
        ];
        assert_eq!(verdict(true, false, 4, &others, dufs), Firewall::Unset);
        // A rule's path with %variables% in it, in another case, is the same program.
        let program = format!(r"{}\Programs\Dufs\dufs.exe", std::env::var("LOCALAPPDATA").unwrap());
        let written = Rule { program: r"%LOCALAPPDATA%\programs\DUFS\DUFS.EXE".into(), ..rule(true, 4, TCP) };
        assert_eq!(verdict(true, false, 4, &[written], &program), Firewall::Allowed);
    }

    #[test]
    fn reads_what_windows_has_of_this_pcs_networks() {
        // The network traffic goes out by is one Windows has as public, private or domain, and its
        // firewall can be read there (a program it has no rules for: off, or kept out).
        if let Some(first) = imp::addresses().first() {
            let reach = imp::reach(first.address.parse().unwrap(), Some(r"C:\No\Such\dufs.exe"));
            assert!(reach.network.is_some() && matches!(reach.firewall, Firewall::Off | Firewall::Unset), "{reach:?}");
        }
        // An address that isn't this PC's says nothing.
        assert_eq!(imp::reach(Ipv4Addr::new(192, 0, 2, 1), Some(r"C:\No\Such\dufs.exe")), Reach { network: None, firewall: Firewall::Unknown });
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
        assert!(imp::addresses().iter().all(|a| !a.address.starts_with("127.")));
    }
}
