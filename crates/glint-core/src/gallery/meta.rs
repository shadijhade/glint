//! What the Gallery and the Photo Map know of each photo and video, kept in one file (`gallery-meta.bin`, with
//! Glint's data): when it was taken, where, and how big it is, each read once from the file's EXIF and kept by
//! path with the size and date the file had, so a changed file is noticed and read again. It replaces
//! `gallery-dates.json` and `gallery-geo.json`, which held the same photos twice and were rewritten whole as JSON.
//!
//! A cold scan reads each file once, four at a time, and hands the records on when it is done.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// The file starts with these, then the version of the layout, then how many records follow.
const MAGIC: &[u8; 6] = b"GLMETA";
/// The layout this build writes and reads. A file of another version is read as empty and written over.
pub const VERSION: u32 = 1;
const HEADER_LEN: usize = MAGIC.len() + 4 + 4;

/// A record's place is missing.
const NO_COORD: i32 = i32::MIN;

/// When it was taken was read (0 in `taken`: it doesn't say).
pub const F_DATE: u8 = 1;
/// Where it was taken was read (a place, or none).
pub const F_GEO: u8 = 2;
/// Its size in pixels was read.
pub const F_DIMS: u8 = 4;

/// What is known of one file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meta {
    /// The file's size and last-write time (ms) when it was read: a record is trusted only while they hold.
    pub size: u64,
    pub modified: i64,
    /// Ms since 1970, as the file says (the camera's clock read as this PC's); 0 when it doesn't say.
    pub taken: i64,
    /// Latitude and longitude in millionths of a degree ([`NO_COORD`] when it has no place).
    pub lat: i32,
    pub lon: i32,
    pub width: u32,
    pub height: u32,
    /// Which of the above were read: [`F_DATE`], [`F_GEO`], [`F_DIMS`].
    pub flags: u8,
}

impl Meta {
    /// Nothing read of a file of this size and date.
    pub fn new(size: u64, modified: i64) -> Self {
        Self {
            size,
            modified,
            taken: 0,
            lat: NO_COORD,
            lon: NO_COORD,
            width: 0,
            height: 0,
            flags: 0,
        }
    }

    /// Where it was taken, as degrees.
    pub fn place(&self) -> Option<[f64; 2]> {
        (self.flags & F_GEO != 0 && self.lat != NO_COORD && self.lon != NO_COORD)
            .then(|| [f64::from(self.lat) / 1e6, f64::from(self.lon) / 1e6])
    }

    /// Keeps where it was taken (a place off the globe, or not finite, is none).
    pub fn set_place(&mut self, place: Option<[f64; 2]>) {
        let valid =
            place.filter(|[lat, lon]| lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0);
        (self.lat, self.lon) = match valid {
            // Within ±180e6, so the casts are exact.
            Some([lat, lon]) => ((lat * 1e6).round() as i32, (lon * 1e6).round() as i32),
            None => (NO_COORD, NO_COORD),
        };
    }

    /// Whether this record still describes a file of this size and date.
    pub fn is_of(&self, size: u64, modified: i64) -> bool {
        self.size == size && self.modified == modified
    }
}

/// Every record, by the file's path in lower case.
pub type Records = HashMap<String, Meta>;

/// The file's bytes: the header, then each record (path length, path, then its numbers), all little-endian.
pub fn encode(records: &Records) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + records.len() * 80);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    let mut written = 0u32;
    for (path, m) in records {
        let Ok(len) = u16::try_from(path.len()) else {
            continue;
        };
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&m.size.to_le_bytes());
        out.extend_from_slice(&m.modified.to_le_bytes());
        out.extend_from_slice(&m.taken.to_le_bytes());
        out.extend_from_slice(&m.lat.to_le_bytes());
        out.extend_from_slice(&m.lon.to_le_bytes());
        out.extend_from_slice(&m.width.to_le_bytes());
        out.extend_from_slice(&m.height.to_le_bytes());
        out.push(m.flags);
        written += 1;
    }
    let at = MAGIC.len() + 4;
    out[at..at + 4].copy_from_slice(&written.to_le_bytes());
    out
}

