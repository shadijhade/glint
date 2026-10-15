//! NTFS file records ("FILE" records of the Master File Table) parsed from raw bytes: what the index
//! needs from each (names and their folders, size, last write time, attributes), plus the boot sector
//! and data runs that say where the $MFT itself lies.
//!
//! Layouts follow the NTFS on-disk format as documented by the Linux NTFS project (layout.h) and
//! Microsoft's FILE_RECORD_SEGMENT_HEADER / ATTRIBUTE_RECORD_HEADER. Every offset is bounds-checked:
//! a malformed record is skipped, never trusted.

pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

const RECORD_IN_USE: u16 = 0x1;
const RECORD_IS_DIRECTORY: u16 = 0x2;

/// $FILE_NAME namespaces: a DOS name is only an 8.3 alias of another name.
pub const NAMESPACE_POSIX: u8 = 0;
pub const NAMESPACE_WIN32: u8 = 1;
pub const NAMESPACE_DOS: u8 = 2;
pub const NAMESPACE_WIN32_AND_DOS: u8 = 3;

/// Bits $STANDARD_INFORMATION keeps that Windows never reports as file attributes: 0x40000 marks
/// extended attributes (system DLLs have them; in directory listings the same value means "recall
/// on open"), and 0x10000000 and 0x20000000 mark the presence of indexes.
const INTERNAL_ATTRIBUTES: u32 = 0x0004_0000 | 0x1000_0000 | 0x2000_0000;

/// Update sequence arrays protect every 512 bytes of a record, whatever the sector size.
const FIXUP_STRIDE: usize = 512;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A volume's layout, from its boot sector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub bytes_per_sector: u32,
    pub bytes_per_cluster: u32,
    pub record_size: u32,
    /// First cluster of the $MFT.
    pub mft_lcn: u64,
    pub total_sectors: u64,
}

pub fn parse_boot_sector(b: &[u8]) -> Result<Geometry, String> {
    if b.get(3..11) != Some(b"NTFS    ".as_slice()) {
        return Err("not an NTFS boot sector".into());
    }
    let bytes_per_sector = u16_at(b, 0x0B).unwrap_or(0) as u32;
    // Above 0x80 the count is a negative power of two (clusters past 64 KiB).
    let spc = b[0x0D] as u32;
    let sectors_per_cluster = if spc <= 0x80 { spc } else { 1u32.checked_shl(256 - spc).unwrap_or(0) };
    let bytes_per_cluster = bytes_per_sector * sectors_per_cluster;
    let clusters_per_record = b[0x40] as i8;
    let record_size = if clusters_per_record > 0 {
        bytes_per_cluster * clusters_per_record as u32
    } else {
        1u32.checked_shl(clusters_per_record.unsigned_abs() as u32).unwrap_or(0)
    };
    let geometry = Geometry {
        bytes_per_sector,
        bytes_per_cluster,
        record_size,
        mft_lcn: u64_at(b, 0x30).unwrap_or(0),
        total_sectors: u64_at(b, 0x28).unwrap_or(0),
    };
    let sane = [512, 1024, 2048, 4096].contains(&bytes_per_sector)
        && bytes_per_cluster.is_power_of_two()
        && bytes_per_cluster <= 2 << 20
        && record_size.is_power_of_two()
        && (1024..=65536).contains(&record_size)
        && geometry.mft_lcn > 0;
    if sane {
        Ok(geometry)
    } else {
        Err(format!("unexpected NTFS geometry {geometry:?}"))
    }
}

/// A run of clusters: `lcn` is None for a sparse run (no clusters on disk).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    pub lcn: Option<u64>,
    pub clusters: u64,
}

