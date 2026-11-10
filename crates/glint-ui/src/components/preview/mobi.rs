//! MOBI, AZW and AZW3 books: the reader foliate-js had in `src/vendor/foliate/mobi.js`, written here and read by
//! [`book`](super::book)'s view like an EPUB.
//!
//! The format is a Palm database (PalmDB) of records. Record 0 holds the PalmDOC header, the MOBI header and the
//! EXTH metadata; the next records are the text, compressed one by one (nothing, PalmDOC's LZ77, or HUFF/CDIC); after
//! them come the pictures. An old book ("MOBI 6") is one HTML text cut into chapters at its `<mbp:pagebreak>`s; a
//! KF8 book (AZW3, or the second half of a combined file) is XHTML parts built from a skeleton and its fragments.
//!
//! | TypeScript (`mobi.js`)                    | Here                                                                    |
//! |-------------------------------------------|-------------------------------------------------------------------------|
//! | `isMOBI`, `PDB`                           | [`Pdb`]: the `BOOKMOBI` check, the record table, a record at a time off the disk |
//! | `getStruct`, the header tables            | [`get_headers`]: PalmDOC, MOBI, EXTH ([`Exth`]) and the KF8 header, read from record 0 |
//! | `MOBI_LANG`                               | [`locale_language`]                                                     |
//! | `decompressPalmDOC`                       | [`decompress_palmdoc`]                                                  |
//! | `huffcdic`, `read32Bits`                  | [`Huff`] and [`read32`]                                                 |
//! | `removeTrailingEntries`                   | [`strip_trailing`]                                                      |
//! | `MOBI.open` (the combined file's boundary) | [`open_reader`]: a KF8 half after `boundary` is read in place of the MOBI 6 half |
//! | `getIndexData`, `getNCX`, `getVarLen`     | [`get_index_data`], [`Ncx`], [`var_len`]                                 |
//! | `MOBI6` (sections, `filepos`, `recindex`) | [`Old`], [`Converter`]: the old HTML made well-formed, anchors where `filepos` points, pictures from `recindex` |
//! | `KF8` (FDST, skeleton, fragments, `kindle:` URIs) | [`Kf8`]: [`Kf8::part_text`] builds a part; [`Kf8::transform`] turns `kindle:pos`, `kindle:embed` and `kindle:flow` into links, pictures and styles |
//! | `getMetadata`, `getCover`                 | [`Metadata`], the book's title, language and cover                      |
//!
//! # How it differs from foliate
//!
//! - **The whole text is unpacked once, when the book is opened**, on the thread that opens books (never the UI's).
//!   foliate unpacked records lazily from the front and the back; here the text is held in memory (at most
//!   [`MAX_TEXT`]) so a `filepos` or a fragment offset can be looked up without unpacking again. A chapter is turned
//!   into blocks when it is asked for.
//! - **Chapters are blocks, not an iframe.** The HTML of a chapter becomes well-formed XHTML ([`Converter`] for the
//!   old books, [`Kf8::transform`] for the new) and goes through [`xhtml_document`] like an EPUB chapter. Pictures
//!   travel as `data:` addresses, since that reader takes a chapter's pictures from a zip.
//! - **A damaged text record is skipped** (foliate stopped); a missing EXTH header is read as empty.
//! - **Blank sections are joined to the next one**, so a pagebreak after a pagebreak is not an empty page. A section
//!   longer than [`SPLIT`] bytes is cut at a paragraph so no chapter is a megabyte of blocks.
//!
//! # Not ported
//!
//! - **DRM.** A book whose PalmDOC header says it is encrypted is refused with [`BookError::Drm`]. Nothing here
//!   tries to remove it.
//! - **Fonts, audio, video** inside the book (`FONT`, `VIDE`, `AUDI` records), the `RESC` spine and its page-spread
//!   properties, fixed-layout books' viewport. The book's own CSS is read as in EPUB (see `book.rs`).
//! - **A book that foliate opened as `TEXtREAd`** (plain PalmDOC text) was never opened by foliate either (the
//!   check is for `BOOKMOBI`).
//!
//! The tests build their books in memory, byte by byte, as the format says. **No real Kindle file was available
//! while writing this**, so the KF8 reader follows foliate's code and the published format and is exercised by
//! books made to match it, not by books from a store.

use super::book::{xhtml_document, Book, BookError, Chapter, Source, TocItem};
use super::rich::{picture_of, rtl_language, sniff_image};
use crate::t;
use gpui_kit::ImageFormat;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::{Arc, Mutex};

/// `0xffffffff`: "no such record" in the headers.
const NONE: u32 = 0xffff_ffff;
/// The most text a book may unpack to.
const MAX_TEXT: usize = 256 * 1024 * 1024;
/// The most one packed text record may be (they are about 4 KB).
const MAX_TEXT_RECORD: u64 = 1024 * 1024;
/// The most one text record may unpack to under HUFF/CDIC.
const MAX_RECORD_OUT: usize = 16 * 1024 * 1024;
/// The most one record of any other kind (a picture, an index) may be.
const MAX_RECORD: u64 = 64 * 1024 * 1024;
/// The biggest picture put inside a chapter.
const MAX_INLINE_IMAGE: usize = 16 * 1024 * 1024;
/// The biggest cover read for the contents panel.
const MAX_COVER: usize = 4 * 1024 * 1024;
/// An old book's section longer than this is cut in two (at a paragraph if one is near).
const SPLIT: usize = 400_000;
/// How far past the cut the next paragraph is looked for.
const SPLIT_REACH: usize = 64 * 1024;
/// How deeply elements of an old book's HTML may nest in the XHTML made of it (the reader refuses 100).
const MAX_NEST: usize = 80;
/// The longest a start tag is taken to be when its quotes are followed.
const MAX_TAG: usize = 4096;

// ---- Why a book does not open ----------------------------------------------------------------------------------

fn damaged() -> BookError {
    BookError::Broken(t!("This book is damaged, or is in a form Glint can't read.").to_string())
}

fn too_big() -> BookError {
    BookError::Broken(t!("This book is too big to preview").to_string())
}

fn io_error(error: std::io::Error) -> BookError {
    BookError::Broken(error.to_string())
}

// ---- Bytes -----------------------------------------------------------------------------------------------------

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    b.get(at..end)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_be_bytes)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    b.get(at..end)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_be_bytes)
}

/// `getUint`: a number as wide as its bytes (4, 2, else 1).
fn uint_of(data: &[u8]) -> u32 {
    match data.len() {
        4 => u32_at(data, 0).unwrap_or(0),
        2 => u32::from(u16_at(data, 0).unwrap_or(0)),
        _ => u32::from(data.first().copied().unwrap_or(0)),
    }
}

fn starts_with_ci(b: &[u8], prefix: &[u8]) -> bool {
    b.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Where `needle` first is in `hay` from `from`, ignoring ASCII case.
fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| starts_with_ci(&hay[i..], needle))
}

/// A string of ASCII digits read from the front of `b`.
fn digits_of(b: &[u8]) -> &str {
    let end = b.iter().position(|c| !c.is_ascii_digit()).unwrap_or(b.len());
    std::str::from_utf8(&b[..end]).unwrap_or("")
}

/// A number written in base 32 (`0-9`, `a-v`), as far as it is: `parseInt(text, 32)`.
fn base32(text: &str) -> Option<u32> {
    let mut value: u32 = 0;
    let mut any = false;
    for c in text.chars() {
        let Some(digit) = c.to_digit(32) else { break };
        value = value.checked_mul(32)?.checked_add(digit)?;
        any = true;
    }
    any.then_some(value)
}

/// Standard base64, padded.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        let pick = |shift: u32| char::from(ALPHABET[((n >> shift) & 63) as usize]);
        out.push(pick(18));
        out.push(pick(12));
        out.push(if chunk.len() > 1 { pick(6) } else { '=' });
        out.push(if chunk.len() > 2 { pick(0) } else { '=' });
    }
    out
}

