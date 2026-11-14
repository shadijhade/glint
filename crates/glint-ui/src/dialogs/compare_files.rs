//! Comparing two files and merging their changes. Ported from src/components/explorer/CompareFilesDialog.tsx,
//! with glint-core's `filediff.rs` doing the comparing.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                           |
//! |----------------------------------------------|----------------------------------------------------------------|
//! | `compareFiles(left, right, onClose)`         | [`open`] (two paths) and [`open_sides`] (labels, read-only, on close) |
//! | `CompareSide { path, label, readOnly }`      | [`Side`]                                                       |
//! | `discardCompareEdits()`                      | [`unsaved`] for quitting; closing, Swap and the view buttons ask themselves |
//! | `CompareModal`: Text, Pictures, Bytes, Swap  | [`CompareDialog`]                                              |
//! | `TextView`'s state and `compareTextFiles`    | [`TextModel`] (the comparison, the merge, the edits, the saves) |
//! | `TextView`'s rows, toolbar and find bar      | [`TextPane`]                                                   |
//! | `blocks`, `visibleItems`, `Marked`           | [`Analysis`], [`visible_items`], [`marks`] (pure, tested)      |
//! | `PicturesView`                               | [`PicturesPane`]                                               |
//! | `BytesView`                                  | [`BytesPane`]                                                  |
//! | `writeFileContent(path, text, expectedHash)` | [`Backend::write`]                                             |
//!
//! **The five parts of a dialog** (see [`super`]): the comparing, merging and saving live in a model entity
//! ([`TextModel`]) that asks glint-core through a [`Backend`] (never the UI thread; a fake in tests); the
//! views draw only with kit controls, theme tokens and `t!`; every row, button and bar has a role and a name;
//! Escape closes it (or closes the find bar, or cancels a line's edit, first); and the tests at the bottom
//! open it with a fake backend.
//!
//! **Big files.** The rows are a `uniform_list`, so a file of 100,000 lines draws the dozen rows on screen. A
//! row's text is shaped only when drawn (and GPUI keeps the shaping from frame to frame), a line longer than
//! [`MAX_SHOWN_CHARS`] shows its first part and an ellipsis (the browser clipped it; shaping a minified line
//! of a megabyte every frame would not do), and the comparing, the change blocks and the find all run in the
//! background. Over 8 MB glint-core says "too big" and the dialog offers the bytes instead, as before; the bytes
//! are read 64 KB at a time as rows are drawn, in pages of [`PAGE_ROWS`] rows (GPUI places rows with 32-bit
//! pixels, exact only up to 16 million).
//!
//! **Left and right.** The two files are the *left* and the *right* one, in a way that has nothing to do with
//! the reading direction (they are what the arrows copy between, and what Alt+Left and Alt+Right mean), so, like
//! code and bytes, the panes keep their order in the Arabic interface: the left file stays on the left. The
//! chrome around them (toolbar, buttons) mirrors as everywhere. The TypeScript's panes flipped in Arabic with
//! their arrows still pointing the old way; that was not kept.
//!
//! **Opening it.** `MenuEvent::Compare { left, right }` (the menus' "Compare … and …", "Compare with … in the
//! other pane") and the `CompareFiles` action (Ctrl+M, with the two files selected or one in each pane) call
//! `dialogs::compare_files::open(window, cx, left, right)`. The Versions dialog's `Request::CompareFiles { then,
//! now }` calls it with the made copy as `left`; the Changes view's "side by side" calls [`open_sides`] with
//! [`Side::named`] ("In the last commit", read-only, then "Now") and an `on_close` that discards the temp copy.
//! Quitting asks [`unsaved`] first (`mayQuit`).
//!
//! **Not ported, or changed:**
//! - Ctrl+Z is not bound as an action (only the `Compare` bindings of `actions.rs` are, and Ctrl+Z may not be
//!   bound outside the file list); the dialog's key handler undoes on it when no box has the keyboard, and the
//!   Undo button does too.
//! - A copy or an edit asked for while the comparison of the last one is still running is ignored: its blocks
//!   would be those of text that is no longer there (the TypeScript could mix them up).
//! - The first Enter in the find box goes to the first match (the TypeScript's went to the second).
//! - A failed save over a file changed elsewhere asks "Save anyway?" in a dialog instead of a toast with a button.
//! - The hatching of the empty half of a row is a plain muted fill, and the checks behind a picture are the
//!   muted ground.
//! - The pictures' two sliders (swipe place or blend, and tolerance) are minus and plus buttons that step by
//!   10%; the pixel difference is the same, worked out in the background (GPUI has no slider in the kit).
//! - The hover texts of a row's halves (the whole line, "Double-click to edit line N") are not drawn: the row's
//!   name says both lines, and the list would otherwise create a tooltip per row.
//! - Not exercised by a test: the Difftastic install button and the AST switch (they read the Settings tools
//!   model, whose fake is private to that module).
//! - A line longer than [`MAX_SHOWN_CHARS`] characters is cut when drawn (editing it still has all of it).

use crate::actions;
use crate::components::preview::picture;
use crate::components::settings::tools::Tools;
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::filediff::{CompareOptions, IgnoreWhitespace, TextCompare};
use glint_model::file_types::{kind_of, PreviewKind};
use glint_model::format::{format_count, format_duration, format_size, plural};
use glint_model::hex::{byte_char, hex, parse_offset};
use glint_model::paths::{base_name, parent_path};
use gpui_kit::base::actions::Cancel;
use gpui_kit::{
    canvas, fill, img, point, Bounds, HighlightStyle, Image, ImageFormat, KeyDownEvent, MouseButton, MouseDownEvent,
    ObjectFit, RenderImage, ScrollStrategy, StyledText, UniformListScrollHandle,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

/// A row's height (`h-5`, 20 px at the usual size).
const LINE: Rems = rems(1.25);
/// Lines of context kept around a change in the Context view (`±3`).
const CONTEXT_LINES: usize = 3;
/// The most characters of a line that are drawn (see the module docs).
const MAX_SHOWN_CHARS: usize = 2_000;
/// Bytes in a row of the bytes view (`ROW`).
const BYTES_ROW: u64 = 16;
/// Bytes read at a time in the bytes view (`WINDOW`).
const WINDOW: u64 = 64 * 1024;
/// Rows in a page of the bytes view.
const PAGE_ROWS: u64 = 1 << 18;
/// Windows of the bytes view kept (3 MB for both files).
const KEPT_WINDOWS: usize = 24;

// ---- The two files ----------------------------------------------------------------------------------------

/// One of the two files: its path, and what to call it when its name doesn't say ("In the last commit")
/// (`CompareSide`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    pub path: String,
    pub label: Option<String>,
    /// Never edited or saved here (a temp copy out of a backup or the last commit).
    pub read_only: bool,
}

impl Side {
    /// A plain file.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            label: None,
            read_only: false,
        }
    }

    /// A file shown under `label`, possibly read-only.
    pub fn named(path: impl Into<String>, label: impl Into<String>, read_only: bool) -> Self {
        Self {
            path: path.into(),
            label: Some(label.into()),
            read_only,
        }
    }

    /// What it is called: its label, else its file name (`nameOf`).
    pub fn name(&self) -> String {
        self.label.clone().unwrap_or_else(|| base_name(&self.path).to_string())
    }
}

/// Whether `path` is a picture (`isPicture`): by its name, as the file kinds say.
fn is_picture(path: &str) -> bool {
    let entry = glint_model::entries::with_details(
        glint_model::entries::RawEntry {
            name: base_name(path).to_string(),
            path: Some(path.to_string()),
            ..Default::default()
        },
        "",
    );
    kind_of(&entry).preview == Some(PreviewKind::Image)
}

// ---- Where the answers come from ---------------------------------------------------------------------------

/// A boxed answer from glint-core (or a fake).
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// A window of both files' bytes: their sizes and the bytes from the same offset.
pub use glint_core::filediff::BytesWindow;

/// What the dialog asks of glint-core: glint-core in the app ([`CoreBackend`]), a fake in tests.
pub trait Backend: Send + Sync + 'static {
    /// `compare_text_files`: the two files as text, or with the text of a side as edited (`*_override`).
    fn compare_text(
        &self,
        left: &str,
        right: &str,
        options: CompareOptions,
        left_override: Option<String>,
        right_override: Option<String>,
    ) -> Fut<TextCompare>;
    /// `write_file_content`: saves `content` to `path` and answers its hash; with `expected`, only while the
    /// file still has that hash (an answer saying "since it was compared" if it doesn't).
    fn write(&self, path: &str, content: String, expected: Option<String>) -> Fut<String>;
    /// `compare_bytes`: up to `len` bytes of each file from `offset`.
    fn bytes(&self, left: &str, right: &str, offset: u64, len: u32) -> Fut<BytesWindow>;
    /// `next_byte_difference`.
    fn next_difference(&self, left: &str, right: &str, from: u64) -> Fut<Option<u64>>;
    /// `prev_byte_difference`.
    fn prev_difference(&self, left: &str, right: &str, before: u64) -> Fut<Option<u64>>;
}

/// glint-core.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn compare_text(
        &self,
        left: &str,
        right: &str,
        options: CompareOptions,
        left_override: Option<String>,
        right_override: Option<String>,
    ) -> Fut<TextCompare> {
        Box::pin(glint_core::filediff::compare_text_files(
            left.to_string(),
            right.to_string(),
            Some(options),
            left_override,
            right_override,
        ))
    }

    fn write(&self, path: &str, content: String, expected: Option<String>) -> Fut<String> {
        Box::pin(glint_core::write_file_content(path.to_string(), content, expected))
    }

    fn bytes(&self, left: &str, right: &str, offset: u64, len: u32) -> Fut<BytesWindow> {
        let (left, right) = (left.to_string(), right.to_string());
        Box::pin(glint_core::filediff::compare_bytes(left, right, offset, len))
    }

    fn next_difference(&self, left: &str, right: &str, from: u64) -> Fut<Option<u64>> {
        Box::pin(glint_core::filediff::next_byte_difference(
            left.to_string(),
            right.to_string(),
            from,
        ))
    }

    fn prev_difference(&self, left: &str, right: &str, before: u64) -> Fut<Option<u64>> {
        Box::pin(glint_core::filediff::prev_byte_difference(
            left.to_string(),
            right.to_string(),
            before,
        ))
    }
}

// ---- The comparison as the view needs it -----------------------------------------------------------------

/// A contiguous run of changed rows, with the lines each side holds in it (`ChangeBlock`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeBlock {
    /// Rows `[start_row, end_row)` of the comparison.
    pub start_row: usize,
    pub end_row: usize,
    /// Lines `[start, end)` of the left file the block covers, or where it would go in (an empty range).
    pub left_range: (usize, usize),
    /// The same for the right file.
    pub right_range: (usize, usize),
}

/// A comparison with what is worked out from it once, in the background: the change blocks and which block
/// each row is in.
#[derive(Debug)]
pub struct Analysis {
    pub compare: TextCompare,
    pub blocks: Vec<ChangeBlock>,
    /// The block of each row, `u32::MAX` for an unchanged row.
    row_block: Vec<u32>,
    /// Changed lines on the left and on the right (`Object.keys(leftChanges).length`).
    pub removed: usize,
    pub added: usize,
}

impl Analysis {
    /// Works out the blocks of `compare`.
    pub fn of(compare: TextCompare) -> Self {
        let blocks = change_blocks(&compare);
        let mut row_block = vec![u32::MAX; compare.rows.len()];
        for (index, block) in blocks.iter().enumerate() {
            let index = u32::try_from(index).unwrap_or(u32::MAX - 1);
            if let Some(rows) = row_block.get_mut(block.start_row..block.end_row) {
                rows.fill(index);
            }
        }
        Self {
            removed: compare.left_changes.len(),
            added: compare.right_changes.len(),
            compare,
            blocks,
            row_block,
        }
    }

    /// The block row `row` is in, if it is a changed row.
    pub fn block_of(&self, row: usize) -> Option<usize> {
        self.row_block
            .get(row)
            .copied()
            .filter(|&b| b != u32::MAX)
            .and_then(|b| usize::try_from(b).ok())
    }

    /// The lines of one side (0 left, 1 right).
    pub fn lines(&self, side: usize) -> &[String] {
        if side == 0 {
            &self.compare.left
        } else {
            &self.compare.right
        }
    }

    /// What changed in line `line` of `side`, as UTF-16 ranges; `None` when the line is unchanged and an empty
    /// list when the whole line is.
    pub fn changes(&self, side: usize, line: usize) -> Option<&Vec<[u32; 2]>> {
        let line = u32::try_from(line).ok()?;
        if side == 0 {
            self.compare.left_changes.get(&line)
        } else {
            self.compare.right_changes.get(&line)
        }
    }
}

fn row_is_changed(compare: &TextCompare, [l, r]: [Option<u32>; 2]) -> bool {
    l.is_some_and(|l| compare.left_changes.contains_key(&l))
        || r.is_some_and(|r| compare.right_changes.contains_key(&r))
}

/// Where lines of `side` would go in at row `start_row`: after the last line that side has before it
/// (the TypeScript's walk back through the rows).
fn insertion_point(rows: &[[Option<u32>; 2]], start_row: usize, side: usize) -> usize {
    rows.get(..start_row)
        .unwrap_or(rows)
        .iter()
        .rev()
        .find_map(|row| row[side])
        .map_or(0, |n| n as usize + 1)
}

/// The contiguous changed blocks with their line ranges on each side (`blocks`).
pub fn change_blocks(compare: &TextCompare) -> Vec<ChangeBlock> {
    let rows = &compare.rows;
    let mut list = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if !row_is_changed(compare, rows[i]) {
            i += 1;
            continue;
        }
        let start_row = i;
        let mut spans: [Option<(usize, usize)>; 2] = [None, None];
        while i < rows.len() && row_is_changed(compare, rows[i]) {
            for (side, span) in spans.iter_mut().enumerate() {
                if let Some(line) = rows[i][side] {
                    let (min, max) = (line as usize, line as usize + 1);
                    *span = Some(span.map_or((min, max), |(lo, hi)| (lo.min(min), hi.max(max))));
                }
            }
            i += 1;
        }
        let range = |side: usize| {
            spans[side].unwrap_or_else(|| {
                let at = insertion_point(rows, start_row, side);
                (at, at)
            })
        };
        list.push(ChangeBlock {
            start_row,
            end_row: i,
            left_range: range(0),
            right_range: range(1),
        });
    }
    list
}

/// Which rows are shown (`RowFilter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowFilter {
    All,
    /// The changes with three lines around each.
    Context,
    /// The changes alone.
    Diffs,
}

/// A line of the list: a row, or a fold of unchanged rows (`VisibleItem`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Row {
        row: usize,
        block: Option<usize>,
        is_block_start: bool,
    },
    Fold {
        start: usize,
        end: usize,
        count: usize,
    },
}

/// The list's lines for `filter`, with the folds in `expanded` (keyed by their first row) opened
/// (`visibleItems`).
pub fn visible_items(analysis: &Analysis, filter: RowFilter, expanded: &HashSet<usize>) -> Vec<Item> {
    let total = analysis.compare.rows.len();
    let row_item = |row: usize| {
        let block = analysis.block_of(row);
        Item::Row {
            row,
            block,
            is_block_start: block.is_some_and(|b| analysis.blocks[b].start_row == row),
        }
    };
    match filter {
        RowFilter::All => (0..total).map(row_item).collect(),
        RowFilter::Diffs => (0..total)
            .filter(|&row| analysis.block_of(row).is_some())
            .map(row_item)
            .collect(),
        RowFilter::Context => {
            let mut keep = vec![false; total];
            for block in &analysis.blocks {
                let from = block.start_row.saturating_sub(CONTEXT_LINES);
                let to = (block.end_row + CONTEXT_LINES).min(total);
                if let Some(range) = keep.get_mut(from..to) {
                    range.fill(true);
                }
            }
            let mut out = Vec::new();
            let mut row = 0;
            while row < total {
                if keep[row] {
                    out.push(row_item(row));
                    row += 1;
                    continue;
                }
                let start = row;
                while row < total && !keep[row] {
                    row += 1;
                }
                let count = row - start;
                if count <= 1 || expanded.contains(&start) {
                    out.extend((start..row).map(|k| Item::Row {
                        row: k,
                        block: None,
                        is_block_start: false,
                    }));
                } else {
                    out.push(Item::Fold { start, end: row, count });
                }
            }
            out
        }
    }
}

/// The index in `items` of the row `row`, if it is shown.
pub fn visible_index_of(items: &[Item], row: usize) -> Option<usize> {
    let first = |item: &Item| match *item {
        Item::Row { row, .. } => row,
        Item::Fold { start, .. } => start,
    };
    let at = items.partition_point(|item| first(item) < row);
    match items.get(at) {
        Some(Item::Row { row: found, .. }) if *found == row => Some(at),
        _ => None,
    }
}

/// A side's lines as the file holds them (`serializeLines`).
pub fn serialize_lines(lines: &[String], crlf: bool, trailing_newline: bool) -> String {
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut body = lines.join(eol);
    if trailing_newline && !lines.is_empty() {
        body.push_str(eol);
    }
    body
}

/// `lines` with `range` replaced by `replacement`; a range beyond the end is cut to it, as `slice` does.
pub fn splice_lines(lines: &[String], range: (usize, usize), replacement: &[String]) -> Vec<String> {
    let start = range.0.min(lines.len());
    let end = range.1.min(lines.len()).max(start);
    let mut out = Vec::with_capacity(lines.len() + replacement.len());
    out.extend_from_slice(&lines[..start]);
    out.extend_from_slice(replacement);
    out.extend_from_slice(&lines[end..]);
    out
}

/// The rows where either side has `query` in its line, ignoring case (`searchRowMatches`).
pub fn matching_rows(compare: &TextCompare, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    let has = |lines: &[String], line: Option<u32>| {
        line.and_then(|n| lines.get(n as usize))
            .is_some_and(|text| text.to_lowercase().contains(&needle))
    };
    compare
        .rows
        .iter()
        .enumerate()
        .filter(|(_, [l, r])| has(&compare.left, *l) || has(&compare.right, *r))
        .map(|(row, _)| row)
        .collect()
}

/// Where `query` is in `text`, ignoring case: UTF-8 byte ranges in `text`, in order and not overlapping
/// (`findSpansInLine`; JavaScript counted UTF-16 units).
pub fn find_spans_in_line(text: &str, query: &str) -> Vec<Range<usize>> {
    let needle: Vec<char> = query.to_lowercase().chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // Each character of the lowercased text, with where in `text` it came from.
    let mut hay: Vec<(char, usize, usize)> = Vec::new();
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        hay.extend(ch.to_lowercase().map(|low| (low, start, end)));
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut at = 0;
    while at + needle.len() <= hay.len() {
        if hay[at..at + needle.len()]
            .iter()
            .map(|h| h.0)
            .eq(needle.iter().copied())
        {
            let range = hay[at].1..hay[at + needle.len() - 1].2;
            // A match that shares a character with the last one (a letter that lowercases to two) is one mark.
            match out.last_mut() {
                Some(last) if last.end > range.start => last.end = last.end.max(range.end),
                _ => out.push(range),
            }
            at += needle.len();
        } else {
            at += 1;
        }
    }
    out
}

// ---- A line as it is drawn -------------------------------------------------------------------------------

/// A line ready to draw: tabs spread to tab stops of four, a very long line cut, and the way from the
/// positions the comparison gives (UTF-16 units, as the page counted) and the ones the search gives (bytes of
/// the original) to bytes of what is drawn, which is what GPUI's text runs count.
pub struct Shaped {
    pub text: String,
    /// At every character boundary of the original: its UTF-16 offset, its byte offset, its byte offset drawn.
    bounds: Vec<(u32, usize, usize)>,
    /// Where drawing stops (before the ellipsis of a cut line).
    limit: usize,
}

impl Shaped {
    /// Shapes `text`.
    pub fn new(text: &str) -> Self {
        let mut out = String::with_capacity(text.len());
        let mut bounds = Vec::with_capacity(text.len() + 1);
        let (mut utf16, mut column) = (0u32, 0usize);
        let mut limit = None;
        for (n, (byte, ch)) in text.char_indices().enumerate() {
            if n >= MAX_SHOWN_CHARS {
                limit = Some(out.len());
                out.push('…');
                break;
            }
            bounds.push((utf16, byte, out.len()));
            if ch == '\t' {
                let spaces = 4 - column % 4;
                out.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            } else {
                out.push(ch);
                column += 1;
            }
            utf16 += u32::try_from(ch.len_utf16()).unwrap_or(1);
        }
        let limit = match limit {
            Some(limit) => limit,
            None => {
                bounds.push((utf16, text.len(), out.len()));
                out.len()
            }
        };
        Self {
            text: out,
            bounds,
            limit,
        }
    }

    /// The drawn byte of the first character boundary at or after UTF-16 offset `at`.
    pub fn from_utf16(&self, at: u32) -> usize {
        let i = self.bounds.partition_point(|b| b.0 < at);
        self.bounds.get(i).map_or(self.limit, |b| b.2.min(self.limit))
    }

    /// The drawn byte of the first character boundary at or after byte `at` of the original.
    pub fn from_byte(&self, at: usize) -> usize {
        let i = self.bounds.partition_point(|b| b.1 < at);
        self.bounds.get(i).map_or(self.limit, |b| b.2.min(self.limit))
    }
}

/// How a run of a line is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// What changed in the line.
    Change,
    /// A hit of the find box (drawn over a change).
    Find,
}

/// The marked runs of a drawn line, in order and not overlapping: `changes` (UTF-16 ranges from the comparison;
/// an end before its start or past the line is cut to it) and `finds` (byte ranges of the original), where a
/// find wins over a change (`Marked`).
pub fn marks(shaped: &Shaped, changes: &[[u32; 2]], finds: &[Range<usize>]) -> Vec<(Range<usize>, Mark)> {
    let change_ranges: Vec<Range<usize>> = changes
        .iter()
        .map(|[s, e]| shaped.from_utf16(*s)..shaped.from_utf16(*e))
        .filter(|r| r.start < r.end)
        .collect();
    let find_ranges: Vec<Range<usize>> = finds
        .iter()
        .map(|r| shaped.from_byte(r.start)..shaped.from_byte(r.end))
        .filter(|r| r.start < r.end)
        .collect();
    let mut edges: Vec<usize> = change_ranges
        .iter()
        .chain(&find_ranges)
        .flat_map(|r| [r.start, r.end])
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let covers = |ranges: &[Range<usize>], at: usize| ranges.iter().any(|r| r.start <= at && at < r.end);
    let mut out: Vec<(Range<usize>, Mark)> = Vec::new();
    for pair in edges.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let mark = if covers(&find_ranges, from) {
            Mark::Find
        } else if covers(&change_ranges, from) {
            Mark::Change
        } else {
            continue;
        };
        match out.last_mut() {
            Some((last, kind)) if last.end == from && *kind == mark => last.end = to,
            _ => out.push((from..to, mark)),
        }
    }
    out
}

// ---- How the text is cut up for the bytes view -----------------------------------------------------------------

/// One side of a row of bytes: the hex cells (a space between, and one more after the eighth) and the
/// characters, each with its byte's range in them, and whether it differs from the other side.
pub struct ByteRow {
    pub cells: String,
    pub cell_ranges: Vec<Range<usize>>,
    pub chars: String,
    pub char_ranges: Vec<Range<usize>>,
}