/// Decodes a non-resident attribute's mapping pairs.
pub fn parse_runs(mapping: &[u8]) -> Result<Vec<Run>, String> {
    let mut runs = Vec::new();
    let mut lcn: i64 = 0;
    let mut at = 0;
    while let Some(&header) = mapping.get(at) {
        if header == 0 {
            return Ok(runs);
        }
        let (len_size, off_size) = ((header & 0xF) as usize, (header >> 4) as usize);
        if !(1..=8).contains(&len_size) || off_size > 8 || at + 1 + len_size + off_size > mapping.len() {
            return Err("malformed data run".into());
        }
        let mut field = |size: usize, signed: bool| -> i64 {
            let bytes = &mapping[at + 1..at + 1 + size];
            let mut value: i64 = 0;
            for (i, &b) in bytes.iter().enumerate() {
                value |= (b as i64) << (8 * i);
            }
            if signed && size < 8 && bytes[size - 1] & 0x80 != 0 {
                value -= 1i64 << (8 * size);
            }
            at += size;
            value
        };
        let clusters = field(len_size, false);
        let run = if off_size == 0 {
            Run { lcn: None, clusters: clusters as u64 }
        } else {
            lcn += field(off_size, true);
            if lcn < 0 {
                return Err("data run before the volume start".into());
            }
            Run { lcn: Some(lcn as u64), clusters: clusters as u64 }
        };
        if clusters <= 0 {
            return Err("empty data run".into());
        }
        runs.push(run);
        at += 1;
    }
    Err("unterminated data runs".into())
}

/// Undoes the update sequence protection (the last two bytes of every 512 were swapped out for a
/// check value when the record was written). With `strict`, a sector whose check value is missing
/// means a torn or corrupt record and fails the whole record; otherwise such sectors are taken as
/// already fixed up, which is what records read through the file system can look like.
pub fn apply_fixups(record: &mut [u8], strict: bool) -> bool {
    let (Some(offset), Some(count)) = (u16_at(record, 4), u16_at(record, 6)) else { return false };
    let (offset, count) = (offset as usize, count as usize);
    if count < 2 || offset + 2 * count > record.len() || (count - 1) * FIXUP_STRIDE > record.len() {
        return false;
    }
    let check = [record[offset], record[offset + 1]];
    for i in 1..count {
        let end = i * FIXUP_STRIDE - 2;
        if record[end..end + 2] == check {
            let saved = offset + 2 * i;
            record.copy_within(saved..saved + 2, end);
        } else if strict {
            return false;
        }
    }
    true
}

/// One attribute's header and where its value lies.
#[derive(Debug, Clone, Copy)]
pub struct Attribute<'a> {
    pub kind: u32,
    /// Length of the attribute's own name in UTF-16 units (0 for the default $DATA stream).
    pub name_len: u8,
    pub body: Body<'a>,
}

#[derive(Debug, Clone, Copy)]
pub enum Body<'a> {
    Resident(&'a [u8]),
    NonResident { lowest_vcn: u64, data_size: u64, runs: &'a [u8] },
}

/// The fixed part of a file record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    pub seq: u16,
    pub flags: u16,
    /// Reference to the base record when this is an extension record, else 0.
    pub base: u64,
    attrs_offset: usize,
    used: usize,
}

impl Header {
    pub fn in_use(&self) -> bool {
        self.flags & RECORD_IN_USE != 0
    }
    pub fn dir(&self) -> bool {
        self.flags & RECORD_IS_DIRECTORY != 0
    }
}

/// The header of a record (fixups already applied), if it is one.
pub fn header(record: &[u8]) -> Option<Header> {
    if record.get(0..4)? != b"FILE" {
        return None;
    }
    let header = Header {
        seq: u16_at(record, 0x10)?,
        flags: u16_at(record, 0x16)?,
        base: u64_at(record, 0x20)?,
        attrs_offset: u16_at(record, 0x14)? as usize,
        used: (u32_at(record, 0x18)? as usize).min(record.len()),
    };
    (header.attrs_offset >= 0x30 && header.attrs_offset < header.used).then_some(header)
}

/// The record's attributes in order, stopping at the end marker or anything out of bounds.
pub fn attributes<'a>(record: &'a [u8], header: &Header) -> impl Iterator<Item = Attribute<'a>> + 'a {
    let used = header.used;
    let mut at = header.attrs_offset;
    std::iter::from_fn(move || {
        let kind = u32_at(record, at)?;
        let len = u32_at(record, at + 4)? as usize;
        if kind == ATTR_END || len < 0x18 || at + len > used {
            return None;
        }
        let attr = &record[at..at + len];
        at += len;
        let name_len = attr[9];
        let body = if attr[8] == 0 {
            let size = u32_at(attr, 0x10)? as usize;
            let offset = u16_at(attr, 0x14)? as usize;
            Body::Resident(attr.get(offset..offset.checked_add(size)?)?)
        } else {
            let runs = u16_at(attr, 0x20)? as usize;
            Body::NonResident { lowest_vcn: u64_at(attr, 0x10)?, data_size: u64_at(attr, 0x30)?, runs: attr.get(runs..)? }
        };
        Some(Attribute { kind, name_len, body })
    })
}

