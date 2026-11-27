//! Generalized Cloud Files (`CfAPI`) Live Portals.
//!
//! Mounts virtual sources — Archives (`.zip`/`.7z`/`.tar`), Saved Searches, Git commits, and restic backup
//! snapshots — as live folders under `%USERPROFILE%\Glint portals\<Name>`, visible to File Explorer,
//! PowerShell, VS Code, Notepad, and any Windows application, with on-demand zero-byte hydration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Source backed by a Live Portal folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PortalSource {
    /// Virtual read-only mount of an archive (`C:\path\pack.zip`).
    Archive { archive_path: String },
    /// Live mount of matching files under `root`.
    SavedSearch { query: String, root: String },
    /// Time-travel live folder of a Git repository at revision `rev` (commit OID / ref).
    GitCommit {
        repo_root: String,
        rev: String,
        subfolder: String,
    },
    /// Time-travel live folder of a restic backup snapshot.
    ResticSnapshot {
        plan_id: String,
        snapshot_id: String,
        subfolder: String,
    },
}

/// One mounted portal persisted in `%LOCALAPPDATA%\Glint\portals.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Portal {
    pub id: String,
    pub name: String,
    pub mount_path: String,
    pub source: PortalSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortalEntry {
    pub rel_path: String,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
    /// Optional real path on disk for `SavedSearch` hydration.
    pub real_path: Option<String>,
}

static PORTALS: LazyLock<Mutex<HashMap<String, Portal>>> = LazyLock::new(|| Mutex::new(load_map()));

/// Who wants to hear that a portal was mounted or unmounted: each returns whether it still wants to.
type Listener = Box<dyn Fn() -> bool + Send + Sync>;

static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// Calls `listener` (from whatever thread did it) after a portal is mounted or unmounted, until it returns
/// `false`. The sidebar's Portals section keeps itself up to date with this instead of looking on a timer.
pub fn on_change(listener: impl Fn() -> bool + Send + Sync + 'static) {
    LISTENERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Box::new(listener));
}

/// Tells the listeners the list of portals changed. Never called with the portals' lock held.
fn changed() {
    LISTENERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|listener| listener());
}

fn portals_file() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("portals.json"))
}

/// Base mount directory (`%USERPROFILE%\Glint portals`).
pub fn portals_root() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Glint portals"))
}

