//! The search that runs and what it found: App.tsx's `search`, `closeResults` and its state, ripgrep.ts's
//! `RgMatch` and `searchBatched`.
//!
//! | TypeScript                                   | Here                                                         |
//! |----------------------------------------------|--------------------------------------------------------------|
//! | `SearchRun`                                  | [`Run`]                                                       |
//! | `RgMatch` (a `match` line of rg's JSON)      | [`Match`]                                                     |
//! | `search(query, path, limit, everything)`     | [`Search::search`], given a [`Spec`]                          |
//! | `searchBatched`'s `search-event` listener    | [`Search::receive`] and the pump that parses, off the UI thread |
//! | `cancelSearch`                               | [`Search::stop`]                                              |
//! | `closeResults`                               | [`Search::close`]                                             |
//! | `addToHistory`                               | `AppState::add_to_history`, called by [`Search::search`]      |
//! | Replace all's loop over the files            | [`replace_in_files`]                                          |
//!
//! **Offsets.** A hit's range is a pair of UTF-8 byte offsets into the match line's text, on char boundaries,
//! as ripgrep reports them (`Match::hits`).
//!
//! **Order.** Newer searches replace older ones: every run has an id, the events of any other id are dropped,
//! and core itself ends the older search when a newer one starts.

use super::source::{Engine, Request, Source};
use crate::core_bridge;
use glint_model::store::SearchOptions;
use glint_model::text::{self, RgMatch, Segment, Submatch};
use gpui_kit::{AppContext as _, BackgroundExecutor, Context, EventEmitter, SharedString, Subscription, Task};
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use std::time::Instant;

// ---- Matches ------------------------------------------------------------------------------------------

/// One match line (ripgrep's `match` message, as Results.tsx and Preview.tsx read it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    /// The file (`data.path.text`); empty when ripgrep sent its name as bytes.
    pub path: String,
    /// The line's text, with its line end (`data.lines.text`), or None when ripgrep sent none.
    pub text: Option<String>,
    /// The line's number, from 1 (`data.line_number`).
    pub line_number: Option<u64>,
    /// Where the line starts in the file (`data.absolute_offset`).
    pub absolute_offset: Option<u64>,
    /// The hits in the line: UTF-8 byte ranges (`data.submatches`).
    pub hits: Vec<(usize, usize)>,
    /// A code search with a rewrite: what this match becomes (`data.replacement`).
    pub replacement: Option<String>,
    /// A Documents search's match: where it is in the document ("p. 3", "slide 2", "Sales, row 7").
    pub label: Option<String>,
    /// A Meaning match: how many lines its passage spans, from `line_number`.
    pub line_count: Option<u64>,
}

impl Match {
    /// The match as `glint_model::text::segments` reads it.
    fn rg(&self) -> RgMatch {
        RgMatch {
            lines_text: self.text.clone(),
            submatches: self.hits.iter().map(|&(start, end)| Submatch { start, end }).collect(),
        }
    }

    /// The line in plain and hit parts, trimmed for a one-line row (`segments`).
    pub fn segments(&self) -> Vec<Segment> {
        text::segments(&self.rg())
    }

    /// The same match as ripgrep wrote it, for the JSON export (`JSON.stringify(visible, null, 2)`).
    fn as_json(&self) -> serde_json::Value {
        #[derive(Serialize)]
        struct Text<'a> {
            text: &'a str,
        }
        #[derive(Serialize)]
        struct Hit<'a> {
            r#match: Text<'a>,
            start: usize,
            end: usize,
        }
        #[derive(Serialize)]
        struct Data<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            path: Option<Text<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            lines: Option<Text<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            line_number: Option<u64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            absolute_offset: Option<u64>,
            submatches: Vec<Hit<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            replacement: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            label: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            line_count: Option<u64>,
        }
        #[derive(Serialize)]
        struct Message<'a> {
            r#type: &'static str,
            data: Data<'a>,
        }
        let line = self.text.as_deref();
        let message = Message {
            r#type: "match",
            data: Data {
                path: (!self.path.is_empty()).then_some(Text { text: &self.path }),
                lines: line.map(|text| Text { text }),
                line_number: self.line_number,
                absolute_offset: self.absolute_offset,
                submatches: self
                    .hits
                    .iter()
                    .map(|&(start, end)| Hit {
                        r#match: Text {
                            text: line.and_then(|l| l.get(start..end)).unwrap_or(""),
                        },
                        start,
                        end,
                    })
                    .collect(),
                replacement: self.replacement.as_deref(),
                label: self.label.as_deref(),
                line_count: self.line_count,
            },
        };
        serde_json::to_value(message).unwrap_or(serde_json::Value::Null)
    }
}

