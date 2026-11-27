//! A Windows share as a connection (`\\nas\photos`, [#10](https://github.com/shadijhade/glint/issues/10)): Glint
//! signs in to it with Windows' own SMB client, the one File Explorer uses, and then reads and writes it at its own
//! path, as any folder on this PC. The sign-in is deviceless (`WNetAddConnection2` with no local name): no drive
//! letter, nothing in File Explorer, and Windows doesn't bring it back at the next logon, so Glint signs in again,
//! once a session, the first time it lists, opens or changes something there ([`ensure`]).
//!
//! The user name is the connection's; the password is in Windows Credential Manager with the other connections'
//! secrets ("<app>/remote/<id>"), never in Glint's settings. With neither, Windows signs in as this account, or
//! with what it already keeps for that server. Unlike the other connections, a share has no `remote:<id>` path:
//! its folders are `\\server\share\…`, so everything Glint does with files works there, read and write.
//!
//! Where Windows' client stops, Glint goes on:
//! - **Another account on the same server.** Windows lets a logon session reach a server with one account at a time
//!   (ERROR_SESSION_CREDENTIAL_CONFLICT). Its client tells servers apart by the name they're reached by, so Glint
//!   signs in through another name of the same server, an address its name has ([`aliases`]), and the share opens
//!   there. A connection is for its share by any name of its server ([`connection_for`]).
//! - **A refused password isn't tried again** on every listing: what Windows said is kept until the user name or
//!   password changes, since a NAS may lock an account after a few wrong ones.
//! - **Why, said plainly** ([`Trouble`]): a share that wants a user name and password, an account that's locked or
//!   off, a server that lets in guests only or doesn't sign what it sends (Windows 11 24H2 and later refuse both, and
//!   guests are refused by Windows' Pro editions before it too), a server that doesn't answer. The folder view offers
//!   what fits: signing in right there, opening it read-only through rclone, which reaches it on its own and leaves
//!   Windows' settings as they are (the connection's `via_rclone`), or waking it.
//! - **A sleeping NAS.** Once a share has opened, Glint notes its server's address and network card. When the server
//!   later doesn't answer, Glint says so at once ([`asleep`]) rather than after Windows' half-minute wait, can wake
//!   it with a Wake-on-LAN packet ([`wake`]), and the folder opens when it answers ([`answers`]).

use super::{secrets, state, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::{mpsc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The kind connections of a Windows share have.
pub const KIND: &str = "smb";

fn lock<T>(mutex: &'static Mutex<T>) -> MutexGuard<'static, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A share Glint signed in to this session: the connection's id, and the share as it was reached (itself, or the
/// same share on another name of its server).
#[derive(Clone, Debug)]
struct Signed {
    id: String,
    reached: String,
}

/// The shares Glint signed in to this session, by the share asked for (lower case).
static SIGNED_IN: LazyLock<Mutex<HashMap<String, Signed>>> = LazyLock::new(Default::default);

/// What Windows said to a connection's user name and password: the connection's id, which user name and password
/// were tried (a fingerprint, in memory only), and the answer.
type Tried = (String, u64, Refusal);

/// What was refused, by the share asked for (lower case).
static REFUSED: LazyLock<Mutex<HashMap<String, Tried>>> = LazyLock::new(Default::default);

/// Why Glint couldn't sign in to a share or open it, said for people, and the kind of trouble it is when the folder
/// view can do something about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub trouble: Option<Trouble>,
    pub message: String,
}

impl Refusal {
    fn of(trouble: Option<Trouble>, message: String) -> Self {
        Refusal { trouble, message }
    }
}

impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Refusal { trouble: None, message }
    }
}

/// What kind of trouble Windows has opening a share, when the folder view can help with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// It wants a user name and password: none was given, and this account isn't one it takes.
    SignIn,
    /// The user name and password Glint keeps for it were refused.
    WrongPassword,
    /// The account is known there, but may not open the share.
    NotAllowed,
    /// The account is locked out (too many wrong passwords), turned off, or its password expired.
    Locked,
    Disabled,
    Expired,
    /// Windows is signed in to the server as someone else, and no other name of it was found.
    OtherAccount,
    /// The server lets in guests only, and this Windows doesn't sign in as a guest.
    GuestOnly,
    /// The server doesn't sign what it sends, which this Windows requires.
    Unsigned,
    /// The server doesn't answer: asleep, off, or on another network. `wakeable`: Glint knows its network card.
    Asleep {
        wakeable: bool,
    },
}

impl Trouble {
    const ALL: [Trouble; 11] = [
        Trouble::SignIn,
        Trouble::WrongPassword,
        Trouble::NotAllowed,
        Trouble::Locked,
        Trouble::Disabled,
        Trouble::Expired,
        Trouble::OtherAccount,
        Trouble::GuestOnly,
        Trouble::Unsigned,
        Trouble::Asleep { wakeable: false },
        Trouble::Asleep { wakeable: true },
    ];

    /// Its word in a listing's error (`ListError::kind`), which the app reads back with [`Trouble::from_kind`].
    pub fn kind(self) -> &'static str {
        match self {
            Trouble::SignIn => "share-sign-in",
            Trouble::WrongPassword => "share-wrong-password",
            Trouble::NotAllowed => "share-not-allowed",
            Trouble::Locked => "share-locked",
            Trouble::Disabled => "share-disabled",
            Trouble::Expired => "share-expired",
            Trouble::OtherAccount => "share-other-account",
            Trouble::GuestOnly => "share-guest-only",
            Trouble::Unsigned => "share-unsigned",
            Trouble::Asleep { wakeable: false } => "share-asleep",
            Trouble::Asleep { wakeable: true } => "share-asleep-wakeable",
        }
    }

    pub fn from_kind(kind: &str) -> Option<Trouble> {
        Self::ALL.iter().copied().find(|t| t.kind() == kind)
    }

    /// Glint can open the share read-only on its own, through rclone, where Windows won't.
    pub fn around(self) -> bool {
        matches!(self, Trouble::GuestOnly | Trouble::Unsigned | Trouble::OtherAccount)
    }

    /// About the account tried: what was tried isn't tried again until it changes.
    fn about_account(self) -> bool {
        matches!(
            self,
            Trouble::SignIn | Trouble::WrongPassword | Trouble::Locked | Trouble::Disabled | Trouble::Expired
        )
    }
}

