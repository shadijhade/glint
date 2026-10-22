//! Deep File X-Ray: NTFS Alternate Data Streams (`:Zone.Identifier` inspector & unblocker),
//! physical vs. logical disk allocation (`GetCompressedFileSizeW`), MFT identity & hardlink count
//! (`GetFileInformationByHandle`), Shannon entropy spectrum, and single-pass SHA-256 + CRC32.

use crate::fsx::blocking;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Bytes hashed when the X-Ray opens (256 MB), so a multi-GB ISO doesn't keep it waiting; asked for, the
/// whole file is hashed.
const MAX_HASH_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum bytes sampled for the Shannon entropy & byte distribution spectrum (4 MB).
const MAX_ENTROPY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NtfsStream {
    /** Stream name such as ":Zone.Identifier:$DATA" */
    pub name: String,
    /** Clean stream identifier without ":$DATA", e.g. "Zone.Identifier" */
    pub clean_name: String,
    pub size: u64,
    /** Text preview if the stream is small UTF-8 text (e.g. Zone.Identifier). */
    pub preview: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneIdentifier {
    pub zone_id: u32,
    pub zone_label: String,
    pub host_url: Option<String>,
    pub referrer_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileXRayReport {
    pub path: String,
    pub logical_size: u64,
    pub allocated_size: u64,
    pub hardlink_count: u32,
    pub file_index: String,
    pub volume_serial: String,
    pub attributes: Vec<String>,
    pub streams: Vec<NtfsStream>,
    pub zone_info: Option<ZoneIdentifier>,
    /** Shannon entropy in bits per byte (0.00 to 8.00). */
    pub entropy: f64,
    pub entropy_label: String,
    /** 16 buckets (each covering 16 byte values: 0x00..0x0F, ..., 0xF0..0xFF) as percentages 0..100. */
    pub byte_spectrum: [f64; 16],
    pub null_byte_pct: f64,
    pub ascii_pct: f64,
    pub high_byte_pct: f64,
    pub sha256: String,
    pub crc32: String,
    pub hashes_complete: bool,
    pub elapsed_ms: f64,
}

static CANCEL: AtomicBool = AtomicBool::new(false);

/// Closing X-Ray stops a full hash of a big file.
#[tauri::command]
pub fn cancel_xray() {
    CANCEL.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub async fn inspect_file_xray(path: String, full: Option<bool>) -> Result<FileXRayReport, String> {
    CANCEL.store(false, Ordering::Relaxed);
    blocking(move || analyze_file(&path, full.unwrap_or(false), &CANCEL)).await
}

#[tauri::command]
pub async fn remove_ntfs_stream(path: String, stream_name: String) -> Result<(), String> {
    blocking(move || delete_ads(&path, &stream_name)).await
}

fn clean_stream_name(name: &str) -> &str {
    name.trim_start_matches(':').trim_end_matches(":$DATA").trim_end_matches(":$data")
}

/// Deletes one named stream, and only one the file really has: a name that NTFS would read as the
/// file's own data (`::$DATA`, `:$DATA`…) can never get here and delete the file instead.
fn delete_ads(path: &str, stream_name: &str) -> Result<(), String> {
    let clean = clean_stream_name(stream_name);
    if clean.is_empty() || clean.contains([':', '\\', '/']) || clean.eq_ignore_ascii_case("$DATA") {
        return Err("Invalid NTFS stream name.".into());
    }
    if !list_streams(path).iter().any(|s| s.clean_name.eq_ignore_ascii_case(clean)) {
        return Err(format!("This file has no “:{clean}” stream."));
    }
    let full_stream_path = format!("{path}:{clean}");
    fs::remove_file(&full_stream_path).map_err(|e| format!("Couldn't remove stream “:{clean}”: {e}"))
}

// ---------------------------------------------------------------------------
// Windows NTFS metadata, physical allocation & Alternate Data Streams
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn inspect_ntfs_metadata(path: &str, logical_size: u64) -> (u64, u32, String, String, Vec<String>, Vec<NtfsStream>) {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetCompressedFileSizeW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect();

    // 1. Physical allocated size on disk via GetCompressedFileSizeW
    let mut high: u32 = 0;
    let low = unsafe { GetCompressedFileSizeW(PCWSTR(wide.as_ptr()), Some(&mut high)) };
    let allocated_size = if low == u32::MAX {
        logical_size
    } else {
        ((high as u64) << 32) | (low as u64)
    };

    // 2. MFT File Index, Volume Serial, Hardlink count, and NTFS attributes
    let mut hardlink_count = 1u32;
    let mut file_index = String::from("—");
    let mut volume_serial = String::from("—");
    let mut attributes = Vec::new();

    if let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
    {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.is_ok() {
            hardlink_count = info.nNumberOfLinks;
            let idx = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
            file_index = format!("0x{idx:016X}");
            let vs = info.dwVolumeSerialNumber;
            volume_serial = format!("{:04X}-{:04X}", vs >> 16, vs & 0xFFFF);

            let attr = info.dwFileAttributes;
            const FLAGS: &[(u32, &str)] = &[
                (0x0000_0001, "Read-Only"),
                (0x0000_0002, "Hidden"),
                (0x0000_0004, "System"),
                (0x0000_0020, "Archive"),
                (0x0000_0200, "Sparse"),
                (0x0000_0400, "Reparse Point"),
                (0x0000_0800, "Compressed"),
                (0x0000_1000, "Offline"),
                (0x0000_4000, "Encrypted (EFS)"),
                (0x0008_0000, "Pinned"),
                (0x0010_0000, "Unpinned"),
            ];
            for &(bit, label) in FLAGS {
                if (attr & bit) != 0 {
                    attributes.push(label.to_string());
                }
            }
        }
    }

    (allocated_size, hardlink_count, file_index, volume_serial, attributes, list_streams(path))
}

/// The file's named streams (Alternate Data Streams) via FindFirstStreamW / FindNextStreamW.
#[cfg(windows)]
fn list_streams(path: &str) -> Vec<NtfsStream> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard, WIN32_FIND_STREAM_DATA};

    let wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect();
    let mut streams = Vec::new();
    let mut stream_data = WIN32_FIND_STREAM_DATA::default();
    if let Ok(handle) = unsafe {
        FindFirstStreamW(
            PCWSTR(wide.as_ptr()),
            FindStreamInfoStandard,
            &mut stream_data as *mut _ as *mut _,
            None,
        )
    } {
        loop {
            let len = stream_data.cStreamName.iter().position(|&c| c == 0).unwrap_or(stream_data.cStreamName.len());
            let name = String::from_utf16_lossy(&stream_data.cStreamName[..len]);
            // Skip the default unnamed data stream "::$"
            if !name.eq_ignore_ascii_case("::$DATA") && !name.is_empty() {
                let clean_name = clean_stream_name(&name).to_string();
                let size = stream_data.StreamSize.max(0) as u64;
                let preview = if size > 0 && size <= 4096 && !clean_name.is_empty() {
                    fs::read_to_string(format!("{path}:{clean_name}"))
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                };
                streams.push(NtfsStream {
                    name,
                    clean_name,
                    size,
                    preview,
                });
            }
            stream_data = WIN32_FIND_STREAM_DATA::default();
            if unsafe { FindNextStreamW(handle, &mut stream_data as *mut _ as *mut _) }.is_err() {
                break;
            }
        }
        let _ = unsafe { FindClose(handle) };
    }
    streams
}

#[cfg(not(windows))]
fn inspect_ntfs_metadata(_path: &str, logical_size: u64) -> (u64, u32, String, String, Vec<String>, Vec<NtfsStream>) {
    (logical_size, 1, String::from("—"), String::from("—"), Vec::new(), Vec::new())
}

#[cfg(not(windows))]
fn list_streams(_path: &str) -> Vec<NtfsStream> {
    Vec::new()
}

/// The zone Windows marked a downloaded file with (3: the internet, 4: restricted sites), if it was;
/// a mark that can't be read counts as the internet.
pub(crate) fn zone_of(path: &str) -> Option<u32> {
    let bytes = fs::read(format!("{path}:Zone.Identifier")).ok()?;
    Some(parse_zone_identifier(&String::from_utf8_lossy(&bytes)).map_or(3, |z| z.zone_id))
}

fn parse_zone_identifier(text: &str) -> Option<ZoneIdentifier> {
    let mut zone_id = None;
    let mut host_url = None;
    let mut referrer_url = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "ZoneId" => zone_id = v.trim().parse::<u32>().ok(),
                "HostUrl" => host_url = Some(v.trim().to_string()),
                "ReferrerUrl" => referrer_url = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    let zid = zone_id?;
    let label = match zid {
        0 => "Local Machine (Zone 0)",
        1 => "Local Intranet (Zone 1)",
        2 => "Trusted Sites (Zone 2)",
        3 => "Internet Download (Zone 3 — Blocked)",
        4 => "Restricted Sites (Zone 4 — Blocked)",
        _ => "Unknown Security Zone",
    };
    Some(ZoneIdentifier {
        zone_id: zid,
        zone_label: label.to_string(),
        host_url,
        referrer_url,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn analyze_file(path: &str, full: bool, cancel: &AtomicBool) -> Result<FileXRayReport, String> {
    let started = Instant::now();
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let logical_size = meta.len();

    let (allocated_size, hardlink_count, file_index, volume_serial, attributes, streams) =
        inspect_ntfs_metadata(path, logical_size);

    let zone_info = streams
        .iter()
        .find(|s| s.clean_name.eq_ignore_ascii_case("Zone.Identifier"))
        .and_then(|s| s.preview.as_deref())
        .and_then(parse_zone_identifier);

    // Single-pass streaming read for SHA-256, CRC32, and Shannon Entropy
    let limit = if full { u64::MAX } else { MAX_HASH_BYTES };
    let mut sha256 = Sha256::new();
    let mut crc32 = crc32fast::Hasher::new();
    let mut counts = [0u64; 256];
    let mut entropy_sampled = 0usize;
    let mut bytes_hashed = 0u64;

    if !meta.is_dir() && logical_size > 0 {
        if let Ok(mut file) = File::open(Path::new(path)) {
            let mut buf = vec![0u8; 64 * 1024];
            while bytes_hashed < limit {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let n = match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                let slice = &buf[..n];
                sha256.update(slice);
                crc32.update(slice);
                bytes_hashed += n as u64;

                if entropy_sampled < MAX_ENTROPY_BYTES {
                    let take = (MAX_ENTROPY_BYTES - entropy_sampled).min(n);
                    for &b in &slice[..take] {
                        counts[b as usize] += 1;
                    }
                    entropy_sampled += take;
                }
            }
        }
    }

    let mut entropy = 0.0f64;
    let mut byte_spectrum = [0.0f64; 16];
    let mut null_byte_pct = 0.0f64;
    let mut ascii_pct = 0.0f64;
    let mut high_byte_pct = 0.0f64;

    if entropy_sampled > 0 {
        let total = entropy_sampled as f64;
        for (byte_val, &c) in counts.iter().enumerate() {
            if c > 0 {
                let p = (c as f64) / total;
                entropy -= p * p.log2();
                byte_spectrum[byte_val / 16] += p * 100.0;
            }
        }
        null_byte_pct = (counts[0] as f64 / total) * 100.0;
        let ascii_count: u64 = counts[0x20..=0x7E].iter().sum::<u64>()
            + counts[b'\n' as usize]
            + counts[b'\r' as usize]
            + counts[b'\t' as usize];
        ascii_pct = (ascii_count as f64 / total) * 100.0;
        let high_count: u64 = counts[0x80..=0xFF].iter().sum();
        high_byte_pct = (high_count as f64 / total) * 100.0;
    }

    let entropy_label = if entropy_sampled == 0 {
        "Empty (0.00 bits/B)"
    } else if entropy < 3.5 {
        "Low Entropy — Repetitive / Sparse Data"
    } else if entropy < 5.3 {
        "Medium Entropy — Plain Text / Source Code"
    } else if entropy < 6.8 {
        "Structured Binary — Native Executable / Database"
    } else if entropy < 7.85 {
        "High Entropy — Compressed Media / Archive"
    } else {
        "Maximum Entropy — Encrypted / Random Stream"
    };

    Ok(FileXRayReport {
        path: path.to_string(),
        logical_size,
        allocated_size,
        hardlink_count,
        file_index,
        volume_serial,
        attributes,
        streams,
        zone_info,
        entropy,
        entropy_label: entropy_label.to_string(),
        byte_spectrum,
        null_byte_pct,
        ascii_pct,
        high_byte_pct,
        sha256: hex(&sha256.finalize()),
        crc32: format!("{:08x}", crc32.finalize()),
        hashes_complete: bytes_hashed >= logical_size,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_standard_checksums() {
        let dir = std::env::temp_dir().join(format!("glint_xray_sums_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // NIST's "abc", and the IEEE 802.3 check value of "123456789".
        let abc = dir.join("abc.txt");
        fs::write(&abc, b"abc").unwrap();
        let report = analyze_file(&abc.to_string_lossy(), false, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(report.hashes_complete);
        let digits = dir.join("digits.txt");
        fs::write(&digits, b"123456789").unwrap();
        assert_eq!(analyze_file(&digits.to_string_lossy(), false, &AtomicBool::new(false)).unwrap().crc32, "cbf43926");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_deletes_the_file_itself() {
        let dir = std::env::temp_dir().join(format!("glint_xray_main_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keep.txt");
        fs::write(&file, b"still here").unwrap();
        let path = file.to_string_lossy();
        for name in ["::$DATA", ":$DATA", "$DATA", "", ":", "Zone.Identifier", "x:y"] {
            assert!(delete_ads(&path, name).is_err(), "{name:?} should be refused");
        }
        assert_eq!(fs::read(&file).unwrap(), b"still here");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyzes_file_and_detects_ntfs_alternate_data_stream() {
        let dir = std::env::temp_dir().join(format!("glint_xray_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let file_path = dir.join("sample.bin");
        fs::write(&file_path, b"Hello from Glint X-Ray!").unwrap();

        // Write a simulated Zone.Identifier NTFS Alternate Data Stream
        let ads_path = format!("{}:Zone.Identifier", file_path.display());
        let ads_written = fs::write(&ads_path, "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/file.zip\r\n").is_ok();

        let report = analyze_file(&file_path.to_string_lossy(), false, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.logical_size, 23);
        assert!(report.entropy > 3.0 && report.entropy < 5.0);
        if ads_written {
            assert!(report.streams.iter().any(|s| s.clean_name == "Zone.Identifier"));
            assert_eq!(report.zone_info.as_ref().unwrap().zone_id, 3);
            delete_ads(&file_path.to_string_lossy(), "Zone.Identifier").unwrap();
            let after = analyze_file(&file_path.to_string_lossy(), false, &AtomicBool::new(false)).unwrap();
            assert!(after.zone_info.is_none());
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