/// A cursor over bytes that says None at the end instead of panicking.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let part = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(part)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
}

/// The records in a file's bytes. None when it is not this layout (another file, or another version); a file cut
/// short gives the records before the cut, since each is checked against its file when used.
pub fn decode(bytes: &[u8]) -> Option<Records> {
    if bytes.get(..MAGIC.len())? != MAGIC {
        return None;
    }
    let mut r = Reader {
        data: bytes,
        at: MAGIC.len(),
    };
    if u32::from_le_bytes(r.array()?) != VERSION {
        return None;
    }
    let count = u32::from_le_bytes(r.array()?) as usize;
    let mut records = Records::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let Some(record) = (|| {
            let len = usize::from(u16::from_le_bytes(r.array()?));
            let path = std::str::from_utf8(r.take(len)?).ok()?;
            let m = Meta {
                size: u64::from_le_bytes(r.array()?),
                modified: i64::from_le_bytes(r.array()?),
                taken: i64::from_le_bytes(r.array()?),
                lat: i32::from_le_bytes(r.array()?),
                lon: i32::from_le_bytes(r.array()?),
                width: u32::from_le_bytes(r.array()?),
                height: u32::from_le_bytes(r.array()?),
                flags: r.array::<1>()?[0],
            };
            Some((path.to_string(), m))
        })() else {
            break;
        };
        records.insert(record.0, record.1);
    }
    Some(records)
}

/// The cache's file in `dir`.
pub fn file_in(dir: &Path) -> PathBuf {
    dir.join("gallery-meta.bin")
}

fn old_dates_in(dir: &Path) -> PathBuf {
    dir.join("gallery-dates.json")
}

fn old_geo_in(dir: &Path) -> PathBuf {
    dir.join("gallery-geo.json")
}

/// Writes the records so that a reader never sees half a file: to a temp file beside it, then renamed over it.
pub fn save_to(file: &Path, records: &Records) -> std::io::Result<()> {
    let bytes = encode(records);
    let temp = file.with_extension("bin.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// What `gallery-dates.json` held: path, size, modified, taken.
type OldDates = Vec<(String, u64, i64, i64)>;
/// What `gallery-geo.json` held: path, size, modified, taken (the modified date when the file didn't say), place.
type OldGeo = Vec<(String, u64, i64, i64, Option<[f64; 2]>)>;

/// Adds what the two old JSON files held to `records` (what is already there wins). True when either file
/// existed, so the caller writes the new file and then [`remove_old`]s them.
pub fn import_old(dir: &Path, records: &mut Records) -> bool {
    let mut found = false;
    if let Ok(bytes) = std::fs::read(old_dates_in(dir)) {
        found = true;
        for (path, size, modified, taken) in serde_json::from_slice::<OldDates>(&bytes).unwrap_or_default() {
            let m = records.entry(path).or_insert_with(|| Meta::new(size, modified));
            if m.is_of(size, modified) && m.flags & F_DATE == 0 {
                m.taken = taken;
                m.flags |= F_DATE;
            }
        }
    }
    if let Ok(bytes) = std::fs::read(old_geo_in(dir)) {
        found = true;
        for (path, size, modified, taken, place) in serde_json::from_slice::<OldGeo>(&bytes).unwrap_or_default() {
            let m = records.entry(path).or_insert_with(|| Meta::new(size, modified));
            if !m.is_of(size, modified) {
                continue;
            }
            // The old file kept the modified date where a photo had no date taken: that can't be told from a
            // date taken, so such a record's date is read again with the rest.
            if m.flags & F_DATE == 0 && taken != modified {
                m.taken = taken;
                m.flags |= F_DATE;
            }
            if m.flags & F_GEO == 0 {
                m.set_place(place);
                m.flags |= F_GEO;
            }
        }
    }
    found
}

/// Deletes the old JSON files, once the new file is safely written.
pub fn remove_old(dir: &Path) {
    let _ = std::fs::remove_file(old_dates_in(dir));
    let _ = std::fs::remove_file(old_geo_in(dir));
}

/// The records in `dir`'s cache file, with the old JSON files' folded in. The second value is true when old files
/// were found: the caller should save soon so they can go.
pub fn load(dir: &Path) -> (Records, bool) {
    let mut records = std::fs::read(file_in(dir))
        .ok()
        .and_then(|bytes| decode(&bytes))
        .unwrap_or_default();
    let imported = import_old(dir, &mut records);
    (records, imported)
}

// ---- Saving: soon after the last change, not on every one --------------------------------------------

/// How long after a change the cache is written (more changes in that time go into the same write).
const DEBOUNCE: Duration = Duration::from_millis(1500);

static SAVE_WANTED: AtomicBool = AtomicBool::new(false);
static SAVER_RUNNING: AtomicBool = AtomicBool::new(false);
/// What the saver takes its records from.
type Snapshot = Box<dyn Fn() -> Option<Records> + Send>;

static SNAPSHOT: LazyLock<Mutex<Option<Snapshot>>> = LazyLock::new(|| Mutex::new(None));

/// Asks for the cache to be written soon: `snapshot` gives the records then (the lock they live under is held
/// only while they are copied). A burst of changes makes one write; the write is on a thread of its own.
pub fn schedule_save(dir: Option<PathBuf>, snapshot: impl Fn() -> Option<Records> + Send + 'static) {
    if cfg!(test) {
        return;
    }
    let Some(dir) = dir else {
        return;
    };
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(snapshot));
    SAVE_WANTED.store(true, Ordering::SeqCst);
    if SAVER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(DEBOUNCE);
        SAVE_WANTED.store(false, Ordering::SeqCst);
        let records = SNAPSHOT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|s| s());
        if let Some(records) = records {
            let _ = std::fs::create_dir_all(&dir);
            if save_to(&file_in(&dir), &records).is_ok() {
                remove_old(&dir);
            }
        }
        if !SAVE_WANTED.load(Ordering::SeqCst) {
            SAVER_RUNNING.store(false, Ordering::SeqCst);
            // A change that came between the check and the store would otherwise wait for the next one.
            if SAVE_WANTED.load(Ordering::SeqCst) && !SAVER_RUNNING.swap(true, Ordering::SeqCst) {
                continue;
            }
            break;
        }
    });
}

