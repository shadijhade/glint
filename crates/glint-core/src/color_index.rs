//! Persisted per-folder OKLab dominant color palette index for 120 FPS photo color surfing.
//!
//! Populated passively whenever [`crate::thumbs`] decodes thumbnails (`record_bgra`), and warmed
//! on demand via Windows thumbnail extraction when opening the Color Wheel in an uncached folder.

use crate::thumbs::{Outcome, Pixels};
use glint_model::oklab::{extract_palette_bgra, Oklab, Palette3};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};

const MAGIC: &[u8; 5] = b"GLOK1";
const SAVE_BATCH_THRESHOLD: usize = 128;
const SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(5);

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
static SAVE_WANTED: AtomicBool = AtomicBool::new(false);

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
    if data.len() < 9 || &data[..5] != MAGIC {
        return HashMap::new();
    }
    let count = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
    let mut map = HashMap::with_capacity(count.min((data.len() - 9) / 58));
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
    if cfg!(test) {
        return;
    }
    SAVE_WANTED.store(true, Ordering::SeqCst);
    if SAVING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(SAVE_DEBOUNCE);
        SAVE_WANTED.store(false, Ordering::SeqCst);
        let bytes = {
            let mut cache = lock_cache();
            cache.dirty_adds = 0;
            encode_binary(&cache.entries)
        };
        if let Some(file) = index_file() {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = crate::replace_contents(&file, &bytes);
        }
        if !SAVE_WANTED.load(Ordering::SeqCst) {
            SAVING.store(false, Ordering::SeqCst);
            if SAVE_WANTED.load(Ordering::SeqCst) && !SAVING.swap(true, Ordering::SeqCst) {
                continue;
            }
            break;
        }
    });
}

/// How many thumbnails wait for their palette at most: past it a new one is dropped, never waited for.
const OFFER_QUEUE: usize = 64;
/// How many waiting thumbnails the recorder takes together (one look at the cache to see which are new, one to
/// add the palettes made).
const RECORD_BATCH: usize = 16;

/// A thumbnail's pixels waiting for their palette.
struct Offer {
    path: String,
    modified: i64,
    pixels: Arc<Pixels>,
}

static OFFERS: OnceLock<SyncSender<Offer>> = OnceLock::new();

/// Hands a thumbnail that was just made or found to the palette recorder, after its waiters have been answered.
/// Never blocks and never copies the pixels: when the recorder is behind (a screenful of thumbnails at once) the
/// offer is dropped, and the photo's palette comes the next time its thumbnail does, or from the wheel's warming.
pub fn offer(path: &str, modified: i64, pixels: &Arc<Pixels>) {
    if !is_color_image(path) {
        return;
    }
    let sender = OFFERS.get_or_init(|| {
        let (sender, receiver) = sync_channel(OFFER_QUEUE);
        // If the thread can't start the receiver goes with it and every offer is dropped, which is harmless.
        let _ = std::thread::Builder::new()
            .name("color-palettes".into())
            .spawn(move || record_offers(&receiver));
        sender
    });
    let _ = try_offer(sender, path, modified, pixels);
}

/// One offer into `sender` without waiting; `false` when it was dropped (the queue is full or the recorder gone).
fn try_offer(sender: &SyncSender<Offer>, path: &str, modified: i64, pixels: &Arc<Pixels>) -> bool {
    sender
        .try_send(Offer {
            path: path.to_string(),
            modified,
            pixels: pixels.clone(),
        })
        .is_ok()
}

/// The recorder: one background thread that makes palettes out of the offers, a batch at a time.
fn record_offers(receiver: &Receiver<Offer>) {
    #[cfg(windows)]
    crate::thumbs::win::init_thread(true);
    while let Ok(first) = receiver.recv() {
        let mut batch = vec![first];
        while batch.len() < RECORD_BATCH {
            match receiver.try_recv() {
                Ok(next) => batch.push(next),
                Err(_) => break,
            }
        }
        record_offered(batch);
    }
}

