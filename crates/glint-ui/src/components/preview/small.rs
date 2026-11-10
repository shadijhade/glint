//! Contacts, calendars, mail, design files, and the diagrams' source (SmallPreviews.tsx).
//!
//! | TypeScript                                   | Here                                                               |
//! |----------------------------------------------|--------------------------------------------------------------------|
//! | `SmallPreview` (by `SmallKind`)              | [`SmallPreview`], one view for every kind; [`small_kind`] says which |
//! | `lines`, `property`, `readContacts`          | [`read_contacts`] (pure; tested)                                   |
//! | `icsDate`, `readEvents`                      | [`read_events`], [`When`] (pure; tested)                           |
//! | `Contacts`, `Calendar`                       | cards in a virtualized `list`                                      |
//! | `readEmail` (postal-mime)                    | [`read_email`]: a MIME reader of its own (headers, multipart, base64, quoted-printable, RFC 2047 and 2231, charsets) |
//! | `readOutlook` (msgreader)                    | [`read_outlook`] over [`Cfb`], a compound-file reader of its own   |
//! | `openAttachment`                             | [`Hooks`]: glint-core's `attachment_save`, then the explorer opens the copy |
//! | `DesignPicture` (zip.js)                     | the archive layer (`sources.extract`) reads the picture out of the file; drawn with `img` |
//! | `Graphviz`, `MermaidFile`                    | the source as text (PORT ME(graphviz), PORT ME(mermaid))           |
//!
//! # Where this goes (for the coordinator)
//!
//! - `choose.rs`: the `PORT ME(small previews)` arm becomes its own `Body::Small(SmallKind)` (a variant the
//!   coordinator adds), `Choice::of(..)` with no `later` for the kinds this module draws; [`small_kind`] is the
//!   test (`glint_model::small_kinds::small_kind_of(&entry.ext)`, readable and not archived, as now).
//! - `frame.rs`'s `build`: `Body::Small(kind) => cx.new(|cx| SmallPreview::new(entry.clone(), kind, window, cx)).into()`.
//!   The view needs no other event.
//!
//! # Not ported
//!
//! - **HTML mail is shown as text.** There is no HTML engine here, so a message with an HTML part and no text part
//!   becomes the text of its HTML ([`html_to_text`]: no scripts, styles or pictures, paragraphs and lists kept as
//!   lines), and says so. A message with both shows its text part. Pictures inside a message are never shown;
//!   the line about pictures from the internet stays, because the HTML still names them.
//! - Outlook messages whose body is only RTF (compressed RTF) show no body; messages attached inside a message
//!   (`.msg` in a `.msg`) are not listed.
//! - Graphviz and Mermaid files are not drawn: they show their source. PORT ME(graphviz): a renderer (the
//!   TypeScript used `@hpcc-js/wasm-graphviz`). PORT ME(mermaid): merman (docs/gpui/previews-native.md section 5).
//! - Dates in an event or a message are written with Windows' long date (`format_full_date`), which has seconds
//!   where the TypeScript's `timeStyle: "short"` had none; a date alone is written short.

use super::sources::{sources, Fut};
use super::states::centered;
use crate::kit::prelude::*;
use crate::kit::{Editor, EditorState};
use crate::{core_bridge, explorer, t, theme};
use glint_model::entries::FileEntry;
use glint_model::format::{format_date, format_full_date, format_size};
use glint_model::small_kinds::{small_kind_of, SmallKind};
use gpui_kit::{list, Image, ImageFormat, ImageSource, ListAlignment, ListState, ObjectFit, StyledImage as _, Task};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// The biggest file read for a small preview.
const MAX_BYTES: u64 = 32 << 20;
/// Contacts shown (`slice(0, 500)`) and events shown (`slice(0, 200)`).
const MAX_CARDS: usize = 500;
const MAX_EVENTS: usize = 200;
/// The most of a message's text, or a diagram's source, put into the editor.
const MAX_TEXT: usize = 1 << 20;
/// How deep multipart messages nest before the rest is ignored.
const MAX_DEPTH: usize = 12;
/// The picture a Krita, OpenRaster, Procreate or Sketch file keeps of itself, in the order tried.
pub const DESIGN_PICTURES: [&str; 5] = [
    "mergedimage.png",
    "Thumbnail.png",
    "QuickLook\\Thumbnail.png",
    "previews\\preview.png",
    "preview.png",
];

/// Which small preview a file gets, if any (`smallKind(entry.ext)`).
pub fn small_kind(entry: &FileEntry) -> Option<SmallKind> {
    if entry.dir {
        return None;
    }
    small_kind_of(&entry.ext)
}

// ---- vCard and iCalendar -----------------------------------------------------------------------------------

/// vCard and iCalendar lines, unfolded (a line starting with a space or tab continues the one before).
fn unfold(source: &str) -> Vec<String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let joined = source
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    joined
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

/// One property: its name (upper case, without an `item1.` group), its parameters (upper case) and its value,
/// unescaped (`property`).
struct Property {
    name: String,
    params: String,
    value: String,
}

fn property(line: &str) -> Option<Property> {
    let colon = line.find(':')?;
    let mut head = line[..colon].split(';');
    let name = head.next().unwrap_or_default().to_uppercase();
    let params = head.collect::<Vec<_>>().join(";").to_uppercase();
    let value = unescape(&line[colon + 1..]);
    Some(Property {
        name: strip_group(&name).to_string(),
        params,
        value,
    })
}

/// `ITEM3.TEL` as `TEL` (`replace(/^ITEM\d+\./, "")`).
fn strip_group(name: &str) -> &str {
    if let Some(rest) = name.strip_prefix("ITEM") {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with('.') {
            return &rest[digits + 1..];
        }
    }
    name
}

/// `\n` as a line break, then `\,` `\;` `\\` as themselves.
fn unescape(value: &str) -> String {
    let mut with_breaks = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && matches!(chars.peek(), Some('n' | 'N')) {
            chars.next();
            with_breaks.push('\n');
        } else {
            with_breaks.push(c);
        }
    }
    let mut out = String::with_capacity(with_breaks.len());
    let mut chars = with_breaks.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && matches!(chars.peek(), Some(',' | ';' | '\\')) {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// What kind of number or address a vCard line says it is (`WORK`, `HOME`, `CELL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Work,
    Home,
    Mobile,
}

fn kind_of_params(params: &str) -> Kind {
    if params.contains("WORK") {
        Kind::Work
    } else if params.contains("HOME") {
        Kind::Home
    } else if params.contains("CELL") {
        Kind::Mobile
    } else {
        Kind::Plain
    }
}

/// What a row of a contact is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Phone(Kind),
    Email(Kind),
    Organization,
    Title,
    Address(Kind),
    Website,
    Birthday,
    Note,
}

impl Label {
    /// The label as the TypeScript wrote it: "Phone (work)".
    pub fn text(self) -> SharedString {
        match self {
            Label::Phone(Kind::Plain) => t!("Phone"),
            Label::Phone(Kind::Work) => t!("Phone (work)"),
            Label::Phone(Kind::Home) => t!("Phone (home)"),
            Label::Phone(Kind::Mobile) => t!("Phone (mobile)"),
            Label::Email(Kind::Plain) => t!("Email"),
            Label::Email(Kind::Work) => t!("Email (work)"),
            Label::Email(Kind::Home) => t!("Email (home)"),
            Label::Email(Kind::Mobile) => t!("Email (mobile)"),
            Label::Address(Kind::Plain) => t!("Address"),
            Label::Address(Kind::Work) => t!("Address (work)"),
            Label::Address(Kind::Home) => t!("Address (home)"),
            Label::Address(Kind::Mobile) => t!("Address (mobile)"),
            Label::Organization => t!("Organization"),
            Label::Title => t!("Title"),
            Label::Website => t!("Website"),
            Label::Birthday => t!("Birthday"),
            Label::Note => t!("Note"),
        }
    }
}

/// One contact.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub name: String,
    pub rows: Vec<(Label, String)>,
    /// The picture the card carries, as bytes (PNG, JPEG…).
    pub photo: Option<Vec<u8>>,
}

fn join_parts(value: &str) -> String {
    value
        .split(';')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The contacts of a vCard file (`readContacts`).
pub fn read_contacts(source: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut card: Option<Card> = None;
    for line in unfold(source) {
        let Some(p) = property(&line) else { continue };
        if p.name == "BEGIN" && p.value.eq_ignore_ascii_case("VCARD") {
            card = Some(Card::default());
        } else if p.name == "END" {
            if let Some(done) = card.take() {
                cards.push(done);
            }
        } else if let Some(card) = card.as_mut() {
            let kind = kind_of_params(&p.params);
            match p.name.as_str() {
                "FN" => card.name = p.value,
                "N" if card.name.is_empty() => {
                    let mut parts: Vec<&str> = p.value.split(';').filter(|part| !part.is_empty()).collect();
                    parts.reverse();
                    card.name = parts.join(" ");
                }
                "TEL" => {
                    let number = if p.value.len() >= 4 && p.value[..4].eq_ignore_ascii_case("tel:") {
                        p.value[4..].to_string()
                    } else {
                        p.value
                    };
                    card.rows.push((Label::Phone(kind), number));
                }
                "EMAIL" => card.rows.push((Label::Email(kind), p.value)),
                "ORG" => card.rows.push((Label::Organization, join_parts(&p.value))),
                "TITLE" => card.rows.push((Label::Title, p.value)),
                "ADR" => card.rows.push((Label::Address(kind), join_parts(&p.value))),
                "URL" => card.rows.push((Label::Website, p.value)),
                "BDAY" => card.rows.push((Label::Birthday, p.value)),
                "NOTE" => card.rows.push((Label::Note, p.value)),
                "PHOTO" => {
                    if p.value.len() > 11 && p.value[..11].eq_ignore_ascii_case("data:image/") {
                        if let Some((_, data)) = p.value.split_once(',') {
                            card.photo = super::markdown::base64_decode(data);
                        }
                    } else if p.params.contains("ENCODING=B") || p.params.contains("BASE64") {
                        card.photo = super::markdown::base64_decode(&p.value);
                    }
                }
                _ => {}
            }
        }
    }
    cards
}

/// A date or a moment of an event, as the file wrote it (`icsDate`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum When {
    #[default]
    None,
    /// A date alone.
    Day { year: i64, month: u32, day: u32 },
    /// A time on a date, UTC when `utc` (the `Z`), else the PC's local time.
    Moment {
        year: i64,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        utc: bool,
    },
    /// What isn't a date the reader knows, as written.
    Raw(String),
}

impl When {
    /// `20261109T143000Z`, `20261109T143000`, or `20261109` (only the start of the value is read).
    pub fn parse(value: &str) -> When {
        if value.is_empty() {
            return When::None;
        }
        let digits = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = value.get(range)?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(4..6), digits(6..8)) else {
            return When::Raw(value.to_string());
        };
        let year = i64::from(year);
        if value.as_bytes().get(8) == Some(&b'T') {
            if let (Some(hour), Some(minute), Some(second)) = (digits(9..11), digits(11..13), digits(13..15)) {
                return When::Moment {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    utc: value.as_bytes().get(15) == Some(&b'Z'),
                };
            }
        }
        When::Day { year, month, day }
    }

    /// The moment as milliseconds since 1970 (a date alone is its local midnight).
    pub fn ms(&self) -> Option<i64> {
        match *self {
            When::Day { year, month, day } => Some(local_to_ms(year, month, day, 0, 0, 0)),
            When::Moment {
                year,
                month,
                day,
                hour,
                minute,
                second,
                utc,
            } => Some(if utc {
                utc_ms(year, month, day, hour, minute, second)
            } else {
                local_to_ms(year, month, day, hour, minute, second)
            }),
            _ => None,
        }
    }

    /// As shown: the date in full with its time, or the date alone short, in the language in use.
    pub fn text(&self) -> String {
        match self {
            When::None => String::new(),
            When::Raw(raw) => raw.clone(),
            When::Day { .. } => self
                .ms()
                .map(|ms| format_date(ms, None, crate::i18n::language()))
                .unwrap_or_default(),
            When::Moment { .. } => self.ms().map(format_full_date).unwrap_or_default(),
        }
    }
}

/// Days from 1970-01-01 to a calendar date (Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn utc_ms(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    (days_from_civil(year, month, day) * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
        * 1000
}