// ---- Reading one file --------------------------------------------------------------------------------

/// A file to read.
pub struct Job {
    pub path: String,
    pub size: u64,
    pub modified: i64,
}

/// The record for one file: its EXIF (or a video's track metadata) read once.
fn read_one(job: &Job) -> Meta {
    let mut meta = Meta::new(job.size, job.modified);
    meta.flags = F_DATE | F_GEO;
    if let Some(d) = crate::media::details_now(&job.path) {
        meta.taken = d.taken.map_or(0, |ms| (ms as i64).max(0));
        meta.set_place(d.place);
        if let (Some(w), Some(h)) = (d.width, d.height) {
            meta.width = w;
            meta.height = h;
            meta.flags |= F_DIMS;
        }
    }
    meta
}

/// Threads reading files.
const READERS: usize = 4;

/// Reads every job, four at a time, and hands on `(index into jobs, record)` rows when done.
pub fn scan(jobs: &[Job], flush: &(dyn Fn(Vec<(usize, Meta)>) + Sync)) {
    if jobs.is_empty() {
        return;
    }
    let next = AtomicUsize::new(0);
    let rows = Mutex::new(Vec::with_capacity(jobs.len()));
    std::thread::scope(|scope| {
        for _ in 0..READERS {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else {
                    break;
                };
                let meta = read_one(job);
                rows.lock().unwrap_or_else(|e| e.into_inner()).push((i, meta));
            });
        }
    });
    flush(rows.into_inner().unwrap_or_else(|e| e.into_inner()));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A JPEG of `pad` bytes with an EXIF header: its date taken, its place (when given), and its size.
    pub(crate) fn test_jpeg(date: &str, place: Option<(f64, f64)>, size: (u32, u32), pad: usize) -> Vec<u8> {
        fn entry(buf: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]) {
            buf.extend(tag.to_le_bytes());
            buf.extend(kind.to_le_bytes());
            buf.extend(count.to_le_bytes());
            buf.extend(value);
        }
        fn dms(deg: f64) -> Vec<u8> {
            let deg = deg.abs();
            let d = deg.floor();
            let m = ((deg - d) * 60.0).floor();
            let s = ((deg - d) * 60.0 - m) * 60.0;
            let mut out = Vec::new();
            for (n, den) in [(d as u32, 1u32), (m as u32, 1), ((s * 10_000.0).round() as u32, 10_000)] {
                out.extend(n.to_le_bytes());
                out.extend(den.to_le_bytes());
            }
            out
        }
        let gps = place.is_some();
        let ifd0 = 8usize;
        let ifd0_len = 2 + 12 * (2 + usize::from(gps)) + 4;
        let exif_at = ifd0 + ifd0_len;
        let date_at = exif_at + 2 + 36 + 4;
        let gps_at = date_at + 20;
        let lat_at = gps_at + 2 + 48 + 4;
        let lon_at = lat_at + 24;
        let mut t: Vec<u8> = vec![b'I', b'I', 0x2A, 0, 8, 0, 0, 0];
        t.extend((2 + u16::from(gps)).to_le_bytes());
        entry(&mut t, 0x0112, 3, 1, [1, 0, 0, 0]);
        entry(&mut t, 0x8769, 4, 1, (exif_at as u32).to_le_bytes());
        if gps {
            entry(&mut t, 0x8825, 4, 1, (gps_at as u32).to_le_bytes());
        }
        t.extend([0; 4]);
        t.extend(3u16.to_le_bytes());
        entry(&mut t, 0x9003, 2, 20, (date_at as u32).to_le_bytes());
        entry(&mut t, 0xA002, 4, 1, size.0.to_le_bytes());
        entry(&mut t, 0xA003, 4, 1, size.1.to_le_bytes());
        t.extend([0; 4]);
        t.extend(date.as_bytes());
        t.push(0);
        if let Some((lat, lon)) = place {
            t.extend(4u16.to_le_bytes());
            entry(&mut t, 1, 2, 2, [if lat >= 0.0 { b'N' } else { b'S' }, 0, 0, 0]);
            entry(&mut t, 2, 5, 3, (lat_at as u32).to_le_bytes());
            entry(&mut t, 3, 2, 2, [if lon >= 0.0 { b'E' } else { b'W' }, 0, 0, 0]);
            entry(&mut t, 4, 5, 3, (lon_at as u32).to_le_bytes());
            t.extend([0; 4]);
            t.extend(dms(lat));
            t.extend(dms(lon));
        }
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((2 + 6 + t.len()) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(t);
        jpeg.extend([0xFF, 0xC0, 0, 17, 8]);
        jpeg.extend((size.1 as u16).to_be_bytes());
        jpeg.extend((size.0 as u16).to_be_bytes());
        jpeg.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        jpeg.extend([0xFF, 0xDA, 0, 12, 3, 1, 0, 2, 0x11, 3, 0x11, 0, 0x3F, 0]);
        jpeg.extend((0..pad).map(|i| (i % 200) as u8));
        jpeg.extend([0xFF, 0xD9]);
        jpeg
    }

    fn sample() -> Records {
        let mut records = Records::new();
        let mut a = Meta::new(2048, 1_700_000_000_000);
        a.taken = 1_699_000_000_000;
        a.set_place(Some([35.0116, 135.7681]));
        a.width = 4032;
        a.height = 3024;
        a.flags = F_DATE | F_GEO | F_DIMS;
        records.insert(r"c:\pics\kyoto.jpg".into(), a);
        let mut b = Meta::new(10, 5);
        b.flags = F_DATE | F_GEO;
        records.insert(r"c:\صور\رحلة.jpg".into(), b);
        records
    }

    #[test]
    fn the_file_round_trips_and_a_place_survives_to_the_millionth_of_a_degree() {
        let records = sample();
        let back = decode(&encode(&records)).expect("a file of this layout");
        assert_eq!(back, records);
        let [lat, lon] = back[r"c:\pics\kyoto.jpg"].place().expect("a place");
        assert!((lat - 35.0116).abs() < 1e-6 && (lon - 135.7681).abs() < 1e-6);
        assert_eq!(back[r"c:\صور\رحلة.jpg"].place(), None);
    }

    #[test]
    fn a_damaged_file_never_panics_and_gives_what_it_can() {
        let bytes = encode(&sample());
        // Cut at every length: either not this layout, or the records before the cut.
        for cut in 0..bytes.len() {
            if let Some(records) = decode(&bytes[..cut]) {
                assert!(records.len() <= 2);
            }
        }
        // Another version, or another file altogether, is no cache.
        let mut newer = bytes.clone();
        newer[MAGIC.len()] = 99;
        assert!(decode(&newer).is_none());
        assert!(decode(b"[{\"json\":true}]").is_none());
        // Noise after a valid header.
        let mut noisy = bytes[..HEADER_LEN].to_vec();
        noisy.extend((0..500u32).map(|i| (i * 7) as u8));
        let _ = decode(&noisy);
    }

    #[test]
    fn the_old_json_files_are_folded_in_once_and_go_after_a_write() {
        let dir = std::env::temp_dir().join(format!("glint-meta-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dates: OldDates = vec![(r"c:\a.jpg".into(), 10, 100, 555), (r"c:\b.jpg".into(), 20, 200, 0)];
        let geo: OldGeo = vec![
            (r"c:\a.jpg".into(), 10, 100, 555, Some([1.5, 2.5])),
            (r"c:\c.jpg".into(), 30, 300, 300, Some([10.0, 20.0])),
            (r"c:\d.jpg".into(), 40, 400, 123, None),
        ];
        std::fs::write(old_dates_in(&dir), serde_json::to_vec(&dates).unwrap()).unwrap();
        std::fs::write(old_geo_in(&dir), serde_json::to_vec(&geo).unwrap()).unwrap();

        let (records, imported) = load(&dir);
        assert!(imported);
        let a = records[r"c:\a.jpg"];
        assert_eq!(
            (a.taken, a.place(), a.flags & (F_DATE | F_GEO)),
            (555, Some([1.5, 2.5]), F_DATE | F_GEO)
        );
        // Dated, but no place read yet.
        assert_eq!(records[r"c:\b.jpg"].flags & (F_DATE | F_GEO), F_DATE);
        // Its date taken is the modified date: that is read again.
        assert_eq!(records[r"c:\c.jpg"].flags & (F_DATE | F_GEO), F_GEO);
        assert_eq!(records[r"c:\d.jpg"].taken, 123);

        save_to(&file_in(&dir), &records).unwrap();
        remove_old(&dir);
        assert!(!old_dates_in(&dir).exists() && !old_geo_in(&dir).exists());
        let (again, imported) = load(&dir);
        assert!(!imported);
        assert_eq!(again, records);
        assert!(!dir.join("gallery-meta.bin.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scan_reads_every_file_once() {
        let dir = std::env::temp_dir().join(format!("glint-meta-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut jobs = Vec::new();
        for i in 0..300u32 {
            let place = (i % 3 != 0).then_some((10.0 + f64::from(i) * 0.1, 20.0));
            let bytes = test_jpeg(
                &format!("2020:01:{:02} 10:00:00", i % 28 + 1),
                place,
                (640 + i, 480),
                2_000,
            );
            let path = dir.join(format!("p{i}.jpg"));
            std::fs::write(&path, bytes).unwrap();
            jobs.push(Job {
                path: path.to_string_lossy().into_owned(),
                size: 0,
                modified: i64::from(i),
            });
        }
        jobs.push(Job {
            path: dir.join("missing.jpg").to_string_lossy().into_owned(),
            size: 0,
            modified: 0,
        });
        jobs.push(Job {
            path: dir.join("notes.txt").to_string_lossy().into_owned(),
            size: 0,
            modified: 0,
        });
        std::fs::write(dir.join("notes.txt"), "plain").unwrap();
        let rows = Mutex::new(Vec::new());
        scan(&jobs, &|batch| rows.lock().unwrap().extend(batch));
        let mut rows = rows.into_inner().unwrap();
        rows.sort_by_key(|r| r.0);
        assert_eq!(rows.len(), jobs.len());
        for (i, meta) in &rows[..300] {
            assert_eq!(meta.modified, *i as i64);
            assert!(meta.taken > 0, "file {i} has a date");
            assert_eq!(meta.place().is_some(), i % 3 != 0, "file {i}");
            assert_eq!((meta.width, meta.height), (640 + *i as u32, 480), "file {i}");
            assert_eq!(meta.flags, F_DATE | F_GEO | F_DIMS);
        }
        // Missing and plain files are answered (nothing found), not left out.
        assert_eq!(rows[300].1.taken, 0);
        assert_eq!(rows[301].1.place(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Saving and loading 200,000 records: the old two JSON files (dates, and places) against the one binary
    /// file, written and read from a real folder. Printed with `--nocapture`; the numbers are in the commit.
    #[test]
    fn saving_and_loading_cost_little() {
        use std::time::Instant;
        let n = 200_000usize;
        let mut records = Records::with_capacity(n);
        for i in 0..n {
            let path = format!(r"c:\users\someone\pictures\2023\trip {}\img_{i:06}.jpg", i % 400);
            let mut m = Meta::new(2_000_000 + i as u64, 1_700_000_000_000 + i as i64 * 1_000);
            m.taken = m.modified - 86_400_000;
            if i % 3 != 0 {
                m.set_place(Some([
                    35.0 + (i % 1000) as f64 * 0.001,
                    139.0 + (i % 777) as f64 * 0.001,
                ]));
            }
            (m.width, m.height) = (4000, 3000);
            m.flags = F_DATE | F_GEO | F_DIMS;
            records.insert(path, m);
        }
        let dir = std::env::temp_dir().join(format!("glint-meta-cost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Before: two JSON files, each written whole.
        let dates: OldDates = records
            .iter()
            .map(|(p, m)| (p.clone(), m.size, m.modified, m.taken))
            .collect();
        let geo: OldGeo = records
            .iter()
            .map(|(p, m)| (p.clone(), m.size, m.modified, m.taken, m.place()))
            .collect();
        let t = Instant::now();
        std::fs::write(old_dates_in(&dir), serde_json::to_vec(&dates).unwrap()).unwrap();
        std::fs::write(old_geo_in(&dir), serde_json::to_vec(&geo).unwrap()).unwrap();
        let json_save = t.elapsed();
        let t = Instant::now();
        let a: OldDates = serde_json::from_slice(&std::fs::read(old_dates_in(&dir)).unwrap()).unwrap();
        let b: OldGeo = serde_json::from_slice(&std::fs::read(old_geo_in(&dir)).unwrap()).unwrap();
        let json_load = t.elapsed();
        assert_eq!((a.len(), b.len()), (n, n));
        let json_bytes =
            std::fs::metadata(old_dates_in(&dir)).unwrap().len() + std::fs::metadata(old_geo_in(&dir)).unwrap().len();

        // After: one binary file, written beside itself and renamed.
        let t = Instant::now();
        save_to(&file_in(&dir), &records).unwrap();
        let bin_save = t.elapsed();
        let t = Instant::now();
        let back = decode(&std::fs::read(file_in(&dir)).unwrap()).unwrap();
        let bin_load = t.elapsed();
        assert_eq!(back.len(), n);
        let bin_bytes = std::fs::metadata(file_in(&dir)).unwrap().len();
        println!(
            "{n} records. JSON: save {:.0} ms, load {:.0} ms, {:.1} MB in two files. Binary: save {:.0} ms, load {:.0} ms, {:.1} MB in one.",
            json_save.as_secs_f64() * 1e3,
            json_load.as_secs_f64() * 1e3,
            json_bytes as f64 / 1e6,
            bin_save.as_secs_f64() * 1e3,
            bin_load.as_secs_f64() * 1e3,
            bin_bytes as f64 / 1e6,
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
