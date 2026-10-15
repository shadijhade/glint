//! USN change journal records, as FSCTL_READ_USN_JOURNAL returns them, and which file records a batch
//! of them touched. The index doesn't replay what the records say happened: it rereads each touched
//! record as it is now, which gives the same result however often or late a change is seen.

use crate::tree::{split_ref, FIRST_USER_RECORD, ROOT};

pub const REASON_FILE_CREATE: u32 = 0x0000_0100;
pub const REASON_FILE_DELETE: u32 = 0x0000_0200;
pub const REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
pub const REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;
pub const REASON_HARD_LINK_CHANGE: u32 = 0x0001_0000;
pub const REASON_CLOSE: u32 = 0x8000_0000;

/// Changes that add or remove a name in a folder, and so change the folder too.
const NAME_CHANGES: u32 = REASON_FILE_CREATE | REASON_FILE_DELETE | REASON_RENAME_OLD_NAME | REASON_RENAME_NEW_NAME | REASON_HARD_LINK_CHANGE;

#[derive(Debug, Clone, PartialEq)]
pub struct UsnRecord {
    pub usn: i64,
    /// File reference of the file that changed, and of its folder at the time.
    pub file: u64,
    pub parent: u64,
    /// When the change was recorded (FILETIME).
    pub time: i64,
    pub reason: u32,
    pub name: String,
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

/// One USN_RECORD_V2, or a V3 (128-bit file IDs, of which NTFS uses the low 64 bits).
fn parse_record(r: &[u8]) -> Option<UsnRecord> {
    let (file, parent, rest) = match u16_at(r, 4)? {
        2 => (u64_at(r, 8)?, u64_at(r, 16)?, 24),
        3 => (u64_at(r, 8)?, u64_at(r, 24)?, 40),
        _ => return None,
    };
    let (len, offset) = (u16_at(r, rest + 32)? as usize, u16_at(r, rest + 34)? as usize);
    let units: Vec<u16> = r.get(offset..offset + len)?.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    Some(UsnRecord {
        usn: u64_at(r, rest)? as i64,
        time: u64_at(r, rest + 8)? as i64,
        reason: u32_at(r, rest + 16)?,
        file,
        parent,
        name: String::from_utf16_lossy(&units),
    })
}

/// A page of journal output: the USN to continue from, then the records.
pub fn parse_page(page: &[u8]) -> Option<(i64, Vec<UsnRecord>)> {
    let next = u64_at(page, 0)? as i64;
    let mut records = Vec::new();
    let mut at = 8;
    while let Some(len) = u32_at(page, at) {
        let len = len as usize;
        if len < 8 || at + len > page.len() {
            break;
        }
        if let Some(record) = parse_record(&page[at..at + len]) {
            records.push(record);
        }
        at += len;
    }
    Some((next, records))
}

/// File records a batch touched, each once, in the order first seen (so a new folder is indexed
/// before what was created in it): every changed file, and every folder that gained or lost a name.
/// `since`, when given, skips records stamped before it (a FILETIME).
pub fn touched(records: &[UsnRecord], since: Option<i64>) -> Vec<u32> {
    let mut seen = std::collections::HashSet::new();
    let mut touched = Vec::new();
    let mut touch = |reference: u64| {
        let (number, _) = split_ref(reference);
        if number < u32::MAX as u64 {
            let number = number as u32;
            if (number >= FIRST_USER_RECORD || number == ROOT) && seen.insert(number) {
                touched.push(number);
            }
        }
    };
    for record in records.iter().filter(|r| since.is_none_or(|t| r.time >= t)) {
        touch(record.file);
        if record.reason & NAME_CHANGES != 0 {
            touch(record.parent);
        }
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(usn: i64, file: u64, parent: u64, reason: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let len = (60 + 2 * units.len() + 7) & !7;
        let mut r = vec![0u8; len];
        r[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        r[4..6].copy_from_slice(&2u16.to_le_bytes());
        r[8..16].copy_from_slice(&file.to_le_bytes());
        r[16..24].copy_from_slice(&parent.to_le_bytes());
        r[24..32].copy_from_slice(&usn.to_le_bytes());
        r[32..40].copy_from_slice(&(usn * 10).to_le_bytes());
        r[40..44].copy_from_slice(&reason.to_le_bytes());
        r[56..58].copy_from_slice(&((2 * units.len()) as u16).to_le_bytes());
        r[58..60].copy_from_slice(&60u16.to_le_bytes());
        for (i, u) in units.iter().enumerate() {
            r[60 + 2 * i..62 + 2 * i].copy_from_slice(&u.to_le_bytes());
        }
        r
    }

    #[test]
    fn parses_a_page_and_lists_touched_records() {
        let mut page = 9000i64.to_le_bytes().to_vec();
        page.extend(record(100, 200 | 1 << 48, 30 | 1 << 48, REASON_FILE_CREATE, "new folder"));
        page.extend(record(110, 201 | 1 << 48, 200 | 1 << 48, REASON_FILE_CREATE | REASON_CLOSE, "notes.txt"));
        page.extend(record(120, 201 | 1 << 48, 200 | 1 << 48, 0x2, "notes.txt")); // data extended
        page.extend(record(130, 7, 5, 0x2, "$Bitmap")); // NTFS metadata
        page.extend(record(140, 202 | 3 << 48, 31, REASON_RENAME_OLD_NAME, "été.txt"));
        page.extend([0u8; 4]); // trailing garbage shorter than a record
        let (next, records) = parse_page(&page).unwrap();
        assert_eq!(next, 9000);
        assert_eq!(records.len(), 5);
        assert_eq!((records[1].usn, records[1].file, records[1].name.as_str()), (110, 201 | 1 << 48, "notes.txt"));
        assert_eq!(records[4].name, "été.txt");
        assert_eq!(touched(&records, None), [200, 30, 201, 202, 31]);
        assert_eq!(touched(&records, Some(1150)), [201, 202, 31]);
        assert!(parse_page(&[1, 2, 3]).is_none());
    }
}