/// The text of `row` (the bytes there are, up to 16) for one side; missing bytes are blanks.
pub fn byte_row(row: &[u8]) -> ByteRow {
    let mut cells = String::new();
    let mut cell_ranges = Vec::new();
    let mut chars = String::new();
    let mut char_ranges = Vec::new();
    for k in 0..BYTES_ROW as usize {
        if k == 8 {
            cells.push(' ');
        }
        match row.get(k) {
            Some(&byte) => {
                let start = cells.len();
                cells.push_str(&hex(u64::from(byte), 2));
                cell_ranges.push(start..cells.len());
                let start = chars.len();
                chars.push(byte_char(byte));
                char_ranges.push(start..chars.len());
            }
            None => {
                cells.push_str("  ");
                chars.push(' ');
            }
        }
        if k + 1 < BYTES_ROW as usize {
            cells.push(' ');
        }
    }
    ByteRow {
        cells,
        cell_ranges,
        chars,
        char_ranges,
    }
}

/// Which bytes of `mine` differ from `other` at the same place (`differs`): all of them where the other side
/// has none.
pub fn differing(mine: &[u8], other: &[u8]) -> Vec<bool> {
    mine.iter()
        .enumerate()
        .map(|(k, byte)| other.get(k).is_none_or(|o| o != byte))
        .collect()
}

// ---- The text comparison: the model ----------------------------------------------------------------------

/// Where the comparison stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Asked; waiting for glint-core.
    Loading,
    /// The last attempt failed, with why.
    Failed(SharedString),
    /// `analysis` holds the last comparison.
    Ready,
}

/// How each file ends its lines on disk, so a save writes them back the same way (`diskMeta`).
#[derive(Clone, Copy, Debug)]
struct DiskMeta {
    crlf: [bool; 2],
    newline: [bool; 2],
}

impl Default for DiskMeta {
    fn default() -> Self {
        Self {
            crlf: [false; 2],
            newline: [true; 2],
        }
    }
}

/// A side's lines as edited here.
pub type Lines = Arc<Vec<String>>;

/// What the model tells the pane that it can't show by being drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextEvent {
    /// A new comparison arrived: the blocks, the rows and the matches of the find box are new.
    Compared,
    /// A side was saved (its name).
    Saved(String),
    /// A save didn't go through. `changed` is "the file changed on disk since it was compared": saving over it
    /// anyway is a choice.
    SaveFailed {
        side: usize,
        name: String,
        reason: String,
        changed: bool,
    },
}

/// Two files as text: the comparison, the merge between them, the edits made here and their saving
/// (`TextView`'s state). The edits are the lines of a side as changed here (`None`: as on disk); every change
/// to them compares again, with the edited text, in the background.
pub struct TextModel {
    backend: Arc<dyn Backend>,
    sides: [Side; 2],
    options: CompareOptions,
    state: State,
    analysis: Option<Arc<Analysis>>,
    edits: [Option<Lines>; 2],
    /// What `edits` were before each change, for Undo.
    history: Vec<[Option<Lines>; 2]>,
    disk_meta: Option<DiskMeta>,
    /// Each file as last read from disk (or saved): a save over a file changed elsewhere since is refused.
    disk_hash: [Option<String>; 2],
    saving: [bool; 2],
    /// Which comparison is the latest; an older one's answer is dropped.
    generation: u64,
    running: bool,
    /// Compared again with Difftastic once it was installed (done once).
    difftastic_retried: bool,
}

impl EventEmitter<TextEvent> for TextModel {}

impl TextModel {
    /// A model for `left` and `right`. It compares nothing until [`TextModel::run`].
    pub fn new(backend: Arc<dyn Backend>, left: Side, right: Side) -> Self {
        Self {
            backend,
            sides: [left, right],
            options: CompareOptions::default(),
            state: State::Loading,
            analysis: None,
            edits: [None, None],
            history: Vec::new(),
            disk_meta: None,
            disk_hash: [None, None],
            saving: [false; 2],
            generation: 0,
            running: false,
            difftastic_retried: false,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// The last comparison.
    pub fn analysis(&self) -> Option<Arc<Analysis>> {
        self.analysis.clone()
    }

    pub fn options(&self) -> CompareOptions {
        self.options
    }

    pub fn side(&self, side: usize) -> &Side {
        &self.sides[side.min(1)]
    }

    /// Whether a comparison is under way (what is shown is the one before).
    pub fn running(&self) -> bool {
        self.running
    }

    /// Whether `side` has edits not saved yet.
    pub fn dirty_side(&self, side: usize) -> bool {
        self.edits.get(side).is_some_and(Option::is_some)
    }

    /// Whether either side has edits not saved yet: closing would lose them.
    pub fn dirty(&self) -> bool {
        self.edits.iter().any(Option::is_some)
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Whether a save of `side` is under way.
    pub fn saving(&self, side: usize) -> bool {
        self.saving.get(side).copied().unwrap_or(false)
    }

    /// Whether `side` can be edited and saved here (`canEditLeft`): not read-only, and UTF-8 (a side that
    /// isn't shows � where it isn't, and every such byte would be one when saved).
    pub fn can_edit(&self, side: usize) -> bool {
        let utf8 = self.analysis.as_ref().is_none_or(|a| {
            if side == 0 {
                a.compare.left_utf8
            } else {
                a.compare.right_utf8
            }
        });
        !self.side(side).read_only && utf8
    }

    /// Compares the two files (with the edits made here), in the background (`run`). The comparison already
    /// shown stays until the new one arrives.
    pub fn run(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.running = true;
        if matches!(self.state, State::Failed(_)) {
            self.state = if self.analysis.is_some() {
                State::Ready
            } else {
                State::Loading
            };
        }
        let meta = self.disk_meta.unwrap_or_default();
        let edits = self.edits.clone();
        let backend = self.backend.clone();
        let (left, right) = (self.sides[0].path.clone(), self.sides[1].path.clone());
        let options = self.options;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Joining a hundred thousand lines is not for the UI thread either.
            let overrides = cx
                .background_executor()
                .spawn(async move {
                    let text = |side: usize| {
                        edits[side]
                            .as_ref()
                            .map(|lines| serialize_lines(lines, meta.crlf[side], meta.newline[side]))
                    };
                    (text(0), text(1))
                })
                .await;
            let found = core_bridge::run(backend.compare_text(&left, &right, options, overrides.0, overrides.1))
                .await
                .and_then(|inner| inner);
            let analysed = match found {
                Ok(compare) => Ok(cx
                    .background_executor()
                    .spawn(async move { Arc::new(Analysis::of(compare)) })
                    .await),
                Err(reason) => Err(reason),
            };
            let _ = this.update(cx, |this, cx| this.arrived(generation, analysed, cx));
        })
        .detach();
    }

    fn arrived(&mut self, generation: u64, result: Result<Arc<Analysis>, String>, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.running = false;
        match result {
            Ok(analysis) => {
                let compare = &analysis.compare;
                if self.disk_meta.is_none() {
                    self.disk_meta = Some(DiskMeta {
                        crlf: [compare.left_crlf, compare.right_crlf],
                        newline: [compare.left_newline, compare.right_newline],
                    });
                }
                if let Some(hash) = &compare.left_hash {
                    self.disk_hash[0] = Some(hash.clone());
                }
                if let Some(hash) = &compare.right_hash {
                    self.disk_hash[1] = Some(hash.clone());
                }
                self.analysis = Some(analysis);
                self.state = State::Ready;
                cx.emit(TextEvent::Compared);
            }
            Err(reason) => self.state = State::Failed(reason.into()),
        }
        cx.notify();
    }

    /// Changes how the files are compared (whitespace, case, line mode) and compares again.
    pub fn set_options(&mut self, options: CompareOptions, cx: &mut Context<Self>) {
        if options != self.options {
            self.options = options;
            self.run(cx);
        }
    }

    /// Difftastic is there now: compare again with it, once, unless the options or the edits rule it out.
    pub fn difftastic_installed(&mut self, cx: &mut Context<Self>) {
        let lines = self.analysis.as_ref().is_some_and(|a| a.compare.engine == "lines");
        let plain = !self.options.line_mode
            && self.options.ignore_whitespace == IgnoreWhitespace::None
            && !self.options.ignore_case
            && !self.dirty();
        if lines && plain && !self.difftastic_retried {
            self.difftastic_retried = true;
            self.run(cx);
        }
    }

    /// Remembers `edits` for Undo and sets the new ones; compares again.
    fn edited(&mut self, edits: [Option<Lines>; 2], cx: &mut Context<Self>) {
        self.history.push(std::mem::replace(&mut self.edits, edits));
        self.run(cx);
    }

    /// Copies a block's lines across (`copyBlock`): from the left file to the right one when `to_right`, else
    /// the other way. False when it can't be done: no such block, the target can't be edited, or the blocks
    /// belong to a comparison that is being replaced.
    pub fn copy_block(&mut self, block: usize, to_right: bool, cx: &mut Context<Self>) -> bool {
        let Some(analysis) = self.analysis.clone() else {
            return false;
        };
        let (from, to) = if to_right { (0, 1) } else { (1, 0) };
        if self.running || !self.can_edit(to) {
            return false;
        }
        let Some(block) = analysis.blocks.get(block) else {
            return false;
        };
        let (from_range, to_range) = if to_right {
            (block.left_range, block.right_range)
        } else {
            (block.right_range, block.left_range)
        };
        let current = |side: usize| -> Vec<String> {
            self.edits[side]
                .as_ref()
                .map_or_else(|| analysis.lines(side).to_vec(), |lines| lines.to_vec())
        };
        let source = current(from);
        let start = from_range.0.min(source.len());
        let end = from_range.1.min(source.len()).max(start);
        let replacement = source[start..end].to_vec();
        let merged = splice_lines(&current(to), to_range, &replacement);
        let mut edits = self.edits.clone();
        edits[to] = Some(Arc::new(merged));
        self.edited(edits, cx);
        true
    }

    /// Sets line `line` of `side` to `value` (`commitLineEdit`). Nothing happens if it is that already.
    pub fn edit_line(&mut self, side: usize, line: usize, value: String, cx: &mut Context<Self>) -> bool {
        let Some(analysis) = self.analysis.clone() else {
            return false;
        };
        if self.running || side > 1 || !self.can_edit(side) {
            return false;
        }
        let mut lines: Vec<String> = self.edits[side]
            .as_ref()
            .map_or_else(|| analysis.lines(side).to_vec(), |lines| lines.to_vec());
        match lines.get_mut(line) {
            Some(current) if *current != value => *current = value,
            _ => return false,
        }
        let mut edits = self.edits.clone();
        edits[side] = Some(Arc::new(lines));
        self.edited(edits, cx);
        true
    }

    /// Takes back the last merge or edit (`undo`).
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(before) = self.history.pop() {
            self.edits = before;
            self.run(cx);
        }
    }

    /// Saves `side`'s edits to its file (`saveSide`): refused if the file changed on disk since it was read,
    /// unless `overwrite`. Tells the pane how it went.
    pub fn save(&mut self, side: usize, overwrite: bool, cx: &mut Context<Self>) {
        let Some(lines) = self.edits.get(side).cloned().flatten() else {
            return;
        };
        let target = self.sides[side].clone();
        if target.read_only || self.saving[side] {
            return;
        }
        let meta = self.disk_meta.unwrap_or_default();
        let expected = if overwrite { None } else { self.disk_hash[side].clone() };
        self.saving[side] = true;
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let saved = lines.clone();
            let content = cx
                .background_executor()
                .spawn(async move { serialize_lines(&saved, meta.crlf[side], meta.newline[side]) })
                .await;
            let wrote = core_bridge::run(backend.write(&target.path, content, expected))
                .await
                .and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.saving[side] = false;
                match wrote {
                    Ok(hash) => {
                        this.disk_hash[side] = Some(hash);
                        // Edits made while it was being saved stay.
                        if this.edits[side].as_ref().is_some_and(|now| Arc::ptr_eq(now, &lines)) {
                            this.edits[side] = None;
                        }
                        cx.emit(TextEvent::Saved(target.name()));
                        this.run(cx);
                    }
                    Err(reason) => {
                        let changed = reason.contains("since it was compared");
                        cx.emit(TextEvent::SaveFailed {
                            side,
                            name: target.name(),
                            reason,
                            changed,
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves every side that has edits (Ctrl+S).
    pub fn save_all(&mut self, cx: &mut Context<Self>) {
        for side in 0..2 {
            if self.dirty_side(side) {
                self.save(side, false, cx);
            }
        }
    }
}

// ---- Small drawing helpers ---------------------------------------------------------------------------------

/// The color of a side: red for the left file, green for the right (`bg-red-500`, `bg-emerald-500`).
fn side_tone(side: usize, t: &theme::Tokens) -> Hsla {
    if side == 0 {
        theme::tailwind("red-500").unwrap_or(t.destructive)
    } else {
        theme::tailwind("emerald-500").unwrap_or(t.success)
    }
}

/// What a hit of the find box is drawn on (`bg-amber-400/55`, 40 in the dark).
fn find_tone(t: &theme::Tokens) -> Hsla {
    theme::tailwind("amber-400")
        .unwrap_or(t.warning)
        .opacity(if t.dark { 0.40 } else { 0.55 })
}

/// A short text for a screen reader: at most `max` characters of `text`.
fn clipped(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max).collect();
        cut.push('…');
        cut
    }
}

/// Scrolls a list a page (0.85 of what shows), as Page Down and Page Up do. Returns whether it moved.
// As the text preview's own (private to it).
fn page_list(handle: &UniformListScrollHandle, down: bool) -> bool {
    let base = handle.0.borrow().base_handle.clone();
    let max = base.max_offset().y;
    let viewport = base.bounds().size.height;
    if max <= px(0.) {
        return false;
    }
    let offset = base.offset();
    let step = viewport * 0.85;
    let top = if down { offset.y - step } else { offset.y + step }.clamp(-max, px(0.));
    if top == offset.y {
        return false;
    }
    base.set_offset(point(offset.x, top));
    true
}

/// A small icon button, a row tall: the copy arrows in a row's middle.
fn tiny_button(
    id: impl Into<ElementId>,
    glyph: IconName,
    label: SharedString,
    t: &theme::Tokens,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (hover, ink) = (t.primary.opacity(0.2), t.primary);
    let tip = label.clone();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .test_support()
        .size(space(4.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::size::RADIUS_SM)
        .text_color(t.muted_foreground)
        .cursor_pointer()
        .hover(move |style| style.bg(hover).text_color(ink))
        .tooltip(move |window, cx| tooltip(tip.clone(), window, cx))
        .on_click(move |_, window, cx| {
            // The row under it also takes clicks (to make its block the current one).
            cx.stop_propagation();
            on_click(window, cx);
        })
        .child(icon(glyph).size_3())
}

/// A pill-shaped group of options in the toolbar (`bg-foreground/[0.05] p-0.5`).
fn pill(t: &theme::Tokens) -> Div {
    row()
        .gap(space(0.5))
        .p(space(0.5))
        .rounded(theme::size::RADIUS)
        .bg(t.foreground.opacity(0.05))
}

// ---- The text view ---------------------------------------------------------------------------------------

/// What the text pane asks of the dialog around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneEvent {
    /// "Compare their bytes": the files aren't text (or are too big to be).
    ShowBytes,
}

/// A line being edited here (`editingCell`).
struct Editing {
    side: usize,
    line: usize,
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// The find box (`searchOpen`, `searchQuery`, `searchIdx`).
struct Find {
    open: bool,
    input: Entity<InputState>,
    query: String,
    /// Rows where either side has the query.
    matches: Arc<Vec<usize>>,
    /// Which match was jumped to last (none until Enter).
    current: Option<usize>,
    generation: u64,
    _typing: Subscription,
}

/// What a frame of the list needs, so the rows can be built without borrowing the pane.
struct Frame {
    analysis: Arc<Analysis>,
    items: Arc<Vec<Item>>,
    t: theme::Tokens,
    mono: SharedString,
    can_edit: [bool; 2],
    at: Option<usize>,
    query: SharedString,
    editing: Option<(usize, usize, Entity<InputState>)>,
}

/// The side-by-side text of two files with its toolbar, find box and minimap (`TextView`).
pub struct TextPane {
    model: Entity<TextModel>,
    tools: Option<Entity<Tools>>,
    /// The dialog's focus: where the keyboard goes when a row or a button is used, so Alt+Down and Ctrl+S work.
    focus: FocusHandle,
    filter: RowFilter,
    expanded: HashSet<usize>,
    /// The comparison the rows are drawn from.
    analysis: Option<Arc<Analysis>>,
    items: Arc<Vec<Item>>,
    /// The current change (`at`).
    at: Option<usize>,
    scroll: UniformListScrollHandle,
    find: Find,
    editing: Option<Editing>,
    /// Where the minimap was drawn last, for turning a click into a row.
    minimap: Rc<Cell<Bounds<Pixels>>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaneEvent> for TextPane {}

impl TextPane {
    /// A pane showing `model`, which it asks to compare. `tools` is the Difftastic install button's model.
    pub fn new(
        model: Entity<TextModel>,
        tools: Option<Entity<Tools>>,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Find text in either file…")));
        let typing = cx.subscribe_in(&input, window, Self::on_find_input);
        let mut subscriptions = vec![cx.subscribe_in(&model, window, Self::on_model)];
        if let Some(tools) = &tools {
            subscriptions.push(cx.observe(tools, |this, tools, cx| {
                let installed = tools.read(cx).tool("difftastic").is_some_and(|t| t.origin.is_some());
                if installed {
                    this.model.update(cx, |model, cx| model.difftastic_installed(cx));
                }
            }));
        }
        let pane = Self {
            model,
            tools,
            focus,
            filter: RowFilter::All,
            expanded: HashSet::new(),
            analysis: None,
            items: Arc::new(Vec::new()),
            at: None,
            scroll: UniformListScrollHandle::new(),
            find: Find {
                open: false,
                input,
                query: String::new(),
                matches: Arc::new(Vec::new()),
                current: None,
                generation: 0,
                _typing: typing,
            },
            editing: None,
            minimap: Rc::new(Cell::new(Bounds::default())),
            _subscriptions: subscriptions,
        };
        pane.model.update(cx, |model, cx| model.run(cx));
        pane
    }

    pub fn model(&self) -> &Entity<TextModel> {
        &self.model
    }

    /// Whether there are edits not saved yet.
    pub fn dirty(&self, cx: &App) -> bool {
        self.model.read(cx).dirty()
    }

    /// The current change, 0-based.
    #[cfg(test)]
    pub fn current_change(&self) -> Option<usize> {
        self.at
    }

    /// The rows the list shows (a fold is one).
    pub fn shown(&self) -> &[Item] {
        &self.items
    }

    // ---- The model's news ------------------------------------------------------------------------------

    fn on_model(&mut self, _: &Entity<TextModel>, event: &TextEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextEvent::Compared => {
                self.analysis = self.model.read(cx).analysis();
                self.rebuild_items();
                self.settle_current();
                self.refresh_matches(cx);
            }
            TextEvent::Saved(name) => {
                window.push_notification(
                    Notification::success(t!("Saved {name}", name = isolate(name.clone()))),
                    cx,
                );
            }
            TextEvent::SaveFailed {
                side,
                name,
                reason,
                changed,
            } => {
                if *changed {
                    let (side, model) = (*side, self.model.clone());
                    let question = Question {
                        title: t!("Couldn't save {name}", name = isolate(name.clone())),
                        message: t!(
                            "{name} was changed or removed since it was compared. Save over it anyway?",
                            name = isolate(name.clone())
                        ),
                        ok: t!("Save anyway"),
                        cancel: t!("Cancel"),
                        danger: true,
                    };
                    ask(window, cx, question, move |_, cx| {
                        model.update(cx, |model, cx| model.save(side, true, cx))
                    });
                } else {
                    window.push_notification(
                        Notification::error(t!(
                            "Couldn't save {name}: {reason}",
                            name = isolate(name.clone()),
                            reason = reason.clone()
                        )),
                        cx,
                    );
                }
            }
        }
    }

    /// Works out which rows are shown (`visibleItems`).
    fn rebuild_items(&mut self) {
        let items = match &self.analysis {
            Some(analysis) => visible_items(analysis, self.filter, &self.expanded),
            None => Vec::new(),
        };
        self.items = Arc::new(items);
    }

    /// Keeps the current change within the blocks and in view (the effect on `blocks`): after a copy the
    /// change that was current is gone and the next one has taken its number.
    fn settle_current(&mut self) {
        let blocks = self.block_count();
        self.at = if blocks == 0 {
            None
        } else {
            Some(self.at.map_or(0, |at| at.min(blocks - 1)))
        };
        if let Some(at) = self.at {
            self.reveal_block(at, false);
        }
    }

    /// Scrolls so the change `block` shows; `strict` puts it in the middle even when it is in view already.
    fn reveal_block(&self, block: usize, strict: bool) {
        let Some(row) = self.start_row(block) else {
            return;
        };
        self.reveal_row(row, strict);
    }

    fn start_row(&self, block: usize) -> Option<usize> {
        self.analysis
            .as_ref()
            .and_then(|analysis| analysis.blocks.get(block).map(|b| b.start_row))
    }

    fn reveal_row(&self, row: usize, strict: bool) {
        if let Some(at) = visible_index_of(&self.items, row) {
            if strict {
                self.scroll.scroll_to_item_strict(at, ScrollStrategy::Center);
            } else {
                self.scroll.scroll_to_item(at, ScrollStrategy::Center);
            }
        }
    }

    // ---- Changes ----------------------------------------------------------------------------------------

    fn block_count(&self) -> usize {
        self.analysis.as_ref().map_or(0, |a| a.blocks.len())
    }

    /// Makes change `block` (clamped) the current one and shows it (`selectBlock`).
    pub fn select_block(&mut self, block: usize, cx: &mut Context<Self>) {
        let count = self.block_count();
        if count == 0 {
            self.at = None;
            return;
        }
        let at = block.min(count - 1);
        self.at = Some(at);
        self.reveal_block(at, true);
        cx.notify();
    }

    /// The next (`1`) or previous (`-1`) change (`go`).
    pub fn go(&mut self, step: isize, cx: &mut Context<Self>) {
        if self.block_count() == 0 {
            return;
        }
        let from = self.at.unwrap_or(0);
        self.select_block(from.saturating_add_signed(step), cx);
    }

    /// Copies the current change to the right (or left) file.
    pub fn copy_current(&mut self, to_right: bool, cx: &mut Context<Self>) {
        if let Some(at) = self.at {
            self.model.update(cx, |model, cx| {
                model.copy_block(at, to_right, cx);
            });
        }
    }

    fn set_filter(&mut self, filter: RowFilter, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.filter = filter;
        self.rebuild_items();
        if let Some(at) = self.at {
            self.reveal_block(at, false);
        }
        cx.notify();
    }

    fn unfold(&mut self, start: usize, cx: &mut Context<Self>) {
        self.expanded.insert(start);
        self.rebuild_items();
        cx.notify();
    }

    // ---- Editing a line ----------------------------------------------------------------------------------

    fn start_edit(&mut self, side: usize, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model.read(cx).can_edit(side) {
            return;
        }
        self.commit_edit(window, cx);
        let Some(analysis) = self.model.read(cx).analysis() else {
            return;
        };
        let Some(text) = analysis.lines(side).get(line).cloned() else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_edit(window, cx),
            _ => {}
        });
        self.editing = Some(Editing {
            side,
            line,
            input,
            _subscription: subscription,
        });
        cx.defer_in(window, |this, window, cx| {
            if let Some(editing) = &this.editing {
                editing.input.update(cx, |input, cx| input.focus(window, cx));
            }
        });
        cx.notify();
    }

    /// The edit goes in (on Enter, or as the box loses the keyboard) (`commitLineEdit`).
    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let value = editing.input.read(cx).value().to_string();
        let (side, line) = (editing.side, editing.line);
        self.model.update(cx, |model, cx| {
            model.edit_line(side, line, value, cx);
        });
        if editing.input.read(cx).focus_handle(cx).is_focused(window) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.editing.take().is_none() {
            return false;
        }
        window.focus(&self.focus, cx);
        cx.notify();
        true
    }

    /// Saves the edits (Ctrl+S): one being typed goes in first.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(window, cx);
        self.model.update(cx, |model, cx| model.save_all(cx));
    }