/// Records a batch: the cache is locked once to see which photos are new, the palettes are made without it, and
/// locked once more to add them.
fn record_offered(batch: Vec<Offer>) {
    let keyed: Vec<(String, Offer)> = batch.into_iter().map(|o| (o.path.to_lowercase(), o)).collect();
    let fresh: Vec<&(String, Offer)> = {
        let cache = lock_cache();
        keyed
            .iter()
            .filter(|(key, o)| cache.entries.get(key).is_none_or(|r| r.modified != o.modified))
            .collect()
    };
    let made: Vec<(&String, Record)> = fresh
        .into_iter()
        .filter_map(|(key, o)| {
            let palette = extract_palette_bgra(o.pixels.width, o.pixels.height, &o.pixels.bgra)?;
            Some((
                key,
                Record {
                    modified: o.modified,
                    palette,
                },
            ))
        })
        .collect();
    if made.is_empty() {
        return;
    }
    let should_save = {
        let mut cache = lock_cache();
        for (key, record) in made {
            cache.entries.insert(key.clone(), record);
            cache.dirty_adds += 1;
        }
        cache.dirty_adds >= SAVE_BATCH_THRESHOLD
    };
    if should_save {
        schedule_save();
    }
}

/// Records an extracted OKLab palette from already-decoded BGRA pixels, on the calling thread. The thumbnails
/// themselves go through [`offer`] instead, which keeps this work off their path.
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

/// Photos warmed per call at most.
const WARM_LIMIT: usize = 256;
/// Threads that warm at once.
const WARM_WORKERS: usize = 4;
/// A worker hands over its palettes this many at a time, so matches show while the rest are being made.
const WARM_STREAM: usize = 8;

/// Extracts and caches palettes for uncached images in `items` (up to 256 per call), returning the merged map.
pub fn warm_missing(items: &[(String, i64)]) -> HashMap<String, Palette3> {
    let mut result = lookup_batch(items);
    warm_missing_streaming(items, &AtomicBool::new(false), |batch| result.extend(batch));
    result
}

/// Like [`warm_missing`] but hands each group of palettes made to `on_batch` as it is ready (on the calling thread),
/// keyed by lowercase path, so the photos that match can show before the rest are done. Photos already in the cache
/// are not repeated: the caller has them from [`lookup_batch`]. Setting `stop` ends it after the photos being read
/// (the folder was left, the filter cleared): what was made by then is kept.
pub fn warm_missing_streaming(
    items: &[(String, i64)],
    stop: &AtomicBool,
    on_batch: impl FnMut(HashMap<String, Palette3>),
) {
    warm_with(items, WARM_WORKERS, stop, on_batch);
}

/// The photos among `items` with no palette yet (at most [`WARM_LIMIT`]), as (key, path, modified).
fn missing_of(items: &[(String, i64)]) -> Vec<(String, String, i64)> {
    let cache = lock_cache();
    items
        .iter()
        .filter(|(p, _)| is_color_image(p))
        .filter_map(|(p, m)| {
            let key = p.to_lowercase();
            match cache.entries.get(&key) {
                Some(rec) if *m == 0 || rec.modified == *m => None,
                _ => Some((key, p.clone(), *m)),
            }
        })
        .take(WARM_LIMIT)
        .collect()
}

