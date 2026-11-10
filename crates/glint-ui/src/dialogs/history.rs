//! A file's history: what was renamed, moved, deleted, sent to the Recycle Bin or taken out of it, on the drives
//! instant search covers. Ported from src/components/explorer/HistoryDialog.tsx, built the way
//! [`super::drive_health`] is (see [`super`]).
//!
//! | TypeScript                                | Here                                                              |
//! |-------------------------------------------|-------------------------------------------------------------------|
//! | zustand store `useHistory` and `load()`   | [`History`], a model entity: query, span, answer, loading, problem |
//! | `indexHistory(query, since, MAX)`         | [`Source::answer`], glint-core's `index_history`                  |
//! | `enableIndex()`                           | [`Source::enable`], glint-core's `index_enable`                   |
//! | `openHistory(query)`                      | [`open`]: the Explorer's folder menu passes the folder; the palette passes none |
//! | `HistoryDialog`                           | [`HistoryDialog`]                                                 |
//! | `describe`, `oldFolder`, `dayOf`          | [`describe`], [`old_folder`], [`day_heading`]                     |
//! | the 250 ms wait after typing              | [`History::set_typed`]                                            |
//!
//! **Stale answers.** The TypeScript numbered its requests and dropped the old ones. Here starting a load drops
//! the one under way (its task is cancelled), so only the newest answer reaches the model.
//!
//! **Where the answer comes from.** glint-core returns `shadui_index`'s own `HistoryAnswer`, which glint-ui can't
//! name (it doesn't depend on that crate). The answer crosses as JSON, which is the shape the TypeScript read, and
//! is read here into this file's [`HistoryAnswer`]. A few serde steps for at most 5,000 events, on core's thread.
//!
//! **Local time.** Days are counted from local midnights (the TypeScript's `setHours`), through Windows' own time
//! zone rules ([`local_day`]). The older days' heading is the date alone in Windows' long form, and a change's row
//! shows the time alone, as Windows writes it ([`time_of_day`]); hovering it gives the full date.
//!
//! **Not yet as the TypeScript has it:**
//! - The span (24 h, 7 d, 30 d) starts at 7 d each time the dialog opens. The TypeScript kept the last choice in
//!   its store for the session.
//! - "Turn on instant search" runs glint-core's installer, which asks for administrator approval (UAC). It runs
//!   on core's thread, never inside a GPUI callback, so no modal loop runs on the UI thread. Its owner window is
//!   the main window, found through the app handle.

use crate::core_bridge::{self, Core};
use crate::explorer::{self, NavOptions};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_core::host::{AppHandle, Manager};
use glint_model::format::{format_count, format_duration, format_full_date};
use glint_model::paths::{base_name, parent_path};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One day in milliseconds.
const DAY: i64 = 86_400_000;
/// At most this many events come from the service; a search narrows them down (the TypeScript's `MAX`).
const MAX: u32 = 5000;
/// How long typing waits before the search goes to the service.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// The folder inside a drive's root that holds the Recycle Bin ("C:\$Recycle.Bin\"), in lower case.
const BIN_DIR: &str = "$recycle.bin\\";

// ---- What the service answers --------------------------------------------------------------------------

/// What was renamed, moved and deleted, newest first (the service's `HistoryAnswer`, read from its JSON).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAnswer {
    pub events: Vec<HistoryEvent>,
    /// More matched than came.
    pub limited: bool,
    /// When the oldest event kept happened, in Unix milliseconds (0: none).
    pub oldest: i64,
    /// The service's time, in milliseconds.
    pub elapsed: f64,
}

/// One change (the service's `HistoryEvent`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEvent {
    /// Unix milliseconds.
    pub time: i64,
    pub kind: Kind,
    pub dir: bool,
    /// Where it was (for something in the Recycle Bin, its place there).
    pub path: String,
    /// Where it went (renamed, moved, recycled, restored).
    #[serde(default)]
    pub to: Option<String>,
    /// For something in the Recycle Bin: where it was before.
    #[serde(default)]
    pub was: Option<String>,
    /// Where it is now, if it's still there.
    #[serde(default)]
    pub now: Option<String>,
    /// A folder deleted with what was in it: how many items went with it. A burst: how many more went from `path`.
    pub items: u32,
    pub burst: bool,
}

/// How a change happened (the TypeScript's `HistoryKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Renamed,
    Moved,
    /// Deleted for good, not to the Recycle Bin.
    Deleted,
    /// Put in the Recycle Bin.
    Recycled,
    /// Taken back out of it.
    Restored,
    /// Deleted from it.
    Emptied,
}

impl Kind {
    /// The badge's word.
    fn label(self) -> SharedString {
        match self {
            Self::Renamed => t!("Renamed"),
            Self::Moved => t!("Moved"),
            Self::Deleted => t!("Deleted"),
            Self::Recycled => t!("Recycled"),
            Self::Restored => t!("Restored"),
            Self::Emptied => t!("Emptied"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Renamed => IconName::PenLine,
            Self::Moved => IconName::FolderInput,
            Self::Deleted | Self::Emptied => IconName::FileX,
            Self::Recycled => IconName::Trash,
            Self::Restored => IconName::Undo2,
        }
    }

    /// The badge's color: sky for a rename or move, rose for a deletion, amber for the bin, emerald for a restore
    /// (the TypeScript's `KIND` tones), in the theme's light or dark shade.
    fn tone(self, t: &theme::Tokens) -> Hsla {
        let name = match (self, t.dark) {
            (Self::Renamed | Self::Moved, false) => "sky-600",
            (Self::Renamed | Self::Moved, true) => "sky-400",
            (Self::Deleted | Self::Emptied, _) => "rose-500",
            (Self::Recycled, false) => "amber-700",
            (Self::Recycled, true) => "amber-400",
            (Self::Restored, false) => "emerald-600",
            (Self::Restored, true) => "emerald-400",
        };
        theme::tailwind(name).unwrap_or(t.muted_foreground)
    }
}

/// The span the dialog looks back over (the TypeScript's `SPANS`). The default is a week.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Day,
    Week,
    Month,
}

impl Span {
    /// The three buttons, in order.
    pub const ALL: [Span; 3] = [Span::Day, Span::Week, Span::Month];

    /// How far back, in milliseconds.
    pub fn millis(self) -> i64 {
        match self {
            Self::Day => DAY,
            Self::Week => 7 * DAY,
            Self::Month => 30 * DAY,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    fn title(self) -> SharedString {
        match self {
            Self::Day => t!("The last 24 hours"),
            Self::Week => t!("The last 7 days"),
            Self::Month => t!("The last 30 days: all that's kept"),
        }
    }
}

/// Which kinds of change the list shows (the TypeScript's `FILTERS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Everything,
    Deleted,
    MovedAndRenamed,
    RecycleBin,
}

impl Filter {
    /// The four chips, in order.
    pub const ALL: [Filter; 4] = [
        Filter::Everything,
        Filter::Deleted,
        Filter::MovedAndRenamed,
        Filter::RecycleBin,
    ];

