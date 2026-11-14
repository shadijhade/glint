//! Workbooks and Parquet files as tables (`SheetPreview` in DataPreview.tsx, `isSheetCandidate`, and the
//! `TableView` it draws with).
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `SheetPreview`                               | [`SheetPreview`]: the sheet read by glint-core, the sheets as tabs |
//! | `inspectSheet(path, sheet)`                  | [`SheetSource`] (glint-core's `inspect_sheet`, in the background) |
//! | `TableView` with `total`, `types`, `extra`   | the header, the filter and the grid below; [`Grid`] is its table |
//! | the picker's `role="tablist"` of sheets      | the "Sheets" tab list in the header                           |
//! | `isSheetCandidate`                           | [`is_sheet_candidate`]                                        |
//!
//! glint-core reads one sheet at a time: its first 1,000 rows and 200 columns (an .xlsx or .xlsb sheet is
//! streamed, so a big one isn't read whole), or a Parquet file's first 1,000 rows with each column's type. The
//! header counts the rows the sheet has, not the ones read, and a note under the grid says how many were read.
//! The grid is the kit's `DataTable`, which draws only the rows on screen; the filter box matches any cell, and a
//! click on a column's name sorts it (numbers by value in a number column, anything else by the user's language
//! with digits read as numbers).
//!
//! Picking another sheet reads it while the one before stays on screen (the TypeScript kept its state until the
//! new answer came), and starts the filter and the sort over, as the TypeScript's `key` on the table did.
//!
//! Not ported:
//! - The cell's full text as a hover tooltip (`title={val}`): a cell that doesn't fit ends in an ellipsis and the
//!   whole row is read by screen readers. The Query view has the whole value.
//! - The Previewer view of a workbook (Excel's own, with Office) is the frame's: it is `Body::Shell` in choose.rs.
//! - NOTE(port): the kit's `DataTable` cycles a column's sort descending, ascending, off; the TypeScript went
//!   ascending, descending, off.

