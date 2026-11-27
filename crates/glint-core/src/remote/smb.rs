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
//! Windows lets a logon session reach a server with one account at a time: a share File Explorer already reached as
//! someone else answers ERROR_SESSION_CREDENTIAL_CONFLICT, which is taken as signed in (reading then works if that
//! account may).

use super::{secrets, state, Connection};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// The kind connections of a Windows share have.
pub const KIND: &str = "smb";

/// The shares Glint signed in to this session, in lower case: signed in again only after a change.
static SIGNED_IN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

fn signed_in() -> std::sync::MutexGuard<'static, HashSet<String>> {
    SIGNED_IN.lock().unwrap_or_else(|e| e.into_inner())
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

/// The connection of the kind "smb" whose share `share` is (compared in any case).
fn connection_for(share: &str) -> Option<Connection> {
    state()
        .connections
        .iter()
        .find(|c| {
            c.kind == KIND
                && address(&c.endpoint)
                    .and_then(|a| share_of(&a))
                    .is_some_and(|s| s.eq_ignore_ascii_case(share))
        })
        .cloned()
}

/// Before Glint reads or changes `path`: when it's on a share a connection is for, signs in to that share with the
/// connection's user name and password, once a session. Nothing for any other path, nor for a share no connection
/// is for (Windows answers those as it would File Explorer).
pub fn ensure(path: &str) -> Result<(), String> {
    let Some(share) = share_of(path) else { return Ok(()) };
    if signed_in().contains(&share.to_lowercase()) {
        return Ok(());
    }
    let Some(connection) = connection_for(&share) else {
        return Ok(());
    };
    let password = secrets::read(&connection.id).unwrap_or_default();
    connect(&share, &connection.user, &password)?;
    signed_in().insert(share.to_lowercase());
    Ok(())
}

/// [`ensure`] for each share among `paths`, once each.
pub fn ensure_all<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for path in paths {
        if let Some(share) = share_of(path) {
            if seen.insert(share.to_lowercase()) {
                ensure(path)?;
            }
        }
    }
    Ok(())
}

/// Forgets that Glint signed in to the shares of these connections and signs out of them (Windows' connection, not
/// anything open there), so the next use signs in with what they say now. Only shares Glint signed in to.
pub(crate) fn forget(connections: &[Connection]) {
    for connection in connections.iter().filter(|c| c.kind == KIND) {
        let Some(share) = address(&connection.endpoint).and_then(|a| share_of(&a)) else {
            continue;
        };
        if signed_in().remove(&share.to_lowercase()) {
            disconnect(&share);
        }
    }
}