/// One $FILE_NAME: a name of the file in folder `parent` (a file reference).
#[derive(Debug, Clone, PartialEq)]
pub struct FileName {
    pub parent: u64,
    pub namespace: u8,
    pub name: String,
}

pub fn file_name(value: &[u8]) -> Option<FileName> {
    let len = *value.get(0x40)? as usize;
    let units: Vec<u16> = value.get(0x42..0x42 + 2 * len)?.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    Some(FileName { parent: u64_at(value, 0)?, namespace: value[0x41], name: String::from_utf16_lossy(&units) })
}

/// Where a non-resident attribute list's bytes are, when they don't fit in the base record.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeList {
    Resident(Vec<u8>),
    NonResident { runs: Vec<Run>, size: u64 },
}

/// What the index takes from one record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    pub seq: u16,
    pub in_use: bool,
    pub dir: bool,
    /// Base record reference for an extension record, else 0.
    pub base: u64,
    /// From $STANDARD_INFORMATION: last write time and attributes (base records only).
    pub modified: Option<i64>,
    pub attrs: u32,
    /// Names, DOS 8.3 aliases left out.
    pub names: Vec<FileName>,
    /// Size of the unnamed data stream, from its first extent.
    pub size: Option<u64>,
    pub attribute_list: Option<AttributeList>,
}

/// Parses one record in place (fixups are applied to `record`). None when it isn't a valid file record.
pub fn parse(record: &mut [u8], strict: bool) -> Option<Parsed> {
    if record.get(0..4)? != b"FILE" || !apply_fixups(record, strict) {
        return None;
    }
    let header = header(record)?;
    let mut parsed = Parsed { seq: header.seq, in_use: header.in_use(), dir: header.dir(), base: header.base, ..Default::default() };
    if !parsed.in_use {
        return Some(parsed);
    }
    for attr in attributes(record, &header) {
        match (attr.kind, attr.body) {
            (ATTR_STANDARD_INFORMATION, Body::Resident(value)) => {
                parsed.modified = u64_at(value, 0x08).map(|t| t as i64);
                parsed.attrs = u32_at(value, 0x20).unwrap_or(0) & !INTERNAL_ATTRIBUTES;
            }
            (ATTR_FILE_NAME, Body::Resident(value)) => {
                if let Some(name) = file_name(value).filter(|n| n.namespace != NAMESPACE_DOS) {
                    parsed.names.push(name);
                }
            }
            (ATTR_DATA, body) if attr.name_len == 0 => match body {
                Body::Resident(value) => parsed.size = Some(value.len() as u64),
                Body::NonResident { lowest_vcn: 0, data_size, .. } => parsed.size = Some(data_size),
                Body::NonResident { .. } => {}
            },
            (ATTR_ATTRIBUTE_LIST, Body::Resident(value)) => parsed.attribute_list = Some(AttributeList::Resident(value.to_vec())),
            (ATTR_ATTRIBUTE_LIST, Body::NonResident { data_size, runs, .. }) => {
                parsed.attribute_list = parse_runs(runs).ok().map(|runs| AttributeList::NonResident { runs, size: data_size });
            }
            _ => {}
        }
    }
    Some(parsed)
}

/// The records an attribute list points to for names and data (the base record's own number excluded).
pub fn extension_records(list: &[u8], base: u64) -> Vec<u64> {
    let mut records = Vec::new();
    let mut at = 0;
    while let (Some(kind), Some(len)) = (u32_at(list, at), u16_at(list, at + 4)) {
        if len < 0x1A {
            break;
        }
        if let Some(reference) = u64_at(list, at + 0x10) {
            let number = reference & 0xFFFF_FFFF_FFFF;
            if (kind == ATTR_FILE_NAME || kind == ATTR_DATA) && number != base && !records.contains(&number) {
                records.push(number);
            }
        }
        at += len as usize;
    }
    records
}