    /// Takes back the last merge or edit.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.undo(cx));
    }

    // ---- Find -------------------------------------------------------------------------------------------

    /// Opens the find box and puts the keyboard in it (Ctrl+F).
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = true;
        let input = self.find.input.clone();
        cx.defer_in(window, move |_, window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }

    /// Closes the find box and forgets its query.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = false;
        self.find.query.clear();
        self.find.matches = Arc::new(Vec::new());
        self.find.current = None;
        self.find.generation += 1;
        self.find.input.update(cx, |input, cx| input.set_value("", window, cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn on_find_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.find.query = input.read(cx).value().to_string();
                self.find.current = None;
                self.refresh_matches(cx);
            }
            InputEvent::PressEnter { shift, .. } => self.find_step(if *shift { -1 } else { 1 }, window, cx),
            _ => {}
        }
    }

    /// Looks for the query in the background (`searchRowMatches`).
    fn refresh_matches(&mut self, cx: &mut Context<Self>) {
        self.find.generation += 1;
        let generation = self.find.generation;
        let query = self.find.query.clone();
        let Some(analysis) = self.model.read(cx).analysis().filter(|_| !query.is_empty()) else {
            self.find.matches = Arc::new(Vec::new());
            self.find.current = None;
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move { matching_rows(&analysis.compare, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.find.generation == generation {
                    this.find.matches = Arc::new(rows);
                    if this.find.current.is_some_and(|c| c >= this.find.matches.len()) {
                        this.find.current = None;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Goes to the next (`1`) or previous (`-1`) matching row.
    fn find_step(&mut self, step: isize, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.find.matches.len();
        if count == 0 {
            return;
        }
        let next = match (self.find.current, step > 0) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
        };
        self.find.current = Some(next);
        if let Some(&row) = self.find.matches.get(next) {
            self.reveal_row(row, true);
        }
        cx.notify();
    }

    // ---- Clicks -----------------------------------------------------------------------------------------

    /// A click on a half: its change becomes the current one; a double click edits the line.
    fn pick(
        &mut self,
        side: usize,
        line: usize,
        block: Option<usize>,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.as_ref().is_some_and(|e| e.side == side && e.line == line) {
            return;
        }
        window.focus(&self.focus, cx);
        if let Some(block) = block {
            self.at = Some(block);
        }
        if clicks >= 2 {
            self.start_edit(side, line, window, cx);
        }
        cx.notify();
    }

    /// A click on the minimap at `y`: the change nearest that part of the file (`onClick` of the strip).
    fn minimap_click(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(analysis) = self.model.read(cx).analysis() else {
            return;
        };
        let bounds = self.minimap.get();
        let total = analysis.compare.rows.len();
        if analysis.blocks.is_empty() || total == 0 || bounds.size.height <= px(0.) {
            return;
        }
        let ratio = ((y - bounds.origin.y) / bounds.size.height).clamp(0., 1.);
        let target = ratio * total as f32;
        let best = analysis
            .blocks
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |block: &ChangeBlock| ((block.start_row + block.end_row) as f32 / 2. - target).abs();
                distance(a).total_cmp(&distance(b))
            })
            .map_or(0, |(index, _)| index);
        self.select_block(best, cx);
    }

    // ---- Drawing: the rows -------------------------------------------------------------------------------

    /// The list's line `vix`.
    fn row(&mut self, vix: usize, frame: &Frame, cx: &mut Context<Self>) -> AnyElement {
        let t = &frame.t;
        match frame.items.get(vix).copied() {
            None => div().into_any_element(),
            Some(Item::Fold { start, count, .. }) => {
                let label = t!("{n} unchanged lines — click to expand", n = format_count(count));
                row()
                    .id(("compare-fold", vix))
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .test_support()
                    .h(LINE)
                    .w_full()
                    .gap(space(1.5))
                    .justify_center()
                    .bg(t.muted.opacity(0.4))
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .cursor_pointer()
                    .hover({
                        let (bg, ink) = (t.muted.opacity(0.8), t.foreground);
                        move |style| style.bg(bg).text_color(ink)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.unfold(start, cx)))
                    .child(icon(IconName::UnfoldVertical).size_3())
                    .child(label)
                    .into_any_element()
            }
            Some(Item::Row {
                row: row_ix,
                block,
                is_block_start,
            }) => {
                let [l, r] = frame.analysis.compare.rows.get(row_ix).copied().unwrap_or([None, None]);
                let current = block.is_some() && block == frame.at;
                let left = self.half(vix, 0, l, block, frame, cx);
                let right = self.half(vix, 1, r, block, frame, cx);
                let describe = |side: usize, line: Option<u32>| -> String {
                    match line.and_then(|n| frame.analysis.lines(side).get(n as usize).map(|text| (n, text))) {
                        Some((n, text)) => format!("{}: {}", n + 1, clipped(text, 120)),
                        None => String::new(),
                    }
                };
                let label = format!("{} | {}", describe(0, l), describe(1, r));
                let gutter = self.gutter(vix, block, is_block_start || current, frame, cx);
                row()
                    .id(("compare-row", vix))
                    .role(Role::Row)
                    .aria_label(label)
                    .when(block.is_some(), |this| this.aria_selected(current))
                    .test_support()
                    .h(LINE)
                    .w_full()
                    .flex_none()
                    .when(current, |this| {
                        this.bg(t.primary.opacity(0.04))
                            .border_1()
                            .border_color(t.primary.opacity(0.4))
                    })
                    .child(left)
                    .child(gutter)
                    .child(right)
                    .into_any_element()
            }
        }
    }

    /// The middle strip of a row: the arrows that copy its change across.
    fn gutter(
        &self,
        vix: usize,
        block: Option<usize>,
        arrows: bool,
        frame: &Frame,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &frame.t;
        let model = self.model.clone();
        let buttons: Vec<AnyElement> = match block.filter(|_| arrows) {
            None => Vec::new(),
            Some(block) => {
                // The files keep their places in the Arabic interface (see the module docs), so the arrows
                // point where the files are.
                let mut out = Vec::new();
                for (to_right, glyph, label) in [
                    (true, IconName::ArrowRight, t!("Copy change to right (Alt+Right)")),
                    (false, IconName::ArrowLeft, t!("Copy change to left (Alt+Left)")),
                ] {
                    if !frame.can_edit[usize::from(to_right)] {
                        continue;
                    }
                    let model = model.clone();
                    out.push(
                        tiny_button(
                            ("compare-copy", vix * 2 + usize::from(to_right)),
                            glyph,
                            label,
                            t,
                            move |_, cx| {
                                model.update(cx, |model, cx| {
                                    model.copy_block(block, to_right, cx);
                                });
                            },
                        )
                        .into_any_element(),
                    );
                }
                out
            }
        };
        let _ = cx;
        row()
            .w(space(9.))
            .h_full()
            .flex_none()
            .justify_center()
            .gap(space(0.5))
            .border_l_1()
            .border_r_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.4))
            .children(buttons)
    }

    /// One half of a row: the line number and the line, tinted when it changed, with its changed words marked.
    fn half(
        &mut self,
        vix: usize,
        side: usize,
        line: Option<u32>,
        block: Option<usize>,
        frame: &Frame,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &frame.t;
        let Some(line) = line.map(|l| l as usize) else {
            // Nothing on this side across from the other side's line.
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(t.muted.opacity(0.5))
                .into_any_element();
        };
        let text = frame.analysis.lines(side).get(line).map_or("", String::as_str);
        let changes = frame.analysis.changes(side, line);
        let tone = side_tone(side, t);
        let editing_here = frame
            .editing
            .as_ref()
            .filter(|(s, l, _)| *s == side && *l == line)
            .map(|(_, _, input)| input.clone());
        let content: AnyElement = match editing_here {
            Some(input) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(t.background)
                .border_1()
                .border_color(t.primary)
                .child(
                    Input::new(&input)
                        .xsmall()
                        .appearance(false)
                        .bordered(false)
                        .aria_label(t!("Line {n}", n = line + 1)),
                )
                .into_any_element(),
            None => {
                let shaped = Shaped::new(text);
                let finds = if frame.query.is_empty() {
                    Vec::new()
                } else {
                    find_spans_in_line(text, &frame.query)
                };
                let runs = marks(&shaped, changes.map_or(&[][..], Vec::as_slice), &finds);
                let (change_color, find_color) = (tone.opacity(0.35), find_tone(t));
                let highlights: Vec<(Range<usize>, HighlightStyle)> = runs
                    .into_iter()
                    .map(|(range, mark)| {
                        let color = if mark == Mark::Find { find_color } else { change_color };
                        (
                            range,
                            HighlightStyle {
                                background_color: Some(color),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                let shown = if shaped.text.is_empty() {
                    " ".to_string()
                } else {
                    shaped.text
                };
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pr(space(2.))
                    .child(StyledText::new(shown).with_highlights(highlights))
                    .into_any_element()
            }
        };
        row()
            .id(("compare-half", vix * 2 + side))
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .when(changes.is_some(), |this| this.bg(tone.opacity(0.10)))
            .child(
                div()
                    .w(space(12.))
                    .flex_none()
                    .pr(space(2.))
                    .text_right()
                    .text_color(t.muted_foreground.opacity(0.7))
                    .child((line + 1).to_string()),
            )
            .child(content)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.pick(side, line, block, event.click_count(), window, cx);
            }))
            .into_any_element()
    }
}

// ---- The text view: drawing -------------------------------------------------------------------------------

impl TextPane {
    /// A message in the middle of the pane, named for a screen reader by the message itself.
    fn centered(
        id: &'static str,
        role: Role,
        text: SharedString,
        t: &theme::Tokens,
    ) -> impl IntoElement + ParentElement {
        col()
            .id(id)
            .role(role)
            .aria_label(text.clone())
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(space(2.))
            .p(space(4.))
            .text_center()
            .text_size(theme::size::SM)
            .text_color(t.muted_foreground)
    }

    /// Both files' names, with the Modified and Read-only badges and the Save buttons.
    fn side_headers(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let headers: Vec<AnyElement> = (0..2)
            .map(|n| {
                let side = model.side(n).clone();
                let dirty = model.dirty_side(n);
                let can_edit = model.can_edit(n);
                let saving = model.saving(n);
                let name = side.name();
                let badge = |text: SharedString, tint: Hsla, ink: Hsla| {
                    div()
                        .flex_none()
                        .px(space(1.5))
                        .py(space(0.5))
                        .rounded(theme::size::RADIUS_SM)
                        .bg(tint)
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ink)
                        .child(text)
                };
                let tone = side_tone(n, t);
                let model_entity = self.model.clone();
                row()
                    .id(("compare-side", n))
                    .role(Role::Group)
                    .aria_label(name.clone())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .gap(space(2.))
                    .px(space(3.5))
                    .py(space(1.5))
                    .when(n == 1, |this| this.border_l_1().border_color(t.border))
                    .child(div().size_2().flex_none().rounded_full().bg(tone.opacity(0.7)))
                    .child(
                        div()
                            .id(("compare-side-name", n))
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .tooltip({
                                let path = SharedString::from(side.path.clone());
                                move |window, cx| tooltip(path.clone(), window, cx)
                            })
                            .child(isolate(name)),
                    )
                    .when(side.label.is_some(), |this| {
                        this.child(
                            div()
                                .truncate()
                                .text_color(t.muted_foreground)
                                .child(isolate(base_name(&side.path).to_string())),
                        )
                    })
                    .when(dirty, |this| {
                        let amber = theme::tailwind("amber-500").unwrap_or(t.warning);
                        let text = t!("Modified");
                        this.child(
                            badge(text.clone(), amber.opacity(0.15), t.warning)
                                .id(("compare-modified", n))
                                .role(Role::Status)
                                .aria_label(text)
                                .test_support(),
                        )
                    })
                    .when(!can_edit && !side.read_only, |this| {
                        let text = t!("Read-only: not UTF-8");
                        this.child(
                            badge(text.clone(), t.foreground.opacity(0.06), t.muted_foreground)
                                .id(("compare-not-utf8", n))
                                .role(Role::Status)
                                .aria_label(text)
                                .test_support()
                                .tooltip(move |window, cx| {
                                    tooltip(
                                        t!(
                                            "Its text isn't UTF-8: it shows � where Glint can't read it, so it isn't edited or saved here"
                                        ),
                                        window,
                                        cx,
                                    )
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .when(dirty && can_edit, |this| {
                        this.child(
                            Button::new(("compare-save", n))
                                .primary()
                                .xsmall()
                                .icon(IconName::Save)
                                .label(t!("Save"))
                                .loading(saving)
                                .disabled(saving)
                                .tooltip(t!("Save changes to disk (Ctrl+S)"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.commit_edit(window, cx);
                                    model_entity.update(cx, |model, cx| model.save(n, false, cx));
                                })),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        row()
            .id("compare-sides")
            .w_full()
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.6))
            .text_size(theme::size::XS)
            .children(headers)
    }

    /// The toolbar under the names: what changed, how it was compared, the options, Undo, Find and the
    /// change list's next and previous.
    fn toolbar(&self, analysis: &Analysis, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let options = model.options();
        let can_undo = model.can_undo();
        let dirty = model.dirty();
        let blocks = analysis.blocks.len();
        let engine_difftastic = analysis.compare.engine == "difftastic";

        let summary: AnyElement = if blocks > 0 {
            let changes = plural(blocks, "change");
            let removed = format!("−{}", format_count(analysis.removed));
            let added = format!("+{}", format_count(analysis.added));
            let lines = t!("lines");
            row()
                .id("compare-summary")
                .role(Role::Status)
                .aria_label(format!("{changes} {removed} {added} {lines}"))
                .test_support()
                .flex_none()
                .gap(space(1.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(changes)
                .child(div().text_color(t.destructive).child(removed))
                .child(div().text_color(t.success).child(added))
                .child(lines)
                .into_any_element()
        } else {
            let text = if engine_difftastic {
                t!("The same code")
            } else {
                t!("The same text")
            };
            row()
                .id("compare-summary")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .flex_none()
                .gap(space(1.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(icon(IconName::CircleCheck).size_3p5().text_color(t.success))
                .child(text)
                .into_any_element()
        };

        let engine = if engine_difftastic {
            match &analysis.compare.language {
                Some(language) => t!("Difftastic, as {language}", language = language.clone()),
                None => "Difftastic".into(),
            }
        } else {
            t!("line by line")
        };
        let engine_tip = if engine_difftastic {
            t!("Difftastic compared them by the code's structure: a change of layout alone isn't one")
        } else {
            t!("Compared line by line with intra-line word & character highlighting")
        };
        let engine_info = div()
            .id("compare-engine")
            .truncate()
            .min_w_0()
            .tooltip(move |window, cx| tooltip(engine_tip.clone(), window, cx))
            .child(t!(
                "· {engine} in {time}",
                engine = engine,
                time = format_duration(analysis.compare.elapsed)
            ));

        // Difftastic: offered when it isn't there; its AST switch is shown once it is.
        let tool = self.tools.as_ref().and_then(|tools| {
            let read = tools.read(cx);
            let info = read.tool("difftastic")?;
            Some((tools.clone(), info.origin.is_some(), read.busy("difftastic").is_some()))
        });
        let with_difftastic = tool.as_ref().is_some_and(|(_, installed, _)| *installed);
        let install = tool
            .filter(|(_, installed, _)| !installed && analysis.compare.engine == "lines")
            .map(|(tools, _, busy)| {
                Button::new("compare-difftastic")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Download)
                    .label("Difftastic")
                    .loading(busy)
                    .disabled(busy)
                    .tooltip(t!(
                        "Difftastic compares code by its structure: what changed in it, not its layout. It installs with winget."
                    ))
                    .on_click(move |_, window, cx| {
                        tools.update(cx, |tools, cx| tools.install("difftastic".to_string(), window, cx));
                    })
            });

        let filters = pill(t)
            .id("compare-filters")
            .role(Role::Group)
            .aria_label(t!("Rows to show"))
            .test_support()
            .ms(2.)
            .children(
                [
                    (RowFilter::All, t!("All")),
                    (RowFilter::Context, t!("Context")),
                    (RowFilter::Diffs, t!("Diffs")),
                ]
                .into_iter()
                .enumerate()
                .map(|(n, (filter, label))| {
                    Button::new(("compare-filter", n))
                        .ghost()
                        .xsmall()
                        .selected(self.filter == filter)
                        .label(label.clone())
                        .accessibility_label(label)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
                }),
            );

        let (ws_label, ws_tip, ws_next) = match options.ignore_whitespace {
            IgnoreWhitespace::None => (
                t!("WS: Exact"),
                t!("Whitespace: comparing all whitespace (click to ignore trailing)"),
                IgnoreWhitespace::Trailing,
            ),
            IgnoreWhitespace::Trailing => (
                t!("WS: Trim"),
                t!("Whitespace: ignoring trailing whitespace (click to ignore all whitespace)"),
                IgnoreWhitespace::All,
            ),
            IgnoreWhitespace::All => (
                t!("WS: Ignore"),
                t!("Whitespace: ignoring all whitespace (click to compare all)"),
                IgnoreWhitespace::None,
            ),
        };
        let model_entity = self.model.clone();
        let ws = Button::new("compare-whitespace")
            .ghost()
            .xsmall()
            .selected(options.ignore_whitespace != IgnoreWhitespace::None)
            .label(ws_label)
            .tooltip(ws_tip.clone())
            .accessibility_label(ws_tip)
            .on_click(move |_, _, cx| {
                model_entity.update(cx, |model, cx| {
                    model.set_options(
                        CompareOptions {
                            ignore_whitespace: ws_next,
                            ..model.options()
                        },
                        cx,
                    )
                });
            });
        let model_entity = self.model.clone();
        let case = Button::new("compare-case")
            .ghost()
            .xsmall()
            .selected(options.ignore_case)
            .icon(IconName::CaseSensitive)
            .tooltip(t!("Ignore case differences"))
            .accessibility_label(t!("Ignore case differences"))
            .on_click(move |_, _, cx| {
                model_entity.update(cx, |model, cx| {
                    model.set_options(
                        CompareOptions {
                            ignore_case: !model.options().ignore_case,
                            ..model.options()
                        },
                        cx,
                    )
                });
            });
        let structural =
            !options.line_mode && options.ignore_whitespace == IgnoreWhitespace::None && !options.ignore_case && !dirty;
        let model_entity = self.model.clone();
        let ast = with_difftastic.then(|| {
            let tip = if options.line_mode {
                t!("Using line-by-line diff (click for Difftastic structural diff)")
            } else {
                t!("Using Difftastic structural diff (click for line-by-line diff)")
            };
            Button::new("compare-ast")
                .ghost()
                .xsmall()
                .selected(structural)
                .icon(IconName::Sparkles)
                .label("AST")
                .tooltip(tip.clone())
                .accessibility_label(tip)
                .on_click(move |_, _, cx| {
                    model_entity.update(cx, |model, cx| {
                        model.set_options(
                            CompareOptions {
                                line_mode: !model.options().line_mode,
                                ..model.options()
                            },
                            cx,
                        )
                    });
                })
        });

        let undo = can_undo.then(|| {
            Button::new("compare-undo")
                .ghost()
                .xsmall()
                .icon(IconName::RotateCcw)
                .label(t!("Undo"))
                .tooltip(t!("Undo last merge or edit (Ctrl+Z)"))
                .accessibility_label(t!("Undo last merge or edit (Ctrl+Z)"))
                .on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.undo(cx);
                }))
        });
        let find = Button::new("compare-find")
            .ghost()
            .xsmall()
            .selected(self.find.open)
            .icon(IconName::Search)
            .tooltip(t!("Find in files (Ctrl+F)"))
            .accessibility_label(t!("Find in files (Ctrl+F)"))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.find.open {
                    this.close_find(window, cx);
                } else {
                    this.open_find(window, cx);
                }
            }));
        let counter = (blocks > 0).then(|| {
            let text = self
                .at
                .map(|at| t!("{n} of {total}", n = at + 1, total = blocks))
                .unwrap_or_default();
            div()
                .id("compare-position")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .child(text)
        });
        let previous = Button::new("compare-previous")
            .ghost()
            .xsmall()
            .icon(IconName::ChevronUp)
            .disabled(blocks == 0 || self.at.is_none_or(|at| at == 0))
            .tooltip(t!("Previous change (Alt+Up)"))
            .accessibility_label(t!("Previous change (Alt+Up)"))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
                this.go(-1, cx);
            }));
        let next = Button::new("compare-next")
            .ghost()
            .xsmall()
            .icon(IconName::ChevronDown)
            .disabled(blocks == 0 || self.at.is_some_and(|at| at + 1 >= blocks))
            .tooltip(t!("Next change (Alt+Down)"))
            .accessibility_label(t!("Next change (Alt+Down)"))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
                this.go(1, cx);
            }));

        row_start()
            .id("compare-toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("Comparison"))
            .test_support()
            .w_full()
            .h(space(9.))
            .flex_none()
            .gap(space(2.))
            .px(space(3.5))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(summary)
            .child(engine_info)
            .children(install)
            .child(filters)
            .child(row_start().gap(space(1.)).child(ws).child(case).children(ast))
            .child(div().flex_1())
            .child(
                row_start()
                    .gap(space(1.))
                    .children(undo)
                    .child(find)
                    .children(counter)
                    .child(previous)
                    .child(next),
            )
    }

    /// The find box's bar (`Inline search bar`).
    fn find_bar(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.find.matches.len();
        let status = (!self.find.query.is_empty()).then(|| {
            let text = if count > 0 {
                t!(
                    "{n} of {matches}",
                    n = self.find.current.map_or(1, |c| c + 1).min(count),
                    matches = plural(count, "matching row")
                )
            } else {
                t!("No matches")
            };
            div()
                .id("compare-find-status")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .child(text)
        });
        row_start()
            .id("compare-find-bar")
            .role(Role::Search)
            .aria_label(t!("Find in files"))
            .test_support()
            .w_full()
            .h(space(10.))
            .flex_none()
            .gap(space(2.))
            .px(space(3.5))
            .border_b_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.4))
            .text_size(theme::size::XS)
            .child(icon(IconName::Search).size_3p5().text_color(t.muted_foreground))
            .child(
                Input::new(&self.find.input)
                    .small()
                    .w(space(64.))
                    .aria_label(t!("Find text in either file…")),
            )
            .children(status)
            .child(
                Button::new("compare-find-previous")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronUp)
                    .disabled(count == 0)
                    .tooltip(t!("Previous match (Shift+Enter)"))
                    .accessibility_label(t!("Previous match (Shift+Enter)"))
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(-1, window, cx))),
            )
            .child(
                Button::new("compare-find-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .disabled(count == 0)
                    .tooltip(t!("Next match (Enter)"))
                    .accessibility_label(t!("Next match (Enter)"))
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(1, window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("compare-find-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .tooltip(t!("Close find bar (Esc)"))
                    .accessibility_label(t!("Close find bar (Esc)"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
    }

    /// The minimap beside the list: a mark for every change, red for lines only the left has, green for the
    /// right's, amber for both; click to go there.
    fn minimap_strip(&self, analysis: &Arc<Analysis>, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let (blocks, at) = (analysis.clone(), self.at);
        let total = analysis.compare.rows.len().max(1) as f32;
        let colors = (
            theme::tailwind("amber-500").unwrap_or(t.warning),
            side_tone(0, t),
            side_tone(1, t),
            t.primary,
        );
        let cell = self.minimap.clone();
        div()
            .id("compare-minimap")
            .role(Role::Group)
            .aria_label(t!("Diff minimap — click to jump to change"))
            .test_support()
            .w(space(3.))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.5))
            .cursor_pointer()
            .tooltip(|window, cx| tooltip(t!("Diff minimap — click to jump to change"), window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| this.minimap_click(event.position.y, cx)),
            )
            .child(
                canvas(
                    move |bounds, _, _| cell.set(bounds),
                    move |bounds, _, window, _| {
                        let mut last: Option<(i32, usize)> = None;
                        for (index, block) in blocks.blocks.iter().enumerate() {
                            let top = bounds.size.height * (block.start_row as f32 / total);
                            let height =
                                (bounds.size.height * ((block.end_row - block.start_row) as f32 / total)).max(px(2.));
                            // Past a thousand changes many share a pixel; one quad per pixel is enough.
                            let bucket = f32::from(top).floor() as i32;
                            let kind = usize::from(block.left_range.1 > block.left_range.0) * 2
                                + usize::from(block.right_range.1 > block.right_range.0);
                            if index != at.unwrap_or(usize::MAX) && last == Some((bucket, kind)) {
                                continue;
                            }
                            last = Some((bucket, kind));
                            let color = match kind {
                                3 => colors.0,
                                2 => colors.1,
                                _ => colors.2,
                            }
                            .opacity(0.8);
                            let origin = point(bounds.origin.x + px(2.), bounds.origin.y + top);
                            let size = gpui_kit::size(bounds.size.width - px(4.), height);
                            let shade = if at == Some(index) { colors.3 } else { color };
                            window.paint_quad(fill(Bounds { origin, size }, shade));
                        }
                    },
                )
                .size_full(),
            )
    }
}

impl Render for TextPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.model.read(cx).state().clone();
        let analysis = self.analysis.clone();

        let body: AnyElement = match (state, analysis) {
            (State::Failed(reason), _) => Self::centered("compare-error", Role::Alert, reason.clone(), &t)
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(reason)
                .into_any_element(),
            (_, None) => Self::centered("compare-loading", Role::Status, t!("Comparing the files…"), &t)
                .child(Spinner::new().color(t.primary))
                .into_any_element(),
            (_, Some(analysis)) if analysis.compare.binary || analysis.compare.too_big => {
                let text = if analysis.compare.binary {
                    t!("They aren't text: their bytes can be compared.")
                } else {
                    t!("Too big to compare as text here (over 8 MB).")
                };
                Self::centered("compare-binary", Role::Status, text.clone(), &t)
                    .child(icon(IconName::Binary).size_10())
                    .child(text)
                    .child(
                        Button::new("compare-their-bytes")
                            .outline()
                            .small()
                            .icon(IconName::Binary)
                            .label(t!("Compare their bytes"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PaneEvent::ShowBytes))),
                    )
                    .into_any_element()
            }
            (_, Some(analysis)) => {
                let frame = Rc::new(Frame {
                    analysis: analysis.clone(),
                    items: self.items.clone(),
                    t: t.clone(),
                    mono: theme::mono_font(cx),
                    can_edit: {
                        let model = self.model.read(cx);
                        [model.can_edit(0), model.can_edit(1)]
                    },
                    at: self.at,
                    query: SharedString::from(self.find.query.clone()),
                    editing: self.editing.as_ref().map(|e| (e.side, e.line, e.input.clone())),
                });
                let list_frame = frame.clone();
                let list = uniform_list(
                    "compare-rows",
                    frame.items.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        range.map(|vix| this.row(vix, &list_frame, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full();
                let rows_area = div()
                    .id("compare-table")
                    .role(Role::Table)
                    .aria_label(t!("Side by side"))
                    .test_support()
                    .relative()
                    // Code and bytes read left to right even in the Arabic interface.
                    .text_left()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .font_family(frame.mono.clone())
                    .text_size(theme::size::XS)
                    .child(list)
                    .child(Scrollbar::vertical(&self.scroll).viewport_from_layout());
                col()
                    .size_full()
                    .min_h_0()
                    .child(self.side_headers(&t, cx))
                    .child(self.toolbar(&analysis, &t, cx))
                    .when(self.find.open, |this| this.child(self.find_bar(&t, cx)))
                    .child(
                        row()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(rows_area)
                            .when(!analysis.compare.rows.is_empty(), |this| {
                                this.child(self.minimap_strip(&analysis, &t, cx))
                            }),
                    )
                    .into_any_element()
            }
        };

        div()
            .id("compare-text")
            .size_full()
            .min_h_0()
            // Escape cancels a line's edit, or closes the find box it is typed in; otherwise it closes the dialog.
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                if this.cancel_edit(window, cx) {
                    return;
                }
                if this.find.open && this.find.input.read(cx).focus_handle(cx).is_focused(window) {
                    this.close_find(window, cx);
                    return;
                }
                cx.propagate();
            }))
            .child(body)
    }
}

