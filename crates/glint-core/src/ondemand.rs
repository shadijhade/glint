//! Files On-Demand for remotes: a remote connection (an rclone cloud, S3, WebDAV, SFTP, a paired PC)
//! shown in File Explorer, and every app's Open dialog, as a folder of placeholders, with Windows' Cloud
//! Files API, as OneDrive does: names and sizes come when a folder is opened, a file's bytes when an app
//! reads it, downloaded through Glint's remote clients (remote/). Read-only in this version: deleting or
//! renaming there is refused. Each is at %USERPROFILE%\Glint remotes\<name>, while Glint runs (Windows
//! says the cloud provider isn't running otherwise).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

const ROOT_DIR: &str = "Glint remotes";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Mount {
    pub id: String,
    pub folder: String,
}

fn saved_path() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("ondemand.json"))
}

fn saved() -> Vec<Mount> {
    saved_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(mounts: &[Mount]) {
    if let Some(path) = saved_path() {
        let _ = std::fs::write(path, serde_json::to_vec_pretty(mounts).unwrap_or_default());
    }
}

/// The remotes shown in File Explorer now: their connection ids.
pub fn ondemand_list() -> Vec<Mount> {
    saved()
}

/// Shows a remote in File Explorer, on demand: its folder there.
pub async fn ondemand_mount(id: String, name: String) -> Result<String, String> {
    crate::fsx::blocking(move || {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).ok_or("No user folder")?;
        let clean = crate::archive::tree::clean_name(name.trim()).unwrap_or_else(|| id.clone());
        let folder = home.join(ROOT_DIR).join(clean);
        imp::mount(&id, &folder)?;
        let folder = folder.to_string_lossy().into_owned();
        let mut mounts = saved();
        mounts.retain(|m| m.id != id);
        mounts.push(Mount { id, folder: folder.clone() });
        save(&mounts);
        Ok(folder)
    })
    .await
}

/// Stops showing a remote in File Explorer: its placeholders go (what's on the remote stays).
pub async fn ondemand_unmount(id: String) -> Result<(), String> {
    crate::fsx::blocking(move || {
        let mut mounts = saved();
        if let Some(mount) = mounts.iter().find(|m| m.id == id) {
            imp::unmount(&mount.id, &PathBuf::from(&mount.folder))?;
        }
        mounts.retain(|m| m.id != id);
        save(&mounts);
        Ok(())
    })
    .await
}

/// A remote's placeholders in `folder`, and gone again (for the end-to-end test in devices.rs).
#[cfg(test)]
pub(crate) fn mount_folder(id: &str, folder: &std::path::Path) -> Result<(), String> {
    imp::mount(id, folder)
}

#[cfg(test)]
pub(crate) fn unmount_folder(id: &str, folder: &std::path::Path) -> Result<(), String> {
    imp::unmount(id, folder)
}

/// Connects the remotes shown in File Explorer again, as Glint starts.
pub fn start() {
    let mounts = saved();
    if mounts.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for mount in mounts {
            let _ = imp::mount(&mount.id, &PathBuf::from(&mount.folder));
        }
    });
}

/// Connections made: each one's id, folder and Windows' key for it.
struct Connected {
    id: String,
    key: i64,
    /// The callbacks' context: the id, kept alive while connected.
    context: *mut String,
}
unsafe impl Send for Connected {}

static CONNECTED: Mutex<Vec<Connected>> = Mutex::new(Vec::new());

