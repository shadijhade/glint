//! Searching inside files: the other half of a search. `find.rs` looks for the words in names; this looks for them
//! in what the files hold, and adds the files whose lines match to the same run, so one list shows both.
//!
//! It replaces the text mode the window once had (Search.tsx, Results.tsx and the `search` of App.tsx): the same
//! engines answer, what they find is a file's row with its matching lines under it.
//!
//! # How it runs
//!
//! [`Explorer::run_find_in`] starts it beside the name search when the search has words to look for
//! ([`inside_words`]) in a folder on disk ([`can_search_inside`]). A whole drive is searched only when asked
//! ([`Wider::inside_too`], the results' bar offers it): its run says so with [`Inside::offered`]. A search as it is
//! typed looks inside only where the content index covers the folder, since it must answer at once.
//!
//! Core has one content search at a time: a newer one ends the older (in any tab), which keeps what it found.
//! Which engine answers follows the search box's switches (`persistence::app(cx).options`): the content index for
//! plain words in a folder it covers, the documents' reader when Documents or Meaning is on, else ripgrep.
//!
//! What core finds comes as `search-event`s ([`Explorer::inside_heard`]). One pump reads them in order: each is
//! parsed on a background thread, the files not seen yet are read for their size and date through the [`Source`],
//! then the lot is added to the run ([`Explorer::inside_apply`]).
//!
//! # What a run holds
//!
//! [`Inside`]: every file's matching lines by path ([`Hit`]), and which files were found by their contents alone
//! (`only`): those are appended to [`FindRun::entries`], so every operation on a result (open, copy, rename,
//! delete, drag) works on them as on any other. The rows put them under "Inside files" and each file's lines
//! under its row (`rows.rs`).

use super::find_types::*;
use super::model::{Explorer, Prefs};
use super::types::*;
use glint_model::entries::{matcher, terms, FileEntry};
use glint_model::paths::{in_archive_path, is_folder_path, is_recycle_bin, is_remote_path, is_root, THIS_PC};
use glint_model::store::SearchOptions;
use glint_model::text::{self, RgMatch, Submatch};
use gpui_kit::{AppContext as _, Context, Task};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// The fewest characters looked for inside files: one letter is in every file.
const FEWEST_CHARACTERS: usize = 2;

// ---- What is asked -----------------------------------------------------------------------------------------

/// Which of core's searches answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsideEngine {
    /// ripgrep over the files (`run_ripgrep_batched`).
    Ripgrep,
    /// The content index, for plain words in a folder it covers (`run_content_search`).
    Index,
    /// Documents (PDF, Office, books), pictures' text, and Meaning (`run_doc_search`).
    Documents,
}

/// A search inside files. Core builds the tool's arguments from these; nothing here is passed through as a flag.
#[derive(Clone, Debug, PartialEq)]
pub struct InsideRequest {
    /// Names the search in its `search-event`s.
    pub id: String,
    pub query: String,
    pub path: String,
    pub options: SearchOptions,
    pub exclusions: Vec<String>,
    /// Skipping nothing: hidden files, what .gitignore files list and the exclusions too.
    pub everything: bool,
    /// Stop after this many matching lines (None: all).
    pub max_results: Option<usize>,
}

/// The filters and other terms of a search that aren't words to look for inside files.
fn is_word(term: &str) -> bool {
    if term.contains(['\\', '/', '*', '?']) {
        return false;
    }
    match term.split_once(':') {
        Some((key, _)) => !matches!(
            key.to_ascii_lowercase().as_str(),
            "ext"
                | "tag"
                | "path"
                | "size"
                | "dm"
                | "file"
                | "files"
                | "folder"
                | "folders"
                | "width"
                | "height"
                | "taken"
                | "camera"
                | "duration"
                | "artist"
                | "album"
                | "git"
        ),
        None => true,
    }
}

/// The words of a search that are looked for inside files, as one phrase: its terms that are neither filters
/// (`ext:pdf`), paths (`src\lib`) nor wildcards (`*.rs`). Empty when there is nothing to look for: no such term,
/// one letter, or a search for folders alone.
pub fn inside_words(query: &str) -> String {
    let all = terms(query);
    let folders_only = all.iter().any(|t| {
        let t = t.to_ascii_lowercase();
        t == "folder:" || t == "folders:"
    });
    if folders_only {
        return String::new();
    }
    let words: Vec<&str> = all.iter().map(String::as_str).filter(|t| is_word(t)).collect();
    let phrase = words.join(" ");
    if phrase.chars().count() < FEWEST_CHARACTERS {
        return String::new();
    }
    phrase
}

