//! JSON, YAML and TOML as a tree; CSV and TSV as a table.
//! Ported from DataTree.tsx and DataPreview.tsx's `TabularPreview` and `TableView`.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                       | Here                                                          |
//! |--------------------------------------------------|---------------------------------------------------------------|
//! | `DataTree`                                       | [`DataTree`]: the values of a JSON, YAML or TOML file as a tree |
//! | `jqQuery`, `jqStop`                              | [`QuerySource`] (glint-core's `jq_query` in its own process), [`parse_answer`] |
//! | `treeRows`, `initiallyOpen`, `PAGE`              | glint_model::json_tree, cached as [`Row`]s                    |
//! | `openAll`, `more`, `toggle`, the keys            | [`DataTree::open_all`], [`DataTree::more`], [`DataTree::toggle`], [`DataTree::on_key_down`] |
//! | `TabularPreview`, `TableView`                    | [`TableViewer`]: a virtualized table with a filter, sorting, and the raw text |
//! | `inspectTabular`                                 | [`TableSource`] (glint-core's `inspect_tabular`, the first 256 KB) |
//! | which preview a file gets                        | [`DataPreview`], by [`data_kind`]                             |
//!
//! Tree: the file is read in a process of its own (so a file or filter that never ends or eats memory is stopped
//! there, not here), up to 64 MB and 100,000 values; big objects and arrays show 500 items at a time and Alt+click
//! opens everything under one. The jq filter is run by jaq, an optional tool: when it isn't installed the error
//! says so (Settings, Tools installs it).
//!
//! Table: the first 256 KB of the file, sampled by glint-core. The kit's `DataTable` draws only the rows on screen.
//!
//! Not ported:
//! - The tree has no horizontal scroll: a row longer than the pane is cut at its end (strings are cut at 400
//!   characters anyway), and its full value is what Copy gives.
//! - The "jaq needed" card with Install (`ToolNeeded`) shows over the filter while a filter other than `.` is in
//!   force and jaq isn't installed (the app's tools model, from `dialogs::action_run::tools_model`); tests that read
//!   through a fake source have no tools model, so no card.
//! - NOTE(port): the kit's `DataTable` cycles a column's sort descending, ascending, off; the TypeScript went
//!   ascending, descending, off.

use super::hex::{core_hex_source, CopySource};
use super::text::{
    big_icon, entry_of, is_archived_path, name_of, note_centered, page_list, redraw_after_delay, waiting, TextPreview,
};
use crate::components::settings::tools::{needed, Tools};
use crate::dialogs::action_run::tools_model;
use crate::kit::prelude::*;
use crate::{actions, core_bridge, t, theme};
use glint_core::data_lens::TablePreview;
use glint_model::file_types::{data_format, DataFormat};
use glint_model::format::format_count;
use glint_model::json_tree::{
    initially_open, is_container, jq_path, parse_json, preview, size_of, tree_rows, JsonValue, RowKey, PAGE,
};
use gpui_kit::{ClipboardItem, ScrollStrategy, Task, UniformListScrollHandle};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

/// The most containers one Alt+click opens (`OPEN_ALL`): opening everything under a big one would be millions of
/// rows.
const OPEN_ALL: isize = 2000;
/// A row of the tree (`ROW`, 22 px at the usual size).
const ROW: Rems = rems(1.375);
/// How much of a delimited file is sampled (`inspectTabular`'s default).
pub const TABLE_BYTES: usize = 256 * 1024;

// ---- Where the data comes from -------------------------------------------------------------------------------

/// What a jq query gave: the values (a filter can give several, a file's documents are its values), whether it
/// stopped at the caps, the error it ended with (after the values before it), and how long it took in ms.
#[derive(Clone, Debug, PartialEq)]
pub struct JqAnswer {
    /// The values, in order.
    pub values: Vec<JsonValue>,
    /// It stopped at 100,000 values or 64 MB of them.
    pub truncated: bool,
    /// Why it ended early: the file couldn't be read as its format, or the filter failed.
    pub error: Option<String>,
    /// Milliseconds.
    pub elapsed: f64,
}

/// A query: the path, the file's format and the jq filter (`.` for the file as it is).
pub type QuerySource = Arc<
    dyn Fn(String, DataFormat, String) -> Pin<Box<dyn Future<Output = Result<JqAnswer, String>> + Send>> + Send + Sync,
>;

/// A delimited file's first rows.
pub type TableSource =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<TablePreview, String>> + Send>> + Send + Sync>;

/// What the data previewers read from: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct DataSource {
    /// `jq_query`.
    pub query: QuerySource,
    /// Stops the query running (`jq_stop`).
    pub stop: Arc<dyn Fn() + Send + Sync>,
    /// `inspect_tabular`.
    pub table: TableSource,
    /// A copy on disk of a file in an archive (`archive_extract`).
    pub copy: CopySource,
}

/// The words glint-core's jq worker is told a format by.
fn format_word(format: DataFormat) -> &'static str {
    match format {
        DataFormat::Json => "json",
        DataFormat::Yaml => "yaml",
        DataFormat::Toml => "toml",
    }
}

/// What `jq_query` answers: a line of JSON saying how it went (`truncated`, `error`, `elapsed`), then the values as
/// a JSON array.
pub fn parse_answer(bytes: &[u8]) -> Result<JqAnswer, String> {
    #[derive(serde::Deserialize)]
    struct Head {
        #[serde(default)]
        truncated: bool,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        elapsed: f64,
    }
    let line = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("The answer had no header")?;
    let head: Head = serde_json::from_slice(&bytes[..line]).map_err(|e| e.to_string())?;
    let text = std::str::from_utf8(&bytes[line + 1..]).map_err(|e| e.to_string())?;
    // Numbers stay as written: a big integer or 1.50 isn't turned into a float and back.
    let values = match parse_json(text).map_err(|e| e.to_string())? {
        JsonValue::Array(items) => items,
        _ => Vec::new(),
    };
    Ok(JqAnswer {
        values,
        truncated: head.truncated,
        error: head.error.filter(|e| !e.is_empty()),
        elapsed: head.elapsed,
    })
}

/// glint-core's jq worker, `inspect_tabular`, and the archive copy.
pub fn core_data_source() -> DataSource {
    DataSource {
        query: Arc::new(|path, format, filter| {
            Box::pin(async move {
                let answer = glint_core::jq::jq_query(path, format_word(format).to_string(), filter).await?;
                // The values can be many megabytes of JSON: read off the thread that runs core's tasks.
                glint_core::host::async_runtime::spawn_blocking(move || parse_answer(&answer.into_bytes()))
                    .await
                    .map_err(|e| e.to_string())?
            })
        }),
        stop: Arc::new(glint_core::jq::jq_stop),
        table: Arc::new(|path| Box::pin(glint_core::data_lens::inspect_tabular(path, TABLE_BYTES))),
        copy: core_hex_source().copy,
    }
}

// ---- Which preview a file gets -------------------------------------------------------------------------------

/// How a data file is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKind {
    /// JSON, YAML or TOML: the values as a tree.
    Tree(DataFormat),
    /// CSV, TSV and JSON lines: the rows as a table.
    Table,
}

/// A file's way of being shown, by its extension (`dataFormat`, and `TABULAR_EXTS` in DataPreview.tsx).
pub fn data_kind(path: &str) -> Option<DataKind> {
    if let Some(format) = data_format(&entry_of(path)) {
        return Some(DataKind::Tree(format));
    }
    matches!(super::text::ext_of(path).as_str(), "csv" | "tsv" | "jsonl" | "ndjson").then_some(DataKind::Table)
}

// ---- The tree's rows -----------------------------------------------------------------------------------------

/// What kind of leaf a value is, for its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeafKind {
    Null,
    Bool,
    Str,
    Num,
}

/// What a row shows besides its key.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Body {
    /// An object or array: how many it holds, whether it is open, and its one-line preview for when it isn't.
    Container {
        array: bool,
        size: usize,
        open: bool,
        preview: String,
    },
    /// A value that holds nothing: as JSON writes it (strings cut at 400).
    Leaf { text: String, kind: LeafKind },
    /// "N more…": the rest of a big object or array.
    More { count: usize },
}

/// A row's key as shown: an index, or a name (written in quotes when it isn't a plain word).
#[derive(Clone, Debug, PartialEq, Eq)]
enum KeyText {
    Index(usize),
    Name(String),
}

/// One line of the tree, with everything it shows worked out once (the rows are cached between changes).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    id: String,
    path: String,
    root: usize,
    depth: usize,
    /// The child index at each level below the value, to find it again (`value_at`).
    steps: Vec<usize>,
    key: Option<KeyText>,
    body: Body,
}

/// A key as the tree writes it (`keyText`): a plain word as it is, anything else in quotes.
pub fn key_text(key: &str) -> String {
    let mut chars = key.chars();
    let plain = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '-');
    if plain {
        key.to_string()
    } else {
        preview(&JsonValue::String(key.to_string()), usize::MAX)
    }
}