/// The matches as the export writes them: CSV (`File,Line,Content`; a Documents search's second column is
/// `Where`) or, for a `.json` file, ripgrep's messages with two-space indentation.
pub fn export_text(matches: &[&Match], file: &str, documents: bool) -> String {
    if file.to_lowercase().ends_with(".json") {
        let all: Vec<serde_json::Value> = matches.iter().map(|m| m.as_json()).collect();
        return serde_json::to_string_pretty(&all).unwrap_or_default();
    }
    let csv = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
    let mut out = format!("File,{},Content", if documents { "Where" } else { "Line" });
    for m in matches {
        let second = m
            .label
            .clone()
            .or_else(|| m.line_number.map(|n| n.to_string()))
            .unwrap_or_default();
        let line = m.text.as_deref().unwrap_or("").trim();
        out.push('\n');
        out.push_str(&[csv(&m.path), csv(&second), csv(line)].join(","));
    }
    out
}

// ---- What core sends -----------------------------------------------------------------------------------

/// One `search-event` as core emitted it: the search it belongs to, its kind ("data", "error", "finished")
/// and ripgrep's JSON lines.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RawEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub lines: Vec<String>,
}

/// What a batch of lines said, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Matches(Vec<Match>),
    /// The end-of-search summary.
    Summary {
        /// How long the search took, in milliseconds.
        elapsed_ms: Option<f64>,
        /// How many files (documents) were searched.
        searches: Option<u64>,
        /// A Documents search's documents that couldn't be read.
        unreadable: u64,
    },
    /// The search failed, with ripgrep's words.
    Error(String),
    /// The search is over.
    Finished,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: Data,
}

#[derive(Default, Deserialize)]
struct Data {
    path: Option<Text>,
    lines: Option<Text>,
    line_number: Option<u64>,
    absolute_offset: Option<u64>,
    #[serde(default)]
    submatches: Vec<Hit>,
    replacement: Option<String>,
    label: Option<String>,
    line_count: Option<u64>,
    elapsed_total: Option<Elapsed>,
    stats: Option<Stats>,
}

#[derive(Deserialize)]
struct Text {
    text: Option<String>,
}

#[derive(Deserialize)]
struct Hit {
    start: u64,
    end: u64,
}

#[derive(Deserialize)]
struct Elapsed {
    #[serde(default)]
    secs: u64,
    #[serde(default)]
    nanos: u64,
}

#[derive(Deserialize)]
struct Stats {
    searches: Option<u64>,
    unreadable: Option<u64>,
}

