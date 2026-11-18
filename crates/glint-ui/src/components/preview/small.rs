//! Contacts, calendars and design files, and Graphviz sources (SmallPreviews.tsx).
//!
//! | TypeScript                                   | Here                                                               |
//! |----------------------------------------------|--------------------------------------------------------------------|
//! | `SmallPreview` (by `SmallKind`)              | [`SmallPreview`], one view for every kind; [`small_kind`] says which |
//! | `lines`, `property`, `readContacts`          | [`read_contacts`] (pure; tested)                                   |
//! | `icsDate`, `readEvents`                      | [`read_events`], [`When`] (pure; tested)                           |
//! | `Contacts`, `Calendar`                       | cards in a virtualized `list`                                      |
//! | `DesignPicture` (zip.js)                     | the archive layer (`sources.extract`) reads the picture out of the file; drawn with `img` |
//! | `Graphviz`                                   | the diagram drawn ([`super::diagram`]), with the source one button away |
//!
//! Email (`.eml`) and Outlook (`.msg`) messages had a viewer here until 0.14 (a MIME reader, HTML mail laid out as
//! a page, a compound-file reader for `.msg`); they are text or Windows' previewer's business now.
//!
//! # Where this goes (for the coordinator)
//!
//! - `choose.rs`: the arm is its own `Body::Small(SmallKind)`, `Choice::of(..)` with no `later` for the kinds this module draws; [`small_kind`] is the
//!   test (`glint_model::small_kinds::small_kind_of(&entry.ext)`, readable and not archived, as now).
//! - `frame.rs`'s `build`: `Body::Small(kind) => cx.new(|cx| SmallPreview::new(entry.clone(), kind, window, cx)).into()`.
//!   The view needs no other event.
//!
//! # Not ported
//!
//! - **Graphviz files** are drawn (layout-rs for `.dot` and `.gv`: see [`super::diagram`] for how it differs from
//!   Graphviz), fitted to the width and scrolled down, as the TypeScript's `h-full overflow-auto p-3` was. A button
//!   toggles the source (the read-only editor it always was); a file that can't be drawn shows "Graphviz couldn't
//!   draw it: …" above its source, where the TypeScript replaced the preview with the message. Not ported: zoom (the TypeScript had none), the white page
//!   behind a Graphviz graph (it takes the app's background and text colour), a Word 97 template named `.dot` still
//!   says what it is.
//! - Dates in an event are written with Windows' long date (`format_full_date`), which has seconds
//!   where the TypeScript's `timeStyle: "short"` had none; a date alone is written short.

use super::sources::sources;
use super::states::centered;
use super::{diagram, svg_image};
use crate::kit::prelude::*;
use crate::kit::{Editor, EditorState};
use crate::{t, theme};
use glint_model::entries::FileEntry;
use glint_model::format::{format_date, format_full_date};
#[cfg(test)]
use glint_model::small_kinds::small_kind_of;
use glint_model::small_kinds::SmallKind;
use gpui_kit::{list, Image, ImageFormat, ImageSource, ListAlignment, ListState, ObjectFit, StyledImage as _, Task};
use std::path::PathBuf;
use std::sync::Arc;

/// The biggest file read for a small preview.
const MAX_BYTES: u64 = 32 << 20;
/// Contacts shown (`slice(0, 500)`) and events shown (`slice(0, 200)`).
const MAX_CARDS: usize = 500;
const MAX_EVENTS: usize = 200;
/// The most of a message's text, or a diagram's source, put into the editor.
const MAX_TEXT: usize = 1 << 20;
/// The picture a Krita, OpenRaster, Procreate or Sketch file keeps of itself, in the order tried.
pub const DESIGN_PICTURES: [&str; 5] = [
    "mergedimage.png",
    "Thumbnail.png",
    "QuickLook\\Thumbnail.png",
    "previews\\preview.png",
    "preview.png",
];