/// The rows to show for `values` with `open` containers open: json_tree's rows, each made ready to draw.
fn build_rows(values: &[JsonValue], open: &HashMap<String, usize>) -> Vec<Row> {
    let mut counters: Vec<usize> = vec![0, 0];
    let mut chain: Vec<usize> = Vec::new();
    tree_rows(values, open)
        .into_iter()
        .map(|row| {
            let steps = if row.more.is_some() {
                chain.iter().take(row.depth.saturating_sub(1)).copied().collect()
            } else if row.depth == 0 {
                chain.clear();
                counters[1] = 0;
                Vec::new()
            } else {
                if counters.len() <= row.depth + 1 {
                    counters.resize(row.depth + 2, 0);
                }
                let index = counters[row.depth];
                counters[row.depth] += 1;
                counters[row.depth + 1] = 0;
                chain.truncate(row.depth - 1);
                chain.push(index);
                chain.clone()
            };
            let key = row.key.map(|key| match key {
                RowKey::Index(i) => KeyText::Index(i),
                RowKey::Name(name) => KeyText::Name(key_text(name)),
            });
            let body = if let Some(count) = row.more {
                Body::More { count }
            } else if is_container(row.value) {
                Body::Container {
                    array: matches!(row.value, JsonValue::Array(_)),
                    size: size_of(row.value),
                    open: row.open,
                    preview: preview(row.value, 80),
                }
            } else {
                let kind = match row.value {
                    JsonValue::Null => LeafKind::Null,
                    JsonValue::Bool(_) => LeafKind::Bool,
                    JsonValue::String(_) => LeafKind::Str,
                    _ => LeafKind::Num,
                };
                Body::Leaf {
                    text: preview(row.value, 400),
                    kind,
                }
            };
            Row {
                id: row.id,
                path: row.path,
                root: row.root,
                depth: row.depth,
                steps,
                key,
                body,
            }
        })
        .collect()
}

/// The value a row stands for, found again from its steps.
fn value_at<'a>(values: &'a [JsonValue], root: usize, steps: &[usize]) -> Option<&'a JsonValue> {
    let mut value = values.get(root)?;
    for &step in steps {
        value = match value {
            JsonValue::Array(items) => items.get(step)?,
            JsonValue::Object(pairs) => &pairs.get(step)?.1,
            _ => return None,
        };
    }
    Some(value)
}

/// A value as `JSON.stringify(value, null, 2)` writes it (numbers as they were written).
pub fn pretty_json(value: &JsonValue) -> String {
    fn write(value: &JsonValue, indent: usize, out: &mut String) {
        match value {
            JsonValue::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&"  ".repeat(indent + 1));
                    write(item, indent + 1, out);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&"  ".repeat(indent));
                out.push(']');
            }
            JsonValue::Object(pairs) if !pairs.is_empty() => {
                out.push_str("{\n");
                for (i, (key, item)) in pairs.iter().enumerate() {
                    out.push_str(&"  ".repeat(indent + 1));
                    out.push_str(&preview(&JsonValue::String(key.clone()), usize::MAX));
                    out.push_str(": ");
                    write(item, indent + 1, out);
                    out.push_str(if i + 1 < pairs.len() { ",\n" } else { "\n" });
                }
                out.push_str(&"  ".repeat(indent));
                out.push('}');
            }
            JsonValue::Array(_) => out.push_str("[]"),
            JsonValue::Object(_) => out.push_str("{}"),
            JsonValue::String(_) => out.push_str(&preview(value, usize::MAX)),
            other => out.push_str(&preview(other, usize::MAX)),
        }
    }
    let mut out = String::new();
    write(value, 0, &mut out);
    out
}

/// Opens `value` and the containers under it, a page of each, as many as the budget allows (`openAll`'s `walk`).
fn walk_open(
    value: &JsonValue,
    id: &str,
    path: &str,
    prefix: &str,
    open: &mut HashMap<String, usize>,
    budget: &mut isize,
) {
    if !is_container(value) {
        return;
    }
    let spent = *budget <= 0;
    *budget -= 1;
    if spent {
        return;
    }
    let shown = open.get(id).copied().unwrap_or(0).max(PAGE);
    open.insert(id.to_string(), shown);
    let children: Vec<(RowKey, &JsonValue)> = match value {
        JsonValue::Array(items) => items
            .iter()
            .take(PAGE)
            .enumerate()
            .map(|(i, v)| (RowKey::Index(i), v))
            .collect(),
        JsonValue::Object(pairs) => pairs
            .iter()
            .take(PAGE)
            .map(|(k, v)| (RowKey::Name(k.as_str()), v))
            .collect(),
        _ => Vec::new(),
    };
    for (key, child) in children {
        let child_path = jq_path(path, key);
        walk_open(
            child,
            &format!("{prefix}{child_path}"),
            &child_path,
            prefix,
            open,
            budget,
        );
    }
}

/// "1.2" or "34": a duration in ms as the status line says it.
fn elapsed_text(ms: f64) -> String {
    if ms < 10. {
        format!("{ms:.1}")
    } else {
        format!("{}", ms.round())
    }
}

// ---- The tree ------------------------------------------------------------------------------------------------

/// What the last query gave.
enum Answer {
    /// Not answered yet.
    Waiting,
    /// The answer.
    Values {
        values: Vec<JsonValue>,
        truncated: bool,
        error: Option<String>,
        elapsed: f64,
    },
    /// The query failed outright (the file is too big or can't be read, the filter wouldn't start, it was stopped).
    Failed(String),
}

/// A JSON, YAML or TOML file as a tree of its values, with a jq filter over it, run by jaq (installed on demand).
pub struct DataTree {
    path: String,
    format: DataFormat,
    source: DataSource,
    /// The filter box.
    typed: Entity<InputState>,
    /// The filter running or run last (`.` is the file as it is).
    filter: String,
    answer: Answer,
    running: bool,
    /// The open containers, by row id, and how many of their children show.
    open: HashMap<String, usize>,
    selected: Option<String>,
    rows: Arc<Vec<Row>>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    height: std::rc::Rc<std::cell::Cell<Pixels>>,
    started: Instant,
    generation: u64,
    _task: Task<()>,
    _typed_events: Subscription,
    /// The tools model the jaq card reads (none without core), and the subscription that redraws with it.
    tools: Option<Entity<Tools>>,
    _tools_watch: Option<Subscription>,
}

impl DataTree {
    /// The tree of the file at `path`, read as `format`.
    pub fn new(path: String, format: DataFormat, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::with_source(path, format, core_data_source(), window, cx);
        if let Some(tools) = tools_model(window, cx) {
            this._tools_watch = Some(cx.observe(&tools, |_, _, cx| cx.notify()));
            this.tools = Some(tools);
        }
        this
    }

    /// Reads the tools the jaq card needs from `tools` (tests pass a model over a fake backend).
    pub fn with_tools(mut self, tools: Entity<Tools>, cx: &mut Context<Self>) -> Self {
        self._tools_watch = Some(cx.observe(&tools, |_, _, cx| cx.notify()));
        self.tools = Some(tools);
        self
    }

    /// The card that offers to install jaq: while a filter is in force and jaq isn't there.
    fn jaq_card(&self, cx: &App) -> Option<AnyElement> {
        let tools = self.tools.as_ref().filter(|_| self.filter != ".")?;
        needed(
            tools,
            "jaq",
            t!("Filters run in jaq, jq rewritten in Rust: a 5 MB program from its GitHub release, checked before it's used."),
            IconName::Braces,
            cx,
        )
    }