// ---- The bytes view ---------------------------------------------------------------------------------------

/// A window of bytes: being read, or here.
enum Slot {
    Loading,
    Ready(Arc<BytesWindow>),
}

/// Both files' bytes side by side in hex, 16 to a row, with the next and previous place they differ
/// (`BytesView`). The bytes are read 64 KB at a time as rows are drawn.
pub struct BytesPane {
    backend: Arc<dyn Backend>,
    left: String,
    right: String,
    windows: HashMap<u64, Slot>,
    /// Both files' sizes, once the first window is in.
    sizes: Option<(u64, u64)>,
    error: Option<SharedString>,
    /// The difference the list is at (`shown`).
    shown: Option<u64>,
    /// Where the first difference is: `None` while looking, `Some(None)` when there is none.
    first: Option<Option<u64>>,
    busy: bool,
    /// Which page of rows the list holds (a page is [`PAGE_ROWS`] rows).
    page: u64,
    offset: Entity<InputState>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _typing: Subscription,
}

/// What a frame of the bytes list needs.
struct BytesFrame {
    t: theme::Tokens,
    shown: Option<u64>,
    mono: SharedString,
}

impl BytesPane {
    /// The bytes of `left` and `right`; the first window and the first difference are asked for at once.
    pub fn new(
        backend: Arc<dyn Backend>,
        left: &Side,
        right: &Side,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let offset = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Go to 0x…")));
        let typing = cx.subscribe_in(&offset, window, Self::on_offset_input);
        let mut pane = Self {
            backend,
            left: left.path.clone(),
            right: right.path.clone(),
            windows: HashMap::new(),
            sizes: None,
            error: None,
            shown: None,
            first: None,
            busy: false,
            page: 0,
            offset,
            scroll: UniformListScrollHandle::new(),
            focus,
            _typing: typing,
        };
        pane.load(0, cx);
        let looking = pane.backend.next_difference(&pane.left, &pane.right, 0);
        cx.spawn(async move |this, cx| {
            let found = core_bridge::run(looking).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match found {
                    Ok(at) => {
                        this.first = Some(at);
                        if let Some(at) = at {
                            this.show(at, cx);
                        }
                    }
                    Err(reason) => this.error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
        pane
    }

    /// The difference the list is at.
    #[cfg(test)]
    pub fn shown_offset(&self) -> Option<u64> {
        self.shown
    }

    /// Where the first difference is: `None` while looking, `Some(None)` when the files are the same.
    #[cfg(test)]
    pub fn first_difference(&self) -> Option<Option<u64>> {
        self.first
    }

    fn rows_total(&self) -> u64 {
        let size = self.sizes.map_or(0, |(a, b)| a.max(b));
        size.div_ceil(BYTES_ROW)
    }

    fn pages(&self) -> u64 {
        self.rows_total().div_ceil(PAGE_ROWS).max(1)
    }

    /// Reads window `n` unless it is here or on its way.
    fn load(&mut self, n: u64, cx: &mut Context<Self>) {
        if self.windows.contains_key(&n) {
            return;
        }
        self.windows.insert(n, Slot::Loading);
        let asked = self.backend.bytes(
            &self.left,
            &self.right,
            n * WINDOW,
            u32::try_from(WINDOW).unwrap_or(u32::MAX),
        );
        cx.spawn(async move |this, cx| {
            let got = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match got {
                    Ok(window) => {
                        this.sizes = Some((window.left_size, window.right_size));
                        this.windows.insert(n, Slot::Ready(Arc::new(window)));
                        this.forget_far(n);
                    }
                    Err(reason) => this.error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Lets go of the windows farthest from `near` beyond the ones kept.
    fn forget_far(&mut self, near: u64) {
        while self.windows.len() > KEPT_WINDOWS {
            let farthest = self.windows.keys().copied().max_by_key(|n| n.abs_diff(near));
            match farthest {
                Some(n) if n != near => {
                    self.windows.remove(&n);
                }
                _ => break,
            }
        }
    }

    /// Shows the difference at `offset`: the page it is on, scrolled to its row.
    pub fn show(&mut self, offset: u64, cx: &mut Context<Self>) {
        self.shown = Some(offset);
        let row = offset / BYTES_ROW;
        self.page = row / PAGE_ROWS;
        self.scroll
            .scroll_to_item_strict(usize::try_from(row % PAGE_ROWS).unwrap_or(0), ScrollStrategy::Center);
        cx.notify();
    }

    /// The next difference after the row the list is at (`next`).
    pub fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let from = (self.shown.unwrap_or(0) / BYTES_ROW + 1) * BYTES_ROW;
        let asked = self.backend.next_difference(&self.left, &self.right, from);
        cx.spawn_in(window, async move |this, cx| {
            let found = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match found {
                    Ok(Some(at)) => this.show(at, cx),
                    Ok(None) => window.push_notification(
                        Notification::info(t!("No more differences after this one")).id::<BytesToast>(),
                        cx,
                    ),
                    Err(reason) => this.error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The last difference before the row the list is at (`prev`).
    pub fn prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.shown.unwrap_or(0);
        if self.busy || shown == 0 {
            return;
        }
        self.busy = true;
        cx.notify();
        let before = shown / BYTES_ROW * BYTES_ROW;
        let asked = self.backend.prev_difference(&self.left, &self.right, before);
        cx.spawn_in(window, async move |this, cx| {
            let found = core_bridge::run(asked).await.and_then(|inner| inner);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match found {
                    Ok(Some(at)) => this.show(at, cx),
                    Ok(None) => window.push_notification(
                        Notification::info(t!("No earlier differences before this one")).id::<BytesToast>(),
                        cx,
                    ),
                    Err(reason) => this.error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Enter in the offset box: go to 0x1F00, or 8192 (`parseInt` of the TypeScript).
    fn on_offset_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::PressEnter { .. }) {
            return;
        }
        let max = self.sizes.map_or(0, |(a, b)| a.max(b));
        if let Some(offset) = parse_offset(&input.read(cx).value()).filter(|&o| o < max) {
            self.show(offset, cx);
        }
    }

    /// Moves a page of rows (the buttons for files too long for one).
    fn go_page(&mut self, page: u64, cx: &mut Context<Self>) {
        self.page = page.min(self.pages() - 1);
        self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Page Up and Page Down on the list.
    pub fn page(&mut self, down: bool) -> bool {
        page_list(&self.scroll, down)
    }

    /// One side of a row: its hex and its characters, differing bytes marked and the one shown stronger.
    fn byte_side(side: usize, mine: &ByteRow, diff: &[bool], offset: u64, frame: &BytesFrame) -> impl IntoElement {
        let t = &frame.t;
        let tone = side_tone(side, t);
        let mut cell_marks: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut char_marks: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        for (k, (cell, ch)) in mine.cell_ranges.iter().zip(&mine.char_ranges).enumerate() {
            let here = frame.shown == Some(offset + k as u64);
            if !(here || diff.get(k).copied().unwrap_or(false)) {
                continue;
            }
            let color = if here {
                t.primary.opacity(0.5)
            } else {
                tone.opacity(0.3)
            };
            let style = HighlightStyle {
                background_color: Some(color),
                ..Default::default()
            };
            cell_marks.push((cell.clone(), style));
            char_marks.push((ch.clone(), style));
        }
        row()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .gap(space(3.))
            .when(side == 1, |this| this.border_l_1().border_color(t.border).pl(space(3.)))
            .child(StyledText::new(mine.cells.clone()).with_highlights(cell_marks))
            .child(
                div()
                    .text_color(t.muted_foreground)
                    .child(StyledText::new(mine.chars.clone()).with_highlights(char_marks)),
            )
    }

    /// The list's row `ix` of the page.
    fn row(&mut self, ix: usize, frame: &BytesFrame, cx: &mut Context<Self>) -> AnyElement {
        let t = &frame.t;
        let offset = (self.page * PAGE_ROWS + ix as u64) * BYTES_ROW;
        let n = offset / WINDOW;
        let ready = match self.windows.get(&n) {
            Some(Slot::Ready(window)) => Some(window.clone()),
            Some(Slot::Loading) => None,
            None => {
                self.load(n, cx);
                None
            }
        };
        let label = hex(offset, 8);
        let offset_cell = div()
            .w(space(20.))
            .flex_none()
            .text_color(t.muted_foreground.opacity(0.7))
            .child(label.clone());
        let Some(window) = ready else {
            return row()
                .id(("compare-bytes-row", ix))
                .role(Role::Row)
                .aria_label(label)
                .test_support()
                .h(LINE)
                .child(offset_cell.text_color(t.muted_foreground.opacity(0.5)))
                .into_any_element();
        };
        let local = usize::try_from(offset - n * WINDOW).unwrap_or(usize::MAX);
        let slice = |bytes: &[u8]| -> Vec<u8> {
            bytes
                .get(local..)
                .map(|rest| rest[..rest.len().min(BYTES_ROW as usize)].to_vec())
                .unwrap_or_default()
        };
        let (l, r) = (slice(&window.left), slice(&window.right));
        let (left, right) = (byte_row(&l), byte_row(&r));
        let aria = format!("{label}: {} | {}", left.cells.trim(), right.cells.trim());
        row()
            .id(("compare-bytes-row", ix))
            .role(Role::Row)
            .aria_label(aria)
            .test_support()
            .h(LINE)
            .w_full()
            .gap(space(3.))
            .child(offset_cell)
            .child(Self::byte_side(0, &left, &differing(&l, &r), offset, frame))
            .child(Self::byte_side(1, &right, &differing(&r, &l), offset, frame))
            .into_any_element()
    }
}

/// Marks the bytes view's "no more differences" notification, so a second replaces the first.
struct BytesToast;

impl Render for BytesPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        if let Some(reason) = self.error.clone() {
            return TextPane::centered("compare-bytes-error", Role::Alert, reason.clone(), &t)
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(reason)
                .into_any_element();
        }
        let Some((left_size, right_size)) = self.sizes else {
            return TextPane::centered("compare-bytes-loading", Role::Status, t!("Reading the files…"), &t)
                .child(Spinner::new().color(t.primary))
                .into_any_element();
        };

        let verdict: AnyElement = match (self.first, self.shown) {
            (None, _) => row()
                .id("compare-bytes-verdict")
                .role(Role::Status)
                .aria_label(t!("Looking for where they differ…"))
                .test_support()
                .gap(space(1.5))
                .child(Spinner::new().xsmall())
                .child(t!("Looking for where they differ…"))
                .into_any_element(),
            (Some(None), _) => {
                let text = t!("The same, byte for byte");
                row()
                    .id("compare-bytes-verdict")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .gap(space(1.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(icon(IconName::CircleCheck).size_3p5().text_color(t.success))
                    .child(text)
                    .into_any_element()
            }
            (Some(Some(first)), shown) => {
                let text = match shown {
                    Some(at) => t!("At 0x{offset}", offset = hex(at, 8)),
                    None => t!("First difference at 0x{offset}", offset = hex(first, 8)),
                };
                div()
                    .id("compare-bytes-verdict")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(text)
                    .into_any_element()
            }
        };
        let apart = if left_size == right_size {
            SharedString::default()
        } else {
            t!(
                " ({n} bytes apart)",
                n = format_count(usize::try_from(left_size.abs_diff(right_size)).unwrap_or(usize::MAX))
            )
        };
        let sizes = t!(
            "· {left} and {right}",
            left = format_size(left_size),
            right = format_size(right_size)
        );
        let can_move = matches!(self.first, Some(Some(_))) && !self.busy;
        let at_first = self
            .shown
            .zip(self.first.flatten())
            .is_some_and(|(shown, first)| shown <= first);
        let pages = self.pages();
        let page_buttons = (pages > 1).then(|| {
            row_start()
                .gap(space(1.))
                .child(
                    Button::new("compare-bytes-part-before")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronLeft)
                        .disabled(self.page == 0)
                        .tooltip(t!("The part before"))
                        .accessibility_label(t!("The part before"))
                        .on_click(cx.listener(|this, _, _, cx| this.go_page(this.page.saturating_sub(1), cx))),
                )
                .child(
                    Button::new("compare-bytes-part-after")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronRight)
                        .disabled(self.page + 1 >= pages)
                        .tooltip(t!("The part after"))
                        .accessibility_label(t!("The part after"))
                        .on_click(cx.listener(|this, _, _, cx| this.go_page(this.page + 1, cx))),
                )
        });

        let toolbar = row_start()
            .id("compare-bytes-toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("Comparison"))
            .test_support()
            .w_full()
            .h(space(9.))
            .flex_none()
            .gap(space(2.))
            .px(space(3.5))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(verdict)
            .child(div().truncate().min_w_0().child(format!("{sizes}{apart}")))
            .child(div().flex_1())
            .children(page_buttons)
            .child(
                Input::new(&self.offset)
                    .small()
                    .w(space(28.))
                    .aria_label(t!("Go to byte offset")),
            )
            .child(
                Button::new("compare-bytes-previous")
                    .ghost()
                    .xsmall()
                    .loading(self.busy)
                    .icon(IconName::ChevronUp)
                    .label(t!("Prev"))
                    .disabled(!can_move || at_first || self.shown.is_none_or(|s| s == 0))
                    .tooltip(t!("Previous difference (Alt+Up)"))
                    .accessibility_label(t!("Previous difference (Alt+Up)"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.prev(window, cx);
                    })),
            )
            .child(
                Button::new("compare-bytes-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .label(t!("Next"))
                    .disabled(!can_move)
                    .tooltip(t!("Next difference (Alt+Down)"))
                    .accessibility_label(t!("Next difference (Alt+Down)"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.next(window, cx);
                    })),
            );

        let rows_in_page =
            usize::try_from(self.rows_total().saturating_sub(self.page * PAGE_ROWS).min(PAGE_ROWS)).unwrap_or(0);
        let frame = Rc::new(BytesFrame {
            t: t.clone(),
            shown: self.shown,
            mono: theme::mono_font(cx),
        });
        let mono = frame.mono.clone();
        let list = uniform_list(
            "compare-bytes-rows",
            rows_in_page,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range.map(|ix| this.row(ix, &frame, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        col()
            .id("compare-bytes")
            .size_full()
            .min_h_0()
            .child(toolbar)
            .child(
                div()
                    .id("compare-bytes-table")
                    .role(Role::Table)
                    .aria_label(t!("Bytes side by side"))
                    .test_support()
                    .relative()
                    .text_left()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .px(space(3.5))
                    .font_family(mono)
                    .text_size(theme::size::XS)
                    .child(list)
                    .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
            )
            .into_any_element()
    }
}

// ---- The pictures view ------------------------------------------------------------------------------------

/// How two pictures are shown (`PictureMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureMode {
    Side,
    Swipe,
    Onion,
    Diff,
}

/// A picture being read and decoded, or here.
enum Loaded {
    Loading,
    Failed(SharedString),
    Ready(Arc<RenderImage>),
}

/// The pixel difference of two pictures: a picture of it, and how many pixels differ.
struct PixelDiff {
    image: Arc<RenderImage>,
    changed: u64,
    total: u64,
}

/// A picture's size in pixels.
fn pixels_of(image: &RenderImage) -> (u32, u32) {
    let size = image.size(0);
    (
        u32::try_from(size.width.0).unwrap_or(0),
        u32::try_from(size.height.0).unwrap_or(0),
    )
}

/// The biggest side the pixel difference is worked out at (`2048`).
const DIFF_SIDE: f64 = 2048.;

/// A 32-bit top-down BMP of BGRA `pixels`, which GPUI's own BMP decoder turns into a picture (`image` is not a
/// dependency of glint-ui; the thumbnails come the same way).
// glint_core::thumbs::encode_bmp does the same for its own pixel type.
fn bmp_of(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    const HEADERS: u32 = 14 + 108;
    let image = u32::try_from(bgra.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADERS as usize + bgra.len());
    out.extend(b"BM");
    out.extend((HEADERS + image).to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(HEADERS.to_le_bytes());
    out.extend(108u32.to_le_bytes());
    out.extend(i32::try_from(width).unwrap_or(i32::MAX).to_le_bytes());
    // Negative: the rows run top to bottom.
    out.extend((-i32::try_from(height).unwrap_or(i32::MAX)).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    // BI_BITFIELDS, so the masks below (alpha included) are what counts.
    out.extend(3u32.to_le_bytes());
    out.extend(image.to_le_bytes());
    out.extend(2835i32.to_le_bytes());
    out.extend(2835i32.to_le_bytes());
    out.extend([0u8; 8]);
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        out.extend(mask.to_le_bytes());
    }
    out.extend(0x7352_4742u32.to_le_bytes());
    out.extend([0u8; 48]);
    out.extend(bgra);
    out
}

/// Compares two pictures' pixels (`computePixelDiff`): pixels that differ by more than `tolerance` percent in
/// any channel are red, the rest a faint gray of the first picture. Works at most [`DIFF_SIDE`] pixels on a
/// side. Returns the BMP of it with its size, and how many pixels differ of how many.
pub fn pixel_diff(a: &RenderImage, b: &RenderImage, tolerance: u32) -> Option<(Vec<u8>, u32, u32, u64, u64)> {
    let ((aw, ah), (bw, bh)) = (pixels_of(a), pixels_of(b));
    if aw == 0 || ah == 0 || bw == 0 || bh == 0 {
        return None;
    }
    let (w, h) = (aw.max(bw), ah.max(bh));
    let scale = (DIFF_SIDE / f64::from(w.max(h))).min(1.);
    let cw = ((f64::from(w) * scale).round() as u32).max(1);
    let ch = ((f64::from(h) * scale).round() as u32).max(1);
    let (da, db) = (a.as_bytes(0)?, b.as_bytes(0)?);
    let threshold = ((f64::from(tolerance) / 100. * 255.).round()) as i16;
    // The pixel of `bytes` (a picture `iw` wide and `ih` tall, drawn at the corner) at canvas position x, y.
    let sample = |bytes: &[u8], iw: u32, ih: u32, x: u32, y: u32| -> [u8; 4] {
        let sx = (u64::from(x) * u64::from(w) / u64::from(cw)) as u32;
        let sy = (u64::from(y) * u64::from(h) / u64::from(ch)) as u32;
        if sx >= iw || sy >= ih {
            return [0; 4];
        }
        let at = (sy as usize * iw as usize + sx as usize) * 4;
        bytes
            .get(at..at + 4)
            .and_then(|p| <[u8; 4]>::try_from(p).ok())
            .unwrap_or([0; 4])
    };
    let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
    let mut changed = 0u64;
    for y in 0..ch {
        for x in 0..cw {
            let (pa, pb) = (sample(da, aw, ah, x, y), sample(db, bw, bh, x, y));
            let differs = pa
                .iter()
                .zip(&pb)
                .any(|(p, q)| (i16::from(*p) - i16::from(*q)).abs() > threshold);
            if differs {
                changed += 1;
                // BGRA of rgb(255, 45, 85) at 240.
                out.extend([85, 45, 255, 240]);
            } else {
                // Pixels are BGRA: the luminance of the first picture, faint.
                let lum =
                    (0.299 * f64::from(pa[2]) + 0.587 * f64::from(pa[1]) + 0.114 * f64::from(pa[0])).round() as u8;
                out.extend([lum, lum, lum, (f64::from(pa[3]) * 0.28).round() as u8]);
            }
        }
    }
    Some((bmp_of(cw, ch, &out), cw, ch, changed, u64::from(cw) * u64::from(ch)))
}

/// Two pictures side by side, swiped, blended or compared pixel by pixel (`PicturesView`).
pub struct PicturesPane {
    sides: [Side; 2],
    pictures: [Loaded; 2],
    mode: PictureMode,
    /// The swipe's place or the second picture's opacity, in percent (`amount`).
    amount: u32,
    /// How much a pixel may differ and still be the same, in percent (`tolerance`).
    tolerance: u32,
    zoom: f32,
    pan: Point<Pixels>,
    /// Where the pointer went down and where the pan was then.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    diff: Option<PixelDiff>,
    diff_generation: u64,
    /// The space the pictures may take, measured while drawing (a frame later it is used).
    area: Rc<Cell<Size<Pixels>>>,
    focus: FocusHandle,
}

impl PicturesPane {
    /// The two pictures; both are read and decoded in the background at once.
    pub fn new(left: &Side, right: &Side, focus: FocusHandle, cx: &mut Context<Self>) -> Self {
        let pane = Self {
            sides: [left.clone(), right.clone()],
            pictures: [Loaded::Loading, Loaded::Loading],
            mode: PictureMode::Side,
            amount: 50,
            tolerance: 5,
            zoom: 1.,
            pan: Point::default(),
            drag: None,
            diff: None,
            diff_generation: 0,
            area: Rc::new(Cell::new(Size::default())),
            focus,
        };
        for n in 0..2 {
            let path = pane.sides[n].path.clone();
            let renderer = cx.svg_renderer();
            cx.spawn(async move |this, cx| {
                let decoded = cx
                    .background_executor()
                    .spawn(async move { decode(&path, renderer) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.pictures[n] = match decoded {
                        Ok(image) => Loaded::Ready(image),
                        Err(reason) => Loaded::Failed(reason.into()),
                    };
                    this.refresh_diff(cx);
                    cx.notify();
                });
            })
            .detach();
        }
        pane
    }

    /// The mode shown.
    pub fn mode(&self) -> PictureMode {
        self.mode
    }

    /// The two pictures' sizes once decoded.
    pub fn dimensions(&self) -> [Option<(u32, u32)>; 2] {
        [0, 1].map(|n| match &self.pictures[n] {
            Loaded::Ready(image) => Some(pixels_of(image)),
            _ => None,
        })
    }

    /// How many pixels differ of how many, once worked out.
    #[cfg(test)]
    pub fn difference(&self) -> Option<(u64, u64)> {
        self.diff.as_ref().map(|d| (d.changed, d.total))
    }

    /// Lets the pictures go of the GPU's atlas; call before the pane is dropped.
    pub fn release(&mut self, window: &mut Window) {
        for picture in &mut self.pictures {
            if let Loaded::Ready(image) = std::mem::replace(picture, Loaded::Loading) {
                window.drop_image(image).ok();
            }
        }
        if let Some(diff) = self.diff.take() {
            window.drop_image(diff.image).ok();
        }
    }

    /// Works the pixel difference out again in the background (when it is the mode shown).
    fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        if self.mode != PictureMode::Diff {
            return;
        }
        let (Loaded::Ready(a), Loaded::Ready(b)) = (&self.pictures[0], &self.pictures[1]) else {
            return;
        };
        self.diff_generation += 1;
        let generation = self.diff_generation;
        let (a, b, tolerance) = (a.clone(), b.clone(), self.tolerance);
        let renderer = cx.svg_renderer();
        cx.spawn(async move |this, cx| {
            let worked = cx
                .background_executor()
                .spawn(async move {
                    let (bmp, _, _, changed, total) = pixel_diff(&a, &b, tolerance)?;
                    let image = Image::from_bytes(ImageFormat::Bmp, bmp).to_image_data(renderer).ok()?;
                    Some(PixelDiff { image, changed, total })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.diff_generation == generation {
                    this.diff = worked;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn set_mode(&mut self, mode: PictureMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.refresh_diff(cx);
        cx.notify();
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = ((zoom * 100.).round() / 100.).clamp(0.25, 8.);
        cx.notify();
    }

    fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.;
        self.pan = Point::default();
        cx.notify();
    }

    /// One picture, fitted and zoomed about the middle of `frame` and panned.
    fn picture(&self, image: &Arc<RenderImage>) -> impl IntoElement {
        div()
            .absolute()
            .size_full()
            .left(self.pan.x)
            .top(self.pan.y)
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(image.clone())
                    .object_fit(ObjectFit::Contain)
                    .w(gpui_kit::relative(self.zoom))
                    .h(gpui_kit::relative(self.zoom)),
            )
    }

    /// A frame the pictures are drawn in, on a muted ground.
    fn frame(&self, t: &theme::Tokens) -> Div {
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(t.muted.opacity(0.5))
    }

    /// A picture that isn't there (yet): a spinner, or why not.
    fn missing(&self, n: usize, t: &theme::Tokens) -> AnyElement {
        match &self.pictures[n] {
            Loaded::Failed(reason) => TextPane::centered("compare-picture-failed", Role::Alert, reason.clone(), t)
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(reason.clone())
                .into_any_element(),
            _ => TextPane::centered("compare-picture-loading", Role::Status, t!("Loading…"), t)
                .child(Spinner::new().color(t.primary))
                .into_any_element(),
        }
    }

    /// A step button for a percentage.
    fn stepper(
        id: &'static str,
        label: SharedString,
        value: u32,
        max: u32,
        cx: &mut Context<Self>,
        set: fn(&mut Self, u32, &mut Context<Self>),
    ) -> impl IntoElement {
        row()
            .id(id)
            .role(Role::Group)
            .aria_label(format!("{label}: {value}%"))
            .test_support()
            .gap(space(1.))
            .child(
                Button::new((id, 0usize))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Minus)
                    .disabled(value == 0)
                    .tooltip(label.clone())
                    .accessibility_label(t!("Less: {what}", what = label.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| set(this, value.saturating_sub(10), cx))),
            )
            .child(div().w(space(9.)).text_center().child(format!("{value}%")))
            .child(
                Button::new((id, 1usize))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .disabled(value >= max)
                    .tooltip(label.clone())
                    .accessibility_label(t!("More: {what}", what = label))
                    .on_click(cx.listener(move |this, _, _, cx| set(this, (value + 10).min(max), cx))),
            )
    }
}

/// Reads and decodes a picture (on a background thread), with GPUI's own decoders.
fn decode(path: &str, renderer: gpui_kit::SvgRenderer) -> Result<Arc<RenderImage>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let ext = base_name(path).rsplit('.').next().unwrap_or("").to_lowercase();
    let format = picture::sniff(&bytes, &ext).ok_or_else(|| t!("This image can't be shown here").to_string())?;
    Image::from_bytes(format, bytes)
        .to_image_data(renderer)
        .map_err(|_| t!("This image can't be shown here").to_string())
}

impl Render for PicturesPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let modes = [
            (PictureMode::Side, t!("Side by side")),
            (PictureMode::Swipe, t!("Swipe")),
            (PictureMode::Onion, t!("Onion skin")),
            (PictureMode::Diff, t!("Pixel diff")),
        ];
        let mode_buttons = pill(&t)
            .id("compare-picture-modes")
            .role(Role::Group)
            .aria_label(t!("Show them"))
            .test_support()
            .children(modes.into_iter().enumerate().map(|(n, (mode, label))| {
                Button::new(("compare-picture-mode", n))
                    .ghost()
                    .xsmall()
                    .selected(self.mode == mode)
                    .label(label.clone())
                    .accessibility_label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
            }));

        let stepper_for_mode: Option<AnyElement> = match self.mode {
            PictureMode::Swipe => Some(
                Self::stepper(
                    "compare-swipe",
                    t!("Where the swipe is"),
                    self.amount,
                    100,
                    cx,
                    |this, v, cx| {
                        this.amount = v;
                        cx.notify();
                    },
                )
                .into_any_element(),
            ),
            PictureMode::Onion => Some(
                Self::stepper(
                    "compare-onion",
                    t!("How much of the second shows"),
                    self.amount,
                    100,
                    cx,
                    |this, v, cx| {
                        this.amount = v;
                        cx.notify();
                    },
                )
                .into_any_element(),
            ),
            _ => None,
        };
        let diff_controls: Option<AnyElement> = (self.mode == PictureMode::Diff).then(|| {
            let stats = self.diff.as_ref().map(|d| {
                let text = if d.changed == 0 {
                    t!("Identical pixels")
                } else {
                    t!(
                        "{percent}% ({count} px) differ",
                        percent = format!("{:.2}", d.changed as f64 / d.total.max(1) as f64 * 100.),
                        count = format_count(usize::try_from(d.changed).unwrap_or(usize::MAX))
                    )
                };
                let tone = if d.changed == 0 { t.success } else { t.destructive };
                div()
                    .id("compare-picture-stats")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tone)
                    .child(text)
            });
            row_start()
                .gap(space(2.))
                .child(Self::stepper(
                    "compare-tolerance",
                    t!("Pixel difference tolerance"),
                    self.tolerance,
                    50,
                    cx,
                    |this, v, cx| {
                        this.tolerance = v;
                        this.refresh_diff(cx);
                        cx.notify();
                    },
                ))
                .children(stats)
                .into_any_element()
        });

        let dims = {
            let [a, b] = self.dimensions();
            let show = |d: Option<(u32, u32)>| d.map_or_else(|| "…".to_string(), |(w, h)| format!("{w} × {h}"));
            format!("{} · {}", show(a), show(b))
        };
        let toolbar = row_start()
            .id("compare-pictures-toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("Comparison"))
            .test_support()
            .w_full()
            .h(space(9.))
            .flex_none()
            .gap(space(3.))
            .px(space(3.5))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(mode_buttons)
            .children(stepper_for_mode)
            .children(diff_controls)
            .child(div().flex_1())
            .child(
                Button::new("compare-zoom-out")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ZoomOut)
                    .tooltip(t!("Zoom out"))
                    .accessibility_label(t!("Zoom out"))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom - 0.25, cx))),
            )
            .child(
                Button::new("compare-zoom-reset")
                    .ghost()
                    .xsmall()
                    .label(format!("{}%", (self.zoom * 100.).round()))
                    .tooltip(t!("Reset zoom and pan"))
                    .accessibility_label(t!("Reset zoom and pan"))
                    .on_click(cx.listener(|this, _, _, cx| this.reset_view(cx))),
            )
            .child(
                Button::new("compare-zoom-in")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ZoomIn)
                    .tooltip(t!("Zoom in"))
                    .accessibility_label(t!("Zoom in"))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom + 0.25, cx))),
            )
            .child(
                div()
                    .id("compare-picture-sizes")
                    .role(Role::Status)
                    .aria_label(dims.clone())
                    .test_support()
                    .child(dims),
            );

        // What measures the space, so the swipe knows how wide the whole picture is.
        let area_cell = self.area.clone();
        let id = cx.entity().entity_id();
        let measure = canvas(
            move |bounds, window, _| {
                if area_cell.get() != bounds.size {
                    area_cell.set(bounds.size);
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let ready = |n: usize| match &self.pictures[n] {
            Loaded::Ready(image) => Some(image.clone()),
            _ => None,
        };
        let stage: AnyElement = match (self.mode, ready(0), ready(1)) {
            (PictureMode::Side, a, b) => row()
                .size_full()
                .children([(0, a), (1, b)].map(|(n, image)| {
                    let frame = self
                        .frame(&t)
                        .when(n == 1, |this| this.border_l_1().border_color(t.border));
                    match image {
                        Some(image) => frame
                            .id(("compare-picture", n))
                            .role(Role::Image)
                            .aria_label(base_name(&self.sides[n].path).to_string())
                            .test_support()
                            .child(self.picture(&image))
                            .into_any_element(),
                        None => frame.child(self.missing(n, &t)).into_any_element(),
                    }
                }))
                .into_any_element(),
            (PictureMode::Diff, _, _) => {
                let frame = self.frame(&t);
                match &self.diff {
                    Some(diff) => frame
                        .id("compare-picture-diff")
                        .role(Role::Image)
                        .aria_label(t!("Pixel diff"))
                        .test_support()
                        .child(self.picture(&diff.image))
                        .into_any_element(),
                    None => frame
                        .child(
                            TextPane::centered("compare-picture-working", Role::Status, t!("Loading…"), &t)
                                .child(Spinner::new().color(t.primary)),
                        )
                        .into_any_element(),
                }
            }
            (mode, Some(a), Some(b)) => {
                let area = self.area.get();
                let fraction = self.amount as f32 / 100.;
                let second: AnyElement = if mode == PictureMode::Swipe {
                    // Only the part of the second picture right of the line shows; it stays where it would be whole.
                    let (full_w, full_h) = (area.width, area.height);
                    let from = full_w * fraction;
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(full_w - from)
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .w(full_w)
                                .h(full_h)
                                .child(self.picture(&b)),
                        )
                        .child(div().absolute().top_0().bottom_0().left_0().w(px(2.)).bg(t.primary))
                        .into_any_element()
                } else {
                    div()
                        .absolute()
                        .size_full()
                        .opacity(fraction)
                        .child(self.picture(&b))
                        .into_any_element()
                };
                self.frame(&t)
                    .id("compare-picture-blend")
                    .role(Role::Image)
                    .aria_label(if mode == PictureMode::Swipe {
                        t!("Swipe")
                    } else {
                        t!("Onion skin")
                    })
                    .test_support()
                    .child(self.picture(&a))
                    .child(second)
                    .into_any_element()
            }
            (_, a, _) => {
                let n = usize::from(a.is_some());
                self.frame(&t).child(self.missing(n, &t)).into_any_element()
            }
        };

        col().id("compare-pictures").size_full().min_h_0().child(toolbar).child(
            div().relative().flex_1().min_h_0().w_full().child(measure).child(
                div()
                    .size_full()
                    .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
                        // Ctrl and the wheel zoom; so does the wheel once zoomed in (`onWheel`).
                        if event.modifiers.control || this.zoom > 1. {
                            let up = event.delta.pixel_delta(px(16.)).y > px(0.);
                            let factor = if up { 1.15 } else { 1. / 1.15 };
                            this.set_zoom(this.zoom * factor, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.drag = Some((event.position, this.pan));
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                        if let (Some((start, pan)), true) = (this.drag, event.dragging()) {
                            this.pan = point(
                                pan.x + (event.position.x - start.x),
                                pan.y + (event.position.y - start.y),
                            );
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            this.drag = None;
                        }),
                    )
                    .child(stage),
            ),
        )
    }
}

// ---- The dialog ------------------------------------------------------------------------------------------

/// A question with two buttons: a title, the message, and the words on its Cancel and OK buttons.
struct Question {
    title: SharedString,
    message: SharedString,
    ok: SharedString,
    cancel: SharedString,
    /// OK is drawn as a danger button.
    danger: bool,
}

/// Asks `question`; `on_ok` runs after it has closed. (Like [`super::confirm`], which has no word of its own for
/// Cancel.)
fn ask(window: &mut Window, cx: &mut App, question: Question, on_ok: impl Fn(&mut Window, &mut App) + 'static) {
    let on_ok = Rc::new(on_ok);
    let Question {
        title,
        message,
        ok,
        cancel,
        danger,
    } = question;
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let on_ok = on_ok.clone();
        let ok_button = Button::new("compare-ask-ok").label(ok.clone());
        let ok_button = if danger {
            ok_button.danger()
        } else {
            ok_button.primary()
        };
        dialog
            .title(title.clone())
            .w(px(440.))
            .child(
                div()
                    .id("compare-ask")
                    .role(Role::Group)
                    .aria_label(title.clone())
                    .aria_description(message.clone())
                    .test_support()
                    .text_start()
                    .child(message.clone()),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("compare-ask-cancel")
                            .label(cancel.clone())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(ok_button.on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        on_ok(window, cx);
                    })),
            )
    });
}

/// "The edits made in Compare aren't saved. Discard them?" (`discardCompareEdits`): `on_discard` runs after
/// the question has closed, if Discard was chosen.
pub fn ask_discard(window: &mut Window, cx: &mut App, on_discard: impl Fn(&mut Window, &mut App) + 'static) {
    let question = Question {
        title: t!("Unsaved edits"),
        message: t!("The edits made in Compare aren't saved. Discard them?"),
        ok: t!("Discard"),
        cancel: t!("Keep editing"),
        danger: true,
    };
    ask(window, cx, question, on_discard);
}

/// Which view of the two files the dialog shows (`View`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Text,
    Pictures,
    Bytes,
}

/// The view's entity.
enum Body {
    Text(Entity<TextPane>),
    Pictures(Entity<PicturesPane>),
    Bytes(Entity<BytesPane>),
}

/// What waits on a question about unsaved edits.
#[derive(Clone, Copy)]
enum After {
    View(View),
    Swap,
}

/// The dialog that shows the open comparison, for [`unsaved`].
struct Live(WeakEntity<CompareDialog>);

impl Global for Live {}

/// Whether the open comparison has edits not saved yet: quitting would lose them (`discardCompareEdits` at
/// `mayQuit`). Ask with [`ask_discard`] first.
pub fn unsaved(cx: &App) -> bool {
    cx.try_global::<Live>()
        .and_then(|live| live.0.upgrade())
        .is_some_and(|dialog| dialog.read(cx).dirty(cx))
}

/// What runs once the comparison is gone (a temp copy is removed).
pub type OnClose = Box<dyn FnOnce(&mut App)>;

/// What the views are made of, besides the two files.
#[derive(Clone)]
struct Wiring {
    backend: Arc<dyn Backend>,
    tools: Option<Entity<Tools>>,
    /// The dialog's focus: where the keyboard goes when a row or a button is used.
    focus: FocusHandle,
}

/// Two files, side by side (`CompareModal`).
pub struct CompareDialog {
    wiring: Wiring,
    left: Side,
    right: Side,
    swapped: bool,
    view: View,
    /// Both are pictures: the Pictures view is offered, and shown first.
    pictures: bool,
    body: Body,
    body_subscription: Option<Subscription>,
    on_close: Option<OnClose>,
    _release: Subscription,
}

impl CompareDialog {
    /// A comparison of `left` and `right`, comparing at once. `on_close` runs when it is gone (a temp copy is
    /// removed).
    pub fn new(
        backend: Arc<dyn Backend>,
        tools: Option<Entity<Tools>>,
        left: Side,
        right: Side,
        on_close: Option<OnClose>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pictures = is_picture(&left.path) && is_picture(&right.path);
        let wiring = Wiring {
            backend,
            tools,
            focus: cx.focus_handle(),
        };
        let release = cx.on_release(|this, cx| {
            if let Some(on_close) = this.on_close.take() {
                on_close(cx);
            }
        });
        let view = if pictures { View::Pictures } else { View::Text };
        let (body, body_subscription) = Self::build_body(view, &wiring, left.clone(), right.clone(), window, cx);
        window.focus(&wiring.focus, cx);
        Self {
            wiring,
            left,
            right,
            swapped: false,
            view,
            pictures,
            body,
            body_subscription,
            on_close,
            _release: release,
        }
    }