/// A date and time on this PC's clock as milliseconds since 1970, by Windows' own time zone rules (UTC if
/// Windows can't convert it).
fn local_to_ms(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::Time::TzSpecificLocalTimeToSystemTime;
    let naive = utc_ms(year, month, day, hour, minute, second);
    let (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(s)) = (
        u16::try_from(year),
        u16::try_from(month),
        u16::try_from(day),
        u16::try_from(hour),
        u16::try_from(minute),
        u16::try_from(second),
    ) else {
        return naive;
    };
    let local = SYSTEMTIME {
        wYear: y,
        wMonth: mo,
        wDayOfWeek: 0,
        wDay: d,
        wHour: h,
        wMinute: mi,
        wSecond: s,
        wMilliseconds: 0,
    };
    let mut utc = SYSTEMTIME::default();
    // SAFETY: both pointers are to live locals; the call reads the first and writes the second.
    let converted = unsafe { TzSpecificLocalTimeToSystemTime(None, &local, &mut utc) };
    if converted.is_err() {
        return naive;
    }
    utc_ms(
        i64::from(utc.wYear),
        u32::from(utc.wMonth),
        u32::from(utc.wDay),
        u32::from(utc.wHour),
        u32::from(utc.wMinute),
        u32::from(utc.wSecond),
    )
}

/// One event of a calendar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub summary: String,
    pub start: When,
    pub end: When,
    pub location: String,
    pub description: String,
    pub organizer: String,
    pub attendees: Vec<String>,
}

/// `CN="Name"` out of a property's parameters, if it has one.
fn common_name(line: &str) -> Option<String> {
    let head = &line[..line.find(':')?];
    let lower = head.to_ascii_lowercase();
    let at = lower.find("cn=")? + 3;
    let rest = head[at..].strip_prefix('"').unwrap_or(&head[at..]);
    let name: String = rest.chars().take_while(|c| !matches!(c, '"' | ';' | ':')).collect();
    (!name.is_empty()).then_some(name)
}

fn without_mailto(value: &str) -> String {
    if value.len() >= 7 && value[..7].eq_ignore_ascii_case("mailto:") {
        value[7..].to_string()
    } else {
        value.to_string()
    }
}

/// The events of an iCalendar file (`readEvents`).
pub fn read_events(source: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut event: Option<Event> = None;
    for line in unfold(source) {
        let Some(p) = property(&line) else { continue };
        if p.name == "BEGIN" && p.value.eq_ignore_ascii_case("VEVENT") {
            event = Some(Event::default());
        } else if p.name == "END" && p.value.eq_ignore_ascii_case("VEVENT") {
            if let Some(done) = event.take() {
                events.push(done);
            }
        } else if let Some(event) = event.as_mut() {
            let who = || common_name(&line).unwrap_or_else(|| without_mailto(&p.value));
            match p.name.as_str() {
                "SUMMARY" => event.summary = p.value.clone(),
                "DTSTART" => event.start = When::parse(&p.value),
                "DTEND" => event.end = When::parse(&p.value),
                "LOCATION" => event.location = p.value.clone(),
                "DESCRIPTION" => event.description = p.value.clone(),
                "ORGANIZER" => event.organizer = who(),
                "ATTENDEE" => event.attendees.push(who()),
                _ => {}
            }
        }
    }
    events
}

// ---- Mail: MIME --------------------------------------------------------------------------------------------

/// A file attached to a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub data: Arc<Vec<u8>>,
}

/// A message, ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub subject: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    /// When it was sent or delivered, ms since 1970.
    pub date: Option<i64>,
    /// The `Date:` header as written, when it isn't a date this reader knows.
    pub date_raw: String,
    pub text: String,
    /// The message had an HTML part and no text part: `text` is the HTML's text.
    pub html_only: bool,
    /// The HTML names pictures on the internet.
    pub remote: bool,
    pub attachments: Vec<Attachment>,
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Bytes as text in the charset a message names (UTF-8 when it names none); what isn't valid in it is read
/// the way glint-core reads any older text file.
fn decode_charset(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.map(|c| c.trim().trim_matches('"').to_ascii_lowercase());
    match label.as_deref() {
        None | Some("") | Some("utf-8") | Some("utf8") | Some("us-ascii") | Some("ascii") => {
            match std::str::from_utf8(bytes) {
                Ok(text) => text.to_string(),
                Err(_) => glint_core::textenc::decode(bytes, false).text,
            }
        }
        Some("iso-8859-1") | Some("latin1") | Some("l1") => latin1(bytes),
        Some(other) => glint_core::textenc::decode_as(bytes, other)
            .unwrap_or_else(|| glint_core::textenc::decode(bytes, false).text),
    }
}