#[cfg(not(windows))]
mod imp {
    use std::path::Path;
    pub fn mount(_: &str, _: &Path) -> Result<(), String> {
        Err("Files On-Demand is Windows'".into())
    }
    pub fn unmount(_: &str, _: &Path) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::{Connected, CONNECTED};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::NTSTATUS;
    use windows::Win32::Storage::CloudFilters::*;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO};

    /// Glint's provider id for the Cloud Files API.
    const PROVIDER_ID: GUID = GUID::from_u128(0x6f1d_8a0e_5b2c_4e4b_9f3a_1c7d_2e8b_4a60);
    const STATUS_SUCCESS: NTSTATUS = NTSTATUS(0);
    const STATUS_UNSUCCESSFUL: NTSTATUS = NTSTATUS(0xC000_0001_u32 as i32);
    const STATUS_ACCESS_DENIED: NTSTATUS = NTSTATUS(0xC000_0022_u32 as i32);
    /// What's handed over at a time while a file comes (a multiple of 4 KB, as Windows wants).
    const CHUNK: u64 = 1 << 20;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    fn e2s(e: windows::core::Error) -> String {
        e.message()
    }

    pub fn mount(id: &str, folder: &Path) -> Result<(), String> {
        if CONNECTED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|c| c.id == id) {
            return Ok(());
        }
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let path = wide(folder);
        let name: Vec<u16> = "Glint".encode_utf16().chain([0]).collect();
        let version: Vec<u16> = env!("CARGO_PKG_VERSION").encode_utf16().chain([0]).collect();
        let registration = CF_SYNC_REGISTRATION {
            StructSize: std::mem::size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: PCWSTR(name.as_ptr()),
            ProviderVersion: PCWSTR(version.as_ptr()),
            SyncRootIdentity: id.as_ptr().cast(),
            SyncRootIdentityLength: id.len() as u32,
            FileIdentity: std::ptr::null(),
            FileIdentityLength: 0,
            ProviderId: PROVIDER_ID,
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: std::mem::size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY { Primary: CF_HYDRATION_POLICY_FULL, Modifier: CF_HYDRATION_POLICY_MODIFIER(0) },
            Population: CF_POPULATION_POLICY { Primary: CF_POPULATION_POLICY_FULL, Modifier: CF_POPULATION_POLICY_MODIFIER(0) },
            InSync: CF_INSYNC_POLICY_TRACK_ALL,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        unsafe { CfRegisterSyncRoot(PCWSTR(path.as_ptr()), &registration, &policies, CF_REGISTER_FLAG_UPDATE) }.map_err(|e| format!("Windows didn't take it as a cloud folder: {}", e2s(e)))?;
        let callbacks = [
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS, Callback: Some(on_fetch_placeholders) },
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_DATA, Callback: Some(on_fetch_data) },
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NOTIFY_DELETE, Callback: Some(on_delete) },
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NOTIFY_RENAME, Callback: Some(on_rename) },
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NONE, Callback: None },
        ];
        let context = Box::into_raw(Box::new(id.to_string()));
        let key = match unsafe { CfConnectSyncRoot(PCWSTR(path.as_ptr()), callbacks.as_ptr(), Some(context as *const _), CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH) } {
            Ok(key) => key,
            Err(e) => {
                drop(unsafe { Box::from_raw(context) });
                return Err(format!("Couldn't connect it: {}", e2s(e)));
            }
        };
        CONNECTED.lock().unwrap_or_else(|e| e.into_inner()).push(Connected { id: id.to_string(), key: key.0, context });
        // The top's names, made now (deeper folders fill in as they're opened).
        let items = crate::host::async_runtime::block_on(crate::remote::list_raw(id, ""))?;
        let mut made = placeholders(&items, "");
        let _ = unsafe { CfCreatePlaceholders(PCWSTR(path.as_ptr()), &mut made.infos, CF_CREATE_FLAG_NONE, None) };
        Ok(())
    }

    pub fn unmount(id: &str, folder: &Path) -> Result<(), String> {
        let connected = {
            let mut list = CONNECTED.lock().unwrap_or_else(|e| e.into_inner());
            list.iter().position(|c| c.id == id).map(|i| list.remove(i))
        };
        if let Some(c) = connected {
            let _ = unsafe { CfDisconnectSyncRoot(CF_CONNECTION_KEY(c.key)) };
            drop(unsafe { Box::from_raw(c.context) });
        }
        let path = wide(folder);
        let _ = unsafe { CfUnregisterSyncRoot(PCWSTR(path.as_ptr())) };
        let _ = std::fs::remove_dir_all(folder);
        Ok(())
    }

    /// Placeholders for a folder's items, with what they point at kept alive beside them: each one's
    /// Windows name, and its own path on the remote as its identity.
    struct Made {
        infos: Vec<CF_PLACEHOLDER_CREATE_INFO>,
        _names: Vec<Vec<u16>>,
        _identities: Vec<Vec<u8>>,
    }

    fn placeholders(items: &[crate::remote::Item], folder_raw: &str) -> Made {
        let mut names = Vec::new();
        let mut identities = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for item in items {
            let Some(mut shown) = crate::archive::tree::clean_name(&item.name) else { continue };
            // Two names Windows can't tell apart: the second gets a number.
            let mut n = 2;
            while !seen.insert(shown.to_lowercase()) {
                shown = format!("{} ({n})", crate::archive::tree::clean_name(&item.name).unwrap_or_default());
                n += 1;
            }
            names.push(shown.encode_utf16().chain([0]).collect::<Vec<u16>>());
            let raw = if folder_raw.is_empty() { item.name.clone() } else { format!("{folder_raw}/{}", item.name) };
            identities.push(raw.into_bytes());
        }
        let infos = items
            .iter()
            .zip(names.iter().zip(&identities))
            .map(|(item, (name, identity))| {
                let time = if item.modified > 0 { item.modified * 10_000 + 116_444_736_000_000_000 } else { 0 };
                CF_PLACEHOLDER_CREATE_INFO {
                    RelativeFileName: PCWSTR(name.as_ptr()),
                    FsMetadata: CF_FS_METADATA {
                        BasicInfo: FILE_BASIC_INFO { CreationTime: time, LastAccessTime: time, LastWriteTime: time, ChangeTime: time, FileAttributes: if item.dir { FILE_ATTRIBUTE_DIRECTORY.0 } else { FILE_ATTRIBUTE_NORMAL.0 } },
                        FileSize: if item.dir { 0 } else { item.size as i64 },
                    },
                    FileIdentity: identity.as_ptr().cast(),
                    FileIdentityLength: identity.len() as u32,
                    Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
                    Result: Default::default(),
                    CreateUsn: 0,
                }
            })
            .collect();
        Made { infos, _names: names, _identities: identities }
    }

    unsafe fn identity(info: &CF_CALLBACK_INFO) -> String {
        if info.FileIdentity.is_null() || info.FileIdentityLength == 0 {
            return String::new();
        }
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(info.FileIdentity.cast::<u8>(), info.FileIdentityLength as usize) }).into_owned()
    }

    unsafe fn connection_id(info: &CF_CALLBACK_INFO) -> String {
        if info.CallbackContext.is_null() {
            return String::new();
        }
        unsafe { (*(info.CallbackContext as *const String)).clone() }
    }

    fn operation(info: &CF_CALLBACK_INFO, kind: CF_OPERATION_TYPE) -> CF_OPERATION_INFO {
        CF_OPERATION_INFO { StructSize: std::mem::size_of::<CF_OPERATION_INFO>() as u32, Type: kind, ConnectionKey: info.ConnectionKey, TransferKey: info.TransferKey, CorrelationVector: std::ptr::null(), SyncStatus: std::ptr::null(), RequestKey: info.RequestKey }
    }

    fn param_size<T>() -> u32 {
        (std::mem::offset_of!(CF_OPERATION_PARAMETERS, Anonymous) + std::mem::size_of::<T>()) as u32
    }

    /// A folder opened for the first time: its items from the remote.
    unsafe extern "system" fn on_fetch_placeholders(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
        let info = unsafe { &*info };
        let (id, raw) = unsafe { (connection_id(info), identity(info)) };
        let listed = crate::host::async_runtime::block_on(crate::remote::list_raw(&id, &raw));
        let mut made = placeholders(listed.as_deref().unwrap_or_default(), &raw);
        let op = operation(info, CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_4>(),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                TransferPlaceholders: CF_OPERATION_PARAMETERS_0_4 {
                    Flags: CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_DISABLE_ON_DEMAND_POPULATION,
                    CompletionStatus: if listed.is_ok() { STATUS_SUCCESS } else { STATUS_UNSUCCESSFUL },
                    PlaceholderTotalCount: made.infos.len() as i64,
                    PlaceholderArray: if made.infos.is_empty() { std::ptr::null_mut() } else { made.infos.as_mut_ptr() },
                    PlaceholderCount: made.infos.len() as u32,
                    EntriesProcessed: 0,
                },
            },
        };
        let _ = unsafe { CfExecute(&op, &mut params) };
    }

    fn transfer(info: &CF_CALLBACK_INFO, status: NTSTATUS, buffer: &[u8], offset: u64, length: u64) -> bool {
        let op = operation(info, CF_OPERATION_TYPE_TRANSFER_DATA);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_0>(),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                TransferData: CF_OPERATION_PARAMETERS_0_0 { Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE, CompletionStatus: status, Buffer: if buffer.is_empty() { std::ptr::null() } else { buffer.as_ptr().cast() }, Offset: offset as i64, Length: length as i64 },
            },
        };
        unsafe { CfExecute(&op, &mut params) }.is_ok()
    }

    /// An app reads a file: it's downloaded, and handed to Windows as it comes, a chunk at a time.
    unsafe extern "system" fn on_fetch_data(info: *const CF_CALLBACK_INFO, params: *const CF_CALLBACK_PARAMETERS) {
        use std::io::{Read, Seek};
        let info = unsafe { &*info };
        let fetch = unsafe { (*params).Anonymous.FetchData };
        let (id, raw) = unsafe { (connection_id(info), identity(info)) };
        let size = info.FileSize.max(0) as u64;
        let fail = || {
            transfer(info, STATUS_UNSUCCESSFUL, &[], fetch.RequiredFileOffset as u64, fetch.RequiredLength as u64);
        };
        let Ok(dir) = crate::archive::extract::new_folder("ondemand").map_err(|e| e.message()) else { return fail() };
        let temp = dir.join("file");
        // The download runs on Glint's runtime; this thread hands over what's come.
        let came = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let (now, finished, dest, id2, raw2) = (came.clone(), done.clone(), temp.clone(), id.clone(), raw.clone());
        let download = crate::host::async_runtime::spawn(async move {
            let result = crate::remote::download_raw(&id2, &raw2, &dest, size, &mut |n| {
                now.fetch_add(n, Ordering::SeqCst);
                true
            })
            .await;
            finished.store(true, Ordering::SeqCst);
            result
        });
        let mut sent = 0u64;
        let mut buffer = vec![0u8; CHUNK as usize];
        let mut file: Option<std::fs::File> = None;
        loop {
            let finished = done.load(Ordering::SeqCst);
            let have = came.load(Ordering::SeqCst).min(size);
            // Whole chunks as they come, the rest at the end.
            while have - sent >= CHUNK || (finished && have > sent && have == size) {
                let length = (have - sent).min(CHUNK);
                if file.is_none() {
                    file = std::fs::File::open(&temp).ok();
                }
                let Some(f) = file.as_mut() else { break };
                if f.seek(std::io::SeekFrom::Start(sent)).is_err() || f.read_exact(&mut buffer[..length as usize]).is_err() {
                    break;
                }
                if !transfer(info, STATUS_SUCCESS, &buffer[..length as usize], sent, length) {
                    // Windows stopped wanting it (the app closed the file).
                    let _ = std::fs::remove_dir_all(&dir);
                    return;
                }
                sent += length;
            }
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let ok = crate::host::async_runtime::block_on(download).ok().and_then(Result::ok).is_some();
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
        if !ok || sent < size {
            fail();
        }
    }

    /// Read-only: what's deleted or renamed there isn't, so it's refused.
    unsafe extern "system" fn on_delete(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
        let info = unsafe { &*info };
        let op = operation(info, CF_OPERATION_TYPE_ACK_DELETE);
        let mut params = CF_OPERATION_PARAMETERS { ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_7>(), Anonymous: CF_OPERATION_PARAMETERS_0 { AckDelete: CF_OPERATION_PARAMETERS_0_7 { Flags: CF_OPERATION_ACK_DELETE_FLAG_NONE, CompletionStatus: STATUS_ACCESS_DENIED } } };
        let _ = unsafe { CfExecute(&op, &mut params) };
    }

    unsafe extern "system" fn on_rename(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
        let info = unsafe { &*info };
        let op = operation(info, CF_OPERATION_TYPE_ACK_RENAME);
        let mut params = CF_OPERATION_PARAMETERS { ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_6>(), Anonymous: CF_OPERATION_PARAMETERS_0 { AckRename: CF_OPERATION_PARAMETERS_0_6 { Flags: CF_OPERATION_ACK_RENAME_FLAG_NONE, CompletionStatus: STATUS_ACCESS_DENIED } } };
        let _ = unsafe { CfExecute(&op, &mut params) };
    }
}