    /// Whether the filter shows a change of `kind`.
    pub fn keeps(self, kind: Kind) -> bool {
        match self {
            Self::Everything => true,
            Self::Deleted => matches!(kind, Kind::Deleted | Kind::Emptied),
            Self::MovedAndRenamed => matches!(kind, Kind::Moved | Kind::Renamed),
            Self::RecycleBin => matches!(kind, Kind::Recycled | Kind::Restored | Kind::Emptied),
        }
    }

    fn label(self) -> SharedString {
        match self {
            Self::Everything => t!("Everything"),
            Self::Deleted => t!("Deleted"),
            Self::MovedAndRenamed => t!("Moved and renamed"),
            Self::RecycleBin => t!("Recycle Bin"),
        }
    }
}

/// Why the service gave no answer. "tooOld" and "off" are the two the TypeScript told apart; anything else is shown
/// as it came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The instant search service running is from before this history (update it).
    TooOld,
    /// Instant search isn't on (turn it on).
    Off,
    /// Anything else, with the reason.
    Other(SharedString),
}

impl Problem {
    fn from_reason(reason: String) -> Self {
        if reason == "tooOld" {
            Self::TooOld
        } else if reason == "off" {
            Self::Off
        } else {
            Self::Other(reason.into())
        }
    }
}

// ---- Where the answers come from ----------------------------------------------------------------------

type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Where the dialog gets its answers: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Source {
    /// Changes whose path has `query` in it, since `since` (Unix ms), at most `max` (`index_history`).
    pub answer: Arc<dyn Fn(String, i64, u32) -> Fut<HistoryAnswer> + Send + Sync>,
    /// Installs and starts instant search, after the administrator prompt (`index_enable`).
    pub enable: Arc<dyn Fn() -> Fut<()> + Send + Sync>,
}

/// glint-core's history and instant search, as a [`Source`]. Needs the app handle, which the enable call
/// uses to find the main window.
pub fn core_source(app: AppHandle) -> Source {
    let enable_app = app.clone();
    Source {
        answer: Arc::new(|query: String, since: i64, max: u32| {
            Box::pin(async move {
                let answer = glint_core::index::index_history(query, since as f64, max).await?;
                // The same JSON shape the TypeScript read; see the module notes for why it goes through it.
                let value = serde_json::to_value(&answer).map_err(|error| error.to_string())?;
                serde_json::from_value::<HistoryAnswer>(value).map_err(|error| error.to_string())
            })
        }),
        enable: Arc::new(move || {
            let app = enable_app.clone();
            Box::pin(async move {
                let window = app
                    .get_webview_window("main")
                    .ok_or_else(|| "the main window isn't registered".to_string())?;
                glint_core::index::index_enable(window).await.map(|_| ())
            })
        }),
    }
}

// ---- The model ----------------------------------------------------------------------------------------

/// The history the dialog shows: what was asked for, and the last answer. One lives in the dialog's view.
pub struct History {
    /// The part of a name, or a folder's path, the answer is narrowed to ("" for everything).
    query: String,
    span: Span,
    /// Shared, so the dialog's day list can keep it without copying up to 5,000 events.
    answer: Option<Rc<HistoryAnswer>>,
    /// Counts the answers that have come back (good or not). The dialog rebuilds its list when this changes.
    generation: u64,
    problem: Option<Problem>,
    loading: bool,
    source: Source,
    /// The load under way. Starting another drops it, so a late answer never lands over a newer one.
    load_task: Option<Task<()>>,
    /// The wait after typing, until the search goes to the service.
    typing_task: Option<Task<()>>,
}

impl History {
    /// A history that asks `source` for `query` over the default span once [`History::load`] runs.
    pub fn new(source: Source, query: String) -> Self {
        Self {
            query,
            span: Span::Week,
            answer: None,
            generation: 0,
            problem: None,
            loading: false,
            source,
            load_task: None,
            typing_task: None,
        }
    }

    /// The part of a name the answer is narrowed to.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// How far back the answer goes.
    pub fn span(&self) -> Span {
        self.span
    }

    /// The last answer, if the service gave one.
    pub fn answer(&self) -> Option<&HistoryAnswer> {
        self.answer.as_deref()
    }

    /// The last answer, shared (a clone of the pointer, not of the events).
    pub fn answer_shared(&self) -> Option<Rc<HistoryAnswer>> {
        self.answer.clone()
    }

    /// How many answers have come back so far (see the field). Equal numbers mean the same answer.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Why there's no answer, if there isn't one.
    pub fn problem(&self) -> Option<&Problem> {
        self.problem.as_ref()
    }

    /// Whether a load is under way.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// Asks the service for the history now. The old answer stays on screen, dimmed, until the new one arrives.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.problem = None;
        cx.notify();
        let since = now_ms() - self.span.millis();
        let read = (self.source.answer)(self.query.clone(), since, MAX);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.generation = this.generation.wrapping_add(1);
                match result {
                    Ok(answer) => this.answer = Some(Rc::new(answer)),
                    Err(reason) => {
                        this.answer = None;
                        this.problem = Some(Problem::from_reason(reason));
                    }
                }
                cx.notify();
            });
        }));
    }

    /// The search box changed to `text`. The search goes to the service once typing has paused for a moment.
    pub fn set_typed(&mut self, text: String, cx: &mut Context<Self>) {
        if text == self.query {
            self.typing_task = None;
            return;
        }
        self.typing_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                this.query = text;
                this.load(cx);
            });
        }));
    }

    /// Looks back over another span, and asks again.
    pub fn set_span(&mut self, span: Span, cx: &mut Context<Self>) {
        self.span = span;
        self.load(cx);
    }

    /// Turns instant search on (or updates it), then asks for the history. The task says whether that worked, so
    /// the dialog can tell the person when it didn't.
    pub fn enable(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let turn_on = (self.source.enable)();
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(turn_on).await.and_then(|inner| inner);
            if result.is_ok() {
                let _ = this.update(cx, |this, cx| this.load(cx));
            }
            result
        })
    }
}

/// The time in Unix milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

// ---- What the rows say ----------------------------------------------------------------------------------

/// A count in the user's own grouping (the TypeScript's `formatCount`).
fn count_text(n: u32) -> String {
    format_count(usize::try_from(n).unwrap_or(usize::MAX))
}

/// " and 3 items in it", for a folder deleted with things in it (the TypeScript's `inIt`).
fn items_in_text(items: u32) -> String {
    if items == 1 {
        t!("and {n} item in it", n = count_text(items)).to_string()
    } else {
        t!("and {n} items in it", n = count_text(items)).to_string()
    }
}