/// Sanitizes a human portal label into a Windows-safe single folder name.
pub fn sanitize_name(name: &str) -> String {
    let bad = |c: char| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
    let cleaned: String = name
        .chars()
        .map(|c| if bad(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string();
    if cleaned.is_empty() {
        "Portal".to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

fn unique_mount_path(root: &Path, name: &str, existing: &HashMap<String, Portal>) -> PathBuf {
    let base = sanitize_name(name);
    let candidate = root.join(&base);
    if !existing
        .values()
        .any(|p| p.mount_path.eq_ignore_ascii_case(&candidate.to_string_lossy()))
        && !candidate.exists()
    {
        return candidate;
    }
    for i in 2..=999 {
        let next = root.join(format!("{base} ({i})"));
        if !existing
            .values()
            .any(|p| p.mount_path.eq_ignore_ascii_case(&next.to_string_lossy()))
            && !next.exists()
        {
            return next;
        }
    }
    root.join(format!("{base}-{}", std::process::id()))
}

fn load_map() -> HashMap<String, Portal> {
    let Some(path) = portals_file() else {
        return HashMap::new();
    };
    let Ok(bytes) = fs::read(path) else {
        return HashMap::new();
    };
    serde_json::from_slice::<Vec<Portal>>(&bytes)
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect()
}

fn save_map(map: &HashMap<String, Portal>) {
    let Some(path) = portals_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut items: Vec<&Portal> = map.values().collect();
    items.sort_by_key(|a| a.name.to_lowercase());
    if let Ok(json) = serde_json::to_vec_pretty(&items) {
        let _ = fs::write(path, json);
    }
}

fn next_id(source: &PortalSource) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match source {
        PortalSource::Archive { archive_path } => {
            0u8.hash(&mut hasher);
            archive_path.to_lowercase().hash(&mut hasher);
        }
        PortalSource::SavedSearch { query, root } => {
            1u8.hash(&mut hasher);
            query.hash(&mut hasher);
            root.to_lowercase().hash(&mut hasher);
        }
        PortalSource::GitCommit {
            repo_root,
            rev,
            subfolder,
        } => {
            2u8.hash(&mut hasher);
            repo_root.to_lowercase().hash(&mut hasher);
            rev.hash(&mut hasher);
            subfolder.to_lowercase().hash(&mut hasher);
        }
        PortalSource::ResticSnapshot {
            plan_id,
            snapshot_id,
            subfolder,
        } => {
            3u8.hash(&mut hasher);
            plan_id.hash(&mut hasher);
            snapshot_id.hash(&mut hasher);
            subfolder.to_lowercase().hash(&mut hasher);
        }
    }
    format!("portal-{:016x}", hasher.finish())
}

/// Lists all currently mounted Live Portals.
pub fn portals_list() -> Vec<Portal> {
    let mut items: Vec<Portal> = PORTALS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    items.sort_by_key(|a| a.name.to_lowercase());
    items
}

/// Starts serving previously persisted Live Portals on Windows startup.
pub fn start() {
    #[cfg(windows)]
    {
        let existing = portals_list();
        if existing.is_empty() {
            return;
        }
        for portal in existing {
            std::thread::Builder::new()
                .name(format!("portal-{}", portal.id))
                .spawn(move || {
                    let _ = win::mount_and_connect(&portal);
                })
                .ok();
        }
    }
}

/// Mounts a source as a live Windows CfAPI folder under `%USERPROFILE%\Glint portals\<name>`.
pub async fn portal_mount(name: String, source: PortalSource) -> Result<Portal, String> {
    crate::fsx::blocking(move || mount_sync(&name, source)).await
}

/// Refreshes the placeholders inside an already mounted portal.
pub async fn portal_refresh(id: String) -> Result<(), String> {
    crate::fsx::blocking(move || {
        let portal = PORTALS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
            .ok_or_else(|| "Portal no longer exists".to_string())?;
        #[cfg(windows)]
        {
            win::populate(&portal)
        }
        #[cfg(not(windows))]
        {
            let _ = portal;
            Ok(())
        }
    })
    .await
}

/// Unmounts and removes a portal cleanly.
pub async fn portal_unmount(id: String) -> Result<(), String> {
    crate::fsx::blocking(move || unmount_sync(&id)).await
}

fn mount_sync(name: &str, source: PortalSource) -> Result<Portal, String> {
    let id = next_id(&source);
    {
        let map = PORTALS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = map.get(&id) {
            return Ok(existing.clone());
        }
    }
    let root = portals_root().ok_or_else(|| "USERPROFILE is not set".to_string())?;
    let _ = fs::create_dir_all(&root);
    let mount_path = {
        let map = PORTALS.lock().unwrap_or_else(|e| e.into_inner());
        unique_mount_path(&root, name, &map)
    };
    let portal = Portal {
        id: id.clone(),
        name: name.to_string(),
        mount_path: mount_path.to_string_lossy().into_owned(),
        source,
    };
    #[cfg(windows)]
    win::mount_and_connect(&portal)?;
    {
        let mut map = PORTALS.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(id, portal.clone());
        save_map(&map);
    }
    changed();
    Ok(portal)
}

fn unmount_sync(id: &str) -> Result<(), String> {
    let removed = {
        let mut map = PORTALS.lock().unwrap_or_else(|e| e.into_inner());
        let item = map.remove(id);
        if item.is_some() {
            save_map(&map);
        }
        item
    };
    let Some(portal) = removed else {
        return Ok(());
    };
    #[cfg(windows)]
    win::unmount(&portal);
    changed();
    Ok(())
}

/// Whether `mount_path` is a folder Glint made for a portal: a direct child of the portals folder. Unmounting
/// deletes the folder, and the path comes from `portals.json`, which anything can edit: a path anywhere else
/// is left alone.
fn is_portal_folder(mount_path: &str, root: &Path) -> bool {
    let path = Path::new(mount_path);
    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    path.parent().is_some_and(|parent| same(parent, root))
        && path.file_name().is_some_and(|name| name != "." && name != "..")
}

/// `Glint.exe portals remove`, which the setup's and Scoop's uninstallers run (not their updates): every
/// folder Glint shows through the Cloud Files API leaves Windows, the Live Portals and the remotes shown in
/// File Explorer (ondemand.rs), as no program would be left to fill their placeholders in. What they show
/// stays where it is.
pub fn cli() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("portals") {
        return None;
    }
    if args.next().as_deref() != Some("remove") || args.next().is_some() {
        return Some(2);
    }
    crate::tools::init_outside_app();
    for portal in portals_list() {
        let _ = unmount_sync(&portal.id);
    }
    if let Some(root) = portals_root() {
        // Only once nothing else is in it.
        let _ = fs::remove_dir(root);
    }
    crate::ondemand::remove_all();
    Some(0)
}

/// Lists virtual entries for `source` without hydrating file bytes.
pub(crate) fn portal_list_entries(source: &PortalSource) -> Result<Vec<PortalEntry>, String> {
    match source {
        PortalSource::Archive { archive_path } => {
            let raw = crate::archive::portal_list(archive_path)?;
            Ok(raw
                .into_iter()
                .map(|(rel_path, dir, size, modified)| PortalEntry {
                    rel_path,
                    dir,
                    size,
                    modified,
                    real_path: None,
                })
                .collect())
        }
        PortalSource::GitCommit {
            repo_root,
            rev,
            subfolder,
        } => {
            let mut out = Vec::new();
            let mut stack = vec![(subfolder.clone(), String::new())];
            while let Some((folder, prefix)) = stack.pop() {
                let entries = crate::git::list_tree_at(repo_root, rev, &folder)?;
                for entry in entries {
                    let rel = if prefix.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{prefix}\\{}", entry.name)
                    };
                    let next_folder = if folder.trim_end_matches(['\\', '/']).is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}\\{}", folder.trim_end_matches(['\\', '/']), entry.name)
                    };
                    out.push(PortalEntry {
                        rel_path: rel.clone(),
                        dir: entry.dir,
                        size: entry.size,
                        modified: entry.modified,
                        real_path: None,
                    });
                    if entry.dir && out.len() < 10_000 {
                        stack.push((next_folder, rel));
                    }
                }
            }
            Ok(out)
        }
        PortalSource::ResticSnapshot {
            plan_id,
            snapshot_id,
            subfolder,
        } => {
            let mut out = Vec::new();
            let mut stack = vec![(subfolder.clone(), String::new())];
            while let Some((folder, prefix)) = stack.pop() {
                let entries = crate::backup::list_folder_in_snapshot(plan_id, snapshot_id, &folder)?;
                for entry in entries {
                    let rel = if prefix.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{prefix}\\{}", entry.name)
                    };
                    let next_folder = format!("{}\\{}", folder.trim_end_matches(['\\', '/']), entry.name);
                    out.push(PortalEntry {
                        rel_path: rel.clone(),
                        dir: entry.dir,
                        size: entry.size,
                        modified: entry.modified,
                        real_path: None,
                    });
                    if entry.dir && out.len() < 10_000 {
                        stack.push((next_folder, rel));
                    }
                }
            }
            Ok(out)
        }
        PortalSource::SavedSearch { query, root } => {
            let mut out = Vec::new();
            let req = shadui_index::proto::SearchRequest {
                query: query.clone(),
                roots: if root.is_empty() {
                    Vec::new()
                } else {
                    vec![root.clone()]
                },
                hidden: false,
                exclude: Vec::new(),
                max_results: Some(2_000),
                fuzzy: false,
                rank: true,
                marks: false,
                count_only: false,
            };
            let mut hits = Vec::new();
            let search_ok = shadui_index::client::search(&req, |ev| {
                if let shadui_index::client::Event::Hits(batch) = ev {
                    hits.extend(batch);
                }
                hits.len() < 2_000
            })
            .is_ok();
            if search_ok && !hits.is_empty() {
                for hit in hits.into_iter().filter(|h| !h.dir).take(2_000) {
                    let name = Path::new(&hit.path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&hit.path)
                        .to_string();
                    out.push(PortalEntry {
                        rel_path: make_unique_flat_name(&out, &name),
                        dir: false,
                        size: hit.size,
                        modified: hit.modified,
                        real_path: Some(hit.path),
                    });
                }
                return Ok(out);
            }
            // Fallback recursive scan when instant index service isn't running.
            let needle = query.trim().to_ascii_lowercase();
            let mut queue = std::collections::VecDeque::new();
            if !root.is_empty() {
                queue.push_back(PathBuf::from(root));
            }
            while let Some(dir) = queue.pop_front() {
                if out.len() >= 2_000 {
                    break;
                }
                let Ok(rd) = fs::read_dir(&dir) else {
                    continue;
                };
                for item in rd.flatten() {
                    let p = item.path();
                    let Ok(meta) = item.metadata() else {
                        continue;
                    };
                    if meta.is_dir() {
                        queue.push_back(p);
                        continue;
                    }
                    let name = item.file_name().to_string_lossy().into_owned();
                    if needle.is_empty() || name.to_ascii_lowercase().contains(&needle) {
                        let modified = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map_or(0, |d| d.as_millis() as i64);
                        out.push(PortalEntry {
                            rel_path: make_unique_flat_name(&out, &name),
                            dir: false,
                            size: meta.len(),
                            modified,
                            real_path: Some(p.to_string_lossy().into_owned()),
                        });
                    }
                }
            }
            Ok(out)
        }
    }
}