/// The unnamed $DATA extents of a record as (first VCN, data size when it's the first extent, runs):
/// how the $MFT finds itself.
pub fn data_extents(record: &[u8]) -> Vec<(u64, Option<u64>, Vec<Run>)> {
    let Some(header) = header(record) else { return Vec::new() };
    attributes(record, &header)
        .filter(|a| a.kind == ATTR_DATA && a.name_len == 0)
        .filter_map(|a| match a.body {
            Body::NonResident { lowest_vcn, data_size, runs } => {
                Some((lowest_vcn, (lowest_vcn == 0).then_some(data_size), parse_runs(runs).ok()?))
            }
            Body::Resident(_) => None,
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds file records the way NTFS writes them, update sequence included.
    pub struct RecordBuilder {
        bytes: Vec<u8>,
        at: usize,
    }

    impl RecordBuilder {
        pub fn new(size: usize, seq: u16, flags: u16, base: u64) -> Self {
            let mut bytes = vec![0u8; size];
            bytes[0..4].copy_from_slice(b"FILE");
            let count = size / FIXUP_STRIDE + 1;
            bytes[4..6].copy_from_slice(&0x30u16.to_le_bytes());
            bytes[6..8].copy_from_slice(&(count as u16).to_le_bytes());
            bytes[0x10..0x12].copy_from_slice(&seq.to_le_bytes());
            bytes[0x16..0x18].copy_from_slice(&flags.to_le_bytes());
            bytes[0x1C..0x20].copy_from_slice(&(size as u32).to_le_bytes());
            bytes[0x20..0x28].copy_from_slice(&base.to_le_bytes());
            let attrs = (0x30 + 2 * count + 7) & !7;
            bytes[0x14..0x16].copy_from_slice(&(attrs as u16).to_le_bytes());
            RecordBuilder { bytes, at: attrs }
        }

        fn attribute(&mut self, kind: u32, name: &[u16], resident: Option<&[u8]>, non_resident: Option<(u64, u64, &[u8])>) -> &mut Self {
            let header = if resident.is_some() { 0x18 } else { 0x40 };
            let name_at = header;
            let value_at = (name_at + 2 * name.len() + 7) & !7;
            let body_len = resident.map_or_else(|| non_resident.unwrap().2.len(), |v| v.len());
            let len = (value_at + body_len + 7) & !7;
            let a = &mut self.bytes[self.at..self.at + len];
            a[0..4].copy_from_slice(&kind.to_le_bytes());
            a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
            a[9] = name.len() as u8;
            a[10..12].copy_from_slice(&(name_at as u16).to_le_bytes());
            for (i, u) in name.iter().enumerate() {
                a[name_at + 2 * i..name_at + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
            }
            match (resident, non_resident) {
                (Some(value), _) => {
                    a[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
                    a[0x14..0x16].copy_from_slice(&(value_at as u16).to_le_bytes());
                    a[value_at..value_at + value.len()].copy_from_slice(value);
                }
                (None, Some((lowest_vcn, data_size, runs))) => {
                    a[8] = 1;
                    a[0x10..0x18].copy_from_slice(&lowest_vcn.to_le_bytes());
                    a[0x20..0x22].copy_from_slice(&(value_at as u16).to_le_bytes());
                    a[0x30..0x38].copy_from_slice(&data_size.to_le_bytes());
                    a[value_at..value_at + runs.len()].copy_from_slice(runs);
                }
                _ => unreachable!(),
            }
            self.at += len;
            self
        }

        pub fn std_info(&mut self, modified: i64, attrs: u32) -> &mut Self {
            let mut value = [0u8; 0x48];
            value[0x08..0x10].copy_from_slice(&modified.to_le_bytes());
            value[0x20..0x24].copy_from_slice(&attrs.to_le_bytes());
            self.attribute(ATTR_STANDARD_INFORMATION, &[], Some(&value), None)
        }

        pub fn file_name(&mut self, parent: u64, namespace: u8, name: &str) -> &mut Self {
            let units: Vec<u16> = name.encode_utf16().collect();
            let mut value = vec![0u8; 0x42 + 2 * units.len()];
            value[0..8].copy_from_slice(&parent.to_le_bytes());
            value[0x40] = units.len() as u8;
            value[0x41] = namespace;
            for (i, u) in units.iter().enumerate() {
                value[0x42 + 2 * i..0x44 + 2 * i].copy_from_slice(&u.to_le_bytes());
            }
            self.attribute(ATTR_FILE_NAME, &[], Some(&value), None)
        }

        pub fn data(&mut self, value: &[u8]) -> &mut Self {
            self.attribute(ATTR_DATA, &[], Some(value), None)
        }

        pub fn data_runs(&mut self, stream: &str, lowest_vcn: u64, size: u64, runs: &[u8]) -> &mut Self {
            let name: Vec<u16> = stream.encode_utf16().collect();
            self.attribute(ATTR_DATA, &name, None, Some((lowest_vcn, size, runs)))
        }

        pub fn attribute_list(&mut self, entries: &[(u32, u64)]) -> &mut Self {
            let mut value = Vec::new();
            for &(kind, reference) in entries {
                let mut entry = [0u8; 0x20];
                entry[0..4].copy_from_slice(&kind.to_le_bytes());
                entry[4..6].copy_from_slice(&0x20u16.to_le_bytes());
                entry[0x10..0x18].copy_from_slice(&reference.to_le_bytes());
                value.extend_from_slice(&entry);
            }
            self.attribute(ATTR_ATTRIBUTE_LIST, &[], Some(&value), None)
        }

        /// The finished record as written to disk: end marker, used size, and update sequence.
        pub fn build(&mut self) -> Vec<u8> {
            let mut bytes = self.bytes.clone();
            bytes[self.at..self.at + 4].copy_from_slice(&ATTR_END.to_le_bytes());
            bytes[0x18..0x1C].copy_from_slice(&((self.at + 8) as u32).to_le_bytes());
            let count = u16_at(&bytes, 6).unwrap() as usize;
            let check = [0x5A, 0xA5];
            bytes[0x30..0x32].copy_from_slice(&check);
            for i in 1..count {
                let end = i * FIXUP_STRIDE - 2;
                let saved = [bytes[end], bytes[end + 1]];
                bytes[0x30 + 2 * i..0x32 + 2 * i].copy_from_slice(&saved);
                bytes[end..end + 2].copy_from_slice(&check);
            }
            bytes
        }
    }

    #[test]
    fn parses_a_file_with_hard_links_and_a_dos_alias() {
        // 0x40000 (extended attributes) is NTFS-internal and must not read as "recall on open".
        let mut record = RecordBuilder::new(1024, 7, RECORD_IN_USE, 0)
            .std_info(133_000_000_000_000_000, 0x0040_0022 | 0x0004_0000)
            .file_name(5 | 5 << 48, NAMESPACE_WIN32, "Long file name.txt")
            .file_name(5 | 5 << 48, NAMESPACE_DOS, "LONGFI~1.TXT")
            .file_name(40 | 2 << 48, NAMESPACE_POSIX, "a much longer hard link name.txt")
            .data(b"hello")
            .build();
        // The last name spans the first 512-byte fixup boundary: it decodes because fixups go first.
        let parsed = parse(&mut record, true).unwrap();
        assert!(parsed.in_use && !parsed.dir);
        assert_eq!((parsed.seq, parsed.base, parsed.attrs, parsed.size), (7, 0, 0x0040_0022, Some(5)));
        assert_eq!(parsed.modified, Some(133_000_000_000_000_000));
        let names: Vec<(u64, &str)> = parsed.names.iter().map(|n| (n.parent, n.name.as_str())).collect();
        assert_eq!(names, [(5 | 5 << 48, "Long file name.txt"), (40 | 2 << 48, "a much longer hard link name.txt")]);
    }

    #[test]
    fn non_resident_sizes_come_from_the_first_extent() {
        let runs = [0x21, 0x10, 0x00, 0x01, 0x00];
        let mut record = RecordBuilder::new(1024, 1, RECORD_IN_USE, 0)
            .std_info(1, 0x20)
            .file_name(5, NAMESPACE_WIN32_AND_DOS, "big.iso")
            .data_runs("Zone.Identifier", 0, 99, &runs)
            .data_runs("", 0, 4_700_000_000, &runs)
            .build();
        assert_eq!(parse(&mut record, true).unwrap().size, Some(4_700_000_000));
        // A later extent (in an extension record) says nothing about the size.
        let mut extension = RecordBuilder::new(1024, 3, RECORD_IN_USE, 77 | 1 << 48).data_runs("", 16, 0, &runs).build();
        let parsed = parse(&mut extension, true).unwrap();
        assert_eq!((parsed.base, parsed.size, parsed.modified), (77 | 1 << 48, None, None));
    }

    #[test]
    fn directories_free_records_and_garbage() {
        let mut dir = RecordBuilder::new(4096, 2, RECORD_IN_USE | RECORD_IS_DIRECTORY, 0).file_name(5, 1, "Users").build();
        let parsed = parse(&mut dir, true).unwrap();
        assert!(parsed.dir && parsed.size.is_none());
        let mut free = RecordBuilder::new(1024, 9, 0, 0).file_name(5, 1, "deleted.txt").build();
        let parsed = parse(&mut free, true).unwrap();
        assert!(!parsed.in_use && parsed.names.is_empty());
        assert!(parse(&mut vec![0u8; 1024], true).is_none());
        let mut baad = dir.clone();
        baad[0..4].copy_from_slice(b"BAAD");
        assert!(parse(&mut baad, true).is_none());
    }

    #[test]
    fn torn_records_fail_only_strict_parsing() {
        let mut record = RecordBuilder::new(1024, 1, RECORD_IN_USE, 0).file_name(5, 1, "a.txt").build();
        record[1022] ^= 0xFF;
        assert!(parse(&mut record.clone(), true).is_none());
        // Records the file system hands out may come with fixups already applied.
        let mut fixed = RecordBuilder::new(1024, 1, RECORD_IN_USE, 0).file_name(5, 1, "a.txt").build();
        assert!(apply_fixups(&mut fixed, true));
        assert_eq!(parse(&mut fixed.clone(), false).unwrap().names[0].name, "a.txt");
    }

    #[test]
    fn attribute_lists_point_to_extension_records() {
        let mut record = RecordBuilder::new(1024, 1, RECORD_IN_USE, 0)
            .std_info(1, 0)
            .attribute_list(&[(ATTR_STANDARD_INFORMATION, 50), (ATTR_FILE_NAME, 61 | 1 << 48), (ATTR_DATA, 62), (ATTR_DATA, 62), (ATTR_FILE_NAME, 50)])
            .build();
        let parsed = parse(&mut record, true).unwrap();
        let Some(AttributeList::Resident(list)) = parsed.attribute_list else { panic!("no list") };
        assert_eq!(extension_records(&list, 50), [61, 62]);
    }

    #[test]
    fn decodes_data_runs() {
        // 0x10 clusters at 0x100, then 0x20 at 0x100 - 0x80 = 0x80, a sparse run, then 1 at +0x10.
        let runs = parse_runs(&[0x21, 0x10, 0x00, 0x01, 0x11, 0x20, 0x80, 0x01, 0x05, 0x11, 0x01, 0x10, 0x00]).unwrap();
        assert_eq!(
            runs,
            [
                Run { lcn: Some(0x100), clusters: 0x10 },
                Run { lcn: Some(0x80), clusters: 0x20 },
                Run { lcn: None, clusters: 5 },
                Run { lcn: Some(0x90), clusters: 1 }
            ]
        );
        assert!(parse_runs(&[0x21, 0x10]).is_err());
        assert!(parse_runs(&[0x11, 0x01, 0xF0, 0x00]).is_err());
        let mut record = RecordBuilder::new(1024, 1, RECORD_IN_USE, 0).data_runs("", 0, 0x4000, &[0x21, 0x10, 0x00, 0x01, 0x00]).build();
        apply_fixups(&mut record, true);
        assert_eq!(data_extents(&record), [(0, Some(0x4000), vec![Run { lcn: Some(0x100), clusters: 0x10 }])]);
    }

    #[test]
    fn reads_the_boot_sector() {
        let mut boot = vec![0u8; 512];
        boot[3..11].copy_from_slice(b"NTFS    ");
        boot[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        boot[0x0D] = 8;
        boot[0x28..0x30].copy_from_slice(&1_000_000u64.to_le_bytes());
        boot[0x30..0x38].copy_from_slice(&786_432u64.to_le_bytes());
        boot[0x40] = 0xF6; // -10: 1 KiB records
        let g = parse_boot_sector(&boot).unwrap();
        assert_eq!((g.bytes_per_cluster, g.record_size, g.mft_lcn), (4096, 1024, 786_432));
        boot[0x40] = 1;
        assert_eq!(parse_boot_sector(&boot).unwrap().record_size, 4096);
        boot[0x40] = 0xF6;
        boot[0x0D] = 0xF4; // 2^12 sectors: 2 MiB clusters
        assert_eq!(parse_boot_sector(&boot).unwrap().bytes_per_cluster, 2 << 20);
        boot[3] = b'X';
        assert!(parse_boot_sector(&boot).is_err());
    }
}