    /// [`DataTree::new`] reading through `source` (tests pass fakes).
    pub fn with_source(
        path: String,
        format: DataFormat,
        source: DataSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(".items[] | select(.size > 1000) | .name"));
        let events = cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let text = this.typed.read(cx).value().to_string();
                this.run(&text, window, cx);
            }
        });
        let mut this = Self {
            path,
            format,
            source,
            typed,
            filter: ".".to_string(),
            answer: Answer::Waiting,
            running: false,
            open: HashMap::new(),
            selected: None,
            rows: Arc::new(Vec::new()),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            height: Default::default(),
            started: Instant::now(),
            generation: 0,
            _task: Task::ready(()),
            _typed_events: events,
            tools: None,
            _tools_watch: None,
        };
        this.query(cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file: the filter starts over.
    pub fn set_path(&mut self, path: String, format: DataFormat, window: &mut Window, cx: &mut Context<Self>) {
        self.path = path;
        self.format = format;
        self.filter = ".".to_string();
        self.typed.update(cx, |typed, cx| typed.set_value("", window, cx));
        self.query(cx);
    }

    /// Reads the file again (it changed on disk).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.query(cx);
    }

    /// The values the filter gave.
    fn values(&self) -> &[JsonValue] {
        match &self.answer {
            Answer::Values { values, .. } => values,
            _ => &[],
        }
    }

    /// The selected row's index among the rows.
    fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.rows.iter().position(|row| &row.id == id)
    }

    /// The rows shown now, as text: key and value (tests read this).
    pub fn row_texts(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row_label(row, self.values().len() > 1))
            .collect()
    }

    /// Runs the query for the current filter (the effect that `useEffect` was).
    fn query(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.running = true;
        self.started = Instant::now();
        redraw_after_delay(cx);
        let (path, format, filter) = (self.path.clone(), self.format, self.filter.clone());
        let source = self.source.clone();
        let archived = is_archived_path(&self.path);
        self._task = cx.spawn(async move |this, cx| {
            let result = core_bridge::run(async move {
                let path = if archived { (source.copy)(path).await? } else { path };
                (source.query)(path, format, filter).await
            })
            .await
            .and_then(|inner| inner);
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(answer) => {
                        this.open = initially_open(&answer.values).into_iter().collect();
                        this.selected = None;
                        this.answer = Answer::Values {
                            values: answer.values,
                            truncated: answer.truncated,
                            error: answer.error,
                            elapsed: answer.elapsed,
                        };
                    }
                    Err(reason) => {
                        this.answer = Answer::Failed(if reason == "stopped" {
                            t!("Stopped.").to_string()
                        } else {
                            reason
                        });
                    }
                }
                this.running = false;
                this.rebuild();
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// Scrolls the rows a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let moved = page_list(&self.scroll, down);
        if moved {
            cx.notify();
        }
        moved
    }

    /// Stops the query running (`jqStop`).
    pub fn stop(&mut self) {
        (self.source.stop)();
    }

    /// Runs a filter (`run`): the text, or `.` when it is empty.
    pub fn run(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.typed
            .update(cx, |typed, cx| typed.set_value(text.clone(), window, cx));
        let trimmed = text.trim();
        self.filter = if trimmed.is_empty() {
            ".".to_string()
        } else {
            trimmed.to_string()
        };
        self.query(cx);
    }

    /// Works the rows out again after the values or the open containers changed.
    fn rebuild(&mut self) {
        let rows = build_rows(self.values(), &self.open);
        self.rows = Arc::new(rows);
    }

    /// Opens or closes a container (`toggle`).
    pub fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.open.remove(id).is_none() {
            self.open.insert(id.to_string(), PAGE);
        }
        self.rebuild();
        cx.notify();
    }

    /// Opens a container and what is under it (`openAll`).
    pub fn open_all(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|row| row.id == id).cloned() else {
            return;
        };
        let prefix = &row.id[..row.id.find(':').map_or(0, |at| at + 1)];
        let mut open = std::mem::take(&mut self.open);
        if let Some(value) = value_at(self.values(), row.root, &row.steps) {
            let mut budget = OPEN_ALL;
            walk_open(value, &row.id, &row.path, prefix, &mut open, &mut budget);
        }
        self.open = open;
        self.rebuild();
        cx.notify();
    }

    /// Shows the next page of a big container (`more`); `id` is the "more" row's.
    pub fn more(&mut self, id: &str, cx: &mut Context<Self>) {
        let parent = id.trim_end_matches("more").trim_end_matches('\u{0}').to_string();
        let shown = self.open.get(&parent).copied().unwrap_or(PAGE) + PAGE;
        self.open.insert(parent, shown);
        self.rebuild();
        cx.notify();
    }

    /// Selects the row at `index` (kept inside the rows) and scrolls to it (`select`).
    pub fn select(&mut self, index: isize, cx: &mut Context<Self>) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        let index = index.clamp(0, last as isize) as usize;
        self.selected = Some(self.rows[index].id.clone());
        self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Clicking a row selects it and opens or closes it; Alt opens everything under it.
    fn click_row(&mut self, id: &str, alt: bool, cx: &mut Context<Self>) {
        self.selected = Some(id.to_string());
        let Some(row) = self.rows.iter().find(|row| row.id == id) else {
            return;
        };
        match row.body {
            Body::Container { open, .. } => {
                if alt && !open {
                    self.open_all(id, cx);
                } else {
                    self.toggle(id, cx);
                }
            }
            Body::More { .. } => self.more(id, cx),
            Body::Leaf { .. } => cx.notify(),
        }
    }

    /// Copies text, saying so.
    fn copy(text: String, what: SharedString, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(Notification::success(t!("Copied the {what}", what = what)), cx);
    }

    /// The selected row's value as JSON, for Copy.
    fn selected_json(&self) -> Option<String> {
        let row = self.rows.iter().find(|row| Some(&row.id) == self.selected.as_ref())?;
        if matches!(row.body, Body::More { .. }) {
            return None;
        }
        value_at(self.values(), row.root, &row.steps).map(pretty_json)
    }

    /// The arrows, Home, End, Page, Enter and Ctrl+C (`onKey`).
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.selected_index().map_or(-1, |i| i as isize);
        let rows = self.rows.clone();
        let row = usize::try_from(at).ok().and_then(|i| rows.get(i));
        let key = event.keystroke.key.as_str();
        let ctrl = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        let page =
            ((f32::from(self.height.get()) / (ROW.0 * f32::from(theme::rem_size(cx)))).floor() as isize - 1).max(1);
        match key {
            "down" => self.select(at + 1, cx),
            "up" => self.select(at - 1, cx),
            "home" => self.select(0, cx),
            "end" => self.select(rows.len() as isize - 1, cx),
            "pagedown" => self.select(at + page, cx),
            "pageup" => self.select(at - page, cx),
            "right" => match row.map(|r| (&r.id, &r.body)) {
                Some((id, Body::Container { open, .. })) => {
                    if *open {
                        self.select(at + 1, cx);
                    } else {
                        let id = id.clone();
                        self.toggle(&id, cx);
                    }
                }
                _ => return,
            },
            "left" => {
                let Some(row) = row else { return };
                if matches!(row.body, Body::Container { open: true, .. }) {
                    let id = row.id.clone();
                    self.toggle(&id, cx);
                } else if let Some(parent) = rows[..at as usize].iter().rposition(|r| r.depth < row.depth) {
                    self.select(parent as isize, cx);
                }
            }
            "enter" | "space" => {
                let Some(row) = row else { return };
                let id = row.id.clone();
                match row.body {
                    Body::More { .. } => self.more(&id, cx),
                    Body::Container { .. } => self.toggle(&id, cx),
                    Body::Leaf { .. } => return,
                }
            }
            "c" if ctrl => {
                if let Some(json) = self.selected_json() {
                    Self::copy(json, t!("value"), window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The filter bar: `jq`, the box, and Run or Stop.
    fn filter_bar(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let action = if self.running {
            Button::new("json-stop")
                .ghost()
                .small()
                .icon(IconName::Square)
                .label(t!("Stop"))
                .on_click(cx.listener(|this, _, _, _| this.stop()))
        } else {
            Button::new("json-run")
                .ghost()
                .small()
                .label(t!("Run"))
                .tooltip(t!("Run the filter (Enter); Esc clears it"))
                .on_click(cx.listener(|this, _, window, cx| {
                    let text = this.typed.read(cx).value().to_string();
                    this.run(&text, window, cx);
                }))
        };
        row_start()
            .flex_none()
            .items_center()
            .gap(space(1.5))
            .px(space(2.))
            .py(space(1.5))
            .border_b_1()
            .border_color(t.border)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let typed = this.typed.read(cx).value().to_string();
                if event.keystroke.key == "escape" && (!typed.is_empty() || this.filter != ".") {
                    this.run("", window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("json-jq")
                    .ps(1.)
                    .font_family(theme::mono_font(cx))
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted_foreground)
                    .tooltip(|window, cx| tooltip(t!("A jq filter, run by jaq"), window, cx))
                    .child("jq"),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.typed)
                        .small()
                        .aria_label(t!("jq filter"))
                        .font_family(theme::mono_font(cx)),
                ),
            )
            .child(action)
    }

    /// The footer: the selected value's path and its buttons, or how the query went.
    fn footer(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let many = self.values().len() > 1;
        let selected = self
            .rows
            .iter()
            .find(|row| Some(&row.id) == self.selected.as_ref())
            .filter(|row| !matches!(row.body, Body::More { .. }))
            .cloned();
        let bar = row_start()
            .h(space(8.))
            .flex_none()
            .items_center()
            .gap(space(1.))
            .px(space(2.))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground);
        match selected {
            Some(row) => {
                let path_text = format!(
                    "{}{}",
                    if many {
                        format!("#{} ", row.root + 1)
                    } else {
                        String::new()
                    },
                    row.path
                );
                let path = row.path.clone();
                let json = self.selected_json().unwrap_or_default();
                let filterable = !many && row.path != ".";
                bar.child(
                    div()
                        .id("json-path")
                        .role(Role::Status)
                        .aria_label(path_text.clone())
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::mono_font(cx))
                        .text_color(t.foreground)
                        .child(path_text),
                )
                .child({
                    let path = path.clone();
                    Button::new("json-copy-path")
                        .ghost()
                        .compact()
                        .icon(IconName::ClipboardCopy)
                        .label(t!("Path"))
                        .tooltip(t!("Copy its jq path"))
                        .on_click(move |_, window, cx| Self::copy(path.clone(), t!("path"), window, cx))
                })
                .child(
                    Button::new("json-copy-value")
                        .ghost()
                        .compact()
                        .icon(IconName::ClipboardCopy)
                        .label(t!("Value"))
                        .tooltip(t!("Copy it as JSON (Ctrl+C)"))
                        .on_click(move |_, window, cx| Self::copy(json.clone(), t!("value"), window, cx)),
                )
                .when(filterable, |this| {
                    this.child(
                        Button::new("json-filter-to")
                            .ghost()
                            .compact()
                            .icon(IconName::Funnel)
                            .label(t!("Filter"))
                            .tooltip(t!("Filter to it"))
                            .on_click(cx.listener(move |this, _, window, cx| this.run(&path, window, cx))),
                    )
                })
                .into_any_element()
            }
            None => {
                let text: SharedString = match &self.answer {
                    Answer::Values {
                        values,
                        truncated,
                        elapsed,
                        ..
                    } => {
                        let count = values.len();
                        let what = if count == 1 { t!("value") } else { t!("values") };
                        let more = if *truncated {
                            t!(" (stopped there: there were more)")
                        } else {
                            SharedString::default()
                        };
                        let how = if self.filter == "." { t!("read") } else { t!("jaq") };
                        t!(
                            "{count} {what}{more} · {how} in {ms} ms",
                            count = format_count(count),
                            what = what,
                            more = more,
                            how = how,
                            ms = elapsed_text(*elapsed)
                        )
                    }
                    _ if self.running => t!("Running…"),
                    _ => SharedString::default(),
                };
                bar.child(
                    div()
                        .id("json-status")
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .test_support()
                        .truncate()
                        .child(text),
                )
                .into_any_element()
            }
        }
    }

    /// One row of the list.
    fn tree_row(
        &self,
        index: usize,
        row: &Row,
        many: bool,
        t: &theme::Tokens,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = Some(&row.id) == self.selected.as_ref();
        let indent = 1.5 + row.depth as f32 * 4.;
        let id = row.id.clone();
        let click_id = id.clone();
        let base = row_start()
            .id(("json-row", index))
            .role(Role::TreeItem)
            .aria_label(row_label(row, many))
            .aria_selected(selected)
            .test_support()
            .h(ROW)
            .w_full()
            .ps(indent)
            .pe(4.)
            .gap(space(1.))
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden()
            .font_family(mono.clone())
            .text_size(rems(0.8125))
            .when(selected, |this| this.bg(t.accent))
            .when(!selected, |this| this.hover(|this| this.bg(t.accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.click_row(&click_id, event.modifiers.alt, cx);
                }),
            );
        if let Body::More { count } = row.body {
            let text = t!(
                "{count} more… (show {next})",
                count = format_count(count),
                next = format_count(count.min(PAGE))
            );
            return base
                .aria_label(text.clone())
                .text_color(t.primary)
                .child(div().w(space(3.5)).flex_none())
                .child(text)
                .into_any_element();
        }
        let muted = t.muted_foreground;
        let chevron = match &row.body {
            Body::Container { open, .. } => icon(if *open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .size_3p5()
            .flex_none()
            .text_color(muted)
            .into_any_element(),
            _ => div().w(space(3.5)).flex_none().into_any_element(),
        };
        let key = match &row.key {
            Some(KeyText::Index(i)) => Some(
                div()
                    .flex_none()
                    .text_color(muted)
                    .child(format!("{i}: "))
                    .into_any_element(),
            ),
            Some(KeyText::Name(name)) => Some(
                row_start()
                    .flex_none()
                    .child(div().text_color(t.tk_function).child(name.clone()))
                    .child(div().text_color(muted).child(": "))
                    .into_any_element(),
            ),
            None if many => Some(
                div()
                    .flex_none()
                    .text_color(muted)
                    .child(format!("#{} ", row.root + 1))
                    .into_any_element(),
            ),
            None => None,
        };
        let value = match &row.body {
            Body::Container {
                array,
                size,
                open,
                preview,
            } => {
                let text = if *open {
                    let (a, b) = if *array { ('[', ']') } else { ('{', '}') };
                    format!("{a}{}{b}", format_count(*size))
                } else {
                    preview.clone()
                };
                div().text_color(muted).child(text).into_any_element()
            }
            Body::Leaf { text, kind } => match kind {
                LeafKind::Null => div().italic().text_color(muted).child("null").into_any_element(),
                LeafKind::Str => div().text_color(t.tk_string).child(text.clone()).into_any_element(),
                LeafKind::Bool => div().text_color(t.tk_keyword).child(text.clone()).into_any_element(),
                LeafKind::Num => div().text_color(t.tk_number).child(text.clone()).into_any_element(),
            },
            Body::More { .. } => div().into_any_element(),
        };
        let base = if let Body::Container { open, .. } = row.body {
            base.aria_expanded(open)
        } else {
            base
        };
        base.child(chevron).children(key).child(value).into_any_element()
    }
}