/// The filters of a search (`ext:pdf size:>1mb dm:7d`), which narrow the files found by their contents as they
/// narrow the ones found by name. Paths and wildcards are about names, so they aren't among them.
pub fn inside_filters(query: &str) -> String {
    terms(query)
        .into_iter()
        .filter(|t| !is_word(t) && !t.contains(['\\', '/', '*', '?']))
        .filter(|t| {
            let key = t.split_once(':').map_or("", |(key, _)| key).to_ascii_lowercase();
            matches!(key.as_str(), "ext" | "size" | "dm" | "file" | "files")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether what is inside the files of `scope` can be searched: a folder on disk (not This PC, an archive, a
/// remote connection or the Recycle Bin).
pub fn can_search_inside(scope: &str) -> bool {
    scope != THIS_PC
        && is_folder_path(scope)
        && !is_remote_path(scope)
        && !is_recycle_bin(scope)
        && !in_archive_path(scope)
}

/// Whether a search from `scope` looks inside the files without being asked: any folder but a drive's top, whose
/// every file would be read.
pub fn inside_unasked(scope: &str) -> bool {
    can_search_inside(scope) && !is_root(scope)
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

/// One match line (ripgrep's `match` message).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    /// The file (`data.path.text`); empty when ripgrep sent its name as bytes.
    pub path: String,
    /// The line's text, with its line end (`data.lines.text`), or None when ripgrep sent none.
    pub text: Option<String>,
    /// The line's number, from 1 (`data.line_number`).
    pub line_number: Option<u64>,
    /// The hits in the line: UTF-8 byte ranges (`data.submatches`).
    pub hits: Vec<(usize, usize)>,
    /// A Documents search's match: where it is in the document ("p. 3", "slide 2", "Sales, row 7").
    pub label: Option<String>,
}

impl Match {
    /// The match as a row shows it: trimmed, in plain and matching parts.
    pub fn hit(&self) -> Hit {
        let rg = RgMatch {
            lines_text: self.text.clone(),
            submatches: self.hits.iter().map(|&(start, end)| Submatch { start, end }).collect(),
        };
        Hit {
            line: self.line_number,
            label: self.label.clone(),
            parts: text::segments(&rg),
        }
    }
}

/// What a batch of lines said, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    Matches(Vec<Match>),
    /// The end-of-search summary: how long it took, in milliseconds.
    Summary {
        elapsed_ms: Option<f64>,
    },
    /// The search failed, with the tool's words.
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
    #[serde(default)]
    submatches: Vec<Sub>,
    label: Option<String>,
    elapsed_total: Option<Elapsed>,
}

#[derive(Deserialize)]
struct Text {
    text: Option<String>,
}

#[derive(Deserialize)]
struct Sub {
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

/// Reads one of ripgrep's lines. Only `match` and `summary` messages are used; anything else, and text that
/// isn't JSON, is skipped.
fn parse_line(line: &str) -> Option<Said> {
    // Most lines are told apart without parsing them: the JSON starts with the type field, except ripgrep 15's
    // summary, which ends with it.
    if !line.starts_with(r#"{"type":"match""#) && !glint_core::is_rg_summary(line) {
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
                hits,
                label: d.label,
            }]))
        }
        "summary" => Some(Said::Summary {
            elapsed_ms: parsed
                .data
                .elapsed_total
                .map(|e| e.secs as f64 * 1000. + e.nanos as f64 / 1e6),
        }),
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

// ---- The search that runs ------------------------------------------------------------------------------------

/// The content search that runs (core has one at a time).
pub(super) struct InsideJob {
    tab: TabId,
    /// The tab's search it belongs to ([`FindRun::id`]).
    find: u64,
    /// Names it in core's events.
    id: String,
    started: Instant,
    /// The files already read for their size and date, so each is read once.
    seen: HashSet<String>,
    /// The start, which waits for the index to say whether it covers the folder. Dropped with the job.
    _starting: Task<()>,
}

/// What the pump read of one event, off the UI thread.
struct Heard {
    id: String,
    said: Vec<Said>,
}

impl Explorer {
    /// The words the active tab's search looks for inside files, when it does or could.
    #[cfg(test)]
    pub fn inside_of(&self, tab: TabId) -> Option<&Inside> {
        self.tabs.get(&tab)?.find.as_ref().map(|find| &find.inside)
    }

    /// Starts looking inside the files for the words of `tab`'s search, replacing any content search that runs.
    /// Nothing when the tab shows no search, the search has no words, or the place can't be searched.
    /// `wider` carries the same "without the limit" and "skipping nothing" the name search was given.
    pub fn search_inside(&mut self, tab: TabId, wider: &Wider, cx: &mut Context<Self>) {
        let Some(find) = self.tabs.get(&tab).and_then(|t| t.find.as_ref()) else {
            return;
        };
        let (find_id, scope) = (find.id, find.scope.clone());
        let words = inside_words(&find.query);
        if words.is_empty() || !can_search_inside(&scope) {
            return;
        }
        self.end_inside_job(cx);
        let prefs = Prefs::read(cx);
        // The search box's switches. A code pattern has no place in a search by name and contents.
        let mut options = crate::persistence::app(cx).map(|app| app.options).unwrap_or_default();
        options.code = false;
        self.searching.inside_serial += 1;
        let id = format!("in-{:x}-{}", std::process::id(), self.searching.inside_serial);
        let request = InsideRequest {
            id: id.clone(),
            query: words.clone(),
            path: scope.clone(),
            options,
            exclusions: if wider.skip_nothing {
                Vec::new()
            } else {
                prefs.inside_exclusions.clone()
            },
            everything: wider.skip_nothing,
            max_results: if wider.unlimited {
                None
            } else {
                prefs.inside_max_results.and_then(|max| usize::try_from(max).ok())
            },
        };
        self.update_tab(tab, |t| {
            if let Some(find) = &mut t.find {
                drop_inside_files(find);
                find.inside = Inside {
                    words,
                    running: true,
                    ..Default::default()
                };
            }
        });
        self.ensure_inside_pump(cx);
        let source = self.source.clone();
        let o = options;
        let everything = wider.skip_nothing;
        let run_id = id.clone();
        let starting = cx.spawn(async move |this, cx| {
            // Words alone, in a folder the content index covers, are answered by the index, in milliseconds.
            let plain = !o.documents && !o.regex && !o.case_sensitive && !o.whole_word && !everything;
            let indexed = plain && source.inside_covers(&scope).await;
            let engine = if indexed {
                InsideEngine::Index
            } else if o.documents {
                InsideEngine::Documents
            } else {
                InsideEngine::Ripgrep
            };
            let started = source.inside_start(engine, request).await;
            this.update(cx, |this, cx| match started {
                Ok(()) => {}
                // Stopped, or replaced, as it started: not a failure.
                Err(reason) if reason == "cancelled" => this.inside_ended(&run_id, None, cx),
                Err(reason) => this.inside_ended(&run_id, Some(reason), cx),
            })
            .ok();
        });
        self.searching.inside = Some(InsideJob {
            tab,
            find: find_id,
            id,
            started: Instant::now(),
            seen: HashSet::new(),
            _starting: starting,
        });
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::TabChanged(tab));
        cx.notify();
    }

    /// Stop: the content search of `tab` ends, keeping what it found.
    pub fn stop_inside(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.searching.inside.as_ref().is_some_and(|job| job.tab == tab) {
            self.end_inside_job(cx);
        }
    }

    /// Ends the content search that runs, if one does: core is told, and its run stops saying it is running.
    pub(super) fn end_inside_job(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.searching.inside.take() else {
            return;
        };
        self.source.inside_cancel();
        let elapsed = job.started.elapsed().as_secs_f64() * 1000.0;
        let ours = |t: &super::Tab| t.find.as_ref().is_some_and(|f| f.id == job.find && f.inside.running);
        if self.tabs.get(&job.tab).is_some_and(ours) {
            self.update_tab(job.tab, |t| {
                if let Some(find) = &mut t.find {
                    find.inside.running = false;
                    find.inside.elapsed = elapsed;
                }
            });
            cx.emit(ExplorerEvent::TabChanged(job.tab));
            cx.notify();
        }
    }

    /// A tab's search went away (left, replaced, its tab closed or moved to another folder): its content search
    /// ends with it.
    pub(super) fn inside_left(&mut self, tab: TabId) {
        if self.searching.inside.as_ref().is_some_and(|job| job.tab == tab) {
            self.searching.inside = None;
            self.source.inside_cancel();
        }
    }

    /// One of core's `search-event`s: handed to the pump, which applies them in order.
    pub fn inside_heard(&mut self, event: RawEvent, cx: &mut Context<Self>) {
        if self.searching.inside.as_ref().is_none_or(|job| job.id != event.id) {
            return;
        }
        self.ensure_inside_pump(cx);
        if let Some(events) = &self.searching.inside_events {
            if events.try_send(event).is_ok() {
                self.searching.inside_waiting += 1;
            }
        }
    }

    /// Whether every event handed over has been applied (tests wait for it).
    #[cfg(test)]
    pub fn inside_idle(&self) -> bool {
        self.searching.inside_waiting == 0
    }

    /// The pump, made at the first content search: the events waiting, parsed off the UI thread, their new files
    /// read through the source, then applied. Every event that is waiting is taken with the first: each application
    /// copies the run's lines and rows, so taking them one at a time while they came faster than they were applied
    /// made a big search's cost grow with the square of what it found.
    fn ensure_inside_pump(&mut self, cx: &mut Context<Self>) {
        if self.searching.inside_events.is_some() {
            return;
        }
        let (events, heard) = async_channel::unbounded::<RawEvent>();
        self.searching.inside_events = Some(events);
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(first) = heard.recv().await {
                let mut waiting = vec![first];
                while let Ok(more) = heard.try_recv() {
                    waiting.push(more);
                }
                // One read per run of events of the same search, in order, with how many events it holds.
                let reads: Vec<(Heard, usize)> = cx
                    .background_spawn(async move {
                        let mut reads: Vec<(Heard, usize)> = Vec::new();
                        for event in &waiting {
                            let said = parse(event);
                            match reads.last_mut() {
                                Some((read, count)) if read.id == event.id => {
                                    read.said.extend(said);
                                    *count += 1;
                                }
                                _ => reads.push((
                                    Heard {
                                        said,
                                        id: event.id.clone(),
                                    },
                                    1,
                                )),
                            }
                        }
                        reads
                    })
                    .await;
                for (read, count) in reads {
                    // The files of this batch that have no row yet.
                    let unread = this.update(cx, |this, _| this.inside_unread(&read));
                    let Ok(unread) = unread else { return };
                    let entries = if unread.is_empty() {
                        Vec::new()
                    } else {
                        source.entries_of(unread).await
                    };
                    let applied = this.update(cx, |this, cx| {
                        this.searching.inside_waiting = this.searching.inside_waiting.saturating_sub(count);
                        this.inside_apply(read, entries, cx);
                    });
                    if applied.is_err() {
                        return;
                    }
                }
            }
        })
        .detach();
    }

    /// The files a batch names that were not read yet (none when the batch is another search's).
    fn inside_unread(&mut self, heard: &Heard) -> Vec<String> {
        let Some(job) = self.searching.inside.as_mut().filter(|job| job.id == heard.id) else {
            return Vec::new();
        };
        let mut unread = Vec::new();
        for said in &heard.said {
            if let Said::Matches(matches) = said {
                for m in matches {
                    if !m.path.is_empty() && job.seen.insert(m.path.clone()) {
                        unread.push(m.path.clone());
                    }
                }
            }
        }
        unread
    }

    /// Adds what a batch found to its run, if the run is still the tab's.
    fn inside_apply(&mut self, heard: Heard, entries: Vec<FileEntry>, cx: &mut Context<Self>) {
        let Some((tab, find_id, started)) = self
            .searching
            .inside
            .as_ref()
            .filter(|job| job.id == heard.id)
            .map(|job| (job.tab, job.find, job.started))
        else {
            return;
        };
        let Some(mut run) = self
            .tabs
            .get(&tab)
            .and_then(|t| t.find.as_ref())
            .filter(|f| f.id == find_id)
            .cloned()
        else {
            self.inside_left(tab);
            return;
        };
        // The filters of the search narrow these files as they narrow the names (`ext:pdf budget`).
        let filters = inside_filters(&run.query);
        let test = matcher(&filters, None, None);
        let mut refused: HashSet<String> = HashSet::new();
        let mut fresh: Vec<FileEntry> = Vec::new();
        for entry in entries {
            if test.as_ref().is_some_and(|test| !test(&entry)) {
                refused.insert(entry.path);
            } else {
                fresh.push(entry);
            }
        }
        let mut ended: Option<Option<String>> = None;
        let mut core_elapsed = None;
        let mut hits = (*run.inside.hits).clone();
        let mut matches = run.inside.matches;
        for said in heard.said {
            match said {
                Said::Matches(found) => {
                    let mut by_file: HashMap<String, Vec<Hit>> = HashMap::new();
                    for m in found {
                        if m.path.is_empty() || refused.contains(&m.path) {
                            continue;
                        }
                        by_file.entry(m.path.clone()).or_default().push(m.hit());
                    }
                    for (path, mut lines) in by_file {
                        matches += lines.len();
                        let all = match hits.get(&path) {
                            Some(before) => {
                                let mut all = before.to_vec();
                                all.append(&mut lines);
                                all
                            }
                            None => lines,
                        };
                        hits.insert(path, all.into());
                    }
                }
                Said::Summary { elapsed_ms } => core_elapsed = elapsed_ms.filter(|ms| *ms > 0.),
                Said::Error(message) => ended = Some(Some(message)),
                Said::Finished => ended = ended.or(Some(None)),
            }
        }
        // A file refused by the filters has no row and no lines; one refused in an earlier batch stays out.
        hits.retain(|path, _| !refused.contains(path));
        if !fresh.is_empty() {
            // The files the name search listed already keep their rows: their lines go under them.
            let listed: HashSet<&str> = run.entries.iter().map(|e| e.path.as_str()).collect();
            let added: Vec<FileEntry> = fresh
                .into_iter()
                .filter(|e| hits.contains_key(&e.path) && !listed.contains(e.path.as_str()))
                .collect();
            if !added.is_empty() {
                let mut only = (*run.inside.only).clone();
                only.extend(added.iter().map(|e| e.path.clone()));
                run.inside.only = Arc::new(only);
                let mut all = Vec::with_capacity(run.entries.len() + added.len());
                all.extend(run.entries.iter().cloned());
                all.extend(added);
                run.entries = all.into();
            }
        }
        run.inside.hits = Arc::new(hits);
        run.inside.matches = matches;
        if let Some(elapsed) = core_elapsed {
            run.inside.elapsed = elapsed;
        }
        let limit = Prefs::one(cx, |s| s.max_results).and_then(|max| usize::try_from(max).ok());
        if let Some(error) = ended.clone() {
            run.inside.running = false;
            if run.inside.elapsed == 0. {
                run.inside.elapsed = started.elapsed().as_secs_f64() * 1000.0;
            }
            run.inside.limited = limit.is_some_and(|limit| matches >= limit);
            if let Some(error) = error {
                run.inside.error = error;
            }
        }
        self.update_tab(tab, |t| t.find = Some(run));
        if ended.is_some() {
            self.searching.inside = None;
        }
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::TabChanged(tab));
        cx.notify();
    }

    /// The content search named `id` couldn't start, or was stopped as it started.
    fn inside_ended(&mut self, id: &str, error: Option<String>, cx: &mut Context<Self>) {
        let Some(job) = self.searching.inside.take_if(|job| job.id == id) else {
            return;
        };
        let elapsed = job.started.elapsed().as_secs_f64() * 1000.0;
        self.update_tab(job.tab, |t| {
            if let Some(find) = t.find.as_mut().filter(|f| f.id == job.find) {
                find.inside.running = false;
                find.inside.elapsed = elapsed;
                if let Some(error) = error {
                    find.inside.error = error;
                }
            }
        });
        cx.emit(ExplorerEvent::TabChanged(job.tab));
        cx.notify();
    }
}

