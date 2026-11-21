//! Persisted per-folder OKLab dominant color palette index for 120 FPS photo color surfing.
//!
//! Populated passively whenever [`crate::thumbs`] decodes thumbnails (`record_bgra`), and warmed
//! on demand via Windows thumbnail extraction when opening the Color Wheel in an uncached folder.

use crate::thumbs::Outcome;
use glint_model::oklab::{extract_palette_bgra, Oklab, Palette3};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

const MAGIC: &[u8; 5] = b"GLOK1";
const SAVE_BATCH_THRESHOLD: usize = 24;

#[derive(Clone, Debug)]
struct Record {
    modified: i64,
    palette: Palette3,
}

#[derive(Default)]
struct Cache {
    loaded: bool,
    entries: HashMap<String, Record>,
    dirty_adds: usize,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
static SAVING: AtomicBool = AtomicBool::new(false);

fn lock_cache() -> MutexGuard<'static, Cache> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if !guard.loaded {
        guard.loaded = true;
        if let Some(path) = index_file() {
            if let Ok(bytes) = std::fs::read(path) {
                guard.entries = decode_binary(&bytes);
            }
        }
    }
    guard
}

fn index_file() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("color-palettes.bin"))
}

fn is_color_image(path: &str) -> bool {
    const EXTS: &[&str] = &[
        "jpg", "jpeg", "jpe", "jfif", "png", "webp", "bmp", "gif", "tif", "tiff", "heic", "heif", "avif", "jxl",
    ];
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTS.iter().any(|k| k.eq_ignore_ascii_case(ext)))
}

fn encode_binary(entries: &HashMap<String, Record>) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + entries.len() * 88);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (path, rec) in entries {
        let pb = path.as_bytes();
        let Ok(len) = u16::try_from(pb.len()) else {
            continue;
        };
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(pb);
        out.extend_from_slice(&rec.modified.to_le_bytes());
        for c in &rec.palette.colors {
            out.extend_from_slice(&c.l.to_le_bytes());
            out.extend_from_slice(&c.a.to_le_bytes());
            out.extend_from_slice(&c.b.to_le_bytes());
        }
        for w in &rec.palette.weights {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
    out
}

fn decode_binary(data: &[u8]) -> HashMap<String, Record> {
    let mut map = HashMap::new();
    if data.len() < 9 || &data[..5] != MAGIC {
        return map;
    }
    let count = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
    let mut at = 9usize;
    let read_f32 = |buf: &[u8], off: &mut usize| -> Option<f32> {
        let slice: [u8; 4] = buf.get(*off..*off + 4)?.try_into().ok()?;
        *off += 4;
        Some(f32::from_le_bytes(slice))
    };
    for _ in 0..count {
        let Some(len_bytes) = data.get(at..at + 2) else {
            break;
        };
        let path_len = u16::from_le_bytes([len_bytes[0], len_bytes[1]]) as usize;
        at += 2;
        let Some(path_bytes) = data.get(at..at + path_len) else {
            break;
        };
        let Ok(path) = std::str::from_utf8(path_bytes) else {
            break;
        };
        at += path_len;
        let Some(mod_bytes) = data.get(at..at + 8) else {
            break;
        };
        let Ok(mod_arr) = <[u8; 8]>::try_from(mod_bytes) else {
            break;
        };
        let modified = i64::from_le_bytes(mod_arr);
        at += 8;

        let mut colors = [Oklab { l: 0.0, a: 0.0, b: 0.0 }; 3];
        let mut weights = [0.0f32; 3];
        let mut ok = true;
        for c in &mut colors {
            if let (Some(l), Some(a), Some(b)) = (
                read_f32(data, &mut at),
                read_f32(data, &mut at),
                read_f32(data, &mut at),
            ) {
                *c = Oklab { l, a, b };
            } else {
                ok = false;
                break;
            }
        }
        if !ok {
            break;
        }
        for w in &mut weights {
            if let Some(val) = read_f32(data, &mut at) {
                *w = val;
            } else {
                ok = false;
                break;
            }
        }
        if !ok {
            break;
        }
        map.insert(
            path.to_string(),
            Record {
                modified,
                palette: Palette3 { colors, weights },
            },
        );
    }
    map
}

fn schedule_save() {
    if cfg!(test) || SAVING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let bytes = {
            let mut cache = lock_cache();
            cache.dirty_adds = 0;
            encode_binary(&cache.entries)
        };
        if let Some(file) = index_file() {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(file, bytes);
        }
        SAVING.store(false, Ordering::SeqCst);
    });
}

/// Passively records an extracted OKLab palette from already-decoded BGRA thumbnail pixels.
pub fn record_bgra(path: &str, modified: i64, width: u32, height: u32, bgra: &[u8]) {
    if !is_color_image(path) {
        return;
    }
    let key = path.to_lowercase();
    {
        let cache = lock_cache();
        if let Some(existing) = cache.entries.get(&key) {
            if existing.modified == modified {
                return;
            }
        }
    }
    let Some(palette) = extract_palette_bgra(width, height, bgra) else {
        return;
    };
    let should_save = {
        let mut cache = lock_cache();
        cache.entries.insert(key, Record { modified, palette });
        cache.dirty_adds += 1;
        cache.dirty_adds >= SAVE_BATCH_THRESHOLD
    };
    if should_save {
        schedule_save();
    }
}