/// "3 more items", for a burst: what else went from one folder (the TypeScript's `more`).
fn more_text(items: u32) -> String {
    if items == 1 {
        t!("{n} more item", n = count_text(items)).to_string()
    } else {
        t!("{n} more items", n = count_text(items)).to_string()
    }
}

/// The name and the place a change is shown with: "report.docx → final.docx" and the folder, or the file and
/// "from → to" (the TypeScript's `describe`). Text is translated; the paths are as Windows gives them.
pub fn describe(e: &HistoryEvent) -> (String, String) {
    let in_it = if e.dir && e.items > 0 {
        format!(" {}", items_in_text(e.items))
    } else {
        String::new()
    };
    match e.kind {
        Kind::Renamed => (
            format!("{} → {}", base_name(&e.path), base_name(e.to.as_deref().unwrap_or(""))),
            parent_path(&e.path),
        ),
        Kind::Moved => (
            base_name(&e.path).to_string(),
            format!(
                "{} → {}",
                parent_path(&e.path),
                parent_path(e.to.as_deref().unwrap_or(""))
            ),
        ),
        Kind::Recycled => (
            format!("{}{in_it}", base_name(&e.path)),
            t!("To the Recycle Bin from {folder}", folder = parent_path(&e.path)).to_string(),
        ),
        Kind::Restored => {
            let at = e.to.as_deref().unwrap_or(e.path.as_str());
            (
                base_name(at).to_string(),
                t!("Back in {folder}", folder = parent_path(at)).to_string(),
            )
        }
        Kind::Emptied if e.burst => (more_text(e.items), t!("Deleted from the Recycle Bin").to_string()),
        Kind::Emptied => match e.was.as_deref() {
            Some(was) => (
                format!("{}{in_it}", base_name(was)),
                t!(
                    "Deleted from the Recycle Bin; it was in {folder}",
                    folder = parent_path(was)
                )
                .to_string(),
            ),
            None => (
                format!("{}{in_it}", base_name(&e.path)),
                t!("Deleted from the Recycle Bin").to_string(),
            ),
        },
        Kind::Deleted if e.burst => (
            more_text(e.items),
            t!("Deleted in {folder}", folder = e.path.clone()).to_string(),
        ),
        Kind::Deleted => (format!("{}{in_it}", base_name(&e.path)), parent_path(&e.path)),
    }
}

/// The folder to open for something no longer there: the one it was in (for a burst, the folder it went from).
/// None for an emptied item whose old place isn't known (the TypeScript's `oldFolder`).
pub fn old_folder(e: &HistoryEvent) -> Option<String> {
    if e.burst {
        return Some(e.path.clone());
    }
    match e.kind {
        Kind::Emptied => e.was.as_deref().map(parent_path),
        _ => Some(parent_path(&e.path)),
    }
}

/// Whether `path` is inside a drive's Recycle Bin ("C:\$Recycle.Bin\…", any case): nowhere to show, since it's
/// restored from there.
fn in_recycle_bin(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\'
        && path
            .get(3..)
            .and_then(|rest| rest.get(..BIN_DIR.len()))
            .is_some_and(|folder| folder.eq_ignore_ascii_case(BIN_DIR))
}

/// A heading for the day `ms` falls on, judged against `now`: Today, Yesterday, or the date alone in Windows'
/// long form (`dayOf`). Days are local.
fn day_heading(ms: i64, now: i64) -> SharedString {
    match local_day(now) - local_day(ms) {
        0 => t!("Today"),
        1 => t!("Yesterday"),
        _ => long_date(ms).into(),
    }
}

// ---- Local time ------------------------------------------------------------------------------------------

/// Unix milliseconds as a local calendar time, from Windows' own time zone rules. `None` when Windows can't
/// convert it.
fn local_system_time(ms: i64) -> Option<windows::Win32::Foundation::SYSTEMTIME> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns from 1601.
    let ticks = u64::try_from(ms.checked_add(11_644_473_600_000)?)
        .ok()?
        .checked_mul(10_000)?;
    let file_time = FILETIME {
        dwLowDateTime: u32::try_from(ticks & 0xFFFF_FFFF).ok()?,
        dwHighDateTime: u32::try_from(ticks >> 32).ok()?,
    };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: every pointer is to a live local; the calls only read the first and write the second.
    unsafe {
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

/// The number of days from 1970-01-01 to a civil date (Howard Hinnant's days-from-civil).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Which local day `ms` falls on, counted from 1970-01-01 (the TypeScript's `setHours(0, 0, 0, 0)` divided by a
/// day). A time Windows can't convert counts by UTC.
fn local_day(ms: i64) -> i64 {
    match local_system_time(ms) {
        Some(t) => days_from_civil(i64::from(t.wYear), i64::from(t.wMonth), i64::from(t.wDay)),
        None => ms.div_euclid(DAY),
    }
}

/// Windows' own picture of a date (`long`) or of a time without seconds, for a locale (`None` is the user's).
fn windows_text(st: &windows::Win32::Foundation::SYSTEMTIME, locale: Option<&str>, long: bool) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, DATE_LONGDATE, TIME_NOSECONDS};
    let name: Option<Vec<u16>> = locale.map(|l| l.encode_utf16().chain(std::iter::once(0)).collect());
    let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let ask = |buf: Option<&mut [u16]>| -> i32 {
        // SAFETY: `st` lives across the call; the locale name is NULL or a NUL-terminated buffer kept alive by
        // `locale`'s copy above; the picture and calendar are NULL (Windows' own); `buf` is None or a writable
        // slice whose length Windows is told.
        unsafe {
            if long {
                GetDateFormatEx(
                    name,
                    DATE_LONGDATE,
                    Some(st as *const _),
                    PCWSTR::null(),
                    buf,
                    PCWSTR::null(),
                )
            } else {
                GetTimeFormatEx(name, TIME_NOSECONDS, Some(st as *const _), PCWSTR::null(), buf)
            }
        }
    };
    let needed = usize::try_from(ask(None)).ok().filter(|&n| n > 0)?;
    let mut buf = vec![0u16; needed];
    let written = usize::try_from(ask(Some(buf.as_mut_slice()))).ok()?;
    // The count includes the terminating NUL, which is not part of the text.
    String::from_utf16(buf.get(..written.min(needed).saturating_sub(1))?).ok()
}

/// The date alone in Windows' long form ("Saturday, October 3, 2026"), for an older day's heading.
fn long_date(ms: i64) -> String {
    local_system_time(ms)
        .and_then(|st| windows_text(&st, None, true))
        .unwrap_or_default()
}

/// The time alone as Windows writes it ("4:15 PM"), for a change's row.
fn time_of_day(ms: i64) -> String {
    local_system_time(ms)
        .and_then(|st| windows_text(&st, None, false))
        .unwrap_or_default()
}

/// One line of the day list: a day's heading, or one change. The list draws each one on its own, so a busy span
/// costs what is on screen, not what the service sent.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    /// A day's heading: "Today", "Yesterday", or the date.
    Day(SharedString),
    /// One change, as its index into the answer's `events`.
    Change(usize),
}