/// Which small preview a file gets, if any (`smallKind(entry.ext)`).
#[cfg(test)]
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

// ---- The view ------------------------------------------------------------------------------------------------

/// Why a file couldn't be shown.
enum Problem {
    Unreadable(String),
    TooBig,
    NoContacts,
    NoEvents,
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
            Problem::WordTemplate => t!("This .dot file is a Word template, which Word opens."),
            Problem::NoPicture => t!("It keeps no picture of itself to show."),
        }
    }
}

/// What was read, off the UI thread.
enum Loaded {
    Contacts(Vec<Card>),
    Events(Vec<Event>),
    Picture(PathBuf),
    Source(String),
}

enum Load {
    Reading,
    Failed(SharedString),
    Contacts {
        cards: Arc<Vec<Card>>,
        photos: Arc<Vec<Option<Arc<Image>>>>,
    },
    Events(Arc<Vec<Event>>),
    Picture(PathBuf),
    Source {
        editor: Entity<EditorState>,
        /// The diagram the file draws, and whether its source shows instead.
        drawing: Drawing,
    },
}

/// A diagram file: what it is, what it says, and which of the picture and the source shows.
struct Drawing {
    language: diagram::Language,
    source: Arc<str>,
    show_source: bool,
}

/// A small preview: one file of one kind.
pub struct SmallPreview {
    entry: FileEntry,
    kind: SmallKind,
    load: Load,
    list: ListState,
    focus: FocusHandle,
    task: Option<Task<()>>,
    /// The scroll of a drawn diagram, for Quick Look's page keys.
    diagram_scroll: gpui_kit::ScrollHandle,
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
        SmallKind::Graphviz => diagram_source(&bytes).map(Loaded::Source),
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
            task: None,
            diagram_scroll: gpui_kit::ScrollHandle::new(),
        };
        // Pictures still being drawn for a view that is gone are not wanted.
        {
            let id = cx.entity_id();
            cx.on_release(move |_, _| svg_image::forget(id)).detach();
        }
        view.start(window, cx);
        view
    }

    /// The kind of file this shows.
    pub fn kind(&self) -> SmallKind {
        self.kind
    }

    /// Whether the file has been read (or failed).
    #[cfg(test)]
    pub fn is_settled(&self) -> bool {
        !matches!(self.load, Load::Reading)
    }

    /// The contacts, once read.
    #[cfg(test)]
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

    /// Why the file couldn't be shown.
    pub fn failure(&self) -> Option<&str> {
        match &self.load {
            Load::Failed(note) => Some(note),
            _ => None,
        }
    }

    /// The picture a design file keeps of itself (a copy of it in the temp folder), once found.
    pub fn picture(&self) -> Option<&std::path::Path> {
        match &self.load {
            Load::Picture(path) => Some(path),
            _ => None,
        }
    }

    /// Scrolls a page (Page Down and Page Up in Quick Look): the contacts, events and a diagram taller than the pane.
    /// Returns whether anything moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if let Load::Source { drawing, .. } = &self.load {
            if drawing.show_source {
                return false;
            }
            let max = self.diagram_scroll.max_offset().y;
            if max <= px(0.) {
                return false;
            }
            let offset = self.diagram_scroll.offset();
            let step = self.diagram_scroll.bounds().size.height * 0.85;
            let top = if down { offset.y - step } else { offset.y + step }.clamp(-max, px(0.));
            if top == offset.y {
                return false;
            }
            self.diagram_scroll.set_offset(gpui_kit::point(offset.x, top));
            cx.notify();
            return true;
        }
        if !matches!(self.load, Load::Contacts { .. } | Load::Events(_)) {
            return false;
        }
        let before = self.list.logical_scroll_top();
        let step = self.list.viewport_bounds().size.height * 0.85;
        self.list.scroll_by(if down { step } else { -step });
        cx.notify();
        self.list.logical_scroll_top() != before
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        svg_image::forget(cx.entity_id());
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
                {
                    let language = diagram::Language::Dot;
                    Load::Source {
                        editor,
                        drawing: Drawing {
                            language,
                            source: Arc::from(text.as_str()),
                            show_source: false,
                        },
                    }
                }
            }
        };
        cx.notify();
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
}