/// What a row says, for a screen reader and for tests: "key: value", or "key: {3}" for a container.
fn row_label(row: &Row, many: bool) -> String {
    if let Body::More { count } = row.body {
        return t!(
            "{count} more… (show {next})",
            count = format_count(count),
            next = format_count(count.min(PAGE))
        )
        .to_string();
    }
    let key = match &row.key {
        Some(KeyText::Index(i)) => format!("{i}: "),
        Some(KeyText::Name(name)) => format!("{name}: "),
        None if many => format!("#{} ", row.root + 1),
        None => String::new(),
    };
    let value = match &row.body {
        Body::Container {
            array,
            size,
            open,
            preview,
        } => {
            if *open {
                let (a, b) = if *array { ('[', ']') } else { ('{', '}') };
                format!("{a}{}{b}", format_count(*size))
            } else {
                preview.clone()
            }
        }
        Body::Leaf { text, .. } => text.clone(),
        Body::More { .. } => String::new(),
    };
    format!("{key}{value}")
}

impl Render for DataTree {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let mono = theme::mono_font(cx);
        let root = col()
            .id("data-tree")
            .size_full()
            .min_h_0()
            .text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let bar = self.filter_bar(&t, cx);
        let error = match &self.answer {
            Answer::Values {
                error: Some(error),
                values,
                ..
            } => Some((error.clone(), !values.is_empty())),
            Answer::Failed(error) => Some((error.clone(), false)),
            _ => None,
        };
        let error = error.map(|(text, with_values)| {
            div()
                .id("json-error")
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .test_support()
                .max_h(space(40.))
                .flex_none()
                .overflow_hidden()
                .px(space(3.))
                .py(space(2.))
                .border_b_1()
                .border_color(t.border)
                .when(with_values, |this| this.bg(t.destructive.opacity(0.05)))
                .font_family(mono.clone())
                .text_size(theme::size::XS)
                .text_color(t.destructive)
                .child(SharedString::from(text))
        });
        let body = if matches!(self.answer, Answer::Waiting) {
            waiting(self.started, None, cx)
        } else {
            let many = self.values().len() > 1;
            let rows = self.rows.clone();
            let count = rows.len();
            let tokens = t.clone();
            let mono = mono.clone();
            let height = self.height.clone();
            let measure = canvas(move |bounds, _, _| height.set(bounds.size.height), |_, _, _, _| {})
                .absolute()
                .size_full();
            let list = uniform_list(
                "json-rows",
                count,
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range
                        .filter_map(|i| rows.get(i).map(|row| (i, row.clone())))
                        .map(|(i, row)| this.tree_row(i, &row, many, &tokens, &mono, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full();
            div()
                .id("json-values")
                .role(Role::Tree)
                .aria_label(t!("Values"))
                .test_support()
                .track_focus(&self.focus)
                .tab_stop(true)
                .on_key_down(cx.listener(Self::on_key_down))
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .py(space(1.))
                .child(measure)
                .child(list)
                .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
                .into_any_element()
        };
        let card = self.jaq_card(cx);
        root.child(bar)
            .children(card)
            .children(error)
            .child(body)
            .child(self.footer(&t, cx))
    }
}

impl Focusable for DataTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ---- The table -----------------------------------------------------------------------------------------------

/// `Number(text)`: the number a cell reads as, with `Number("")` being zero as in JavaScript.
fn js_number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(0.);
    }
    trimmed.parse::<f64>().ok().filter(|n| !n.is_nan())
}