use super::data::shown_rows;
use super::text::{big_icon, name_of, note_centered, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::{actions, core_bridge, t, theme};
use glint_core::data_lens::TablePreview;
use glint_core::sheet::SheetPreview as Sheet;
use glint_model::format::format_count;
use gpui_kit::Task;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

/// The extensions of workbooks and Parquet files (`SHEET_EXTS`).
const SHEET_EXTS: [&str; 11] = [
    "xlsx", "xlsm", "xltx", "xltm", "xlam", "xlsb", "xls", "xla", "xlt", "ods", "parquet",
];

/// Whether the file is a workbook or a Parquet file the preview reads, by its extension (`isSheetCandidate`, for
/// a file on disk).
pub fn is_sheet_candidate(path: &str) -> bool {
    SHEET_EXTS.contains(&super::text::ext_of(path).as_str())
}

/// Whether the file is a workbook (the Previewer view is offered for these) and not Parquet.
#[cfg(test)]
pub fn is_workbook(path: &str) -> bool {
    is_sheet_candidate(path) && super::text::ext_of(path) != "parquet"
}

/// Reads a sheet: the path and which sheet (None: the first one that isn't hidden).
pub type SheetSource =
    Arc<dyn Fn(String, Option<usize>) -> Pin<Box<dyn Future<Output = Result<Sheet, String>> + Send>> + Send + Sync>;

/// glint-core's `inspect_sheet`.
pub fn core_sheet_source() -> SheetSource {
    Arc::new(|path, sheet| Box::pin(glint_core::sheet::inspect_sheet(path, sheet)))
}

/// A column's name with its type when the file has one ("price · DOUBLE"), for its tooltip and screen readers.
pub fn column_title(name: &str, kind: Option<&str>) -> String {
    match kind.filter(|k| !k.is_empty()) {
        Some(kind) => format!("{name} · {kind}"),
        None => name.to_string(),
    }
}

/// The rows line of the header: `{cols} cols · {shown} of {total} rows` while a filter is on, else without
/// `shown of`. `total` is the rows the sheet has.
pub fn counts_text(columns: usize, shown: usize, total: usize, filtered: bool) -> SharedString {
    if filtered {
        t!(
            "{cols} cols · {shown} of {total}{plus} rows",
            cols = format_count(columns),
            shown = format_count(shown),
            total = format_count(total),
            plus = ""
        )
    } else {
        t!(
            "{cols} cols · {total}{plus} rows",
            cols = format_count(columns),
            total = format_count(total),
            plus = ""
        )
    }
}

// ---- The grid --------------------------------------------------------------------------------------------------

/// The grid's columns and rows for the kit's `DataTable`.
struct Grid {
    table: Arc<TablePreview>,
    /// A Parquet file's column types, in order (none for a workbook).
    types: Arc<Vec<String>>,
    /// The rows shown, as indices into the table's.
    order: Vec<usize>,
    query: String,
    /// The sorted column (the table's, without the # column) and whether it is descending.
    sort: Option<(usize, bool)>,
    widths: Vec<Pixels>,
}

impl Grid {
    fn new(table: Arc<TablePreview>, types: Arc<Vec<String>>, rem: Pixels) -> Self {
        // A column is as wide as its widest cell in the first rows (and its name, type and icon), between 3 and
        // 16 rems.
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
                    .max(
                        column_title(&column.name, types.get(i).map(String::as_str))
                            .chars()
                            .count()
                            + 4,
                    )
                    .min(40);
                rem * (widest as f32 * 0.42 + 1.75).clamp(3., 16.)
            })
            .collect();
        let order = shown_rows(&table, "", None);
        Self {
            table,
            types,
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

    fn kind(&self, column: usize) -> Option<&str> {
        self.types.get(column).map(String::as_str)
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
        Column::new(format!("column-{ix}"), column_title(&column.name, self.kind(ix - 1)))
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
        let title: SharedString = column_title(&column.name, self.kind(ix - 1)).into();
        row_start()
            .id(("sheet-th", ix))
            .w_full()
            .min_w_0()
            .gap(space(1.5))
            .tooltip(move |window, cx| tooltip(title.clone(), window, cx))
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
                    .child(isolate(column.name.clone())),
            )
            .children(self.kind(ix - 1).filter(|k| !k.is_empty()).map(|kind| {
                div()
                    .flex_none()
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(SharedString::from(kind.to_string()))
            }))
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

// ---- The previewer -----------------------------------------------------------------------------------------------

/// What the previewer shows.
enum Load {
    /// Nothing read yet.
    Waiting,
    /// The read failed, with why.
    Failed(SharedString),
    /// A sheet read, and its grid.
    Ready {
        sheet: Arc<Sheet>,
        grid: Entity<TableState<Grid>>,
    },
}

/// A workbook's sheet (with its others to pick from), or a Parquet file with its columns' types, as a table.
pub struct SheetPreview {
    path: String,
    source: SheetSource,
    load: Load,
    /// The sheet asked for (None: the first one that isn't hidden).
    wanted: Option<usize>,
    filter: Entity<InputState>,
    started: Instant,
    generation: u64,
    _read: Task<()>,
    _filter_events: Subscription,
}

impl SheetPreview {
    /// The previewer for the workbook or Parquet file at `path`. Reading happens in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_sheet_source(), window, cx)
    }

    /// [`SheetPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: SheetSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter rows…")));
        let events = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_changed(cx);
            }
        });
        let mut this = Self {
            path,
            source,
            load: Load::Waiting,
            wanted: None,
            filter,
            started: Instant::now(),
            generation: 0,
            _read: Task::ready(()),
            _filter_events: events,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file, from its first sheet.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.wanted = None;
            self.read(window, cx);
        }
    }

    /// Reads the sheet again (the file changed on disk).
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read(window, cx);
    }

    /// The sheet showing (its position among the sheets that aren't hidden), once read.
    pub fn sheet_index(&self) -> Option<usize> {
        match &self.load {
            Load::Ready { sheet, .. } => Some(sheet.sheet),
            _ => None,
        }
    }

    /// The names of the sheets there are to pick from (none for Parquet).
    #[cfg(test)]
    pub fn sheet_names(&self) -> Vec<String> {
        match &self.load {
            Load::Ready { sheet, .. } => sheet.sheets.clone(),
            _ => Vec::new(),
        }
    }

    /// The rows the grid shows now, as text.
    #[cfg(test)]
    pub fn shown_cells(&self, cx: &App) -> Vec<Vec<String>> {
        match &self.load {
            Load::Ready { sheet, grid } => {
                let delegate = grid.read(cx).delegate();
                delegate
                    .order
                    .iter()
                    .filter_map(|&i| sheet.table.rows.get(i).cloned())
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Picks a sheet (a click on its tab).
    pub fn select_sheet(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet_index() == Some(index) {
            return;
        }
        self.wanted = Some(index);
        self.read(window, cx);
    }

    /// Scrolls the rows a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Load::Ready { grid, .. } = &self.load else {
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

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.started = Instant::now();
        // The sheet on screen stays until the next one is read; a first read shows the wait.
        if matches!(self.load, Load::Failed(_)) {
            self.load = Load::Waiting;
        }
        let read = (self.source)(self.path.clone(), self.wanted);
        self._read = cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.load = match result {
                    Err(reason) => Load::Failed(reason.into()),
                    Ok(sheet) => {
                        // A new sheet starts the filter and the sort over.
                        this.filter.update(cx, |filter, cx| filter.set_value("", window, cx));
                        let sheet = Arc::new(sheet);
                        let table = Arc::new(sheet.table.clone());
                        let types = Arc::new(sheet.types.clone());
                        let rem = theme::rem_size(cx);
                        let grid = cx.new(|cx| {
                            TableState::new(Grid::new(table, types, rem), window, cx)
                                .col_movable(false)
                                .row_selectable(true)
                        });
                        Load::Ready { sheet, grid }
                    }
                };
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The filter box changed: the rows follow (the sort is kept by the grid).
    fn filter_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().to_string();
        if let Load::Ready { grid, .. } = &self.load {
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

    /// The sheets as tabs (`role="tablist"`), when there is more than one.
    fn picker(&self, sheet: &Sheet, t: &theme::Tokens, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if sheet.sheets.len() < 2 {
            return None;
        }
        Some(
            row_start()
                .id("sheet-tabs")
                .role(Role::TabList)
                .aria_label(t!("Sheets"))
                .test_support()
                .min_w_0()
                .max_w_full()
                .gap(space(0.5))
                .overflow_x_scroll()
                .children(sheet.sheets.iter().enumerate().map(|(i, name)| {
                    let active = i == sheet.sheet;
                    Button::new(("sheet-tab", i))
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(if active {
                                    if t.dark {
                                        t.foreground.opacity(0.12)
                                    } else {
                                        t.background
                                    }
                                } else {
                                    t.background.opacity(0.)
                                })
                                .foreground(if active { t.primary } else { t.muted_foreground })
                                .hover(t.accent)
                                .active(t.accent),
                        )
                        .compact()
                        .flex_none()
                        .h(px(24.))
                        .px(space(2.))
                        .text_size(theme::size::XXS)
                        .when(active, |b| b.font_weight(FontWeight::MEDIUM))
                        .label(isolate(name.clone()))
                        .selected(active)
                        .accessibility_label(SharedString::from(name.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| this.select_sheet(i, window, cx)))
                })),
        )
    }

    /// The header: the format, the counts, how long it took, the sheets, and the filter.
    fn header(&self, sheet: &Sheet, shown: usize, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let filtered = !self.filter.read(cx).value().trim().is_empty();
        let counts = counts_text(sheet.table.columns.len(), shown, sheet.total_rows as usize, filtered);
        let green = theme::tailwind(if t.dark { "emerald-400" } else { "emerald-600" }).unwrap_or(t.primary);
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
                    .id("sheet-format")
                    .role(Role::Group)
                    .aria_label(SharedString::from(sheet.table.format.clone()))
                    .test_support()
                    .gap(space(1.))
                    .px(space(2.))
                    .py(space(0.5))
                    .rounded(theme::size::RADIUS)
                    .bg(green.opacity(0.15))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(green)
                    .child(icon(IconName::Table2).size_3())
                    .child(SharedString::from(sheet.table.format.clone())),
            )
            .child(
                div()
                    .id("sheet-counts")
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
                    .child(format!("{:.2} ms", sheet.table.elapsed_ms)),
            )
            .children(self.picker(sheet, t, cx))
            .child(
                align_row(1.).ms(0.).flex_1().justify_end().child(
                    div()
                        .w(space(40.))
                        .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                            // Escape clears the filter before anything else takes it.
                            if event.keystroke.key == "escape" && !this.filter.read(cx).value().is_empty() {
                                this.filter.update(cx, |filter, cx| filter.set_value("", window, cx));
                                cx.stop_propagation();
                            }
                        }))
                        .child(
                            Input::new(&self.filter)
                                .small()
                                .cleanable(true)
                                .aria_label(t!("Filter rows…"))
                                .prefix(icon(IconName::Search).size_3().text_color(t.muted_foreground)),
                        ),
                ),
            )
    }
}