/// `unescapeHTML`: the entities of an EXTH string.
fn unescape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                other => other
                    .strip_prefix('#')
                    .and_then(|n| {
                        n.strip_prefix(['x', 'X'])
                            .map_or_else(|| n.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok())
                    })
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, end + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of the book's own encoding (`MOBI_ENCODING`: 1252 is Windows-1252, anything else is read as UTF-8).
fn decode_text(encoding: u32, bytes: &[u8]) -> String {
    if encoding == 1252 {
        if let Some(text) = glint_core::textenc::decode_as(bytes, "windows-1252") {
            return text;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

// ---- The Palm database -----------------------------------------------------------------------------------------

trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// A PalmDB file (`PDB`): the records are read off the disk one at a time.
struct Pdb {
    reader: Mutex<Box<dyn ReadSeek>>,
    /// Where each record starts, then the file's length: a record ends where the next one starts.
    offsets: Vec<u64>,
}

impl Pdb {
    /// Reads the header and the record table. A file that is not a `BOOKMOBI` database is unsupported.
    fn open(mut reader: Box<dyn ReadSeek>) -> Result<Self, BookError> {
        let len = reader.seek(SeekFrom::End(0)).map_err(io_error)?;
        reader.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let mut head = [0u8; 78];
        reader.read_exact(&mut head).map_err(|_| BookError::Unsupported)?;
        if &head[60..68] != b"BOOKMOBI" {
            return Err(BookError::Unsupported);
        }
        let count = usize::from(u16::from_be_bytes([head[76], head[77]]));
        if count == 0 {
            return Err(damaged());
        }
        let mut table = vec![0u8; count * 8];
        reader.read_exact(&mut table).map_err(|_| damaged())?;
        let mut offsets: Vec<u64> = (0..count)
            .map(|i| u64::from(u32_at(&table, i * 8).unwrap_or(0)))
            .collect();
        offsets.push(len);
        Ok(Self {
            reader: Mutex::new(reader),
            offsets,
        })
    }

    fn count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Record `index`, unless it is missing, longer than `limit`, or can't be read.
    fn record(&self, index: usize, limit: u64) -> Option<Vec<u8>> {
        let start = *self.offsets.get(index)?;
        let file_len = *self.offsets.last()?;
        let end = (*self.offsets.get(index.checked_add(1)?)?).min(file_len);
        if start > file_len {
            return None;
        }
        let size = end.saturating_sub(start);
        if size > limit {
            return None;
        }
        let mut reader = self.reader.lock().ok()?;
        reader.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = vec![0u8; usize::try_from(size).ok()?];
        reader.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

// ---- The headers -----------------------------------------------------------------------------------------------

/// The EXTH records the reader uses (`EXTH_RECORD_TYPE`).
#[derive(Clone, Debug, Default)]
struct Exth {
    creators: Vec<String>,
    publisher: Option<String>,
    description: Option<String>,
    subjects: Vec<String>,
    date: Option<String>,
    contributors: Vec<String>,
    rights: Option<String>,
    languages: Vec<String>,
    title: Option<String>,
    boundary: Option<u32>,
    cover_offset: Option<u32>,
    thumbnail_offset: Option<u32>,
    page_progression: Option<String>,
}

/// The EXTH header (`buf` starts at its `EXTH` magic). One that isn't there or isn't valid reads as empty.
fn parse_exth(buf: &[u8], encoding: u32) -> Exth {
    let mut exth = Exth::default();
    if buf.get(..4) != Some(b"EXTH") {
        return exth;
    }
    let count = u32_at(buf, 8).unwrap_or(0);
    let mut offset = 12usize;
    for _ in 0..count {
        let (Some(kind), Some(length)) = (u32_at(buf, offset), u32_at(buf, offset + 4)) else {
            break;
        };
        let length = length as usize;
        if length < 8 {
            break;
        }
        let end = offset.saturating_add(length).min(buf.len());
        let data = buf.get(offset + 8..end).unwrap_or(&[]);
        let text = || decode_text(encoding, data);
        match kind {
            100 => exth.creators.push(text()),
            101 => exth.publisher = Some(text()),
            103 => exth.description = Some(text()),
            105 => exth.subjects.push(text()),
            106 => exth.date = Some(text()),
            108 => exth.contributors.push(text()),
            109 => exth.rights = Some(text()),
            121 => exth.boundary = Some(uint_of(data)),
            201 => exth.cover_offset = Some(uint_of(data)),
            202 => exth.thumbnail_offset = Some(uint_of(data)),
            503 => exth.title = Some(text()),
            524 => exth.languages.push(text()),
            527 => exth.page_progression = Some(text()),
            _ => {}
        }
        offset = offset.saturating_add(length);
    }
    exth
}

/// `MOBI_LANG`: a language by the MOBI header's two locale bytes; an entry of `""` is a hole in foliate's table.
#[rustfmt::skip]
const LANGS: &[(u8, &[&str])] = &[
    (1, &["ar", "ar-SA", "ar-IQ", "ar-EG", "ar-LY", "ar-DZ", "ar-MA", "ar-TN", "ar-OM", "ar-YE", "ar-SY", "ar-JO", "ar-LB", "ar-KW", "ar-AE", "ar-BH", "ar-QA"]),
    (2, &["bg"]), (3, &["ca"]), (4, &["zh", "zh-TW", "zh-CN", "zh-HK", "zh-SG"]), (5, &["cs"]), (6, &["da"]),
    (7, &["de", "de-DE", "de-CH", "de-AT", "de-LU", "de-LI"]), (8, &["el"]),
    (9, &["en", "en-US", "en-GB", "en-AU", "en-CA", "en-NZ", "en-IE", "en-ZA", "en-JM", "", "en-BZ", "en-TT", "en-ZW", "en-PH"]),
    (10, &["es", "es-ES", "es-MX", "", "es-GT", "es-CR", "es-PA", "es-DO", "es-VE", "es-CO", "es-PE", "es-AR", "es-EC", "es-CL", "es-UY", "es-PY", "es-BO", "es-SV", "es-HN", "es-NI", "es-PR"]),
    (11, &["fi"]), (12, &["fr", "fr-FR", "fr-BE", "fr-CA", "fr-CH", "fr-LU", "fr-MC"]), (13, &["he"]), (14, &["hu"]),
    (15, &["is"]), (16, &["it", "it-IT", "it-CH"]), (17, &["ja"]), (18, &["ko"]), (19, &["nl", "nl-NL", "nl-BE"]),
    (20, &["no", "nb", "nn"]), (21, &["pl"]), (22, &["pt", "pt-BR", "pt-PT"]), (23, &["rm"]), (24, &["ro"]),
    (25, &["ru"]), (26, &["hr", "", "sr"]), (27, &["sk"]), (28, &["sq"]), (29, &["sv", "sv-SE", "sv-FI"]),
    (30, &["th"]), (31, &["tr"]), (32, &["ur"]), (33, &["id"]), (34, &["uk"]), (35, &["be"]), (36, &["sl"]),
    (37, &["et"]), (38, &["lv"]), (39, &["lt"]), (41, &["fa"]), (42, &["vi"]), (43, &["hy"]), (44, &["az"]),
    (45, &["eu"]), (46, &["hsb"]), (47, &["mk"]), (48, &["st"]), (49, &["ts"]), (50, &["tn"]), (52, &["xh"]),
    (53, &["zu"]), (54, &["af"]), (55, &["ka"]), (56, &["fo"]), (57, &["hi"]), (58, &["mt"]), (59, &["se"]),
    (62, &["ms"]), (63, &["kk"]), (65, &["sw"]), (67, &["uz", "", "uz-UZ"]), (68, &["tt"]), (69, &["bn"]),
    (70, &["pa"]), (71, &["gu"]), (72, &["or"]), (73, &["ta"]), (74, &["te"]), (75, &["kn"]), (76, &["ml"]),
    (77, &["as"]), (78, &["mr"]), (79, &["sa"]), (82, &["cy", "cy-GB"]), (83, &["gl", "gl-ES"]), (87, &["kok"]),
    (97, &["ne"]), (98, &["fy"]),
];

/// The language the MOBI header's locale bytes name: the region's variant if there is one, else the language.
fn locale_language(language: u8, region: u8) -> Option<&'static str> {
    let (_, variants) = LANGS.iter().find(|(id, _)| *id == language)?;
    variants
        .get(usize::from(region >> 2))
        .filter(|v| !v.is_empty())
        .or_else(|| variants.first())
        .copied()
}

/// The KF8 header's pointers (`KF8_HEADER`).
#[derive(Clone, Copy, Debug)]
struct Kf8Header {
    fdst: u32,
    frag: u32,
    skel: u32,
    guide: u32,
}

/// Record 0, read: the PalmDOC header, the MOBI header, EXTH and the KF8 header.
#[derive(Clone, Debug)]
struct Headers {
    compression: u16,
    text_records: u16,
    encryption: u16,
    encoding: u32,
    uid: u32,
    version: u32,
    resource_start: u32,
    huffcdic: u32,
    num_huffcdic: u32,
    trailing_flags: u32,
    indx: u32,
    title: String,
    language: Option<&'static str>,
    exth: Exth,
    kf8: Option<Kf8Header>,
}

/// `#getHeaders`: record 0, or an error when it has no `MOBI` header.
fn get_headers(rec: &[u8]) -> Result<Headers, BookError> {
    if rec.get(16..20) != Some(b"MOBI") {
        return Err(damaged());
    }
    let word = |at: usize| u32_at(rec, at).unwrap_or(0);
    let length = word(20);
    let encoding = word(28);
    let version = word(36);
    let title_offset = word(84) as usize;
    let title_length = word(88) as usize;
    let title_end = title_offset.saturating_add(title_length).min(rec.len());
    let title = decode_text(encoding, rec.get(title_offset.min(title_end)..title_end).unwrap_or(&[]));
    let language = locale_language(rec.get(95).copied().unwrap_or(0), rec.get(94).copied().unwrap_or(0));
    let exth_flag = word(128);
    let exth = if exth_flag & 0x40 != 0 {
        let at = 16usize.saturating_add(length as usize);
        parse_exth(rec.get(at..).unwrap_or(&[]), encoding)
    } else {
        Exth::default()
    };
    // The trailing-entry flags are inside the header only when it is long enough to hold them.
    let trailing_flags = if length >= 228 { word(240) } else { 0 };
    Ok(Headers {
        compression: u16_at(rec, 0).unwrap_or(0),
        text_records: u16_at(rec, 8).unwrap_or(0),
        encryption: u16_at(rec, 12).unwrap_or(0),
        encoding,
        uid: word(32),
        version,
        resource_start: word(108),
        huffcdic: word(112),
        num_huffcdic: word(116),
        trailing_flags,
        indx: word(244),
        title,
        language,
        exth,
        kf8: (version >= 8).then(|| Kf8Header {
            fdst: word(192),
            frag: word(248),
            skel: word(252),
            guide: word(260),
        }),
    })
}

// ---- Unpacking the text ----------------------------------------------------------------------------------------

/// `decompressPalmDOC`: PalmDOC's LZ77. Bytes 1 to 8 copy that many bytes, 0x09 to 0x7f are themselves, 0x80 to
/// 0xbf are a distance and a length, 0xc0 and up are a space and a letter.
fn decompress_palmdoc(array: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(array.len() * 2);
    let mut i = 0;
    while i < array.len() {
        let byte = array[i];
        if byte == 0 {
            out.push(0);
        } else if byte <= 8 {
            let end = (i + 1 + usize::from(byte)).min(array.len());
            out.extend_from_slice(&array[i + 1..end]);
            i += usize::from(byte);
        } else if byte <= 0x7f {
            out.push(byte);
        } else if byte <= 0xbf {
            let next = array.get(i + 1).copied().unwrap_or(0);
            i += 1;
            let pair = (u16::from(byte) << 8) | u16::from(next);
            let distance = usize::from((pair & 0x3fff) >> 3);
            let length = usize::from(pair & 7) + 3;
            for _ in 0..length {
                // A distance that reaches before the start (a damaged record) copies a zero, as foliate's
                // `undefined` became one.
                let b = if distance > 0 && distance <= out.len() {
                    out[out.len() - distance]
                } else {
                    0
                };
                out.push(b);
            }
        } else {
            out.push(b' ');
            out.push(byte ^ 0x80);
        }
        i += 1;
    }
    out
}

/// `read32Bits`: the 32 bits that start `from` bits into `data` (zeros past its end).
fn read32(data: &[u8], from: usize) -> u32 {
    let byte = from >> 3;
    let mut window: u64 = 0;
    for k in 0..5 {
        window = (window << 8) | u64::from(data.get(byte + k).copied().unwrap_or(0));
    }
    ((window >> (8 - (from & 7))) & 0xffff_ffff) as u32
}

/// HUFF/CDIC (`huffcdic`): a Huffman code (`HUFF`, two tables) over a dictionary (the `CDIC` records) whose entries
/// may themselves be packed.
struct Huff {
    /// By the next eight bits: whether the code is complete in them, its length, its value.
    table1: Vec<(bool, u32, u32)>,
    /// By code length (index 0 unused): the least code of the length, and the value codes are counted back from.
    table2: Vec<(u32, u32)>,
    /// The entries, and whether each is unpacked.
    dict: Vec<(Vec<u8>, bool)>,
}

impl Huff {
    /// Reads the tables from the records `first ..` of the book (`count` of them, the HUFF record and its CDICs).
    fn load(pdb: &Pdb, start: usize, first: u32, count: u32) -> Option<Self> {
        let base = start.checked_add(first as usize)?;
        let huff = pdb.record(base, MAX_RECORD)?;
        if huff.get(..4)? != b"HUFF" {
            return None;
        }
        let offset1 = u32_at(&huff, 8)? as usize;
        let offset2 = u32_at(&huff, 12)? as usize;
        let mut table1 = Vec::with_capacity(256);
        for i in 0..256 {
            let x = u32_at(&huff, offset1.checked_add(i * 4)?)?;
            table1.push((x & 0x80 != 0, x & 0x1f, x >> 8));
        }
        let mut table2 = vec![(0, 0)];
        for i in 0..32 {
            let at = offset2.checked_add(i * 8)?;
            table2.push((u32_at(&huff, at)?, u32_at(&huff, at + 4)?));
        }
        let mut dict: Vec<(Vec<u8>, bool)> = Vec::new();
        for i in 1..count as usize {
            let record = pdb.record(base.checked_add(i)?, MAX_RECORD)?;
            if record.get(..4)? != b"CDIC" {
                return None;
            }
            let length = u32_at(&record, 4)? as usize;
            let num_entries = u32_at(&record, 8)? as usize;
            let code_length = u32_at(&record, 12)?;
            if code_length > 16 {
                return None;
            }
            // `numEntries` is the total over all the CDIC records; this one holds at most 2^codeLength of them.
            let n = (1usize << code_length).min(num_entries.saturating_sub(dict.len()));
            let buffer = record.get(length..)?;
            for k in 0..n {
                let offset = usize::from(u16_at(buffer, k * 2)?);
                let x = u16_at(buffer, offset)?;
                let size = usize::from(x & 0x7fff);
                let end = (offset + 2 + size).min(buffer.len());
                let value = buffer.get((offset + 2).min(end)..end).unwrap_or(&[]).to_vec();
                dict.push((value, x & 0x8000 != 0));
            }
        }
        Some(Self { table1, table2, dict })
    }

    /// Unpacks `data` onto `out`. None when the data runs out of the tables' reach (a damaged record).
    fn decompress(&mut self, data: &[u8], out: &mut Vec<u8>, depth: u32) -> Option<()> {
        if depth > 32 {
            return None;
        }
        let bit_length = data.len() * 8;
        let mut i = 0usize;
        while i < bit_length {
            let bits = read32(data, i);
            let (found, mut code_length, mut value) = *self.table1.get((bits >> 24) as usize)?;
            if !found {
                loop {
                    if code_length == 0 || code_length > 32 {
                        return None;
                    }
                    let (least, base) = *self.table2.get(code_length as usize)?;
                    if (bits >> (32 - code_length)) >= least {
                        value = base;
                        break;
                    }
                    code_length += 1;
                }
            } else if code_length == 0 {
                return None;
            }
            i += code_length as usize;
            if i > bit_length {
                break;
            }
            let code = value.checked_sub(bits >> (32 - code_length))? as usize;
            let entry = self.dict.get(code)?;
            if entry.1 {
                out.extend_from_slice(&entry.0);
            } else {
                // The entry is itself packed: unpack it, and keep the result for next time.
                let packed = entry.0.clone();
                let mut unpacked = Vec::new();
                self.decompress(&packed, &mut unpacked, depth + 1)?;
                out.extend_from_slice(&unpacked);
                if let Some(slot) = self.dict.get_mut(code) {
                    *slot = (unpacked, true);
                }
            }
            if out.len() > MAX_RECORD_OUT {
                return None;
            }
        }
        Some(())
    }
}

enum Unpack {
    None,
    PalmDoc,
    Huff(Huff),
}

/// A forward variable-length number (`getVarLen`): 7 bits a byte, the last byte has its top bit set. The number and
/// how many bytes it took; None past the end of `array`.
fn var_len(array: &[u8], at: usize) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    let mut length = 0usize;
    for &byte in array.get(at..)?.iter().take(4) {
        value = (value << 7) | u32::from(byte & 0x7f);
        length += 1;
        if byte & 0x80 != 0 {
            break;
        }
    }
    (length > 0).then_some((value, length))
}

/// `getVarLenFromEnd`: the same, read backwards from the end of the record's text.
fn var_len_from_end(data: &[u8]) -> u32 {
    let mut value: u32 = 0;
    for &byte in &data[data.len().saturating_sub(4)..] {
        if byte & 0x80 != 0 {
            value = 0;
        }
        value = (value << 7) | u32::from(byte & 0x7f);
    }
    value
}

/// `removeTrailingEntries`: a text record ends with entries the flags name (each says its own length) and, if the
/// first flag is set, the bytes of a character cut at the record's end.
fn strip_trailing(mut data: &[u8], flags: u32) -> &[u8] {
    for _ in 0..(flags >> 1).count_ones() {
        let length = var_len_from_end(data) as usize;
        data = &data[..data.len().saturating_sub(length)];
    }
    if flags & 1 != 0 {
        let Some(&last) = data.last() else { return data };
        let length = usize::from(last & 0b11) + 1;
        data = &data[..data.len().saturating_sub(length)];
    }
    data
}

/// All the book's text, unpacked: records `start + 1 ..` in order. A record that can't be unpacked is skipped; a
/// record missing from the file ends the text.
fn load_text(pdb: &Pdb, start: usize, h: &Headers) -> Result<Vec<u8>, BookError> {
    let mut unpack = match h.compression {
        1 => Unpack::None,
        2 => Unpack::PalmDoc,
        17480 => Unpack::Huff(Huff::load(pdb, start, h.huffcdic, h.num_huffcdic).ok_or_else(damaged)?),
        _ => {
            return Err(BookError::Broken(
                t!("This book is packed in a way Glint can't read.").to_string(),
            ))
        }
    };
    let mut text: Vec<u8> = Vec::new();
    for i in 0..usize::from(h.text_records) {
        let Some(record) = start.checked_add(i + 1).and_then(|at| pdb.record(at, MAX_TEXT_RECORD)) else {
            break;
        };
        let data = strip_trailing(&record, h.trailing_flags);
        match &mut unpack {
            Unpack::None => text.extend_from_slice(data),
            Unpack::PalmDoc => text.extend(decompress_palmdoc(data)),
            Unpack::Huff(huff) => {
                let mut out = Vec::new();
                if huff.decompress(data, &mut out, 0).is_some() {
                    text.extend(out);
                }
            }
        }
        if text.len() > MAX_TEXT {
            return Err(too_big());
        }
    }
    if text.is_empty() {
        return Err(damaged());
    }
    Ok(text)
}

// ---- Indexes ---------------------------------------------------------------------------------------------------

/// One entry of an index (`getIndexData`): its name and its tags' values.
struct IndexEntry {
    name: Vec<u8>,
    tags: HashMap<u32, Vec<u32>>,
}

/// An index: its entries, and the strings (CNCX) they point to by offset.
struct Index {
    table: Vec<IndexEntry>,
    cncx: HashMap<u32, String>,
}

/// Reads the index that starts at record `at` (`start` is where the KF8 half starts).
fn get_index_data(pdb: &Pdb, start: usize, at: u32) -> Option<Index> {
    let record = |i: usize| pdb.record(start.checked_add(i)?, MAX_RECORD);
    let first = at as usize;
    let indx_record = record(first)?;
    if indx_record.get(..4)? != b"INDX" {
        return None;
    }
    let indx_length = u32_at(&indx_record, 4)? as usize;
    let num_records = u32_at(&indx_record, 24)? as usize;
    let encoding = u32_at(&indx_record, 28)?;
    let num_cncx = u32_at(&indx_record, 52)? as usize;
    if num_records > pdb.count() || num_cncx > pdb.count() {
        return None;
    }

    let tagx = indx_record.get(indx_length..)?;
    if tagx.get(..4)? != b"TAGX" {
        return None;
    }
    let tagx_length = u32_at(tagx, 4)? as usize;
    let num_control_bytes = u32_at(tagx, 8)? as usize;
    let num_tags = tagx_length.saturating_sub(12) / 4;
    let tag_table: Vec<[u8; 4]> = (0..num_tags)
        .map(|i| {
            tagx.get(12 + i * 4..16 + i * 4)
                .and_then(|s| <[u8; 4]>::try_from(s).ok())
        })
        .collect::<Option<_>>()?;

    let mut cncx: HashMap<u32, String> = HashMap::new();
    let mut cncx_offset = 0u32;
    for i in 0..num_cncx {
        let strings = record(first.checked_add(num_records)?.checked_add(i + 1)?)?;
        let mut pos = 0usize;
        while pos < strings.len() {
            let index = pos;
            let (size, used) = var_len(&strings, pos)?;
            pos += used;
            let end = pos.saturating_add(size as usize).min(strings.len());
            let text = decode_text(encoding, strings.get(pos.min(end)..end).unwrap_or(&[]));
            cncx.insert(cncx_offset.wrapping_add(u32::try_from(index).ok()?), text);
            pos = end;
        }
        cncx_offset = cncx_offset.wrapping_add(0x10000);
    }

    let mut table = Vec::new();
    for i in 0..num_records {
        let data = record(first.checked_add(1)?.checked_add(i)?)?;
        if data.get(..4)? != b"INDX" {
            return None;
        }
        let idxt = u32_at(&data, 20)? as usize;
        let entries = u32_at(&data, 24)? as usize;
        if entries > data.len() {
            return None;
        }
        for j in 0..entries {
            let offset = usize::from(u16_at(&data, idxt.checked_add(4)?.checked_add(2 * j)?)?);
            let length = usize::from(*data.get(offset)?);
            let name = data.get(offset + 1..offset + 1 + length)?.to_vec();

            // Which tags the entry has, from its control bytes.
            let start_pos = offset + 1 + length;
            let mut control = 0usize;
            let mut pos = start_pos + num_control_bytes;
            // (tag, how many values, how many bytes of values, values per count)
            let mut wanted: Vec<(u32, Option<u32>, Option<u32>, u32)> = Vec::new();
            for &[tag, num_values, mask, end] in &tag_table {
                if end & 1 != 0 {
                    control += 1;
                    continue;
                }
                if mask == 0 {
                    continue;
                }
                let value = *data.get(start_pos + control)? & mask;
                if value == mask {
                    if mask.count_ones() > 1 {
                        let (bytes, used) = var_len(&data, pos)?;
                        wanted.push((u32::from(tag), None, Some(bytes), u32::from(num_values)));
                        pos += used;
                    } else {
                        wanted.push((u32::from(tag), Some(1), None, u32::from(num_values)));
                    }
                } else {
                    let count = u32::from(value >> mask.trailing_zeros());
                    wanted.push((u32::from(tag), Some(count), None, u32::from(num_values)));
                }
            }
            let mut tags: HashMap<u32, Vec<u32>> = HashMap::new();
            for (tag, count, bytes, num_values) in wanted {
                let mut values = Vec::new();
                if let Some(count) = count {
                    for _ in 0..count.saturating_mul(num_values) {
                        let (value, used) = var_len(&data, pos)?;
                        values.push(value);
                        pos += used;
                    }
                } else {
                    let bytes = bytes.unwrap_or(0) as usize;
                    let mut taken = 0usize;
                    while taken < bytes {
                        let (value, used) = var_len(&data, pos)?;
                        values.push(value);
                        pos += used;
                        taken += used;
                    }
                }
                tags.insert(tag, values);
            }
            table.push(IndexEntry { name, tags });
        }
    }
    Some(Index { table, cncx })
}

/// One entry of the NCX (the contents) as the KF8 index says it (`getNCX`).
struct Ncx {
    index: usize,
    label: String,
    level: Option<u32>,
    /// The fragment id and the offset in it.
    pos: Option<(u32, u32)>,
    parent: Option<u32>,
    first_child: Option<u32>,
}

fn get_ncx(pdb: &Pdb, start: usize, at: u32) -> Option<Vec<Ncx>> {
    let Index { table, cncx } = get_index_data(pdb, start, at)?;
    Some(
        table
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let first = |tag: u32| entry.tags.get(&tag).and_then(|v| v.first()).copied();
                Ncx {
                    index,
                    label: first(3).and_then(|k| cncx.get(&k)).cloned().unwrap_or_default(),
                    level: first(4),
                    pos: entry.tags.get(&6).and_then(|v| Some((*v.first()?, *v.get(1)?))),
                    parent: first(21),
                    first_child: first(22),
                }
            })
            .collect(),
    )
}

// ---- The book --------------------------------------------------------------------------------------------------

/// What a book says about itself (`getMetadata`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub identifier: String,
    pub title: String,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub language: Option<String>,
    pub published: Option<String>,
    pub description: Option<String>,
    pub subjects: Vec<String>,
    pub rights: Option<String>,
    pub contributors: Vec<String>,
}

fn metadata_of(h: &Headers) -> Metadata {
    let exth = &h.exth;
    let title = exth
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&h.title);
    Metadata {
        identifier: h.uid.to_string(),
        title: unescape_html(title).trim().to_string(),
        authors: exth.creators.iter().map(|a| unescape_html(a)).collect(),
        publisher: exth.publisher.as_deref().map(unescape_html),
        language: exth
            .languages
            .first()
            .cloned()
            .or_else(|| h.language.map(str::to_string)),
        published: exth.date.clone(),
        description: exth.description.as_deref().map(unescape_html),
        subjects: exth.subjects.iter().map(|s| unescape_html(s)).collect(),
        rights: exth.rights.as_deref().map(unescape_html),
        contributors: exth.contributors.clone(),
    }
}