/// What Windows' answer `code` (a Win32 error, or the NTSTATUS some SMB answers come as) says about a share.
pub fn trouble_of(code: u32) -> Option<Trouble> {
    Some(match code {
        // ERROR_LOGON_FAILURE, ERROR_INVALID_PASSWORD, ERROR_BAD_USERNAME, ERROR_NOT_AUTHENTICATED,
        // ERROR_ACCOUNT_RESTRICTION (an empty password, say), ERROR_LOGON_TYPE_NOT_GRANTED.
        1326 | 86 | 2202 | 1244 | 1327 | 1385 => Trouble::SignIn,
        // ERROR_ACCESS_DENIED, ERROR_NETWORK_ACCESS_DENIED.
        5 | 65 => Trouble::NotAllowed,
        1909 => Trouble::Locked,
        1331 => Trouble::Disabled,
        // ERROR_PASSWORD_EXPIRED, ERROR_PASSWORD_MUST_CHANGE, ERROR_ACCOUNT_EXPIRED.
        1330 | 1907 | 1793 => Trouble::Expired,
        1219 => Trouble::OtherAccount,
        // ERROR_SMB_GUEST_LOGON_BLOCKED, STATUS_SMB_GUEST_LOGON_BLOCKED, and STATUS_SMB_NO_SIGNING_ALGORITHM_OVERLAP
        // (Windows 11 24H2's answer to a guest-only server: a guest can't sign).
        1272 | 0xC05D_0002 | 0xC05D_0003 => Trouble::GuestOnly,
        // STATUS_INVALID_SIGNATURE: the server doesn't sign, and Windows requires it.
        0xC000_A000 => Trouble::Unsigned,
        _ => return None,
    })
}