/// The connections of the kind "smb" that are gone from `now`, or changed in it (another share or user name).
pub(crate) fn changed(before: &[Connection], now: &[Connection]) -> Vec<Connection> {
    before
        .iter()
        .filter(|c| c.kind == KIND)
        .filter(|c| {
            !now.iter()
                .any(|n| n.id == c.id && n.kind == KIND && n.endpoint == c.endpoint && n.user == c.user)
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
    if signed_in().remove(&share.to_lowercase()) {
        disconnect(&share);
    }
    connect(&share, &connection.user, password)?;
    signed_in().insert(share.to_lowercase());
    let items = std::fs::read_dir(&path).map_err(|e| crate::fsx::describe(&e))?.count();
    Ok(match items {
        0 => "Connected: there's nothing there yet".into(),
        1 => "Connected: 1 item at its top".into(),
        n => format!("Connected: {n} items at its top"),
    })
}

/// The server of a share (`nas` for `\\nas\photos`), for messages.
fn server_of(share: &str) -> &str {
    share.trim_start_matches('\\').split('\\').next().unwrap_or(share)
}

/// The share's own name (`photos` for `\\nas\photos`), for messages.
fn name_of(share: &str) -> &str {
    share.trim_start_matches('\\').split('\\').nth(1).unwrap_or(share)
}

/// Signs in to `share` (`\\server\share`) with Windows' SMB client, deviceless and not remembered: with `user` and
/// `password`, or with neither as this account (or what Windows keeps for the server). Signed in already, as anyone,
/// is fine.
#[cfg(windows)]
pub fn connect(share: &str, user: &str, password: &str) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ACCOUNT_RESTRICTION, ERROR_BAD_NETPATH, ERROR_BAD_NET_NAME, ERROR_BAD_USERNAME,
        ERROR_INVALID_PASSWORD, ERROR_LOGON_FAILURE, ERROR_NO_NETWORK, ERROR_SESSION_CREDENTIAL_CONFLICT, NO_ERROR,
    };
    use windows::Win32::NetworkManagement::WNet::{
        WNetAddConnection2W, CONNECT_TEMPORARY, NETRESOURCEW, RESOURCETYPE_DISK,
    };
    let remote = HSTRING::from(share);
    let resource = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        lpRemoteName: PWSTR(remote.as_ptr() as *mut u16),
        ..Default::default()
    };
    let user = user.trim();
    let (user_w, password_w) = (HSTRING::from(user), HSTRING::from(password));
    // Neither: this account's sign-in (null asks Windows for its own). A user name with an empty password: an empty one.
    let user_arg = if user.is_empty() {
        PCWSTR::null()
    } else {
        PCWSTR(user_w.as_ptr())
    };
    let password_arg = if user.is_empty() && password.is_empty() {
        PCWSTR::null()
    } else {
        PCWSTR(password_w.as_ptr())
    };
    // Safety: the strings live through the call, and the structure is read only during it.
    let status = unsafe { WNetAddConnection2W(&resource, password_arg, user_arg, CONNECT_TEMPORARY) };
    let (server, name) = (server_of(share), name_of(share));
    match status {
        NO_ERROR | ERROR_SESSION_CREDENTIAL_CONFLICT => Ok(()),
        ERROR_LOGON_FAILURE | ERROR_INVALID_PASSWORD | ERROR_BAD_USERNAME => {
            Err(format!("{server} didn't take that user name and password"))
        }
        ERROR_ACCOUNT_RESTRICTION => Err(format!(
            "{server} won't take that account here (an empty password, or a guest)"
        )),
        ERROR_BAD_NETPATH => Err(format!(
            "Glint can't reach {server}: it may be off, or on another network"
        )),
        ERROR_BAD_NET_NAME => Err(format!("{server} has no share named “{name}”")),
        ERROR_ACCESS_DENIED => Err(format!("{server} doesn't let that account into “{name}”")),
        ERROR_NO_NETWORK => Err("This PC isn't on a network".into()),
        other => Err(format!(
            "Windows couldn't connect to {share}: {}",
            windows::core::Error::from_hresult(other.to_hresult()).message()
        )),
    }
}

#[cfg(not(windows))]
pub fn connect(_share: &str, _user: &str, _password: &str) -> Result<(), String> {
    Err("Windows shares open on Windows".into())
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
        ];
        let mut now = vec![share("a", r"\\nas\a", "me"), share("b", r"\\nas\b", "you")];
        now[0].name = "Renamed".into();
        let gone: Vec<String> = changed(&before, &now).into_iter().map(|c| c.id).collect();
        assert_eq!(
            gone,
            ["b", "c"],
            "a rename keeps the sign-in; another user or a removal doesn't"
        );
    }

    /// Windows' own SMB client on this PC's own admin share, as this account (no user name, no password), then the
    /// reasons it gives for a share that isn't there and an account that isn't. Talks to Windows' network stack and
    /// signs out of what it signed in to: `cargo test -p glint-core --lib -- --ignored this_pcs_own_share`.
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
        disconnect(&share);
        signed_in().remove(&share.to_lowercase());
        let missing = connect(r"\\localhost\no-such-share-for-glint", "", "").expect_err("no such share");
        assert_eq!(missing, "localhost has no share named “no-such-share-for-glint”");
        let stranger =
            connect(r"\\127.0.0.1\IPC$", "glint-test-nobody", "not-a-password").expect_err("no such account");
        assert!(stranger.contains("127.0.0.1"), "{stranger}");
    }

    #[test]
    fn nothing_is_signed_in_to_for_a_path_no_connection_is_for() {
        assert!(ensure(r"\\no-such-server-for-glint\share\a").is_ok());
        assert!(ensure(r"C:\Windows").is_ok());
        assert!(ensure_all([r"D:\a", r"\\nas-without-connection\x\y"]).is_ok());
    }
}