fn hex_value(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Quoted-printable, as a body (`=XX`, soft line breaks) or as an encoded word (`q`: `_` is a space).
fn quoted_printable(input: &[u8], q: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'=' => {
                if let (Some(&a), Some(&b)) = (input.get(i + 1), input.get(i + 2)) {
                    if let (Some(high), Some(low)) = (hex_value(a), hex_value(b)) {
                        out.push(high * 16 + low);
                        i += 3;
                        continue;
                    }
                }
                // A soft line break.
                if input.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else if input.get(i + 1) == Some(&b'\r') && input.get(i + 2) == Some(&b'\n') {
                    i += 3;
                } else {
                    out.push(b'=');
                    i += 1;
                }
            }
            b'_' if q => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// A body with its transfer encoding undone.
fn transfer_decoded(body: &[u8], encoding: &str) -> Vec<u8> {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "base64" => super::markdown::base64_decode(&String::from_utf8_lossy(body)).unwrap_or_else(|| body.to_vec()),
        "quoted-printable" => quoted_printable(body, false),
        _ => body.to_vec(),
    }
}

/// RFC 2047 encoded words in a header (`=?UTF-8?B?...?=`) as text; whitespace between two of them goes.
fn decode_words(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    let mut after_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, tail) = rest.split_at(start);
        let word = parse_word(&tail[2..]);
        match word {
            Some((decoded, used)) => {
                if !(after_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&decoded);
                rest = &tail[2 + used..];
                after_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &tail[2..];
                after_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `charset?B?text?=` (after the `=?`): the text decoded and how many bytes were used.
fn parse_word(after_open: &str) -> Option<(String, usize)> {
    let mut parts = after_open.splitn(3, '?');
    let charset = parts.next()?;
    let encoding = parts.next()?;
    let rest = parts.next()?;
    let end = rest.find("?=")?;
    let payload = &rest[..end];
    if payload.contains(char::is_whitespace) || charset.is_empty() {
        return None;
    }
    let charset = charset.split('*').next().unwrap_or(charset);
    let bytes = match encoding {
        "B" | "b" => super::markdown::base64_decode(payload)?,
        "Q" | "q" => quoted_printable(payload.as_bytes(), true),
        _ => return None,
    };
    Some((
        decode_charset(&bytes, Some(charset)),
        charset.len() + 1 + encoding.len() + 1 + end + 2,
    ))
}

/// A header value split at its `;`s (not inside quotes): the value, and its `name=value` parameters.
fn typed(value: &str) -> (String, Vec<(String, String)>) {
    let mut pieces = Vec::new();
    let (mut current, mut quoted, mut escaped) = (String::new(), false, false);
    for c in value.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if c == '\\' && quoted {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if c == ';' && !quoted {
            pieces.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    pieces.push(current);
    let mut pieces = pieces.into_iter();
    let first = pieces.next().unwrap_or_default().trim().to_ascii_lowercase();
    let params = pieces
        .filter_map(|piece| {
            let (name, value) = piece.split_once('=')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    (first, params)
}

fn percent_bytes(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(&a), Some(&b)) = (bytes.get(i + 1), bytes.get(i + 2)) {
                if let (Some(high), Some(low)) = (hex_value(a), hex_value(b)) {
                    out.push(high * 16 + low);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A parameter's value: plain (with encoded words), or in RFC 2231's forms (`name*=utf-8''%D8...`, `name*0=`,
/// `name*1*=`).
fn param(params: &[(String, String)], key: &str) -> Option<String> {
    let mut pieces: Vec<(usize, bool, &str)> = Vec::new();
    for (name, value) in params {
        if name == key {
            return Some(decode_words(value));
        }
        let Some(rest) = name.strip_prefix(key).and_then(|rest| rest.strip_prefix('*')) else {
            continue;
        };
        if rest.is_empty() {
            pieces.push((0, true, value));
        } else {
            let (number, encoded) = match rest.strip_suffix('*') {
                Some(number) => (number, true),
                None => (rest, false),
            };
            if let Ok(number) = number.parse::<usize>() {
                pieces.push((number, encoded, value));
            }
        }
    }
    if pieces.is_empty() {
        return None;
    }
    pieces.sort_by_key(|piece| piece.0);
    let mut charset = None;
    let mut bytes = Vec::new();
    for (i, (_, encoded, value)) in pieces.iter().enumerate() {
        let mut value = *value;
        if i == 0 && *encoded {
            let mut parts = value.splitn(3, '\'');
            if let (Some(set), Some(_language), Some(rest)) = (parts.next(), parts.next(), parts.next()) {
                charset = Some(set.to_string());
                value = rest;
            }
        }
        if *encoded {
            bytes.extend(percent_bytes(value));
        } else {
            bytes.extend(value.bytes());
        }
    }
    Some(decode_charset(&bytes, charset.as_deref()))
}

/// Where a message's headers end and its body starts: `(end of headers, start of body)`.
fn split_head(bytes: &[u8]) -> (usize, usize) {
    if bytes.starts_with(b"\r\n") {
        return (0, 2);
    }
    if bytes.starts_with(b"\n") {
        return (0, 1);
    }
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            if bytes.get(i + 1) == Some(&b'\n') {
                return (i, i + 2);
            }
            if bytes.get(i + 1) == Some(&b'\r') && bytes.get(i + 2) == Some(&b'\n') {
                return (i, i + 3);
            }
        }
        i += 1;
    }
    (bytes.len(), bytes.len())
}

/// Header names (lower case) and values (unfolded).
fn parse_headers(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    headers
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// The parts of a multipart body, split at the boundary.
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut start: Option<usize> = None;
    let mut at = 0;
    while at < body.len() {
        let end = body[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(body.len(), |p| at + p);
        let next = (end + 1).min(body.len());
        let line = body[at..end].strip_suffix(b"\r").unwrap_or(&body[at..end]);
        let trimmed = {
            let mut l = line;
            while let Some((last, init)) = l.split_last() {
                if *last == b' ' || *last == b'\t' {
                    l = init;
                } else {
                    break;
                }
            }
            l
        };
        let is_close = trimmed == format!("{delimiter}--").as_bytes();
        if trimmed == delimiter.as_bytes() || is_close {
            if let Some(from) = start.take() {
                // The line break before the delimiter belongs to the delimiter.
                let mut to = at;
                if to > from && body[to - 1] == b'\n' {
                    to -= 1;
                    if to > from && body[to - 1] == b'\r' {
                        to -= 1;
                    }
                }
                parts.push(&body[from..to.max(from)]);
            }
            if is_close {
                return parts;
            }
            start = Some(next);
        }
        at = next;
    }
    if let Some(from) = start {
        parts.push(&body[from..]);
    }
    parts
}

#[derive(Default)]
struct Collected {
    texts: Vec<String>,
    htmls: Vec<String>,
    attachments: Vec<Attachment>,
}

fn walk(bytes: &[u8], depth: usize, found: &mut Collected) {
    let (head_end, body_start) = split_head(bytes);
    let headers = parse_headers(&bytes[..head_end]);
    let body = &bytes[body_start.min(bytes.len())..];
    let (kind, kind_params) = typed(header(&headers, "content-type").unwrap_or("text/plain"));
    let (disposition, disposition_params) = typed(header(&headers, "content-disposition").unwrap_or(""));
    if kind.starts_with("multipart/") {
        if let (Some(boundary), true) = (param(&kind_params, "boundary"), depth < MAX_DEPTH) {
            for part in split_multipart(body, &boundary) {
                walk(part, depth + 1, found);
            }
            return;
        }
    }
    let decoded = transfer_decoded(body, header(&headers, "content-transfer-encoding").unwrap_or(""));
    let filename = param(&disposition_params, "filename").or_else(|| param(&kind_params, "name"));
    let is_text = kind == "text/plain" || kind == "text/html";
    if is_text && disposition != "attachment" {
        let text = decode_charset(&decoded, param(&kind_params, "charset").as_deref());
        if kind == "text/html" {
            found.htmls.push(text);
        } else {
            found.texts.push(text);
        }
        return;
    }
    // A picture a message's HTML points at (`cid:`) is part of the message, not a file to open.
    let inline = disposition == "inline"
        || (disposition.is_empty() && kind.starts_with("image/") && header(&headers, "content-id").is_some());
    if inline && header(&headers, "content-id").is_some() {
        return;
    }
    let name = filename.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| {
        if kind == "message/rfc822" {
            "message.eml".into()
        } else {
            "attachment".into()
        }
    });
    found.attachments.push(Attachment {
        name,
        data: Arc::new(decoded),
    });
}

/// Names and addresses (`Name <a@b>, c@d`) as the TypeScript wrote them: `Name <a@b>, c@d`.
fn addresses(value: &str) -> String {
    let mut people = Vec::new();
    let (mut current, mut quoted, mut angle) = (String::new(), false, false);
    let mut flush = |current: &mut String| {
        let item = std::mem::take(current);
        let item = item.trim();
        if !item.is_empty() {
            people.push(person(item));
        }
    };
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            '<' if !quoted => {
                angle = true;
                current.push(c);
            }
            '>' if !quoted => {
                angle = false;
                current.push(c);
            }
            ',' if !quoted && !angle => flush(&mut current),
            _ => current.push(c),
        }
    }
    flush(&mut current);
    people
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn person(item: &str) -> String {
    if let (Some(open), Some(close)) = (item.rfind('<'), item.rfind('>')) {
        if open < close {
            let name = item[..open].trim().trim_matches('"').trim();
            let address = item[open + 1..close].trim();
            let name = decode_words(name);
            return if name.is_empty() {
                address.to_string()
            } else {
                format!("{name} <{address}>")
            };
        }
    }
    decode_words(item).trim().to_string()
}

/// An RFC 5322 date (`Mon, 9 Nov 2026 14:30:00 +0300`) as ms since 1970.
fn parse_mail_date(value: &str) -> Option<i64> {
    // Comments in parentheses go.
    let mut plain = String::new();
    let mut depth = 0;
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let plain = plain.trim();
    let plain = plain.split_once(',').map_or(plain, |(_, rest)| rest).trim();
    let mut words = plain.split_whitespace();
    let day: u32 = words.next()?.parse().ok()?;
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| {
        words
            .clone()
            .next()
            .is_some_and(|w| w.to_ascii_lowercase().starts_with(m))
    })? as u32
        + 1;
    words.next();
    let mut year: i64 = words.next()?.parse().ok()?;
    if year < 50 {
        year += 2000;
    } else if year < 100 {
        year += 1900;
    }
    let mut clock = words.next()?.split(':');
    let hour: u32 = clock.next()?.parse().ok()?;
    let minute: u32 = clock.next()?.parse().ok()?;
    let second: u32 = clock.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let zone = words.next().unwrap_or("+0000");
    let offset = match zone {
        "UT" | "GMT" | "Z" => 0,
        "EST" => -5 * 60,
        "EDT" => -4 * 60,
        "CST" => -6 * 60,
        "CDT" => -5 * 60,
        "MST" => -7 * 60,
        "MDT" => -6 * 60,
        "PST" => -8 * 60,
        "PDT" => -7 * 60,
        z if z.len() == 5 && (z.starts_with('+') || z.starts_with('-')) => {
            let hours: i64 = z[1..3].parse().ok()?;
            let minutes: i64 = z[3..5].parse().ok()?;
            let total = hours * 60 + minutes;
            if z.starts_with('-') {
                -total
            } else {
                total
            }
        }
        _ => 0,
    };
    Some(utc_ms(year, month, day, hour, minute, second) - offset * 60_000)
}

/// Whether HTML names a picture on the internet (`<img … src="http…">`).
fn remote_pictures(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(found) = lower[at..].find("<img") {
        let start = at + found;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &lower[start..end];
        if let Some(src) = tag.find("src") {
            let value = tag[src + 3..].trim_start();
            if let Some(value) = value.strip_prefix('=') {
                let value = value.trim_start().trim_start_matches(['"', '\'']);
                if value.starts_with("http:") || value.starts_with("https:") {
                    return true;
                }
            }
        }
        at = start + 4;
    }
    false
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "hellip" => Some('…'),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        "bull" => Some('•'),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// HTML as the text a person would read: tags gone, scripts and styles dropped, entities decoded, paragraphs
/// and list items on lines of their own (a stand-in for the page the TypeScript showed in a frame).
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut chars = html.char_indices().peekable();
    let lower = html.to_ascii_lowercase();
    let break_line = |out: &mut String| {
        while out.ends_with(' ') {
            out.pop();
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    };
    let break_paragraph = |out: &mut String| {
        break_line(out);
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
    };
    while let Some((i, c)) = chars.next() {
        match c {
            '<' => {
                // A comment runs to its end.
                if lower[i..].starts_with("<!--") {
                    let end = lower[i + 4..].find("-->").map_or(html.len(), |e| i + 4 + e + 3);
                    while chars.peek().is_some_and(|&(j, _)| j < end) {
                        chars.next();
                    }
                    continue;
                }
                let end = lower[i..].find('>').map_or(html.len(), |e| i + e + 1);
                let tag = &lower[i + 1..end.saturating_sub(1).max(i + 1)];
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                let closing = tag.starts_with('/');
                let mut skip_to = end;
                if !closing && (name == "script" || name == "style" || name == "head") {
                    let close = format!("</{name}");
                    skip_to = lower[end.min(lower.len())..].find(&close).map_or(html.len(), |e| {
                        lower[end + e..].find('>').map_or(html.len(), |g| end + e + g + 1)
                    });
                }
                match name.as_str() {
                    "br" | "p" | "div" | "tr" | "table" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    | "blockquote" | "pre" | "hr" | "section" | "article" | "header" | "footer" => {
                        if closing && matches!(name.as_str(), "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                            break_paragraph(&mut out);
                        } else {
                            break_line(&mut out);
                        }
                    }
                    "li" if !closing => {
                        break_line(&mut out);
                        out.push_str("• ");
                    }
                    "li" => break_line(&mut out),
                    "td" | "th" if closing => out.push(' '),
                    _ => {}
                }
                while chars.peek().is_some_and(|&(j, _)| j < skip_to) {
                    chars.next();
                }
            }
            '&' => {
                let rest = &html[i + 1..];
                let decoded = rest
                    .find(';')
                    .filter(|&e| e <= 10)
                    .and_then(|e| entity(&rest[..e]).map(|ch| (ch, e)));
                match decoded {
                    Some((ch, e)) => {
                        out.push(ch);
                        for _ in 0..=e {
                            chars.next();
                        }
                    }
                    None => out.push('&'),
                }
            }
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with([' ', '\n']) {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    let mut text = String::new();
    let mut blanks = 0;
    for line in lines {
        if line.is_empty() {
            blanks += 1;
            if blanks <= 1 && !text.is_empty() {
                text.push('\n');
            }
        } else {
            blanks = 0;
            text.push_str(line);
            text.push('\n');
        }
    }
    text.trim_end().to_string()
}

/// An `.eml` file as a message (`readEmail`).
pub fn read_email(bytes: &[u8]) -> Message {
    let (head_end, _) = split_head(bytes);
    let headers = parse_headers(&bytes[..head_end]);
    let mut found = Collected::default();
    walk(bytes, 0, &mut found);
    let text = found.texts.join("\n").replace("\r\n", "\n");
    let html = found.htmls.join("\n");
    let html_only = text.trim().is_empty() && !html.trim().is_empty();
    let date_text = header(&headers, "date").unwrap_or_default();
    let date = parse_mail_date(date_text);
    Message {
        subject: decode_words(header(&headers, "subject").unwrap_or_default()),
        from: addresses(header(&headers, "from").unwrap_or_default()),
        to: addresses(header(&headers, "to").unwrap_or_default()),
        cc: addresses(header(&headers, "cc").unwrap_or_default()),
        date,
        date_raw: if date.is_some() {
            String::new()
        } else {
            date_text.to_string()
        },
        text: if html_only { html_to_text(&html) } else { text },
        html_only,
        remote: remote_pictures(&html),
        attachments: found.attachments,
    }
}

// ---- Mail: Outlook messages (a compound file) ---------------------------------------------------------------

const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;

fn u16_at(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    data.get(at..at + 8).map_or(0, |b| {
        let mut array = [0u8; 8];
        array.copy_from_slice(b);
        u64::from_le_bytes(array)
    })
}

/// One entry of a compound file's directory.
#[derive(Clone, Debug)]
struct Dir {
    name: String,
    /// 1 storage, 2 stream, 5 root.
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

/// A compound file (what Outlook saves a message as): its storages and streams, read from memory.
pub struct Cfb<'a> {
    data: &'a [u8],
    shift: u32,
    mini_shift: u32,
    cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    dirs: Vec<Dir>,
    mini_stream: Vec<u8>,
}

impl<'a> Cfb<'a> {
    /// Reads the header, the allocation tables and the directory.
    pub fn open(data: &'a [u8]) -> Result<Self, String> {
        const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        if data.len() < 512 || data[..8] != MAGIC {
            return Err("It isn't an Outlook message (no compound file in it)".into());
        }
        let shift = u32::from(u16_at(data, 30));
        let mini_shift = u32::from(u16_at(data, 32));
        if !(7..=16).contains(&shift) || mini_shift > shift {
            return Err("The file's header is damaged".into());
        }
        let mut cfb = Cfb {
            data,
            shift,
            mini_shift,
            cutoff: u64::from(u32_at(data, 56)),
            fat: Vec::new(),
            mini_fat: Vec::new(),
            dirs: Vec::new(),
            mini_stream: Vec::new(),
        };
        // The sectors that hold the allocation table: 109 in the header, then a chain of sectors of more.
        let mut fat_sectors: Vec<u32> = (0..109).map(|i| u32_at(data, 76 + i * 4)).collect();
        let mut next = u32_at(data, 68);
        let mut guard = 0;
        while next != END_OF_CHAIN && next != FREE && guard < 4096 {
            let Some(sector) = cfb.sector(next) else { break };
            let per = sector.len() / 4 - 1;
            fat_sectors.extend((0..per).map(|i| u32_at(sector, i * 4)));
            next = u32_at(sector, per * 4);
            guard += 1;
        }
        for id in fat_sectors.into_iter().filter(|&id| id != FREE && id != END_OF_CHAIN) {
            if let Some(sector) = cfb.sector(id) {
                let entries: Vec<u32> = (0..sector.len() / 4).map(|i| u32_at(sector, i * 4)).collect();
                cfb.fat.extend(entries);
            }
        }
        let directory = cfb.read_chain(u32_at(data, 48));
        cfb.dirs = directory
            .as_chunks::<128>()
            .0
            .iter()
            .map(|entry| {
                let units = usize::from(u16_at(entry, 64) / 2).saturating_sub(1).min(31);
                let name: Vec<u16> = (0..units).map(|i| u16_at(entry, i * 2)).collect();
                Dir {
                    name: String::from_utf16_lossy(&name),
                    kind: entry[66],
                    left: u32_at(entry, 68),
                    right: u32_at(entry, 72),
                    child: u32_at(entry, 76),
                    start: u32_at(entry, 116),
                    size: if shift >= 12 {
                        u64_at(entry, 120)
                    } else {
                        u64::from(u32_at(entry, 120))
                    },
                }
            })
            .collect();
        if cfb.dirs.is_empty() || cfb.dirs[0].kind != 5 {
            return Err("The file has no root".into());
        }
        let mini_fat = cfb.read_chain(u32_at(data, 60));
        cfb.mini_fat = mini_fat
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let root = cfb.dirs[0].clone();
        let mut mini_stream = cfb.read_chain(root.start);
        mini_stream.truncate(usize::try_from(root.size).unwrap_or(usize::MAX));
        cfb.mini_stream = mini_stream;
        Ok(cfb)
    }

    fn sector(&self, id: u32) -> Option<&'a [u8]> {
        let size = 1usize << self.shift;
        let start = (usize::try_from(id).ok()?.checked_add(1)?).checked_mul(size)?;
        self.data.get(start..start.checked_add(size)?)
    }

    fn read_chain(&self, start: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut id = start;
        let mut steps = 0;
        while id != END_OF_CHAIN && id != FREE && steps <= self.fat.len() {
            let Some(sector) = self.sector(id) else { break };
            out.extend_from_slice(sector);
            id = self.fat.get(id as usize).copied().unwrap_or(END_OF_CHAIN);
            steps += 1;
        }
        out
    }

    /// The directory entries directly inside a storage (the root is 0), in tree order.
    pub fn children(&self, storage: usize) -> Vec<usize> {
        let mut found = Vec::new();
        let Some(dir) = self.dirs.get(storage) else {
            return found;
        };
        let mut stack = vec![dir.child];
        while let Some(id) = stack.pop() {
            if id == FREE || found.len() > self.dirs.len() {
                continue;
            }
            let Some(entry) = self.dirs.get(id as usize) else {
                continue;
            };
            if found.contains(&(id as usize)) {
                continue;
            }
            found.push(id as usize);
            stack.push(entry.right);
            stack.push(entry.left);
        }
        found
    }

    /// The name of an entry.
    pub fn name(&self, entry: usize) -> &str {
        self.dirs.get(entry).map_or("", |d| d.name.as_str())
    }

    /// The entry named `name` in a storage (names compare without regard to case).
    pub fn find(&self, storage: usize, name: &str) -> Option<usize> {
        self.children(storage)
            .into_iter()
            .find(|&child| self.dirs[child].name.eq_ignore_ascii_case(name))
    }

    /// A stream's bytes (empty for a storage, or a stream that is damaged).
    pub fn stream(&self, entry: usize) -> Vec<u8> {
        let Some(dir) = self.dirs.get(entry) else {
            return Vec::new();
        };
        if dir.kind != 2 || dir.size > self.data.len() as u64 * 2 {
            return Vec::new();
        }
        let size = usize::try_from(dir.size).unwrap_or(0);
        if dir.size < self.cutoff {
            let unit = 1usize << self.mini_shift;
            let mut out = Vec::with_capacity(size);
            let mut id = dir.start;
            let mut steps = 0;
            while id != END_OF_CHAIN && id != FREE && steps <= self.mini_fat.len() && out.len() < size {
                let from = (id as usize).saturating_mul(unit);
                let Some(block) = self
                    .mini_stream
                    .get(from..from.saturating_add(unit).min(self.mini_stream.len()))
                else {
                    break;
                };
                out.extend_from_slice(block);
                id = self.mini_fat.get(id as usize).copied().unwrap_or(END_OF_CHAIN);
                steps += 1;
            }
            out.truncate(size);
            out
        } else {
            let mut out = self.read_chain(dir.start);
            out.truncate(size);
            out
        }
    }
}

/// One property of a message, recipient or attachment from its `__properties_version1.0` stream.
struct Prop {
    id: u16,
    ty: u16,
    value: [u8; 8],
}

fn properties(stream: &[u8], header: usize) -> Vec<Prop> {
    stream
        .get(header..)
        .unwrap_or_default()
        .as_chunks::<16>()
        .0
        .iter()
        .map(|entry| {
            let mut value = [0u8; 8];
            value.copy_from_slice(&entry[8..16]);
            Prop {
                ty: u16_at(entry, 0),
                id: u16_at(entry, 2),
                value,
            }
        })
        .collect()
}

/// A string property of a storage: Unicode (`001F`) or the PC's code page (`001E`).
fn msg_text(cfb: &Cfb, storage: usize, id: u16) -> Option<String> {
    if let Some(entry) = cfb.find(storage, &format!("__substg1.0_{id:04X}001F")) {
        let bytes = cfb.stream(entry);
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        let text = String::from_utf16_lossy(&units);
        let text = text.trim_end_matches('\0').to_string();
        return (!text.is_empty()).then_some(text);
    }
    let entry = cfb.find(storage, &format!("__substg1.0_{id:04X}001E"))?;
    let bytes = cfb.stream(entry);
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    let text = decode_charset(bytes, Some("windows-1252"));
    (!text.is_empty()).then_some(text)
}

fn msg_binary(cfb: &Cfb, storage: usize, id: u16) -> Option<Vec<u8>> {
    let entry = cfb.find(storage, &format!("__substg1.0_{id:04X}0102"))?;
    Some(cfb.stream(entry))
}

/// `Name <address>`, or whichever there is (`r.name !== r.email`).
fn name_and_address(name: Option<String>, address: Option<String>) -> String {
    match (name, address) {
        (Some(n), Some(a)) if n != a => format!("{n} <{a}>"),
        (Some(n), _) => n,
        (None, Some(a)) => a,
        (None, None) => String::new(),
    }
}

/// An Outlook `.msg` file as a message (`readOutlook`).
pub fn read_outlook(bytes: &[u8]) -> Result<Message, String> {
    let cfb = Cfb::open(bytes)?;
    let root = 0;
    let top = cfb
        .find(root, "__properties_version1.0")
        .map(|e| cfb.stream(e))
        .unwrap_or_default();
    let top_props = properties(&top, 32);
    let filetime = |id: u16| {
        top_props.iter().find(|p| p.id == id && p.ty == 0x0040).map(|p| {
            let ticks = u64::from_le_bytes(p.value);
            i64::try_from(ticks / 10_000).unwrap_or(0) - 11_644_473_600_000
        })
    };
    let date = filetime(0x0E06).or_else(|| filetime(0x0039)).filter(|&ms| ms > 0);

    let mut to = Vec::new();
    let mut cc = Vec::new();
    for child in cfb.children(root) {
        if !cfb.name(child).starts_with("__recip_version1.0_#") {
            continue;
        }
        let props = cfb
            .find(child, "__properties_version1.0")
            .map(|e| cfb.stream(e))
            .unwrap_or_default();
        let kind = properties(&props, 8)
            .iter()
            .find(|p| p.id == 0x0C15 && p.ty == 0x0003)
            .map_or(1, |p| u32_at(&p.value, 0));
        let name = msg_text(&cfb, child, 0x3001);
        let address =
            msg_text(&cfb, child, 0x39FE).or_else(|| msg_text(&cfb, child, 0x3003).filter(|a| a.contains('@')));
        let shown = name_and_address(name, address);
        match kind {
            1 => to.push(shown),
            2 => cc.push(shown),
            _ => {}
        }
    }
    if to.is_empty() {
        to = msg_text(&cfb, root, 0x0E04).into_iter().collect();
    }
    if cc.is_empty() {
        cc = msg_text(&cfb, root, 0x0E03).into_iter().collect();
    }

    let sender = name_and_address(
        msg_text(&cfb, root, 0x0C1A),
        msg_text(&cfb, root, 0x5D01).or_else(|| msg_text(&cfb, root, 0x0C1F).filter(|a| a.contains('@'))),
    );

    let text = msg_text(&cfb, root, 0x1000).unwrap_or_default().replace("\r\n", "\n");
    let html = msg_binary(&cfb, root, 0x1013)
        .map(|b| decode_charset(&b, None))
        .or_else(|| msg_text(&cfb, root, 0x1013))
        .unwrap_or_default();
    let html_only = text.trim().is_empty() && !html.trim().is_empty();

    let mut attachments = Vec::new();
    for child in cfb.children(root) {
        if !cfb.name(child).starts_with("__attach_version1.0_#") {
            continue;
        }
        let props = cfb
            .find(child, "__properties_version1.0")
            .map(|e| cfb.stream(e))
            .unwrap_or_default();
        let hidden = properties(&props, 8)
            .iter()
            .any(|p| p.id == 0x7FFE && p.ty == 0x000B && u16_at(&p.value, 0) != 0);
        if hidden {
            continue;
        }
        let Some(data) = msg_binary(&cfb, child, 0x3701) else {
            continue;
        };
        let name = msg_text(&cfb, child, 0x3707)
            .or_else(|| msg_text(&cfb, child, 0x3704))
            .or_else(|| msg_text(&cfb, child, 0x3001))
            .unwrap_or_else(|| "attachment".into());
        attachments.push(Attachment {
            name,
            data: Arc::new(data),
        });
    }

    Ok(Message {
        subject: msg_text(&cfb, root, 0x0037).unwrap_or_default(),
        from: sender,
        to: to.join(", "),
        cc: cc.join(", "),
        date,
        date_raw: String::new(),
        text: if html_only { html_to_text(&html) } else { text },
        html_only,
        remote: remote_pictures(&html),
        attachments,
    })
}

// ---- What the view asks of the outside world --------------------------------------------------------------

/// Saves an attachment (its name, its bytes) and says where the copy is.
pub type SaveFn = Rc<dyn Fn(String, Vec<u8>) -> Fut<Result<String, String>>>;
/// Opens a file with its app.
pub type OpenFn = Rc<dyn Fn(&str, &mut App)>;

/// Saving an attachment and opening it: glint-core and the explorer in the app, fakes in tests.
#[derive(Clone)]
pub struct Hooks {
    /// Saves the attachment's bytes under its name into a temp folder of Glint's, marked as from the internet;
    /// the path of the copy.
    pub save: SaveFn,
    /// Opens the copy with its app (through the explorer).
    pub open: OpenFn,
}

impl Hooks {
    /// glint-core's `attachment_save`, then `Explorer::open_file`.
    pub fn core() -> Self {
        Hooks {
            save: Rc::new(|name, data| {
                let running = core_bridge::run(glint_core::attachments::attachment_save(percent_encoded(&name), data));
                Box::pin(async move { running.await.and_then(|inner| inner) })
            }),
            open: Rc::new(|path, cx| {
                let path = path.to_string();
                explorer::explorer(cx).update(cx, |explorer, cx| {
                    explorer.open_file(&path, None, cx).detach();
                });
            }),
        }
    }
}

struct SharedHooks(Hooks);

impl Global for SharedHooks {}

/// Sets the hooks the small previews use (tests; the app uses [`Hooks::core`] without this).
pub fn init_with(cx: &mut App, hooks: Hooks) {
    cx.set_global(SharedHooks(hooks));
}

fn hooks(cx: &App) -> Hooks {
    cx.try_global::<SharedHooks>()
        .map_or_else(Hooks::core, |shared| shared.0.clone())
}

/// A name as `attachment_save` takes it from a web page's header: every byte but letters, digits and a few
/// marks as `%XX`.
fn percent_encoded(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b' ' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---- The view ------------------------------------------------------------------------------------------------

/// Why a file couldn't be shown.
enum Problem {
    Unreadable(String),
    TooBig,
    NoContacts,
    NoEvents,
    Outlook(String),
    WordTemplate,
    NoPicture,
}

impl Problem {
    fn text(&self) -> SharedString {
        match self {
            Problem::Unreadable(reason) => t!("Couldn't read it ({reason})", reason = reason.clone()),
            Problem::TooBig => t!("Too big to preview here: open it to read it."),
            Problem::NoContacts => t!("No contacts in it."),
            Problem::NoEvents => t!("No events in it."),
            Problem::Outlook(reason) => {
                t!(
                    "Glint couldn't read this Outlook message: {error}",
                    error = reason.clone()
                )
            }
            Problem::WordTemplate => t!("This .dot file is a Word template, which Word opens."),
            Problem::NoPicture => t!("It keeps no picture of itself to show."),
        }
    }
}

/// What was read, off the UI thread.
enum Loaded {
    Contacts(Vec<Card>),
    Events(Vec<Event>),
    Mail(Message),
    Picture(PathBuf),
    Source(String),
}

/// A message on screen: it, its text in a read-only editor.
struct Mailbox {
    message: Arc<Message>,
    body: Entity<EditorState>,
    cut: bool,
}

enum Load {
    Reading,
    Failed(SharedString),
    Contacts {
        cards: Arc<Vec<Card>>,
        photos: Arc<Vec<Option<Arc<Image>>>>,
    },
    Events(Arc<Vec<Event>>),
    Mail(Mailbox),
    Picture(PathBuf),
    Source {
        editor: Entity<EditorState>,
        note: SharedString,
    },
}

/// A small preview: one file of one kind.
pub struct SmallPreview {
    entry: FileEntry,
    kind: SmallKind,
    load: Load,
    list: ListState,
    focus: FocusHandle,
    /// Why an attachment didn't open.
    problem: Option<SharedString>,
    task: Option<Task<()>>,
}

fn sniff_photo(bytes: &[u8]) -> ImageFormat {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        ImageFormat::Png
    } else if bytes.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if bytes.starts_with(b"BM") {
        ImageFormat::Bmp
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else {
        ImageFormat::Jpeg
    }
}

/// A diagram file's source as text; a Word 97 template (`.dot` is its extension too) is told apart.
fn diagram_source(bytes: &[u8]) -> Result<String, Problem> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Err(Problem::WordTemplate);
    }
    let head = &bytes[..bytes.len().min(MAX_TEXT)];
    Ok(glint_core::textenc::decode(head, bytes.len() > MAX_TEXT).text)
}

fn read_small(kind: SmallKind, path: &str, size: u64) -> Result<Loaded, Problem> {
    if size > MAX_BYTES {
        return Err(Problem::TooBig);
    }
    let bytes = std::fs::read(path).map_err(|e| Problem::Unreadable(e.to_string()))?;
    match kind {
        SmallKind::Contact => {
            let text = glint_core::textenc::decode(&bytes, false).text;
            let cards = read_contacts(&text);
            if cards.is_empty() {
                Err(Problem::NoContacts)
            } else {
                Ok(Loaded::Contacts(cards))
            }
        }
        SmallKind::Calendar => {
            let text = glint_core::textenc::decode(&bytes, false).text;
            let events = read_events(&text);
            if events.is_empty() {
                Err(Problem::NoEvents)
            } else {
                Ok(Loaded::Events(events))
            }
        }
        SmallKind::Email => Ok(Loaded::Mail(read_email(&bytes))),
        SmallKind::Outlook => read_outlook(&bytes).map(Loaded::Mail).map_err(Problem::Outlook),
        SmallKind::Graphviz | SmallKind::Mermaid => diagram_source(&bytes).map(Loaded::Source),
        SmallKind::Design => Err(Problem::NoPicture),
    }
}

impl SmallPreview {
    /// The small preview of `entry` (of `kind`, from [`small_kind`]); the file is read off the UI thread.
    pub fn new(entry: FileEntry, kind: SmallKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = SmallPreview {
            entry,
            kind,
            load: Load::Reading,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            focus: cx.focus_handle(),
            problem: None,
            task: None,
        };
        view.start(window, cx);
        view
    }

    /// The kind of file this shows.
    pub fn kind(&self) -> SmallKind {
        self.kind
    }

    /// Whether the file has been read (or failed).
    pub fn is_settled(&self) -> bool {
        !matches!(self.load, Load::Reading)
    }

    /// The contacts, once read.
    pub fn contacts(&self) -> Option<&[Card]> {
        match &self.load {
            Load::Contacts { cards, .. } => Some(cards),
            _ => None,
        }
    }

    /// The events, once read.
    pub fn events(&self) -> Option<&[Event]> {
        match &self.load {
            Load::Events(events) => Some(events),
            _ => None,
        }
    }

    /// The message, once read.
    pub fn message(&self) -> Option<&Message> {
        match &self.load {
            Load::Mail(mail) => Some(&mail.message),
            _ => None,
        }
    }

    /// Why the file couldn't be shown.
    pub fn failure(&self) -> Option<&str> {
        match &self.load {
            Load::Failed(note) => Some(note),
            _ => None,
        }
    }

    /// Why an attachment didn't open.
    pub fn attachment_problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The picture a design file keeps of itself (a copy of it in the temp folder), once found.
    pub fn picture(&self) -> Option<&std::path::Path> {
        match &self.load {
            Load::Picture(path) => Some(path),
            _ => None,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load = Load::Reading;
        let entry = self.entry.clone();
        let kind = self.kind;
        let extract = sources(cx).extract.clone();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result: Result<Loaded, Problem> = if kind == SmallKind::Design {
                // The archive layer reads the picture out of the file (a Krita, OpenRaster, Procreate or
                // Sketch file is a zip), as zip.js did.
                let mut found = None;
                for name in DESIGN_PICTURES {
                    if let Ok(path) = extract(&format!("{}\\{name}", entry.path)).await {
                        if std::path::Path::new(&path).is_file() {
                            found = Some(PathBuf::from(path));
                            break;
                        }
                    }
                }
                found.map(Loaded::Picture).ok_or(Problem::NoPicture)
            } else {
                let path = entry.path.clone();
                let size = entry.size;
                cx.background_executor()
                    .spawn(async move { read_small(kind, &path, size) })
                    .await
            };
            this.update_in(cx, |this, window, cx| this.loaded(result, window, cx))
                .ok();
        }));
    }

    fn loaded(&mut self, result: Result<Loaded, Problem>, window: &mut Window, cx: &mut Context<Self>) {
        self.load = match result {
            Err(problem) => Load::Failed(problem.text()),
            Ok(Loaded::Contacts(cards)) => {
                let shown = cards.len().min(MAX_CARDS);
                let photos = cards
                    .iter()
                    .take(shown)
                    .map(|card| {
                        card.photo
                            .as_ref()
                            .filter(|bytes| !bytes.is_empty())
                            .map(|bytes| Arc::new(Image::from_bytes(sniff_photo(bytes), bytes.clone())))
                    })
                    .collect();
                self.list.reset(shown + usize::from(cards.len() > MAX_CARDS));
                Load::Contacts {
                    cards: Arc::new(cards),
                    photos: Arc::new(photos),
                }
            }
            Ok(Loaded::Events(events)) => {
                self.list.reset(events.len().min(MAX_EVENTS));
                Load::Events(Arc::new(events))
            }
            Ok(Loaded::Mail(message)) => {
                let cut = message.text.len() > MAX_TEXT;
                let mut end = message.text.len().min(MAX_TEXT);
                while !message.text.is_char_boundary(end) {
                    end -= 1;
                }
                let shown = message.text[..end].replace("\r\n", "\n");
                let body = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .line_number(false)
                        .soft_wrap(true)
                        .searchable(false)
                        .folding(false)
                        .default_value(shown)
                });
                body.update(cx, |editor, cx| editor.set_readonly(true, cx));
                Load::Mail(Mailbox {
                    message: Arc::new(message),
                    body,
                    cut,
                })
            }
            Ok(Loaded::Picture(path)) => Load::Picture(path),
            Ok(Loaded::Source(text)) => {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .line_number(true)
                        .soft_wrap(false)
                        .searchable(false)
                        .folding(false)
                        .default_value(text.replace("\r\n", "\n"))
                });
                editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
                // PORT ME(graphviz): the graph drawn (the TypeScript used @hpcc-js/wasm-graphviz).
                // PORT ME(mermaid): the diagram drawn (merman, docs/gpui/previews-native.md section 5).
                let note = if self.kind == SmallKind::Graphviz {
                    t!("Glint can't draw Graphviz graphs yet, so the file is shown as written.")
                } else {
                    t!("Glint can't draw Mermaid diagrams yet, so the file is shown as written.")
                };
                Load::Source { editor, note }
            }
        };
        cx.notify();
    }

    fn open_attachment(&mut self, at: usize, cx: &mut Context<Self>) {
        let Load::Mail(mail) = &self.load else { return };
        let Some(attachment) = mail.message.attachments.get(at).cloned() else {
            return;
        };
        let hooks = hooks(cx);
        self.problem = None;
        cx.spawn(async move |this, cx| {
            let saved = (hooks.save)(attachment.name.clone(), attachment.data.as_ref().clone()).await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(path) => (hooks.open)(&path, cx),
                    Err(reason) => this.problem = Some(reason.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Keys for the lists: arrows, pages, Home and End -------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let page = self.list.viewport_bounds().size.height * 0.85;
        let line = px(48.);
        match event.keystroke.key.as_str() {
            "down" => self.list.scroll_by(line),
            "up" => self.list.scroll_by(-line),
            "pagedown" => self.list.scroll_by(page),
            "pageup" => self.list.scroll_by(-page),
            "home" => self.list.scroll_to_reveal_item(0),
            "end" => self
                .list
                .scroll_to_reveal_item(self.list.item_count().saturating_sub(1)),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    // ---- Drawing ---------------------------------------------------------------------------------------------

    fn failed(note: SharedString, cx: &App) -> AnyElement {
        let warning = theme::tokens(cx).warning;
        centered(cx)
            .id("small-failed")
            .role(Role::Alert)
            .aria_label(note.clone())
            .test_support()
            .child(icon(IconName::TriangleAlert).size(px(24.)).text_color(warning))
            .child(div().max_w(rems(24.)).min_w_0().child(note))
            .into_any_element()
    }

    fn reading(cx: &App) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let what = t!("Reading…");
        row()
            .id("small-reading")
            .role(Role::Status)
            .aria_label(what.clone())
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(space(2.))
            .text_size(theme::size::SM)
            .text_color(t.muted_foreground)
            .child(Spinner::new().color(t.primary))
            .child(what)
            .into_any_element()
    }

    /// A card's frame: `rounded-lg border p-3`.
    fn card(
        id: (&'static str, usize),
        label: SharedString,
        t: &theme::Tokens,
    ) -> impl IntoElement + ParentElement + Styled {
        row_start()
            .id(id)
            .role(Role::Group)
            .aria_label(label)
            .test_support()
            .w_full()
            .gap(space(3.))
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
    }

    /// Rows of a label and its value (`dl`).
    fn facts(rows: Vec<(SharedString, SharedString)>, t: &theme::Tokens) -> Div {
        col()
            .gap(px(2.))
            .text_size(theme::size::XS)
            .children(rows.into_iter().map(|(label, value)| {
                row_start()
                    .gap(space(3.))
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(rems(6.5))
                            .text_color(t.muted_foreground)
                            .child(label),
                    )
                    .child(div().flex_1().min_w_0().child(value))
            }))
    }

    fn contact_card(&self, at: usize, cx: &App) -> AnyElement {
        let Load::Contacts { cards, photos } = &self.load else {
            return div().into_any_element();
        };
        let Some(card) = cards.get(at) else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let name: SharedString = if card.name.is_empty() {
            t!("(no name)")
        } else {
            isolate(card.name.clone())
        };
        let avatar = match photos.get(at).and_then(Option::as_ref) {
            Some(image) => img(ImageSource::Image(image.clone()))
                .size(rems(3.))
                .flex_none()
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .size(rems(3.))
                .flex_none()
                .rounded_full()
                .bg(t.muted)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::User).size(rems(1.5)).text_color(t.muted_foreground))
                .into_any_element(),
        };
        let rows = card
            .rows
            .iter()
            .map(|(label, value)| (label.text(), isolate(value.clone())))
            .collect();
        div()
            .pb(space(3.))
            .child(
                Self::card(("small-contact", at), name.clone(), &t).child(avatar).child(
                    col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(theme::size::SM)
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(name),
                        )
                        .child(div().mt(space(1.)).child(Self::facts(rows, &t))),
                ),
            )
            .into_any_element()
    }

    fn event_card(&self, at: usize, cx: &App) -> AnyElement {
        let Load::Events(events) = &self.load else {
            return div().into_any_element();
        };
        let Some(event) = events.get(at) else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let title: SharedString = if event.summary.is_empty() {
            t!("(no title)")
        } else {
            isolate(event.summary.clone())
        };
        let mut when = event.start.text();
        if event.end != When::None {
            when.push_str(" – ");
            when.push_str(&event.end.text());
        }
        let rows: Vec<(SharedString, SharedString)> = [
            (t!("When"), when),
            (t!("Where"), event.location.clone()),
            (t!("Organizer"), event.organizer.clone()),
            (t!("Invited"), event.attendees.join(", ")),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| (label, isolate(value)))
        .collect();
        div()
            .pb(space(3.))
            .child(
                Self::card(("small-event", at), title.clone(), &t)
                    .flex_col()
                    .gap(space(2.))
                    .child(
                        row_start()
                            .gap(space(2.))
                            .items_center()
                            .text_size(theme::size::SM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon(IconName::CalendarDays).size_4().flex_none().text_color(t.primary))
                            .child(div().min_w_0().child(title)),
                    )
                    .child(Self::facts(rows, &t))
                    .when(!event.description.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(isolate(event.description.clone())),
                        )
                    }),
            )
            .into_any_element()
    }

    /// A list of cards that scrolls (virtualized), with the keys.
    fn cards(&self, id: &'static str, label: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let rows = list(
            self.list.clone(),
            cx.processor(|this: &mut Self, at: usize, _window, cx| match &this.load {
                Load::Contacts { cards, .. } => {
                    if at >= MAX_CARDS {
                        let t = theme::tokens(cx).clone();
                        div()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!("And {count} more.", count = cards.len() - MAX_CARDS))
                            .into_any_element()
                    } else {
                        this.contact_card(at, cx)
                    }
                }
                Load::Events(_) => this.event_card(at, cx),
                _ => div().into_any_element(),
            }),
        )
        .size_full();
        div()
            .id(id)
            .role(Role::List)
            .aria_label(label)
            .test_support()
            .track_focus(&self.focus)
            .tab_stop(true)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .size_full()
            .child(div().size_full().p(space(4.)).child(rows))
            .child(Scrollbar::vertical(&self.list).viewport_from_layout())
            .into_any_element()
    }

    fn mail(&self, mail: &Mailbox, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let message = &mail.message;
        let subject: SharedString = if message.subject.is_empty() {
            t!("(no subject)")
        } else {
            isolate(message.subject.clone())
        };
        let date = message
            .date
            .map(format_full_date)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| message.date_raw.clone());
        let rows: Vec<(SharedString, String)> = [
            (t!("From"), message.from.clone()),
            (t!("To"), message.to.clone()),
            (t!("Cc"), message.cc.clone()),
            (t!("Date"), date),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .collect();
        let mut seen: Vec<String> = Vec::new();
        let chips: Vec<AnyElement> = message
            .attachments
            .iter()
            .enumerate()
            .map(|(at, attachment)| {
                let size = format_size(attachment.data.len() as u64);
                let mut label = format!("{} ({size})", attachment.name);
                if seen.contains(&label) {
                    label = format!("{label} #{}", at + 1);
                }
                seen.push(label.clone());
                Button::new(("small-attachment", at))
                    .ghost()
                    .small()
                    .icon(IconName::Paperclip)
                    .label(SharedString::from(format!(
                        "{}  {size}",
                        isolate(attachment.name.clone())
                    )))
                    .accessibility_label(SharedString::from(label))
                    .tooltip(t!("Open it with its app"))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_attachment(at, cx)))
                    .into_any_element()
            })
            .collect();
        let notes = [
            message
                .html_only
                .then(|| t!("This message is formatted; it is shown here as plain text.")),
            mail.cut.then(|| t!("Only the start of the message is shown.")),
            message
                .remote
                .then(|| t!("Pictures from the internet aren't loaded, so nobody learns it was opened.")),
        ];
        let header = col()
            .id("small-mail-header")
            .role(Role::Group)
            .aria_label(subject.clone())
            .test_support()
            .flex_none()
            .gap(px(2.))
            .p(space(3.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::SM)
            .child(
                row_start()
                    .gap(space(2.))
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon(IconName::Mail).size_4().flex_none().text_color(t.primary))
                    .child(div().min_w_0().child(subject)),
            )
            .child(
                col()
                    .text_size(theme::size::XS)
                    .children(rows.into_iter().map(|(label, value)| {
                        let tip = SharedString::from(value.clone());
                        row_start()
                            .gap(space(3.))
                            .child(
                                div()
                                    .flex_none()
                                    .w(rems(3.5))
                                    .text_color(t.muted_foreground)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("small-mail-{}", tip.len())))
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                                    .child(isolate(value)),
                            )
                    })),
            )
            .when(!chips.is_empty(), |this| {
                this.child(row_start().flex_wrap().gap(space(1.)).pt(space(1.5)).children(chips))
            })
            .children(self.problem.clone().map(|problem| {
                div()
                    .id("small-mail-problem")
                    .role(Role::Alert)
                    .aria_label(problem.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.destructive)
                    .child(problem)
            }))
            .children(notes.into_iter().flatten().enumerate().map(|(at, note)| {
                div()
                    .id(("small-mail-note", at))
                    .role(Role::Status)
                    .aria_label(note.clone())
                    .test_support()
                    .pt(space(1.))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(note)
            }));
        col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&mail.body)
                        .readonly(true)
                        .appearance(false)
                        .bordered(false)
                        .aria_label(t!("The message"))
                        .text_size(theme::size::SM)
                        .size_full(),
                ),
            )
            .into_any_element()
    }
}