/// The rows of a table to show, as indices: the ones with `query` in a cell (any case), sorted by a column when
/// one is chosen (`rows` in `TableView`): numbers by value in a column of numbers, anything else in the user's
/// language with digits read as numbers.
pub fn shown_rows(table: &TablePreview, query: &str, sort: Option<(usize, bool)>) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<usize> = (0..table.rows.len())
        .filter(|&i| q.is_empty() || table.rows[i].iter().any(|cell| cell.to_lowercase().contains(&q)))
        .collect();
    if let Some((column, descending)) = sort {
        let numeric = table.columns.get(column).is_some_and(|c| c.kind == "number");
        let cell = |row: usize| table.rows[row].get(column).map_or("", String::as_str);
        rows.sort_by(|&a, &b| {
            let ordering = match (numeric, js_number(cell(a)), js_number(cell(b))) {
                (true, Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                _ => glint_model::collate::compare_base_numeric(cell(a), cell(b)),
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }
    rows
}

/// The grid's columns and rows for the kit's `DataTable`.
struct Grid {
    table: Arc<TablePreview>,
    /// The rows shown, as indices into the table's.
    order: Vec<usize>,
    query: String,
    /// The sorted column (the table's, without the # column) and whether it is descending.
    sort: Option<(usize, bool)>,
    widths: Vec<Pixels>,
}

impl Grid {
    fn new(table: Arc<TablePreview>, rem: Pixels) -> Self {
        // A column is as wide as its widest cell in the first rows (and its name and icon), between 3 and 16 rems.
        let widths = table
            .columns
            .iter()
            .enumerate()
            .map(|(i, column)| {
                let widest = table
                    .rows
                    .iter()
                    .take(100)
                    .map(|row| row.get(i).map_or(0, |cell| cell.chars().count()))
                    .max()
                    .unwrap_or(0)
                    .max(column.name.chars().count() + 4)
                    .min(40);
                rem * (widest as f32 * 0.42 + 1.75).clamp(3., 16.)
            })
            .collect();
        let order = shown_rows(&table, "", None);
        Self {
            table,
            order,
            query: String::new(),
            sort: None,
            widths,
        }
    }

    fn cell(&self, row: usize, column: usize) -> &str {
        self.order
            .get(row)
            .and_then(|&i| self.table.rows.get(i))
            .and_then(|cells| cells.get(column))
            .map_or("", String::as_str)
    }
}

impl TableDelegate for Grid {
    fn columns_count(&self, _: &App) -> usize {
        self.table.columns.len() + 1
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, ix: usize, _: &App) -> Column {
        if ix == 0 {
            return Column::new("row", "#")
                .width(px(56.))
                .text_right()
                .resizable(false)
                .movable(false);
        }
        let column = &self.table.columns[ix - 1];
        Column::new(format!("column-{ix}"), column.name.clone())
            .width(self.widths.get(ix - 1).copied().unwrap_or(px(120.)))
            .movable(false)
            .sortable()
            .when(column.kind == "number", |c| c.text_right())
    }

    fn perform_sort(
        &mut self,
        ix: usize,
        sort: gpui_kit::component::table::ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        use gpui_kit::component::table::ColumnSort;
        let sort = match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some((ix - 1, false)),
            ColumnSort::Descending => Some((ix - 1, true)),
        };
        self.sort = sort;
        self.order = shown_rows(&self.table, &self.query, sort);
    }

    fn render_th(&mut self, ix: usize, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        if ix == 0 {
            return div()
                .w_full()
                .text_color(t.muted_foreground)
                .child("#")
                .into_any_element();
        }
        let column = &self.table.columns[ix - 1];
        let (glyph, tone) = match column.kind.as_str() {
            "number" => (IconName::Hash, theme::tailwind("amber-500")),
            "boolean" => (IconName::ToggleLeft, theme::tailwind("emerald-500")),
            _ => (IconName::Type, theme::tailwind("sky-500")),
        };
        row_start()
            .w_full()
            .min_w_0()
            .gap(space(1.5))
            .child(
                icon(glyph)
                    .size_3()
                    .flex_none()
                    .text_color(tone.unwrap_or(t.muted_foreground)),
            )
            .child(
                div()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(column.name.clone()),
            )
            .into_any_element()
    }

    fn render_td(
        &mut self,
        row: usize,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        if ix == 0 {
            return div()
                .w_full()
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child((row + 1).to_string())
                .into_any_element();
        }
        let value = self.cell(row, ix - 1).to_string();
        let kind = self.table.columns[ix - 1].kind.as_str();
        let tone = match kind {
            "number" => Some(theme::tailwind(if t.dark { "amber-400" } else { "amber-700" })),
            "boolean" => Some(theme::tailwind(if t.dark { "emerald-400" } else { "emerald-600" })),
            _ => None,
        };
        div()
            .w_full()
            .truncate()
            .font_family(theme::mono_font(cx))
            .text_size(theme::size::XS)
            .when_some(tone.flatten(), |this, color| this.text_color(color))
            .child(value)
            .into_any_element()
    }

    fn cell_label(&self, row: usize, ix: usize, _: &App) -> Option<SharedString> {
        if ix == 0 {
            return Some((row + 1).to_string().into());
        }
        Some(self.cell(row, ix - 1).to_string().into())
    }

    fn render_empty(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let t = theme::tokens(cx);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme::size::SM)
            .text_color(t.muted_foreground)
            .child(t!("No rows match."))
    }
}

/// What the table shows.
enum TableLoad {
    Waiting,
    Failed(SharedString),
    Ready {
        table: Arc<TablePreview>,
        grid: Entity<TableState<Grid>>,
    },
}

/// A table's rows, filtered and sorted by a click on a column, with the file's text on offer (`TableView` and
/// `TabularPreview`).
pub struct TableViewer {
    path: String,
    source: DataSource,
    load: TableLoad,
    filter: Entity<InputState>,
    /// The file's text in place of the grid (`mode` "raw"), made the first time it is asked for.
    raw: Option<Entity<TextPreview>>,
    showing_raw: bool,
    started: Instant,
    generation: u64,
    _task: Task<()>,
    _filter_events: Subscription,
}

impl TableViewer {
    /// The table of the delimited file at `path`.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_data_source(), window, cx)
    }

    /// [`TableViewer::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: DataSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter rows…")));
        let events = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_changed(cx);
            }
        });
        let mut this = Self {
            path,
            source,
            load: TableLoad::Waiting,
            filter,
            raw: None,
            showing_raw: false,
            started: Instant::now(),
            generation: 0,
            _task: Task::ready(()),
            _filter_events: events,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.raw = None;
            self.showing_raw = false;
            self.read(window, cx);
        }
    }

    /// The rows the grid shows now, as text.
    pub fn shown_cells(&self, cx: &App) -> Vec<Vec<String>> {
        match &self.load {
            TableLoad::Ready { grid, table } => {
                let delegate = grid.read(cx).delegate();
                delegate.order.iter().map(|&i| table.rows[i].clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = TableLoad::Waiting;
        self.started = Instant::now();
        let read = (self.source.table)(self.path.clone());
        self._task = cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.load = match result {
                    Err(reason) => TableLoad::Failed(reason.into()),
                    Ok(table) => {
                        let table = Arc::new(table);
                        let rem = theme::rem_size(cx);
                        let grid_table = table.clone();
                        let grid = cx.new(|cx| {
                            TableState::new(Grid::new(grid_table, rem), window, cx)
                                .col_movable(false)
                                .row_selectable(true)
                        });
                        TableLoad::Ready { table, grid }
                    }
                };
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The filter box changed: the rows follow (the sort is kept by the grid when the filter is cleared).
    fn filter_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().to_string();
        if let TableLoad::Ready { grid, .. } = &self.load {
            grid.update(cx, |grid, cx| {
                let delegate = grid.delegate_mut();
                delegate.query = query.clone();
                delegate.order = shown_rows(&delegate.table, &query, delegate.sort);
                grid.refresh(cx);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Scrolls the rows a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if self.showing_raw {
            return match &self.raw {
                Some(raw) => raw.update(cx, |raw, cx| raw.page(down, cx)),
                None => false,
            };
        }
        let TableLoad::Ready { grid, .. } = &self.load else {
            return false;
        };
        grid.update(cx, |grid, cx| {
            let rows = grid.delegate().order.len();
            let visible = grid.visible_range().rows().clone();
            let page = visible.len().max(2) - 1;
            let target = if down {
                (visible.end.saturating_sub(1) + page).min(rows.saturating_sub(1))
            } else {
                visible.start.saturating_sub(page)
            };
            if rows == 0 || visible.contains(&target) && visible.len() >= rows {
                return false;
            }
            grid.scroll_to_row(target, cx);
            true
        })
    }

    /// Switches between the grid and the file's text (`mode`).
    pub fn toggle_raw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.showing_raw = !self.showing_raw;
        if self.showing_raw && self.raw.is_none() {
            let path = self.path.clone();
            self.raw = Some(cx.new(|cx| TextPreview::new(path, window, cx)));
        }
        cx.notify();
    }

    /// The header: the format, the counts, how long it took, the filter and the Raw button.
    fn header(
        &self,
        table: &TablePreview,
        shown: usize,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = !self.filter.read(cx).value().trim().is_empty();
        let counts = {
            let total = format_count(table.total_rows_sampled);
            let plus = if table.truncated { "+" } else { "" };
            let cols = format_count(table.columns.len());
            if query {
                t!(
                    "{cols} cols · {shown} of {total}{plus} rows",
                    cols = cols,
                    shown = format_count(shown),
                    total = total,
                    plus = plus
                )
            } else {
                t!(
                    "{cols} cols · {total}{plus} rows",
                    cols = cols,
                    total = total,
                    plus = plus
                )
            }
        };
        let green = theme::tailwind(if t.dark { "emerald-400" } else { "emerald-600" }).unwrap_or(t.primary);
        let raw_button = Button::new("table-raw")
            .outline()
            .small()
            .icon(if self.showing_raw {
                IconName::Table2
            } else {
                IconName::FileCode
            })
            .label(if self.showing_raw { t!("Table") } else { t!("Raw") })
            .tooltip(if self.showing_raw {
                t!("Switch to structured table grid")
            } else {
                t!("Switch to raw text view")
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_raw(window, cx)));
        row_start()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(space(2.))
            .px(space(3.))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.2))
            .text_size(theme::size::XS)
            .child(
                row_start()
                    .id("table-format")
                    .role(Role::Group)
                    .aria_label(SharedString::from(table.format.clone()))
                    .test_support()
                    .gap(space(1.))
                    .px(space(2.))
                    .py(space(0.5))
                    .rounded(theme::size::RADIUS)
                    .bg(green.opacity(0.15))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(green)
                    .child(icon(IconName::Table2).size_3())
                    .child(SharedString::from(table.format.clone())),
            )
            .child(
                div()
                    .id("table-counts")
                    .role(Role::Status)
                    .aria_label(counts.clone())
                    .test_support()
                    .text_color(t.muted_foreground)
                    .child(counts),
            )
            .child(
                div()
                    .font_family(theme::mono_font(cx))
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(format!("{:.2} ms", table.elapsed_ms)),
            )
            .child(
                align_row(1.)
                    .ms(0.)
                    .flex_1()
                    .justify_end()
                    .when(!self.showing_raw, |this| {
                        this.child(
                            div().w(space(40.)).child(
                                Input::new(&self.filter)
                                    .small()
                                    .aria_label(t!("Filter rows…"))
                                    .prefix(icon(IconName::Search).size_3().text_color(t.muted_foreground)),
                            ),
                        )
                    })
                    .child(raw_button),
            )
    }
}

impl Render for TableViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("table-viewer")
            .size_full()
            .min_h_0()
            .text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let (table, grid) = match &self.load {
            TableLoad::Waiting => {
                return root.child(waiting(self.started, Some(t!("Reading the table…")), cx));
            }
            TableLoad::Failed(reason) => return root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            TableLoad::Ready { table, grid } => (table.clone(), grid.clone()),
        };
        let shown = grid.read(cx).delegate().order.len();
        let body = match (&self.raw, self.showing_raw) {
            (Some(raw), true) => div().flex_1().min_h_0().child(raw.clone()).into_any_element(),
            _ if table.rows.is_empty() => note_centered("table-empty", t!("This file is empty."), cx),
            _ => div()
                .flex_1()
                .min_h_0()
                .child(
                    DataTable::new(&grid)
                        .bordered(false)
                        .accessibility_label(name_of(&self.path)),
                )
                .into_any_element(),
        };
        let note = (table.truncated && !self.showing_raw).then(|| {
            let text = t!(
                "Showing the first {count} rows sampled from the file header.",
                count = format_count(table.rows.len())
            );
            div()
                .id("table-truncated")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .flex_none()
                .px(space(3.))
                .py(space(2.))
                .border_t_1()
                .border_color(t.border)
                .bg(t.muted.opacity(0.1))
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(text)
        });
        root.child(self.header(&table, shown, &t, cx))
            .child(body)
            .children(note)
    }
}