    /// The view for `view` over `a` and `b`, which starts its work at once.
    fn build_body(
        view: View,
        wiring: &Wiring,
        a: Side,
        b: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Body, Option<Subscription>) {
        match view {
            View::Text => {
                let model = cx.new(|_| TextModel::new(wiring.backend.clone(), a, b));
                let (tools, focus) = (wiring.tools.clone(), wiring.focus.clone());
                let pane = cx.new(|cx| TextPane::new(model, tools, focus, window, cx));
                let subscription =
                    cx.subscribe_in(&pane, window, |this, _, event: &PaneEvent, window, cx| match event {
                        PaneEvent::ShowBytes => this.apply(After::View(View::Bytes), window, cx),
                    });
                (Body::Text(pane), Some(subscription))
            }
            View::Pictures => {
                let focus = wiring.focus.clone();
                (Body::Pictures(cx.new(|cx| PicturesPane::new(&a, &b, focus, cx))), None)
            }
            View::Bytes => {
                let (backend, focus) = (wiring.backend.clone(), wiring.focus.clone());
                (
                    Body::Bytes(cx.new(|cx| BytesPane::new(backend, &a, &b, focus, window, cx))),
                    None,
                )
            }
        }
    }

    /// The views on offer, in order.
    pub fn views(&self) -> Vec<View> {
        let mut views = vec![View::Text];
        if self.pictures {
            views.push(View::Pictures);
        }
        views.push(View::Bytes);
        views
    }

    /// The view shown.
    pub fn view(&self) -> View {
        self.view
    }

    /// The files as shown, left then right (swapped or not).
    pub fn sides(&self) -> (Side, Side) {
        if self.swapped {
            (self.right.clone(), self.left.clone())
        } else {
            (self.left.clone(), self.right.clone())
        }
    }

    /// The text view, when that is the view shown.
    #[cfg(test)]
    pub fn text_pane(&self) -> Option<&Entity<TextPane>> {
        match &self.body {
            Body::Text(pane) => Some(pane),
            _ => None,
        }
    }

    /// The bytes view, when that is the view shown.
    #[cfg(test)]
    pub fn bytes_pane(&self) -> Option<&Entity<BytesPane>> {
        match &self.body {
            Body::Bytes(pane) => Some(pane),
            _ => None,
        }
    }

    /// The pictures view, when that is the view shown.
    #[cfg(test)]
    pub fn pictures_pane(&self) -> Option<&Entity<PicturesPane>> {
        match &self.body {
            Body::Pictures(pane) => Some(pane),
            _ => None,
        }
    }

    /// `Compare “a” and “b”`, in the order shown.
    pub fn title(&self) -> SharedString {
        let (a, b) = self.sides();
        t!("Compare “{a}” and “{b}”", a = isolate(a.name()), b = isolate(b.name()))
    }

    /// Where the files are: each folder once, leaving out a read-only temp copy.
    fn subtitle(&self) -> String {
        let (a, b) = self.sides();
        let mut folders: Vec<String> = Vec::new();
        for side in [&a, &b].into_iter().filter(|s| !s.read_only) {
            let folder = parent_path(&side.path);
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        if folders.is_empty() {
            folders.push(parent_path(&a.path));
        }
        folders.join("  ·  ")
    }

    /// Whether there are edits not saved yet.
    pub fn dirty(&self, cx: &App) -> bool {
        match &self.body {
            Body::Text(pane) => pane.read(cx).dirty(cx),
            _ => false,
        }
    }

    /// Makes the view for the current choice (a new one every time, as the TypeScript's `key` did).
    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_pictures(window, cx);
        let (a, b) = self.sides();
        self.body_subscription = None;
        let (body, subscription) = Self::build_body(self.view, &self.wiring, a, b, window, cx);
        self.body = body;
        self.body_subscription = subscription;
        window.focus(&self.wiring.focus, cx);
        cx.notify();
    }