impl Render for SmallPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let body = match &self.load {
            Load::Reading => Self::reading(cx),
            Load::Failed(note) => Self::failed(note.clone(), cx),
            Load::Contacts { .. } => self.cards("small-contacts", t!("Contacts"), cx),
            Load::Events(_) => self.cards("small-events", t!("Events"), cx),
            Load::Mail(mail) => {
                let (message, body, cut) = (mail.message.clone(), mail.body.clone(), mail.cut);
                self.mail(&Mailbox { message, body, cut }, cx)
            }
            Load::Picture(path) => div()
                .id("small-picture")
                .role(Role::Group)
                .aria_label(isolate(self.entry.name.clone()))
                .test_support()
                .size_full()
                .p(space(3.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(ImageSource::Resource(gpui_kit::Resource::Path(Arc::from(path.clone()))))
                        .max_w_full()
                        .max_h_full()
                        .rounded(theme::size::RADIUS_SM)
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Load::Source { editor, note } => col()
                .size_full()
                .min_h_0()
                .child(
                    div()
                        .id("small-source-note")
                        .role(Role::Status)
                        .aria_label(note.clone())
                        .test_support()
                        .flex_none()
                        .px(space(3.))
                        .py(space(1.5))
                        .border_b_1()
                        .border_color(t.border)
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(note.clone()),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(editor)
                            .readonly(true)
                            .appearance(false)
                            .bordered(false)
                            .aria_label(isolate(self.entry.name.clone()))
                            .text_size(rems(0.8125))
                            .size_full(),
                    ),
                )
                .into_any_element(),
        };
        div()
            .id("small-preview")
            .size_full()
            .min_h_0()
            .text_color(t.foreground)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::preview::sources;
    use crate::components::preview::tests::{bmp, Fake};
    use crate::explorer::fake::file;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::time::Duration;