// ---- The previewer the frame builds ---------------------------------------------------------------------------

/// What the previewer holds.
enum Shown {
    Tree(Entity<DataTree>),
    Table(Entity<TableViewer>),
    /// Not a file this shows.
    Neither,
}

/// JSON, YAML and TOML as a tree; CSV and TSV as a table.
pub struct DataPreview {
    path: String,
    source: DataSource,
    shown: Shown,
}

impl DataPreview {
    /// The previewer for the file at `path`. Reading happens in the background; the view shows its loading state
    /// until then.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_data_source(), window, cx)
    }

    /// [`DataPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: DataSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shown = Self::make(&path, &source, window, cx);
        Self { path, source, shown }
    }

    /// The view for a file: a tree, a table, or neither.
    fn make(path: &str, source: &DataSource, window: &mut Window, cx: &mut Context<Self>) -> Shown {
        match data_kind(path) {
            Some(DataKind::Tree(format)) => {
                let (path, source) = (path.to_string(), source.clone());
                Shown::Tree(cx.new(|cx| DataTree::with_source(path, format, source, window, cx)))
            }
            Some(DataKind::Table) => {
                let (path, source) = (path.to_string(), source.clone());
                Shown::Table(cx.new(|cx| TableViewer::with_source(path, source, window, cx)))
            }
            None => Shown::Neither,
        }
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Scrolls the preview a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        match &self.shown {
            Shown::Tree(tree) => tree.update(cx, |tree, cx| tree.page(down, cx)),
            Shown::Table(table) => table.update(cx, |table, cx| table.page(down, cx)),
            Shown::Neither => false,
        }
    }

    /// How the file is shown.
    pub fn kind(&self) -> Option<DataKind> {
        data_kind(&self.path)
    }

    /// Shows another file: the same kind of preview is told, another kind is made.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path == path {
            return;
        }
        match (&self.shown, data_kind(&path)) {
            (Shown::Tree(tree), Some(DataKind::Tree(format))) => {
                let path = path.clone();
                tree.update(cx, |tree, cx| tree.set_path(path, format, window, cx));
            }
            (Shown::Table(table), Some(DataKind::Table)) => {
                let path = path.clone();
                table.update(cx, |table, cx| table.set_path(path, window, cx));
            }
            _ => self.shown = Self::make(&path, &self.source, window, cx),
        }
        self.path = path;
        cx.notify();
    }
}