    /// Lets the pictures go of the GPU (the dialog is closing, or the view changing).
    pub fn release_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Body::Pictures(pane) = &self.body {
            pane.update(cx, |pane, _| pane.release(window));
        }
    }

    fn apply(&mut self, after: After, window: &mut Window, cx: &mut Context<Self>) {
        match after {
            After::View(view) => self.view = view,
            After::Swap => self.swapped = !self.swapped,
        }
        self.rebuild(window, cx);
    }

    /// Does `after`, asking first when it would lose edits (`unlessEdited`).
    fn request(&mut self, after: After, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty(cx) {
            self.apply(after, window, cx);
            return;
        }
        let this = cx.entity();
        ask_discard(window, cx, move |window, cx| {
            this.update(cx, |this, cx| this.apply(after, window, cx));
        });
    }

    /// Shows `view` (asking about unsaved edits first), unless it is shown.
    pub fn set_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if view != self.view && self.views().contains(&view) {
            self.request(After::View(view), window, cx);
        }
    }

    /// Swaps the sides (asking about unsaved edits first).
    pub fn swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request(After::Swap, window, cx);
    }

    // ---- What the keys do --------------------------------------------------------------------------------

    fn previous_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.body {
            Body::Text(pane) => pane.update(cx, |pane, cx| pane.go(-1, cx)),
            Body::Bytes(pane) => pane.update(cx, |pane, cx| pane.prev(window, cx)),
            Body::Pictures(_) => {}
        }
    }

    fn next_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.body {
            Body::Text(pane) => pane.update(cx, |pane, cx| pane.go(1, cx)),
            Body::Bytes(pane) => pane.update(cx, |pane, cx| pane.next(window, cx)),
            Body::Pictures(_) => {}
        }
    }

    fn copy_change(&mut self, to_right: bool, cx: &mut Context<Self>) {
        if let Body::Text(pane) = &self.body {
            pane.update(cx, |pane, cx| pane.copy_current(to_right, cx));
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Body::Text(pane) = &self.body {
            pane.update(cx, |pane, cx| pane.save(window, cx));
        }
    }

    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Body::Text(pane) = &self.body {
            pane.update(cx, |pane, cx| pane.open_find(window, cx));
        }
    }

    /// Ctrl+Z (undo, when no box has the keyboard) and the paging keys.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_focused_input(cx) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let handled = match (event.keystroke.key.as_str(), &self.body) {
            ("z", Body::Text(pane)) if modifiers.control && !modifiers.alt && !modifiers.shift => {
                pane.update(cx, |pane, cx| pane.undo(cx));
                true
            }
            ("pagedown" | "pageup", Body::Text(pane)) => {
                let (down, scroll) = (event.keystroke.key == "pagedown", pane.read(cx).scroll.clone());
                page_list(&scroll, down)
            }
            ("pagedown" | "pageup", Body::Bytes(pane)) => {
                let down = event.keystroke.key == "pagedown";
                pane.update(cx, |pane, _| pane.page(down))
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    // ---- Drawing -----------------------------------------------------------------------------------------

    /// The Text / Pictures / Bytes switch and Swap.
    fn controls(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let views = self.views();
        let switch = pill(t)
            .id("compare-views")
            .role(Role::Group)
            .aria_label(t!("Compare as"))
            .test_support()
            .children(views.into_iter().enumerate().map(|(n, view)| {
                let (glyph, label) = match view {
                    View::Text => (IconName::FileText, t!("Text")),
                    View::Pictures => (IconName::Image, t!("Pictures")),
                    View::Bytes => (IconName::Binary, t!("Bytes")),
                };
                Button::new(("compare-view", n))
                    .ghost()
                    .xsmall()
                    .selected(self.view == view)
                    .icon(glyph)
                    .label(label.clone())
                    .accessibility_label(label)
                    .on_click(cx.listener(move |this, _, window, cx| this.set_view(view, window, cx)))
            }));
        let swap = Button::new("compare-swap")
            .ghost()
            .xsmall()
            .icon(IconName::ArrowLeftRight)
            .label(t!("Swap"))
            .tooltip(t!("Swap the sides"))
            .accessibility_label(t!("Swap the sides"))
            .on_click(cx.listener(|this, _, window, cx| this.swap(window, cx)));
        row_start().gap(space(2.)).flex_none().child(switch).child(swap)
    }

    /// The names of both files, over the pictures and the bytes (the text view has its own).
    fn names(&self, t: &theme::Tokens) -> impl IntoElement {
        let (a, b) = self.sides();
        row()
            .id("compare-names")
            .w_full()
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.6))
            .text_size(theme::size::XS)
            .children([a, b].into_iter().enumerate().map(|(n, side)| {
                row()
                    .id(("compare-name", n))
                    .role(Role::Group)
                    .aria_label(side.name())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .gap(space(2.))
                    .px(space(3.5))
                    .py(space(1.5))
                    .when(n == 1, |this| this.border_l_1().border_color(t.border))
                    .child(
                        div()
                            .size_2()
                            .flex_none()
                            .rounded_full()
                            .bg(side_tone(n, t).opacity(0.7)),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child(isolate(side.name())),
                    )
                    .when(side.label.is_some(), |this| {
                        this.child(
                            div()
                                .truncate()
                                .text_color(t.muted_foreground)
                                .child(isolate(base_name(&side.path).to_string())),
                        )
                    })
            }))
    }
}

impl Render for CompareDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let rem = theme::rem_size(cx);
        // 92vh less the dialog's title and padding, within the TypeScript's 64rem.
        let height = (window.viewport_size().height * 0.92 - rem * 8.).clamp(rem * 20., rem * 64.);
        let (a, b) = self.sides();
        let subtitle = self.subtitle();
        let body: AnyElement = match &self.body {
            Body::Text(pane) => pane.clone().into_any_element(),
            Body::Pictures(pane) => pane.clone().into_any_element(),
            Body::Bytes(pane) => pane.clone().into_any_element(),
        };
        col()
            .id("compare")
            .role(Role::Group)
            .aria_label(t!("Compare {a} and {b}", a = a.name(), b = b.name()))
            .test_support()
            .key_context("Compare")
            .track_focus(&self.wiring.focus)
            .on_action(
                cx.listener(|this, _: &actions::ComparePreviousChange, window, cx| this.previous_change(window, cx)),
            )
            .on_action(cx.listener(|this, _: &actions::CompareNextChange, window, cx| this.next_change(window, cx)))
            .on_action(cx.listener(|this, _: &actions::CopyChangeRight, _, cx| this.copy_change(true, cx)))
            .on_action(cx.listener(|this, _: &actions::CopyChangeLeft, _, cx| this.copy_change(false, cx)))
            .on_action(cx.listener(|this, _: &actions::CompareSave, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &actions::CompareFind, window, cx| this.find(window, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key_down(event, window, cx)))
            .w_full()
            .h(height)
            .gap(space(2.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .child(
                row_start()
                    .w_full()
                    .flex_none()
                    .gap(space(3.))
                    .child(
                        div()
                            .id("compare-folders")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(subtitle)),
                    )
                    .child(self.controls(&t, cx)),
            )
            .child(
                col()
                    .flex_1()
                    .min_h_0()
                    .rounded(theme::size::RADIUS_LG)
                    .border_1()
                    .border_color(t.border)
                    .overflow_hidden()
                    .when(self.view != View::Text, |this| this.child(self.names(&t)))
                    .child(div().flex_1().min_h_0().child(body)),
            )
    }
}

/// What closing the dialog does (its X, Escape): lets go of the pictures and closes, or asks first when
/// there are edits not saved. True to close now.
fn close_requested(view: &Entity<CompareDialog>, window: &mut Window, cx: &mut App) -> bool {
    if !view.read(cx).dirty(cx) {
        view.update(cx, |view, cx| view.release_pictures(window, cx));
        return true;
    }
    ask_discard(window, cx, |window, cx| window.close_dialog(cx));
    false
}

// ---- Opening it ------------------------------------------------------------------------------------------

// Wired: `MenuEvent::Compare` and the `CompareFiles` action (Ctrl+M) in workspace.rs, `versions::Request::
// CompareFiles` there too, and quitting (`mayQuit`, startup.rs) asks [`unsaved`] and then [`ask_discard`].
/// Opens two files side by side (`compareFiles(left, right)`; `MenuEvent::Compare { left, right }` and the
/// `CompareFiles` action call this): text, or pictures when both are, or bytes.
pub fn open(window: &mut Window, cx: &mut App, left: String, right: String) {
    open_sides(window, cx, Side::new(left), Side::new(right), None);
}

/// [`open`] with each file's label and whether it is read-only, and what to do once the comparison is gone.
pub fn open_sides(window: &mut Window, cx: &mut App, left: Side, right: Side, on_close: Option<OnClose>) {
    let tools = tools_for(window, cx);
    open_with(window, cx, Arc::new(CoreBackend), tools, left, right, on_close);
}

/// The tools model the Difftastic button reads (none without core).
fn tools_for(window: &mut Window, cx: &mut App) -> Option<Entity<Tools>> {
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| Tools::new(backend, window, cx)))
}