    fn b64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    // ---- Contacts and calendars ------------------------------------------------------------------------------

    const PNG_1X1: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    fn vcard() -> String {
        [
            "BEGIN:VCARD",
            "VERSION:3.0",
            "N:علي;محمد;;;",
            "TEL;TYPE=WORK,VOICE:tel:+966 11 555 0100",
            "item1.EMAIL;TYPE=INTERNET;TYPE=HOME:mohammed@example.com",
            "ORG:شركة النور;القسم الأول",
            "ADR;TYPE=CELL:;;شارع الملك\\, الرياض;;;;",
            "NOTE:سطر أول\\nسطر ثان",
            " متابعة",
            &format!("PHOTO;ENCODING=b;TYPE=PNG:{PNG_1X1}"),
            "END:VCARD",
            "BEGIN:VCARD",
            "FN:Second Person",
            "URL:https://example.com",
            "BDAY:19900101",
            "END:VCARD",
        ]
        .join("\r\n")
    }

    #[test]
    fn contacts_are_read_as_the_typescript_did() {
        let cards = read_contacts(&vcard());
        assert_eq!(cards.len(), 2);
        let first = &cards[0];
        // No FN: the N, family name last, reversed.
        assert_eq!(first.name, "محمد علي");
        assert_eq!(
            first.rows,
            vec![
                (Label::Phone(Kind::Work), "+966 11 555 0100".to_string()),
                (Label::Email(Kind::Home), "mohammed@example.com".to_string()),
                (Label::Organization, "شركة النور, القسم الأول".to_string()),
                (Label::Address(Kind::Mobile), "شارع الملك, الرياض".to_string()),
                // The folded line is joined (a line starting with a space continues the one before).
                (Label::Note, "سطر أول\nسطر ثانمتابعة".to_string()),
            ]
        );
        assert!(first
            .photo
            .as_ref()
            .is_some_and(|p| p.starts_with(&[0x89, b'P', b'N', b'G'])));
        assert_eq!(cards[1].name, "Second Person");
        assert_eq!(cards[1].rows[0], (Label::Website, "https://example.com".to_string()));
        assert_eq!(cards[1].rows[1], (Label::Birthday, "19900101".to_string()));
        assert!(read_contacts("not a card").is_empty());
    }