/// [`warm_missing_streaming`] with `workers` threads: each is set up like the thumbnail threads
/// (`thumbs::win::init_thread`), takes the next photo from a shared counter, and sends its palettes in groups.
fn warm_with(
    items: &[(String, i64)],
    workers: usize,
    stop: &AtomicBool,
    mut on_batch: impl FnMut(HashMap<String, Palette3>),
) {
    let missing = missing_of(items);
    if missing.is_empty() {
        return;
    }
    #[cfg(windows)]
    {
        type Made = Vec<(String, i64, Palette3)>;
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (sender, receiver) = std::sync::mpsc::channel::<Made>();
        let (missing_ref, next_ref) = (&missing, &next);
        let mut added = false;
        std::thread::scope(|scope| {
            for _ in 0..workers.clamp(1, WARM_WORKERS).min(missing.len()) {
                let sender = sender.clone();
                scope.spawn(move || {
                    crate::thumbs::win::init_thread(true);
                    let mut group: Made = Vec::with_capacity(WARM_STREAM);
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let at = next_ref.fetch_add(1, Ordering::SeqCst);
                        let Some((key, path, modified)) = missing_ref.get(at) else {
                            break;
                        };
                        // A bug in a decoder mustn't take the whole warming down with it.
                        let outcome = std::panic::catch_unwind(|| crate::thumbs::win::outcome(path, 64, false))
                            .unwrap_or(Outcome::Nothing { lasting: false });
                        if let Outcome::Found { pixels, .. } | Outcome::Smaller { pixels, .. } = outcome {
                            if let Some(palette) = extract_palette_bgra(pixels.width, pixels.height, &pixels.bgra) {
                                group.push((key.clone(), *modified, palette));
                            }
                        }
                        if group.len() >= WARM_STREAM {
                            let _ = sender.send(std::mem::take(&mut group));
                        }
                    }
                    if !group.is_empty() {
                        let _ = sender.send(group);
                    }
                    // These threads live for one warming: COM is let go with them (init_thread's threads live on).
                    // SAFETY: matches init_thread's CoInitializeEx on this same, freshly made thread.
                    unsafe { windows::Win32::System::Com::CoUninitialize() };
                });
            }
            drop(sender);
            for group in receiver {
                let mut batch = HashMap::with_capacity(group.len());
                {
                    let mut cache = lock_cache();
                    for (key, modified, palette) in group {
                        cache.entries.insert(key.clone(), Record { modified, palette });
                        batch.insert(key, palette);
                    }
                }
                added = true;
                on_batch(batch);
            }
        });
        if added {
            schedule_save();
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (workers, stop, &mut on_batch, Outcome::NotCached);
    }
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

    fn red_pixels(side: u32) -> Arc<Pixels> {
        Arc::new(Pixels {
            width: side,
            height: side,
            bgra: [0u8, 0, 255, 255].repeat((side * side) as usize),
        })
    }

    #[test]
    fn an_offered_thumbnail_gets_its_palette_from_the_recorder_thread() {
        let path = format!(r"C:\__glint_color_offer_{}\Sample.JPG", std::process::id());
        offer(&path, 777, &red_pixels(8));
        let started = std::time::Instant::now();
        let got = loop {
            if let Some(palette) = palette_of(&path, 777) {
                break palette;
            }
            assert!(started.elapsed().as_secs() < 10, "the recorder never made the palette");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(got.colors[0].a > 0.15, "red is on the a axis: {:?}", got.colors[0]);
        // Not an image: nothing is queued for it.
        offer(r"C:\__glint_color_offer\notes.txt", 1, &red_pixels(8));
        assert!(palette_of(r"C:\__glint_color_offer\notes.txt", 1).is_none());
        lock_cache().entries.remove(&path.to_lowercase());
    }

    #[test]
    fn a_full_queue_drops_offers_instead_of_waiting() {
        let (sender, _receiver) = sync_channel(1);
        let pixels = red_pixels(4);
        assert!(try_offer(&sender, "a.jpg", 1, &pixels));
        let started = std::time::Instant::now();
        assert!(!try_offer(&sender, "b.jpg", 1, &pixels), "the second doesn't fit");
        assert!(started.elapsed().as_millis() < 100, "a full queue is never waited for");
    }

    #[test]
    fn a_batch_records_each_new_photo_once_and_skips_the_known() {
        let dir = format!(r"C:\__glint_color_batch_{}", std::process::id());
        let known = format!(r"{dir}\known.png");
        record_bgra(&known, 5, 4, 4, &red_pixels(4).bgra);
        let before = palette_of(&known, 5).expect("recorded");
        let blue = Arc::new(Pixels {
            width: 4,
            height: 4,
            bgra: [255u8, 0, 0, 255].repeat(16),
        });
        record_offered(vec![
            // Same photo, same date: left as it is, whatever pixels come.
            Offer {
                path: known.clone(),
                modified: 5,
                pixels: blue.clone(),
            },
            Offer {
                path: format!(r"{dir}\New.PNG"),
                modified: 9,
                pixels: blue,
            },
        ]);
        assert_eq!(palette_of(&known, 5), Some(before));
        let new = palette_of(&format!(r"{dir}\new.png"), 9).expect("the new one is recorded under its lowercase path");
        assert!(new.colors[0].b < -0.1, "blue is on the b axis: {:?}", new.colors[0]);
        let mut cache = lock_cache();
        cache.entries.remove(&known.to_lowercase());
        cache.entries.remove(&format!(r"{dir}\new.png"));
    }

    /// A 64 x 64 32-bit BMP of one color per image (Windows makes thumbnails of BMPs, so these are real files to
    /// warm), as bytes.
    #[cfg(windows)]
    fn bmp(n: usize) -> Vec<u8> {
        let side = 64u32;
        let mut out = Vec::with_capacity(54 + (side * side * 4) as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + side * side * 4).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&side.to_le_bytes());
        out.extend_from_slice(&(-(side as i32)).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]);
        let (r, g, b) = ((n * 37 % 256) as u8, (n * 91 % 256) as u8, (n * 53 % 256) as u8);
        for y in 0..side {
            for x in 0..side {
                // A left half in the image's color and a right half in its opposite, so there are two colors.
                let flip = x >= side / 2 || y >= side * 3 / 4;
                out.extend_from_slice(&if flip {
                    [r, g, b, 255]
                } else {
                    [b, 255 - g, 255 - r, 255]
                });
            }
        }
        out
    }

    /// Warming 256 real images with one thread (the old `warm_missing`) and with four, and what a thumbnail pays
    /// for its palette inline (`record_bgra`) against handing it off (`offer`). Prints the numbers.
    /// `cargo test -p glint-core --lib color_index::tests::warming_and_recording_cost_little -- --nocapture`
    #[cfg(windows)]
    #[test]
    fn warming_and_recording_cost_little() {
        let dir = std::env::temp_dir().join(format!("glint-color-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp folder");
        let make = |set: &str, count: usize| -> Vec<(String, i64)> {
            (0..count)
                .map(|n| {
                    let path = dir.join(format!("{set}-{n:03}.bmp"));
                    std::fs::write(&path, bmp(n)).expect("image written");
                    (path.to_string_lossy().into_owned(), 1_000 + n as i64)
                })
                .collect()
        };
        // Windows' thumbnail machinery wakes up on the first few; those aren't what is measured.
        let warmup = make("warm", 8);
        warm_with(&warmup, 1, &AtomicBool::new(false), |_| {});
        let alone = make("one", 256);
        let together = make("four", 256);

        let time = |items: &[(String, i64)], workers: usize| {
            let started = std::time::Instant::now();
            let (mut batches, mut photos) = (0usize, 0usize);
            warm_with(items, workers, &AtomicBool::new(false), |batch| {
                batches += 1;
                photos += batch.len();
            });
            (started.elapsed().as_secs_f64() * 1000.0, batches, photos)
        };
        let (one_ms, one_batches, one_photos) = time(&alone, 1);
        let (four_ms, four_batches, four_photos) = time(&together, WARM_WORKERS);
        println!(
            "warming 256 images: 1 thread {one_ms:.0} ms ({one_photos} palettes, {one_batches} batches), \
             {WARM_WORKERS} threads {four_ms:.0} ms ({four_photos} palettes, {four_batches} batches)"
        );
        // Whatever Windows made of them, the same photos are warmed either way and are in the cache after.
        assert_eq!(one_photos, four_photos);
        if four_photos > 0 {
            assert!(four_batches > 1, "palettes stream in groups, not all at the end");
            assert!(palette_of(&together[0].0, together[0].1).is_some());
        }

        // The thumbnail's side: inline against handed off, over a 256 x 256 thumbnail.
        let pixels = Arc::new(Pixels {
            width: 256,
            height: 256,
            bgra: bmp(3)[54..].to_vec().repeat(16),
        });
        let rounds = 200;
        let started = std::time::Instant::now();
        for n in 0..rounds {
            record_bgra(&format!(r"{}\inline-{n}.jpg", dir.display()), 1, 256, 256, &pixels.bgra);
        }
        let inline_us = started.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let started = std::time::Instant::now();
        for n in 0..rounds {
            offer(&format!(r"{}\handoff-{n}.jpg", dir.display()), 1, &pixels);
        }
        let handoff_us = started.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        println!("a thumbnail's palette: inline {inline_us:.0} us on its thread, handed off {handoff_us:.1} us");

        let _ = std::fs::remove_dir_all(&dir);
        let prefix = dir.to_string_lossy().to_lowercase();
        lock_cache().entries.retain(|key, _| !key.starts_with(&prefix));
    }
}