/// Reads one of ripgrep's lines. Only `match` and `summary` messages are used; anything else, and text that
/// isn't JSON, is skipped (the TypeScript's `catch { // not a JSON line }`).
fn parse_line(line: &str) -> Option<Said> {
    // The JSON always starts with the type field, so most lines are told apart without parsing them.
    if !line.starts_with(r#"{"type":"match""#) && !line.starts_with(r#"{"type":"summary""#) {
        return None;
    }
    let parsed: Line = serde_json::from_str(line).ok()?;
    match parsed.kind.as_str() {
        "match" => {
            let d = parsed.data;
            let hits = d
                .submatches
                .iter()
                // Offsets that don't fit are dropped, not guessed.
                .filter_map(|h| Some((usize::try_from(h.start).ok()?, usize::try_from(h.end).ok()?)))
                .collect();
            Some(Said::Matches(vec![Match {
                path: d.path.and_then(|p| p.text).unwrap_or_default(),
                text: d.lines.and_then(|l| l.text),
                line_number: d.line_number,
                absolute_offset: d.absolute_offset,
                hits,
                replacement: d.replacement,
                label: d.label,
                line_count: d.line_count,
            }]))
        }
        "summary" => {
            let d = parsed.data;
            Some(Said::Summary {
                // `total.secs * 1000 + total.nanos / 1e6`; zero counts as absent (`if (total)` was about the
                // object, and `elapsed || performance.now() - started` about the number).
                elapsed_ms: d.elapsed_total.map(|e| e.secs as f64 * 1000. + e.nanos as f64 / 1e6),
                searches: d.stats.as_ref().and_then(|s| s.searches),
                unreadable: d.stats.and_then(|s| s.unreadable).unwrap_or(0),
            })
        }
        _ => None,
    }
}

/// What an event says, in order. Runs off the UI thread: a batch can be a thousand lines.
pub fn parse(event: &RawEvent) -> Vec<Said> {
    match event.kind.as_str() {
        "data" => {
            let mut said: Vec<Said> = Vec::new();
            let mut matches: Vec<Match> = Vec::new();
            for line in &event.lines {
                match parse_line(line) {
                    Some(Said::Matches(mut found)) => matches.append(&mut found),
                    Some(summary) => {
                        if !matches.is_empty() {
                            said.push(Said::Matches(std::mem::take(&mut matches)));
                        }
                        said.push(summary);
                    }
                    None => {}
                }
            }
            if !matches.is_empty() {
                said.push(Said::Matches(matches));
            }
            said
        }
        "error" => vec![Said::Error(
            event
                .lines
                .first()
                .cloned()
                .unwrap_or_else(|| "Unknown error".to_string()),
        )],
        "finished" => vec![Said::Finished],
        _ => Vec::new(),
    }
}

// ---- The search -----------------------------------------------------------------------------------------

/// What to search for: the arguments of App.tsx's `search`, with the settings and options it read from the
/// store already looked up.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub query: String,
    pub path: String,
    pub options: SearchOptions,
    /// A code search's rewrite (the Replace box, when it is open and has text).
    pub rewrite: Option<String>,
    /// Stop after this many matches (`settings.maxResults`); None for all.
    pub limit: Option<u64>,
    /// Searched again skipping nothing.
    pub everything: bool,
    pub exclusions: Vec<String>,
}

/// The search on screen (`SearchRun`).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: String,
    pub query: String,
    /// The folder as searched, once core has said (`%USERPROFILE%` expanded, `c:/proj\` tidied).
    pub path: String,
    pub options: SearchOptions,
    pub rewrite: Option<String>,
    pub limit: Option<u64>,
    pub everything: bool,
}

/// Something about the search changed that the view must act on, beyond redrawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchEvent {
    /// A new search began (the results start empty).
    Started,
    /// The search ended.
    Finished,
    /// The results were closed: back to the start screen.
    Closed,
}

/// The search that runs, and what it found so far.
pub struct Search {
    source: Rc<dyn Source>,
    run: Option<Run>,
    matches: Vec<Match>,
    searching: bool,
    error: String,
    /// How long the search took, in milliseconds (0 while it runs).
    duration: f64,
    files_searched: Option<u64>,
    unreadable: u64,
    selected: Option<usize>,
    started_at: Option<Instant>,
    /// Core's own timing from the summary.
    elapsed: f64,
    serial: u64,
    /// Events to the pump that parses them.
    events: async_channel::Sender<RawEvent>,
    /// Events handed to the pump that it hasn't applied yet.
    pending: std::cell::Cell<usize>,
    /// The search's start, which waits for the index to say whether it covers the folder.
    starting: Option<Task<()>>,
    replacing: bool,
    _core: Option<Subscription>,
}