    #[test]
    fn a_photo_may_be_a_data_address() {
        let card = read_contacts(&format!(
            "BEGIN:VCARD\nFN:x\nPHOTO:data:image/png;base64,{PNG_1X1}\nEND:VCARD\n"
        ));
        assert!(card[0].photo.is_some());
    }

    #[test]
    fn labels_say_where_a_number_or_address_belongs() {
        assert_eq!(Label::Phone(Kind::Plain).text(), "Phone");
        assert_eq!(Label::Phone(Kind::Work).text(), "Phone (work)");
        assert_eq!(Label::Email(Kind::Home).text(), "Email (home)");
        assert_eq!(Label::Address(Kind::Mobile).text(), "Address (mobile)");
        assert_eq!(Label::Organization.text(), "Organization");
    }

    fn ics() -> String {
        [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "SUMMARY:اجتماع الفريق",
            "DTSTART:20261109T143000Z",
            "DTEND:20261109T153000Z",
            "LOCATION:قاعة الرياض",
            "DESCRIPTION:جدول\\nالأعمال",
            "ORGANIZER;CN=\"سارة أحمد\":mailto:sara@example.com",
            "ATTENDEE;CN=Bob;ROLE=REQ-PARTICIPANT:mailto:bob@example.com",
            "ATTENDEE:mailto:eve@example.com",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "SUMMARY:Holiday",
            "DTSTART;VALUE=DATE:20261201",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n")
    }

    #[test]
    fn events_are_read_with_their_people_and_times() {
        let events = read_events(&ics());
        assert_eq!(events.len(), 2);
        let meeting = &events[0];
        assert_eq!(meeting.summary, "اجتماع الفريق");
        assert_eq!(meeting.location, "قاعة الرياض");
        assert_eq!(meeting.description, "جدول\nالأعمال");
        assert_eq!(meeting.organizer, "سارة أحمد");
        assert_eq!(
            meeting.attendees,
            vec!["Bob".to_string(), "eve@example.com".to_string()]
        );
        // 2026-11-09 14:30 UTC.
        assert_eq!(meeting.start.ms(), Some(utc_ms(2026, 11, 9, 14, 30, 0)));
        assert_eq!(meeting.end.ms(), Some(utc_ms(2026, 11, 9, 15, 30, 0)));
        assert_eq!(
            events[1].start,
            When::Day {
                year: 2026,
                month: 12,
                day: 1
            }
        );
        assert_eq!(events[1].end, When::None);
    }

    #[test]
    fn dates_parse_as_the_typescript_regex_did() {
        assert_eq!(When::parse(""), When::None);
        assert_eq!(When::parse("tomorrow"), When::Raw("tomorrow".into()));
        assert_eq!(
            When::parse("20261109"),
            When::Day {
                year: 2026,
                month: 11,
                day: 9
            }
        );
        // A time that is cut short is only a date.
        assert_eq!(
            When::parse("20261109T1430"),
            When::Day {
                year: 2026,
                month: 11,
                day: 9
            }
        );
        assert_eq!(
            When::parse("20261109T143005"),
            When::Moment {
                year: 2026,
                month: 11,
                day: 9,
                hour: 14,
                minute: 30,
                second: 5,
                utc: false
            }
        );
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(utc_ms(2000, 1, 1, 0, 0, 0), 946_684_800_000);
        assert!(When::parse("20261109T143000Z").text().contains("2026"));
    }

    // ---- Mail ----------------------------------------------------------------------------------------------------

    fn eml() -> String {
        let subject = format!("=?UTF-8?B?{}?=", b64("رحلة إلى جدة".as_bytes()));
        let from = format!("\"=?UTF-8?B?{}?=\" <ahmad@example.com>", b64("أحمد".as_bytes()));
        let pdf = b64(b"%PDF-1.4 hello");
        [
            format!("From: {from}"),
            "To: Sara <sara@example.com>, bob@example.com".to_string(),
            "Cc: \"Doe, Jane\" <jane@example.com>".to_string(),
            format!("Subject: {subject}"),
            "Date: Mon, 9 Nov 2026 14:30:00 +0300".to_string(),
            "MIME-Version: 1.0".to_string(),
            "Content-Type: multipart/mixed; boundary=\"outer\"".to_string(),
            String::new(),
            "--outer".to_string(),
            "Content-Type: multipart/alternative; boundary=inner".to_string(),
            String::new(),
            "--inner".to_string(),
            "Content-Type: text/plain; charset=utf-8".to_string(),
            "Content-Transfer-Encoding: quoted-printable".to_string(),
            String::new(),
            "Hello =D8=B3=D9=84=D8=A7=D9=85 soft=\r\nbreak".to_string(),
            "--inner".to_string(),
            "Content-Type: text/html; charset=utf-8".to_string(),
            String::new(),
            "<p>Hello <img src=\"https://x.test/p.gif\"></p>".to_string(),
            "--inner--".to_string(),
            String::new(),
            "--outer".to_string(),
            "Content-Type: application/pdf; name=\"x.pdf\"".to_string(),
            "Content-Disposition: attachment; filename*=UTF-8''%D8%AA%D9%82%D8%B1%D9%8A%D8%B1.pdf".to_string(),
            "Content-Transfer-Encoding: base64".to_string(),
            String::new(),
            pdf,
            "--outer".to_string(),
            "Content-Type: image/png".to_string(),
            "Content-Disposition: inline".to_string(),
            "Content-ID: <logo@x>".to_string(),
            "Content-Transfer-Encoding: base64".to_string(),
            String::new(),
            "AAAA".to_string(),
            "--outer--".to_string(),
            String::new(),
        ]
        .join("\r\n")
    }

    #[test]
    fn an_eml_has_its_headers_text_and_attachments() {
        let message = read_email(eml().as_bytes());
        assert_eq!(message.subject, "رحلة إلى جدة");
        assert_eq!(message.from, "أحمد <ahmad@example.com>");
        assert_eq!(message.to, "Sara <sara@example.com>, bob@example.com");
        assert_eq!(message.cc, "Doe, Jane <jane@example.com>");
        // 14:30 at +03:00 is 11:30 UTC.
        assert_eq!(message.date, Some(utc_ms(2026, 11, 9, 11, 30, 0)));
        assert_eq!(message.text, "Hello سلام softbreak");
        assert!(!message.html_only);
        assert!(message.remote, "the HTML names a picture on the internet");
        // The logo the HTML points at (inline, with a Content-ID) is not a file to open.
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].name, "تقرير.pdf");
        assert_eq!(message.attachments[0].data.as_slice(), b"%PDF-1.4 hello");
    }

    #[test]
    fn a_message_of_html_alone_becomes_its_text() {
        let raw = "Subject: Hi\nContent-Type: text/html; charset=iso-8859-1\nContent-Transfer-Encoding: quoted-printable\n\n<html><head><style>p{}</style><title>t</title></head><body><p>Caf=E9 &amp; more</p><ul><li>one</li><li>two</li></ul><script>alert(1)</script></body></html>\n";
        let message = read_email(raw.as_bytes());
        assert!(message.html_only);
        assert_eq!(message.text, "Café & more\n\n• one\n• two");
        assert!(!message.remote);
    }

    #[test]
    fn html_becomes_lines_of_text() {
        assert_eq!(html_to_text("a<br>b<!-- hidden -->c"), "a\nbc");
        assert_eq!(
            html_to_text("<div>one</div><div>two &lt;3 &#x41; &#66;</div>"),
            "one\ntwo <3 A B"
        );
        assert_eq!(html_to_text("  spaced \n\t out  "), "spaced out");
        assert_eq!(html_to_text("<p>مرحبا</p><p>عالم</p>"), "مرحبا\n\nعالم");
    }

    #[test]
    fn encoded_words_and_parameters_are_decoded() {
        assert_eq!(decode_words("=?utf-8?Q?caf=C3=A9_au_lait?="), "café au lait");
        // Spaces between two encoded words go; the text around them stays.
        assert_eq!(decode_words("a =?utf-8?Q?b?= =?utf-8?Q?c?= d"), "a bc d");
        assert_eq!(decode_words("=?broken"), "=?broken");
        let (kind, params) = typed("text/plain; charset=\"utf-8\"; format=flowed");
        assert_eq!(kind, "text/plain");
        assert_eq!(param(&params, "charset").as_deref(), Some("utf-8"));
        // RFC 2231 in pieces.
        let (_, params) = typed("attachment; filename*0*=UTF-8''%D8%AA; filename*1*=%D9%82; filename*2=.txt");
        assert_eq!(param(&params, "filename").as_deref(), Some("تق.txt"));
    }

    #[test]
    fn dates_in_headers_have_zones() {
        assert_eq!(
            parse_mail_date("Fri, 06 Nov 2026 08:00:00 GMT"),
            Some(utc_ms(2026, 11, 6, 8, 0, 0))
        );
        assert_eq!(
            parse_mail_date("6 Nov 2026 08:00 -0500 (EST)"),
            Some(utc_ms(2026, 11, 6, 13, 0, 0))
        );
        assert_eq!(parse_mail_date("yesterday"), None);
    }

    // ---- Outlook messages ------------------------------------------------------------------------------------------

    struct Node {
        name: String,
        data: Option<Vec<u8>>,
        kids: Vec<Node>,
    }

    fn storage(name: &str, kids: Vec<Node>) -> Node {
        Node {
            name: name.into(),
            data: None,
            kids,
        }
    }

    fn stream(name: &str, data: Vec<u8>) -> Node {
        Node {
            name: name.into(),
            data: Some(data),
            kids: Vec::new(),
        }
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).chain([0, 0]).collect()
    }

    fn props(header: usize, entries: &[(u16, u16, u64)]) -> Vec<u8> {
        let mut out = vec![0u8; header];
        for &(id, ty, value) in entries {
            out.extend(ty.to_le_bytes());
            out.extend(id.to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(value.to_le_bytes());
        }
        out
    }

    struct Entry {
        name: String,
        kind: u8,
        child: u32,
        right: u32,
        data: Vec<u8>,
    }

    fn flatten(kids: Vec<Node>, out: &mut Vec<Entry>) -> u32 {
        let mut ids = Vec::new();
        for kid in kids {
            let id = out.len();
            out.push(Entry {
                name: kid.name,
                kind: if kid.data.is_some() { 2 } else { 1 },
                child: FREE,
                right: FREE,
                data: kid.data.unwrap_or_default(),
            });
            let child = flatten(kid.kids, out);
            out[id].child = child;
            ids.push(id);
        }
        for pair in ids.windows(2) {
            out[pair[0]].right = pair[1] as u32;
        }
        ids.first().map_or(FREE, |&id| id as u32)
    }

    /// A compound file with 512-byte sectors, small streams in the mini stream and the rest in sectors.
    #[allow(clippy::needless_range_loop)]
    fn build_cfb(kids: Vec<Node>) -> Vec<u8> {
        let mut entries = vec![Entry {
            name: "Root Entry".into(),
            kind: 5,
            child: FREE,
            right: FREE,
            data: Vec::new(),
        }];
        let first = flatten(kids, &mut entries);
        entries[0].child = first;
        let mut mini = Vec::new();
        let mut mini_fat: Vec<u32> = Vec::new();
        let mut starts = vec![END_OF_CHAIN; entries.len()];
        for (i, e) in entries.iter().enumerate() {
            if e.kind == 2 && !e.data.is_empty() && e.data.len() < 4096 {
                let begin = mini_fat.len() as u32;
                let units = e.data.len().div_ceil(64);
                for u in 0..units {
                    mini_fat.push(if u + 1 == units {
                        END_OF_CHAIN
                    } else {
                        begin + u as u32 + 1
                    });
                }
                mini.extend(&e.data);
                mini.resize(mini_fat.len() * 64, 0);
                starts[i] = begin;
            }
        }
        let dir_sectors = entries.len().div_ceil(4);
        // Sector 0 is the allocation table, 1 the mini table, then the directory.
        let mut fat: Vec<u32> = vec![0xFFFF_FFFD, END_OF_CHAIN];
        for d in 0..dir_sectors {
            fat.push(if d + 1 == dir_sectors {
                END_OF_CHAIN
            } else {
                fat.len() as u32 + 1
            });
        }
        let mut data_sectors: Vec<Vec<u8>> = Vec::new();
        let mut alloc = |fat: &mut Vec<u32>, bytes: &[u8]| -> u32 {
            let begin = fat.len() as u32;
            let count = bytes.len().div_ceil(512);
            for c in 0..count {
                fat.push(if c + 1 == count {
                    END_OF_CHAIN
                } else {
                    begin + c as u32 + 1
                });
                let mut sector = bytes[c * 512..bytes.len().min((c + 1) * 512)].to_vec();
                sector.resize(512, 0);
                data_sectors.push(sector);
            }
            begin
        };
        let mini_start = if mini.is_empty() {
            END_OF_CHAIN
        } else {
            alloc(&mut fat, &mini)
        };
        for (i, e) in entries.iter().enumerate() {
            if e.kind == 2 && e.data.len() >= 4096 {
                starts[i] = alloc(&mut fat, &e.data);
            }
        }
        starts[0] = mini_start;
        let mut file = vec![0u8; 512];
        file[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        file[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
        file[26..28].copy_from_slice(&3u16.to_le_bytes());
        file[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
        file[30..32].copy_from_slice(&9u16.to_le_bytes());
        file[32..34].copy_from_slice(&6u16.to_le_bytes());
        file[44..48].copy_from_slice(&1u32.to_le_bytes());
        file[48..52].copy_from_slice(&2u32.to_le_bytes());
        file[56..60].copy_from_slice(&4096u32.to_le_bytes());
        file[60..64].copy_from_slice(&1u32.to_le_bytes());
        file[64..68].copy_from_slice(&1u32.to_le_bytes());
        file[68..72].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        for i in 0..109 {
            let v = if i == 0 { 0 } else { FREE };
            file[76 + i * 4..80 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        let table = |values: &[u32]| {
            let mut sector = vec![0xFFu8; 512];
            for (i, v) in values.iter().enumerate() {
                sector[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            sector
        };
        assert!(
            fat.len() <= 128 && mini_fat.len() <= 128,
            "the test file fits one table"
        );
        file.extend(table(&fat));
        file.extend(table(&mini_fat));
        let mut directory = vec![0u8; dir_sectors * 512];
        for slot in 0..dir_sectors * 4 {
            let at = slot * 128;
            let entry = &mut directory[at..at + 128];
            entry[68..80].copy_from_slice(&[0xFF; 12]);
            if let Some(e) = entries.get(slot) {
                let units: Vec<u16> = e.name.encode_utf16().collect();
                for (i, u) in units.iter().enumerate() {
                    entry[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
                }
                entry[64..66].copy_from_slice(&(((units.len() + 1) * 2) as u16).to_le_bytes());
                entry[66] = e.kind;
                entry[67] = 1;
                entry[72..76].copy_from_slice(&e.right.to_le_bytes());
                entry[76..80].copy_from_slice(&e.child.to_le_bytes());
                entry[116..120].copy_from_slice(&starts[slot].to_le_bytes());
                let size = if e.kind == 5 { mini.len() } else { e.data.len() };
                entry[120..124].copy_from_slice(&(size as u32).to_le_bytes());
            }
        }
        file.extend(directory);
        for sector in data_sectors {
            file.extend(sector);
        }
        file
    }

    fn msg() -> Vec<u8> {
        let delivered = (utc_ms(2026, 11, 9, 14, 30, 0) + 11_644_473_600_000) as u64 * 10_000;
        let big: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        build_cfb(vec![
            stream("__substg1.0_0037001F", utf16("رسالة تجريبية")),
            stream("__substg1.0_1000001F", utf16("مرحبا\r\nسطر ثان")),
            stream("__substg1.0_0C1A001F", utf16("أحمد")),
            stream("__substg1.0_5D01001F", utf16("ahmad@example.com")),
            stream("__properties_version1.0", props(32, &[(0x0E06, 0x0040, delivered)])),
            storage(
                "__recip_version1.0_#00000000",
                vec![
                    stream("__substg1.0_3001001F", utf16("سارة")),
                    stream("__substg1.0_39FE001F", utf16("sara@example.com")),
                    stream("__properties_version1.0", props(8, &[(0x0C15, 0x0003, 1)])),
                ],
            ),
            storage(
                "__recip_version1.0_#00000001",
                vec![
                    stream("__substg1.0_3001001F", utf16("Bob")),
                    stream("__substg1.0_39FE001F", utf16("bob@example.com")),
                    stream("__properties_version1.0", props(8, &[(0x0C15, 0x0003, 2)])),
                ],
            ),
            storage(
                "__attach_version1.0_#00000000",
                vec![
                    stream("__substg1.0_3707001F", utf16("تقرير.bin")),
                    stream("__substg1.0_37010102", big),
                ],
            ),
            storage(
                "__attach_version1.0_#00000001",
                vec![
                    stream("__substg1.0_3707001F", utf16("small.txt")),
                    stream("__substg1.0_37010102", b"tiny".to_vec()),
                ],
            ),
            storage(
                "__attach_version1.0_#00000002",
                vec![
                    stream("__substg1.0_3707001F", utf16("hidden.png")),
                    stream("__substg1.0_37010102", b"png".to_vec()),
                    stream("__properties_version1.0", props(8, &[(0x7FFE, 0x000B, 1)])),
                ],
            ),
        ])
    }

    #[test]
    fn an_outlook_message_has_its_people_text_and_attachments() {
        let message = read_outlook(&msg()).expect("the message is read");
        assert_eq!(message.subject, "رسالة تجريبية");
        assert_eq!(message.from, "أحمد <ahmad@example.com>");
        assert_eq!(message.to, "سارة <sara@example.com>");
        assert_eq!(message.cc, "Bob <bob@example.com>");
        assert_eq!(message.text, "مرحبا\nسطر ثان");
        assert_eq!(message.date, Some(utc_ms(2026, 11, 9, 14, 30, 0)));
        let names: Vec<&str> = message.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["تقرير.bin", "small.txt"], "the hidden one is left out");
        // The big one came out of sectors, the small one out of the mini stream.
        let expected: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(message.attachments[0].data.as_slice(), expected.as_slice());
        assert_eq!(message.attachments[1].data.as_slice(), b"tiny");
    }

    #[test]
    fn what_is_not_a_compound_file_is_refused() {
        assert!(read_outlook(b"plain text").is_err());
        let mut damaged = msg();
        damaged.truncate(700);
        // A cut file doesn't crash the reader.
        let _ = read_outlook(&damaged);
    }

    // ---- The views --------------------------------------------------------------------------------------------------

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-small-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the temp folder is made");
        dir
    }

    fn entry_of(dir: &std::path::Path, name: &str, bytes: &[u8]) -> FileEntry {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("the file is written");
        let mut entry = file("C:", name, bytes.len() as u64, 0);
        entry.path = path.to_string_lossy().into_owned();
        entry
    }

    fn open(
        cx: &mut TestAppContext,
        entry: FileEntry,
        kind: SmallKind,
        rtl: bool,
    ) -> (Entity<SmallPreview>, VisualTestContext) {
        cx.update(testing::init);
        testing::set_rtl(rtl);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| SmallPreview::new(entry, kind, window, cx))
        });
        (view, visual)
    }

    fn settle(view: &Entity<SmallPreview>, visual: &mut VisualTestContext) {
        for _ in 0..600 {
            visual.run_until_parked();
            if view.read_with(visual, |v, _| v.is_settled()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(view.read_with(visual, |v, _| v.is_settled()), "the file was never read");
        for _ in 0..3 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    fn has(visual: &mut VisualTestContext, label: &str) -> bool {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn failure(view: &Entity<SmallPreview>, visual: &VisualTestContext) -> Option<String> {
        view.read_with(visual, |v, _| v.failure().map(str::to_string))
    }

    #[gpui_kit::test]
    fn contacts_are_cards_with_an_arabic_name(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("contacts");
        let entry = entry_of(&dir, "جهات.vcf", vcard().as_bytes());
        let (view, mut visual) = open(cx, entry, SmallKind::Contact, true);
        settle(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.contacts().map(<[Card]>::len)), Some(2));
        assert!(has(&mut visual, &isolate("محمد علي")), "the Arabic contact's card");
        assert!(has(&mut visual, "Second Person"));
        assert!(has(&mut visual, "Contacts"));
        let _ = std::fs::remove_dir_all(&dir);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_file_with_no_contacts_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("nocontacts");
        let entry = entry_of(&dir, "empty.vcf", b"nothing here");
        let (view, mut visual) = open(cx, entry, SmallKind::Contact, false);
        settle(&view, &mut visual);
        assert_eq!(failure(&view, &visual).as_deref(), Some("No contacts in it."));
        assert!(has(&mut visual, "No contacts in it."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn events_show_when_where_and_who(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("events");
        let entry = entry_of(&dir, "تقويم.ics", ics().as_bytes());
        let (view, mut visual) = open(cx, entry, SmallKind::Calendar, true);
        settle(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.events().map(<[Event]>::len)), Some(2));
        assert!(has(&mut visual, &isolate("اجتماع الفريق")), "the Arabic event's card");
        assert!(has(&mut visual, "Holiday"));
        let _ = std::fs::remove_dir_all(&dir);
        testing::set_rtl(false);
    }

    fn fake_hooks(cx: &mut TestAppContext, log: Rc<RefCell<Vec<String>>>, fail: bool) {
        let saved = log.clone();
        let opened = log;
        cx.update(|cx| {
            init_with(
                cx,
                Hooks {
                    save: Rc::new(move |name, data| {
                        saved.borrow_mut().push(format!("save {name} {} bytes", data.len()));
                        Box::pin(async move {
                            if fail {
                                Err("The disk is full".to_string())
                            } else {
                                Ok(format!("C:\\Temp\\Glint attachments\\1\\{name}"))
                            }
                        })
                    }),
                    open: Rc::new(move |path, _| opened.borrow_mut().push(format!("open {path}"))),
                },
            )
        });
    }

    #[gpui_kit::test]
    fn a_mail_shows_headers_and_opens_an_attachment_through_the_hooks(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("mail");
        let entry = entry_of(&dir, "رسالة.eml", eml().as_bytes());
        let log = Rc::new(RefCell::new(Vec::new()));
        cx.update(testing::init);
        fake_hooks(cx, log.clone(), false);
        let (view, mut visual) = open(cx, entry, SmallKind::Email, true);
        settle(&view, &mut visual);
        assert_eq!(
            view.read_with(&visual, |v, _| v.message().map(|m| m.subject.clone()))
                .as_deref(),
            Some("رحلة إلى جدة")
        );
        assert!(has(&mut visual, &isolate("رحلة إلى جدة")), "the subject, an Arabic one");
        let size = format_size(14);
        assert!(
            has(&mut visual, &format!("تقرير.pdf ({size})")),
            "the attachment's chip"
        );
        assert!(has(
            &mut visual,
            "Pictures from the internet aren't loaded, so nobody learns it was opened."
        ));
        assert!(has(&mut visual, "The message"));
        visual.update(|window, cx| window.click(("small-attachment", 0usize), cx));
        visual.run_until_parked();
        assert_eq!(
            *log.borrow(),
            vec![
                "save تقرير.pdf 14 bytes".to_string(),
                "open C:\\Temp\\Glint attachments\\1\\تقرير.pdf".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn an_attachment_that_cannot_be_saved_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("mailfail");
        let entry = entry_of(&dir, "m.eml", eml().as_bytes());
        let log = Rc::new(RefCell::new(Vec::new()));
        cx.update(testing::init);
        fake_hooks(cx, log.clone(), true);
        let (view, mut visual) = open(cx, entry, SmallKind::Email, false);
        settle(&view, &mut visual);
        visual.update(|window, cx| window.click(("small-attachment", 0usize), cx));
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(
            view.read_with(&visual, |v, _| v.attachment_problem().map(str::to_string))
                .as_deref(),
            Some("The disk is full")
        );
        assert!(has(&mut visual, "The disk is full"));
        assert_eq!(log.borrow().len(), 1, "nothing was opened");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn an_outlook_message_and_a_damaged_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("outlook");
        let entry = entry_of(&dir, "رسالة.msg", &msg());
        let (view, mut visual) = open(cx, entry, SmallKind::Outlook, true);
        settle(&view, &mut visual);
        assert_eq!(
            view.read_with(&visual, |v, _| v.message().map(|m| m.attachments.len())),
            Some(2)
        );
        assert!(has(&mut visual, &isolate("رسالة تجريبية")));
        assert!(has(&mut visual, &format!("small.txt ({})", format_size(4))));
        let bad = entry_of(
            &dir,
            "bad.msg",
            b"not a compound file at all, just some bytes that are long enough to look like one......",
        );
        let (view, mut visual) = open(cx, bad, SmallKind::Outlook, false);
        settle(&view, &mut visual);
        let failed = failure(&view, &visual).unwrap_or_default();
        assert!(
            failed.starts_with("Glint couldn't read this Outlook message:"),
            "{failed}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_design_file_shows_the_picture_it_keeps_of_itself(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("design");
        let entry = entry_of(&dir, "رسم.kra", b"PK");
        let picture = dir.join("merged.bmp");
        std::fs::write(&picture, bmp(4, 2, [0x20, 0x40, 0x80])).expect("the picture is written");
        let fake = Rc::new(Fake::default());
        fake.extracted.borrow_mut().insert(
            format!("{}\\mergedimage.png", entry.path),
            picture.to_string_lossy().into_owned(),
        );
        cx.update(testing::init);
        cx.update(|cx| sources::init_with(cx, fake.sources()));
        let (view, mut visual) = open(cx, entry.clone(), SmallKind::Design, true);
        settle(&view, &mut visual);
        assert_eq!(
            view.read_with(&visual, |v, _| v.picture().map(<std::path::Path>::to_path_buf)),
            Some(picture)
        );
        assert!(
            has(&mut visual, &isolate("رسم.kra")),
            "the picture is named by the file"
        );
        // A file that keeps none says so.
        let none = entry_of(&dir, "none.ora", b"PK");
        let (view, mut visual) = open(cx, none, SmallKind::Design, false);
        settle(&view, &mut visual);
        assert_eq!(
            failure(&view, &visual).as_deref(),
            Some("It keeps no picture of itself to show.")
        );
        let _ = std::fs::remove_dir_all(&dir);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn graphviz_and_mermaid_show_their_source_until_a_renderer_exists(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("diagrams");
        let entry = entry_of(&dir, "graph.gv", "digraph { مرحبا -> world }".as_bytes());
        let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, false);
        settle(&view, &mut visual);
        assert!(has(
            &mut visual,
            "Glint can't draw Graphviz graphs yet, so the file is shown as written."
        ));
        let entry = entry_of(&dir, "flow.mmd", b"graph TD; A-->B");
        let (view, mut visual) = open(cx, entry, SmallKind::Mermaid, false);
        settle(&view, &mut visual);
        assert!(has(
            &mut visual,
            "Glint can't draw Mermaid diagrams yet, so the file is shown as written."
        ));
        // .dot is also Word 97's template extension.
        let word = entry_of(
            &dir,
            "letter.dot",
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0],
        );
        let (view, mut visual) = open(cx, word, SmallKind::Graphviz, false);
        settle(&view, &mut visual);
        assert_eq!(
            failure(&view, &visual).as_deref(),
            Some("This .dot file is a Word template, which Word opens.")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_kinds_are_told_by_the_extension() {
        assert_eq!(small_kind(&file("C:", "a.vcf", 1, 0)), Some(SmallKind::Contact));
        assert_eq!(small_kind(&file("C:", "a.msg", 1, 0)), Some(SmallKind::Outlook));
        assert_eq!(small_kind(&file("C:", "a.txt", 1, 0)), None);
    }

    #[test]
    fn attachment_names_travel_percent_encoded() {
        assert_eq!(percent_encoded("report (1).pdf"), "report (1).pdf");
        assert_eq!(percent_encoded("تق.pdf"), "%D8%AA%D9%82.pdf");
        assert_eq!(percent_encoded("a%b"), "a%25b");
    }
}