impl SmallPreview {
    /// A diagram file's source in the read-only editor, with `note` above it (why it is shown so, or why it
    /// couldn't be drawn).
    fn source_view(
        entry: &FileEntry,
        editor: &Entity<EditorState>,
        note: Option<SharedString>,
        t: &theme::Tokens,
    ) -> AnyElement {
        col()
            .size_full()
            .min_h_0()
            .children(note.map(|note| {
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
                    .child(note)
            }))
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(editor)
                        .readonly(true)
                        .appearance(false)
                        .bordered(false)
                        .aria_label(isolate(entry.name.clone()))
                        .text_size(rems(0.8125))
                        .size_full(),
                ),
            )
            .into_any_element()
    }

    /// A Graphviz or Mermaid file: the diagram drawn, fitted to the width and scrolled down (`h-full overflow-auto
    /// p-3`), or its source; a button between them. A diagram that can't be drawn shows why above its source.
    fn diagram_file(
        &self,
        editor: &Entity<EditorState>,
        drawing: &Drawing,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wake = cx.entity_id();
        let outcome = diagram::request(cx, drawing.language, &drawing.source, wake);
        let failed = match &outcome {
            svg_image::Lookup::Done(done) => done.as_ref().as_ref().err().cloned(),
            svg_image::Lookup::Making => None,
        };
        if let Some(reason) = failed {
            let note = drawing.language.failure_in_file(&reason);
            return Self::source_view(&self.entry, editor, Some(note), t);
        }
        if drawing.show_source {
            return col()
                .size_full()
                .min_h_0()
                .child(self.diagram_toolbar(drawing, cx))
                .child(Self::source_view(&self.entry, editor, None, t))
                .into_any_element();
        }
        let body = match &outcome {
            svg_image::Lookup::Done(done) => match done.as_ref() {
                Ok(drawn) => div()
                    .id("small-diagram")
                    .track_scroll(&self.diagram_scroll)
                    .role(Role::Group)
                    .aria_label(isolate(self.entry.name.clone()))
                    .aria_description(drawing.language.picture_name())
                    .test_support()
                    .size_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(space(3.))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .child(svg_image::picture(
                                "small-diagram-picture",
                                drawn,
                                isolate(self.entry.name.clone()),
                            )),
                    )
                    .into_any_element(),
                Err(_) => div().into_any_element(),
            },
            svg_image::Lookup::Making => Self::reading(cx),
        };
        col()
            .size_full()
            .min_h_0()
            .child(self.diagram_toolbar(drawing, cx))
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// The row above a drawn diagram: the button that shows its source (or the picture again).
    fn diagram_toolbar(&self, drawing: &Drawing, cx: &mut Context<Self>) -> Div {
        let label = if drawing.show_source {
            t!("Show the diagram")
        } else {
            t!("Show the source")
        };
        row().flex_none().justify_end().px(space(2.)).py(space(1.)).child(
            Button::new("small-diagram-toggle")
                .ghost()
                .small()
                .label(label.clone())
                .accessibility_label(label)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Load::Source { drawing, .. } = &mut this.load {
                        drawing.show_source = !drawing.show_source;
                        cx.notify();
                    }
                })),
        )
    }
}