impl EventEmitter<SearchEvent> for Search {}

impl Search {
    /// A search that asks `source`, and listens for core's `search-event`s.
    pub fn new(source: Rc<dyn Source>, cx: &mut Context<Self>) -> Self {
        let (events, heard) = async_channel::unbounded::<RawEvent>();
        // One pump, in order: each event is parsed on a background thread, then applied here.
        cx.spawn(async move |this, cx| {
            while let Ok(event) = heard.recv().await {
                let id = event.id.clone();
                let said = cx.background_spawn(async move { parse(&event) }).await;
                let applied = this.update(cx, |this, cx| {
                    this.apply(&id, said, cx);
                    this.pending.set(this.pending.get().saturating_sub(1));
                });
                if applied.is_err() {
                    return;
                }
            }
        })
        .detach();
        let core = core_bridge::subscribe(cx, |this, event, _| {
            if event.name == "search-event" {
                if let Ok(raw) = serde_json::from_value::<RawEvent>(event.payload.clone()) {
                    this.receive(raw);
                }
            }
        });
        Search {
            source,
            run: None,
            matches: Vec::new(),
            searching: false,
            error: String::new(),
            duration: 0.,
            files_searched: None,
            unreadable: 0,
            selected: None,
            started_at: None,
            elapsed: 0.,
            serial: 0,
            events,
            pending: std::cell::Cell::new(0),
            starting: None,
            replacing: false,
            _core: core,
        }
    }

    // ---- What there is ---------------------------------------------------------------------------------

    /// The search on screen, or None at the start screen.
    pub fn run(&self) -> Option<&Run> {
        self.run.as_ref()
    }

    /// Everything found so far, in the order it arrived (each file's matches together).
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    /// Whether the search is still running.
    pub fn searching(&self) -> bool {
        self.searching
    }

    /// Why the search failed, or empty.
    pub fn error(&self) -> &str {
        &self.error
    }

    /// How long the finished search took, in milliseconds (0 while it runs).
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// A Documents search's documents read, once the summary came.
    pub fn files_searched(&self) -> Option<u64> {
        self.files_searched
    }

    /// A Documents search's documents it couldn't read.
    pub fn unreadable(&self) -> u64 {
        self.unreadable
    }

    /// The match picked, if any (`selected`).
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Whether a replace is under way (a second would replace in what the first just wrote).
    pub fn replacing(&self) -> bool {
        self.replacing
    }

    pub(super) fn set_replacing(&mut self, replacing: bool) {
        self.replacing = replacing;
    }

    /// The source, for the work around the search (reading files, opening them).
    pub fn source(&self) -> Rc<dyn Source> {
        self.source.clone()
    }

    /// The search stopped at its limit: it finished, a limit was set, and that many matches came
    /// (`limited`).
    pub fn limited(&self) -> bool {
        !self.searching
            && self
                .run
                .as_ref()
                .and_then(|r| r.limit)
                .is_some_and(|limit| self.matches.len() as u64 >= limit)
    }

    /// The match shown: the one picked, else (with auto-preview on) the first (`current`).
    pub fn current(&self, auto_open_preview: bool) -> Option<usize> {
        self.selected
            .filter(|&i| i < self.matches.len())
            .or_else(|| (auto_open_preview && !self.matches.is_empty()).then_some(0))
    }

    /// The files with matches, each once, in the order they came (`files` in `replaceAll`).
    pub fn files(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.matches
            .iter()
            .filter(|m| !m.path.is_empty() && seen.insert(m.path.as_str()))
            .map(|m| m.path.clone())
            .collect()
    }

    // ---- Doing ---------------------------------------------------------------------------------------------

    /// Picks a match, or none (`onSelect`).
    pub fn select(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != index {
            self.selected = index;
            cx.notify();
        }
    }