/// [`open_sides`] for a given backend (tests pass a fake).
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tools: Option<Entity<Tools>>,
    left: Side,
    right: Side,
    on_close: Option<OnClose>,
) -> Entity<CompareDialog> {
    let view = cx.new(|cx| CompareDialog::new(backend, tools, left, right, on_close, window, cx));
    cx.set_global(Live(view.downgrade()));
    let rem = theme::rem_size(cx);
    // min(96vw, 120rem).
    let width = (window.viewport_size().width * 0.96).min(rem * 120.).max(px(480.));
    let shown = view.clone();
    // The dialog layer takes the keyboard when it opens; Alt+Down, Ctrl+S and the others need it in the content.
    let after = view.clone();
    window.open_dialog(cx, move |dialog, _window, cx| {
        let closing = shown.clone();
        dialog
            .title(shown.read(cx).title())
            .w(width)
            .centered()
            // Enter belongs to the dialog's own boxes and buttons, not to the dialog layer (which would close it).
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, window, cx| close_requested(&closing, window, cx))
            .child(shown.clone())
    });
    after.update(cx, |view, cx| window.focus(&view.wiring.focus, cx));
    view
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::Mutex;
    use std::time::Duration;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    /// `start..end` (written out: clippy takes a one-range array for a mistake).
    fn span(start: usize, end: usize) -> Range<usize> {
        Range { start, end }
    }

    // ---- The pure parts ----------------------------------------------------------------------------------

    /// A comparison by hand: `rows` by line on each side, and the changed lines.
    fn compare_of(rows: &[[Option<u32>; 2]], left: &[u32], right: &[u32]) -> TextCompare {
        TextCompare {
            rows: rows.to_vec(),
            left_changes: left.iter().map(|&l| (l, Vec::new())).collect(),
            right_changes: right.iter().map(|&r| (r, Vec::new())).collect(),
            ..Default::default()
        }
    }

    fn paired(n: u32) -> Vec<[Option<u32>; 2]> {
        (0..n).map(|i| [Some(i), Some(i)]).collect()
    }

    #[test]
    fn blocks_group_changed_rows_and_keep_each_sides_lines() {
        // Rows 1-2 changed on both sides; row 5 only on the right, which has a line the left doesn't.
        let mut rows = paired(5);
        rows.push([None, Some(5)]);
        rows.push([Some(5), Some(6)]);
        let compare = compare_of(&rows, &[1, 2], &[1, 2, 5]);
        let blocks = change_blocks(&compare);
        assert_eq!(
            blocks,
            [
                ChangeBlock {
                    start_row: 1,
                    end_row: 3,
                    left_range: (1, 3),
                    right_range: (1, 3)
                },
                // Only the right has lines here: the left's range is where they would go in, after its line 4.
                ChangeBlock {
                    start_row: 5,
                    end_row: 6,
                    left_range: (5, 5),
                    right_range: (5, 6)
                },
            ]
        );
        let analysis = Analysis::of(compare);
        assert_eq!((analysis.removed, analysis.added), (2, 3));
        assert_eq!(analysis.block_of(0), None);
        assert_eq!(analysis.block_of(2), Some(0));
        assert_eq!(analysis.block_of(5), Some(1));
        assert_eq!(analysis.block_of(99), None);
    }

    #[test]
    fn a_block_at_the_start_with_nothing_before_it_goes_in_at_line_zero() {
        let rows = vec![[None, Some(0)], [Some(0), Some(1)]];
        let blocks = change_blocks(&compare_of(&rows, &[], &[0]));
        assert_eq!(blocks[0].left_range, (0, 0));
        assert_eq!(blocks[0].right_range, (0, 1));
    }

    #[test]
    fn rows_filter_to_all_context_or_diffs_with_folds_keyed_by_their_first_row() {
        // 30 rows, changes at 10 and 25.
        let analysis = Analysis::of(compare_of(&paired(30), &[10, 25], &[10, 25]));
        let none = HashSet::new();
        assert_eq!(visible_items(&analysis, RowFilter::All, &none).len(), 30);
        let diffs = visible_items(&analysis, RowFilter::Diffs, &none);
        assert_eq!(diffs.len(), 2);
        assert!(matches!(
            diffs[0],
            Item::Row {
                row: 10,
                block: Some(0),
                is_block_start: true
            }
        ));
        // Three rows either side of each change; the rest folded.
        let context = visible_items(&analysis, RowFilter::Context, &none);
        let folds: Vec<_> = context
            .iter()
            .filter_map(|item| match item {
                Item::Fold { start, end, count } => Some((*start, *end, *count)),
                _ => None,
            })
            .collect();
        assert_eq!(folds, [(0, 7, 7), (14, 22, 8)], "{context:?}");
        // Two folds, the seven rows kept around each change, and the one row after the last that isn't worth a fold.
        assert_eq!(context.len(), 2 + 7 + 7 + 1);
        // Opening a fold shows its rows.
        let open: HashSet<usize> = [14].into();
        let opened = visible_items(&analysis, RowFilter::Context, &open);
        assert_eq!(opened.len(), context.len() - 1 + 8);
        assert!(!opened.iter().any(|item| matches!(item, Item::Fold { start: 14, .. })));
        // A single unchanged row between kept ones isn't folded: there is no point.
        let analysis = Analysis::of(compare_of(&paired(12), &[0, 8], &[0, 8]));
        let items = visible_items(&analysis, RowFilter::Context, &none);
        assert!(!items.iter().any(|item| matches!(item, Item::Fold { .. })));
    }

    #[test]
    fn a_row_is_found_in_the_list_whatever_the_filter() {
        let analysis = Analysis::of(compare_of(&paired(30), &[10, 25], &[10, 25]));
        let items = visible_items(&analysis, RowFilter::Diffs, &HashSet::new());
        assert_eq!(visible_index_of(&items, 25), Some(1));
        assert_eq!(visible_index_of(&items, 11), None);
        let context = visible_items(&analysis, RowFilter::Context, &HashSet::new());
        let at = visible_index_of(&context, 10).expect("row 10 is kept");
        assert!(matches!(context[at], Item::Row { row: 10, .. }));
        // A folded row isn't there.
        assert_eq!(visible_index_of(&context, 3), None);
    }

    #[test]
    fn lines_are_written_back_as_they_were_read() {
        let text = lines("a\nb");
        assert_eq!(serialize_lines(&text, false, true), "a\nb\n");
        assert_eq!(serialize_lines(&text, true, true), "a\r\nb\r\n");
        assert_eq!(serialize_lines(&text, true, false), "a\r\nb");
        // An empty file stays empty, whatever it ended with.
        assert_eq!(serialize_lines(&[], true, true), "");
    }

    #[test]
    fn splicing_replaces_a_range_and_clamps_one_beyond_the_end() {
        let base = lines("a\nb\nc\nd");
        let with = |range: (usize, usize), new: &[&str]| {
            let new: Vec<String> = new.iter().map(|s| s.to_string()).collect();
            splice_lines(&base, range, &new).join(",")
        };
        assert_eq!(with((1, 3), &["X"]), "a,X,d");
        assert_eq!(
            with((2, 2), &["X", "Y"]),
            "a,b,X,Y,c,d",
            "an empty range is an insertion"
        );
        assert_eq!(with((1, 3), &[]), "a,d", "nothing to put in is a deletion");
        assert_eq!(with((3, 99), &["X"]), "a,b,c,X");
        assert_eq!(with((90, 99), &["X"]), "a,b,c,d,X");
        assert_eq!(
            with((3, 1), &["X"]),
            "a,b,c,X,d",
            "an end before the start counts as empty"
        );
    }

    #[test]
    fn find_spans_ignore_case_and_count_bytes_of_the_original() {
        assert_eq!(find_spans_in_line("Hello hello HELLO", "hello"), [0..5, 6..11, 12..17]);
        assert_eq!(find_spans_in_line("aaaa", "aa"), [0..2, 2..4], "hits don't overlap");
        assert!(find_spans_in_line("abc", "").is_empty());
        assert!(find_spans_in_line("abc", "x").is_empty());
        // Bytes, not UTF-16 units or characters: the é is two bytes, the 😀 four.
        assert_eq!(find_spans_in_line("é😀x", "x"), [span(6, 7)]);
        assert_eq!(find_spans_in_line("é😀x", "É"), [span(0, 2)]);
        // Arabic: the letters are two bytes each, the space one.
        assert_eq!(find_spans_in_line("تقرير الربع", "الربع"), [span(11, 21)]);
        // A capital that lowercases to two characters still maps back to its own bytes.
        assert_eq!(find_spans_in_line("İx", "x"), [span(2, 3)]);
    }

    #[test]
    fn matching_rows_look_at_both_sides_ignoring_case() {
        let compare = TextCompare {
            left: lines("Alpha\nBeta\nGamma"),
            right: lines("alpha\ndelta"),
            rows: vec![[Some(0), Some(0)], [Some(1), Some(1)], [Some(2), None]],
            ..Default::default()
        };
        assert_eq!(matching_rows(&compare, "ALPHA"), [0]);
        assert_eq!(
            matching_rows(&compare, "ta"),
            [1],
            "beta on the left, delta on the right: one row"
        );
        assert_eq!(matching_rows(&compare, "gam"), [2]);
        assert!(matching_rows(&compare, "").is_empty());
    }

    #[test]
    fn shaping_spreads_tabs_and_maps_utf16_and_byte_positions_to_drawn_bytes() {
        let shaped = Shaped::new("a\tb");
        assert_eq!(shaped.text, "a   b", "the tab reaches the next stop of four");
        assert_eq!(shaped.from_utf16(0), 0);
        assert_eq!(shaped.from_utf16(1), 1, "before the tab");
        assert_eq!(shaped.from_utf16(2), 4, "after it");
        assert_eq!(shaped.from_utf16(3), 5);
        assert_eq!(shaped.from_utf16(99), 5, "past the end is the end");
        // UTF-16: 😀 is two units and four bytes; é one unit and two bytes.
        let shaped = Shaped::new("é😀x");
        assert_eq!(shaped.from_utf16(1), 2);
        assert_eq!(shaped.from_utf16(3), 6);
        assert_eq!(shaped.from_byte(6), 6);
        assert_eq!(shaped.from_byte(2), 2);
        // A long line is cut, and a position past the cut is the cut.
        let long = "x".repeat(MAX_SHOWN_CHARS + 500);
        let shaped = Shaped::new(&long);
        assert_eq!(shaped.text.chars().count(), MAX_SHOWN_CHARS + 1);
        assert!(shaped.text.ends_with('…'));
        assert_eq!(
            shaped.from_utf16(u32::try_from(MAX_SHOWN_CHARS + 100).unwrap()),
            MAX_SHOWN_CHARS
        );
        assert_eq!(Shaped::new("").text, "");
    }

    #[test]
    fn marks_put_finds_over_changes_and_never_overlap() {
        let shaped = Shaped::new("let name = world;");
        // The change is "name = world" (units 4 to 16); the find is "= wo" (units 9 to 13), inside it.
        let runs = marks(&shaped, &[[4, 16]], &[span(9, 13)]);
        assert_eq!(
            runs,
            [(4..9, Mark::Change), (9..13, Mark::Find), (13..16, Mark::Change)]
        );
        // Two touching changes are one run; a find alone is a find; nothing is nothing.
        assert_eq!(marks(&shaped, &[[0, 3], [3, 6]], &[]), [(0..6, Mark::Change)]);
        assert_eq!(marks(&shaped, &[], &[span(0, 3)]), [(0..3, Mark::Find)]);
        assert!(marks(&shaped, &[], &[]).is_empty());
        // An empty or backwards range, or one past the end, draws nothing wrong.
        assert!(marks(&shaped, &[[5, 5], [9, 3]], &[]).is_empty());
        assert_eq!(marks(&shaped, &[[10, 999]], &[]), [(10..17, Mark::Change)]);
        // Arabic: the second word sits at UTF-16 units 6 to 11, which are bytes 11 to 21.
        let shaped = Shaped::new("تقرير الربع");
        assert_eq!(marks(&shaped, &[[6, 11]], &[]), [(11..21, Mark::Change)]);
    }

    #[test]
    fn a_row_of_bytes_is_hex_and_characters_with_each_bytes_place() {
        let row = byte_row(&[0x48, 0x69, 0x00, 0xFF]);
        assert!(row.cells.starts_with("48 69 00 FF "));
        assert_eq!(row.cell_ranges[1], 3..5);
        assert_eq!(
            row.cells.len(),
            16 * 3 - 1 + 1,
            "16 cells, the gaps, and one more after the eighth"
        );
        assert_eq!(&row.chars[..2], "Hi");
        assert_eq!(row.char_ranges.len(), 4);
        assert_eq!(row.chars.chars().count(), 16);
        // A full row has a wider gap after its eighth byte.
        let full = byte_row(&[0u8; 16]);
        assert_eq!(&full.cells[23..26], "  0", "the gap between the two halves");
        assert_eq!(
            differing(&[1, 2, 3], &[1, 9]),
            [false, true, true],
            "no byte on the other side differs"
        );
        assert!(differing(&[], &[1]).is_empty());
    }

    #[test]
    fn a_side_is_named_by_its_label_or_its_file() {
        assert_eq!(Side::new("C:\\a\\تقرير.txt").name(), "تقرير.txt");
        assert_eq!(
            Side::named("C:\\tmp\\x", "In the last commit", true).name(),
            "In the last commit"
        );
        assert!(is_picture("C:\\a\\photo.PNG"));
        assert!(!is_picture("C:\\a\\notes.txt"));
    }

    #[test]
    fn every_icon_the_dialog_draws_is_in_the_embedded_assets() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::TriangleAlert,
            IconName::ArrowLeftRight,
            IconName::ArrowLeft,
            IconName::ArrowRight,
            IconName::Binary,
            IconName::CaseSensitive,
            IconName::CircleCheck,
            IconName::ChevronDown,
            IconName::ChevronUp,
            IconName::ChevronLeft,
            IconName::ChevronRight,
            IconName::Download,
            IconName::FileText,
            IconName::Image,
            IconName::Minus,
            IconName::Plus,
            IconName::RotateCcw,
            IconName::Save,
            IconName::Search,
            IconName::Sparkles,
            IconName::UnfoldVertical,
            IconName::X,
            IconName::ZoomIn,
            IconName::ZoomOut,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    // ---- A pretend disk -------------------------------------------------------------------------------------

    /// Text files in memory, compared by row (the real comparing is glint-core's, tested there), with a note of
    /// what the dialog asked.
    #[derive(Default)]
    struct Fake {
        files: Mutex<HashMap<String, Vec<u8>>>,
        asked: Mutex<Vec<String>>,
        too_big: bool,
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl Fake {
        fn with(files: &[(&str, &str)]) -> Arc<Self> {
            let fake = Fake::default();
            for (path, text) in files {
                lock(&fake.files).insert(path.to_string(), text.as_bytes().to_vec());
            }
            Arc::new(fake)
        }

        fn put_bytes(&self, path: &str, bytes: &[u8]) {
            lock(&self.files).insert(path.to_string(), bytes.to_vec());
        }

        fn text(&self, path: &str) -> String {
            String::from_utf8_lossy(&lock(&self.files).get(path).cloned().unwrap_or_default()).into_owned()
        }

        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            lock(&self.files)
                .get(path)
                .cloned()
                .ok_or_else(|| format!("Couldn't open {path}: not found"))
        }

        fn asked(&self) -> Vec<String> {
            lock(&self.asked).clone()
        }
    }

    fn utf16_len(text: &str) -> u32 {
        u32::try_from(text.encode_utf16().count()).unwrap_or(0)
    }

    impl Backend for Fake {
        fn compare_text(
            &self,
            left: &str,
            right: &str,
            _: CompareOptions,
            left_override: Option<String>,
            right_override: Option<String>,
        ) -> Fut<TextCompare> {
            lock(&self.asked).push(format!(
                "compare {} {} {}{}",
                left,
                right,
                left_override.is_some() as u8,
                right_override.is_some() as u8
            ));
            let sides = [(left, left_override), (right, right_override)].map(|(path, over)| match over {
                Some(text) => Ok((text.into_bytes(), false)),
                None => self.read(path).map(|bytes| (bytes, true)),
            });
            let too_big = self.too_big;
            Box::pin(async move {
                let [l, r] = sides;
                let ((lb, ld), (rb, rd)) = (l?, r?);
                let mut c = TextCompare {
                    left_crlf: lb.windows(2).any(|w| w == b"\r\n"),
                    right_crlf: rb.windows(2).any(|w| w == b"\r\n"),
                    left_newline: lb.ends_with(b"\n"),
                    right_newline: rb.ends_with(b"\n"),
                    left_utf8: std::str::from_utf8(&lb).is_ok(),
                    right_utf8: std::str::from_utf8(&rb).is_ok(),
                    left_hash: ld.then(|| glint_core::filediff::content_hash(&lb)),
                    right_hash: rd.then(|| glint_core::filediff::content_hash(&rb)),
                    engine: "lines".into(),
                    elapsed: 3.0,
                    ..Default::default()
                };
                if too_big {
                    c.too_big = true;
                    return Ok(c);
                }
                if lb.contains(&0) || rb.contains(&0) {
                    c.binary = true;
                    return Ok(c);
                }
                let text = |b: &[u8]| String::from_utf8_lossy(b).replace("\r\n", "\n");
                c.left = text(&lb).lines().map(str::to_string).collect();
                c.right = text(&rb).lines().map(str::to_string).collect();
                for i in 0..c.left.len().max(c.right.len()) {
                    let at = |n: usize, len: usize| u32::try_from(i).ok().filter(|_| i < len).map(|_| n as u32);
                    c.rows.push([at(i, c.left.len()), at(i, c.right.len())]);
                    let i32_ = i as u32;
                    match (c.left.get(i), c.right.get(i)) {
                        (Some(a), Some(b)) if a != b => {
                            let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
                            let start = utf16_len(&a.chars().take(common).collect::<String>());
                            let start_b = utf16_len(&b.chars().take(common).collect::<String>());
                            c.left_changes.insert(i32_, vec![[start, utf16_len(a)]]);
                            c.right_changes.insert(i32_, vec![[start_b, utf16_len(b)]]);
                        }
                        (Some(_), None) => {
                            c.left_changes.insert(i32_, Vec::new());
                        }
                        (None, Some(_)) => {
                            c.right_changes.insert(i32_, Vec::new());
                        }
                        _ => {}
                    }
                }
                Ok(c)
            })
        }

        fn write(&self, path: &str, content: String, expected: Option<String>) -> Fut<String> {
            lock(&self.asked).push(format!("write {path}"));
            let now = self.read(path).ok();
            let changed = expected.is_some_and(|e| now.as_deref().map(glint_core::filediff::content_hash) != Some(e));
            if !changed {
                lock(&self.files).insert(path.to_string(), content.clone().into_bytes());
            }
            Box::pin(async move {
                if changed {
                    Err("it was changed or removed since it was compared. Compare again first".into())
                } else {
                    Ok(glint_core::filediff::content_hash(content.as_bytes()))
                }
            })
        }

        fn bytes(&self, left: &str, right: &str, offset: u64, len: u32) -> Fut<BytesWindow> {
            let both = (self.read(left), self.read(right));
            Box::pin(async move {
                let (l, r) = (both.0?, both.1?);
                let cut = |b: &[u8]| {
                    let from = usize::try_from(offset).unwrap_or(usize::MAX).min(b.len());
                    b[from..(from + len as usize).min(b.len())].to_vec()
                };
                Ok(BytesWindow {
                    left_size: l.len() as u64,
                    right_size: r.len() as u64,
                    left: cut(&l),
                    right: cut(&r),
                })
            })
        }

        fn next_difference(&self, left: &str, right: &str, from: u64) -> Fut<Option<u64>> {
            let both = (self.read(left), self.read(right));
            Box::pin(async move {
                let (l, r) = (both.0?, both.1?);
                let end = l.len().max(r.len());
                Ok((usize::try_from(from).unwrap_or(usize::MAX)..end)
                    .find(|&i| l.get(i) != r.get(i))
                    .map(|i| i as u64))
            })
        }

        fn prev_difference(&self, left: &str, right: &str, before: u64) -> Fut<Option<u64>> {
            let both = (self.read(left), self.read(right));
            Box::pin(async move {
                let (l, r) = (both.0?, both.1?);
                let end = usize::try_from(before).unwrap_or(usize::MAX).min(l.len().max(r.len()));
                // The first differing byte of the last row (of 16) that has one.
                Ok((0..end).rev().find(|&i| l.get(i) != r.get(i)).map(|i| {
                    let row = i & !15;
                    (row..=i).find(|&k| l.get(k) != r.get(k)).unwrap_or(i) as u64
                }))
            })
        }
    }

    // ---- Driving the window ------------------------------------------------------------------------------------

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

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

    fn has_id(cx: &mut VisualTestContext, id: impl Into<ElementId> + Clone) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn rows_drawn(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::Row)
                .iter()
                .filter(|row| row.label().is_some_and(|l| l.contains('|')))
                .count()
        })
    }

    /// Lets the work on core's runtime and the background come back to the foreground until `done`.
    fn settle(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..1000 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("{what} never happened");
    }

    fn open_fake(
        cx: &mut VisualTestContext,
        fake: &Arc<Fake>,
        left: Side,
        right: Side,
        on_close: Option<OnClose>,
    ) -> Entity<CompareDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let dialog = cx.update(|window, cx| open_with(window, cx, backend, None, left, right, on_close));
        cx.run_until_parked();
        dialog
    }

    /// Opens `a.txt` and `b.txt` of the fake and waits for their rows.
    fn open_texts(cx: &mut VisualTestContext, fake: &Arc<Fake>) -> Entity<CompareDialog> {
        let dialog = open_fake(cx, fake, Side::new("C:\\w\\a.txt"), Side::new("C:\\w\\b.txt"), None);
        settle(cx, "the rows", |cx| has(cx, "Side by side"));
        dialog
    }

    fn text_pane(cx: &VisualTestContext, dialog: &Entity<CompareDialog>) -> Entity<TextPane> {
        dialog
            .read_with(cx, |d, _| d.text_pane().cloned())
            .expect("the text view")
    }

    fn model_of(cx: &VisualTestContext, dialog: &Entity<CompareDialog>) -> Entity<TextModel> {
        text_pane(cx, dialog).read_with(cx, |p, _| p.model().clone())
    }

    fn pair() -> Arc<Fake> {
        Fake::with(&[
            ("C:\\w\\a.txt", "one\ntwo\nthree\n"),
            ("C:\\w\\b.txt", "one\n2\nthree\nfour\n"),
        ])
    }

    // ---- The text view -----------------------------------------------------------------------------------------

    #[gpui_kit::test]
    fn rows_are_labelled_and_the_changes_counted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        settle(&mut visual, "the rows", |cx| has(cx, "2 changes −1 +2 lines"));
        // The dialog, its title and the table have names and roles.
        assert!(has(&mut visual, "Compare a.txt and b.txt"));
        assert_eq!(role_of(&mut visual, "Side by side"), Some(Role::Table));
        assert_eq!(role_of(&mut visual, "2 changes −1 +2 lines"), Some(Role::Status));
        visual.update(|window, _| {
            assert!(!window.find_all_by_role(Role::Dialog).is_empty());
            // Every row says its lines on both sides.
            assert_eq!(
                window.try_find_by_label("1: one | 1: one").and_then(|e| e.role()),
                Some(Role::Row)
            );
            assert_eq!(
                window.try_find_by_label("2: two | 2: 2").and_then(|e| e.role()),
                Some(Role::Row)
            );
            assert_eq!(
                window.try_find_by_label(" | 4: four").and_then(|e| e.role()),
                Some(Role::Row)
            );
            // The files' names are groups, and the engine is told.
            assert_eq!(
                window.try_find_by_label("a.txt").and_then(|e| e.role()),
                Some(Role::Group)
            );
        });
        // The first change is the current one.
        assert_eq!(
            text_pane(&visual, &dialog).read_with(&visual, |p, _| p.current_change()),
            Some(0)
        );
        assert!(has(&mut visual, "1 of 2"));
        assert!(has(&mut visual, "Previous change (Alt+Up)") && has(&mut visual, "Next change (Alt+Down)"));
        // Only the right one asked glint-core (and not with edits).
        assert_eq!(fake.asked(), ["compare C:\\w\\a.txt C:\\w\\b.txt 00"]);
    }

    #[gpui_kit::test]
    fn the_same_text_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::with(&[("C:\\w\\a.txt", "same\n"), ("C:\\w\\b.txt", "same\n")]);
        let _dialog = open_texts(&mut visual, &fake);
        settle(&mut visual, "the verdict", |cx| has(cx, "The same text"));
        assert_eq!(role_of(&mut visual, "The same text"), Some(Role::Status));
        // Nothing to go to: the arrows are off.
        visual.update(|window, _| {
            let next = window.find("compare-next");
            assert_eq!(next.disabled(), Some(true), "no next change in the same text");
        });
    }

    #[gpui_kit::test]
    fn alt_down_and_alt_up_walk_the_changes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        let pane = text_pane(&visual, &dialog);
        let at = |cx: &VisualTestContext| pane.read_with(cx, |p, _| p.current_change());
        assert_eq!(at(&visual), Some(0));
        visual.update(|window, cx| window.press("alt-down", cx));
        assert_eq!(at(&visual), Some(1));
        assert!(has(&mut visual, "2 of 2"));
        // The last change: Next stops there.
        visual.update(|window, cx| window.press("alt-down", cx));
        assert_eq!(at(&visual), Some(1));
        visual.update(|window, cx| window.press("alt-up", cx));
        assert_eq!(at(&visual), Some(0));
        // The buttons do the same.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-next", cx);
        });
        assert_eq!(at(&visual), Some(1));
    }

    #[gpui_kit::test]
    fn copying_a_change_across_edits_the_target_and_ctrl_s_saves_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        assert!(!dialog.read_with(&visual, |d, cx| d.dirty(cx)));
        // Alt+Right copies the current change (line 2) from the left file into the right one.
        visual.update(|window, cx| window.press("alt-right", cx));
        settle(&mut visual, "the edit's comparison", |cx| has(cx, "Modified"));
        assert!(dialog.read_with(&visual, |d, cx| d.dirty(cx)));
        assert!(unsaved_for_test(&mut visual));
        // The edited text was compared, not the disk's.
        assert!(fake.asked().iter().any(|a| a.ends_with(" 01")), "{:?}", fake.asked());
        // Only the right file is Modified, and it has a Save button; the left has none.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("compare-save", 1usize)).is_some());
            assert!(window.try_find(("compare-save", 0usize)).is_none());
        });
        assert!(has(&mut visual, "Undo last merge or edit (Ctrl+Z)"));
        // The file on disk is untouched until it is saved.
        assert_eq!(fake.text("C:\\w\\b.txt"), "one\n2\nthree\nfour\n");
        visual.update(|window, cx| window.press("ctrl-s", cx));
        settle(&mut visual, "the save", |cx| !dialog.read_with(cx, |d, cx| d.dirty(cx)));
        assert_eq!(fake.text("C:\\w\\b.txt"), "one\ntwo\nthree\nfour\n");
        // The saved text is compared again from disk, and the first change is gone.
        settle(&mut visual, "the new comparison", |cx| has(cx, "1 change −0 +1 lines"));
        assert!(!has(&mut visual, "Modified"));
    }

    fn unsaved_for_test(cx: &mut VisualTestContext) -> bool {
        cx.update(|_, cx| unsaved(cx))
    }

    #[gpui_kit::test]
    fn copying_to_the_left_and_undo(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        let model = model_of(&visual, &dialog);
        visual.update(|window, cx| window.press("alt-left", cx));
        settle(&mut visual, "the edit", |cx| {
            model.read_with(cx, |m, _| m.dirty_side(0))
        });
        // The left file took the right one's "2" for its "two".
        settle(&mut visual, "the comparison", |cx| {
            model.read_with(cx, |m, _| !m.running())
        });
        let analysis = model.read_with(&visual, |m, _| m.analysis()).expect("compared");
        assert_eq!(analysis.compare.left, lines("one\n2\nthree"));
        // Undo (the button) takes it back.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-undo", cx);
        });
        settle(&mut visual, "the undo", |cx| !model.read_with(cx, |m, _| m.dirty()));
        settle(&mut visual, "the comparison", |cx| {
            model.read_with(cx, |m, _| !m.running())
        });
        assert!(!model.read_with(&visual, |m, _| m.can_undo()));
        assert_eq!(
            model
                .read_with(&visual, |m, _| m.analysis())
                .map(|a| a.compare.left.clone()),
            Some(lines("one\ntwo\nthree"))
        );
    }

    #[gpui_kit::test]
    fn ctrl_z_undoes_when_no_box_has_the_keyboard(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        let model = model_of(&visual, &dialog);
        visual.update(|window, cx| window.press("alt-right", cx));
        settle(&mut visual, "the edit", |cx| {
            model.read_with(cx, |m, _| m.dirty_side(1))
        });
        settle(&mut visual, "the comparison", |cx| {
            model.read_with(cx, |m, _| !m.running())
        });
        // Ctrl+Z is not an action of the Compare context (it may not be bound outside the file list): the
        // dialog's own key handler takes it.
        visual.update(|window, cx| window.press("ctrl-z", cx));
        settle(&mut visual, "the undo", |cx| !model.read_with(cx, |m, _| m.dirty()));
        assert!(!model.read_with(&visual, |m, _| m.can_undo()));
    }

    #[gpui_kit::test]
    fn a_click_on_the_minimap_goes_to_the_nearest_change(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let a: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 5\n", "LINE 5\n").replace("line 30\n", "LINE 30\n");
        let fake = Fake::with(&[("C:\\w\\a.txt", &a), ("C:\\w\\b.txt", &b)]);
        let dialog = open_texts(&mut visual, &fake);
        let pane = text_pane(&visual, &dialog);
        assert_eq!(pane.read_with(&visual, |p, _| p.current_change()), Some(0));
        // The strip's bounds are measured as it is drawn; the next frame knows them.
        visual.update(|window, cx| window.render_frame(cx));
        visual.update(|window, cx| window.render_frame(cx));
        let bounds = visual.update(|window, _| window.find("compare-minimap").bounds());
        // Near the bottom of the strip: the change at line 30 (row 30 of 40).
        visual.update(|window, cx| {
            window.click_at(
                "compare-minimap",
                point(bounds.size.width / 2., bounds.size.height * 0.8),
                cx,
            )
        });
        assert_eq!(pane.read_with(&visual, |p, _| p.current_change()), Some(1));
        visual.update(|window, cx| {
            window.click_at(
                "compare-minimap",
                point(bounds.size.width / 2., bounds.size.height * 0.1),
                cx,
            )
        });
        assert_eq!(pane.read_with(&visual, |p, _| p.current_change()), Some(0));
    }

    #[gpui_kit::test]
    fn a_read_only_side_is_never_edited_or_offered_the_arrows(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_fake(
            &mut visual,
            &fake,
            Side::new("C:\\w\\a.txt"),
            Side::named("C:\\w\\b.txt", "In the last commit", true),
            None,
        );
        settle(&mut visual, "the rows", |cx| has(cx, "Side by side"));
        let model = model_of(&visual, &dialog);
        assert!(model.read_with(&visual, |m, _| m.can_edit(0) && !m.can_edit(1)));
        // Copying into it does nothing.
        visual.update(|window, cx| window.press("alt-right", cx));
        visual.run_until_parked();
        assert!(!model.read_with(&visual, |m, _| m.dirty()));
        // The label is its name; the file name is shown beside it.
        assert!(has(&mut visual, "In the last commit"));
        assert!(has(&mut visual, "Compare a.txt and In the last commit"));
        // Only the left file's arrow into... the right (read-only) isn't drawn; the left's (to the left) is.
        visual.update(|window, cx| {
            window.render_frame(cx);
            // The first change is on the list's row 1: its arrows are numbered 2 (to the left) and 3 (to the right).
            assert!(window.try_find(("compare-copy", 2usize)).is_some(), "copy to the left");
            assert!(
                window.try_find(("compare-copy", 3usize)).is_none(),
                "no copy to the read-only right"
            );
        });
    }

    #[gpui_kit::test]
    fn a_file_changed_elsewhere_is_not_overwritten_without_asking(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        visual.update(|window, cx| window.press("alt-right", cx));
        settle(&mut visual, "the edit", |cx| has(cx, "Modified"));
        // Someone else writes the file.
        fake.put_bytes("C:\\w\\b.txt", b"one\nsomething else\nthree\n");
        visual.update(|window, cx| window.press("ctrl-s", cx));
        settle(&mut visual, "the question", |cx| has(cx, "Couldn't save b.txt"));
        // Nothing was written, the edits are still there, and it asks.
        assert_eq!(fake.text("C:\\w\\b.txt"), "one\nsomething else\nthree\n");
        assert!(dialog.read_with(&visual, |d, cx| d.dirty(cx)));
        // Cancel leaves it all as it is.
        visual.update(|window, cx| window.click("compare-ask-cancel", cx));
        visual.run_until_parked();
        assert_eq!(fake.text("C:\\w\\b.txt"), "one\nsomething else\nthree\n");
        // Asking again and saying "Save anyway" writes it.
        visual.update(|window, cx| window.press("ctrl-s", cx));
        settle(&mut visual, "the question", |cx| has(cx, "Couldn't save b.txt"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-ask-ok", cx);
        });
        settle(&mut visual, "the save", |cx| !dialog.read_with(cx, |d, cx| d.dirty(cx)));
        assert_eq!(fake.text("C:\\w\\b.txt"), "one\ntwo\nthree\nfour\n");
    }

    #[gpui_kit::test]
    fn crlf_files_are_written_back_with_crlf(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::with(&[
            ("C:\\w\\a.txt", "one\r\ntwo\r\nthree"),
            ("C:\\w\\b.txt", "one\r\n2\r\nthree\r\n"),
        ]);
        let dialog = open_texts(&mut visual, &fake);
        visual.update(|window, cx| window.press("alt-right", cx));
        settle(&mut visual, "the edit", |cx| has(cx, "Modified"));
        visual.update(|window, cx| window.press("ctrl-s", cx));
        settle(&mut visual, "the save", |cx| !dialog.read_with(cx, |d, cx| d.dirty(cx)));
        assert_eq!(
            fake.text("C:\\w\\b.txt"),
            "one\r\ntwo\r\nthree\r\n",
            "the right file's own endings"
        );
    }

    #[gpui_kit::test]
    fn closing_with_edits_asks_and_keep_editing_stays(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        visual.update(|window, cx| window.press("alt-right", cx));
        settle(&mut visual, "the edit", |cx| has(cx, "Modified"));
        // Escape asks first.
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        assert!(has(&mut visual, "Unsaved edits"));
        assert_eq!(role_of(&mut visual, "Unsaved edits"), Some(Role::Group));
        // Keep editing: the question goes, the comparison stays with its edits.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-ask-cancel", cx);
        });
        visual.run_until_parked();
        assert!(!has(&mut visual, "Unsaved edits"));
        assert!(dialog.read_with(&visual, |d, cx| d.dirty(cx)));
        assert!(has(&mut visual, "Side by side"));
        // Discard closes both.
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-ask-ok", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Discard closes the comparison too"));
        assert_eq!(fake.text("C:\\w\\b.txt"), "one\n2\nthree\nfour\n", "nothing was saved");
    }

    #[gpui_kit::test]
    fn closing_without_edits_just_closes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let _dialog = open_texts(&mut visual, &fake);
        visual.update(|window, cx| {
            // The keyboard starts inside the dialog.
            assert!(window.focused(cx).is_some(), "the keyboard is in the dialog");
            window.press("escape", cx)
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape closes it"));
    }

    #[gpui_kit::test]
    fn on_close_runs_once_when_the_comparison_is_gone(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let closed = Rc::new(RefCell::new(0));
        let flag = closed.clone();
        // The dialog layer holds the dialog until its exit has finished; the release hook is the dialog's own.
        let backend: Arc<dyn Backend> = fake.clone();
        let dialog = visual.update(|window, cx| {
            cx.new(|cx| {
                CompareDialog::new(
                    backend,
                    None,
                    Side::named(r"C:\w\a.txt", "In the last commit", true),
                    Side::new(r"C:\w\b.txt"),
                    Some(Box::new(move |_| *flag.borrow_mut() += 1)),
                    window,
                    cx,
                )
            })
        });
        visual.run_until_parked();
        assert_eq!(*closed.borrow(), 0);
        drop(dialog);
        visual.update(|_, _| ());
        visual.run_until_parked();
        assert_eq!(*closed.borrow(), 1, "on_close ran once");
    }

    #[gpui_kit::test]
    fn the_context_view_folds_what_is_the_same_and_a_click_unfolds_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let a: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 5\n", "LINE 5\n").replace("line 30\n", "LINE 30\n");
        let fake = Fake::with(&[("C:\\w\\a.txt", &a), ("C:\\w\\b.txt", &b)]);
        let dialog = open_texts(&mut visual, &fake);
        let pane = text_pane(&visual, &dialog);
        assert_eq!(pane.read_with(&visual, |p, _| p.shown().len()), 40);
        // Context: three rows around each change, the rest folded.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("compare-filter", 1usize), cx);
        });
        visual.update(|window, cx| window.render_frame(cx));
        let shown = pane.read_with(&visual, |p, _| p.shown().to_vec());
        let folds: Vec<usize> = shown
            .iter()
            .filter_map(|i| match i {
                Item::Fold { count, .. } => Some(*count),
                _ => None,
            })
            .collect();
        assert_eq!(folds, [2, 18, 6], "{shown:?}");
        assert!(has(&mut visual, "18 unchanged lines — click to expand"));
        assert_eq!(
            role_of(&mut visual, "18 unchanged lines — click to expand"),
            Some(Role::Button)
        );
        // Clicking a fold opens it.
        let at = shown
            .iter()
            .position(|i| matches!(i, Item::Fold { count: 18, .. }))
            .expect("the fold");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("compare-fold", at), cx);
        });
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(pane.read_with(&visual, |p, _| p.shown().len()), shown.len() - 1 + 18);
        // Diffs: the changed rows alone; All: back to every row.
        visual.update(|window, cx| window.click(("compare-filter", 2usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(pane.read_with(&visual, |p, _| p.shown().len()), 2);
        visual.update(|window, cx| window.click(("compare-filter", 0usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(pane.read_with(&visual, |p, _| p.shown().len()), 40);
    }

    #[gpui_kit::test]
    fn ctrl_f_finds_in_both_files_and_enter_steps_through_the_matches(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let a: String = (0..30).map(|i| format!("row {i} alpha\n")).collect();
        let b = a.replace("row 20 alpha", "row 20 beta alpha");
        let fake = Fake::with(&[("C:\\w\\a.txt", &a), ("C:\\w\\b.txt", &b)]);
        let dialog = open_texts(&mut visual, &fake);
        let pane = text_pane(&visual, &dialog);
        assert!(!has(&mut visual, "Find in files"));
        visual.update(|window, cx| window.press("ctrl-f", cx));
        visual.run_until_parked();
        assert_eq!(role_of(&mut visual, "Find in files"), Some(Role::Search));
        // The box has the keyboard: typing goes into it.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.input("row 2", cx)
        });
        settle(&mut visual, "the matches", |cx| has(cx, "1 of 11 matching rows"));
        // Enter goes to the first match, then the next; Shift+Enter back.
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert!(has(&mut visual, "1 of 11 matching rows"));
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert!(has(&mut visual, "2 of 11 matching rows"));
        visual.update(|window, cx| window.press("shift-enter", cx));
        visual.run_until_parked();
        assert!(has(&mut visual, "1 of 11 matching rows"));
        // No hit says so.
        visual.update(|window, cx| window.input("zzz", cx));
        settle(&mut visual, "no matches", |cx| has(cx, "No matches"));
        // Escape closes only the bar, and the dialog stays.
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        assert!(!has(&mut visual, "Find in files"));
        visual.update(|window, cx| {
            assert!(
                window.has_active_dialog(cx),
                "Escape closed the find bar, not the dialog"
            )
        });
        assert!(has(&mut visual, "Side by side"));
        drop(pane);
    }

    #[gpui_kit::test]
    fn a_double_click_edits_a_line_and_enter_commits_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        let model = model_of(&visual, &dialog);
        // Row 2 (index 1), the right half: "2".
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.double_click(("compare-half", 3usize), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            // The box has the keyboard and the line's text.
            assert!(window.has_focused_input(cx), "the line's box has the keyboard");
            window.input("!", cx);
            window.press("enter", cx);
        });
        settle(&mut visual, "the edit", |cx| {
            model.read_with(cx, |m, _| m.dirty_side(1))
        });
        settle(&mut visual, "the comparison", |cx| {
            model.read_with(cx, |m, _| !m.running())
        });
        let edited = model
            .read_with(&visual, |m, _| m.analysis())
            .map(|a| a.compare.right[1].clone());
        assert!(
            edited.is_some_and(|line| line.chars().count() == 2 && line.contains('!')),
            "the line took the typing"
        );
        assert!(has(&mut visual, "Modified"));
        // The keyboard is back for Alt+Down.
        visual.update(|window, cx| assert!(!window.has_focused_input(cx)));
    }

    #[gpui_kit::test]
    fn escape_cancels_a_line_edit_without_closing_the_dialog(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        let model = model_of(&visual, &dialog);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.double_click(("compare-half", 3usize), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.input("zzz", cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "the dialog stays"));
        assert!(!model.read_with(&visual, |m, _| m.dirty()), "nothing was edited");
    }

    #[gpui_kit::test]
    fn a_non_utf8_side_is_read_only_and_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        fake.put_bytes("C:\\w\\b.txt", b"one\n\xff\xfe\nthree\n");
        let dialog = open_texts(&mut visual, &fake);
        let model = model_of(&visual, &dialog);
        assert!(model.read_with(&visual, |m, _| m.can_edit(0) && !m.can_edit(1)));
        assert_eq!(role_of(&mut visual, "Read-only: not UTF-8"), Some(Role::Status));
        visual.update(|window, cx| window.press("alt-right", cx));
        visual.run_until_parked();
        assert!(
            !model.read_with(&visual, |m, _| m.dirty()),
            "copying into a non-UTF-8 file is refused"
        );
    }

    #[gpui_kit::test]
    fn whitespace_and_case_options_compare_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        let model = model_of(&visual, &dialog);
        // WS: Exact -> Trim -> Ignore -> Exact (the button is named by what a click does).
        const EXACT: &str = "Whitespace: comparing all whitespace (click to ignore trailing)";
        const TRIM: &str = "Whitespace: ignoring trailing whitespace (click to ignore all whitespace)";
        const IGNORE: &str = "Whitespace: ignoring all whitespace (click to compare all)";
        assert!(has(&mut visual, EXACT));
        visual.update(|window, cx| window.click("compare-whitespace", cx));
        settle(&mut visual, "WS: Trim", |cx| has(cx, TRIM));
        assert_eq!(
            model.read_with(&visual, |m, _| m.options().ignore_whitespace),
            IgnoreWhitespace::Trailing
        );
        visual.update(|window, cx| window.click("compare-whitespace", cx));
        settle(&mut visual, "WS: Ignore", |cx| has(cx, IGNORE));
        visual.update(|window, cx| window.click("compare-whitespace", cx));
        settle(&mut visual, "WS: Exact", |cx| has(cx, EXACT));
        visual.update(|window, cx| window.click("compare-case", cx));
        settle(&mut visual, "ignoring case", |cx| {
            model.read_with(cx, |m, _| m.options().ignore_case)
        });
        settle(&mut visual, "the comparison", |cx| {
            model.read_with(cx, |m, _| !m.running())
        });
        // The Difftastic switch isn't there without the tool.
        assert!(!has_id(&mut visual, "compare-ast"));
    }

    #[gpui_kit::test]
    fn a_hundred_thousand_lines_draw_only_the_rows_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let a: String = (0..100_000)
            .map(|i| format!("{i} the quick brown fox jumps over the lazy dog\n"))
            .collect();
        // A change every thousand lines.
        let mut b = String::with_capacity(a.len());
        for (i, line) in a.lines().enumerate() {
            if i % 1000 == 500 {
                b.push_str(&format!("{i} changed: the quick brown cat\n"));
            } else {
                b.push_str(line);
                b.push('\n');
            }
        }
        let fake = Fake::with(&[("C:\\w\\a.txt", &a), ("C:\\w\\b.txt", &b)]);
        let started = std::time::Instant::now();
        let dialog = open_texts(&mut visual, &fake);
        settle(&mut visual, "the changes", |cx| has(cx, "100 changes −100 +100 lines"));
        let first_frame = started.elapsed();
        let pane = text_pane(&visual, &dialog);
        assert_eq!(pane.read_with(&visual, |p, _| p.shown().len()), 100_000);
        // A screenful is drawn, not a hundred thousand.
        let drawn = rows_drawn(&mut visual);
        assert!(drawn > 5 && drawn < 120, "{drawn} rows drawn");
        // Next change a hundred times: each is a jump that draws a screenful again.
        let walked = std::time::Instant::now();
        for _ in 0..50 {
            visual.update(|window, cx| window.press("alt-down", cx));
            visual.update(|window, cx| window.render_frame(cx));
        }
        assert_eq!(pane.read_with(&visual, |p, _| p.current_change()), Some(50));
        let drawn = rows_drawn(&mut visual);
        assert!(drawn > 5 && drawn < 120, "{drawn} rows drawn after walking");
        // The row of the current change (line 50,500, which is the 51st change) is on screen.
        assert!(has(
            &mut visual,
            "50501: 50500 the quick brown fox jumps over the lazy dog | 50501: 50500 changed: the quick brown cat"
        ));
        let jumps = walked.elapsed();
        // A frame with nothing new (scrolling redraws the list the same way).
        let still = std::time::Instant::now();
        for _ in 0..20 {
            visual.update(|window, cx| {
                cx.notify(pane.entity_id());
                window.render_frame(cx)
            });
        }
        eprintln!(
            "100,000 lines: first frame {first_frame:?}, 50 jumps {jumps:?}, a redraw {:?}",
            still.elapsed() / 20
        );
    }

    #[gpui_kit::test]
    fn arabic_names_and_lines_and_the_mirrored_chrome_keep_the_files_in_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::with(&[
            ("C:\\w\\تقرير.txt", "السطر الأول\nالسطر الثاني\n"),
            ("C:\\w\\نسخة.txt", "السطر الأول\nسطر آخر\nثالث\n"),
        ]);
        let dialog = open_fake(
            &mut visual,
            &fake,
            Side::new("C:\\w\\تقرير.txt"),
            Side::new("C:\\w\\نسخة.txt"),
            None,
        );
        settle(&mut visual, "the rows", |cx| has(cx, "Side by side"));
        // Names appear as the files are called, without the layout-only isolates, in the order given.
        assert_eq!(role_of(&mut visual, "تقرير.txt"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "نسخة.txt"), Some(Role::Group));
        assert!(has(&mut visual, "Compare تقرير.txt and نسخة.txt"));
        assert!(has(&mut visual, "2: السطر الثاني | 2: سطر آخر"));
        assert_eq!(dialog.read_with(&visual, |d, _| d.sides().0.name()), "تقرير.txt");
        // Find in Arabic.
        visual.update(|window, cx| window.press("ctrl-f", cx));
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.input("سطر", cx)
        });
        settle(&mut visual, "the matches", |cx| has(cx, "1 of 2 matching rows"));
        // Swap puts the Arabic names the other way round.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-swap", cx);
        });
        visual.run_until_parked();
        assert_eq!(dialog.read_with(&visual, |d, _| d.sides().0.name()), "نسخة.txt");
        assert!(has(&mut visual, "Compare نسخة.txt and تقرير.txt"));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn swap_exchanges_the_sides_and_the_view_buttons_switch_views(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        assert_eq!(dialog.read_with(&visual, |d, _| d.views()), [View::Text, View::Bytes]);
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-swap", cx);
        });
        visual.run_until_parked();
        settle(&mut visual, "the swapped rows", |cx| has(cx, "Compare b.txt and a.txt"));
        assert_eq!(
            dialog.read_with(&visual, |d, _| d.sides().0.path.clone()),
            "C:\\w\\b.txt"
        );
        settle(&mut visual, "the rows", |cx| has(cx, "2: 2 | 2: two"));
        // Bytes, then back to text.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("compare-view", 1usize), cx);
        });
        visual.run_until_parked();
        assert_eq!(dialog.read_with(&visual, |d, _| d.view()), View::Bytes);
        settle(&mut visual, "the bytes", |cx| has(cx, "Bytes side by side"));
        visual.update(|window, cx| window.click(("compare-view", 0usize), cx));
        visual.run_until_parked();
        assert_eq!(dialog.read_with(&visual, |d, _| d.view()), View::Text);
    }

    #[gpui_kit::test]
    fn swapping_with_edits_asks_first(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        let dialog = open_texts(&mut visual, &fake);
        visual.update(|window, cx| window.press("alt-right", cx));
        settle(&mut visual, "the edit", |cx| has(cx, "Modified"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-swap", cx);
        });
        visual.run_until_parked();
        assert!(has(&mut visual, "Unsaved edits"));
        assert!(
            !dialog.read_with(&visual, |d, _| d.sides().0.path == "C:\\w\\b.txt"),
            "not swapped yet"
        );
        // Discard swaps, and the edits are gone with the old view.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("compare-ask-ok", cx);
        });
        visual.run_until_parked();
        assert_eq!(
            dialog.read_with(&visual, |d, _| d.sides().0.path.clone()),
            "C:\\w\\b.txt"
        );
        assert!(!dialog.read_with(&visual, |d, cx| d.dirty(cx)));
    }

    // ---- Binary and big files, and the bytes -----------------------------------------------------------------------

    #[gpui_kit::test]
    fn files_that_are_not_text_say_so_and_offer_their_bytes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = pair();
        fake.put_bytes("C:\\w\\a.txt", b"MZ\0\0\x01\x02\x03");
        fake.put_bytes("C:\\w\\b.txt", b"MZ\0\0\x01\x09\x03\x04");
        let dialog = open_fake(
            &mut visual,
            &fake,
            Side::new("C:\\w\\a.txt"),
            Side::new("C:\\w\\b.txt"),
            None,
        );
        settle(&mut visual, "the note", |cx| {
            has(cx, "They aren't text: their bytes can be compared.")
        });
        assert_eq!(
            role_of(&mut visual, "They aren't text: their bytes can be compared."),
            Some(Role::Status)
        );
        assert_eq!(role_of(&mut visual, "Compare their bytes"), Some(Role::Button));
        visual.update(|window, cx| window.click("compare-their-bytes", cx));
        settle(&mut visual, "the bytes", |cx| has(cx, "Bytes side by side"));
        assert_eq!(dialog.read_with(&visual, |d, _| d.view()), View::Bytes);
    }

    #[gpui_kit::test]
    fn files_over_eight_megabytes_say_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake {
            too_big: true,
            ..Default::default()
        });
        fake.put_bytes("C:\\w\\a.log", b"x");
        fake.put_bytes("C:\\w\\b.log", b"y");
        let _dialog = open_fake(
            &mut visual,
            &fake,
            Side::new("C:\\w\\a.log"),
            Side::new("C:\\w\\b.log"),
            None,
        );
        settle(&mut visual, "the note", |cx| {
            has(cx, "Too big to compare as text here (over 8 MB).")
        });
        assert_eq!(role_of(&mut visual, "Compare their bytes"), Some(Role::Button));
    }

    #[gpui_kit::test]
    fn a_comparison_that_fails_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::with(&[("C:\\w\\a.txt", "x")]);
        let _dialog = open_fake(
            &mut visual,
            &fake,
            Side::new("C:\\w\\a.txt"),
            Side::new("C:\\w\\missing.txt"),
            None,
        );
        settle(&mut visual, "the error", |cx| {
            has(cx, "Couldn't open C:\\w\\missing.txt: not found")
        });
        assert_eq!(
            role_of(&mut visual, "Couldn't open C:\\w\\missing.txt: not found"),
            Some(Role::Alert)
        );
    }

    fn open_bytes(cx: &mut VisualTestContext, fake: &Arc<Fake>) -> (Entity<CompareDialog>, Entity<BytesPane>) {
        let dialog = open_fake(cx, fake, Side::new("C:\\w\\a.bin"), Side::new("C:\\w\\b.bin"), None);
        cx.update(|window, cx| {
            dialog.update(cx, |d, cx| d.set_view(View::Bytes, window, cx));
        });
        settle(cx, "the bytes", |cx| has(cx, "Bytes side by side"));
        let pane = dialog
            .read_with(cx, |d, _| d.bytes_pane().cloned())
            .expect("the bytes view");
        (dialog, pane)
    }

    #[gpui_kit::test]
    fn the_bytes_view_goes_to_each_difference_in_turn(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut a = vec![7u8; 5000];
        let mut b = a.clone();
        b[0x20] = 1;
        b[0x25] = 2;
        b[0x1000] = 3;
        a.push(9);
        let fake = Fake::with(&[]);
        fake.put_bytes("C:\\w\\a.bin", &a);
        fake.put_bytes("C:\\w\\b.bin", &b);
        let (dialog, pane) = open_bytes(&mut visual, &fake);
        settle(&mut visual, "the first difference", |cx| {
            pane.read_with(cx, |p, _| p.shown_offset().is_some())
        });
        assert_eq!(pane.read_with(&visual, |p, _| p.first_difference()), Some(Some(0x20)));
        assert_eq!(pane.read_with(&visual, |p, _| p.shown_offset()), Some(0x20));
        assert!(has(&mut visual, "At 0x00000020"));
        // The row of it is drawn, labelled with its offset and both sides' bytes.
        visual.update(|window, cx| {
            window.render_frame(cx);
            let row = window
                .find_all_by_role(Role::Row)
                .into_iter()
                .find(|r| r.label().is_some_and(|l| l.starts_with("00000020:")));
            assert!(row.is_some(), "the row at 0x20 is drawn");
        });
        // Next: the same row has another (0x25), which is not a new row: the next after the row is 0x1000.
        visual.update(|window, cx| window.press("alt-down", cx));
        settle(&mut visual, "the next difference", |cx| {
            pane.read_with(cx, |p, _| p.shown_offset() == Some(0x1000))
        });
        assert!(has(&mut visual, "At 0x00001000"));
        // Past the last difference in the bytes both have: 0x1388 is where a is longer.
        visual.update(|window, cx| window.press("alt-down", cx));
        settle(&mut visual, "the end", |cx| {
            pane.read_with(cx, |p, _| p.shown_offset() == Some(5000))
        });
        // Previous goes back.
        visual.update(|window, cx| window.press("alt-up", cx));
        settle(&mut visual, "back", |cx| {
            pane.read_with(cx, |p, _| p.shown_offset() == Some(0x1000))
        });
        visual.update(|window, cx| window.press("alt-up", cx));
        settle(&mut visual, "back again", |cx| {
            pane.read_with(cx, |p, _| p.shown_offset() == Some(0x20))
        });
        // The first one: Previous is off.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-bytes-previous").disabled(), Some(true));
        });
        drop(dialog);
    }

    #[gpui_kit::test]
    fn the_bytes_view_says_when_they_are_the_same_and_goes_to_an_offset(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::with(&[]);
        fake.put_bytes("C:\\w\\a.bin", &[1u8; 300]);
        fake.put_bytes("C:\\w\\b.bin", &[1u8; 300]);
        let (_dialog, pane) = open_bytes(&mut visual, &fake);
        settle(&mut visual, "the verdict", |cx| has(cx, "The same, byte for byte"));
        assert_eq!(role_of(&mut visual, "The same, byte for byte"), Some(Role::Status));
        assert_eq!(pane.read_with(&visual, |p, _| p.first_difference()), Some(None));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-bytes-next").disabled(), Some(true));
        });
        // "Go to": hex with 0x, and a number beyond the end is ignored.
        pane.update_in(&mut visual, |p, window, cx| {
            p.offset.update(cx, |input, cx| input.set_value("0x40", window, cx));
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            pane.update(cx, |p, cx| p.offset.update(cx, |input, cx| input.focus(window, cx)));
        });
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert_eq!(pane.read_with(&visual, |p, _| p.shown_offset()), Some(0x40));
        pane.update_in(&mut visual, |p, window, cx| {
            p.offset.update(cx, |input, cx| input.set_value("0x9999", window, cx));
        });
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert_eq!(
            pane.read_with(&visual, |p, _| p.shown_offset()),
            Some(0x40),
            "past the end: ignored"
        );
    }

    // ---- Pictures -------------------------------------------------------------------------------------------------

    /// A BMP of `w` x `h` pixels, every one `bgra`, written to a temp file named `name`.
    fn write_bmp(name: &str, w: u32, h: u32, bgra: [u8; 4]) -> String {
        let dir = std::env::temp_dir().join(format!("glint-compare-pictures-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp folder");
        let path = dir.join(name);
        let pixels: Vec<u8> = (0..w * h).flat_map(|_| bgra).collect();
        std::fs::write(&path, bmp_of(w, h, &pixels)).expect("the picture is written");
        path.to_string_lossy().into_owned()
    }

    #[gpui_kit::test]
    fn two_pictures_open_in_the_pictures_view_and_the_pixel_diff_counts_what_differs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let a = write_bmp("a.bmp", 8, 4, [10, 20, 30, 255]);
        let b = write_bmp("b.bmp", 8, 4, [10, 20, 30, 255]);
        let c = write_bmp("c.bmp", 8, 4, [200, 20, 30, 255]);
        let fake = Fake::with(&[]);
        let dialog = open_fake(&mut visual, &fake, Side::new(&a), Side::new(&b), None);
        assert_eq!(
            dialog.read_with(&visual, |d, _| d.views()),
            [View::Text, View::Pictures, View::Bytes]
        );
        assert_eq!(
            dialog.read_with(&visual, |d, _| d.view()),
            View::Pictures,
            "pictures open as pictures"
        );
        let pane = dialog
            .read_with(&visual, |d, _| d.pictures_pane().cloned())
            .expect("the pictures view");
        settle(&mut visual, "both pictures", |cx| {
            pane.read_with(cx, |p, _| p.dimensions() == [Some((8, 4)); 2])
        });
        assert!(has(&mut visual, "8 × 4 · 8 × 4"));
        assert_eq!(role_of(&mut visual, "Show them"), Some(Role::Group));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window
                .find_all_by_label("a.bmp")
                .iter()
                .any(|e| e.role() == Some(Role::Image)));
        });
        // The modes.
        for (n, mode) in [
            (1usize, PictureMode::Swipe),
            (2, PictureMode::Onion),
            (0, PictureMode::Side),
        ] {
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.click(("compare-picture-mode", n), cx);
            });
            visual.run_until_parked();
            assert_eq!(pane.read_with(&visual, |p, _| p.mode()), mode);
            visual.update(|window, cx| window.render_frame(cx));
        }
        // The same pixels: nothing differs.
        visual.update(|window, cx| window.click(("compare-picture-mode", 3usize), cx));
        settle(&mut visual, "the diff", |cx| {
            pane.read_with(cx, |p, _| p.difference().is_some())
        });
        assert_eq!(pane.read_with(&visual, |p, _| p.difference()), Some((0, 32)));
        assert!(has(&mut visual, "Identical pixels"));
        // Zoom.
        visual.update(|window, cx| window.click("compare-zoom-in", cx));
        assert_eq!(pane.read_with(&visual, |p, _| p.zoom), 1.25);
        visual.update(|window, cx| window.click("compare-zoom-reset", cx));
        assert_eq!(pane.read_with(&visual, |p, _| p.zoom), 1.);
        // Pictures that do differ: every pixel of the third is another blue.
        let renderer = visual.update(|_, cx| cx.svg_renderer());
        let (first, third) = (
            decode(&a, renderer.clone()).expect("a"),
            decode(&c, renderer).expect("c"),
        );
        let (bmp, w, h, changed, total) = pixel_diff(&first, &third, 5).expect("a difference");
        assert_eq!((w, h, changed, total), (8, 4, 32, 32));
        assert_eq!(&bmp[..2], b"BM");
        let (_, _, _, same_changed, same_total) = pixel_diff(&first, &first, 0).expect("a result");
        assert_eq!((same_changed, same_total), (0, 32));
        // Pictures of other sizes compare over the bigger: what only one has differs.
        let small = decode(
            &write_bmp("small.bmp", 4, 4, [10, 20, 30, 255]),
            visual.update(|_, cx| cx.svg_renderer()),
        )
        .expect("small");
        let (_, w, h, changed, total) = pixel_diff(&first, &small, 5).expect("a difference");
        assert_eq!((w, h, total), (8, 4, 32));
        assert_eq!(
            changed, 16,
            "the right half of the first picture has nothing across from it"
        );
    }

    #[test]
    fn the_pixel_diff_picture_is_a_bmp_gpui_can_read() {
        let bmp = bmp_of(2, 1, &[1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(bmp.len(), 14 + 108 + 8);
        // Height is stored negative: the rows run top to bottom.
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), -1);
        assert_eq!(&bmp[bmp.len() - 8..], &[1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