/// Takes the files an earlier content search added out of a run's results (a newer one starts from the names).
pub(super) fn drop_inside_files(find: &mut FindRun) {
    if find.inside.only.is_empty() {
        return;
    }
    let only = find.inside.only.clone();
    let kept: Vec<FileEntry> = find
        .entries
        .iter()
        .filter(|e| !only.contains(&e.path))
        .cloned()
        .collect();
    find.entries = kept.into();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(path: &str, text: &str, number: u64, hits: &[(usize, usize)]) -> String {
        let subs: Vec<serde_json::Value> = hits
            .iter()
            .map(|(start, end)| serde_json::json!({ "match": { "text": &text[*start..*end] }, "start": start, "end": end }))
            .collect();
        serde_json::json!({
            "type": "match",
            "data": { "path": { "text": path }, "lines": { "text": text }, "line_number": number, "submatches": subs }
        })
        .to_string()
    }

    #[test]
    fn the_words_looked_for_inside_files_leave_out_filters_paths_and_wildcards() {
        assert_eq!(inside_words("budget"), "budget");
        assert_eq!(inside_words("ext:pdf budget meeting"), "budget meeting");
        assert_eq!(inside_words("\"two words\" size:>1mb"), "two words");
        assert_eq!(inside_words("*.rs"), "", "a wildcard is about names");
        assert_eq!(inside_words("src\\lib"), "", "a path is about names");
        assert_eq!(inside_words("ext:pdf"), "", "filters alone look for nothing");
        assert_eq!(inside_words("a"), "", "one letter is in every file");
        assert_eq!(inside_words("folder: notes"), "", "folders hold no lines");
        assert_eq!(inside_words("http://x"), "", "a slash makes it a path");
        assert_eq!(
            inside_words("12:30 meeting"),
            "12:30 meeting",
            "a colon alone is no filter"
        );
    }

    #[test]
    fn only_the_filters_that_say_which_files_narrow_what_is_found_inside() {
        assert_eq!(inside_filters("ext:pdf budget size:>1mb"), "ext:pdf size:>1mb");
        assert_eq!(inside_filters("budget"), "");
        assert_eq!(inside_filters("path:src tag:work dm:7d x"), "dm:7d");
    }

    #[test]
    fn contents_are_searched_in_folders_on_disk_and_unasked_below_a_drives_top() {
        assert!(can_search_inside("C:\\Users\\me"));
        assert!(can_search_inside("D:\\"));
        assert!(!can_search_inside(THIS_PC));
        assert!(inside_unasked("C:\\Users\\me"));
        assert!(!inside_unasked("D:\\"), "a whole drive waits to be asked");
    }

    #[test]
    fn a_match_line_becomes_a_hit_in_plain_and_matching_parts() {
        let event = RawEvent {
            id: "a".into(),
            kind: "data".into(),
            lines: vec![
                line("C:\\x\\a.txt", "    the needle is here\n", 3, &[(8, 14)]),
                "not json".into(),
                r#"{"type":"summary","data":{"elapsed_total":{"secs":1,"nanos":500000000}}}"#.into(),
                // ripgrep 15 writes the summary with its type last.
                r#"{"data":{"elapsed_total":{"secs":2,"nanos":0}},"type":"summary"}"#.into(),
            ],
        };
        let said = parse(&event);
        assert_eq!(said.len(), 3);
        assert_eq!(
            said[2],
            Said::Summary {
                elapsed_ms: Some(2000.)
            }
        );
        let Said::Matches(matches) = &said[0] else {
            panic!("matches first")
        };
        let hit = matches[0].hit();
        assert_eq!(hit.line, Some(3));
        let parts: Vec<(&str, bool)> = hit.parts.iter().map(|p| (p.text.as_str(), p.hit)).collect();
        assert_eq!(parts, [("the ", false), ("needle", true), (" is here", false)]);
        assert_eq!(
            said[1],
            Said::Summary {
                elapsed_ms: Some(1500.)
            }
        );
    }

    #[test]
    fn errors_and_the_end_are_events_of_their_own() {
        let failed = RawEvent {
            id: "a".into(),
            kind: "error".into(),
            lines: vec!["regex parse error".into()],
        };
        assert_eq!(parse(&failed), [Said::Error("regex parse error".into())]);
        let ended = RawEvent {
            id: "a".into(),
            kind: "finished".into(),
            lines: Vec::new(),
        };
        assert_eq!(parse(&ended), [Said::Finished]);
    }
}