    /// Starts a search, replacing the one on screen (`search`). Nothing for an empty query or folder. The
    /// search goes into the history first; the matches stream in as core finds them.
    pub fn search(&mut self, spec: Spec, cx: &mut Context<Self>) {
        if spec.query.is_empty() || spec.path.is_empty() {
            return;
        }
        self.serial += 1;
        let id = format!("{:x}-{}", std::process::id(), self.serial);
        self.run = Some(Run {
            id: id.clone(),
            query: spec.query.clone(),
            path: spec.path.clone(),
            options: spec.options,
            rewrite: spec.rewrite.clone(),
            limit: spec.limit,
            everything: spec.everything,
        });
        self.matches.clear();
        self.selected = None;
        self.error.clear();
        self.duration = 0.;
        self.elapsed = 0.;
        self.files_searched = None;
        self.unreadable = 0;
        self.searching = true;
        self.started_at = Some(Instant::now());
        let (query, path) = (spec.query.clone(), spec.path.clone());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        crate::persistence::update_app(cx, |app| app.add_to_history(&query, &path, now));
        cx.emit(SearchEvent::Started);
        cx.notify();

        let source = self.source.clone();
        // Words alone, in a folder the content index covers, are answered by the index, in milliseconds.
        let o = spec.options;
        let plain =
            !o.code && !o.documents && !o.meaning && !o.regex && !o.case_sensitive && !o.whole_word && !spec.everything;
        // A search still waiting to start is dropped with its task: the newer one replaces it.
        self.starting = Some(cx.spawn(async move |this, cx| {
            let indexed = plain && source.covers(&spec.path).await.unwrap_or(false);
            let engine = if o.code {
                Engine::Code
            } else if indexed {
                Engine::Index
            } else if o.documents || o.meaning {
                Engine::Documents
            } else {
                Engine::Ripgrep
            };
            let request = Request {
                id: id.clone(),
                query: spec.query.clone(),
                path: spec.path.clone(),
                options: spec.options,
                rewrite: spec.rewrite.clone(),
                exclusions: spec.exclusions.clone(),
                everything: spec.everything,
                max_results: spec.limit.and_then(|l| usize::try_from(l).ok()),
            };
            let started = source.start(engine, request).await;
            this.update(cx, |this, cx| {
                if !this.is_current(&id) {
                    return;
                }
                match started {
                    Ok(folder) => {
                        // Paths show relative to the folder as searched (`c:/proj\` or `%USERPROFILE%` typed).
                        if let Some(run) = this.run.as_mut().filter(|r| r.path != folder) {
                            run.path = folder;
                            cx.notify();
                        }
                    }
                    // Stopped, or replaced, as it started: not a failure.
                    Err(reason) if reason == "cancelled" => this.finish(cx),
                    Err(reason) => {
                        this.error = reason;
                        this.finish(cx);
                    }
                }
            })
            .ok();
        }));
    }

