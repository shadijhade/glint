//! Drive health for This PC: what Windows and the drives themselves say about each physical disk
//! (health, temperature, wear, errors, hours used), read without admin rights where Windows allows it:
//! the storage stack's own counters and temperature, an NVMe drive's health log, a SATA drive's SMART
//! table and its own failure prediction.


/// One physical disk, and the drive letters on it.
#[derive(Debug, Default, Clone)]
pub struct DriveHealth {
    /// \\.\PhysicalDriveN's N.
    pub disk: u32,
    pub model: String,
    /// nvme, sata, usb, sd, raid, virtual or other.
    pub bus: String,
    /// Drive letters ("C:\") whose volume is on this disk.
    pub letters: Vec<String>,
    /// good, warning or bad: the worst of what's below.
    pub verdict: &'static str,
    /// Why, when it isn't good (plain words).
    pub reasons: Vec<String>,
    pub temperature: Option<i32>,
    /// The temperature the drive itself calls too hot, when it says.
    pub temperature_warning: Option<i32>,
    /// How much of its rated life is used (0–100+), for SSDs.
    pub wear: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub unsafe_shutdowns: Option<u64>,
    /// Errors the drive couldn't correct (NVMe media errors, or SATA's uncorrectable sectors).
    pub media_errors: Option<u64>,
    /// Sectors moved aside because they failed (SATA).
    pub reallocated: Option<u64>,
    /// Sectors waiting to be checked because they failed to read (SATA).
    pub pending: Option<u64>,
    /// NVMe spare blocks left, and the level below which the drive warns.
    pub spare: Option<u32>,
    pub spare_threshold: Option<u32>,
    /// What's been written to it, in bytes (NVMe).
    pub written: Option<u64>,
    /// The drive predicts it will fail (SATA's own judgment, or NVMe's critical warnings).
    pub predicts_failure: bool,
    /// The drive says it's over its temperature limit (NVMe).
    pub too_hot: bool,
    /// Whether Windows could read anything from the drive itself.
    pub readable: bool,
}

pub async fn drive_health() -> Result<Vec<DriveHealth>, String> {
    crate::fsx::blocking(|| Ok(read_all())).await
}