/// What both kinds of book share: the file, the unpacked text, and where the pictures start.
struct Core {
    pdb: Pdb,
    text: Vec<u8>,
    encoding: u32,
    /// The first picture's record (the MOBI header's, not the KF8 half's, as foliate had it).
    resource_start: usize,
}

impl Core {
    fn decode(&self, bytes: &[u8]) -> String {
        decode_text(self.encoding, bytes)
    }

    /// Picture `index` (0 is the first after `resource_start`), unless that record is a font, a video or a sound,
    /// or not a picture.
    fn resource(&self, index: usize) -> Option<Vec<u8>> {
        let bytes = self.pdb.record(self.resource_start.checked_add(index)?, MAX_RECORD)?;
        let head = bytes.get(..4)?;
        if head == b"FONT" || head == b"VIDE" || head == b"AUDI" {
            return None;
        }
        Some(bytes)
    }

    /// A picture as a `data:` address, the form a chapter's pictures take here.
    fn image_uri(&self, index: usize) -> Option<String> {
        let bytes = self.resource(index)?;
        if bytes.len() > MAX_INLINE_IMAGE {
            return None;
        }
        let mime = match sniff_image(&bytes)? {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Bmp => "image/bmp",
            ImageFormat::Tiff => "image/tiff",
            ImageFormat::Svg => "image/svg+xml",
            _ => "image/png",
        };
        Some(format!("data:{mime};base64,{}", base64_encode(&bytes)))
    }

    /// `getCover`: the cover picture (or the thumbnail), when it is small enough to show.
    fn cover(&self, exth: &Exth) -> Option<Arc<gpui_kit::Image>> {
        let offset = exth
            .cover_offset
            .filter(|o| *o < NONE)
            .or(exth.thumbnail_offset.filter(|o| *o < NONE))?;
        let bytes = self.resource(offset as usize)?;
        if bytes.len() > MAX_COVER {
            return None;
        }
        let picture = picture_of(bytes, "", None);
        picture
            .size
            .is_none_or(|(w, h)| w <= 3000 && h <= 3000)
            .then_some(picture.image)
            .flatten()
    }
}

/// A chapter's name inside the book: links to it are `part0003.html#id`.
fn part_name(ix: usize) -> String {
    format!("part{ix:04}.html")
}

/// The chapter a `partNNNN.html` name is.
fn part_index(path: &str) -> Option<usize> {
    path.strip_prefix("part")?.strip_suffix(".html")?.parse().ok()
}

/// Opens the MOBI, AZW or AZW3 book at `path`.
pub fn open_mobi(path: &str) -> Result<Book, BookError> {
    open_mobi_details(path).map(|(book, _)| book)
}

/// [`open_mobi`], and what the book says about itself (its author, publisher, language…).
pub fn open_mobi_details(path: &str) -> Result<(Book, Metadata), BookError> {
    let file = std::fs::File::open(path).map_err(io_error)?;
    open_reader(Box::new(file))
}

/// A book from its bytes (the tests').
#[cfg(test)]
fn open_bytes(bytes: Vec<u8>) -> Result<(Book, Metadata), BookError> {
    open_reader(Box::new(std::io::Cursor::new(bytes)))
}

/// `MOBI.open`: the headers (the KF8 half's, in a combined file), the text, then the chapters of whichever kind.
fn open_reader(reader: Box<dyn ReadSeek>) -> Result<(Book, Metadata), BookError> {
    let pdb = Pdb::open(reader)?;
    let record0 = pdb.record(0, MAX_RECORD).ok_or_else(damaged)?;
    let first = get_headers(&record0)?;
    if first.encryption != 0 {
        return Err(BookError::Drm);
    }
    let resource_start = first.resource_start as usize;
    let mut headers = first;
    let mut start = 0usize;
    if headers.version < 8 {
        // A combined file: the KF8 book begins at the record `boundary` says. If it can't be opened the old half is.
        if let Some(boundary) = headers.exth.boundary.filter(|b| *b < NONE) {
            let kf8 = pdb
                .record(boundary as usize, MAX_RECORD)
                .and_then(|rec| get_headers(&rec).ok())
                .filter(|h| h.kf8.is_some());
            if let Some(kf8) = kf8 {
                headers = kf8;
                start = boundary as usize;
            }
        }
    }
    if headers.encryption != 0 {
        return Err(BookError::Drm);
    }
    let text = load_text(&pdb, start, &headers)?;
    let core = Core {
        pdb,
        text,
        encoding: headers.encoding,
        resource_start,
    };
    let metadata = metadata_of(&headers);
    let language = metadata.language.clone().unwrap_or_default();
    let cover = core.cover(&headers.exth);
    let (parts, source): (Parts, Arc<dyn Source>) = match headers.kf8 {
        Some(kf8) => {
            let (kf8, parts) = Kf8::open(core, &headers, kf8, start).ok_or_else(damaged)?;
            (parts, Arc::new(MobiSource::Kf8(kf8)))
        }
        None => {
            let (old, parts) = Old::open(core);
            (parts, Arc::new(MobiSource::Old(old)))
        }
    };
    let rtl = match headers.exth.page_progression.as_deref() {
        Some(d) if d.eq_ignore_ascii_case("rtl") => true,
        Some(d) if d.eq_ignore_ascii_case("ltr") => false,
        _ => rtl_language(&language),
    };
    let count = parts.weights.len();
    let book = Book {
        title: metadata.title.clone(),
        language: super::book::named_language(&language),
        rtl,
        toc: parts.toc,
        count,
        weights: parts.weights,
        start: parts.start.min(count.saturating_sub(1)),
        cover,
        pages: false,
        source,
    };
    Ok((book, metadata))
}

/// What a kind of book works out about its chapters.
struct Parts {
    weights: Vec<u64>,
    toc: Vec<TocItem>,
    start: usize,
}

enum MobiSource {
    Old(Old),
    Kf8(Kf8),
}

impl Source for MobiSource {
    fn chapter(&self, ix: usize) -> Result<Chapter, String> {
        let text = match self {
            MobiSource::Old(old) => old.convert(ix, false).map(|c| old.core.decode(&c.xhtml)),
            MobiSource::Kf8(kf8) => kf8.part_xhtml(ix),
        };
        let text = text.ok_or_else(|| t!("This part of the book can't be read.").to_string())?;
        Ok(Chapter::Text(Arc::new(xhtml_document(&text, &part_name(ix), None))))
    }

    fn locate(&self, path: &str) -> Option<usize> {
        let ix = part_index(path)?;
        let count = match self {
            MobiSource::Old(old) => old.chapters.len(),
            MobiSource::Kf8(kf8) => kf8.parts.len(),
        };
        (ix < count).then_some(ix)
    }
}

// ---- HTML, read leniently ---------------------------------------------------------------------------------------

/// Whether the `<` at `i` starts a tag, a comment or a declaration (else it is a less-than sign in the text).
fn is_tag_start(b: &[u8], i: usize) -> bool {
    match b.get(i + 1) {
        Some(c) if c.is_ascii_alphabetic() => true,
        Some(b'!') | Some(b'?') => true,
        Some(b'/') => b.get(i + 2).is_some_and(u8::is_ascii_alphabetic),
        _ => false,
    }
}

/// Just past the `>` that ends the tag (or comment) at `start`: quoted attribute values may hold a `>`.
fn tag_end(b: &[u8], start: usize) -> usize {
    let rest = &b[start..];
    for (open, close) in [(&b"<!--"[..], &b"-->"[..]), (&b"<![CDATA["[..], &b"]]>"[..])] {
        if rest.starts_with(open) {
            return (open.len()..=rest.len().saturating_sub(close.len()))
                .find(|&i| rest[i..].starts_with(close))
                .map_or(b.len(), |i| start + i + close.len());
        }
    }
    let mut quote = 0u8;
    let mut previous = 0u8;
    for (k, &c) in rest.iter().enumerate().skip(1).take(MAX_TAG) {
        if quote != 0 {
            if c == quote {
                quote = 0;
                previous = c;
            }
            continue;
        }
        match c {
            b'>' => return start + k + 1,
            b'"' | b'\'' if previous == b'=' => quote = c,
            c if !c.is_ascii_whitespace() => previous = c,
            _ => {}
        }
    }
    // Quotes that never close: the first `>` is the end.
    rest.iter().position(|&c| c == b'>').map_or(b.len(), |k| start + k + 1)
}

/// A tag, read: its lowercase name and attributes.
struct Tag {
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: Vec<(String, Vec<u8>)>,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&[u8]> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_slice())
    }
}