fn make_unique_flat_name(existing: &[PortalEntry], name: &str) -> String {
    if !existing.iter().any(|e| e.rel_path.eq_ignore_ascii_case(name)) {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    for i in 2..=9999 {
        let candidate = if ext.is_empty() {
            format!("{stem} ({i})")
        } else {
            format!("{stem} ({i}).{ext}")
        };
        if !existing.iter().any(|e| e.rel_path.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
    }
    format!("{stem}-{}", existing.len())
}

/// Hydrates bytes for `identity` (relative path or resolved path) into a readable [`File`].
pub(crate) fn portal_open_identity(source: &PortalSource, identity: &str) -> Result<File, String> {
    match source {
        PortalSource::Archive { archive_path } => {
            let extracted = crate::archive::portal_extract(archive_path, identity)?;
            File::open(&extracted).map_err(|e| format!("Couldn't open extracted file: {e}"))
        }
        PortalSource::SavedSearch { .. } => {
            // Identity stores real full path for saved searches.
            File::open(identity).map_err(|e| format!("Couldn't read {identity}: {e}"))
        }
        PortalSource::GitCommit {
            repo_root,
            rev,
            subfolder,
        } => {
            let rel_in_repo = if let Some(prefix) = crate::git::rel_path_in(repo_root, subfolder) {
                if prefix.is_empty() {
                    identity.replace('\\', "/")
                } else {
                    format!("{prefix}/{}", identity.replace('\\', "/"))
                }
            } else {
                let sub = subfolder.trim_matches(['\\', '/']).replace('\\', "/");
                if sub.is_empty() {
                    identity.replace('\\', "/")
                } else {
                    format!("{sub}/{}", identity.replace('\\', "/"))
                }
            };
            let temp = crate::archive::extract::new_folder("portals")
                .map_err(|e| e.message())?
                .join(
                    Path::new(identity)
                        .file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new("blob")),
                );
            crate::git::extract_blob_at(repo_root, rev, &rel_in_repo, &temp)?;
            File::open(&temp).map_err(|e| format!("Couldn't open hydrated Git blob: {e}"))
        }
        PortalSource::ResticSnapshot {
            plan_id,
            snapshot_id,
            subfolder,
        } => {
            let full = format!("{}\\{}", subfolder.trim_end_matches(['\\', '/']), identity);
            let extracted = crate::host::async_runtime::block_on(crate::backup::backup_open(
                plan_id.clone(),
                snapshot_id.clone(),
                full,
                false,
            ))?;
            File::open(&extracted).map_err(|e| format!("Couldn't open backup file: {e}"))
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn read_identity_bytes(source: &PortalSource, identity: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = portal_open_identity(source, identity)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

#[cfg(windows)]
mod win {
    use super::{portal_list_entries, portal_open_identity, Portal, PortalEntry, PortalSource};
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::{LazyLock, Mutex};
    use windows::core::{Error, PCWSTR};
    use windows::Win32::Foundation::{STATUS_ACCESS_DENIED, STATUS_UNSUCCESSFUL, WIN32_ERROR};
    use windows::Win32::Storage::CloudFilters::*;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_DIRECTORY};

    const DISPLAY_NAME: &str = "Glint Portal";
    const CHUNK: usize = 256 << 10;

    struct Connected {
        key: i64,
        mount: String,
    }

    static ACTIVE: LazyLock<Mutex<HashMap<String, Connected>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    static SOURCES: LazyLock<Mutex<HashMap<String, PortalSource>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn nt_epoch(ms: i64) -> i64 {
        if ms <= 0 {
            0
        } else {
            ms.saturating_mul(10_000).saturating_add(116_444_736_000_000_000)
        }
    }

    fn metadata_of(entry: &PortalEntry) -> CF_FS_METADATA {
        let time = nt_epoch(entry.modified);
        CF_FS_METADATA {
            BasicInfo: windows::Win32::Storage::FileSystem::FILE_BASIC_INFO {
                CreationTime: time,
                LastAccessTime: time,
                LastWriteTime: time,
                ChangeTime: time,
                FileAttributes: if entry.dir {
                    FILE_ATTRIBUTE_DIRECTORY.0
                } else {
                    FILE_ATTRIBUTE_ARCHIVE.0
                },
            },
            FileSize: if entry.dir { 0 } else { entry.size as i64 },
        }
    }

    fn operation(info: &CF_CALLBACK_INFO, kind: CF_OPERATION_TYPE) -> CF_OPERATION_INFO {
        CF_OPERATION_INFO {
            StructSize: size_of::<CF_OPERATION_INFO>() as u32,
            Type: kind,
            ConnectionKey: info.ConnectionKey,
            TransferKey: info.TransferKey,
            CorrelationVector: std::ptr::null(),
            SyncStatus: std::ptr::null(),
            RequestKey: info.RequestKey,
        }
    }

    const fn param_size<T>() -> u32 {
        (std::mem::offset_of!(CF_OPERATION_PARAMETERS, Anonymous) + size_of::<T>()) as u32
    }

    unsafe fn callback_identity(info: &CF_CALLBACK_INFO) -> String {
        if info.FileIdentity.is_null() || info.FileIdentityLength == 0 {
            return String::new();
        }
        // SAFETY: CfAPI passes valid `FileIdentity` buffer of length `FileIdentityLength`.
        let bytes = std::slice::from_raw_parts(info.FileIdentity as *const u8, info.FileIdentityLength as usize);
        String::from_utf8_lossy(bytes).into_owned()
    }

    unsafe fn callback_portal_id(info: &CF_CALLBACK_INFO) -> String {
        if info.CallbackContext.is_null() {
            return String::new();
        }
        // SAFETY: CallbackContext points to leaked `String` owned for connection lifetime or static key.
        let s = &*(info.CallbackContext as *const String);
        s.clone()
    }

    fn transfer_chunk(
        info: &CF_CALLBACK_INFO,
        status: windows::Win32::Foundation::NTSTATUS,
        buf: &[u8],
        offset: u64,
        len: u64,
    ) -> bool {
        let op = operation(info, CF_OPERATION_TYPE_TRANSFER_DATA);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_0>(),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                TransferData: CF_OPERATION_PARAMETERS_0_0 {
                    Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                    CompletionStatus: status,
                    Buffer: if buf.is_empty() {
                        std::ptr::null()
                    } else {
                        buf.as_ptr().cast()
                    },
                    Offset: offset as i64,
                    Length: len as i64,
                },
            },
        };
        // SAFETY: `op` and `params` reference valid stack slices for duration of `CfExecute`.
        unsafe { CfExecute(&op, &mut params) }.is_ok()
    }

    unsafe extern "system" fn on_fetch_placeholders(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
        let info = &*info;
        let op = operation(info, CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_4>(),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                TransferPlaceholders: CF_OPERATION_PARAMETERS_0_4 {
                    Flags: CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_DISABLE_ON_DEMAND_POPULATION,
                    CompletionStatus: windows::Win32::Foundation::NTSTATUS(0),
                    PlaceholderTotalCount: 0,
                    PlaceholderArray: std::ptr::null_mut(),
                    PlaceholderCount: 0,
                    EntriesProcessed: 0,
                },
            },
        };
        let _ = CfExecute(&op, &mut params);
    }

    unsafe extern "system" fn on_fetch_data(info: *const CF_CALLBACK_INFO, params: *const CF_CALLBACK_PARAMETERS) {
        let info = &*info;
        let fetch = (*params).Anonymous.FetchData;
        let offset = fetch.RequiredFileOffset as u64;
        let want = fetch.RequiredLength as u64;
        let portal_id = callback_portal_id(info);
        let identity = callback_identity(info);

        let fail = || {
            transfer_chunk(info, STATUS_UNSUCCESSFUL, &[], offset, want);
        };
        let source = {
            SOURCES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&portal_id)
                .cloned()
        };
        let Some(source) = source else {
            return fail();
        };
        let Ok(mut file) = portal_open_identity(&source, &identity) else {
            return fail();
        };
        use std::io::Seek;
        if file.seek(std::io::SeekFrom::Start(offset)).is_err() {
            return fail();
        }
        let mut buf = vec![0u8; CHUNK.min(want.max(1) as usize)];
        let mut sent = 0u64;
        while sent < want {
            let need = ((want - sent) as usize).min(buf.len());
            let Ok(read) = file.read(&mut buf[..need]) else {
                return fail();
            };
            if read == 0 {
                break;
            }
            if !transfer_chunk(
                info,
                windows::Win32::Foundation::NTSTATUS(0),
                &buf[..read],
                offset + sent,
                read as u64,
            ) {
                return;
            }
            sent += read as u64;
        }
        if sent < want {
            transfer_chunk(
                info,
                windows::Win32::Foundation::NTSTATUS(0),
                &[],
                offset + sent,
                want - sent,
            );
        }
    }

    unsafe extern "system" fn on_delete(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
        let info = &*info;
        let op = operation(info, CF_OPERATION_TYPE_ACK_DELETE);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_7>(),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckDelete: CF_OPERATION_PARAMETERS_0_7 {
                    Flags: CF_OPERATION_ACK_DELETE_FLAG_NONE,
                    CompletionStatus: STATUS_ACCESS_DENIED,
                },
            },
        };
        let _ = CfExecute(&op, &mut params);
    }

    unsafe extern "system" fn on_rename(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
        let info = &*info;
        let op = operation(info, CF_OPERATION_TYPE_ACK_RENAME);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: param_size::<CF_OPERATION_PARAMETERS_0_6>(),
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckRename: CF_OPERATION_PARAMETERS_0_6 {
                    Flags: CF_OPERATION_ACK_RENAME_FLAG_NONE,
                    CompletionStatus: STATUS_ACCESS_DENIED,
                },
            },
        };
        let _ = CfExecute(&op, &mut params);
    }

    pub(super) fn mount_and_connect(portal: &Portal) -> Result<(), String> {
        let mount = PathBuf::from(&portal.mount_path);
        fs::create_dir_all(&mount).map_err(|e| format!("Couldn't create portal folder: {e}"))?;
        let sync_path = wide(&portal.mount_path);
        let name_w = wide(DISPLAY_NAME);
        let ver_w = wide("1.0");
        let root_id = format!("glint-portal-{}", portal.id);

        let reg = CF_SYNC_REGISTRATION {
            StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: PCWSTR(name_w.as_ptr()),
            ProviderVersion: PCWSTR(ver_w.as_ptr()),
            SyncRootIdentity: root_id.as_ptr().cast(),
            SyncRootIdentityLength: root_id.len() as u32,
            ..Default::default()
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY {
                Primary: CF_HYDRATION_POLICY_FULL,
                Modifier: CF_HYDRATION_POLICY_MODIFIER(0),
            },
            Population: CF_POPULATION_POLICY {
                Primary: CF_POPULATION_POLICY_FULL,
                Modifier: CF_POPULATION_POLICY_MODIFIER(0),
            },
            InSync: CF_INSYNC_POLICY_TRACK_ALL,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        // SAFETY: Valid pointers for duration of `CfRegisterSyncRoot`.
        unsafe { CfRegisterSyncRoot(PCWSTR(sync_path.as_ptr()), &reg, &policies, CF_REGISTER_FLAG_UPDATE) }
            .map_err(|e: Error| format!("Windows couldn't register cloud portal: {}", e.message()))?;

        SOURCES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(portal.id.clone(), portal.source.clone());

        let ctx = Box::into_raw(Box::new(portal.id.clone()));
        let callbacks = [
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
                Callback: Some(on_fetch_placeholders),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_DATA,
                Callback: Some(on_fetch_data),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NOTIFY_DELETE,
                Callback: Some(on_delete),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NOTIFY_RENAME,
                Callback: Some(on_rename),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NONE,
                Callback: None,
            },
        ];
        // SAFETY: `callbacks` is terminated by `CF_CALLBACK_TYPE_NONE` and `ctx` lives for sync connection.
        let key = unsafe {
            CfConnectSyncRoot(
                PCWSTR(sync_path.as_ptr()),
                callbacks.as_ptr(),
                Some(ctx as *const _),
                CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH,
            )
        }
        .map_err(|e: Error| {
            // SAFETY: Clean up `ctx` when connection fails.
            unsafe { drop(Box::from_raw(ctx)) };
            format!("Couldn't connect cloud portal: {}", e.message())
        })?;

        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(
            portal.id.clone(),
            Connected {
                key: key.0,
                mount: portal.mount_path.clone(),
            },
        );
        populate(portal)?;
        let _ = WIN32_ERROR(0);
        Ok(())
    }

    pub(super) fn populate(portal: &Portal) -> Result<(), String> {
        let entries = portal_list_entries(&portal.source)?;
        let root = Path::new(&portal.mount_path);
        // Group entries by parent relative folder so directory placeholders exist before child files.
        let mut by_parent: BTreeMap<(usize, String), Vec<PortalEntry>> = BTreeMap::new();
        for entry in entries {
            let p = Path::new(&entry.rel_path);
            let parent = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
            let depth = if parent.is_empty() {
                0
            } else {
                parent.split(['\\', '/']).filter(|s| !s.is_empty()).count()
            };
            by_parent.entry((depth, parent)).or_default().push(entry);
        }
        for ((_, rel_parent), items) in by_parent {
            let target_dir = if rel_parent.is_empty() {
                root.to_path_buf()
            } else {
                root.join(&rel_parent)
            };
            let _ = fs::create_dir_all(&target_dir);
            let dir_w = wide(&target_dir.to_string_lossy());
            // Keep UTF-16 & byte identity buffers alive until `CfCreatePlaceholders` finishes.
            let mut keep_names: Vec<Vec<u16>> = Vec::with_capacity(items.len());
            let mut keep_ids: Vec<Vec<u8>> = Vec::with_capacity(items.len());
            let mut infos: Vec<CF_PLACEHOLDER_CREATE_INFO> = Vec::with_capacity(items.len());
            for item in &items {
                let leaf = Path::new(&item.rel_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&item.rel_path);
                keep_names.push(wide(leaf));
                let id_str = item.real_path.clone().unwrap_or_else(|| item.rel_path.clone());
                keep_ids.push(id_str.into_bytes());
            }
            for ((item, name_w), id_bytes) in items.iter().zip(&keep_names).zip(&keep_ids) {
                let mut flags = CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC;
                if item.dir {
                    flags |= CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION;
                }
                infos.push(CF_PLACEHOLDER_CREATE_INFO {
                    RelativeFileName: PCWSTR(name_w.as_ptr()),
                    FsMetadata: metadata_of(item),
                    FileIdentity: id_bytes.as_ptr().cast(),
                    FileIdentityLength: id_bytes.len() as u32,
                    Flags: flags,
                    ..Default::default()
                });
            }
            if !infos.is_empty() {
                // SAFETY: `keep_names` and `keep_ids` outlive `CfCreatePlaceholders`.
                let _ = unsafe { CfCreatePlaceholders(PCWSTR(dir_w.as_ptr()), &mut infos, CF_CREATE_FLAG_NONE, None) };
            }
        }
        Ok(())
    }

    pub(super) fn unmount(portal: &Portal) {
        if let Some(conn) = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&portal.id) {
            // SAFETY: Disconnects registered CfAPI connection key.
            unsafe {
                let _ = CfDisconnectSyncRoot(CF_CONNECTION_KEY(conn.key));
            }
            let w = wide(&conn.mount);
            // SAFETY: Unregisters sync root and cleans placeholder tree.
            unsafe {
                let _ = CfUnregisterSyncRoot(PCWSTR(w.as_ptr()));
            }
        } else {
            let w = wide(&portal.mount_path);
            unsafe {
                let _ = CfUnregisterSyncRoot(PCWSTR(w.as_ptr()));
            }
        }
        SOURCES.lock().unwrap_or_else(|e| e.into_inner()).remove(&portal.id);
        if super::portals_root().is_some_and(|root| super::is_portal_folder(&portal.mount_path, &root)) {
            let _ = fs::remove_dir_all(&portal.mount_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Command;

    #[test]
    fn only_a_folder_made_for_a_portal_is_ever_deleted_on_unmount() {
        let root = Path::new("C:\\Users\\me\\Glint portals");
        assert!(is_portal_folder("C:\\Users\\me\\Glint portals\\Repo", root));
        assert!(is_portal_folder("c:\\users\\ME\\glint portals\\تقارير", root));
        // A hand-edited portals.json must not turn Unmount into "delete my documents".
        assert!(!is_portal_folder("C:\\Users\\me\\Documents", root));
        assert!(!is_portal_folder("C:\\Users\\me\\Glint portals", root));
        assert!(!is_portal_folder("C:\\Users\\me\\Glint portals\\a\\b", root));
        assert!(!is_portal_folder("C:\\Users\\me\\Glint portals\\..", root));
        assert!(!is_portal_folder("C:\\", root));
        assert!(!is_portal_folder("", root));
    }

    #[test]
    fn listeners_hear_changes_until_they_say_they_are_done() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = heard.clone();
        on_change(move || counter.fetch_add(1, Ordering::SeqCst) < 1);
        changed();
        changed();
        changed();
        // The first call said "keep going", the second said "done", and nothing called it after that.
        assert_eq!(heard.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn portal_json_and_name_sanitization_round_trip() {
        assert_eq!(sanitize_name("v0.14: release <tag>?"), "v0.14_ release _tag__");
        assert_eq!(sanitize_name("...   "), "Portal");

        let p = Portal {
            id: "portal-1".into(),
            name: "Repo Snapshot".into(),
            mount_path: "C:\\Users\\me\\Glint portals\\Repo Snapshot".into(),
            source: PortalSource::GitCommit {
                repo_root: "C:\\Repo".into(),
                rev: "abc123".into(),
                subfolder: "src".into(),
            },
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Portal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn git_portal_lists_and_hydrates_blobs_from_commit() {
        let dir = std::env::temp_dir().join(format!("glint-portal-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
        };
        assert!(run(&["init"]).success());
        fs::write(dir.join("hello.txt"), b"hello portal").unwrap();
        fs::write(dir.join("sub").join("nested.rs"), b"fn main() {}").unwrap();
        assert!(run(&["add", "."]).success());
        assert!(run(&["commit", "-m", "init"]).success());

        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let oid = String::from_utf8_lossy(&head.stdout).trim().to_string();
        let source = PortalSource::GitCommit {
            repo_root: dir.to_string_lossy().into_owned(),
            rev: oid,
            subfolder: dir.to_string_lossy().into_owned(),
        };
        let list = portal_list_entries(&source).unwrap();
        assert!(list.iter().any(|e| e.rel_path == "hello.txt" && !e.dir));
        assert!(list.iter().any(|e| e.rel_path == "sub\\nested.rs" && !e.dir));
        assert_eq!(read_identity_bytes(&source, "hello.txt").unwrap(), b"hello portal");
        assert_eq!(read_identity_bytes(&source, "sub\\nested.rs").unwrap(), b"fn main() {}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_portal_lists_and_hydrates_archived_entries() {
        let dir = std::env::temp_dir().join(format!("glint-portal-zip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("pack.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            z.start_file("inside/readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"archived portal bytes").unwrap();
            z.finish().unwrap();
        }

        let source = PortalSource::Archive {
            archive_path: zip_path.to_string_lossy().into_owned(),
        };
        let list = portal_list_entries(&source).unwrap();
        assert!(list.iter().any(|e| e.rel_path == "inside\\readme.txt" && !e.dir));
        assert_eq!(
            read_identity_bytes(&source, "inside\\readme.txt").unwrap(),
            b"archived portal bytes"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