impl Render for SheetPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("sheet-preview")
            .role(Role::Group)
            .aria_label(t!("Table of {name}", name = name_of(&self.path)))
            .test_support()
            .size_full()
            .min_h_0()
            .text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let (sheet, grid) = match &self.load {
            Load::Waiting => return root.child(waiting(self.started, Some(t!("Reading the table…")), cx)),
            Load::Failed(reason) => return root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            Load::Ready { sheet, grid } => (sheet.clone(), grid.clone()),
        };
        let shown = grid.read(cx).delegate().order.len();
        let body = if sheet.table.rows.is_empty() {
            note_centered("sheet-empty", t!("This file is empty."), cx)
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    DataTable::new(&grid)
                        .bordered(false)
                        .accessibility_label(t!("Rows of {name}", name = name_of(&self.path))),
                )
                .into_any_element()
        };
        let note = sheet.table.truncated.then(|| {
            let text = t!(
                "Showing the first {count} rows of {total}. Open it to see the rest, or ask Query.",
                count = format_count(sheet.table.rows.len()),
                total = format_count(sheet.total_rows as usize)
            );
            div()
                .id("sheet-truncated")
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
        root.child(self.header(&sheet, shown, &t, cx))
            .child(body)
            .children(note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::data_lens::TableColumn;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    fn columns(list: &[(&str, &str)]) -> Vec<TableColumn> {
        list.iter()
            .map(|(name, kind)| TableColumn {
                name: (*name).into(),
                kind: (*kind).into(),
            })
            .collect()
    }

    /// A sheet read: `names` are the sheets there are, `at` the one shown.
    fn sheet_of(names: &[&str], at: usize, columns: Vec<TableColumn>, rows: Vec<Vec<String>>, types: &[&str]) -> Sheet {
        let total = rows.len();
        Sheet {
            table: TablePreview {
                format: if types.is_empty() { "XLSX" } else { "Parquet" }.into(),
                delimiter: String::new(),
                total_rows_sampled: total,
                columns,
                rows,
                truncated: false,
                elapsed_ms: 2.5,
            },
            sheets: names.iter().map(|n| (*n).to_string()).collect(),
            sheet: at,
            types: types.iter().map(|t| (*t).to_string()).collect(),
            total_rows: total as u64,
            total_cols: 3,
        }
    }

    fn cells(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|c| (*c).to_string()).collect())
            .collect()
    }

    /// A workbook of two sheets, the second named in Arabic, remembering which sheets were asked for.
    fn workbook() -> (SheetSource, Arc<Mutex<Vec<Option<usize>>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        let source: SheetSource = Arc::new(move |_, sheet| {
            if let Ok(mut log) = log.lock() {
                log.push(sheet);
            }
            let names = ["Sales", "المبيعات"];
            let at = sheet.unwrap_or(0);
            let read = if at == 0 {
                sheet_of(
                    &names,
                    0,
                    columns(&[("item", "text"), ("qty", "number"), ("paid", "boolean")]),
                    cells(&[&["pen", "10", "true"], &["book", "2", "false"], &["قلم", "300", "true"]]),
                    &[],
                )
            } else {
                sheet_of(
                    &names,
                    1,
                    columns(&[("البند", "text"), ("الكمية", "number")]),
                    cells(&[&["دفتر", "5"], &["حقيبة", "9"]]),
                    &[],
                )
            };
            Box::pin(async move { Ok(read) })
        });
        (source, asked)
    }

    fn open(cx: &mut TestAppContext, path: &str, source: SheetSource) -> (Entity<SheetPreview>, VisualTestContext) {
        cx.update(testing::init);
        let path = path.to_string();
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| SheetPreview::with_source(path, source, window, cx))
        });
        (view, visual)
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
        panic!("the sheet never arrived");
    }

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..3 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    fn ready(view: &Entity<SheetPreview>, visual: &mut VisualTestContext, sheet: usize) {
        settle(visual, |cx| view.read_with(cx, |v, _| v.sheet_index() == Some(sheet)));
        frames(visual);
    }

    #[test]
    fn workbooks_and_parquet_are_told_by_their_extension() {
        for path in [
            "C:\\a\\x.xlsx",
            "C:\\a\\x.XLSM",
            "C:\\a\\x.xlsb",
            "C:\\a\\x.xls",
            "C:\\a\\x.ods",
            "C:\\a\\x.parquet",
            "C:\\a\\x.xlam",
        ] {
            assert!(is_sheet_candidate(path), "{path}");
        }
        assert!(!is_sheet_candidate("C:\\a\\x.csv"));
        assert!(!is_sheet_candidate("C:\\a\\x.xlsx.bak"));
        assert!(is_workbook("C:\\a\\x.xlsx"));
        assert!(!is_workbook("C:\\a\\x.parquet"));
    }

    #[test]
    fn a_columns_title_has_its_type_when_the_file_gives_one() {
        assert_eq!(column_title("price", Some("DOUBLE")), "price · DOUBLE");
        assert_eq!(column_title("price", Some("")), "price");
        assert_eq!(column_title("السعر", None), "السعر");
    }

    #[test]
    fn the_counts_say_how_many_rows_the_sheet_has_and_how_many_the_filter_leaves() {
        assert_eq!(counts_text(3, 3, 1200, false), "3 cols · 1,200 rows");
        assert_eq!(counts_text(3, 40, 1200, true), "3 cols · 40 of 1,200 rows");
    }

    #[gpui_kit::test]
    fn the_first_sheet_shows_as_a_table_with_its_sheets_as_tabs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (source, asked) = workbook();
        let (view, mut visual) = open(cx, "C:\\data\\book.xlsx", source);
        // The read is under way: the UI thread didn't wait for it.
        assert!(view.read_with(&visual, |v, _| v.sheet_index().is_none()));
        ready(&view, &mut visual, 0);
        assert_eq!(asked.lock().map(|a| a.clone()).unwrap_or_default(), vec![None]);
        assert!(has(&mut visual, "XLSX"));
        assert!(has(&mut visual, "3 cols · 3 rows"));
        assert!(has(&mut visual, "Sales"), "the sheets are tabs");
        assert!(has(&mut visual, "المبيعات"));
        let tabs = visual.update(|window, _| window.find_all_by_role(Role::TabList).len());
        assert_eq!(tabs, 1);
        assert_eq!(view.read_with(&visual, |v, cx| v.shown_cells(cx).len()), 3);
        assert!(has(&mut visual, "قلم"), "Arabic cells show");
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn picking_a_sheet_reads_it_and_starts_the_filter_over(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, asked) = workbook();
        let (view, mut visual) = open(cx, "C:\\data\\book.xlsx", source);
        ready(&view, &mut visual, 0);
        // A filter on the first sheet...
        visual.update(|window, cx| {
            let filter = view.read(cx).filter.clone();
            filter.update(cx, |f, cx| f.focus(window, cx));
            window.input("bo", cx);
        });
        frames(&mut visual);
        assert_eq!(
            view.read_with(&visual, |v, cx| v.shown_cells(cx)),
            cells(&[&["book", "2", "false"]])
        );
        assert!(has(&mut visual, "3 cols · 1 of 3 rows"));
        // ...doesn't follow to the second.
        visual.update(|window, cx| window.click(("sheet-tab", 1_usize), cx));
        ready(&view, &mut visual, 1);
        assert_eq!(asked.lock().map(|a| a.clone()).unwrap_or_default(), vec![None, Some(1)]);
        assert_eq!(
            view.read_with(&visual, |v, cx| v.shown_cells(cx)),
            cells(&[&["دفتر", "5"], &["حقيبة", "9"]])
        );
        assert_eq!(
            view.read_with(&visual, |v, cx| v.filter.read(cx).value().to_string()),
            ""
        );
        assert!(has(&mut visual, "2 cols · 2 rows"));
        // Asking for the sheet that is showing reads nothing.
        visual.update(|window, cx| view.update(cx, |v, cx| v.select_sheet(1, window, cx)));
        assert_eq!(asked.lock().map(|a| a.len()).unwrap_or(0), 2);
    }

    #[gpui_kit::test]
    fn a_parquet_file_shows_its_columns_types_and_a_note_when_there_are_more_rows(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let source: SheetSource = Arc::new(|_, _| {
            let mut read = sheet_of(
                &[],
                0,
                columns(&[("id", "number"), ("name", "text")]),
                (0..1000).map(|i| vec![i.to_string(), format!("row {i}")]).collect(),
                &["INT64", "BYTE_ARRAY · String"],
            );
            read.table.truncated = true;
            read.total_rows = 250_000;
            Box::pin(async move { Ok(read) })
        });
        let (view, mut visual) = open(cx, "C:\\data\\events.parquet", source);
        ready(&view, &mut visual, 0);
        assert!(has(&mut visual, "Parquet"));
        // The header counts the file's rows, not the 1,000 read, and a note says so.
        assert!(has(&mut visual, "2 cols · 250,000 rows"));
        assert!(has(
            &mut visual,
            "Showing the first 1,000 rows of 250,000. Open it to see the rest, or ask Query."
        ));
        // No sheets to pick from.
        assert!(view.read_with(&visual, |v, _| v.sheet_names().is_empty()));
        let tabs = visual.update(|window, _| window.find_all_by_role(Role::TabList).len());
        assert_eq!(tabs, 0);
        // A thousand rows exist; only the ones that fit are drawn.
        let drawn = visual.update(|window, _| window.find_all_by_role(Role::Row).len());
        assert!(drawn > 0 && drawn < 100, "{drawn} rows drawn");
    }

    #[gpui_kit::test]
    fn an_empty_sheet_and_a_failed_read_say_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let empty: SheetSource = Arc::new(|_, _| {
            let read = sheet_of(&["Sheet1"], 0, columns(&[]), Vec::new(), &[]);
            Box::pin(async move { Ok(read) })
        });
        let (view, mut visual) = open(cx, "C:\\data\\blank.xlsx", empty);
        ready(&view, &mut visual, 0);
        assert!(has(&mut visual, "This file is empty."));
        let failing: SheetSource =
            Arc::new(|_, _| Box::pin(async { Err("The workbook is password-protected".to_string()) }));
        let (view, mut visual) = open(cx, "C:\\data\\locked.xlsx", failing);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Failed(_)))
        });
        assert!(has(&mut visual, "The workbook is password-protected"));
    }

    #[test]
    fn every_icon_the_sheet_view_draws_is_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Table2,
            IconName::Hash,
            IconName::ToggleLeft,
            IconName::Type,
            IconName::Search,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