impl Render for DataPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().id("data-preview").size_full().min_h_0();
        match &self.shown {
            Shown::Tree(tree) => root.child(tree.clone()),
            Shown::Table(table) => root.child(table.clone()),
            Shown::Neither => root.child(big_icon(
                &self.path,
                Some(t!("This isn't a data file Glint shows as a tree or a table")),
                cx,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::data_lens::TableColumn;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    fn object(pairs: Vec<(&str, JsonValue)>) -> JsonValue {
        JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn text(s: &str) -> JsonValue {
        JsonValue::String(s.to_string())
    }

    fn sample() -> Vec<JsonValue> {
        vec![object(vec![
            ("name", text("glint")),
            (
                "tags",
                JsonValue::Array(vec![text("fast"), JsonValue::Number(2.), JsonValue::Null]),
            ),
            ("empty", JsonValue::Array(vec![])),
            ("ok", JsonValue::Bool(true)),
        ])]
    }

    /// A source answering queries with `values`, remembering the filters it was asked for and how often it was
    /// stopped.
    #[allow(clippy::type_complexity)] // a test helper returning the fake and its two logs
    fn fake(
        answer: Result<Vec<JsonValue>, String>,
        table: Result<TablePreview, String>,
    ) -> (DataSource, Arc<Mutex<Vec<(String, String)>>>, Arc<AtomicUsize>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicUsize::new(0));
        let (log, stops) = (asked.clone(), stopped.clone());
        let source = DataSource {
            query: Arc::new(move |path, _, filter| {
                if let Ok(mut log) = log.lock() {
                    log.push((path, filter));
                }
                let answer = answer.clone().map(|values| JqAnswer {
                    values,
                    truncated: false,
                    error: None,
                    elapsed: 1.25,
                });
                Box::pin(async move { answer })
            }),
            stop: Arc::new(move || {
                stops.fetch_add(1, Ordering::SeqCst);
            }),
            table: Arc::new(move |_| {
                let table = table.clone();
                Box::pin(async move { table })
            }),
            copy: Arc::new(|path| {
                Box::pin(async move { Ok(format!("C:\\temp\\copy-of-{}", glint_model::paths::base_name(&path))) })
            }),
        };
        (source, asked, stopped)
    }

    fn no_table() -> Result<TablePreview, String> {
        Err("not a table".to_string())
    }

    fn table_of(columns: &[(&str, &str)], rows: &[&[&str]]) -> TablePreview {
        TablePreview {
            format: "CSV".into(),
            delimiter: ",".into(),
            columns: columns
                .iter()
                .map(|(name, kind)| TableColumn {
                    name: (*name).into(),
                    kind: (*kind).into(),
                })
                .collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| (*c).to_string()).collect())
                .collect(),
            total_rows_sampled: rows.len(),
            truncated: false,
            elapsed_ms: 0.5,
        }
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the data never arrived");
    }

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..3 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    fn open_tree(
        cx: &mut TestAppContext,
        path: &str,
        format: DataFormat,
        source: DataSource,
    ) -> (Entity<DataTree>, VisualTestContext) {
        cx.update(testing::init);
        let path = path.to_string();
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| DataTree::with_source(path, format, source, window, cx))
        });
        (view, visual)
    }

    fn ready_tree(view: &Entity<DataTree>, visual: &mut VisualTestContext) {
        settle(visual, |cx| {
            view.read_with(cx, |v, _| !matches!(v.answer, Answer::Waiting))
        });
        frames(visual);
    }

    fn rows_of(view: &Entity<DataTree>, visual: &VisualTestContext) -> Vec<String> {
        view.read_with(visual, |v, _| v.row_texts())
    }

    #[test]
    fn files_get_a_tree_or_a_table_by_their_extension() {
        assert_eq!(data_kind("C:\\a\\x.json"), Some(DataKind::Tree(DataFormat::Json)));
        assert_eq!(data_kind("C:\\a\\x.geojson"), Some(DataKind::Tree(DataFormat::Json)));
        assert_eq!(data_kind("C:\\a\\x.YML"), Some(DataKind::Tree(DataFormat::Yaml)));
        assert_eq!(data_kind("C:\\a\\x.toml"), Some(DataKind::Tree(DataFormat::Toml)));
        assert_eq!(data_kind("C:\\a\\x.csv"), Some(DataKind::Table));
        assert_eq!(data_kind("C:\\a\\x.tsv"), Some(DataKind::Table));
        assert_eq!(data_kind("C:\\a\\x.ndjson"), Some(DataKind::Table));
        assert_eq!(data_kind("C:\\a\\x.jsonl"), Some(DataKind::Table));
        assert_eq!(data_kind("C:\\a\\x.txt"), None);
        assert_eq!(data_kind("C:\\a\\json"), None);
    }

    #[test]
    fn an_answer_is_a_header_line_then_the_values_with_their_numbers_as_written() {
        let bytes = b"{\"truncated\":true,\"error\":null,\"elapsed\":12.5}\n[{\"big\":123456789012345678901234567890,\"price\":1.50},[1,2],\"x\"]";
        let answer = parse_answer(bytes).expect("a good answer");
        assert!(answer.truncated);
        assert_eq!(answer.error, None);
        assert_eq!(answer.elapsed, 12.5);
        assert_eq!(answer.values.len(), 3);
        let JsonValue::Object(pairs) = &answer.values[0] else {
            panic!("an object")
        };
        assert_eq!(
            pairs[0].1,
            JsonValue::Big(glint_model::json_tree::JsonNumber::new(
                "123456789012345678901234567890"
            ))
        );
        assert_eq!(
            pairs[1].1,
            JsonValue::Big(glint_model::json_tree::JsonNumber::new("1.50"))
        );
        // An error with the values before it.
        let with_error =
            parse_answer(b"{\"truncated\":false,\"error\":\"cannot index\",\"elapsed\":1}\n[1]").expect("an answer");
        assert_eq!(with_error.error.as_deref(), Some("cannot index"));
        assert_eq!(with_error.values.len(), 1);
        assert!(parse_answer(b"no newline").is_err());
        assert!(parse_answer(b"{}\nnot json").is_err());
    }

    #[test]
    fn keys_are_quoted_unless_they_are_plain_words() {
        assert_eq!(key_text("name"), "name");
        assert_eq!(key_text("_id"), "_id");
        assert_eq!(key_text("$ref"), "$ref");
        assert_eq!(key_text("two-words"), "two-words");
        assert_eq!(key_text("two words"), "\"two words\"");
        assert_eq!(key_text("1st"), "\"1st\"");
        assert_eq!(key_text(""), "\"\"");
        assert_eq!(key_text("الاسم"), "\"الاسم\"");
        assert_eq!(key_text("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn rows_know_where_their_values_are() {
        let values = sample();
        let mut open = HashMap::new();
        open.insert("0:.".to_string(), PAGE);
        open.insert("0:.tags".to_string(), PAGE);
        let rows = build_rows(&values, &open);
        let labels: Vec<_> = rows.iter().map(|r| row_label(r, false)).collect();
        assert_eq!(
            labels,
            [
                "{4}",
                "name: \"glint\"",
                "tags: [3]",
                "0: \"fast\"",
                "1: 2",
                "2: null",
                "empty: []",
                "ok: true"
            ]
        );
        // Every row finds its own value again from its steps, at any depth.
        for row in &rows {
            let value = value_at(&values, row.root, &row.steps).expect("the value is there");
            match &row.body {
                Body::Leaf { text, .. } => assert_eq!(&preview(value, 400), text),
                Body::Container { size, .. } => assert_eq!(size_of(value), *size),
                Body::More { .. } => unreachable!("nothing is cut"),
            }
        }
        assert_eq!(rows[5].steps, [1, 2]);
        assert_eq!(rows[7].steps, [3]);
        // Copy gives JSON as JSON.stringify(value, null, 2) writes it.
        assert_eq!(pretty_json(&values[0]), "{\n  \"name\": \"glint\",\n  \"tags\": [\n    \"fast\",\n    2,\n    null\n  ],\n  \"empty\": [],\n  \"ok\": true\n}");
        assert_eq!(pretty_json(&JsonValue::Object(vec![])), "{}");
        assert_eq!(
            pretty_json(&JsonValue::Big(glint_model::json_tree::JsonNumber::new("1.50"))),
            "1.50"
        );
    }

    #[test]
    fn a_big_container_shows_a_page_and_a_row_for_the_rest() {
        let values = vec![JsonValue::Array(
            (0..1200).map(|i| JsonValue::Number(f64::from(i))).collect(),
        )];
        let mut open = HashMap::new();
        open.insert("0:.".to_string(), PAGE);
        let rows = build_rows(&values, &open);
        assert_eq!(rows.len(), 1 + PAGE + 1);
        assert_eq!(rows.last().map(|r| r.body.clone()), Some(Body::More { count: 700 }));
        // Another page: the row for the rest is smaller.
        open.insert("0:.".to_string(), 2 * PAGE);
        let rows = build_rows(&values, &open);
        assert_eq!(rows.last().map(|r| r.body.clone()), Some(Body::More { count: 200 }));
    }

    #[test]
    fn alt_click_opens_what_is_under_a_container_within_its_budget() {
        let values = sample();
        let mut open = HashMap::new();
        let mut budget = OPEN_ALL;
        walk_open(&values[0], "0:.", ".", "0:", &mut open, &mut budget);
        let mut ids: Vec<_> = open.keys().cloned().collect();
        ids.sort();
        assert_eq!(ids, ["0:.", "0:.empty", "0:.tags"]);
        // A budget of two opens the value and its first container only.
        let mut open = HashMap::new();
        let mut budget = 2;
        walk_open(&values[0], "0:.", ".", "0:", &mut open, &mut budget);
        assert_eq!(open.len(), 2);
    }

    #[test]
    fn rows_are_filtered_by_any_cell_and_sorted_by_value_or_by_language() {
        let table = table_of(
            &[("name", "text"), ("size", "number")],
            &[
                &["b10", "100"],
                &["a2", "9"],
                &["B1", "1000"],
                &["ملف", ""],
                &["a10", "x"],
            ],
        );
        assert_eq!(shown_rows(&table, "", None), [0, 1, 2, 3, 4]);
        assert_eq!(shown_rows(&table, "  B1 ", None), [0, 2], "b10 has b1 in it too");
        assert_eq!(shown_rows(&table, "1000", None), [2]);
        assert_eq!(shown_rows(&table, "ملف", None), [3]);
        assert_eq!(shown_rows(&table, "zzz", None), Vec::<usize>::new());
        // Text: case ignored, digits as numbers (a2 before a10).
        let by_name = shown_rows(&table, "", Some((0, false)));
        let names: Vec<_> = by_name.iter().map(|&i| table.rows[i][0].as_str()).collect();
        assert_eq!(&names[..4], ["a2", "a10", "B1", "b10"]);
        // Numbers by value (9 before 100), an empty cell counting as zero as JavaScript's Number("") does.
        let by_size = shown_rows(&table, "", Some((1, false)));
        assert_eq!(by_size[0], 3, "the empty cell is 0");
        assert_eq!(&by_size[1..3], &[1, 0]);
        assert_eq!(
            shown_rows(&table, "", Some((1, true)))[0],
            4,
            "text after numbers when descending"
        );
    }

    #[gpui_kit::test]
    fn the_tree_shows_the_values_and_opens_and_closes_by_click_and_key(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, asked, _) = fake(Ok(sample()), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\data\\project.json", DataFormat::Json, source);
        // Under way: nothing to show yet.
        assert!(view.read_with(&visual, |v, _| matches!(v.answer, Answer::Waiting)));
        ready_tree(&view, &mut visual);
        assert_eq!(
            asked.lock().map(|a| a.clone()).unwrap_or_default(),
            [("C:\\data\\project.json".to_string(), ".".to_string())]
        );
        // The value is open, and so is the small array inside it.
        assert_eq!(
            rows_of(&view, &visual),
            [
                "{4}",
                "name: \"glint\"",
                "tags: [3]",
                "0: \"fast\"",
                "1: 2",
                "2: null",
                "empty: [0]",
                "ok: true"
            ]
        );
        assert!(has(&mut visual, "name: \"glint\""));
        assert!(has(&mut visual, "Values"));
        assert!(has(&mut visual, "1 value · read in 1.2 ms") || has(&mut visual, "1 value · read in 1.3 ms"));
        // Clicking a container closes it, and clicking again opens it.
        visual.update(|window, cx| window.click(("json-row", 2usize), cx));
        visual.run_until_parked();
        assert_eq!(rows_of(&view, &visual)[2], "tags: [\"fast\", 2, null]");
        assert_eq!(rows_of(&view, &visual).len(), 5);
        visual.update(|window, cx| window.click(("json-row", 2usize), cx));
        visual.run_until_parked();
        assert_eq!(rows_of(&view, &visual).len(), 8);
        // The selected row's path is in the footer, and Filter runs it.
        assert!(has(&mut visual, ".tags"));
        // Keys: down to the next row, left closes the open array under the cursor, right opens it and steps in.
        visual.update(|window, cx| {
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
            window.press("left", cx);
        });
        visual.run_until_parked();
        assert_eq!(rows_of(&view, &visual).len(), 5, "left closes an open container");
        visual.update(|window, cx| window.press("right", cx));
        visual.run_until_parked();
        assert_eq!(rows_of(&view, &visual).len(), 8, "right opens a closed one");
        visual.update(|window, cx| window.press("right", cx));
        visual.run_until_parked();
        assert_eq!(
            view.read_with(&visual, |v, _| v.selected.clone()).as_deref(),
            Some("0:.tags[0]")
        );
        visual.update(|window, cx| window.press("left", cx));
        visual.run_until_parked();
        assert_eq!(
            view.read_with(&visual, |v, _| v.selected.clone()).as_deref(),
            Some("0:.tags"),
            "left on a leaf goes to its parent"
        );
        // Ctrl+C copies the value as JSON.
        visual.update(|window, cx| window.press("ctrl-c", cx));
        visual.run_until_parked();
        let copied = visual.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("[\n  \"fast\",\n  2,\n  null\n]"));
    }

    #[gpui_kit::test]
    fn several_values_are_numbered_and_big_ones_page(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        // A YAML file with two documents.
        let docs = vec![
            object(vec![("name", text("glint"))]),
            object(vec![("name", text("second"))]),
        ];
        let (source, _, _) = fake(Ok(docs), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\data\\two.yaml", DataFormat::Yaml, source);
        ready_tree(&view, &mut visual);
        assert_eq!(
            rows_of(&view, &visual),
            ["#1 {1}", "name: \"glint\"", "#2 {1}", "name: \"second\""]
        );
        assert!(has(&mut visual, "2 values · read in 1.2 ms") || has(&mut visual, "2 values · read in 1.3 ms"));

        // A big array: the first 500, a row for the 700 more, and clicking it shows the next 500.
        let big = vec![JsonValue::Array(
            (0..1200).map(|i| JsonValue::Number(f64::from(i))).collect(),
        )];
        let (source, _, _) = fake(Ok(big), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\data\\big.json", DataFormat::Json, source);
        ready_tree(&view, &mut visual);
        assert_eq!(rows_of(&view, &visual).len(), 502);
        let more = format!("{} more… (show {})", format_count(700), format_count(500));
        assert!(view.read_with(&visual, |v, _| v.row_texts().last().cloned()) == Some(more));
        view.update_in(&mut visual, |v, _, cx| v.more("0:.\u{0}more", cx));
        assert_eq!(rows_of(&view, &visual).len(), 1 + 1000 + 1);
    }

    #[gpui_kit::test]
    fn arabic_keys_and_strings_show_and_a_bad_file_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let values = vec![object(vec![("الاسم", text("مرحبا بالعالم")), ("name", text("x"))])];
        let (source, _, _) = fake(Ok(values), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\بيانات\\ملف.json", DataFormat::Json, source);
        ready_tree(&view, &mut visual);
        assert_eq!(
            rows_of(&view, &visual),
            ["{2}", "\"الاسم\": \"مرحبا بالعالم\"", "name: \"x\""]
        );
        assert!(has(&mut visual, &t!("jq filter")));
        testing::set_rtl(false);

        // A file that can't be read: the reason, and no tree.
        let (source, _, _) = fake(
            Err("This file can't be read as JSON: expected value at line 1 column 2".into()),
            no_table(),
        );
        let (view, mut visual) = open_tree(cx, "C:\\data\\bad.json", DataFormat::Json, source);
        ready_tree(&view, &mut visual);
        assert!(has(
            &mut visual,
            "This file can't be read as JSON: expected value at line 1 column 2"
        ));
        assert!(rows_of(&view, &visual).is_empty());

        // A file too big for the tree, said the way glint-core says it.
        let (source, _, _) = fake(Err("This file is over 64 MB, too big for the tree.".into()), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\data\\huge.json", DataFormat::Json, source);
        ready_tree(&view, &mut visual);
        assert!(has(&mut visual, "This file is over 64 MB, too big for the tree."));
    }

    #[gpui_kit::test]
    fn a_filter_runs_again_and_stop_and_stopped_are_handled(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, asked, stopped) = fake(Ok(sample()), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\data\\project.json", DataFormat::Json, source);
        ready_tree(&view, &mut visual);
        visual.update(|window, cx| view.update(cx, |v, cx| v.run(" .tags[] ", window, cx)));
        settle(&mut visual, |_| asked.lock().map(|a| a.len()).unwrap_or(0) == 2);
        assert_eq!(
            asked.lock().ok().and_then(|a| a.last().cloned()),
            Some(("C:\\data\\project.json".to_string(), ".tags[]".to_string()))
        );
        ready_tree(&view, &mut visual);
        assert!(has(&mut visual, "1 value · jaq in 1.2 ms") || has(&mut visual, "1 value · jaq in 1.3 ms"));
        // An empty filter is the file as it is.
        visual.update(|window, cx| view.update(cx, |v, cx| v.run("", window, cx)));
        settle(&mut visual, |_| asked.lock().map(|a| a.len()).unwrap_or(0) == 3);
        assert_eq!(
            asked.lock().ok().and_then(|a| a.last().map(|l| l.1.clone())).as_deref(),
            Some(".")
        );
        // Stop asks glint-core to stop the query.
        view.update(&mut visual, |v, _| v.stop());
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        // A stopped query answers "stopped", which is shown as the word.
        let (source, _, _) = fake(Err("stopped".into()), no_table());
        let (view, mut visual) = open_tree(cx, "C:\\data\\slow.json", DataFormat::Json, source);
        ready_tree(&view, &mut visual);
        assert!(has(&mut visual, "Stopped."));
    }

    #[gpui_kit::test]
    fn the_table_shows_its_rows_and_filters_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let table = table_of(
            &[("name", "text"), ("size", "number"), ("done", "boolean")],
            &[
                &["alpha", "10", "true"],
                &["beta", "2", "false"],
                &["غاما", "300", "true"],
            ],
        );
        let (source, _, _) = fake(Ok(vec![]), Ok(table));
        cx.update(testing::init);
        let (mut visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| TableViewer::with_source("C:\\data\\sheet.csv".into(), source, window, cx))
        });
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, TableLoad::Ready { .. }))
        });
        frames(&mut visual);
        assert!(has(&mut visual, "CSV"));
        assert!(has(&mut visual, "3 cols · 3 rows"));
        assert_eq!(view.read_with(&visual, |v, cx| v.shown_cells(cx).len()), 3);
        assert!(has(&mut visual, "غاما"), "Arabic cells show");
        // Filtering by any cell narrows the rows and the counts.
        visual.update(|window, cx| {
            let filter = view.read(cx).filter.clone();
            filter.update(cx, |f, cx| f.focus(window, cx));
            window.input("bet", cx);
        });
        frames(&mut visual);
        assert_eq!(
            view.read_with(&visual, |v, cx| v.shown_cells(cx)),
            vec![vec!["beta".to_string(), "2".to_string(), "false".to_string()]]
        );
        assert!(has(&mut visual, "3 cols · 1 of 3 rows"));
        // Raw text on offer: the button flips and the file's text takes the grid's place.
        assert!(has(&mut visual, "Raw") || has(&mut visual, "table-raw"));
        visual.update(|window, cx| window.click("table-raw", cx));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.showing_raw && v.raw.is_some()));
    }

    #[gpui_kit::test]
    fn a_long_table_only_draws_its_visible_rows_and_says_it_is_a_sample(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let rows: Vec<Vec<String>> = (0..20_000).map(|i| vec![format!("row {i}"), i.to_string()]).collect();
        let mut table = table_of(&[("name", "text"), ("n", "number")], &[]);
        table.total_rows_sampled = rows.len();
        table.rows = rows;
        table.truncated = true;
        let (source, _, _) = fake(Ok(vec![]), Ok(table));
        cx.update(testing::init);
        let (mut visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| TableViewer::with_source("C:\\data\\big.csv".into(), source, window, cx))
        });
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, TableLoad::Ready { .. }))
        });
        frames(&mut visual);
        assert!(has(&mut visual, &format!("2 cols · {}+ rows", format_count(20_000))));
        assert!(has(
            &mut visual,
            &format!(
                "Showing the first {} rows sampled from the file header.",
                format_count(20_000)
            )
        ));
        // Far fewer rows are in the tree of elements than the file has.
        let drawn = visual.update(|window, _| window.find_all_by_role(Role::Row).len());
        assert!(drawn < 200, "{drawn} rows drawn");
    }

    #[gpui_kit::test]
    fn the_previewer_picks_a_tree_a_table_or_neither(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, _, _) = fake(Ok(sample()), Ok(table_of(&[("a", "text")], &[&["1"]])));
        cx.update(testing::init);
        let (mut visual, view) = {
            let source = source.clone();
            testing::open_window(cx, move |window, cx| {
                cx.new(|cx| DataPreview::with_source("C:\\data\\a.json".into(), source, window, cx))
            })
        };
        assert!(view.read_with(&visual, |v, _| matches!(v.shown, Shown::Tree(_))));
        // Another file of the other kind makes the other view; a file that is neither says so.
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_path("C:\\data\\b.csv".into(), window, cx)));
        assert!(view.read_with(&visual, |v, _| matches!(v.shown, Shown::Table(_))));
        assert_eq!(view.read_with(&visual, |v, _| v.kind()), Some(DataKind::Table));
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_path("C:\\data\\c.txt".into(), window, cx)));
        assert!(view.read_with(&visual, |v, _| matches!(v.shown, Shown::Neither)));
        assert!(has(
            &mut visual,
            "This isn't a data file Glint shows as a tree or a table"
        ));
    }

    #[test]
    fn every_icon_the_data_views_draw_is_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::ChevronDown,
            IconName::ChevronRight,
            IconName::ClipboardCopy,
            IconName::Funnel,
            IconName::Square,
            IconName::Table2,
            IconName::FileCode,
            IconName::Search,
            IconName::Hash,
            IconName::ToggleLeft,
            IconName::Type,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