/// Windows 11 24H2 or later, which requires SMB signing by default and signs in as a guest on no edition: the folder
/// view says so when a server lets in guests only or doesn't sign.
pub fn strict_windows() -> bool {
    #[cfg(windows)]
    {
        windows_version::OsVersion::current().build >= 26100
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Windows' answers that mean the server didn't answer, or wasn't found: ERROR_BAD_NETPATH, ERROR_UNEXP_NET_ERR,
/// ERROR_NETNAME_DELETED, ERROR_SEM_TIMEOUT, ERROR_NETWORK_UNREACHABLE, ERROR_HOST_UNREACHABLE,
/// ERROR_CONNECTION_ABORTED.
fn unanswered(code: u32) -> bool {
    matches!(code, 53 | 59 | 64 | 121 | 1231 | 1232 | 1236)
}

/// The share a path is on (`\\server\share`, as the path spells it); None for a path that isn't on one, a server
/// on its own included, and for this PC's `\\?\` and `\\.\` paths and WSL's.
pub fn share_of(path: &str) -> Option<String> {
    let rest = path.strip_prefix("\\\\")?;
    let mut parts = rest.split('\\');
    let (server, share) = (parts.next()?, parts.next()?);
    let local = matches!(server, "" | "?" | ".")
        || server.eq_ignore_ascii_case("wsl.localhost")
        || server.eq_ignore_ascii_case("wsl$");
    (!local && !share.is_empty()).then(|| format!("\\\\{server}\\{share}"))
}

/// A share's address as it's typed (`\\nas\photos`, `//nas/photos`, `smb://nas/photos/2024`, `nas\photos\`) as
/// Glint's path for it (`\\nas\photos\2024`); None when it names no share.
pub fn address(typed: &str) -> Option<String> {
    let typed = typed.trim();
    let rest = match typed.get(..6) {
        Some(scheme) if scheme.eq_ignore_ascii_case("smb://") => &typed[6..],
        _ => typed,
    };
    let names: Vec<&str> = rest
        .split(['\\', '/'])
        .filter(|n| !n.trim().is_empty())
        .map(str::trim)
        .collect();
    // A server is a name or an address; "C:" is a drive (an IPv6 address is written as Windows' literal, with dashes).
    if names.len() < 2 || names[0].contains(':') {
        return None;
    }
    let path = format!("\\\\{}", names.join("\\"));
    share_of(&path).map(|_| path)
}

/// The server of a share or a path on one (`nas` for `\\nas\photos\a`).
pub fn server_of(share: &str) -> &str {
    share.trim_start_matches('\\').split('\\').next().unwrap_or(share)
}

/// The share's own name (`photos` for `\\nas\photos`), for messages.
fn name_of(share: &str) -> &str {
    share.trim_start_matches('\\').split('\\').nth(1).unwrap_or(share)
}

/// The share of a connection's address.
fn share_of_connection(connection: &Connection) -> Option<String> {
    address(&connection.endpoint).and_then(|a| share_of(&a))
}

/// The connection of the kind "smb" that's for `share` (compared in any case): the one added for it, else one for
/// the share of that name on another name of its server (an address it has, or the name it has): a tab left there
/// after Windows was signed in to the server as someone else, or the address typed for a share added by name.
/// Connections opened through rclone aren't signed in to through Windows.
fn connection_for(share: &str) -> Option<Connection> {
    let connections: Vec<Connection> = state()
        .connections
        .iter()
        .filter(|c| c.kind == KIND && !c.via_rclone)
        .cloned()
        .collect();
    if let Some(found) = connections
        .iter()
        .find(|c| share_of_connection(c).is_some_and(|s| s.eq_ignore_ascii_case(share)))
    {
        return Some(found.clone());
    }
    connections.into_iter().find(|c| {
        share_of_connection(c).is_some_and(|s| {
            name_of(&s).eq_ignore_ascii_case(name_of(share)) && same_server(server_of(&s), server_of(share))
        })
    })
}

/// A user name and password, as a number to tell a change by (kept in memory only).
fn fingerprint(user: &str, password: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (user.trim().to_lowercase(), password).hash(&mut hasher);
    hasher.finish()
}

/// `path` on the share reached instead of the one it names; None when it's the same.
fn rebase(path: &str, share: &str, reached: &str) -> Option<String> {
    (!share.eq_ignore_ascii_case(reached)).then(|| format!("{reached}{}", &path[share.len().min(path.len())..]))
}

/// Before Glint reads or changes `path`: when it's on a share a connection is for, signs in to that share with the
/// connection's user name and password, once a session. Nothing for any other path, nor for a share no connection
/// is for (Windows answers those as it would File Explorer). Returns `path` as it's reached when that's on another
/// name of its server (Windows was signed in to it as someone else), else None.
pub fn ensure(path: &str) -> Result<Option<String>, Refusal> {
    let Some(share) = share_of(path) else { return Ok(None) };
    let key = share.to_lowercase();
    if let Some(signed) = lock(&SIGNED_IN).get(&key) {
        return Ok(rebase(path, &share, &signed.reached));
    }
    // A path on another name of the connection's server signs in at that name.
    let Some(connection) = connection_for(&share) else {
        return Ok(None);
    };
    let password = secrets::read(&connection.id).unwrap_or_default();
    let tried = fingerprint(&connection.user, &password);
    if let Some((_, was, refusal)) = lock(&REFUSED).get(&key) {
        if *was == tried {
            return Err(refusal.clone());
        }
    }
    let kept = !connection.user.trim().is_empty() || !password.is_empty();
    match connect(&share, &connection.user, &password) {
        Ok(reached) => {
            lock(&REFUSED).remove(&key);
            let moved = rebase(path, &share, &reached);
            lock(&SIGNED_IN).insert(
                key,
                Signed {
                    id: connection.id,
                    reached,
                },
            );
            Ok(moved)
        }
        // Windows reaches the server as someone else and it has no other name: that account it is (it may be let in).
        Err(refusal) if refusal.trouble == Some(Trouble::OtherAccount) => {
            lock(&SIGNED_IN).insert(
                key,
                Signed {
                    id: connection.id,
                    reached: share,
                },
            );
            Ok(None)
        }
        Err(mut refusal) => {
            if refusal.trouble == Some(Trouble::SignIn) && kept {
                refusal.trouble = Some(Trouble::WrongPassword);
            }
            // Not tried again until the user name or password changes: a NAS may lock the account after a few.
            if refusal.trouble.is_some_and(Trouble::about_account) {
                lock(&REFUSED).insert(key, (connection.id, tried, refusal.clone()));
            }
            Err(refusal)
        }
    }
}

/// [`ensure`] for each share among `paths`, once each.
pub fn ensure_all<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for path in paths {
        if let Some(share) = share_of(path) {
            if seen.insert(share.to_lowercase()) {
                ensure(path).map_err(|r| r.message)?;
            }
        }
    }
    Ok(())
}

/// Forgets that Glint signed in to the shares of these connections and signs out of them (Windows' connection, not
/// anything open there), so the next use signs in with what they say now; and forgets what was refused.
pub(crate) fn forget(connections: &[Connection]) {
    let ids: HashSet<&str> = connections
        .iter()
        .filter(|c| c.kind == KIND)
        .map(|c| c.id.as_str())
        .collect();
    if ids.is_empty() {
        return;
    }
    lock(&REFUSED).retain(|_, (id, _, _)| !ids.contains(id.as_str()));
    let mut gone = Vec::new();
    lock(&SIGNED_IN).retain(|_, signed| {
        let keep = !ids.contains(signed.id.as_str());
        if !keep && !gone.contains(&signed.reached) {
            gone.push(signed.reached.clone());
        }
        keep
    });
    for share in gone {
        disconnect(&share);
    }
}

/// The connections of the kind "smb" that are gone from `now`, or changed in it (another share or user name, or now
/// opened through rclone or Windows instead).
pub(crate) fn changed(before: &[Connection], now: &[Connection]) -> Vec<Connection> {
    before
        .iter()
        .filter(|c| c.kind == KIND)
        .filter(|c| {
            !now.iter().any(|n| {
                n.id == c.id
                    && n.kind == KIND
                    && n.endpoint == c.endpoint
                    && n.user == c.user
                    && n.via_rclone == c.via_rclone
            })
        })
        .cloned()
        .collect()
}

/// Tries a share's connection as typed: signs in with `password` (Glint's own earlier sign-in to it signed out
/// first, so these are the ones tried) and says what's at its top, or why it didn't connect.
pub fn test(connection: &Connection, password: &str) -> Result<String, String> {
    let path =
        address(&connection.endpoint).ok_or("Which share? Its address goes in the connection, as \\\\server\\share")?;
    let share = share_of(&path).ok_or("Which share? Its address goes in the connection, as \\\\server\\share")?;
    if !connection.id.is_empty() {
        forget(std::slice::from_ref(connection));
    }
    // A sign-in to the share for a connection as it was named before (a new one's id follows its name).
    let earlier = lock(&SIGNED_IN).remove(&share.to_lowercase());
    if let Some(earlier) = earlier {
        disconnect(&earlier.reached);
    }
    lock(&REFUSED).remove(&share.to_lowercase());
    let reached = match connect(&share, &connection.user, password) {
        Ok(reached) => reached,
        Err(refusal) => return Err(refusal.message),
    };
    lock(&SIGNED_IN).insert(
        share.to_lowercase(),
        Signed {
            id: connection.id.clone(),
            reached: reached.clone(),
        },
    );
    let at = rebase(&path, &share, &reached).unwrap_or(path);
    let items = std::fs::read_dir(&at).map_err(|e| crate::fsx::describe(&e))?.count();
    Ok(match items {
        0 => "Connected: there's nothing there yet".into(),
        1 => "Connected: 1 item at its top".into(),
        n => format!("Connected: {n} items at its top"),
    })
}

/// What Glint says when Windows answered `code` to signing in to (or opening) `share`; a server that doesn't answer
/// is asked once more (a quick knock) to tell asleep from not found.
pub fn refusal(share: &str, code: u32) -> Refusal {
    let (server, name) = (server_of(share), name_of(share));
    let trouble = trouble_of(code);
    let message = match trouble {
        Some(Trouble::SignIn | Trouble::WrongPassword) => format!("{server} didn't take that user name and password"),
        Some(Trouble::NotAllowed) => format!("{server} doesn't let that account into “{name}”"),
        Some(Trouble::Locked) => format!("{server} locked that account after too many wrong passwords"),
        Some(Trouble::Disabled) => format!("That account is turned off on {server}"),
        Some(Trouble::Expired) => format!("That account's password has expired on {server}"),
        Some(Trouble::OtherAccount) => format!(
            "Windows is already signed in to {server} as someone else (in File Explorer, or a mapped drive), and lets this PC use one account per server"
        ),
        Some(Trouble::GuestOnly) => format!("{server} lets in guests only, and Windows here doesn't sign in as a guest"),
        Some(Trouble::Unsigned) => format!("{server} doesn't sign what it sends, which Windows here requires"),
        Some(Trouble::Asleep { .. }) | None => match code {
            67 => format!("{server} has no share named “{name}”"),
            1222 => "This PC isn't on a network".into(),
            code if unanswered(code) => {
                return match knock(server) {
                    Knock::Silent { wakeable } => Refusal::of(
                        Some(Trouble::Asleep { wakeable }),
                        format!("{server} isn't answering: it may be asleep, off, or on another network"),
                    ),
                    Knock::Unknown => Refusal::of(None, format!("Glint can't find {server} on this network")),
                    Knock::Answered => Refusal::of(None, format!("Glint can't reach {server}: {}", message_of(code))),
                }
            }
            code => format!("Windows couldn't open {share}: {}", message_of(code)),
        },
    };
    Refusal::of(trouble, message)
}

/// Windows' own words for an error code.
fn message_of(code: u32) -> String {
    #[cfg(windows)]
    {
        windows::core::Error::from_hresult(windows::Win32::Foundation::WIN32_ERROR(code).to_hresult())
            .message()
            .trim_end_matches(['.', ' ', '\r', '\n'])
            .to_string()
    }
    #[cfg(not(windows))]
    {
        format!("error {code}")
    }
}

/// Why Windows wouldn't list `path` (its error `code`), when it's on a share or names a server: the trouble the
/// folder view can help with, or None to say it as before.
pub fn listing_refused(path: &str, code: u32) -> Option<Refusal> {
    let share = share_of(path).or_else(|| path.strip_prefix(r"\\").map(|_| format!(r"\\{}\", server_of(path))))?;
    // A folder that isn't there (anymore) and the like keep their own words.
    if trouble_of(code).is_none() && !unanswered(code) && code != 67 {
        return None;
    }
    let refusal = refusal(&share, code);
    // A server on its own lists its shares, which needs no sign-in of a share: only "not answering" is said for it.
    match (share_of(path).is_some(), refusal.trouble) {
        (_, Some(Trouble::Asleep { .. })) | (true, _) => Some(refusal),
        (false, _) => None,
    }
}

/// Signs in to `share` (`\\server\share`) with Windows' SMB client, deviceless and not remembered: with `user` and
/// `password`, or with neither as this account (or what Windows keeps for the server). Returns the share as reached:
/// itself, or the same share on another name of its server when Windows was signed in there as someone else.
#[cfg(windows)]
pub fn connect(share: &str, user: &str, password: &str) -> Result<String, Refusal> {
    let user = user.trim();
    // Neither: this account's sign-in, or what Windows keeps for the server.
    let own = user.is_empty() && password.is_empty();
    match add(share, user, password, own) {
        0 => Ok(share.to_string()),
        // Signed in already as someone else; with nothing of our own to sign in with, that's the one to use.
        1219 if own => Ok(share.to_string()),
        // Windows' client tells servers apart by name: the same server by another name has no sign-in yet.
        1219 => {
            for alias in aliases(server_of(share)) {
                let other = format!(r"\\{alias}\{}", name_of(share));
                match add(&other, user, password, own) {
                    0 => return Ok(other),
                    1219 => continue,
                    code => return Err(refusal(&other, code)),
                }
            }
            Err(refusal(share, 1219))
        }
        code => Err(refusal(share, code)),
    }
}

#[cfg(not(windows))]
pub fn connect(_share: &str, _user: &str, _password: &str) -> Result<String, Refusal> {
    Err(Refusal::from("Windows shares open on Windows".to_string()))
}

/// `WNetAddConnection2` for `share`, deviceless and not remembered: Windows' answer.
#[cfg(windows)]
fn add(share: &str, user: &str, password: &str, own: bool) -> u32 {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::NetworkManagement::WNet::{
        WNetAddConnection2W, CONNECT_TEMPORARY, NETRESOURCEW, RESOURCETYPE_DISK,
    };
    let remote = HSTRING::from(share);
    let resource = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        lpRemoteName: PWSTR(remote.as_ptr() as *mut u16),
        ..Default::default()
    };
    let (user_w, password_w) = (HSTRING::from(user), HSTRING::from(password));
    let user_arg = if user.is_empty() {
        PCWSTR::null()
    } else {
        PCWSTR(user_w.as_ptr())
    };
    // Null asks Windows for its own; a user name with an empty password is an empty one.
    let password_arg = if own {
        PCWSTR::null()
    } else {
        PCWSTR(password_w.as_ptr())
    };
    // Safety: the strings live through the call, and the structure is read only during it.
    unsafe { WNetAddConnection2W(&resource, password_arg, user_arg, CONNECT_TEMPORARY) }.0
}

/// Signs out of `share`'s deviceless connections in this session; a drive letter mapped to it stays, and so does
/// anything open there (Windows then keeps the connection until it's closed).
#[cfg(windows)]
pub fn disconnect(share: &str) {
    use windows::Win32::NetworkManagement::WNet::{WNetCancelConnection2W, NET_CONNECT_FLAGS};
    // Safety: the string lives through the call.
    let _ = unsafe { WNetCancelConnection2W(&windows::core::HSTRING::from(share), NET_CONNECT_FLAGS(0), false) };
}

#[cfg(not(windows))]
pub fn disconnect(_share: &str) {}

// ---- Another name of the same server ------------------------------------------------------------------------------

/// The addresses names resolved to this session (with their IPv6 zones), and when: a name that doesn't resolve can
/// take seconds to say so.
static RESOLVED: LazyLock<Mutex<HashMap<String, Resolved>>> = LazyLock::new(Default::default);

/// When a name was resolved, and to what.
type Resolved = (Instant, Vec<SocketAddr>);

/// How long an answer about a name is used before it's asked again.
const RESOLVED_FOR: Duration = Duration::from_secs(120);

/// What the name `server` resolves to, IPv4 first; nothing when it doesn't resolve within two seconds.
fn resolve(server: &str) -> Vec<SocketAddr> {
    let key = server.to_lowercase();
    if let Some((at, found)) = lock(&RESOLVED).get(&key) {
        if at.elapsed() < RESOLVED_FOR {
            return found.clone();
        }
    }
    let (tx, rx) = mpsc::channel();
    let name = server.to_string();
    std::thread::spawn(move || {
        let found: Vec<SocketAddr> = (name.as_str(), 445)
            .to_socket_addrs()
            .map(Iterator::collect)
            .unwrap_or_default();
        let _ = tx.send(found);
    });
    let mut found = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    found.sort_by_key(SocketAddr::is_ipv6);
    found.dedup();
    lock(&RESOLVED).insert(key, (Instant::now(), found.clone()));
    found
}

/// The addresses of `server` (a name or an address), IPv4 first. An address is its own.
fn addresses(server: &str) -> Vec<IpAddr> {
    if let Some(ip) = ip_of(server) {
        return vec![ip];
    }
    let mut found: Vec<IpAddr> = resolve(server).iter().map(SocketAddr::ip).collect();
    found.dedup();
    found
}

/// `server` as an address: dotted IPv4, or Windows' IPv6 literal (`fe80--1s12.ipv6-literal.net`).
fn ip_of(server: &str) -> Option<IpAddr> {
    if let Ok(ip) = server.parse::<Ipv4Addr>() {
        return Some(IpAddr::V4(ip));
    }
    let literal = server.to_ascii_lowercase();
    let body = literal.strip_suffix(".ipv6-literal.net")?;
    let without_zone = body.split('s').next()?;
    without_zone.replace('-', ":").parse::<Ipv6Addr>().ok().map(IpAddr::V6)
}

/// An IPv6 address as a server name Windows' SMB client takes: colons as dashes, its zone after an "s".
fn literal(ip: Ipv6Addr, zone: u32) -> String {
    let zone = if zone == 0 { String::new() } else { format!("s{zone}") };
    format!("{}{zone}.ipv6-literal.net", ip.to_string().replace(':', "-"))
}

/// Other names of `server` that Windows' SMB client takes for another server, and that lead to the same one: the
/// addresses its name has, IPv4 first. Nothing for an address (it has no other name Glint can be sure of).
pub fn aliases(server: &str) -> Vec<String> {
    if ip_of(server).is_some() {
        return Vec::new();
    }
    let mut names = Vec::new();
    for address in resolve(server) {
        let name = match address {
            SocketAddr::V4(v4) => v4.ip().to_string(),
            SocketAddr::V6(v6) => literal(*v6.ip(), v6.scope_id()),
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names.truncate(3);
    names
}

/// Whether two names (or addresses) are the same server: the same name, or names with an address in common.
fn same_server(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    let theirs = addresses(a);
    !theirs.is_empty() && addresses(b).iter().any(|ip| theirs.contains(ip))
}

// ---- A server that doesn't answer -----------------------------------------------------------------------------------

/// What Glint noted about a server once a share there opened: its IPv4 address, its network card (empty when it's
/// not on this PC's network), and when, in seconds since 1970.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Noted {
    ip: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    mac: String,
    #[serde(default)]
    at: u64,
}

/// The servers noted, by name in lower case (read from Glint's data folder when first asked).
static NOTED: LazyLock<Mutex<Option<HashMap<String, Noted>>>> = LazyLock::new(Default::default);

/// When servers last answered this session, by name in lower case.
static ANSWERED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// The servers looked at to be noted this session.
static LEARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// A server that answered this recently isn't knocked on again before Glint opens a share there.
const FRESH: Duration = Duration::from_secs(60);

/// How long a knock waits for an answer: a server on the network answers in milliseconds.
const KNOCK_WAIT: Duration = Duration::from_millis(1200);

fn noted_file() -> Option<std::path::PathBuf> {
    crate::tools::data_dir().map(|d| d.join("shares.json"))
}

fn noted(server: &str) -> Option<Noted> {
    let mut all = lock(&NOTED);
    let all = all.get_or_insert_with(|| {
        noted_file()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    all.get(&server.to_lowercase()).cloned()
}

fn note(server: &str, seen: Noted) {
    let text = {
        let mut all = lock(&NOTED);
        let all = all.get_or_insert_with(Default::default);
        if all.get(&server.to_lowercase()) == Some(&seen) {
            return;
        }
        all.insert(server.to_lowercase(), seen);
        serde_json::to_vec_pretty(&*all).ok()
    };
    if let (Some(file), Some(text)) = (noted_file(), text) {
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(file, text);
    }
}

/// A folder on a share opened (or a server's shares were listed): the server answered. The first time this session,
/// its address and network card are noted, on a thread of their own, for telling later whether it's asleep and for
/// waking it.
pub fn opened(path: &str) {
    if !path.starts_with(r"\\") {
        return;
    }
    let server = server_of(path).to_lowercase();
    if server.is_empty() || server == "?" || server == "." {
        return;
    }
    lock(&ANSWERED).insert(server.clone(), Instant::now());
    if !lock(&LEARNED).insert(server.clone()) {
        return;
    }
    std::thread::spawn(move || learn(&server));
}

/// Notes `server`'s IPv4 address and, when it's on this PC's network, its network card: only once Glint has seen
/// it answer as an SMB server over TCP (445 or 139), so a server reached only otherwise is never taken for asleep.
fn learn(server: &str) {
    let Some(ip) = addresses(server).into_iter().find_map(|ip| match ip {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
        _ => None,
    }) else {
        return;
    };
    if !probe(&[IpAddr::V4(ip)], KNOCK_WAIT) {
        return;
    }
    let mac = mac_of(ip).map(|m| hex(&m)).unwrap_or_default();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    note(
        server,
        Noted {
            ip: ip.to_string(),
            mac,
            at,
        },
    );
}

/// Where to knock for `server`: the addresses its name has now, and the one noted (a sleeping NAS doesn't answer
/// the questions that find it by name either).
fn targets(server: &str) -> (Vec<IpAddr>, Option<Noted>) {
    let seen = noted(server);
    let mut found = addresses(server);
    if let Some(ip) = seen.as_ref().and_then(|s| s.ip.parse::<IpAddr>().ok()) {
        if !found.contains(&ip) {
            found.push(ip);
        }
    }
    (found, seen)
}

enum Knock {
    Answered,
    /// Nothing answered at its addresses; `wakeable`: its network card is noted.
    Silent {
        wakeable: bool,
    },
    /// No address for it at all.
    Unknown,
}

fn knock(server: &str) -> Knock {
    let (found, seen) = targets(server);
    if found.is_empty() {
        return Knock::Unknown;
    }
    if probe(&found, KNOCK_WAIT) {
        lock(&ANSWERED).insert(server.to_lowercase(), Instant::now());
        lock(&SILENT).remove(&server.to_lowercase());
        Knock::Answered
    } else {
        Knock::Silent {
            wakeable: seen.is_some_and(|s| !s.mac.is_empty()),
        }
    }
}

/// Before Glint opens `path`: when it's on a server noted before that hasn't answered in the last minute, knocks
/// (a second at most) and says it's asleep when nothing answers, instead of waiting on Windows' half-minute try.
/// None when it answered, or isn't a server Glint knows.
pub fn asleep(path: &str) -> Option<Refusal> {
    if !path.starts_with(r"\\") || path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return None;
    }
    let server = server_of(path);
    if server.is_empty()
        || lock(&ANSWERED)
            .get(&server.to_lowercase())
            .is_some_and(|at| at.elapsed() < FRESH)
    {
        return None;
    }
    let seen = noted(server)?;
    let said = |wakeable: bool| {
        Refusal::of(
            Some(Trouble::Asleep { wakeable }),
            format!("{server} isn't answering: it may be asleep, off, or on another network"),
        )
    };
    // Knocked on a moment ago without an answer (checking a typed path, then opening it): not knocked on again.
    if lock(&SILENT)
        .get(&server.to_lowercase())
        .is_some_and(|at| at.elapsed() < SILENT_FOR)
    {
        return Some(said(!seen.mac.is_empty()));
    }
    match knock(server) {
        Knock::Silent { wakeable } => {
            lock(&SILENT).insert(server.to_lowercase(), Instant::now());
            Some(said(wakeable))
        }
        Knock::Answered | Knock::Unknown => None,
    }
}

/// Servers knocked on without an answer, and when: a typed path is checked and then opened, one knock for both.
static SILENT: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// How long a knock nothing answered stands for.
const SILENT_FOR: Duration = Duration::from_secs(5);

/// Whether `server` answers now, for the folder view to open a share there as soon as it does.
pub fn answers(server: &str) -> bool {
    matches!(knock(server), Knock::Answered)
}

/// Wakes `server` with a Wake-on-LAN packet to its noted network card: on this PC's network (sent from the address
/// this PC reaches it by), to its /24 network and straight to its address, on ports 9 and 7.
pub fn wake(server: &str) -> Result<(), String> {
    let seen = noted(server)
        .filter(|s| !s.mac.is_empty())
        .ok_or_else(|| format!("Glint hasn't seen {server}'s network card yet"))?;
    let mac = unhex(&seen.mac).ok_or_else(|| format!("Glint hasn't seen {server}'s network card yet"))?;
    let ip: Ipv4Addr = seen
        .ip
        .parse()
        .map_err(|_| format!("Glint hasn't seen {server}'s address yet"))?;
    lock(&ANSWERED).remove(&server.to_lowercase());
    send_wake(ip, mac)
}

/// A Wake-on-LAN packet: six 0xFF bytes, then the network card's address sixteen times.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xFF; 102];
    for copy in packet[6..].as_chunks_mut::<6>().0 {
        *copy = mac;
    }
    packet
}

fn send_wake(ip: Ipv4Addr, mac: [u8; 6]) -> Result<(), String> {
    let packet = magic_packet(mac);
    // The address this PC reaches the server's network by (a UDP "connection" sends nothing), so a broadcast leaves
    // on that network rather than the default one.
    let local = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| s.connect((ip, 9)).and_then(|()| s.local_addr()))
        .map(|a| a.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let socket = UdpSocket::bind((local, 0)).map_err(|e| format!("Glint couldn't send the wake-up: {e}"))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("Glint couldn't send the wake-up: {e}"))?;
    let [a, b, c, _] = ip.octets();
    let mut sent = false;
    for target in [Ipv4Addr::BROADCAST, Ipv4Addr::new(a, b, c, 255), ip] {
        for port in [9, 7] {
            sent |= socket.send_to(&packet, (target, port)).is_ok();
        }
    }
    sent.then_some(())
        .ok_or_else(|| "Glint couldn't send the wake-up on this network".into())
}

/// Whether anything answers at `targets` as an SMB server over TCP (445, or 139 for SMB over NetBIOS) within
/// `wait`: all knocked on at once, the first answer is enough.
fn probe(targets: &[IpAddr], wait: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    for ip in targets.iter().take(3) {
        for port in [445, 139] {
            let (tx, address) = (tx.clone(), SocketAddr::new(*ip, port));
            std::thread::spawn(move || {
                let _ = tx.send(TcpStream::connect_timeout(&address, wait).is_ok());
            });
        }
    }
    drop(tx);
    let until = Instant::now() + wait + Duration::from_millis(100);
    while let Ok(answered) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        if answered {
            return true;
        }
    }
    false
}

/// The network card of `ip` on this PC's network, by ARP; None off it, or when nothing answers.
#[cfg(windows)]
fn mac_of(ip: Ipv4Addr) -> Option<[u8; 6]> {
    use windows::Win32::NetworkManagement::IpHelper::SendARP;
    let mut mac = [0u8; 8];
    let mut length = mac.len() as u32;
    // Safety: the buffer and its length live through the call, which writes at most `length` bytes.
    let status = unsafe { SendARP(u32::from_ne_bytes(ip.octets()), 0, mac.as_mut_ptr().cast(), &mut length) };
    let card: [u8; 6] = mac[..6].try_into().ok()?;
    (status == 0 && length >= 6 && card != [0; 6]).then_some(card)
}

#[cfg(not(windows))]
fn mac_of(_ip: Ipv4Addr) -> Option<[u8; 6]> {
    None
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn unhex(text: &str) -> Option<[u8; 6]> {
    let digits: String = text.chars().filter(char::is_ascii_hexdigit).collect();
    if digits.len() != 12 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

// ---- Through rclone ------------------------------------------------------------------------------------------------

/// A share's connection opened through rclone, read-only, where Windows won't sign in (guests only, no signing,
/// another account already signed in): rclone's own SMB client reaches the server, and Windows' settings stay as
/// they are. The server, the share and the folder in it, and the account (`DOMAIN\name` split; a guest without one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rclone {
    pub host: String,
    pub share_path: String,
    pub user: String,
    pub domain: String,
}

impl Rclone {
    pub fn of(connection: &Connection) -> Result<Rclone, String> {
        let path = address(&connection.endpoint)
            .ok_or("Which share? Its address goes in the connection, as \\\\server\\share")?;
        let names: Vec<&str> = path.trim_start_matches('\\').split('\\').collect();
        let host = match ip_of(names[0]) {
            Some(IpAddr::V6(v6)) => v6.to_string(),
            _ => names[0].to_string(),
        };
        let typed = connection.user.trim();
        let (domain, user) = match typed.split_once('\\') {
            Some((domain, user)) => (domain.trim().to_string(), user.trim().to_string()),
            None => (String::new(), typed.to_string()),
        };
        Ok(Rclone {
            host,
            share_path: names[1..].join("/"),
            user: if user.is_empty() { "guest".into() } else { user },
            domain,
        })
    }

    /// rclone's on-the-fly remote for it, with the password rclone obscured (`obscured`, empty for none):
    /// `:smb,host="nas",user="guest":photos`. Values are quoted, quotes in them doubled.
    pub fn remote(&self, obscured: &str) -> String {
        let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
        let mut options = vec![
            format!("host={}", quote(&self.host)),
            format!("user={}", quote(&self.user)),
        ];
        if !self.domain.is_empty() {
            options.push(format!("domain={}", quote(&self.domain)));
        }
        if !obscured.is_empty() {
            options.push(format!("pass={}", quote(obscured)));
        }
        format!(":smb,{}", options.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_is_its_server_and_name_and_typed_addresses_become_glints_paths() {
        assert_eq!(
            share_of(r"\\192.168.1.6\share").as_deref(),
            Some(r"\\192.168.1.6\share")
        );
        assert_eq!(share_of(r"\\nas\Photos\2024\a.jpg").as_deref(), Some(r"\\nas\Photos"));
        for not_one in [
            r"\\nas",
            r"\\nas\",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            r"\\wsl.localhost\Ubuntu\home",
            r"C:\Users",
            "remote:nas",
            "",
        ] {
            assert_eq!(share_of(not_one), None, "{not_one}");
        }
        for (typed, path) in [
            (r"\\192.168.1.6\share", r"\\192.168.1.6\share"),
            (r"  \\nas\photos\  ", r"\\nas\photos"),
            ("//nas/photos/2024", r"\\nas\photos\2024"),
            ("smb://nas/photos/2024/", r"\\nas\photos\2024"),
            ("SMB://nas/photos", r"\\nas\photos"),
            (r"nas\photos", r"\\nas\photos"),
        ] {
            assert_eq!(address(typed).as_deref(), Some(path), "{typed}");
        }
        for nothing in [
            "",
            "nas",
            r"\\nas",
            "smb://",
            r"C:\x",
            "D:/photos",
            r"\\?\C:\x",
            r"\\wsl$\Ubuntu",
        ] {
            assert_eq!(address(nothing), None, "{nothing}");
        }
    }

    #[test]
    fn a_connection_that_changed_or_went_is_told_apart_from_one_that_did_not() {
        let share = |id: &str, endpoint: &str, user: &str| Connection {
            id: id.into(),
            kind: KIND.into(),
            endpoint: endpoint.into(),
            user: user.into(),
            ..Default::default()
        };
        let before = vec![
            share("a", r"\\nas\a", "me"),
            share("b", r"\\nas\b", ""),
            share("c", r"\\nas\c", ""),
            share("d", r"\\nas\d", ""),
        ];
        let mut now = vec![
            share("a", r"\\nas\a", "me"),
            share("b", r"\\nas\b", "you"),
            share("d", r"\\nas\d", ""),
        ];
        now[0].name = "Renamed".into();
        now[2].via_rclone = true;
        let gone: Vec<String> = changed(&before, &now).into_iter().map(|c| c.id).collect();
        assert_eq!(
            gone,
            ["b", "c", "d"],
            "a rename keeps the sign-in; another user, a removal or opening it through rclone doesn't"
        );
    }

    #[test]
    fn windows_answers_are_read_as_the_trouble_they_are() {
        for (code, trouble) in [
            (1326, Trouble::SignIn),
            (86, Trouble::SignIn),
            (1244, Trouble::SignIn),
            (5, Trouble::NotAllowed),
            (1909, Trouble::Locked),
            (1331, Trouble::Disabled),
            (1330, Trouble::Expired),
            (1907, Trouble::Expired),
            (1219, Trouble::OtherAccount),
            (1272, Trouble::GuestOnly),
            (0xC05D_0003, Trouble::GuestOnly),
            (0xC000_A000, Trouble::Unsigned),
        ] {
            assert_eq!(trouble_of(code), Some(trouble), "{code:#x}");
        }
        for neither in [0, 2, 3, 53, 67, 1222] {
            assert_eq!(trouble_of(neither), None, "{neither}");
        }
        for trouble in Trouble::ALL {
            assert_eq!(
                Trouble::from_kind(trouble.kind()),
                Some(trouble),
                "{trouble:?} survives the trip to the app"
            );
        }
        assert_eq!(Trouble::from_kind("denied"), None);
        assert!(Trouble::GuestOnly.around() && Trouble::Unsigned.around() && Trouble::OtherAccount.around());
        assert!(!Trouble::WrongPassword.around() && !Trouble::Asleep { wakeable: true }.around());
        let said = refusal(r"\\nas\photos", 1272);
        assert_eq!(said.trouble, Some(Trouble::GuestOnly));
        assert_eq!(
            said.message,
            "nas lets in guests only, and Windows here doesn't sign in as a guest"
        );
        assert_eq!(refusal(r"\\nas\photos", 67).message, "nas has no share named “photos”");
        assert_eq!(refusal(r"\\nas\photos", 67).trouble, None);
        // A listing's error: a share's trouble or Windows' words for a missing share; a missing folder keeps its own.
        assert_eq!(
            listing_refused(r"\\nas\photos\2024", 1272).and_then(|r| r.trouble),
            Some(Trouble::GuestOnly)
        );
        assert_eq!(
            listing_refused(r"\\nas\photos\2024", 5).and_then(|r| r.trouble),
            Some(Trouble::NotAllowed)
        );
        assert_eq!(
            listing_refused(r"\\nas\nope", 67).map(|r| r.message).as_deref(),
            Some("nas has no share named “nope”")
        );
        assert_eq!(listing_refused(r"\\nas\photos\gone", 3), None);
        assert_eq!(listing_refused(r"C:\Users", 1326), None, "only a share's path");
        assert_eq!(
            listing_refused(r"\\nas", 1326),
            None,
            "a server's list of shares needs no sign-in of a share"
        );
    }

    #[test]
    fn a_path_on_a_share_reached_by_another_name_moves_there() {
        assert_eq!(
            rebase(r"\\nas\photos\2024\a.jpg", r"\\nas\photos", r"\\192.168.1.6\photos").as_deref(),
            Some(r"\\192.168.1.6\photos\2024\a.jpg")
        );
        assert_eq!(
            rebase(r"\\nas\photos", r"\\nas\photos", r"\\192.168.1.6\photos").as_deref(),
            Some(r"\\192.168.1.6\photos")
        );
        assert_eq!(
            rebase(r"\\NAS\Photos\x", r"\\NAS\Photos", r"\\nas\photos"),
            None,
            "the same share in another case stays"
        );
    }

    #[test]
    fn addresses_are_their_own_and_ipv6_ones_are_windows_literals() {
        assert_eq!(ip_of("192.168.1.6"), Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 6))));
        assert_eq!(
            ip_of("fe80--1s12.ipv6-literal.net"),
            Some(IpAddr::V6("fe80::1".parse().unwrap()))
        );
        assert_eq!(ip_of("nas"), None);
        assert_eq!(literal("fe80::1".parse().unwrap(), 12), "fe80--1s12.ipv6-literal.net");
        assert_eq!(
            literal("2001:db8::7".parse().unwrap(), 0),
            "2001-db8--7.ipv6-literal.net"
        );
        assert!(
            aliases("192.168.1.6").is_empty(),
            "an address has no other name Glint is sure of"
        );
        assert!(same_server("NAS", "nas"));
        assert!(
            same_server("localhost", "127.0.0.1"),
            "a name and its address are one server"
        );
        assert!(!same_server("127.0.0.1", "192.0.2.1"));
    }

    #[test]
    fn a_wake_up_is_six_ffs_and_the_card_sixteen_times() {
        let mac = [0x00, 0x11, 0x32, 0xAA, 0xBB, 0xCC];
        let packet = magic_packet(mac);
        assert_eq!(packet[..6], [0xFF; 6]);
        assert!(packet[6..].chunks(6).all(|c| c == mac));
        assert_eq!(hex(&mac), "00:11:32:AA:BB:CC");
        assert_eq!(unhex("00:11:32:aa:bb:cc"), Some(mac));
        assert_eq!(unhex("00-11-32-AA-BB-CC"), Some(mac));
        assert_eq!(unhex("00:11"), None);
    }

    #[test]
    fn a_share_opened_through_rclone_is_rclones_own_remote() {
        let connection = |endpoint: &str, user: &str| Connection {
            id: "nas".into(),
            kind: KIND.into(),
            endpoint: endpoint.into(),
            user: user.into(),
            via_rclone: true,
            ..Default::default()
        };
        let guest = Rclone::of(&connection(r"\\192.168.1.6\share", "")).unwrap();
        assert_eq!(guest.remote(""), r#":smb,host="192.168.1.6",user="guest""#);
        assert_eq!(guest.share_path, "share");
        let named = Rclone::of(&connection(r"\\nas\photos\2024", r"NAS\ali")).unwrap();
        assert_eq!(named.share_path, "photos/2024");
        assert_eq!(
            named.remote("c2VjcmV0"),
            r#":smb,host="nas",user="ali",domain="NAS",pass="c2VjcmV0""#
        );
        let odd = Rclone::of(&connection(r"\\fe80--1.ipv6-literal.net\a", "say \"hi\"")).unwrap();
        assert_eq!(odd.remote(""), r#":smb,host="fe80::1",user="say ""hi""""#);
    }

    /// Windows' own SMB client on this PC's own admin share, as this account (no user name, no password), then the
    /// reasons it gives for a share that isn't there and an account that isn't, and the account tried by another
    /// name of the server once Windows is signed in to it as this one. Talks to Windows' network stack and signs out
    /// of what it signed in to: `cargo test -p glint-core --lib -- --ignored this_pcs_own_share`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn windows_signs_in_to_this_pcs_own_share_and_says_why_not_for_others() {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let share = format!(r"\\localhost\{}$", drive.trim_end_matches(':'));
        let connection = Connection {
            id: "smb-test".into(),
            kind: KIND.into(),
            endpoint: share.clone(),
            ..Default::default()
        };
        let said = test(&connection, "").expect("signed in as this account");
        assert!(said.starts_with("Connected: "), "{said}");
        let missing = connect(r"\\localhost\no-such-share-for-glint", "", "").expect_err("no such share");
        assert_eq!(
            missing.message,
            "localhost has no share named “no-such-share-for-glint”"
        );
        // Windows reaches localhost as this account now: another account is tried at its address instead, which
        // then says what it says of an account it doesn't know (not that someone else is signed in).
        let stranger = connect(&share, "glint-test-nobody", "not-a-password").expect_err("no such account");
        assert_eq!(stranger.trouble, Some(Trouble::SignIn), "{stranger:?}");
        assert!(
            !stranger.message.starts_with("localhost"),
            "tried by another name: {stranger:?}"
        );
        disconnect(&share);
        lock(&SIGNED_IN).clear();
    }

    /// A server that isn't there is told apart from one that doesn't answer, quickly.
    #[test]
    fn a_server_that_is_not_there_is_said_so_without_waiting_on_windows() {
        let started = Instant::now();
        assert!(
            !probe(&["192.0.2.1".parse().unwrap()], Duration::from_millis(300)),
            "TEST-NET-1 answers nothing"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            asleep(r"\\no-such-server-for-glint\share").is_none(),
            "a server never noted isn't called asleep"
        );
    }

    #[test]
    fn nothing_is_signed_in_to_for_a_path_no_connection_is_for() {
        assert!(ensure(r"\\no-such-server-for-glint\share\a").is_ok());
        assert!(ensure(r"C:\Windows").is_ok());
        assert!(ensure_all([r"D:\a", r"\\nas-without-connection\x\y"]).is_ok());
    }
}