/// The headings and changes the filter keeps, in the service's order (newest first): each day's heading, then its
/// changes. Built once per answer and filter (see [`Shown`]), never on each frame.
fn flatten(events: &[HistoryEvent], filter: Filter, now: i64) -> Vec<Item> {
    let mut items = Vec::new();
    let mut day: Option<i64> = None;
    for (index, event) in events.iter().enumerate() {
        if !filter.keeps(event.kind) {
            continue;
        }
        let this_day = local_day(event.time);
        if day != Some(this_day) {
            day = Some(this_day);
            items.push(Item::Day(day_heading(event.time, now)));
        }
        items.push(Item::Change(index));
    }
    items
}

// ---- The dialog ---------------------------------------------------------------------------------------

/// The day list as built for one answer and one filter. Rebuilt only when either changes, so typing in the search
/// box, the spinner and the scroll cost nothing here.
#[derive(Default)]
struct Shown {
    /// The answer's generation and the filter the list was built from (None before the first build).
    key: Option<(u64, Filter)>,
    /// The answer the change items index into.
    answer: Option<Rc<HistoryAnswer>>,
    /// The headings and changes, in order.
    items: Rc<Vec<Item>>,
    /// How many changes are in `items` (the summary counts them).
    count: usize,
}

/// The history in a dialog: the search and filters, the day groups, the states (loading, the service off or
/// too old, another failure, nothing found), and the summary.
pub struct HistoryDialog {
    history: Entity<History>,
    input: Entity<InputState>,
    filter: Filter,
    /// The day list's scroll state. Virtualized: `list` draws only the rows on screen.
    list: ListState,
    shown: Shown,
    _redraw_with_model: Subscription,
    _typing: Subscription,
}