    /// Stop: ends the search that runs (`cancelSearch`). The results found so far stay; core says
    /// "finished" when the process has ended.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.searching {
            self.source.cancel();
            cx.notify();
        }
    }

    /// Back to the start screen (`closeResults`).
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.searching {
            self.source.cancel();
        }
        self.starting = None;
        self.run = None;
        self.matches.clear();
        self.selected = None;
        self.searching = false;
        cx.emit(SearchEvent::Closed);
        cx.notify();
    }

    /// Hands an event from core to the pump. Any thread's events are fine; they are applied in order.
    pub fn receive(&self, event: RawEvent) {
        if self.events.try_send(event).is_ok() {
            self.pending.set(self.pending.get() + 1);
        }
    }

    /// Whether every event handed over has been applied (or dropped as another search's).
    pub fn idle(&self) -> bool {
        self.pending.get() == 0
    }

    fn is_current(&self, id: &str) -> bool {
        self.run.as_ref().is_some_and(|r| r.id == id)
    }

    /// The search ended (`finish`): its duration is core's, else measured here.
    fn finish(&mut self, cx: &mut Context<Self>) {
        if !self.searching {
            return;
        }
        self.searching = false;
        self.duration = if self.elapsed > 0. {
            self.elapsed
        } else {
            self.started_at.map_or(0., |at| at.elapsed().as_secs_f64() * 1000.)
        };
        cx.emit(SearchEvent::Finished);
        cx.notify();
    }

    /// Applies what an event said, if its search is still the one on screen.
    fn apply(&mut self, id: &str, said: Vec<Said>, cx: &mut Context<Self>) {
        if !self.is_current(id) {
            return;
        }
        for said in said {
            match said {
                Said::Matches(found) => self.matches.extend(found),
                Said::Summary {
                    elapsed_ms,
                    searches,
                    unreadable,
                } => {
                    if let Some(ms) = elapsed_ms {
                        self.elapsed = ms;
                    }
                    self.files_searched = searches;
                    self.unreadable = unreadable;
                }
                Said::Error(message) => {
                    self.error = message;
                    self.finish(cx);
                }
                Said::Finished => self.finish(cx),
            }
        }
        cx.notify();
    }
}

// ---- Replace all -------------------------------------------------------------------------------------------

/// How a Replace all went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Replaced {
    /// Files that were changed.
    pub changed: usize,
    /// Files that couldn't be read, changed or written.
    pub failed: usize,
}

/// Replace all in each of `files`, one at a time: reads it, replaces the way ripgrep found the matches
/// (`replaceText`, on a background thread) and writes it back if that changed anything. A file that fails
/// is counted and the rest go on.
pub async fn replace_in_files(
    source: Rc<dyn Source>,
    files: Vec<String>,
    query: String,
    options: SearchOptions,
    replacement: String,
    background: BackgroundExecutor,
) -> Replaced {
    let mut done = Replaced::default();
    for path in files {
        let Ok(content) = source.read(&path).await else {
            done.failed += 1;
            continue;
        };
        let (text, find, with) = (content.clone(), query.clone(), replacement.clone());
        let next = background
            .spawn(async move { text::replace_text(&text, &find, &options, &with) })
            .await;
        match next {
            Ok(next) if next != content => match source.write(&path, next).await {
                Ok(()) => done.changed += 1,
                Err(_) => done.failed += 1,
            },
            Ok(_) => {}
            Err(_) => done.failed += 1,
        }
    }
    done
}