impl Render for SmallPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        svg_image::release_retired(window);
        let t = theme::tokens(cx).clone();
        let body = match &self.load {
            Load::Reading => Self::reading(cx),
            Load::Failed(note) => Self::failed(note.clone(), cx),
            Load::Contacts { .. } => self.cards("small-contacts", t!("Contacts"), cx),
            Load::Events(_) => self.cards("small-events", t!("Events"), cx),
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
            Load::Source { editor, drawing } => self.diagram_file(editor, drawing, &t, cx),
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
    use std::rc::Rc;
    use std::time::Duration;

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

    #[test]
    fn small_kinds_are_told_by_the_extension() {
        assert_eq!(small_kind(&file("C:", "a.vcf", 1, 0)), Some(SmallKind::Contact));
        assert_eq!(small_kind(&file("C:", "a.ics", 1, 0)), Some(SmallKind::Calendar));
        // Mail has no viewer of its own.
        assert_eq!(small_kind(&file("C:", "a.eml", 1, 0)), None);
        assert_eq!(small_kind(&file("C:", "a.msg", 1, 0)), None);
        assert_eq!(small_kind(&file("C:", "a.txt", 1, 0)), None);
    }

    // ---- Diagram files (feature `diagrams`) -------------------------------------------------------------------------

    mod drawn {
        use super::*;
        use crate::components::preview::{diagram, svg_image};
        use gpui_kit::Role;

        /// The labelled elements of the last frame: role, label, width, height.
        fn labelled(visual: &mut VisualTestContext) -> Vec<(Option<Role>, String, f32, f32)> {
            visual.update(|window, cx| {
                window.render_frame(cx);
                window
                    .elements()
                    .into_iter()
                    .filter_map(|e| {
                        let label = e.label()?.to_string();
                        let size = e.bounds().size;
                        Some((e.role(), label, f32::from(size.width), f32::from(size.height)))
                    })
                    .collect()
            })
        }

        fn with_role(visual: &mut VisualTestContext, role: Role) -> Vec<(String, f32, f32)> {
            labelled(visual)
                .into_iter()
                .filter(|(r, ..)| *r == Some(role))
                .map(|(_, l, w, h)| (l, w, h))
                .collect()
        }

        fn wait(visual: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
            for _ in 0..1200 {
                visual.run_until_parked();
                if done(visual) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("never happened: {what}: {:?}", labelled(visual));
        }

        fn picture_is_shown(visual: &mut VisualTestContext, name: &str) -> bool {
            with_role(visual, Role::Image)
                .iter()
                .any(|(label, ..)| label.contains(name))
        }

        #[gpui_kit::test]
        fn dot_files_are_drawn_and_their_source_is_a_button_away(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            svg_image::clear();
            let dir = temp_dir("drawn");
            let entry = entry_of(&dir, "flow.gv", b"digraph { rankdir=LR; Start -> Ok; Ok -> Done }");
            let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, false);
            settle(&view, &mut visual);
            wait(&mut visual, "the Graphviz picture", |v| picture_is_shown(v, "flow.gv"));
            // The picture has a size, and the way to the source is offered.
            assert!(with_role(&mut visual, Role::Image)
                .iter()
                .all(|(_, w, h)| *w > 40. && *h > 40.));
            assert!(has(&mut visual, "Show the source"));
            visual.update(|window, cx| window.click("small-diagram-toggle", cx));
            visual.run_until_parked();
            assert!(has(&mut visual, "Show the diagram"));
            assert!(!picture_is_shown(&mut visual, "flow.gv"));
            visual.update(|window, cx| window.click("small-diagram-toggle", cx));
            visual.run_until_parked();
            assert!(picture_is_shown(&mut visual, "flow.gv"));

            // An identifier in Arabic, unquoted as DOT allows.
            let entry = entry_of(
                &dir,
                "graph.gv",
                "digraph { rankdir=LR; مرحبا -> world; world -> مرحبا }".as_bytes(),
            );
            let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, false);
            settle(&view, &mut visual);
            wait(&mut visual, "the Graphviz picture", |v| picture_is_shown(v, "graph.gv"));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[gpui_kit::test]
        fn a_diagram_that_cannot_be_drawn_says_why_above_its_source(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            svg_image::clear();
            let dir = temp_dir("broken");
            let entry = entry_of(&dir, "bad.gv", b"digraph { a -> }");
            let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, false);
            settle(&view, &mut visual);
            wait(&mut visual, "the Graphviz failure", |v| {
                with_role(v, Role::Status)
                    .iter()
                    .any(|(l, ..)| l.starts_with("Graphviz couldn't draw it: "))
            });
            assert!(!picture_is_shown(&mut visual, "bad.gv"));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[gpui_kit::test]
        fn a_tall_diagram_scrolls_a_page_for_quick_look(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            svg_image::clear();
            let dir = temp_dir("tall");
            let mut source = String::from("digraph {\n");
            for n in 0..30 {
                source.push_str(&format!("  S{n} -> S{};\n", n + 1));
            }
            source.push('}');
            let entry = entry_of(&dir, "tall.gv", source.as_bytes());
            let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, false);
            settle(&view, &mut visual);
            wait(&mut visual, "the picture", |v| picture_is_shown(v, "tall.gv"));
            // Laid out once the picture is there.
            for _ in 0..3 {
                visual.update(|window, cx| window.render_frame(cx));
            }
            let moved = visual.update(|_, cx| view.update(cx, |v, cx| v.page(true, cx)));
            assert!(moved, "a diagram taller than the pane scrolls");
            // And back up.
            assert!(visual.update(|_, cx| view.update(cx, |v, cx| v.page(false, cx))));
            assert!(!visual.update(|_, cx| view.update(cx, |v, cx| v.page(false, cx))));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[gpui_kit::test]
        fn a_diagram_wider_than_the_pane_shrinks_to_fit_and_keeps_its_shape(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            svg_image::clear();
            let dir = temp_dir("wide");
            let mut source = String::from("digraph {\n  rankdir=LR;\n");
            for n in 0..20 {
                source.push_str(&format!(
                    "  N{n} [label=\"Station number {n}\"];\n  N{n} -> N{};\n",
                    n + 1
                ));
            }
            source.push('}');
            let entry = entry_of(&dir, "wide.gv", source.as_bytes());
            let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, false);
            settle(&view, &mut visual);
            wait(&mut visual, "the picture", |v| picture_is_shown(v, "wide.gv"));
            let drawn = svg_image::peek(&diagram::key_of(diagram::Language::Dot, &source))
                .and_then(|outcome| outcome.as_ref().as_ref().ok().map(|d| (d.width, d.height)))
                .expect("it was drawn");
            assert!(
                drawn.0 > 1200.,
                "the diagram is wider than the 1000 px window: {drawn:?}"
            );
            // The picture is no wider than the window, and keeps the shape it was drawn in.
            let frame = with_role(&mut visual, Role::Image)
                .into_iter()
                .find(|(l, ..)| l.contains("wide.gv"))
                .expect("the picture");
            assert!(frame.1 <= 1000. && frame.1 > 300., "{frame:?}");
            assert!(
                ((frame.1 / frame.2) / (drawn.0 / drawn.1) - 1.).abs() < 0.03,
                "{frame:?} vs {drawn:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[gpui_kit::test]
        fn an_arabic_diagram_file_is_drawn_in_an_arabic_window(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            svg_image::clear();
            let dir = temp_dir("arabic");
            let source = "digraph { rankdir=RL; البداية -> النهاية }";
            let entry = entry_of(&dir, "مخطط.gv", source.as_bytes());
            let (view, mut visual) = open(cx, entry, SmallKind::Graphviz, true);
            settle(&view, &mut visual);
            wait(&mut visual, "the picture", |v| picture_is_shown(v, "مخطط.gv"));
            assert!(svg_image::peek(&diagram::key_of(diagram::Language::Dot, source)).is_some_and(|o| o.is_ok()));
            let _ = std::fs::remove_dir_all(&dir);
            testing::set_rtl(false);
        }
    }

    #[gpui_kit::test]
    fn a_dot_file_that_is_a_word_template_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = temp_dir("template");
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
}