fn parse_tag(raw: &[u8]) -> Option<Tag> {
    let mut i = 1;
    let closing = raw.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    let name_start = i;
    while i < raw.len() && !raw[i].is_ascii_whitespace() && raw[i] != b'/' && raw[i] != b'>' {
        i += 1;
    }
    if i == name_start || !raw[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = String::from_utf8_lossy(&raw[name_start..i]).to_ascii_lowercase();
    let mut attrs = Vec::new();
    while i < raw.len() {
        let c = raw[i];
        if c.is_ascii_whitespace() || c == b'/' {
            i += 1;
            continue;
        }
        if c == b'>' {
            break;
        }
        let key_start = i;
        while i < raw.len() && !raw[i].is_ascii_whitespace() && !matches!(raw[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        if i == key_start {
            i += 1;
            continue;
        }
        let key = String::from_utf8_lossy(&raw[key_start..i]).to_ascii_lowercase();
        while i < raw.len() && raw[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if raw.get(i) == Some(&b'=') {
            i += 1;
            while i < raw.len() && raw[i].is_ascii_whitespace() {
                i += 1;
            }
            match raw.get(i) {
                Some(&q) if q == b'"' || q == b'\'' => {
                    let from = i + 1;
                    let to = raw[from.min(raw.len())..]
                        .iter()
                        .position(|&c| c == q)
                        .map_or(raw.len(), |p| from + p);
                    i = (to + 1).min(raw.len());
                    raw[from.min(to)..to].to_vec()
                }
                _ => {
                    let from = i;
                    while i < raw.len() && !raw[i].is_ascii_whitespace() && raw[i] != b'>' {
                        i += 1;
                    }
                    raw[from..i].to_vec()
                }
            }
        } else {
            key.clone().into_bytes()
        };
        attrs.push((key, value));
    }
    Some(Tag {
        name,
        closing,
        self_closing: raw.ends_with(b"/>"),
        attrs,
    })
}

/// A name XML accepts (no prefix: the old books' `mbp:` tags and `xml:` attributes are dropped).
fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// Elements with no end tag.
const VOID: [&str; 17] = [
    "br", "hr", "img", "meta", "link", "input", "area", "base", "col", "param", "source", "wbr", "embed", "track",
    "basefont", "frame", "isindex",
];

const INLINE: [&str; 27] = [
    "a", "b", "i", "u", "s", "em", "strong", "font", "span", "sup", "sub", "small", "big", "tt", "code", "cite",
    "strike", "del", "ins", "q", "abbr", "acronym", "dfn", "kbd", "samp", "var", "label",
];

/// How many open elements an `<name>` start tag ends first: a `<p>` ends a `<p>` still open (with only inline
/// elements between), an `<li>` ends an `<li>`, and so on.
fn closes_for(stack: &[String], name: &str) -> usize {
    let (targets, passes): (&[&str], &[&str]) = match name {
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "dl" | "table" | "blockquote" | "pre"
        | "hr" | "center" | "form" | "address" | "fieldset" => (&["p"], &[]),
        "li" => (&["li"], &["p"]),
        "dt" | "dd" => (&["dt", "dd"], &["p"]),
        "tr" => (&["tr"], &["p", "td", "th"]),
        "td" | "th" => (&["td", "th"], &["p"]),
        _ => return 0,
    };
    for (depth, open) in stack.iter().rev().enumerate() {
        if targets.contains(&open.as_str()) {
            return depth + 1;
        }
        if !(INLINE.contains(&open.as_str()) || passes.contains(&open.as_str())) {
            break;
        }
    }
    0
}

fn escape_attr(out: &mut Vec<u8>, value: &[u8]) {
    for &c in value {
        match c {
            b'"' => out.extend_from_slice(b"&quot;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            c if c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r') => {}
            c => out.push(c),
        }
    }
}

// ---- MOBI 6 ----------------------------------------------------------------------------------------------------

/// A `filepos` an old book's links point to: the digits as written (the anchor is named by them) and the number.
struct FilePos {
    text: String,
    number: usize,
}

/// The offsets (in the text) of each `<mbp:pagebreak>` (`mbpPagebreakRegex`).
fn find_pagebreaks(t: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = t[i..].iter().position(|&c| c == b'<') {
        let at = i + rel;
        let mut q = at + 1;
        while t.get(q).is_some_and(u8::is_ascii_whitespace) {
            q += 1;
        }
        if t.get(q..).is_some_and(|r| starts_with_ci(r, b"mbp:")) {
            q += 4;
        }
        if t.get(q..).is_some_and(|r| starts_with_ci(r, b"pagebreak")) {
            if let Some(close) = t[q..].iter().position(|&c| c == b'>') {
                out.push(at);
                i = q + close + 1;
                continue;
            }
        }
        i = at + 1;
    }
    out
}

/// Every `filepos=` number in a tag (`fileposRegex`), once each, in order of position.
fn scan_fileposes(t: &[u8]) -> Vec<FilePos> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut found: Vec<FilePos> = Vec::new();
    let mut i = 0;
    while let Some(rel) = t[i..].iter().position(|&c| c == b'<') {
        let at = i + rel;
        let Some(close) = t[at + 1..].iter().position(|&c| c == b'<' || c == b'>') else {
            break;
        };
        let end = at + 1 + close;
        if t[end] == b'>' && end > at + 1 {
            let body = &t[at + 1..end];
            // The last `filepos=` followed by digits, as the greedy expression finds it.
            let mut from = 1;
            let mut hit = None;
            while let Some(p) = find_ci(body, b"filepos=", from) {
                let mut k = p + 8;
                if matches!(body.get(k), Some(b'"') | Some(b'\'')) {
                    k += 1;
                }
                let digits = digits_of(&body[k.min(body.len())..]);
                if !digits.is_empty() {
                    hit = Some(digits.to_string());
                }
                from = p + 1;
            }
            if let Some(text) = hit {
                if seen.insert(text.clone()) {
                    let number = text.parse().unwrap_or(usize::MAX);
                    found.push(FilePos { text, number });
                }
            }
        }
        i = end.max(at + 1);
    }
    found.sort_by_key(|f| f.number);
    found
}

/// Whether a stretch of an old book's HTML shows nothing: no text and no picture.
fn is_blank(t: &[u8]) -> bool {
    let mut i = 0;
    while i < t.len() {
        if t[i] == b'<' && is_tag_start(t, i) {
            let end = tag_end(t, i);
            let raw = &t[i..end];
            if starts_with_ci(&raw[1..], b"img") || find_ci(raw, b"recindex", 0).is_some() {
                return false;
            }
            i = end;
        } else {
            if t[i] > b' ' {
                return false;
            }
            i += 1;
        }
    }
    true
}

/// Where to cut a section that is too long: at a paragraph near `from` if there is one.
fn split_at(t: &[u8], from: usize) -> usize {
    let limit = (from + SPLIT_REACH).min(t.len());
    let mut i = from;
    while let Some(rel) = t[i.min(limit)..limit].iter().position(|&c| c == b'<') {
        let at = i.min(limit) + rel;
        let tail = &t[at + 1..];
        let named =
            |name: &[u8]| starts_with_ci(tail, name) && tail.get(name.len()).is_none_or(|c| !c.is_ascii_alphanumeric());
        if named(b"p") || named(b"br") || named(b"div") || named(b"h1") || named(b"h2") || named(b"h3") {
            return at;
        }
        i = at + 1;
    }
    from.min(t.len())
}

/// An old book: its text cut into chapters, and the `filepos`es its links use.
struct Old {
    core: Core,
    chapters: Vec<Range<usize>>,
    fileposes: Vec<FilePos>,
}

/// An entry of the contents page: how far it is indented, the `filepos` it goes to, and its words.
struct RawEntry {
    /// In half units: a paragraph is 3 (1.5), a quotation 4 (2).
    indent: u32,
    filepos: String,
    label: Vec<u8>,
}

struct Converted {
    xhtml: Vec<u8>,
    entries: Vec<RawEntry>,
}

impl Old {
    /// Cuts the text into chapters, finds the `filepos`es, and reads the guide and the contents page.
    fn open(core: Core) -> (Self, Parts) {
        let text = &core.text;
        let mut starts = vec![0usize];
        starts.extend(find_pagebreaks(text));
        starts.dedup();
        let mut pieces: Vec<Range<usize>> = Vec::new();
        for (k, &start) in starts.iter().enumerate() {
            let end = starts.get(k + 1).copied().unwrap_or(text.len());
            let mut from = start;
            while end - from > SPLIT + SPLIT_REACH {
                let cut = split_at(text, from + SPLIT);
                if cut <= from {
                    break;
                }
                pieces.push(from..cut);
                from = cut;
            }
            pieces.push(from..end);
        }
        // A blank section goes into the next one, so nothing is an empty page.
        let mut chapters: Vec<Range<usize>> = Vec::new();
        let mut carry: Option<usize> = None;
        for piece in pieces {
            let start = carry.take().unwrap_or(piece.start);
            if is_blank(&text[piece.clone()]) {
                carry = Some(start);
            } else {
                chapters.push(start..piece.end);
            }
        }
        match (carry, chapters.last_mut()) {
            (Some(_), Some(last)) => last.end = text.len(),
            (Some(start), None) => chapters.push(start..text.len()),
            _ => {}
        }
        let fileposes = scan_fileposes(text);
        let weights = chapters.iter().map(|r| (r.end - r.start).max(1) as u64).collect();
        let old = Old {
            core,
            chapters,
            fileposes,
        };
        let (toc, start) = old.guide();
        (old, Parts { weights, toc, start })
    }

    /// The chapter the byte at `number` of the text is in.
    fn chapter_of(&self, number: usize) -> Option<usize> {
        let ix = self.chapters.partition_point(|r| r.end <= number);
        (ix < self.chapters.len()).then_some(ix)
    }

    /// The guide (`<reference>`s in the first section's head): where the contents page is, and where reading begins.
    fn guide(&self) -> (Vec<TocItem>, usize) {
        let text = &self.core.text;
        let head_end = find_ci(text, b"</head", 0).unwrap_or(text.len().min(64 * 1024));
        let head = &text[..head_end.min(text.len())];
        let mut references: Vec<(Vec<String>, usize)> = Vec::new();
        let mut i = 0;
        while let Some(rel) = head[i..].iter().position(|&c| c == b'<') {
            let at = i + rel;
            if is_tag_start(head, at) {
                let end = tag_end(head, at);
                if let Some(tag) = parse_tag(&head[at..end]).filter(|t| t.name == "reference" && !t.closing) {
                    let words = tag
                        .attr("type")
                        .map(|v| {
                            String::from_utf8_lossy(v)
                                .split_whitespace()
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(number) = tag.attr("filepos").and_then(|v| digits_of(v).parse::<usize>().ok()) {
                        references.push((words, number));
                    }
                }
                i = end;
            } else {
                i = at + 1;
            }
        }
        let start = references
            .iter()
            .find(|(words, _)| words.iter().any(|w| w == "bodymatter" || w == "text"))
            .and_then(|(_, number)| self.chapter_of(*number))
            .unwrap_or(0);
        let toc = references
            .iter()
            .find(|(words, _)| words.iter().any(|w| w == "toc"))
            .and_then(|(_, number)| self.chapter_of(*number))
            .and_then(|ix| self.convert(ix, true))
            .map(|converted| self.toc_of(converted.entries))
            .unwrap_or_default();
        (toc, start)
    }

    /// The contents page's links as a tree, nested by how far each is indented (foliate's `getIndent` and levels).
    fn toc_of(&self, entries: Vec<RawEntry>) -> Vec<TocItem> {
        struct Node {
            item: TocItem,
            children: Vec<usize>,
        }
        let mut nodes: Vec<Node> = Vec::new();
        let mut roots: Vec<usize> = Vec::new();
        let mut last_item: Option<usize> = None;
        let mut last_level = 0u32;
        let mut last_indent = 0u32;
        let mut level_of_indent: HashMap<u32, u32> = HashMap::new();
        let mut parent_of_level: HashMap<u32, usize> = HashMap::new();
        for entry in entries {
            let label = entry_label(&self.core, &entry.label);
            let Some(chapter) = entry.filepos.parse::<usize>().ok().and_then(|n| self.chapter_of(n)) else {
                continue;
            };
            if label.is_empty() {
                continue;
            }
            let level = if entry.indent > last_indent {
                last_level + 1
            } else if entry.indent == last_indent {
                last_level
            } else {
                level_of_indent
                    .get(&entry.indent)
                    .copied()
                    .unwrap_or_else(|| last_level.saturating_sub(1))
            };
            let id = nodes.len();
            nodes.push(Node {
                item: TocItem {
                    label,
                    chapter,
                    anchor: Some(format!("filepos{}", entry.filepos)),
                    children: Vec::new(),
                },
                children: Vec::new(),
            });
            if level > last_level {
                match last_item {
                    Some(parent) => {
                        nodes[parent].children.push(id);
                        parent_of_level.insert(level, parent);
                    }
                    None => roots.push(id),
                }
            } else {
                match parent_of_level.get(&level) {
                    Some(&parent) => nodes[parent].children.push(id),
                    None => roots.push(id),
                }
            }
            last_item = Some(id);
            last_level = level;
            last_indent = entry.indent;
            level_of_indent.insert(entry.indent, level);
        }
        fn build(nodes: &[Node], id: usize, depth: usize) -> TocItem {
            let mut item = nodes[id].item.clone();
            if depth < 32 {
                item.children = nodes[id].children.iter().map(|c| build(nodes, *c, depth + 1)).collect();
            }
            item
        }
        roots.iter().map(|r| build(&nodes, *r, 0)).collect()
    }

    /// Chapter `ix` as XHTML (in the book's encoding). With `collect`, also the contents page's links.
    fn convert(&self, ix: usize, collect: bool) -> Option<Converted> {
        let range = self.chapters.get(ix)?.clone();
        let input = self.core.text.get(range.clone())?;
        let lo = self.fileposes.partition_point(|f| f.number < range.start);
        let hi = self.fileposes.partition_point(|f| f.number < range.end);
        let mut converter = Converter {
            old: self,
            out: Vec::with_capacity(input.len() + input.len() / 4 + 64),
            stack: Vec::new(),
            dropped: HashMap::new(),
            in_head: false,
            collect,
            entries: Vec::new(),
            open_entry: None,
            images: HashMap::new(),
            anchors: &self.fileposes[lo..hi],
            next_anchor: 0,
            base: range.start,
            skip: None,
        };
        converter.run(input);
        Some(Converted {
            xhtml: converter.out,
            entries: converter.entries,
        })
    }
}

/// A contents entry's words: its bytes decoded, entities undone, white space collapsed.
fn entry_label(core: &Core, bytes: &[u8]) -> String {
    let text = unescape_html(&core.decode(bytes));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Makes one chapter of an old book well-formed XHTML: names in lower case, attributes quoted, void elements closed,
/// elements closed where HTML would close them, the `mbp:` tags dropped, an anchor where each `filepos` points, a
/// link where a `filepos` is, a picture where a `recindex` is.
struct Converter<'a> {
    old: &'a Old,
    out: Vec<u8>,
    stack: Vec<String>,
    /// Inline start tags left out for nesting too deep, by name, so their end tags are left out too.
    dropped: HashMap<String, usize>,
    in_head: bool,
    collect: bool,
    entries: Vec<RawEntry>,
    open_entry: Option<usize>,
    images: HashMap<usize, Option<String>>,
    anchors: &'a [FilePos],
    next_anchor: usize,
    base: usize,
    /// A `<script>` or `<style>` just opened: its contents are skipped up to its end tag.
    skip: Option<String>,
}

impl Converter<'_> {
    fn run(&mut self, input: &[u8]) {
        self.out.extend_from_slice(b"<html><body>");
        let mut i = 0;
        while i < input.len() {
            if input[i] == b'<' && is_tag_start(input, i) {
                let end = tag_end(input, i);
                self.flush_anchors(i);
                self.tag(&input[i..end]);
                i = end;
                if let Some(name) = self.skip.take() {
                    let close = format!("</{name}");
                    i = match find_ci(input, close.as_bytes(), i) {
                        Some(at) => tag_end(input, at),
                        None => input.len(),
                    };
                }
            } else {
                let mut j = i + 1;
                while j < input.len() && !(input[j] == b'<' && is_tag_start(input, j)) {
                    j += 1;
                }
                self.text(&input[i..j], i);
                i = j;
            }
        }
        self.flush_anchors(usize::MAX);
        self.pop_to(0);
        self.out.extend_from_slice(b"</body></html>");
    }

    /// Where, in this chapter, the next anchor goes.
    fn next_offset(&self) -> Option<usize> {
        self.anchors
            .get(self.next_anchor)
            .map(|f| f.number.saturating_sub(self.base))
    }

    fn emit_anchor(&mut self) {
        if let Some(f) = self.anchors.get(self.next_anchor) {
            self.out.extend_from_slice(b"<a id=\"filepos");
            self.out.extend_from_slice(f.text.as_bytes());
            self.out.extend_from_slice(b"\"></a>");
            self.next_anchor += 1;
        }
    }

    /// Emits the anchors that are at or before `upto`.
    fn flush_anchors(&mut self, upto: usize) {
        while self.next_offset().is_some_and(|off| off <= upto) {
            self.emit_anchor();
        }
    }

    /// Text starting `at` bytes into the chapter; anchors that fall inside it split it.
    fn text(&mut self, bytes: &[u8], at: usize) {
        let mut from = 0;
        while let Some(off) = self.next_offset() {
            if off >= at + bytes.len() {
                break;
            }
            let cut = off.saturating_sub(at).max(from);
            self.raw_text(&bytes[from..cut]);
            self.emit_anchor();
            from = cut;
        }
        self.raw_text(&bytes[from..]);
    }

    fn raw_text(&mut self, bytes: &[u8]) {
        if self.in_head || bytes.is_empty() {
            return;
        }
        if let Some(entry) = self.open_entry.and_then(|e| self.entries.get_mut(e)) {
            entry.label.extend_from_slice(bytes);
        }
        for &c in bytes {
            match c {
                b'<' => self.out.extend_from_slice(b"&lt;"),
                c if c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r') => {}
                c => self.out.push(c),
            }
        }
    }

    /// Ends elements down to `len` open ones.
    fn pop_to(&mut self, len: usize) {
        while self.stack.len() > len {
            let Some(name) = self.stack.pop() else { break };
            self.out.extend_from_slice(b"</");
            self.out.extend_from_slice(name.as_bytes());
            self.out.push(b'>');
            if name == "a" {
                self.open_entry = None;
            }
        }
    }

    fn tag(&mut self, raw: &[u8]) {
        let Some(tag) = parse_tag(raw) else { return };
        let name = tag.name.as_str();
        if name == "pagebreak" || name.contains(':') || !valid_name(name) {
            return;
        }
        if tag.closing {
            match name {
                "head" => self.in_head = false,
                "html" | "body" => {}
                _ if VOID.contains(&name) => {}
                _ => {
                    if let Some(pos) = self.stack.iter().rposition(|n| n == name) {
                        self.pop_to(pos);
                    } else if let Some(count) = self.dropped.get_mut(name) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
            return;
        }
        match name {
            "head" => {
                self.in_head = !tag.self_closing;
                return;
            }
            "body" => {
                self.in_head = false;
                return;
            }
            "html" => return,
            _ => {}
        }
        if self.in_head {
            return;
        }
        if name == "script" || name == "style" {
            if !tag.self_closing {
                self.skip = Some(name.to_string());
            }
            return;
        }
        let close = closes_for(&self.stack, name);
        self.pop_to(self.stack.len() - close);
        let void = VOID.contains(&name);
        if !void && !tag.self_closing && self.stack.len() >= MAX_NEST && INLINE.contains(&name) {
            *self.dropped.entry(name.to_string()).or_insert(0) += 1;
            return;
        }
        if name == "a" && self.collect {
            if let Some(filepos) = tag
                .attr("filepos")
                .map(|v| digits_of(v).to_string())
                .filter(|d| !d.is_empty())
            {
                let indent = self
                    .stack
                    .iter()
                    .map(|n| match n.as_str() {
                        "p" => 3,
                        "blockquote" => 4,
                        _ => 0,
                    })
                    .sum();
                self.entries.push(RawEntry {
                    indent,
                    filepos,
                    label: Vec::new(),
                });
                self.open_entry = Some(self.entries.len() - 1);
            }
        }
        self.write_start(&tag, void || tag.self_closing);
        if !(void || tag.self_closing) {
            self.stack.push(name.to_string());
        } else if name == "a" {
            self.open_entry = None;
        }
    }

    /// A picture of the book as a `data:` address, kept for the rest of the chapter.
    fn image(&mut self, index: usize) -> Option<String> {
        let old = self.old;
        self.images
            .entry(index)
            .or_insert_with(|| old.core.image_uri(index))
            .clone()
    }

    fn write_start(&mut self, tag: &Tag, closed: bool) {
        let recindex = tag
            .attr("recindex")
            .and_then(|v| digits_of(v).parse::<usize>().ok())
            .filter(|_| tag.name == "img");
        let src = recindex.and_then(|n| n.checked_sub(1)).and_then(|i| self.image(i));
        let href = (tag.name == "a")
            .then(|| tag.attr("filepos"))
            .flatten()
            .map(digits_of)
            .filter(|d| !d.is_empty())
            .and_then(|d| {
                let chapter = d.parse::<usize>().ok().and_then(|n| self.old.chapter_of(n))?;
                Some(format!("{}#filepos{d}", part_name(chapter)))
            });
        let has_href = tag.name == "a" && tag.attr("filepos").is_some();
        self.out.push(b'<');
        self.out.extend_from_slice(tag.name.as_bytes());
        let mut seen: Vec<&str> = Vec::new();
        for (key, value) in &tag.attrs {
            let skipped = matches!(key.as_str(), "filepos" | "recindex" | "mediarecindex")
                || (has_href && key == "href")
                || (recindex.is_some() && key == "src");
            if skipped || !valid_name(key) || seen.contains(&key.as_str()) {
                continue;
            }
            seen.push(key);
            self.out.push(b' ');
            self.out.extend_from_slice(key.as_bytes());
            self.out.extend_from_slice(b"=\"");
            escape_attr(&mut self.out, value);
            self.out.push(b'"');
        }
        if let Some(uri) = src {
            self.out.extend_from_slice(b" src=\"");
            self.out.extend_from_slice(uri.as_bytes());
            self.out.push(b'"');
        }
        if let Some(href) = href {
            self.out.extend_from_slice(b" href=\"");
            self.out.extend_from_slice(href.as_bytes());
            self.out.push(b'"');
        }
        self.out.extend_from_slice(if closed { b"/>" } else { b">" });
    }
}

// ---- KF8 -------------------------------------------------------------------------------------------------------

/// One skeleton of the skeleton table: a part's document with holes (`skelTable`).
struct Skel {
    offset: usize,
    length: usize,
}

/// One fragment of the fragment table: text put into a skeleton (`fragTable`).
struct Frag {
    /// Where in the skeleton (counted from the skeleton's start in the text) it goes.
    insert_offset: usize,
    /// The fragment's number, which `kindle:pos:fid:` links name.
    index: u32,
    offset: usize,
    length: usize,
}

/// A chapter of a KF8 book: a skeleton and its fragments (`#sections`).
struct Part {
    skel: Skel,
    frags: Vec<Frag>,
    /// The skeleton's length and its fragments'.
    length: usize,
}

/// Where a `kindle:pos` link goes: a chapter, and the id inside it.
#[derive(Clone, Debug)]
struct Pos {
    part: usize,
    /// `id` or `name` as written, or `aid-` and an `aid`.
    fragment: Option<String>,
    /// The `aid` it is, when that names it: the chapter marks it with an anchor.
    aid: Option<String>,
}

/// A KF8 book (an AZW3, or the KF8 half of a combined file).
struct Kf8 {
    core: Core,
    parts: Vec<Part>,
    /// The flows of the text (FDST): the main text, style sheets, SVG.
    flows: Vec<Range<usize>>,
    /// The places links and contents entries go to, worked out when the book was opened.
    resolved: HashMap<(u32, u32), Pos>,
    /// By chapter: the `aid`s some link goes to.
    aids: Vec<HashSet<String>>,
}

/// `getFragmentSelector`: the first `id`, `name` or `aid` attribute in `text`, and its value.
fn first_selector(text: &[u8]) -> Option<(&'static str, String)> {
    let mut i = 0;
    while i < text.len() {
        if text[i].is_ascii_whitespace() {
            for name in ["id", "name", "aid"] {
                let rest = &text[i + 1..];
                if !starts_with_ci(rest, name.as_bytes()) {
                    continue;
                }
                let mut k = name.len();
                let skip_ws = |k: &mut usize| {
                    while rest.get(*k).is_some_and(u8::is_ascii_whitespace) {
                        *k += 1;
                    }
                };
                skip_ws(&mut k);
                if rest.get(k) != Some(&b'=') {
                    continue;
                }
                k += 1;
                skip_ws(&mut k);
                if !matches!(rest.get(k), Some(b'"') | Some(b'\'')) {
                    continue;
                }
                k += 1;
                let len = rest[k..].iter().position(|&c| c == b'"' || c == b'\'')?;
                return Some((name, String::from_utf8_lossy(&rest[k..k + len]).into_owned()));
            }
        }
        i += 1;
    }
    None
}

/// The value of attribute `name` in a start tag, if it has one.
fn tag_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let b = tag.as_bytes();
    let mut i = 1;
    while i < b.len() {
        if b[i].is_ascii_whitespace() && starts_with_ci(&b[i + 1..], name.as_bytes()) {
            let mut k = i + 1 + name.len();
            while b.get(k).is_some_and(u8::is_ascii_whitespace) {
                k += 1;
            }
            if b.get(k) == Some(&b'=') {
                k += 1;
                while b.get(k).is_some_and(u8::is_ascii_whitespace) {
                    k += 1;
                }
                if let Some(&q) = b.get(k).filter(|c| **c == b'"' || **c == b'\'') {
                    let from = k + 1;
                    let len = b[from..].iter().position(|&c| c == q)?;
                    return tag.get(from..from + len);
                }
            }
        }
        i += 1;
    }
    None
}

/// A `kindle:pos:fid:XXXX:off:YYYYYYYYYY` address at the start of `text`: the numbers (base 32) and its length.
fn parse_pos(text: &str) -> Option<(u32, u32, usize)> {
    let rest = text.strip_prefix("kindle:pos:fid:")?;
    let word = |s: &str| {
        s.bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
            .count()
    };
    let fid_len = word(rest);
    let fid = base32(&rest[..fid_len])?;
    let after = rest[fid_len..].strip_prefix(":off:")?;
    let off_len = word(after);
    let off = base32(&after[..off_len])?;
    Some((fid, off, "kindle:pos:fid:".len() + fid_len + ":off:".len() + off_len))
}

/// A `kindle:embed:XXXX` or `kindle:flow:XXXX` address with its optional `?mime=type/subtype`: the kind, the number,
/// the type, and the length.
fn parse_resource(text: &str) -> Option<(&str, u32, Option<&str>, usize)> {
    let (kind, rest) = if let Some(r) = text.strip_prefix("kindle:embed:") {
        ("embed", r)
    } else {
        ("flow", text.strip_prefix("kindle:flow:")?)
    };
    let id_len = rest
        .bytes()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
        .count();
    let id = base32(&rest[..id_len])?;
    let mut used = if kind == "embed" {
        "kindle:embed:".len()
    } else {
        "kindle:flow:".len()
    } + id_len;
    let mut mime = None;
    if let Some(after) = rest[id_len..].strip_prefix("?mime=") {
        let slash = after
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
            .count();
        if slash > 0 && after[slash..].starts_with('/') {
            let sub = after[slash + 1..]
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'+' | b'.'))
                .count();
            if sub > 0 {
                mime = Some(&after[..slash + 1 + sub]);
                used += "?mime=".len() + slash + 1 + sub;
            }
        }
    }
    Some((kind, id, mime, used))
}

impl Kf8 {
    /// `KF8.init`: the flows, the skeleton and fragment tables, the contents and the guide. None when a table the
    /// parts are built from is missing or damaged.
    fn open(core: Core, h: &Headers, kf8: Kf8Header, start: usize) -> Option<(Self, Parts)> {
        // The FDST: where each flow of the text starts and ends. foliate carried on without one.
        let mut flows: Vec<Range<usize>> = Vec::new();
        if let Some(fdst) = core.pdb.record(start.checked_add(kf8.fdst as usize)?, MAX_RECORD) {
            if fdst.get(..4) == Some(b"FDST") {
                let entries = u32_at(&fdst, 8).unwrap_or(0) as usize;
                for i in 0..entries.min(fdst.len() / 8) {
                    let at = 12 + i * 8;
                    if let (Some(s), Some(e)) = (u32_at(&fdst, at), u32_at(&fdst, at + 4)) {
                        let end = (e as usize).min(core.text.len());
                        flows.push((s as usize).min(end)..end);
                    }
                }
            }
        }

        let skels = get_index_data(&core.pdb, start, kf8.skel)?;
        let skels: Vec<(usize, Skel)> = skels
            .table
            .iter()
            .map(|entry| {
                let num_frag = *entry.tags.get(&1)?.first()? as usize;
                let six = entry.tags.get(&6)?;
                Some((
                    num_frag,
                    Skel {
                        offset: *six.first()? as usize,
                        length: *six.get(1)? as usize,
                    },
                ))
            })
            .collect::<Option<_>>()?;
        let frag_index = get_index_data(&core.pdb, start, kf8.frag)?;
        let frags: Vec<Frag> = frag_index
            .table
            .iter()
            .map(|entry| {
                let six = entry.tags.get(&6)?;
                Some(Frag {
                    insert_offset: std::str::from_utf8(&entry.name).ok()?.trim().parse().ok()?,
                    index: *entry.tags.get(&4)?.first()?,
                    offset: *six.first()? as usize,
                    length: *six.get(1)? as usize,
                })
            })
            .collect::<Option<_>>()?;

        // A skeleton takes the next `numFrag` fragments. One with none is not a chapter (foliate: `linear: 'no'`).
        let mut parts: Vec<Part> = Vec::new();
        let mut frag_iter = frags.into_iter();
        for (num_frag, skel) in skels {
            let mine: Vec<Frag> = frag_iter.by_ref().take(num_frag).collect();
            if mine.is_empty() {
                continue;
            }
            let length = skel.length + mine.iter().map(|f| f.length).sum::<usize>();
            parts.push(Part {
                skel,
                frags: mine,
                length,
            });
        }
        if parts.is_empty() {
            return None;
        }

        // The contents and the guide, and every place a link in the text goes.
        let ncx = (h.indx < NONE)
            .then(|| get_ncx(&core.pdb, start, h.indx))
            .flatten()
            .unwrap_or_default();
        let guide: Vec<(Vec<String>, u32)> = (kf8.guide < NONE)
            .then(|| get_index_data(&core.pdb, start, kf8.guide))
            .flatten()
            .map(|index| {
                index
                    .table
                    .iter()
                    .filter_map(|entry| {
                        let fid = entry
                            .tags
                            .get(&6)
                            .and_then(|v| v.first())
                            .or_else(|| entry.tags.get(&3).and_then(|v| v.first()))
                            .copied()?;
                        let words = String::from_utf8_lossy(&entry.name)
                            .split_whitespace()
                            .map(str::to_string)
                            .collect();
                        Some((words, fid))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut keys: HashSet<(u32, u32)> = HashSet::new();
        keys.extend(ncx.iter().filter_map(|n| n.pos));
        keys.extend(guide.iter().map(|(_, fid)| (*fid, 0)));
        let main = flows.first().cloned().unwrap_or(0..core.text.len());
        if let Some(main_text) = core.text.get(main) {
            let mut i = 0;
            let needle = b"kindle:pos:fid:";
            while let Some(at) =
                (i..main_text.len().saturating_sub(needle.len() - 1)).find(|&k| main_text[k..].starts_with(needle))
            {
                let tail = String::from_utf8_lossy(&main_text[at..(at + 48).min(main_text.len())]).into_owned();
                if let Some((fid, off, used)) = parse_pos(&tail) {
                    keys.insert((fid, off));
                    i = at + used;
                } else {
                    i = at + needle.len();
                }
            }
        }
        // Which chapter, and which of its fragments, each fragment number is (the first, if a number is used twice).
        let mut fragments: HashMap<u32, (usize, usize)> = HashMap::new();
        for (part_ix, part) in parts.iter().enumerate() {
            for (frag_ix, frag) in part.frags.iter().enumerate() {
                fragments.entry(frag.index).or_insert((part_ix, frag_ix));
            }
        }
        let mut resolved: HashMap<(u32, u32), Pos> = HashMap::new();
        let mut aids: Vec<HashSet<String>> = vec![HashSet::new(); parts.len()];
        for (fid, off) in keys {
            if let Some(pos) = resolve_pos(&core, &parts, &fragments, fid, off) {
                if let Some(aid) = &pos.aid {
                    aids[pos.part].insert(aid.clone());
                }
                resolved.insert((fid, off), pos);
            }
        }

        let toc = {
            let roots = ncx.iter().filter(|n| n.level == Some(0));
            roots.flat_map(|n| ncx_item(&ncx, n, 0, &resolved)).collect()
        };
        let start_part = guide
            .iter()
            .find(|(words, _)| words.iter().any(|w| w == "bodymatter" || w == "text"))
            .and_then(|(_, fid)| resolved.get(&(*fid, 0)))
            .map_or(0, |pos| pos.part);
        let weights = parts.iter().map(|p| p.length.max(1) as u64).collect();
        let kf8 = Kf8 {
            core,
            parts,
            flows,
            resolved,
            aids,
        };
        Some((
            kf8,
            Parts {
                weights,
                toc,
                start: start_part,
            },
        ))
    }

    /// Part `ix` as it is in the text: the skeleton with its fragments put in (`loadText`).
    fn part_text(&self, ix: usize) -> Option<Vec<u8>> {
        let part = self.parts.get(ix)?;
        let text = &self.core.text;
        let from = part.skel.offset.min(text.len());
        let to = part.skel.offset.saturating_add(part.length).min(text.len());
        let raw = &text[from..to];
        let mut doc = raw[..part.skel.length.min(raw.len())].to_vec();
        for frag in &part.frags {
            let insert = frag.insert_offset.saturating_sub(part.skel.offset).min(doc.len());
            let start = part.skel.length.saturating_add(frag.offset).min(raw.len());
            let end = start.saturating_add(frag.length).min(raw.len());
            doc.splice(insert..insert, raw[start..end].iter().copied());
        }
        Some(doc)
    }

    /// Part `ix` as XHTML, its `kindle:` addresses made into links, pictures and styles.
    fn part_xhtml(&self, ix: usize) -> Option<String> {
        let text = self.core.decode(&self.part_text(ix)?);
        Some(self.transform(ix, &text))
    }

    /// The style sheet a `<link>` to a flow names, as text.
    fn flow_text(&self, id: u32) -> Option<String> {
        let range = self.flows.get(id as usize)?;
        Some(self.core.decode(self.core.text.get(range.clone())?))
    }

    /// Makes part `ix`'s text work as a chapter: a `<link>` to a style flow becomes the style; `kindle:pos` becomes
    /// `partNNNN.html#id`; `kindle:embed` and picture flows become `data:` addresses; an element some link goes to by
    /// its `aid` gets an anchor.
    fn transform(&self, ix: usize, text: &str) -> String {
        let b = text.as_bytes();
        let targets = self.aids.get(ix);
        let mut out = String::with_capacity(text.len() + 256);
        let mut i = 0;
        while let Some(rel) = text[i..].find('<') {
            let at = i + rel;
            out.push_str(&text[i..at]);
            if !is_tag_start(b, at) {
                out.push_str("&lt;");
                i = at + 1;
                continue;
            }
            let end = tag_end(b, at).min(text.len());
            let tag = &text[at..end];
            i = end;
            if tag.starts_with("<!") || tag.starts_with("<?") {
                out.push_str(tag);
                continue;
            }
            if starts_with_ci(tag.as_bytes(), b"<link") && tag.contains("kindle:flow:") {
                let css = tag
                    .find("kindle:flow:")
                    .and_then(|k| parse_resource(&tag[k..]))
                    .filter(|(_, _, mime, _)| mime.is_none_or(|m| m == "text/css"))
                    .and_then(|(_, id, _, _)| self.flow_text(id));
                if let Some(css) = css {
                    out.push_str("<style type=\"text/css\"><![CDATA[");
                    out.push_str(&css.replace("]]>", "] ]>"));
                    out.push_str("]]></style>");
                }
                continue;
            }
            if !tag.starts_with("</") {
                if let Some(aid) = tag_attr(tag, "aid").filter(|a| targets.is_some_and(|t| t.contains(*a))) {
                    out.push_str("<a id=\"aid-");
                    out.push_str(aid);
                    out.push_str("\"></a>");
                }
            }
            self.replace_addresses(tag, &mut out);
        }
        out.push_str(&text[i..]);
        out
    }

    /// Writes `tag` with its `kindle:` addresses replaced.
    fn replace_addresses(&self, tag: &str, out: &mut String) {
        let mut rest = tag;
        while let Some(at) = rest.find("kindle:") {
            out.push_str(&rest[..at]);
            rest = &rest[at..];
            if let Some((fid, off, used)) = parse_pos(rest) {
                if let Some(pos) = self.resolved.get(&(fid, off)) {
                    out.push_str(&part_name(pos.part));
                    if let Some(fragment) = &pos.fragment {
                        out.push('#');
                        out.push_str(fragment);
                    }
                }
                rest = &rest[used..];
            } else if let Some((kind, id, mime, used)) = parse_resource(rest) {
                let uri = match (kind, mime) {
                    ("embed", _) => id.checked_sub(1).and_then(|i| self.core.image_uri(i as usize)),
                    ("flow", Some("image/svg+xml")) => self
                        .flows
                        .get(id as usize)
                        .and_then(|r| self.core.text.get(r.clone()))
                        .map(|svg| format!("data:image/svg+xml;base64,{}", base64_encode(svg))),
                    _ => None,
                };
                out.push_str(&uri.unwrap_or_default());
                rest = &rest[used..];
            } else {
                out.push_str("kindle:");
                rest = &rest["kindle:".len()..];
            }
        }
        out.push_str(rest);
    }
}

/// Where fragment `fid` at `off` is (`resolveHref`): its chapter, and the first `id`, `name` or `aid` at or after the
/// offset in the fragment's text.
fn resolve_pos(
    core: &Core,
    parts: &[Part],
    fragments: &HashMap<u32, (usize, usize)>,
    fid: u32,
    off: u32,
) -> Option<Pos> {
    let &(part_ix, frag_ix) = fragments.get(&fid)?;
    let part = parts.get(part_ix)?;
    let frag = part.frags.get(frag_ix)?;
    let start = part
        .skel
        .offset
        .saturating_add(part.skel.length)
        .saturating_add(frag.offset);
    let end = start.saturating_add(frag.length).min(core.text.len());
    let raw = core.text.get(start.min(end)..end).unwrap_or(&[]);
    let rest = raw.get((off as usize).min(raw.len())..).unwrap_or(&[]);
    let (fragment, aid) = match first_selector(rest) {
        Some(("aid", value)) => (Some(format!("aid-{value}")), Some(value)),
        Some((_, value)) => (Some(value), None),
        None => (None, None),
    };
    Some(Pos {
        part: part_ix,
        fragment,
        aid,
    })
}

/// A contents entry of the NCX, and under it the entries whose parent it is. An entry that goes nowhere hands its
/// children up.
fn ncx_item(items: &[Ncx], item: &Ncx, depth: usize, resolved: &HashMap<(u32, u32), Pos>) -> Vec<TocItem> {
    let children: Vec<TocItem> = if item.first_child.is_some() && depth < 32 {
        items
            .iter()
            .filter(|x| x.parent.is_some_and(|p| p as usize == item.index))
            .flat_map(|x| ncx_item(items, x, depth + 1, resolved))
            .collect()
    } else {
        Vec::new()
    };
    let label = unescape_html(&item.label).trim().to_string();
    match item.pos.and_then(|key| resolved.get(&key)) {
        Some(pos) if !label.is_empty() => vec![TocItem {
            label,
            chapter: pos.part,
            anchor: pos.fragment.clone(),
            children,
        }],
        _ => children,
    }
}

#[cfg(test)]
mod tests {
    use super::super::book::open_book;
    use super::super::rich::{Align, Block, Document, TEST_PNG};
    use super::*;

    // ---- Writing books ----------------------------------------------------------------------------------------

    fn put16(b: &mut [u8], at: usize, v: usize) {
        b[at..at + 2].copy_from_slice(&(v as u16).to_be_bytes());
    }

    fn put32(b: &mut [u8], at: usize, v: usize) {
        b[at..at + 4].copy_from_slice(&(v as u32).to_be_bytes());
    }

    /// What record 0 says.
    #[derive(Clone, Default)]
    struct Rec0<'a> {
        version: usize,
        compression: usize,
        text_len: usize,
        text_records: usize,
        encryption: usize,
        encoding: usize,
        uid: usize,
        resource_start: usize,
        huffcdic: usize,
        num_huffcdic: usize,
        trailing: usize,
        indx: usize,
        /// FDST, fragment, skeleton and guide records (KF8).
        kf8: [usize; 4],
        title: &'a str,
        exth: Vec<(u32, Vec<u8>)>,
        /// The locale bytes: language, region.
        lang: (usize, usize),
    }

    fn record0(r: &Rec0) -> Vec<u8> {
        let header_len = if r.version >= 8 { 264 } else { 232 };
        let mut exth = Vec::new();
        if !r.exth.is_empty() {
            let mut body = Vec::new();
            for (kind, data) in &r.exth {
                body.extend(kind.to_be_bytes());
                body.extend(((data.len() + 8) as u32).to_be_bytes());
                body.extend(data);
            }
            exth.extend(b"EXTH");
            exth.extend(((12 + body.len()) as u32).to_be_bytes());
            exth.extend((r.exth.len() as u32).to_be_bytes());
            exth.extend(body);
            while exth.len() % 4 != 0 {
                exth.push(0);
            }
        }
        let title_offset = 16 + header_len + exth.len();
        let mut rec = vec![0u8; 16 + header_len];
        put16(&mut rec, 0, r.compression);
        put32(&mut rec, 4, r.text_len);
        put16(&mut rec, 8, r.text_records);
        put16(&mut rec, 10, 4096);
        put16(&mut rec, 12, r.encryption);
        rec[16..20].copy_from_slice(b"MOBI");
        put32(&mut rec, 20, header_len);
        put32(&mut rec, 24, 2);
        put32(&mut rec, 28, r.encoding);
        put32(&mut rec, 32, r.uid);
        put32(&mut rec, 36, r.version);
        put32(&mut rec, 84, title_offset);
        put32(&mut rec, 88, r.title.len());
        rec[94] = r.lang.1 as u8;
        rec[95] = r.lang.0 as u8;
        put32(&mut rec, 108, r.resource_start);
        put32(&mut rec, 112, r.huffcdic);
        put32(&mut rec, 116, r.num_huffcdic);
        put32(&mut rec, 128, if exth.is_empty() { 0 } else { 0x40 });
        put32(&mut rec, 240, r.trailing);
        put32(&mut rec, 244, if r.indx == 0 { NONE as usize } else { r.indx });
        if r.version >= 8 {
            put32(&mut rec, 192, r.kf8[0]);
            put32(&mut rec, 248, r.kf8[1]);
            put32(&mut rec, 252, r.kf8[2]);
            put32(&mut rec, 260, r.kf8[3]);
        }
        rec.extend(exth);
        rec.extend(r.title.as_bytes());
        rec.extend([0u8; 4]);
        rec
    }

    /// A Palm database of `BOOKMOBI` type holding `records`.
    fn pdb(records: &[Vec<u8>]) -> Vec<u8> {
        let n = records.len();
        let mut out = vec![0u8; 78];
        out[..4].copy_from_slice(b"Test");
        out[60..68].copy_from_slice(b"BOOKMOBI");
        put16(&mut out, 76, n);
        let mut offset = 78 + n * 8 + 2;
        for record in records {
            out.extend((offset as u32).to_be_bytes());
            out.extend([0u8; 4]);
            offset += record.len();
        }
        out.extend([0u8, 0]);
        for record in records {
            out.extend(record);
        }
        out
    }

    /// A PalmDOC compressor, as simple as it can be: back references, space-and-letter pairs, literals, runs.
    fn palmdoc_compress(data: &[u8]) -> Vec<u8> {
        let literal = |b: u8| b == 0 || (9..=0x7f).contains(&b);
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let mut best = (0usize, 0usize);
            if i + 3 <= data.len() {
                for start in i.saturating_sub(2047)..i {
                    let mut len = 0;
                    while len < 10 && i + len < data.len() && data[start + len] == data[i + len] {
                        len += 1;
                    }
                    if len > best.1 {
                        best = (i - start, len);
                    }
                }
            }
            if best.1 >= 3 {
                let pair = 0x8000u16 | ((best.0 as u16) << 3) | ((best.1 - 3) as u16);
                out.extend(pair.to_be_bytes());
                i += best.1;
            } else if data[i] == b' ' && data.get(i + 1).is_some_and(|c| (0x40..0x80).contains(c)) {
                out.push(data[i + 1] ^ 0x80);
                i += 2;
            } else if literal(data[i]) {
                out.push(data[i]);
                i += 1;
            } else {
                let mut j = i;
                while j < data.len() && j - i < 8 && !literal(data[j]) {
                    j += 1;
                }
                out.push((j - i) as u8);
                out.extend(&data[i..j]);
                i = j;
            }
        }
        out
    }

    /// What an old book is made of.
    #[derive(Clone, Default)]
    struct OldSpec<'a> {
        text: &'a [u8],
        /// 1, or 2 for PalmDOC.
        compression: usize,
        trailing: bool,
        encoding: usize,
        title: &'a str,
        exth: Vec<(u32, Vec<u8>)>,
        images: Vec<Vec<u8>>,
        lang: (usize, usize),
        encryption: usize,
    }

    fn old_book(spec: &OldSpec) -> Vec<u8> {
        let mut records = vec![Vec::new()];
        let chunks: Vec<&[u8]> = spec.text.chunks(4096).collect();
        for chunk in &chunks {
            let mut record = if spec.compression == 2 {
                palmdoc_compress(chunk)
            } else {
                chunk.to_vec()
            };
            if spec.trailing {
                // The bytes of a character cut at the end (the last says it is two long), then one trailing
                // entry of four bytes.
                record.extend([0xaa, 0x01]);
                record.extend([1, 2, 3, 0x84]);
            }
            records.push(record);
        }
        let first_image = records.len();
        records.extend(spec.images.iter().cloned());
        records[0] = record0(&Rec0 {
            version: 6,
            compression: spec.compression.max(1),
            text_len: spec.text.len(),
            text_records: chunks.len(),
            encryption: spec.encryption,
            encoding: if spec.encoding == 0 { 65001 } else { spec.encoding },
            uid: 77,
            resource_start: first_image,
            trailing: if spec.trailing { 0b11 } else { 0 },
            title: spec.title,
            exth: spec.exth.clone(),
            lang: spec.lang,
            ..Default::default()
        });
        pdb(&records)
    }

    fn simple(text: &str) -> Vec<u8> {
        old_book(&OldSpec {
            text: text.as_bytes(),
            title: "Simple",
            ..Default::default()
        })
    }

    /// The document of chapter `ix`.
    fn chapter(book: &Book, ix: usize) -> Arc<Document> {
        match book.chapter(ix) {
            Ok(Chapter::Text(doc)) => doc,
            Ok(Chapter::Page(_)) => panic!("a comic page"),
            Err(reason) => panic!("chapter {ix}: {reason}"),
        }
    }

    fn lines(doc: &Document) -> Vec<String> {
        doc.plain_text().lines().map(str::to_string).collect()
    }

    fn links(doc: &Document) -> Vec<String> {
        let mut out = Vec::new();
        for block in &doc.blocks {
            if let Block::Paragraph(text)
            | Block::Heading { text, .. }
            | Block::Item { text, .. }
            | Block::Quote { text, .. } = block
            {
                for span in &text.spans {
                    if let Some(link) = &span.style.link {
                        out.push(link.to_string());
                    }
                }
            }
        }
        out
    }

    fn open(bytes: Vec<u8>) -> (Book, Metadata) {
        open_bytes(bytes).unwrap_or_else(|e| panic!("did not open: {e:?}"))
    }

    // ---- The small parts -------------------------------------------------------------------------------------

    #[test]
    fn palmdoc_unpacks_literals_runs_pairs_and_back_references() {
        // 0x00 and 0x09-0x7f are themselves; 0x03 copies the next three bytes; 0xc1 is a space and 'A'.
        assert_eq!(
            decompress_palmdoc(&[b'h', b'i', 0, 3, 0x80, 0xff, 0x41, 0xc1]),
            b"hi\0\x80\xffA A"
        );
        // A back reference: distance 3, length 3 (0x8000 | 3 << 3 | 0) after "abc" gives "abcabc".
        assert_eq!(decompress_palmdoc(&[b'a', b'b', b'c', 0x80, 0x18]), b"abcabc");
        // Overlapping copy: distance 1, length 5 repeats the last byte.
        assert_eq!(decompress_palmdoc(&[b'x', 0x80, 0x0a]), b"xxxxxx");
        // Damage never panics: a distance before the start copies zeros, a cut pair and a cut run end quietly.
        assert_eq!(decompress_palmdoc(&[0x80, 0x18]), [0, 0, 0]);
        assert_eq!(decompress_palmdoc(&[0x80]), [0, 0, 0]);
        assert_eq!(decompress_palmdoc(&[5, b'a']), b"a");
        assert_eq!(decompress_palmdoc(&[]), b"");
    }

    #[test]
    fn what_the_test_compressor_writes_the_reader_reads_back() {
        let mut samples: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"a".to_vec(),
            b"abcabcabcabcabcabc".to_vec(),
            "الكتاب الأول — ثمة كلمات كثيرة، وكلمات أكثر".as_bytes().to_vec(),
            (0..=255u8).collect(),
            b"The quick brown fox jumps over the lazy dog. The quick brown fox jumps again.".repeat(30),
        ];
        // A pseudo-random mix.
        let mut x = 12345u32;
        samples.push(
            (0..5000)
                .map(|_| {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    (x >> 24) as u8
                })
                .collect(),
        );
        for sample in samples {
            assert_eq!(decompress_palmdoc(&palmdoc_compress(&sample)), sample);
        }
    }

    #[test]
    fn thirty_two_bits_are_read_from_any_bit() {
        let data = [0b1010_0000, 0b0000_0001, 0xff, 0x00, 0x80, 0x01];
        assert_eq!(read32(&data, 0), 0b1010_0000_0000_0001_1111_1111_0000_0000);
        assert_eq!(read32(&data, 1), 0b0100_0000_0000_0011_1111_1110_0000_0001);
        assert_eq!(read32(&data, 8), 0b0000_0001_1111_1111_0000_0000_1000_0000);
        // Past the end the bits are zeros.
        assert_eq!(read32(&data, 40), 1 << 24);
        assert_eq!(read32(&data, 400), 0);
    }

    #[test]
    fn trailing_entries_and_the_cut_character_come_off_the_end() {
        // One entry whose length (4, on the last byte, which has the top bit as the start of the number).
        assert_eq!(strip_trailing(b"text\x01\x02\x03\x84", 0b10), b"text");
        // Two entries (3 bytes and 2), then the cut character: the last byte 0x01 says two bytes.
        assert_eq!(strip_trailing(b"text\xaa\x01\x09\x82\x07\x08\x83", 0b111), b"text");
        // No flags, nothing taken.
        assert_eq!(strip_trailing(b"abc", 0), b"abc");
        // Lengths past the start leave nothing, and never panic.
        assert!(strip_trailing(b"ab", 0b10).is_empty());
        assert_eq!(strip_trailing(b"", 0b11), b"");
        // A length of two bytes: 0x81, 0x05 is (1 << 7) | 5 = 133.
        let long = [vec![b'x'; 200], vec![0x81, 0x05]].concat();
        assert_eq!(strip_trailing(&long, 0b10).len(), 200 - 131);
        assert_eq!(var_len(&[0x01, 0x85, 9], 0), Some((133, 2)));
        assert_eq!(var_len(&[0x81, 9], 0), Some((1, 1)));
        assert_eq!(var_len(&[], 0), None);
        // Four bytes at most, none with the top bit.
        assert_eq!(
            var_len(&[1, 2, 3, 4, 5], 0),
            Some(((((1 << 7) | 2) << 7 | 3) << 7 | 4, 4))
        );
    }

    #[test]
    fn numbers_text_and_addresses_are_read_the_way_foliate_read_them() {
        assert_eq!(base32("0001"), Some(1));
        assert_eq!(base32("000A"), Some(10));
        assert_eq!(base32("0010"), Some(32));
        assert_eq!(base32("vv"), Some(1023));
        assert_eq!(base32("zz"), None);
        assert_eq!(base32("1z"), Some(1));
        assert_eq!(base32(""), None);
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            super::super::markdown::base64_decode(&base64_encode(TEST_PNG)).as_deref(),
            Some(TEST_PNG)
        );
        assert_eq!(
            unescape_html("Tom &amp; Jerry &lt;3 &#233; &#x41; &bogus; &"),
            "Tom & Jerry <3 é A &bogus; &"
        );
        assert_eq!(
            parse_pos("kindle:pos:fid:000A:off:0000000010\">x"),
            Some((10, 32, "kindle:pos:fid:000A:off:0000000010".len()))
        );
        assert_eq!(parse_pos("kindle:pos:fid:0001"), None);
        assert_eq!(parse_pos("kindle:embed:0001"), None);
        assert_eq!(
            parse_resource("kindle:embed:0007?mime=image/jpeg\" alt"),
            Some((
                "embed",
                7,
                Some("image/jpeg"),
                "kindle:embed:0007?mime=image/jpeg".len()
            ))
        );
        assert_eq!(
            parse_resource("kindle:flow:0001?mime=text/css"),
            Some(("flow", 1, Some("text/css"), "kindle:flow:0001?mime=text/css".len()))
        );
        assert_eq!(
            parse_resource("kindle:flow:0003\""),
            Some(("flow", 3, None, "kindle:flow:0003".len()))
        );
        assert_eq!(
            first_selector(b"<p class=\"a\" aid=\"07\" id=\"x\">"),
            Some(("aid", "07".to_string()))
        );
        assert_eq!(
            first_selector(b"<a href='#' name = 'top'>"),
            Some(("name", "top".to_string()))
        );
        assert_eq!(first_selector(b"<p data-id=\"no\" class=\"c\">"), None);
        assert_eq!(tag_attr("<p aid=\"4\" id='s'>", "aid"), Some("4"));
        assert_eq!(tag_attr("<p aid=\"4\" id='s'>", "id"), Some("s"));
        assert_eq!(tag_attr("<p>", "aid"), None);
    }

    #[test]
    fn languages_come_from_the_locale_bytes() {
        assert_eq!(locale_language(9, 0), Some("en"));
        assert_eq!(locale_language(9, 4), Some("en-US"));
        assert_eq!(locale_language(9, 8), Some("en-GB"));
        // A hole in the table, and a region past the end, give the language.
        assert_eq!(locale_language(9, 36), Some("en"));
        assert_eq!(locale_language(9, 200), Some("en"));
        assert_eq!(locale_language(1, 0), Some("ar"));
        assert_eq!(locale_language(26, 8), Some("sr"));
        assert_eq!(locale_language(255, 0), None);
    }

    #[test]
    fn tags_are_found_past_quoted_greater_than_signs_and_stray_less_than_signs_are_text() {
        let html = b"<a title=\"x > y\" href=z>t</a>";
        assert_eq!(tag_end(html, 0), "<a title=\"x > y\" href=z>".len());
        // Quotes that never close end at the first `>`.
        assert_eq!(tag_end(b"<a title=\"x>rest", 0), "<a title=\"x>".len());
        assert_eq!(tag_end(b"<!-- a > b --><p>", 0), "<!-- a > b -->".len());
        assert!(is_tag_start(b"<p>", 0) && is_tag_start(b"</p>", 0) && is_tag_start(b"<!x>", 0));
        assert!(!is_tag_start(b"< p", 0) && !is_tag_start(b"<3", 0));
        assert!(!is_tag_start(b"</", 0) && !is_tag_start(b"<", 0));
        let tag = parse_tag(b"<IMG SRC=a.png Alt='x y' hidden/>").expect("a tag");
        assert_eq!(tag.name, "img");
        assert!(tag.self_closing && !tag.closing);
        assert_eq!(tag.attr("src"), Some(&b"a.png"[..]));
        assert_eq!(tag.attr("alt"), Some(&b"x y"[..]));
        assert_eq!(tag.attr("hidden"), Some(&b"hidden"[..]));
        assert!(parse_tag(b"</P>").is_some_and(|t| t.closing && t.name == "p"));
        assert!(parse_tag(b"<!-- x -->").is_none() && parse_tag(b"<>").is_none());
    }

    #[test]
    fn pagebreaks_and_fileposes_are_found_in_the_bytes() {
        let text = "a<mbp:pagebreak/>b< Mbp:PAGEBREAK >c<pagebreak>d<mbp:nu>e<a FILEPOS=\"0000000123\">f\
                    <a href=x filepos=45>g<a filepos=123>";
        let at = |needle: &str| text.find(needle).expect("in the text");
        assert_eq!(
            find_pagebreaks(text.as_bytes()),
            [at("<mbp:pagebreak/>"), at("< Mbp"), at("<pagebreak>")]
        );
        let positions = scan_fileposes(text.as_bytes());
        let found: Vec<(&str, usize)> = positions.iter().map(|f| (f.text.as_str(), f.number)).collect();
        assert_eq!(found, [("45", 45), ("0000000123", 123), ("123", 123)]);
    }

    // ---- Old books --------------------------------------------------------------------------------------------

    #[test]
    fn an_uncompressed_book_has_its_title_author_language_text_and_cover() {
        let text = "<html><head></head><body><h1>The Book</h1><p>First paragraph.</p><p>Second, with <i>style</i>.</p>\
                    <mbp:pagebreak/><h2>Two</h2><p>Chapter two text.</p></body></html>";
        let bytes = old_book(&OldSpec {
            text: text.as_bytes(),
            title: "Header Title",
            exth: vec![
                (100, b"Ada Lovelace".to_vec()),
                (100, b"Charles B.".to_vec()),
                (101, b"Press &amp; Co".to_vec()),
                (524, b"fr".to_vec()),
                (201, 0u32.to_be_bytes().to_vec()),
            ],
            images: vec![TEST_PNG.to_vec()],
            lang: (9, 4),
            ..Default::default()
        });
        let (book, meta) = open(bytes);
        assert_eq!(meta.title, "Header Title");
        assert_eq!(book.title, "Header Title");
        assert_eq!(meta.authors, ["Ada Lovelace", "Charles B."]);
        assert_eq!(meta.publisher.as_deref(), Some("Press & Co"));
        assert_eq!(
            meta.language.as_deref(),
            Some("fr"),
            "EXTH's language wins over the locale bytes"
        );
        assert_eq!(meta.identifier, "77");
        assert_eq!(book.count, 2);
        assert!(!book.rtl && !book.pages);
        assert!(book.cover.is_some(), "EXTH 201 names the cover");
        assert_eq!(book.weights.len(), 2);
        assert_eq!(
            lines(&chapter(&book, 0)),
            ["The Book", "First paragraph.", "Second, with style."]
        );
        assert_eq!(lines(&chapter(&book, 1)), ["Two", "Chapter two text."]);
        assert!(book.chapter(2).is_err(), "no such chapter says so");
    }

    #[test]
    fn the_exth_title_beats_the_headers_and_the_locale_names_the_language() {
        let bytes = old_book(&OldSpec {
            text: b"<p>x</p>",
            title: "Header",
            exth: vec![(503, b"Real &amp; True".to_vec())],
            lang: (9, 4),
            ..Default::default()
        });
        let (book, meta) = open(bytes);
        assert_eq!(book.title, "Real & True");
        assert_eq!(meta.language.as_deref(), Some("en-US"));
        // A book with no title at all: open_book gives it its file's name; here it is empty.
        let (blank, _) = open(old_book(&OldSpec {
            text: b"<p>x</p>",
            ..Default::default()
        }));
        assert_eq!(blank.title, "");
    }

    #[test]
    fn palmdoc_text_over_several_records_with_trailing_bytes_reads_back() {
        let paragraph = "<p>Another paragraph of the book, with some words in it, again and again.</p>";
        let text = format!("<html><body>{}</body></html>", paragraph.repeat(300));
        assert!(text.len() > 4096 * 4);
        for (compression, trailing) in [(1, false), (2, false), (2, true), (1, true)] {
            let bytes = old_book(&OldSpec {
                text: text.as_bytes(),
                compression,
                trailing,
                title: "Long",
                ..Default::default()
            });
            let (book, _) = open(bytes);
            let doc = chapter(&book, 0);
            assert_eq!(doc.blocks.len(), 300, "compression {compression}, trailing {trailing}");
            assert!(lines(&doc).iter().all(|l| l.starts_with("Another paragraph")));
        }
    }

    #[test]
    fn windows_1252_text_is_decoded_and_utf8_arabic_turns_the_pages_the_other_way() {
        let latin = old_book(&OldSpec {
            text: b"<p>caf\xe9 \x93quoted\x94 &amp; &eacute;</p>",
            encoding: 1252,
            ..Default::default()
        });
        let (book, _) = open(latin);
        assert_eq!(lines(&chapter(&book, 0)), ["café “quoted” & é"]);
        let arabic = old_book(&OldSpec {
            text: "<p>بسم الله الرحمن الرحيم</p>".as_bytes(),
            title: "كتاب",
            lang: (1, 0),
            ..Default::default()
        });
        let (book, meta) = open(arabic);
        assert!(book.rtl, "an Arabic book");
        assert_eq!(meta.language.as_deref(), Some("ar"));
        assert_eq!(
            book.language.as_deref(),
            Some("ar"),
            "the book carries it for read aloud"
        );
        assert_eq!(book.title, "كتاب");
        assert_eq!(lines(&chapter(&book, 0)), ["بسم الله الرحمن الرحيم"]);
    }

    #[test]
    fn untidy_html_becomes_paragraphs_lists_tables_and_text() {
        let text = "<HTML><BODY><P ALIGN=center>One<P>Two <B>bold</B><BR>after &amp; more\
                    <UL><LI>a<LI>b</UL><P>3 &lt; 4 and 3 < 4 <mbp:nu>kept</mbp:nu><script>var x = '<p>no</p>';</script><p>end\
                    <table><tr><td>c1<td>c2<tr><td>d1<td>d2</table><font size=2 color=red><i>deep</font></i>";
        let (book, _) = open(simple(text));
        let doc = chapter(&book, 0);
        let got = lines(&doc);
        assert_eq!(
            got,
            [
                "One",
                "Two bold",
                "after & more",
                "• a",
                "• b",
                "3 < 4 and 3 < 4 kept",
                "end",
                "c1\tc2",
                "d1\td2",
                "deep"
            ]
        );
        // The first paragraph is centred.
        assert!(matches!(&doc.blocks[0], Block::Paragraph(t) if t.align == Align::Center));
    }

    #[test]
    fn prefixed_tags_control_bytes_and_runaway_nesting_cannot_make_a_chapter_unreadable() {
        // A control byte, a name with a colon (dropped, its text kept), an attribute with a colon, a stray quote.
        let text = "<p xml:lang=\"en\" o:x=\"1\" class=a\"b>ok\u{1}\u{2}</p><o:p>x</o:p><p>after</p>";
        let (book, _) = open(simple(text));
        assert_eq!(lines(&chapter(&book, 0)), ["ok", "x", "after"]);
        // A thousand unclosed font tags would nest past the reader's limit; the text still arrives as blocks.
        let nested = format!("<p>{}deep text</p><p>next</p>", "<font>".repeat(1000));
        let (book, _) = open(simple(&nested));
        let doc = chapter(&book, 0);
        assert_eq!(lines(&doc), ["deep text", "next"]);
        assert!(
            matches!(&doc.blocks[0], Block::Paragraph(_)),
            "blocks, not the tags-stripped fallback"
        );
    }

    #[test]
    fn filepos_links_anchors_the_guide_and_the_contents_page_all_agree() {
        let make = |t: usize, a: usize, b: usize, c: usize, d: usize| {
            format!(
                "<html><head><guide><reference type=\"toc\" title=\"Contents\" filepos={t:010} />\
                 <reference type=\"text\" title=\"Start\" filepos={a:010} /></guide></head><body>\
                 <h2>Contents</h2><p><a filepos={a:010}>Chapter &amp; One</a></p>\
                 <blockquote><p><a filepos={b:010}>Section 1.1</a></p></blockquote>\
                 <p><a filepos={c:010}>Chapter Two</a></p>\
                 <mbp:pagebreak/><h2>Chapter &amp; One</h2><p>First para. <a filepos={d:010}>jump</a></p>\
                 <p>Section 1.1 body</p>\
                 <mbp:pagebreak/><h2>Chapter Two</h2><p>Last text</p></body></html>"
            )
        };
        let zero = make(0, 0, 0, 0, 0);
        let at = |needle: &str| zero.find(needle).expect("in the text");
        let (t, a, b, c, d) = (
            at("<h2>Contents"),
            at("<h2>Chapter &amp; One"),
            at("<p>Section 1.1 body"),
            at("<h2>Chapter Two"),
            at("<p>Last text"),
        );
        let text = make(t, a, b, c, d);
        assert_eq!(text.len(), zero.len(), "the numbers are as wide as the zeros");
        let (book, _) = open(simple(&text));
        assert_eq!(book.count, 3);
        assert_eq!(book.start, 1, "the guide's `text` reference is the start");
        // The contents page, nested by how far each link is indented.
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].label, "Chapter & One");
        assert_eq!(
            (book.toc[0].chapter, book.toc[0].anchor.clone()),
            (1, Some(format!("filepos{a:010}")))
        );
        assert_eq!(book.toc[0].children.len(), 1);
        assert_eq!(book.toc[0].children[0].label, "Section 1.1");
        assert_eq!(
            (book.toc[0].children[0].chapter, book.toc[0].children[0].anchor.clone()),
            (1, Some(format!("filepos{b:010}")))
        );
        assert_eq!((book.toc[1].label.as_str(), book.toc[1].chapter), ("Chapter Two", 2));
        // The chapters carry the anchors, on the blocks they point at.
        let one = chapter(&book, 1);
        assert_eq!(one.anchors.get(&format!("filepos{a:010}")), Some(&0));
        assert_eq!(one.anchors.get(&format!("filepos{b:010}")), Some(&2));
        let two = chapter(&book, 2);
        assert_eq!(two.anchors.get(&format!("filepos{d:010}")), Some(&1));
        // A link goes to `partNNNN.html#anchor`, which the view finds with `locate`.
        assert_eq!(links(&one), [format!("part0002.html#filepos{d:010}")]);
        assert_eq!(book.source.locate("part0002.html"), Some(2));
        assert_eq!(book.source.locate("part0003.html"), None);
        assert_eq!(book.source.locate("chapter.html"), None);
        // The contents page itself links to the chapters.
        assert_eq!(
            links(&chapter(&book, 0)),
            [
                format!("part0001.html#filepos{a:010}"),
                format!("part0001.html#filepos{b:010}"),
                format!("part0002.html#filepos{c:010}")
            ]
        );
    }

    #[test]
    fn blank_sections_join_the_next_and_a_long_section_is_cut_at_a_paragraph() {
        let text = "<mbp:pagebreak/><mbp:pagebreak/> <p>Real</p><mbp:pagebreak/><div> </div>\
                    <mbp:pagebreak/><p>Last</p><mbp:pagebreak/>";
        let (book, _) = open(simple(text));
        assert_eq!(book.count, 2);
        assert_eq!(lines(&chapter(&book, 0)), ["Real"]);
        assert_eq!(lines(&chapter(&book, 1)), ["Last"]);
        // Only blanks: one chapter, empty.
        let (blank, _) = open(simple("<mbp:pagebreak/> <mbp:pagebreak/>"));
        assert_eq!(blank.count, 1);
        // One section of over 2 MB: several chapters, no paragraph cut in two, every paragraph there.
        let paragraph = "<p>A paragraph of ordinary length, as books have them, to be counted.</p>";
        let text = paragraph.repeat(30_000);
        let (long, _) = open(simple(&text));
        assert!(long.count >= 4, "{} chapters", long.count);
        let total: usize = (0..long.count).map(|i| chapter(&long, i).blocks.len()).sum();
        assert_eq!(total, 30_000);
        assert_eq!(long.weights.iter().sum::<u64>() as usize, text.len());
    }

    #[test]
    fn a_picture_by_recindex_is_in_the_chapter_and_a_missing_one_is_only_empty() {
        let text = "<p>Before</p><img recindex=\"0001\" alt=\"first\"><p>Middle</p><img recindex=\"0009\">\
                    <img src=\"web.png\"><p>After</p>";
        let bytes = old_book(&OldSpec {
            text: text.as_bytes(),
            images: vec![TEST_PNG.to_vec()],
            ..Default::default()
        });
        let (book, _) = open(bytes);
        let doc = chapter(&book, 0);
        let pictures: Vec<(bool, String)> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Picture(p) => Some((p.image.is_some(), p.alt.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(pictures.len(), 3);
        assert_eq!(pictures[0], (true, "first".to_string()));
        assert!(
            !pictures[1].0 && !pictures[2].0,
            "a record that is not there, and a web address, are not pictures"
        );
        assert_eq!(lines(&doc).iter().filter(|l| l.starts_with(['B', 'M', 'A'])).count(), 3);
    }

    // ---- HUFF/CDIC --------------------------------------------------------------------------------------------

    /// The code the test book uses: dictionary indexes 0 to 3 are three bits (the code is `3 - index`), 4 to 7 are
    /// ten bits (`519 - index`, which start with a 1 and so need the second table).
    fn huff_code(index: usize) -> (u32, u32) {
        if index < 4 {
            (3 - index as u32, 3)
        } else {
            (519 - index as u32, 10)
        }
    }

    /// Dictionary indexes as bits, MSB first, padded to a byte with fewer zeros than the shortest code.
    fn huff_encode(indexes: &[usize]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        let push = |index: usize, bits: &mut Vec<bool>| {
            let (code, length) = huff_code(index);
            for k in (0..length).rev() {
                bits.push((code >> k) & 1 == 1);
            }
        };
        for &index in indexes {
            push(index, &mut bits);
        }
        // Index 0 is the empty string, so it fills up harmlessly.
        while (8 - bits.len() % 8) % 8 >= 3 {
            push(0, &mut bits);
        }
        bits.chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |b, (i, bit)| b | (u8::from(*bit) << (7 - i)))
            })
            .collect()
    }

    /// The HUFF record and the CDIC record of the test book's dictionary, whose entry 5 is the packed `packed`.
    fn huff_records(packed: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
        let entries: Vec<(Vec<u8>, bool)> = vec![
            (b"".to_vec(), true),
            (b"Hello, ".to_vec(), true),
            (b"world".to_vec(), true),
            (b". ".to_vec(), true),
            (b"<mbp:pagebreak/>".to_vec(), true),
            (packed, false),
            (b"<p>".to_vec(), true),
            (b"</p>".to_vec(), true),
        ];
        let mut huff = vec![0u8; 24];
        huff[..4].copy_from_slice(b"HUFF");
        put32(&mut huff, 4, 24);
        put32(&mut huff, 8, 24);
        put32(&mut huff, 12, 24 + 1024);
        for byte in 0..256usize {
            // Eight bits starting 0 hold a complete three-bit code; the others need the second table.
            let x: u32 = if byte < 0x80 { 0x80 | 3 | (3 << 8) } else { 4 };
            huff.extend(x.to_be_bytes());
        }
        for length in 1..=32u32 {
            let (least, base) = if length == 10 { (512u32, 519u32) } else { (u32::MAX, 0) };
            huff.extend(least.to_be_bytes());
            huff.extend(base.to_be_bytes());
        }
        let mut cdic = vec![0u8; 16];
        cdic[..4].copy_from_slice(b"CDIC");
        put32(&mut cdic, 4, 16);
        put32(&mut cdic, 8, entries.len());
        put32(&mut cdic, 12, 3);
        let mut body = Vec::new();
        let mut offsets = Vec::new();
        for (value, literal) in &entries {
            offsets.push(entries.len() * 2 + body.len());
            let flag = if *literal { 0x8000 } else { 0 };
            body.extend(((value.len() as u16) | flag).to_be_bytes());
            body.extend(value);
        }
        for offset in offsets {
            cdic.extend((offset as u16).to_be_bytes());
        }
        cdic.extend(body);
        (huff, cdic)
    }

    fn huff_book(text: Vec<u8>, huff: Vec<u8>, cdic: Vec<u8>) -> Vec<u8> {
        pdb(&[
            record0(&Rec0 {
                version: 6,
                compression: 17480,
                text_len: 60,
                text_records: 1,
                encoding: 65001,
                uid: 5,
                resource_start: 4,
                huffcdic: 2,
                num_huffcdic: 2,
                title: "Packed",
                ..Default::default()
            }),
            text,
            huff,
            cdic,
        ])
    }

    #[test]
    fn huffcdic_text_is_unpacked_through_both_tables_and_packed_entries() {
        // <p>Hello, world. </p> | pagebreak | <p>world. Hello, </p>
        let text = huff_encode(&[6, 5, 3, 7, 4, 6, 2, 3, 1, 7]);
        let (huff, cdic) = huff_records(huff_encode(&[1, 2]));
        let (book, meta) = open(huff_book(text, huff, cdic));
        assert_eq!(meta.title, "Packed");
        assert_eq!(book.count, 2);
        assert_eq!(lines(&chapter(&book, 0)), ["Hello, world."]);
        assert_eq!(lines(&chapter(&book, 1)), ["world. Hello,"]);
    }

    #[test]
    fn huffcdic_that_is_damaged_is_an_error_and_never_a_panic() {
        let (mut huff, cdic) = huff_records(huff_encode(&[1, 2]));
        // A table that points past the record.
        put32(&mut huff, 8, 100_000);
        let text = huff_encode(&[6, 7]);
        assert!(matches!(
            open_bytes(huff_book(text.clone(), huff, cdic.clone())).err(),
            Some(BookError::Broken(_))
        ));
        // Text that names a code the dictionary lacks is skipped; the book then has no text to show.
        let (huff, _) = huff_records(huff_encode(&[1, 2]));
        assert!(open_bytes(huff_book(vec![0xff; 40], huff.clone(), cdic.clone())).is_err());
        // An entry that unpacks to itself is cut off by the depth limit.
        let (huff, looping) = huff_records(huff_encode(&[5]));
        assert!(open_bytes(huff_book(huff_encode(&[5]), huff, looping)).is_err());
        // A missing CDIC.
        let (huff, _) = huff_records(huff_encode(&[1, 2]));
        assert!(open_bytes(huff_book(text, huff, Vec::new())).is_err());
    }

    // ---- Why a book does not open -----------------------------------------------------------------------------

    #[test]
    fn a_book_with_drm_says_so_and_a_file_that_is_not_a_book_is_unsupported() {
        for encryption in [1, 2] {
            let bytes = old_book(&OldSpec {
                text: b"<p>locked</p>",
                encryption,
                ..Default::default()
            });
            assert_eq!(open_bytes(bytes).err(), Some(BookError::Drm));
        }
        assert_eq!(
            BookError::Drm.message(),
            "This book has DRM, which only its store's app can open."
        );
        // An encrypted KF8 book too.
        let (mut records, _) = kf8_records(1);
        records[0][13] = 2;
        assert_eq!(open_bytes(pdb(&records)).err(), Some(BookError::Drm));
        // A plain PalmDOC text file ("TEXtREAd") was never opened by foliate either.
        let mut text_file = simple("<p>x</p>");
        text_file[60..68].copy_from_slice(b"TEXtREAd");
        assert_eq!(open_bytes(text_file).err(), Some(BookError::Unsupported));
        assert_eq!(
            open_bytes(b"not a book at all".to_vec()).err(),
            Some(BookError::Unsupported)
        );
        assert_eq!(open_bytes(Vec::new()).err(), Some(BookError::Unsupported));
        // Unknown packing, no record 0, no text.
        let mut odd = simple("<p>x</p>");
        let at = u32::from_be_bytes([odd[78], odd[79], odd[80], odd[81]]) as usize;
        odd[at + 1] = 9;
        assert!(matches!(open_bytes(odd).err(), Some(BookError::Broken(_))));
        assert!(matches!(
            open_bytes(pdb(&[Vec::new()])).err(),
            Some(BookError::Broken(_))
        ));
        let no_text = record0(&Rec0 {
            version: 6,
            compression: 1,
            ..Default::default()
        });
        assert!(matches!(open_bytes(pdb(&[no_text])).err(), Some(BookError::Broken(_))));
    }

    #[test]
    fn damaged_files_never_panic() {
        let kf8 = kf8_file();
        let old = old_book(&OldSpec {
            text:
                b"<html><body><p>Some text <a filepos=0000000010>here</a></p><mbp:pagebreak/><p>more</p></body></html>",
            compression: 2,
            images: vec![TEST_PNG.to_vec()],
            exth: vec![(100, b"A".to_vec()), (121, 1u32.to_be_bytes().to_vec())],
            ..Default::default()
        });
        let mut seed = 0x1234_5678u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as usize
        };
        let mut opened = 0;
        for original in [kf8, old] {
            // Cut short at every few places.
            for cut in (0..original.len()).step_by(37) {
                touch(open_bytes(original[..cut].to_vec()));
            }
            // Bytes changed anywhere, the headers more often.
            for round in 0..600 {
                let mut bytes = original.clone();
                for _ in 0..1 + round % 5 {
                    let limit = if round % 2 == 0 {
                        900.min(bytes.len())
                    } else {
                        bytes.len()
                    };
                    let at = next() % limit;
                    bytes[at] = (next() % 256) as u8;
                }
                if touch(open_bytes(bytes)) {
                    opened += 1;
                }
            }
        }
        assert!(opened > 0, "some damaged books still open");
    }

    /// Reads every chapter of an opened book; whether it opened.
    fn touch(result: Result<(Book, Metadata), BookError>) -> bool {
        match result {
            Ok((book, _)) => {
                for ix in 0..book.count.min(50) {
                    let _ = book.chapter(ix);
                }
                let _ = book.source.locate("part0000.html");
                true
            }
            Err(_) => false,
        }
    }

    // ---- KF8 --------------------------------------------------------------------------------------------------

    fn vwi(value: u32) -> Vec<u8> {
        let mut groups = Vec::new();
        let mut x = value;
        loop {
            groups.push((x & 0x7f) as u8);
            x >>= 7;
            if x == 0 {
                break;
            }
        }
        groups.reverse();
        if let Some(last) = groups.last_mut() {
            *last |= 0x80;
        }
        groups
    }

    /// The strings of a CNCX record, and the key each is found by.
    fn cncx_record(strings: &[&str]) -> (Vec<u8>, Vec<u32>) {
        let mut record = Vec::new();
        let mut keys = Vec::new();
        for s in strings {
            keys.push(record.len() as u32);
            record.extend(vwi(s.len() as u32));
            record.extend(s.as_bytes());
        }
        (record, keys)
    }

    type Tags = Vec<(u8, Vec<u32>)>;

    /// An index: its header, one data record, and the CNCX record if there is one. `table` is the TAGX
    /// (tag, values, mask); each entry is a name and the values of the tags it has.
    fn index_records(table: &[(u8, u8, u8)], entries: &[(String, Tags)], cncx: Option<Vec<u8>>) -> Vec<Vec<u8>> {
        let mut tagx = b"TAGX".to_vec();
        tagx.extend(((12 + 4 * (table.len() + 1)) as u32).to_be_bytes());
        tagx.extend(1u32.to_be_bytes());
        for (tag, values, mask) in table {
            tagx.extend([*tag, *values, *mask, 0]);
        }
        tagx.extend([0, 0, 0, 1]);
        let mut header = vec![0u8; 192];
        header[..4].copy_from_slice(b"INDX");
        put32(&mut header, 4, 192);
        put32(&mut header, 24, 1);
        put32(&mut header, 28, 65001);
        put32(&mut header, 52, usize::from(cncx.is_some()));
        header.extend(tagx);
        let mut data = vec![0u8; 192];
        data[..4].copy_from_slice(b"INDX");
        put32(&mut data, 4, 192);
        let mut offsets = Vec::new();
        for (name, values) in entries {
            offsets.push(data.len());
            data.push(name.len() as u8);
            data.extend(name.as_bytes());
            let mut control = 0u8;
            let mut body = Vec::new();
            for (tag, count, mask) in table {
                if let Some((_, vals)) = values.iter().find(|(t, _)| t == tag) {
                    assert_eq!(vals.len(), usize::from(*count));
                    control |= 1 << mask.trailing_zeros();
                    for v in vals {
                        body.extend(vwi(*v));
                    }
                }
            }
            data.push(control);
            data.extend(body);
        }
        let idxt = data.len();
        data.extend(b"IDXT");
        for offset in &offsets {
            data.extend((*offset as u16).to_be_bytes());
        }
        put32(&mut data, 20, idxt);
        put32(&mut data, 24, entries.len());
        let mut out = vec![header, data];
        out.extend(cncx);
        out
    }

    const SKEL0_HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><html xmlns=\"http://www.w3.org/1999/xhtml\">\
        <head><title>T</title><link rel=\"stylesheet\" type=\"text/css\" href=\"kindle:flow:0001?mime=text/css\"/>\
        </head><body aid=\"0\">";
    const SKEL_TAIL: &str = "</body></html>";
    const FRAG0A: &str = "<h1 aid=\"1\">One</h1><p aid=\"2\">Alpha <b>beta</b> \
        <a href=\"kindle:pos:fid:0002:off:0000000000\">to two</a> <img src=\"kindle:embed:0001?mime=image/png\" alt=\"pic\"/></p>";
    const FRAG0B: &str = "<p aid=\"3\">Second fragment</p>";
    const SKEL1_HEAD: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head><body aid=\"6\">";
    const FRAG1: &str = "<p aid=\"4\" id=\"second\">Gamma <a href=\"kindle:pos:fid:0000:off:0000000000\">back</a> \
        <a href=\"kindle:pos:fid:0009:off:0000000000\">nowhere</a></p><p aid=\"5\">Delta</p>";
    const CSS: &str = "p { font-style: italic }";

    /// The records of a KF8 book, numbered from 0 as the book sees them, and the index of its picture.
    fn kf8_records(compression: usize) -> (Vec<Vec<u8>>, usize) {
        let skel0 = format!("{SKEL0_HEAD}{SKEL_TAIL}");
        let skel1 = format!("{SKEL1_HEAD}{SKEL_TAIL}");
        let s1 = skel0.len() + FRAG0A.len() + FRAG0B.len();
        let main = format!("{skel0}{FRAG0A}{FRAG0B}{skel1}{FRAG1}");
        let text = format!("{main}{CSS}");
        let mut records = vec![Vec::new()];
        let chunks: Vec<&[u8]> = text.as_bytes().chunks(4096).collect();
        for chunk in &chunks {
            records.push(if compression == 2 {
                palmdoc_compress(chunk)
            } else {
                chunk.to_vec()
            });
        }
        let mut fdst = b"FDST".to_vec();
        fdst.extend(12u32.to_be_bytes());
        fdst.extend(2u32.to_be_bytes());
        for (start, end) in [(0, main.len()), (main.len(), text.len())] {
            fdst.extend((start as u32).to_be_bytes());
            fdst.extend((end as u32).to_be_bytes());
        }
        let fdst_at = records.len();
        records.push(fdst);

        let skel_at = records.len();
        let skel_entries: Vec<(String, Tags)> = vec![
            ("SKEL0000".into(), vec![(1, vec![2]), (6, vec![0, skel0.len() as u32])]),
            (
                "SKEL0001".into(),
                vec![(1, vec![1]), (6, vec![s1 as u32, skel1.len() as u32])],
            ),
        ];
        records.extend(index_records(&[(1, 1, 1), (6, 2, 2)], &skel_entries, None));

        let (cncx, keys) = cncx_record(&["div", "body"]);
        let frag_at = records.len();
        let ins0 = SKEL0_HEAD.len();
        let ins1 = ins0 + FRAG0A.len();
        let ins2 = s1 + SKEL1_HEAD.len();
        let frag = |insert: usize, key: u32, fid: u32, offset: usize, length: usize| -> (String, Tags) {
            (
                format!("{insert:010}"),
                vec![(2, vec![key]), (4, vec![fid]), (6, vec![offset as u32, length as u32])],
            )
        };
        let frag_entries = vec![
            frag(ins0, keys[0], 0, 0, FRAG0A.len()),
            frag(ins1, keys[0], 1, FRAG0A.len(), FRAG0B.len()),
            frag(ins2, keys[1], 2, 0, FRAG1.len()),
        ];
        records.extend(index_records(
            &[(2, 1, 1), (4, 1, 2), (6, 2, 4)],
            &frag_entries,
            Some(cncx),
        ));

        // The contents: Part One (with Sub A in it) and Part Two.
        let (cncx, keys) = cncx_record(&["Part One", "Sub A", "Part Two &amp; Last"]);
        let sub_offset = FRAG0A.find("<p aid=\"2\"").expect("in the fragment") as u32;
        let ncx_at = records.len();
        let ncx_entries: Vec<(String, Tags)> = vec![
            (
                "00".into(),
                vec![(3, vec![keys[0]]), (4, vec![0]), (6, vec![0, 0]), (22, vec![1])],
            ),
            (
                "01".into(),
                vec![
                    (3, vec![keys[1]]),
                    (4, vec![1]),
                    (6, vec![0, sub_offset]),
                    (21, vec![0]),
                ],
            ),
            ("02".into(), vec![(3, vec![keys[2]]), (4, vec![0]), (6, vec![2, 0])]),
        ];
        records.extend(index_records(
            &[(3, 1, 1), (4, 1, 2), (6, 2, 4), (21, 1, 8), (22, 1, 16)],
            &ncx_entries,
            Some(cncx),
        ));

        // The guide: reading starts in the second chapter.
        let (cncx, keys) = cncx_record(&["Start"]);
        let guide_at = records.len();
        records.extend(index_records(
            &[(1, 1, 1), (6, 2, 2)],
            &[("text".into(), vec![(1, vec![keys[0]]), (6, vec![2, 0])])],
            Some(cncx),
        ));

        let image_at = records.len();
        records.push(TEST_PNG.to_vec());
        records[0] = record0(&Rec0 {
            version: 8,
            compression,
            text_len: text.len(),
            text_records: chunks.len(),
            encoding: 65001,
            uid: 9,
            resource_start: image_at,
            indx: ncx_at,
            kf8: [fdst_at, frag_at, skel_at, guide_at],
            title: "KF8 Title",
            exth: vec![
                (100, b"Grace Hopper".to_vec()),
                (524, b"en".to_vec()),
                (201, 0u32.to_be_bytes().to_vec()),
            ],
            ..Default::default()
        });
        (records, image_at)
    }

    fn kf8_file() -> Vec<u8> {
        pdb(&kf8_records(1).0)
    }

    #[test]
    fn a_kf8_book_is_built_from_its_skeletons_and_fragments() {
        for compression in [1, 2] {
            let (book, meta) = open(pdb(&kf8_records(compression).0));
            assert_eq!(meta.title, "KF8 Title");
            assert_eq!(meta.authors, ["Grace Hopper"]);
            assert_eq!(meta.language.as_deref(), Some("en"));
            assert!(book.cover.is_some());
            assert_eq!(book.count, 2);
            assert_eq!(
                book.start, 1,
                "the guide's `text` entry names a fragment of the second chapter"
            );

            // Two fragments are put into the first skeleton, in order.
            let one = chapter(&book, 0);
            assert_eq!(
                lines(&one),
                ["One", "Alpha beta to two", "pic", "Second fragment"],
                "compression {compression}"
            );
            let pictures = one
                .blocks
                .iter()
                .filter(|b| matches!(b, Block::Picture(p) if p.image.is_some()))
                .count();
            assert_eq!(pictures, 1, "kindle:embed:0001 is the first picture record");
            // The style sheet in flow 1 reaches the paragraphs.
            let italic = |doc: &Document| {
                doc.blocks
                    .iter()
                    .any(|b| matches!(b, Block::Paragraph(t) if t.spans.iter().any(|s| s.style.italic)))
            };
            assert!(italic(&one), "p {{ font-style: italic }}");

            // Links: kindle:pos names a fragment; the target is its first aid, so the chapter marks that.
            let two = chapter(&book, 1);
            assert_eq!(lines(&two), ["Gamma back nowhere", "Delta"]);
            assert_eq!(links(&one), ["part0001.html#aid-4"]);
            assert_eq!(
                links(&two),
                ["part0000.html#aid-1"],
                "a link to a fragment that isn't there is not a link"
            );
            assert_eq!(one.anchors.get("aid-1"), Some(&0));
            assert_eq!(one.anchors.get("aid-2"), Some(&1));
            assert_eq!(two.anchors.get("aid-4"), Some(&0));
            assert_eq!(two.anchors.get("second"), Some(&0));
            assert_eq!(book.source.locate("part0001.html"), Some(1));
            assert_eq!(book.source.locate("part0002.html"), None);

            // The contents: the NCX's tree, positions turned into chapters and anchors.
            assert_eq!(book.toc.len(), 2);
            assert_eq!(book.toc[0].label, "Part One");
            assert_eq!((book.toc[0].chapter, book.toc[0].anchor.as_deref()), (0, Some("aid-1")));
            assert_eq!(book.toc[0].children.len(), 1);
            assert_eq!(book.toc[0].children[0].label, "Sub A");
            assert_eq!(book.toc[0].children[0].anchor.as_deref(), Some("aid-2"));
            assert_eq!(book.toc[1].label, "Part Two & Last");
            assert_eq!((book.toc[1].chapter, book.toc[1].anchor.as_deref()), (1, Some("aid-4")));
            assert_eq!(book.weights.len(), 2);
            assert!(book.weights.iter().all(|w| *w > 0));
        }
    }

    #[test]
    fn a_combined_file_reads_its_kf8_half_and_falls_back_to_the_old_half_when_it_cannot() {
        let (half, image_at) = kf8_records(1);
        let old_text = "<p>The old half.</p>";
        let boundary = 3;
        let combined = |boundary_value: u32| {
            let mut records = vec![
                record0(&Rec0 {
                    version: 6,
                    compression: 1,
                    text_len: old_text.len(),
                    text_records: 1,
                    encoding: 65001,
                    resource_start: boundary + image_at,
                    title: "Old Title",
                    exth: vec![(121, boundary_value.to_be_bytes().to_vec())],
                    ..Default::default()
                }),
                old_text.as_bytes().to_vec(),
                b"EOF ".to_vec(),
            ];
            assert_eq!(records.len(), boundary);
            records.extend(half.clone());
            pdb(&records)
        };
        // The second half is the book, with its pictures found from the first header's pointer.
        let (book, meta) = open(combined(boundary as u32));
        assert_eq!(meta.title, "KF8 Title");
        assert_eq!(book.count, 2);
        let doc = chapter(&book, 0);
        assert_eq!(lines(&doc)[0], "One");
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Picture(p) if p.image.is_some())));
        // The marker says there is none, or points at nothing, or at something that is not a KF8 header.
        for value in [NONE, 9000, 1] {
            let (old, meta) = open(combined(value));
            assert_eq!(meta.title, "Old Title", "boundary {value:#x}");
            assert_eq!(lines(&chapter(&old, 0)), ["The old half."]);
        }
    }

    #[test]
    fn a_kf8_book_missing_a_table_is_damaged_not_a_panic() {
        let (mut records, _) = kf8_records(1);
        // The skeleton index is gone.
        let skel_at = u32::from_be_bytes(records[0][252..256].try_into().expect("four bytes")) as usize;
        records[skel_at] = b"nope".to_vec();
        assert!(matches!(open_bytes(pdb(&records)).err(), Some(BookError::Broken(_))));
    }

    // ---- Through the book view's opener -----------------------------------------------------------------------

    #[test]
    fn open_book_reads_the_mobi_family_by_extension_and_names_a_book_with_no_title_by_its_file() {
        let dir = std::env::temp_dir().join(format!("glint-mobi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp folder");
        let old = old_book(&OldSpec {
            text: b"<p>Plain</p>",
            ..Default::default()
        });
        for (name, bytes) in [
            ("Untitled.mobi", old.clone()),
            ("Same.azw", old),
            ("Kindle.azw3", kf8_file()),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, bytes).expect("written");
            let book = open_book(&path.to_string_lossy()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert!(book.count >= 1, "{name}");
            if name == "Untitled.mobi" {
                assert_eq!(book.title, "Untitled.mobi", "the file's name stands in");
            }
        }
        // Not a book, and no file.
        let bad = dir.join("bad.mobi");
        std::fs::write(&bad, b"hello").expect("written");
        assert_eq!(open_book(&bad.to_string_lossy()).err(), Some(BookError::Unsupported));
        assert!(matches!(
            open_book(&dir.join("missing.azw3").to_string_lossy()).err(),
            Some(BookError::Broken(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_ten_megabyte_book_opens_and_turns_quickly() {
        let line = "<p>Line of the big book, with a few ordinary words in it to be read.</p>\n";
        let mut text = String::new();
        let mut n = 0;
        while text.len() < 10 * 1024 * 1024 {
            text.push_str(line);
            n += 1;
            if n % 800 == 0 {
                text.push_str("<mbp:pagebreak/>");
            }
        }
        let bytes = old_book(&OldSpec {
            text: text.as_bytes(),
            title: "Big",
            ..Default::default()
        });
        let started = std::time::Instant::now();
        let (book, _) = open(bytes);
        let opening = started.elapsed();
        assert!(book.count > 100, "{} chapters", book.count);
        let started = std::time::Instant::now();
        let doc = chapter(&book, 3);
        let reading = started.elapsed();
        assert_eq!(doc.blocks.len(), 800);
        // Generous: a debug build on a slow machine, with other tests running.
        assert!(opening.as_secs_f32() < 20., "opening took {opening:?}");
        assert!(reading.as_secs_f32() < 5., "a chapter took {reading:?}");
        eprintln!("10 MB book: opened in {opening:?}, one chapter in {reading:?}");
    }
}