#[cfg(windows)]
fn read_all() -> Vec<DriveHealth> {
    let letters = letters_by_disk();
    let mut out = Vec::new();
    // Disks are numbered from 0 with few gaps; stop after a run of missing ones.
    let mut missing = 0;
    for disk in 0..64u32 {
        match win::read(disk) {
            Some(mut health) => {
                missing = 0;
                health.letters = letters.iter().filter(|(_, d)| *d == disk).map(|(l, _)| l.clone()).collect();
                judge(&mut health);
                out.push(health);
            }
            None => {
                missing += 1;
                if missing > 8 {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn read_all() -> Vec<DriveHealth> {
    Vec::new()
}

/// Sets `verdict` and `reasons` from what was read.
fn judge(h: &mut DriveHealth) {
    let mut bad = Vec::new();
    let mut warn = Vec::new();
    if h.predicts_failure {
        bad.push("The drive predicts it will fail soon".to_string());
    }
    if let (Some(spare), Some(threshold)) = (h.spare, h.spare_threshold) {
        if threshold > 0 && spare < threshold {
            bad.push(format!("Spare blocks are below the safe level ({spare}%)"));
        }
    }
    if let Some(wear) = h.wear {
        if wear >= 100 {
            bad.push(format!("It has used {wear}% of its rated life"));
        } else if wear >= 90 {
            warn.push(format!("It has used {wear}% of its rated life"));
        }
    }
    if let Some(n) = h.media_errors.filter(|&n| n > 0) {
        warn.push(format!("{n} error{} it couldn't correct", if n == 1 { "" } else { "s" }));
    }
    if let Some(n) = h.reallocated.filter(|&n| n > 0) {
        warn.push(format!("{n} sector{} moved aside after failing", if n == 1 { "" } else { "s" }));
    }
    if let Some(n) = h.pending.filter(|&n| n > 0) {
        warn.push(format!("{n} sector{} waiting to be checked after failing to read", if n == 1 { "" } else { "s" }));
    }
    if let Some(t) = h.temperature {
        let limit = h.temperature_warning.filter(|&w| w > 40 && w < 120).unwrap_or(70);
        if t >= limit || h.too_hot {
            warn.push(format!("It's running hot: {t} °C"));
        }
    } else if h.too_hot {
        warn.push("It's running hot".to_string());
    }
    h.verdict = if !bad.is_empty() {
        "bad"
    } else if !warn.is_empty() {
        "warning"
    } else {
        "good"
    };
    bad.extend(warn);
    h.reasons = bad;
}

/// Each local drive letter, and the physical disk its volume starts on.
#[cfg(windows)]
fn letters_by_disk() -> Vec<(String, u32)> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{CreateFileW, GetDriveTypeW, GetLogicalDrives, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::IO::DeviceIoControl;
    // IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: VOLUME_DISK_EXTENTS { NumberOfDiskExtents, (DiskNumber, StartingOffset, ExtentLength)[] }.
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u8 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i) as char;
        let root = HSTRING::from(format!("{letter}:\\"));
        // Fixed and removable drives only: no network shares, discs or RAM disks.
        let kind = unsafe { GetDriveTypeW(&root) };
        if kind != 2 && kind != 3 {
            continue;
        }
        let Ok(handle) = (unsafe { CreateFileW(&HSTRING::from(format!(r"\\.\{letter}:")), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }) else { continue };
        let mut buf = [0u8; 256];
        let mut got = 0u32;
        let ok = unsafe { DeviceIoControl(handle, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, 0, Some(buf.as_mut_ptr().cast()), buf.len() as u32, Some(&mut got), None) }.is_ok();
        let _ = unsafe { CloseHandle(handle) };
        if ok && got >= 32 && u32::from_le_bytes(buf[0..4].try_into().unwrap()) >= 1 {
            // The first extent starts at offset 8 (8-byte aligned): its disk number.
            out.push((format!("{letter}:\\"), u32::from_le_bytes(buf[8..12].try_into().unwrap())));
        }
    }
    out
}

#[cfg(windows)]
mod win {
    use super::DriveHealth;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::IO::DeviceIoControl;

    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const IOCTL_STORAGE_PREDICT_FAILURE: u32 = 0x002D_1100;
    const IOCTL_STORAGE_GET_COUNTERS: u32 = 0x002D_1108;
    const STORAGE_DEVICE_PROPERTY: u32 = 0;
    const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;
    const STORAGE_DEVICE_TEMPERATURE_PROPERTY: u32 = 52;
    const PROTOCOL_TYPE_NVME: u32 = 3;
    const NVME_DATA_TYPE_LOG_PAGE: u32 = 2;
    const NVME_LOG_PAGE_HEALTH_INFO: u32 = 2;

    /// A disk opened for asking questions only (no reading or writing, so no admin rights).
    struct Disk(HANDLE);

    impl Drop for Disk {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    impl Disk {
        fn open(n: u32) -> Option<Disk> {
            unsafe { CreateFileW(&HSTRING::from(format!(r"\\.\PhysicalDrive{n}")), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.ok().map(Disk)
        }

        /// Sends `code` with `input`: what came back, or None.
        fn ask(&self, code: u32, input: &[u8], out_len: usize) -> Option<Vec<u8>> {
            let mut out = vec![0u8; out_len];
            let mut got = 0u32;
            let input_ptr = (!input.is_empty()).then(|| input.as_ptr().cast());
            unsafe { DeviceIoControl(self.0, code, input_ptr, input.len() as u32, Some(out.as_mut_ptr().cast()), out.len() as u32, Some(&mut got), None) }.ok()?;
            out.truncate(got as usize);
            Some(out)
        }

        /// IOCTL_STORAGE_QUERY_PROPERTY for `property` (a standard query), with `extra` after the query.
        fn property(&self, property: u32, extra: &[u8], out_len: usize) -> Option<Vec<u8>> {
            // STORAGE_PROPERTY_QUERY: PropertyId, QueryType (0: standard), AdditionalParameters.
            let mut query = Vec::with_capacity(8 + extra.len().max(4));
            query.extend_from_slice(&property.to_le_bytes());
            query.extend_from_slice(&0u32.to_le_bytes());
            if extra.is_empty() {
                query.extend_from_slice(&[0; 4]);
            } else {
                query.extend_from_slice(extra);
            }
            self.ask(IOCTL_STORAGE_QUERY_PROPERTY, &query, out_len)
        }
    }

    fn u16_at(b: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
    }
    fn u32_at(b: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    }
    fn u64_at(b: &[u8], at: usize) -> Option<u64> {
        Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
    }
    /// A 128-bit NVMe counter, as far as 64 bits go.
    fn u128_at(b: &[u8], at: usize) -> Option<u64> {
        let low = u64_at(b, at)?;
        let high = u64_at(b, at + 8)?;
        Some(if high > 0 { u64::MAX } else { low })
    }
    /// A NUL-terminated string at offset `at` (0 means none).
    fn text_at(b: &[u8], at: usize) -> String {
        if at == 0 || at >= b.len() {
            return String::new();
        }
        let end = b[at..].iter().position(|&c| c == 0).map_or(b.len(), |p| at + p);
        String::from_utf8_lossy(&b[at..end]).trim().to_string()
    }

    pub fn read(n: u32) -> Option<DriveHealth> {
        let disk = Disk::open(n)?;
        let mut h = DriveHealth { disk: n, verdict: "good", ..DriveHealth::default() };

        // STORAGE_DEVICE_DESCRIPTOR: VendorIdOffset at 12, ProductIdOffset at 16, BusType at 28.
        let d = disk.property(STORAGE_DEVICE_PROPERTY, &[], 1024)?;
        let vendor = text_at(&d, u32_at(&d, 12).unwrap_or(0) as usize);
        let product = text_at(&d, u32_at(&d, 16).unwrap_or(0) as usize);
        h.model = if vendor.is_empty() || product.starts_with(&vendor) { product } else { format!("{vendor} {product}") };
        let bus = u32_at(&d, 28).unwrap_or(0);
        h.bus = match bus {
            0x11 => "nvme",
            0x0B | 0x03 => "sata",
            0x07 => "usb",
            0x0C | 0x0D => "sd",
            0x08 => "raid",
            0x0E | 0x0F => "virtual",
            _ => "other",
        }
        .into();

        // The storage stack's own counters (Windows 10 1903 and later).
        if let Some(c) = disk.ask(IOCTL_STORAGE_GET_COUNTERS, &[], 4096) {
            h.readable = true;
            let count = u32_at(&c, 8).unwrap_or(0) as usize;
            for i in 0..count.min(64) {
                // STORAGE_COUNTER: Type (4 bytes, padded to 8), Value (8 bytes).
                let at = 16 + i * 16;
                let (Some(kind), Some(value)) = (u32_at(&c, at), u64_at(&c, at + 8)) else { break };
                match kind {
                    1 if value > 0 && value < 200 => h.temperature = Some(value as i32),
                    5 | 8 if value > 0 => h.media_errors = Some(h.media_errors.unwrap_or(0) + value),
                    10 => h.power_cycles = Some(value),
                    14 if value <= 255 => h.wear = Some(value as u32),
                    17 => h.power_on_hours = Some(value),
                    _ => {}
                }
            }
        }

        // The temperature Windows reads, and the drive's warning level.
        if let Some(t) = disk.property(STORAGE_DEVICE_TEMPERATURE_PROPERTY, &[], 512) {
            if t.len() >= 32 && u16_at(&t, 12).unwrap_or(0) > 0 {
                h.readable = true;
                let warning = u16_at(&t, 10).map(|w| w as i16 as i32).filter(|&w| w > 0);
                h.temperature_warning = warning;
                let now = u16_at(&t, 26).map(|v| v as i16 as i32).filter(|&v| v > 0 && v < 200);
                if now.is_some() {
                    h.temperature = now;
                }
            }
        }

        if h.bus == "nvme" {
            nvme(&disk, &mut h);
        } else {
            sata(&disk, &mut h);
        }
        Some(h)
    }

    /// An NVMe drive's health log (log page 2).
    fn nvme(disk: &Disk, h: &mut DriveHealth) {
        // STORAGE_PROTOCOL_SPECIFIC_DATA (40 bytes), the log asked for right after it.
        let mut spec = Vec::with_capacity(40);
        for v in [PROTOCOL_TYPE_NVME, NVME_DATA_TYPE_LOG_PAGE, NVME_LOG_PAGE_HEALTH_INFO, 0, 40, 512, 0, 0, 0, 0] {
            spec.extend_from_slice(&v.to_le_bytes());
        }
        spec.resize(40 + 512, 0);
        // STORAGE_PROTOCOL_DATA_DESCRIPTOR: Version, Size, then the specific data (40 bytes) and the log.
        let Some(d) = disk.property(STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY, &spec, 8 + 40 + 512) else { return };
        let offset = u32_at(&d, 8 + 16).unwrap_or(40) as usize;
        let Some(log) = d.get(8 + offset..8 + offset + 512) else { return };
        h.readable = true;
        let warning = log[0];
        if let Some(kelvin) = u16_at(log, 1).filter(|&k| k > 273 && k < 473) {
            h.temperature = Some(kelvin as i32 - 273);
        }
        h.spare = Some(log[3] as u32);
        h.spare_threshold = Some(log[4] as u32);
        h.wear = Some(log[5] as u32);
        h.written = u128_at(log, 48).map(|units| units.saturating_mul(512_000));
        h.power_cycles = u128_at(log, 112);
        h.power_on_hours = u128_at(log, 128);
        h.unsafe_shutdowns = u128_at(log, 144);
        h.media_errors = u128_at(log, 160);
        // Critical warnings: reliability degraded (bit 2), read-only (bit 3); over temperature (bit 1).
        if warning & 0b0000_1100 != 0 {
            h.predicts_failure = true;
        }
        h.too_hot = warning & 0b0000_0010 != 0;
    }

    /// A SATA drive's own failure prediction, and its SMART table where Windows hands it out.
    fn sata(disk: &Disk, h: &mut DriveHealth) {
        // STORAGE_PREDICT_FAILURE: PredictFailure, then the 512-byte SMART data.
        let Some(p) = disk.ask(IOCTL_STORAGE_PREDICT_FAILURE, &[], 4 + 512) else { return };
        if p.len() < 4 + 362 {
            return;
        }
        h.readable = true;
        h.predicts_failure = u32_at(&p, 0).unwrap_or(0) != 0;
        let table = &p[4..];
        // 30 attributes of 12 bytes from offset 2: id, flags (2), value, worst, raw (6), reserved.
        let mut life_left: Option<u32> = None;
        for i in 0..30 {
            let a = &table[2 + i * 12..2 + i * 12 + 12];
            let id = a[0];
            if id == 0 {
                continue;
            }
            let value = a[3] as u32;
            let raw = u64::from_le_bytes([a[5], a[6], a[7], a[8], a[9], a[10], 0, 0]);
            match id {
                5 => h.reallocated = Some(raw & 0xFFFF_FFFF),
                9 => h.power_on_hours = Some(raw & 0xFFFF_FFFF),
                12 => h.power_cycles = Some(raw & 0xFFFF_FFFF),
                194 | 190 if h.temperature.is_none() && (raw & 0xFF) > 0 && (raw & 0xFF) < 120 => h.temperature = Some((raw & 0xFF) as i32),
                197 => h.pending = Some(raw & 0xFFFF_FFFF),
                198 => h.media_errors = Some(raw & 0xFFFF_FFFF),
                192 => h.unsafe_shutdowns = Some(raw & 0xFFFF_FFFF),
                // Life left as the normalized value (100 = new) on most SSDs: Samsung 177, 231 SSD Life
                // Left, Intel's 233, Crucial's 202, others' 169.
                177 | 231 | 233 | 202 | 169 if value > 0 && value <= 100 => life_left = Some(life_left.map_or(value, |l: u32| l.min(value))),
                _ => {}
            }
        }
        if h.wear.is_none() {
            h.wear = life_left.map(|left| 100 - left);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn judges_what_it_reads() {
        let mut h = DriveHealth { wear: Some(3), temperature: Some(41), ..DriveHealth::default() };
        judge(&mut h);
        assert_eq!((h.verdict, h.reasons.len()), ("good", 0));
        let mut h = DriveHealth { wear: Some(93), reallocated: Some(8), ..DriveHealth::default() };
        judge(&mut h);
        assert_eq!(h.verdict, "warning");
        assert_eq!(h.reasons.len(), 2);
        let mut h = DriveHealth { spare: Some(4), spare_threshold: Some(10), ..DriveHealth::default() };
        judge(&mut h);
        assert_eq!(h.verdict, "bad");
    }

    /// What this PC's drives say, unelevated: `cargo test --lib drivehealth -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_this_pcs_drives() {
        for h in read_all() {
            eprintln!("{h:#?}");
        }
    }
}