impl HistoryDialog {
    /// Shows `history` in a dialog with its query in the search box, and puts the cursor there.
    pub fn new(history: Entity<History>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = history.read(cx).query().to_string();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("Part of a name or a path…"))
                .default_value(query)
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let redraw = cx.observe(&history, |_, _, cx| cx.notify());
        let typing = cx.subscribe_in(&input, window, Self::on_input);
        Self {
            history,
            input,
            filter: Filter::Everything,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            shown: Shown::default(),
            _redraw_with_model: redraw,
            _typing: typing,
        }
    }

    fn on_input(&mut self, input: &Entity<InputState>, event: &InputEvent, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(event, InputEvent::Change) {
            let text = input.read(cx).value().to_string();
            self.history.update(cx, |history, cx| history.set_typed(text, cx));
        }
    }

    /// Turns instant search on, then reads the history. A failure is told in a notification.
    fn enable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let turned_on = self.history.update(cx, |history, cx| history.enable(cx));
        cx.spawn_in(window, async move |_, cx| {
            if let Err(reason) = turned_on.await {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(t!("Couldn't turn on instant search: {reason}", reason = reason)),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// The span buttons and the refresh button, then the search box at the end of the row.
    fn toolbar(&self, span: Span, loading: bool, t: &theme::Tokens, cx: &Context<Self>) -> impl IntoElement {
        let spans = row_start()
            .gap(space(0.5))
            .p(space(0.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.4))
            .children(Span::ALL.into_iter().enumerate().map(|(i, option)| {
                Button::new(("history-span", i))
                    .ghost()
                    .xsmall()
                    .selected(option == span)
                    .label(option.label())
                    .tooltip(option.title())
                    .accessibility_label(option.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.history.update(cx, |history, cx| history.set_span(option, cx));
                    }))
            }));
        let refresh = Button::new("history-refresh")
            .ghost()
            .xsmall()
            .icon(IconName::RefreshCw)
            .loading(loading)
            .tooltip(t!("Refresh"))
            .accessibility_label(t!("Refresh"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.history.update(cx, |history, cx| history.load(cx));
            }));
        let search = Input::new(&self.input)
            .small()
            .w(space(64.))
            .aria_label(t!("Part of a name or a path…"));
        row_start()
            .w_full()
            .flex_wrap()
            .justify_between()
            .items_center()
            .gap(space(2.))
            .child(row_start().gap(space(2.)).child(spans).child(refresh))
            .child(search)
    }

    /// The filter chips, in a band under the toolbar.
    fn filters(&self, t: &theme::Tokens, cx: &Context<Self>) -> impl IntoElement {
        row_start()
            .flex_wrap()
            .items_center()
            .gap(space(1.))
            .px(space(4.))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.2))
            .text_size(theme::size::XS)
            .children(Filter::ALL.into_iter().enumerate().map(|(i, option)| {
                Button::new(("history-filter", i))
                    .ghost()
                    .small()
                    .selected(option == self.filter)
                    .label(option.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = option;
                        cx.notify();
                    }))
            }))
    }

    /// What the service said when it gave no history: the off or too-old notice with its button, or the reason.
    fn problem_view(&self, problem: &Problem, t: &theme::Tokens, cx: &Context<Self>) -> AnyElement {
        match problem {
            Problem::Off | Problem::TooOld => {
                let (title, state, button) = if *problem == Problem::Off {
                    (
                        t!("This comes from instant search, which isn't on"),
                        t!("on"),
                        t!("Turn on instant search"),
                    )
                } else {
                    (
                        t!("The instant search service running is from before this history"),
                        t!("updated"),
                        t!("Update instant search"),
                    )
                };
                col()
                    .id("history-problem")
                    .role(Role::Alert)
                    .aria_label(title.clone())
                    .test_support()
                    .items_center()
                    .gap(space(3.))
                    .py(space(8.))
                    .text_center()
                    .child(icon(IconName::Clock).size_7().text_color(t.muted_foreground))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .child(
                        div()
                            .max_w(space(144.))
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!(
                                "Its service reads each drive's change journal, which Windows keeps anyway: once it's {state}, what was renamed, moved and deleted shows here, from as far back as the journal goes.",
                                state = state
                            )),
                    )
                    .child(
                        Button::new("history-enable")
                            .small()
                            .mt(space(1.))
                            .icon(IconName::Sparkles)
                            .label(button.clone())
                            .accessibility_label(button)
                            .tooltip(t!("Windows asks for administrator approval"))
                            .on_click(cx.listener(|this, _, window, cx| this.enable(window, cx))),
                    )
                    .into_any_element()
            }
            Problem::Other(reason) => col()
                .id("history-error")
                .role(Role::Alert)
                .aria_label(t!("Couldn't read the history"))
                .test_support()
                .items_center()
                .gap(space(2.))
                .py(space(8.))
                .text_center()
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(t!("Couldn't read the history")),
                )
                .child(
                    div()
                        .max_w(space(144.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(reason.clone()),
                )
                .into_any_element(),
        }
    }

    /// One change: its badge, its name and place, its time, and a button to show it where it is now or to open
    /// the folder it was in. `index` is the change's place in the answer. `first` and `last` say where it sits in
    /// its day's card: the card's top and bottom edges are drawn by its first and last rows.
    fn event_row(index: usize, e: &HistoryEvent, (first, last): (bool, bool), t: &theme::Tokens) -> impl IntoElement {
        let kind = e.kind;
        let tone = kind.tone(t);
        let (name, place) = describe(e);
        // Still in the Recycle Bin is no place to show: it's restored from there.
        let still_there = e.now.as_deref().filter(|path| !in_recycle_bin(path));
        let place_text = match still_there {
            Some(path) if e.to.as_deref() != Some(path) => {
                format!("{place} · {}", t!("now in {folder}", folder = parent_path(path)))
            }
            _ => place.clone(),
        };
        // What the button does: show it where it is now, or open the folder it was in.
        let jump: Option<(String, Option<String>, SharedString)> = match (still_there, old_folder(e)) {
            (Some(path), _) => Some((
                parent_path(path),
                Some(path.to_string()),
                t!("Show it where it is now: {now}", now = path),
            )),
            (None, Some(folder)) => {
                let title = t!("Open the folder it was in: {folder}", folder = folder.as_str());
                Some((folder, None, title))
            }
            (None, None) => None,
        };
        let icon_name = if still_there.is_some() {
            IconName::FolderInput
        } else {
            IconName::FolderOpen
        };
        let action =
            row_start()
                .w(space(14.))
                .flex_none()
                .justify_end()
                .children(jump.map(|(folder, select, title)| {
                    Button::new(("history-open", index))
                        .ghost()
                        .xsmall()
                        .icon(icon_name)
                        .tooltip(title.clone())
                        .accessibility_label(title)
                        .on_click(move |_, window, cx| {
                            // The dialog closes first, then the folder opens (with the item selected, when it's there).
                            window.close_dialog(cx);
                            let mut options = NavOptions::default();
                            if let Some(path) = select.clone() {
                                options = options.selecting(path);
                            }
                            explorer::explorer(cx).update(cx, |explorer, cx| {
                                explorer.navigate(folder.clone(), options, cx);
                            });
                        })
                }));

        row_start()
            .id(("history-row", index))
            .role(Role::Group)
            .aria_label(format!("{}: {name}, {place}", kind.label()))
            .test_support()
            .items_center()
            .gap(space(3.))
            .px(space(3.))
            .py(space(2.))
            .text_size(theme::size::XS)
            // The day's card: its sides and the line under each row, its top on the first row, its corners at both ends.
            .border_x_1()
            .border_b_1()
            .border_color(t.border)
            .bg(t.card)
            .when(first, |this| this.border_t_1().rounded_t(theme::size::RADIUS_LG))
            .when(last, |this| this.rounded_b(theme::size::RADIUS_LG))
            .hover(|style| style.bg(t.accent))
            .child(
                row_start()
                    .w(space(22.))
                    .flex_none()
                    .items_center()
                    .gap(space(1.))
                    .rounded(theme::size::RADIUS_SM)
                    .px(space(1.5))
                    .py(space(0.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tone)
                    .bg(tone.opacity(0.15))
                    .child(icon(kind.icon()).size_3().flex_none())
                    .child(kind.label()),
            )
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .text_start()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .truncate()
                            .child(isolate(name)),
                    )
                    .child(
                        div()
                            .text_color(t.muted_foreground)
                            .truncate()
                            .child(isolate(place_text)),
                    ),
            )
            .child(
                div()
                    .id(("history-time", index))
                    .flex_none()
                    .text_color(t.muted_foreground)
                    .tooltip({
                        let full: SharedString = format_full_date(e.time).into();
                        move |window, cx| crate::kit::tooltip(full.clone(), window, cx)
                    })
                    .child(time_of_day(e.time)),
            )
            .child(action)
    }

    /// The day list: `list` draws the headings and changes on screen only, and asks for each one as it scrolls into
    /// view. Each row reads its change from the cached items and the shared answer; nothing is regrouped here.
    fn day_list(&self, loading: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = list(
            self.list.clone(),
            cx.processor(|this: &mut Self, ix: usize, _: &mut Window, cx: &mut Context<Self>| {
                let t = theme::tokens(cx).clone();
                let items = this.shown.items.clone();
                let Some(item) = items.get(ix) else {
                    return div().into_any_element();
                };
                match item {
                    Item::Day(heading) => div()
                        .px(space(1.))
                        .pt(space(if ix == 0 { 0. } else { 4. }))
                        .pb(space(1.5))
                        .text_size(theme::size::XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.muted_foreground)
                        .child(heading.clone())
                        .into_any_element(),
                    Item::Change(at) => {
                        let Some(event) = this.shown.answer.as_ref().and_then(|answer| answer.events.get(*at)) else {
                            return div().into_any_element();
                        };
                        // The card's top is drawn by its first row (the one after a heading), its bottom by its last.
                        let first = !matches!(
                            ix.checked_sub(1).and_then(|prev| items.get(prev)),
                            Some(Item::Change(_))
                        );
                        let last = !matches!(items.get(ix + 1), Some(Item::Change(_)));
                        div()
                            .px(space(1.))
                            .child(Self::event_row(*at, event, (first, last), &t))
                            .into_any_element()
                    }
                }
            }),
        )
        .flex_1()
        .min_h_0()
        .w_full();
        // A flex column, so the list (a flex child that fills it) gets the body's height to scroll within.
        col()
            .id("history-days")
            .flex_1()
            .min_h_0()
            .w_full()
            .when(loading, |this| this.opacity(0.6))
            .child(rows)
    }

    /// The line under the list: how many changes, how far back the service kept them, and how long it took.
    fn summary(answer: &HistoryAnswer, count: usize) -> SharedString {
        let mut line = format!(
            "{} {}",
            format_count(count),
            if count == 1 { t!("change") } else { t!("changes") }
        );
        if answer.limited {
            line.push(' ');
            line.push_str(&t!(
                "(the newest {max}; a search finds older ones)",
                max = count_text(MAX)
            ));
        }
        if answer.oldest != 0 {
            line.push_str(&format!(
                " · {}",
                t!("kept since {date}", date = format_full_date(answer.oldest))
            ));
        }
        line.push_str(&format!(" · {}", format_duration(answer.elapsed)));
        line.into()
    }
}