/// A short account of a pattern the replace can't use (`toRegExp` threw).
pub fn pattern_problem(query: &str, options: &SearchOptions) -> Option<SharedString> {
    text::to_reg_exp(query, options).err().map(|e| e.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(path: &str, text: &str, number: u64, hits: &[(usize, usize)]) -> String {
        let hits: Vec<String> = hits
            .iter()
            .map(|(s, e)| format!(r#"{{"match":{{"text":"x"}},"start":{s},"end":{e}}}"#))
            .collect();
        format!(
            r#"{{"type":"match","data":{{"path":{{"text":{}}},"lines":{{"text":{}}},"line_number":{number},"absolute_offset":0,"submatches":[{}]}}}}"#,
            serde_json::to_string(path).unwrap_or_default(),
            serde_json::to_string(text).unwrap_or_default(),
            hits.join(",")
        )
    }

    #[test]
    fn a_match_line_is_read_with_its_byte_offsets() {
        let said = parse(&RawEvent {
            id: "a".into(),
            kind: "data".into(),
            lines: vec![
                r#"{"type":"begin","data":{"path":{"text":"C:\\a.txt"}}}"#.into(),
                line("C:\\a.txt", "مرحبا hello\n", 3, &[(11, 16)]),
                "not json at all".into(),
                r#"{"type":"end","data":{}}"#.into(),
            ],
        });
        let [Said::Matches(found)] = said.as_slice() else {
            panic!("{said:?}")
        };
        assert_eq!(found.len(), 1);
        let m = &found[0];
        assert_eq!((m.path.as_str(), m.line_number), ("C:\\a.txt", Some(3)));
        // The Arabic word is 10 bytes and a space: the hit starts at byte 11, on a char boundary.
        let segments = m.segments();
        assert_eq!(
            segments.iter().map(|s| (s.text.as_str(), s.hit)).collect::<Vec<_>>(),
            [("مرحبا ", false), ("hello", true), ("", false)]
        );
    }

    #[test]
    fn the_summary_gives_the_time_the_files_searched_and_the_unreadable() {
        let said = parse(&RawEvent {
            id: "a".into(),
            kind: "data".into(),
            lines: vec![
                line("a", "x\n", 1, &[(0, 1)]),
                r#"{"type":"summary","data":{"elapsed_total":{"secs":1,"nanos":500000000,"human":"1.5s"},"stats":{"searches":42,"unreadable":3}}}"#.into(),
            ],
        });
        assert!(matches!(said[0], Said::Matches(_)));
        assert_eq!(
            said[1],
            Said::Summary {
                elapsed_ms: Some(1500.),
                searches: Some(42),
                unreadable: 3
            }
        );
        // A summary without stats leaves them unknown.
        let bare = parse(&RawEvent {
            id: "a".into(),
            kind: "data".into(),
            lines: vec![r#"{"type":"summary","data":{}}"#.into()],
        });
        assert_eq!(
            bare,
            [Said::Summary {
                elapsed_ms: None,
                searches: None,
                unreadable: 0
            }]
        );
    }

    #[test]
    fn errors_and_the_end_are_events_of_their_own() {
        let error = parse(&RawEvent {
            id: "a".into(),
            kind: "error".into(),
            lines: vec!["regex parse error".into()],
        });
        assert_eq!(error, [Said::Error("regex parse error".into())]);
        let silent = parse(&RawEvent {
            id: "a".into(),
            kind: "error".into(),
            lines: vec![],
        });
        assert_eq!(silent, [Said::Error("Unknown error".into())]);
        let finished = parse(&RawEvent {
            id: "a".into(),
            kind: "finished".into(),
            lines: vec![],
        });
        assert_eq!(finished, [Said::Finished]);
    }

    #[test]
    fn a_documents_match_carries_where_it_is() {
        let said = parse(&RawEvent {
            id: "a".into(),
            kind: "data".into(),
            lines: vec![r#"{"type":"match","data":{"path":{"text":"C:\\a.pdf"},"lines":{"text":"hi"},"line_number":2,"label":"p. 3","line_count":4,"submatches":[]}}"#.into()],
        });
        let [Said::Matches(found)] = said.as_slice() else {
            panic!("{said:?}")
        };
        assert_eq!(found[0].label.as_deref(), Some("p. 3"));
        assert_eq!(found[0].line_count, Some(4));
    }

    #[test]
    fn csv_quotes_its_cells_and_a_documents_search_says_where() {
        let a = Match {
            path: "C:\\a \"x\".txt".into(),
            text: Some("  say \"hi\"\r\n".into()),
            line_number: Some(7),
            ..Default::default()
        };
        let b = Match {
            path: "C:\\b.pdf".into(),
            text: Some("page".into()),
            line_number: Some(1),
            label: Some("p. 2".into()),
            ..Default::default()
        };
        let csv = export_text(&[&a], "x.csv", false);
        assert_eq!(
            csv,
            "File,Line,Content\n\"C:\\a \"\"x\"\".txt\",\"7\",\"say \"\"hi\"\"\""
        );
        let docs = export_text(&[&b], "x.CSV", true);
        assert_eq!(docs, "File,Where,Content\n\"C:\\b.pdf\",\"p. 2\",\"page\"");
        // JSON is ripgrep's message, indented by two.
        let json = export_text(&[&a], "x.json", false);
        assert!(json.starts_with("[\n  {\n    \"type\": \"match\""), "{json}");
    }
}