/// Returns a cached OKLab palette for `(path, modified)` if available.
pub fn palette_of(path: &str, modified: i64) -> Option<Palette3> {
    let key = path.to_lowercase();
    let cache = lock_cache();
    cache
        .entries
        .get(&key)
        .filter(|r| modified == 0 || r.modified == modified)
        .map(|r| r.palette)
}

/// Returns all cached palettes for `(path, modified)` items, keyed by lowercase path.
pub fn lookup_batch(items: &[(String, i64)]) -> HashMap<String, Palette3> {
    let cache = lock_cache();
    let mut out = HashMap::new();
    for (path, modified) in items {
        let key = path.to_lowercase();
        if let Some(rec) = cache.entries.get(&key) {
            if *modified == 0 || rec.modified == *modified {
                out.insert(key, rec.palette);
            }
        }
    }
    out
}

/// Extracts and caches palettes for uncached images in `items` (up to 256 per call), returning the merged map.
pub fn warm_missing(items: &[(String, i64)]) -> HashMap<String, Palette3> {
    let mut result = lookup_batch(items);
    let missing: Vec<(String, String, i64)> = items
        .iter()
        .filter(|(p, _)| is_color_image(p))
        .filter_map(|(p, m)| {
            let key = p.to_lowercase();
            if result.contains_key(&key) {
                None
            } else {
                Some((key, p.clone(), *m))
            }
        })
        .take(256)
        .collect();

    if missing.is_empty() {
        return result;
    }

    #[cfg(windows)]
    {
        crate::thumbs::win::init_thread(true);
        let extracted = std::sync::Mutex::new(Vec::with_capacity(missing.len()));
        for chunk in missing.chunks(64) {
            for (key, path, modified) in chunk {
                let outcome = crate::thumbs::win::outcome(path, 64, false);
                if let Outcome::Found { pixels, .. } | Outcome::Smaller { pixels, .. } = outcome {
                    if let Some(palette) = extract_palette_bgra(pixels.width, pixels.height, &pixels.bgra) {
                        extracted
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push((key.clone(), *modified, palette));
                    }
                }
            }
        }
        let added = extracted.into_inner().unwrap_or_else(|e| e.into_inner());
        if !added.is_empty() {
            {
                let mut cache = lock_cache();
                for (key, modified, palette) in &added {
                    cache.entries.insert(
                        key.clone(),
                        Record {
                            modified: *modified,
                            palette: *palette,
                        },
                    );
                    result.insert(key.clone(), *palette);
                }
            }
            schedule_save();
        }
    }

    #[cfg(not(windows))]
    let _ = Outcome::NotCached;

    result
}

/// Decodes a single image's dominant palette synchronously when not yet in cache.
pub fn dominant_color_of(path: &str, modified: i64) -> Option<Oklab> {
    if let Some(p) = palette_of(path, modified) {
        return Some(p.colors[0]);
    }
    let warmed = warm_missing(&[(path.to_string(), modified)]);
    warmed.get(&path.to_lowercase()).map(|p| p.colors[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_round_trip_and_truncation_safety() {
        let mut map = HashMap::new();
        map.insert(
            r"c:\photos\sunset.jpg".to_string(),
            Record {
                modified: 1_700_000_000_000,
                palette: Palette3 {
                    colors: [
                        Oklab {
                            l: 0.72,
                            a: 0.14,
                            b: 0.11,
                        },
                        Oklab {
                            l: 0.45,
                            a: -0.05,
                            b: -0.18,
                        },
                        Oklab {
                            l: 0.22,
                            a: 0.01,
                            b: 0.02,
                        },
                    ],
                    weights: [0.65, 0.25, 0.10],
                },
            },
        );
        let bytes = encode_binary(&map);
        let decoded = decode_binary(&bytes);
        assert_eq!(decoded.len(), 1);
        let rec = decoded.get(r"c:\photos\sunset.jpg").expect("present");
        assert_eq!(rec.modified, 1_700_000_000_000);
        assert!((rec.palette.weights[0] - 0.65).abs() < 1e-6);

        // Truncated or corrupted input must never panic.
        assert!(decode_binary(b"BAD!").is_empty());
        for cut in 0..bytes.len() {
            let _ = decode_binary(&bytes[..cut]);
        }
    }

    #[test]
    fn record_and_lookup_batch_round_trips() {
        let path = format!(r"C:\__glint_color_test_{}\sample.png", std::process::id());
        // 4x4 pure bright red BGRA ([0, 0, 255, 255])
        let mut bgra = Vec::with_capacity(4 * 4 * 4);
        for _ in 0..16 {
            bgra.extend_from_slice(&[0, 0, 255, 255]);
        }
        record_bgra(&path, 12345, 4, 4, &bgra);
        let got = lookup_batch(&[(path.clone(), 12345)]);
        let key = path.to_lowercase();
        let pal = got.get(&key).expect("palette recorded");
        assert!(pal.colors[0].a > 0.15);

        // Clean test entry from global cache.
        lock_cache().entries.remove(&key);
    }
}