impl Render for HistoryDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let now = now_ms();
        let history = self.history.read(cx);
        let span = history.span();
        let loading = history.loading();
        let query = history.query().trim().to_string();
        let problem = history.problem().cloned();
        let answer = history.answer_shared();
        // The day list is rebuilt only when the answer or the filter has changed (see [`Shown`]): not on each render,
        // and not while the search box is typed in.
        let key = (history.generation(), self.filter);
        if self.shown.key != Some(key) {
            let items = answer
                .as_deref()
                .map(|answer| flatten(&answer.events, self.filter, now))
                .unwrap_or_default();
            let count = items.iter().filter(|item| matches!(item, Item::Change(_))).count();
            // A new list starts at the top; a redraw for any other reason keeps the place (the reset is only here).
            self.list.reset(items.len());
            self.shown = Shown {
                key: Some(key),
                answer: answer.clone(),
                items: Rc::new(items),
                count,
            };
        }
        let count = self.shown.count;

        let body = if let Some(problem) = &problem {
            self.problem_view(problem, &t, cx)
        } else if answer.is_some() {
            if count == 0 {
                let head = if query.is_empty() {
                    t!("Nothing renamed, moved or deleted")
                } else {
                    t!("Nothing with “{query}” in it", query = query.clone())
                };
                let line: SharedString = format!("{head} {}", t!("in this time")).into();
                col()
                    .id("history-empty")
                    .role(Role::Status)
                    .aria_label(line.clone())
                    .test_support()
                    .items_center()
                    .gap(space(2.))
                    .py(space(8.))
                    .text_center()
                    .text_color(t.muted_foreground)
                    .child(icon(IconName::Clock).size_7())
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(t.foreground).child(line))
                    .child(
                        div()
                            .max_w(space(144.))
                            .text_size(theme::size::XS)
                            .child(t!("What programs do with their own files (AppData, caches, temporary and Windows' folders) isn't kept here.")),
                    )
                    .into_any_element()
            } else {
                self.day_list(loading, cx).into_any_element()
            }
        } else if loading {
            row_start()
                .id("history-loading")
                .role(Role::Status)
                .aria_label(t!("Loading"))
                .test_support()
                .py(space(12.))
                .justify_center()
                .child(Spinner::new().color(t.primary))
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let summary: Option<SharedString> = answer
            .filter(|_| problem.is_none())
            .map(|answer| Self::summary(&answer, count));

        col()
            .id("history")
            .role(Role::Group)
            .aria_label(t!("Moved and deleted"))
            .test_support()
            .text_size(theme::size::SM)
            // The dialog layer sits outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .gap(space(3.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!(
                        "What was renamed, moved and deleted on the drives instant search covers"
                    )),
            )
            .child(self.toolbar(span, loading, &t, cx))
            .child(self.filters(&t, cx))
            .child(
                // A fixed height, so the day list has a height to fill and scroll within (the TypeScript's min 20 rem,
                // grown to fill the dialog; the dialog itself has no height of its own here).
                col()
                    .id("history-body")
                    .h(space(96.))
                    .justify_center()
                    .p(space(1.))
                    .child(body),
            )
            .child(
                row_start()
                    .justify_between()
                    .items_center()
                    .gap(space(3.))
                    .pt(space(2.5))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(div().flex_1().min_w_0().truncate().child(summary.unwrap_or_default()))
                    .child(
                        Button::new("history-close")
                            .outline()
                            .small()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

/// Opens the dialog in `window`, searching for `query` (part of a name, or a folder's path) when given, and reads
/// the history at once. Reads through glint-core.
pub fn open(window: &mut Window, cx: &mut App, query: &str) {
    let source = core_source(cx.global::<Core>().app.clone());
    open_with(window, cx, source, query);
}

/// [`open`] with a source of its own (tests pass one with fake answers). Returns the dialog's view, so a test can
/// look at what it built.
pub fn open_with(window: &mut Window, cx: &mut App, source: Source, query: &str) -> Entity<HistoryDialog> {
    let history = cx.new(|_| History::new(source, query.to_string()));
    history.update(cx, |history, cx| history.load(cx));
    let view = cx.new(|cx| HistoryDialog::new(history.clone(), window, cx));
    super::show(window, cx, t!("Moved and deleted"), px(880.), view.clone());
    view
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A source whose answer the test can change, which records what it was asked and counts enables.
    #[derive(Clone)]
    struct Fake {
        answer: Arc<Mutex<Result<HistoryAnswer, String>>>,
        asked: Arc<Mutex<Vec<(String, i64, u32)>>>,
        enables: Arc<AtomicUsize>,
    }

    impl Fake {
        fn new(answer: Result<HistoryAnswer, String>) -> Self {
            Self {
                answer: Arc::new(Mutex::new(answer)),
                asked: Arc::new(Mutex::new(Vec::new())),
                enables: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn set_answer(&self, answer: Result<HistoryAnswer, String>) {
            *self.answer.lock().unwrap() = answer;
        }

        fn asked(&self) -> Vec<(String, i64, u32)> {
            self.asked.lock().unwrap().clone()
        }

        fn source(&self) -> Source {
            let (answer, asked, enables) = (self.answer.clone(), self.asked.clone(), self.enables.clone());
            Source {
                answer: Arc::new(move |query: String, since: i64, max: u32| {
                    asked.lock().unwrap().push((query, since, max));
                    let answer = answer.lock().unwrap().clone();
                    Box::pin(async move { answer })
                }),
                enable: Arc::new(move || {
                    enables.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                }),
            }
        }
    }

    fn event(kind: Kind, path: &str) -> HistoryEvent {
        HistoryEvent {
            time: now_ms() - 60_000,
            kind,
            dir: false,
            path: path.into(),
            to: None,
            was: None,
            now: None,
            items: 0,
            burst: false,
        }
    }

    fn answer(events: Vec<HistoryEvent>) -> HistoryAnswer {
        HistoryAnswer {
            events,
            limited: false,
            oldest: 0,
            elapsed: 3.0,
        }
    }

    /// Completes a frame, then asks the accessibility tree. Queries read "the last completed frame".
    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    /// Lets the model's work (on core's runtime, another thread) come back to the foreground until `done`.
    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the history never arrived");
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// An app with the skeleton's globals, and a window with the dialog layer. (The explorer's global isn't made
    /// here: nothing in these tests clicks through to a folder.)
    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    #[test]
    fn describe_reads_as_the_typescript_did() {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let renamed = HistoryEvent {
            to: Some("C:\\docs\\final.docx".into()),
            ..event(Kind::Renamed, "C:\\docs\\report.docx")
        };
        assert_eq!(
            describe(&renamed),
            ("report.docx → final.docx".to_string(), "C:\\docs".to_string())
        );
        let moved = HistoryEvent {
            to: Some("D:\\backup\\a.txt".into()),
            ..event(Kind::Moved, "C:\\docs\\a.txt")
        };
        assert_eq!(
            describe(&moved),
            ("a.txt".to_string(), "C:\\docs → D:\\backup".to_string())
        );
        let folder = HistoryEvent {
            dir: true,
            items: 3,
            ..event(Kind::Recycled, "C:\\old")
        };
        assert_eq!(
            describe(&folder),
            (
                "old and 3 items in it".to_string(),
                "To the Recycle Bin from C:\\".to_string()
            )
        );
        let restored = HistoryEvent {
            to: Some("C:\\docs\\a.txt".into()),
            ..event(Kind::Restored, "C:\\$Recycle.Bin\\a.txt")
        };
        assert_eq!(
            describe(&restored),
            ("a.txt".to_string(), "Back in C:\\docs".to_string())
        );
        let emptied = HistoryEvent {
            was: Some("C:\\docs\\gone.txt".into()),
            ..event(Kind::Emptied, "C:\\$Recycle.Bin\\x")
        };
        assert_eq!(
            describe(&emptied),
            (
                "gone.txt".to_string(),
                "Deleted from the Recycle Bin; it was in C:\\docs".to_string()
            )
        );
        let burst = HistoryEvent {
            burst: true,
            items: 1,
            ..event(Kind::Deleted, "C:\\cache")
        };
        assert_eq!(
            describe(&burst),
            ("1 more item".to_string(), "Deleted in C:\\cache".to_string())
        );
        assert_eq!(old_folder(&emptied), Some("C:\\docs".to_string()));
        assert_eq!(old_folder(&event(Kind::Emptied, "C:\\x")), None);
    }

    /// A day list's shape: "D" for a heading, "C" for a change.
    fn shape(items: &[Item]) -> String {
        items
            .iter()
            .map(|item| match item {
                Item::Day(_) => 'D',
                Item::Change(_) => 'C',
            })
            .collect()
    }

    /// Local noon of the day `ms` falls on.
    fn local_noon(ms: i64) -> i64 {
        let t = local_system_time(ms).expect("a time Windows can convert");
        let since_midnight = (i64::from(t.wHour) * 3600 + i64::from(t.wMinute) * 60 + i64::from(t.wSecond)) * 1000
            + i64::from(t.wMilliseconds);
        ms - since_midnight + DAY / 2
    }

    #[test]
    fn local_days_headings_and_times_read_as_windows_writes_them() {
        use windows::Win32::Foundation::SYSTEMTIME;
        let _guard = testing::lock();
        testing::set_rtl(false);
        // Civil days: the epoch, a leap day, and a date before 1970.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28), 2);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        // Local midday today, yesterday and last week: judged by local days, whatever this PC's zone.
        let today = local_noon(now_ms());
        assert_eq!(day_heading(today, today), "Today");
        assert_eq!(day_heading(today - DAY, today), "Yesterday");
        assert_eq!(local_day(today) - local_day(today - 2 * DAY), 2);
        let older = day_heading(today - 5 * DAY, today).to_string();
        assert!(older != "Today" && older != "Yesterday" && older.chars().any(|c| c.is_ascii_digit()));
        assert_eq!(older, long_date(today - 5 * DAY), "the date alone, in the long form");
        // Windows' pictures for a named locale: the time alone, and the long date.
        let st = SYSTEMTIME {
            wYear: 2026,
            wMonth: 10,
            wDay: 3,
            wDayOfWeek: 6,
            wHour: 16,
            wMinute: 15,
            ..SYSTEMTIME::default()
        };
        assert_eq!(windows_text(&st, Some("en-US"), false).as_deref(), Some("4:15 PM"));
        let long = windows_text(&st, Some("en-US"), true).unwrap_or_default();
        assert!(
            long.contains("October") && long.contains("2026") && !long.contains("4:15"),
            "{long}"
        );
    }

    #[test]
    fn days_keep_the_order_and_drop_what_the_filter_hides() {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let day = |n: i64| n * DAY + 1_000;
        let events = vec![
            HistoryEvent {
                time: day(10) + 500,
                ..event(Kind::Deleted, "C:\\a")
            },
            HistoryEvent {
                time: day(10),
                ..event(Kind::Renamed, "C:\\b")
            },
            HistoryEvent {
                time: day(9),
                ..event(Kind::Deleted, "C:\\c")
            },
        ];
        let now = day(10);
        let everything = flatten(&events, Filter::Everything, now);
        assert_eq!(shape(&everything), "DCCDC");
        assert_eq!(everything[0], Item::Day("Today".into()));
        assert_eq!(everything[1], Item::Change(0));
        assert_eq!(everything[2], Item::Change(1));
        assert_eq!(everything[3], Item::Day("Yesterday".into()));
        assert_eq!(everything[4], Item::Change(2));
        assert_eq!(
            shape(&flatten(&events, Filter::Deleted, now)),
            "DCDC",
            "two days still have a deletion"
        );
        let moved = flatten(&events, Filter::MovedAndRenamed, now);
        assert_eq!(shape(&moved), "DC");
        assert_eq!(moved[1], Item::Change(1));
        assert!(flatten(&events, Filter::RecycleBin, now).is_empty());
    }

    #[gpui_kit::test]
    fn the_day_list_is_built_once_per_answer_and_filter(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let renamed = HistoryEvent {
            to: Some("C:\\b2.txt".into()),
            ..event(Kind::Renamed, "C:\\b.txt")
        };
        let fake = Fake::new(Ok(answer(vec![event(Kind::Deleted, "C:\\a.txt"), renamed])));
        let view = visual.update(|window, cx| open_with(window, cx, fake.source(), ""));
        settle(&mut visual, |cx| has(cx, "Deleted: a.txt, C:\\"));
        assert!(has(&mut visual, "Renamed: b.txt → b2.txt, C:\\"));
        let first = view.read_with(&visual, |v, _| Rc::as_ptr(&v.shown.items));
        // Redrawing with the same answer and filter (as typing or the spinner does) reuses the list.
        visual.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        visual.run_until_parked();
        assert!(has(&mut visual, "Deleted: a.txt, C:\\"));
        assert_eq!(view.read_with(&visual, |v, _| Rc::as_ptr(&v.shown.items)), first);
        // A new filter builds a new one: only the deletion is kept.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("history-filter", 1_usize), cx);
        });
        visual.run_until_parked();
        assert!(has(&mut visual, "Deleted: a.txt, C:\\"));
        assert!(!has(&mut visual, "Renamed: b.txt → b2.txt, C:\\"));
        assert_ne!(view.read_with(&visual, |v, _| Rc::as_ptr(&v.shown.items)), first);
    }

    #[gpui_kit::test]
    fn a_long_answer_draws_only_the_rows_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        // The service sends up to 5,000 changes; the dialog must not lay out all of them.
        // All on one day (local noon of today, back 50 minutes at most), so the heading count is fixed.
        let noon = local_noon(now_ms());
        let events: Vec<HistoryEvent> = (0..3000i64)
            .map(|i| HistoryEvent {
                time: noon - i * 1_000,
                ..event(Kind::Deleted, &format!("C:\\dir\\file{i}.txt"))
            })
            .collect();
        let fake = Fake::new(Ok(answer(events)));
        let view = visual.update(|window, cx| open_with(window, cx, fake.source(), ""));
        settle(&mut visual, |cx| has(cx, "Deleted: file0.txt, C:\\dir"));
        assert_eq!(
            view.read_with(&visual, |v, _| v.shown.items.len()),
            3001,
            "one heading and 3,000 changes are in the list"
        );
        let drawn = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find_all_by_role(Role::Group).len()
        });
        assert!(drawn < 100, "{drawn} groups drawn for 3,000 changes");
        assert!(
            !has(&mut visual, "Deleted: file2999.txt, C:\\dir"),
            "the last change is far below the fold"
        );
    }

    #[gpui_kit::test]
    fn the_dialog_asks_the_service_then_lists_what_it_says(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let fake = Fake::new(Ok(answer(vec![
            HistoryEvent {
                to: Some("C:\\docs\\final.txt".into()),
                ..event(Kind::Renamed, "C:\\docs\\report.txt")
            },
            event(Kind::Deleted, "C:\\docs\\old.txt"),
        ])));
        visual.update(|window, cx| open_with(window, cx, fake.source(), "report"));
        // The request is out at once, and the old answer's place is the spinner.
        visual.run_until_parked();
        settle(&mut visual, |cx| has(cx, "Renamed: report.txt → final.txt, C:\\docs"));
        let asked = fake.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, "report");
        assert_eq!(asked[0].2, MAX);
        assert!(
            (now_ms() - 7 * DAY - asked[0].1).abs() < 60_000,
            "the default span is a week"
        );
        assert_eq!(role_of(&mut visual, "Deleted: old.txt, C:\\docs"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Moved and deleted"), Some(Role::Group));
    }

    #[gpui_kit::test]
    fn a_span_button_asks_the_service_again_over_that_span(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let moved = HistoryEvent {
            to: Some("D:\\a.txt".into()),
            ..event(Kind::Moved, "C:\\a.txt")
        };
        let fake = Fake::new(Ok(answer(vec![moved])));
        visual.update(|window, cx| open_with(window, cx, fake.source(), ""));
        settle(&mut visual, |cx| has(cx, "Moved: a.txt, C:\\ → D:\\"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("history-span", 2_usize), cx);
        });
        settle(&mut visual, |_| fake.asked().len() == 2);
        assert!((now_ms() - 30 * DAY - fake.asked()[1].1).abs() < 60_000, "30 days");
    }

    #[gpui_kit::test]
    fn typing_waits_for_a_pause_and_then_searches_for_what_was_typed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let fake = Fake::new(Ok(answer(Vec::new())));
        let source = fake.source();
        let history = visual.update(|_, cx| cx.new(|_| History::new(source, String::new())));
        visual.update(|_, cx| history.update(cx, |history, cx| history.load(cx)));
        settle(&mut visual, |cx| history.read_with(cx, |h, _| !h.loading()));
        visual.update(|_, cx| history.update(cx, |history, cx| history.set_typed("pdf".into(), cx)));
        visual.run_until_parked();
        assert_eq!(
            fake.asked().len(),
            1,
            "nothing is asked while the typing is still going"
        );
        cx.executor().advance_clock(DEBOUNCE + Duration::from_millis(50));
        settle(&mut visual, |_| fake.asked().len() == 2);
        assert_eq!(fake.asked()[1].0, "pdf");
        visual.update(|_, cx| assert_eq!(history.read(cx).query(), "pdf"));
    }

    #[gpui_kit::test]
    fn an_index_that_is_off_offers_to_turn_it_on_and_then_reads_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let fake = Fake::new(Err("off".into()));
        visual.update(|window, cx| open_with(window, cx, fake.source(), ""));
        settle(&mut visual, |cx| {
            has(cx, "This comes from instant search, which isn't on")
        });
        assert_eq!(
            role_of(&mut visual, "This comes from instant search, which isn't on"),
            Some(Role::Alert)
        );
        assert_eq!(role_of(&mut visual, "Turn on instant search"), Some(Role::Button));
        // The button turns it on, and the history is read again: with the service answering now.
        fake.set_answer(Ok(answer(vec![event(Kind::Deleted, "C:\\gone.txt")])));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("history-enable", cx);
        });
        settle(&mut visual, |cx| has(cx, "Deleted: gone.txt, C:\\"));
        assert_eq!(fake.enables.load(Ordering::SeqCst), 1);
        assert_eq!(fake.asked().len(), 2, "read again once it's on");
    }

    #[gpui_kit::test]
    fn an_empty_answer_names_the_query(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let fake = Fake::new(Ok(answer(Vec::new())));
        visual.update(|window, cx| open_with(window, cx, fake.source(), "nothing"));
        let line = "Nothing with “nothing” in it in this time";
        settle(&mut visual, |cx| has(cx, line));
        assert_eq!(role_of(&mut visual, line), Some(Role::Status));
    }

    #[gpui_kit::test]
    fn reveal_is_offered_for_what_is_still_there_and_never_for_the_bin(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut visual = window(cx);
        let moved = HistoryEvent {
            to: Some("D:\\new\\a.txt".into()),
            now: Some("D:\\new\\a.txt".into()),
            ..event(Kind::Moved, "C:\\a.txt")
        };
        let binned = HistoryEvent {
            now: Some("C:\\$Recycle.Bin\\S-1\\$R1.txt".into()),
            ..event(Kind::Recycled, "C:\\b.txt")
        };
        let fake = Fake::new(Ok(answer(vec![moved, binned])));
        visual.update(|window, cx| open_with(window, cx, fake.source(), ""));
        settle(&mut visual, |cx| has(cx, "Show it where it is now: D:\\new\\a.txt"));
        assert_eq!(
            role_of(&mut visual, "Show it where it is now: D:\\new\\a.txt"),
            Some(Role::Button)
        );
        // The copy in the Recycle Bin has nowhere to be shown, so it offers the folder it was in, and never the bin.
        assert_eq!(
            role_of(&mut visual, "Open the folder it was in: C:\\"),
            Some(Role::Button)
        );
        assert!(!has(
            &mut visual,
            "Show it where it is now: C:\\$Recycle.Bin\\S-1\\$R1.txt"
        ));
        assert!(!has(&mut visual, "now in C:\\$Recycle.Bin\\S-1"));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_dialog_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let arabic = HistoryEvent {
            to: Some("C:\\مستندات\\التقرير النهائي.docx".into()),
            ..event(Kind::Renamed, "C:\\مستندات\\تقرير.docx")
        };
        let fake = Fake::new(Ok(answer(vec![arabic.clone()])));
        visual.update(|window, cx| open_with(window, cx, fake.source(), "تقرير"));
        let (name, place) = describe(&arabic);
        let label = format!("{}: {name}, {place}", Kind::Renamed.label());
        settle(&mut visual, |cx| has(cx, &label));
        assert_eq!(role_of(&mut visual, &label), Some(Role::Group));
        // The dialog's own name is the catalog's Arabic.
        assert_eq!(role_of(&mut visual, "ما نُقل وحُذف"), Some(Role::Group));
        testing::set_rtl(false);
    }
}
