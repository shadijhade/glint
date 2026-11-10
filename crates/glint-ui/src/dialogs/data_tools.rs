//! Data tools: SQL over CSV, JSON and Parquet files (alone or several together), a profile of every column, and
//! converting data files. Ported from src/components/explorer/DataTools.tsx and QueryView.tsx, built the way
//! [`super::drive_health`] is.
//!
//! | TypeScript                            | Here                                                              |
//! |---------------------------------------|-------------------------------------------------------------------|
//! | `DUCK_READS`, `isDataFile`            | [`DUCK_READS`], [`is_data_file`] (by name; the menus know the rest) |
//! | `queryTogether(entries)`              | [`query_together`]                                                |
//! | `TogetherModal`                       | [`TogetherDialog`]                                                |
//! | `QueryView` (`sqlName`, `firstRows`)  | [`QueryView`], [`sql_name`], [`first_rows`]                       |
//! | `convertData(entries)`                | [`convert_data`]                                                  |
//! | `ConvertModal`                        | [`DataConvertDialog`]                                             |
//! | `ProfileView`                         | [`ProfileView`]                                                   |
//! | `sqlTables`, `sqlRun`, `duck_tables`, `duck_run`, `duck_profile`, `duck_convert`, `duck_cancel` | [`Backend`] ([`CoreBackend`] is glint-core) |
//! | `useActionRun`, `WhereChooser`, `Choices`, `RunProgress`, `RunResult`, `RunFooter` | [`action_run`]       |
//! | `ToolNeeded id="duckdb"`              | `settings::tools::needed`, through [`action_run::Tooling`]        |
//!
//! **How the coordinator wires it.**
//! * `MenuEvent::QueryTogether { paths }` is `dialogs::data_tools::query_together(window, cx, paths)`. It wants two
//!   or more data files and says so in a notification otherwise.
//! * `MenuEvent::ConvertData { paths }` is `dialogs::data_tools::convert_data(window, cx, paths)`.
//! * The preview pane's Query and Profile views (built in components/preview/frame.rs) are
//!   [`QueryView::new`] over `vec![path]` (with the database's draft, if the schema view asked for one) and
//!   [`ProfileView::new`]. Both take a [`Backend`] and a [`Tooling`]; `data_tools::core_backend(cx)` and
//!   `Tooling::new(action_run::tools_model(window, cx))` are the app's.
//!
//! The answer's table has the columns and sorting of the preview's table, but no filter box: a SQL `WHERE` is the
//! filter here. A query runs on Ctrl+Enter or the Run button; a newer run's answer replaces an older one's.

use super::action_run;

use crate::components::preview::data::shown_rows;
use crate::kit::prelude::*;
use crate::kit::{Textarea, TextareaState};
use crate::{core_bridge, t, theme};
use action_run::{
    choices, error_view, footer, on_core, progress_text, progress_view, result_view, section_title, shell, whole,
    ActionRun, Choice, FooterSpec, Fut, ResultWords, Run, Tooling, WhereChooser,
};
use glint_core::actions::{Output, Progress, Report};
use glint_core::data_lens::TablePreview;
use glint_core::duck::{ColumnProfile, DataFormat, Profile};
use glint_model::format::{format_count, format_duration, plural};
use glint_model::js::to_fixed;
use glint_model::paths::base_name;
use std::path::Path;
use std::sync::Arc;

/// What DuckDB reads itself (duck.rs's `READS`).
pub const DUCK_READS: &[&str] = glint_core::duck::READS;

/// Why DuckDB is needed (`DUCK_WHY`).
fn duck_why() -> SharedString {
    t!("DuckDB reads data files of any size where they are, in seconds, and describes every column. Nothing is uploaded.")
}

/// Whether the file called `path` is a data file DuckDB reads, by its extension (`isDataFile` minus the entry's
/// state: a file on disk, there to be read, is for the menus to know). Plain `.txt` files are not.
pub fn is_data_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .is_some_and(|ext| ext != "txt" && DUCK_READS.contains(&ext.as_str()))
}

/// A table's name as SQL takes it: as it is when it's a plain word, else in double quotes (`sqlName`).
pub fn sql_name(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// The query a table starts with (`firstRows`).
pub fn first_rows(table: &str) -> String {
    format!("SELECT * FROM {} LIMIT 100", sql_name(table))
}

// ---- The calls --------------------------------------------------------------------------------------------

/// The tables a query can read (`SqlTables`).
#[derive(Clone, Debug, PartialEq)]
pub struct Tables {
    pub tables: Vec<String>,
    /// Who answers: "DuckDB" or "SQLite".
    pub engine: String,
    /// Loading the file took this long (0 when it was loaded already).
    pub load_ms: f64,
}

/// A query's answer (`SqlAnswer`): the first 1,000 rows, as the table view shows them.
#[derive(Clone, Debug)]
pub struct Answer {
    pub engine: String,
    pub table: TablePreview,
    pub load_ms: f64,
}

/// The calls the data tools make. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// The tables there are to query in `paths`: DuckDB's for several files (`duck_tables`), else `sql_tables`.
    fn tables(&self, paths: Vec<String>) -> Fut<Tables>;
    /// Runs one query over `paths` (`duck_run` for several files, else `sql_run`).
    fn query(&self, paths: Vec<String>, sql: String) -> Fut<Answer>;
    /// Describes every column of a data file (`duck_profile`).
    fn profile(&self, path: String) -> Fut<Profile>;
    /// Writes each of `files` as `format` (`duck_convert`), reporting progress on `progress`.
    fn convert(
        &self,
        files: Vec<String>,
        format: DataFormat,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the conversion under way (`duck_cancel`).
    fn cancel(&self);
}

/// glint-core's data tools, owned by the main window as the shell dialogs it opens are.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Backend for CoreBackend {
    fn tables(&self, paths: Vec<String>) -> Fut<Tables> {
        let asked = match paths.as_slice() {
            [only] => on_core(glint_core::sql::sql_tables(only.clone())),
            _ => on_core(glint_core::duck::duck_tables(paths)),
        };
        Box::pin(async move {
            asked.await.map(|found| Tables {
                tables: found.tables,
                engine: found.engine.to_string(),
                load_ms: found.load_ms,
            })
        })
    }

    fn query(&self, paths: Vec<String>, sql: String) -> Fut<Answer> {
        let asked = match paths.as_slice() {
            [only] => on_core(glint_core::sql::sql_run(only.clone(), sql)),
            _ => on_core(glint_core::duck::duck_run(paths, sql)),
        };
        Box::pin(async move {
            asked.await.map(|found| Answer {
                engine: found.engine.to_string(),
                table: TablePreview {
                    format: "SQL".into(),
                    delimiter: String::new(),
                    total_rows_sampled: found.rows.len(),
                    columns: found.columns,
                    rows: found.rows,
                    truncated: found.truncated,
                    elapsed_ms: found.elapsed_ms,
                },
                load_ms: found.load_ms,
            })
        })
    }

    fn profile(&self, path: String) -> Fut<Profile> {
        on_core(glint_core::duck::duck_profile(path))
    }

    fn convert(
        &self,
        files: Vec<String>,
        format: DataFormat,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        match action_run::main_window(&self.app) {
            Ok(window) => on_core(glint_core::duck::duck_convert(window, files, format, output, progress)),
            Err(reason) => Box::pin(async move { Err(reason) }),
        }
    }

    fn cancel(&self) {
        glint_core::duck::duck_cancel();
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn tables(&self, _: Vec<String>) -> Fut<Tables> {
        action_run::no_core()
    }

    fn query(&self, _: Vec<String>, _: String) -> Fut<Answer> {
        action_run::no_core()
    }

    fn profile(&self, _: String) -> Fut<Profile> {
        action_run::no_core()
    }

    fn convert(
        &self,
        _: Vec<String>,
        _: DataFormat,
        _: Output,
        _: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        action_run::no_core()
    }

    fn cancel(&self) {}
}

/// The app's backend: core's if it is started.
pub fn core_backend(cx: &App) -> Arc<dyn Backend> {
    match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    }
}

// ---- Small pieces -----------------------------------------------------------------------------------------

/// A spinner and what is being read, in the middle (`Waiting`).
fn waiting(id: &'static str, what: SharedString, t: &theme::Tokens) -> AnyElement {
    col()
        .id(id)
        .role(Role::Status)
        .aria_label(what.clone())
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(3.))
        .p(space(6.))
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        .child(Spinner::new().color(t.primary))
        .child(div().text_center().child(what))
        .into_any_element()
}

/// A message in the middle, `tone` colored (`BigIcon`'s note).
fn note(id: &'static str, text: SharedString, color: Hsla, role: Role) -> AnyElement {
    col()
        .id(id)
        .role(role)
        .aria_label(text.clone())
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(3.))
        .p(space(6.))
        .text_center()
        .text_size(theme::size::SM)
        .text_color(color)
        .child(div().max_w(space(96.)).child(text))
        .into_any_element()
}

// ---- The answer's table -----------------------------------------------------------------------------------

/// The answer's columns and rows for the kit's `DataTable`, sortable by a click on a column.
struct AnswerGrid {
    table: Arc<TablePreview>,
    /// The rows shown, as indices into the table's.
    order: Vec<usize>,
    widths: Vec<Pixels>,
}

impl AnswerGrid {
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
        Self { table, order, widths }
    }

    fn cell(&self, row: usize, column: usize) -> &str {
        self.order
            .get(row)
            .and_then(|&i| self.table.rows.get(i))
            .and_then(|cells| cells.get(column))
            .map_or("", String::as_str)
    }
}

impl TableDelegate for AnswerGrid {
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
        self.order = shown_rows(&self.table, "", sort);
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

// ---- The query view ---------------------------------------------------------------------------------------

/// Where the file's tables stand.
enum Load {
    Waiting,
    Failed(SharedString),
    Ready(Tables),
}

/// What the last query gave.
enum Answered {
    /// Nothing asked yet.
    None,
    Failed(SharedString),
    Done {
        answer: Arc<TablePreview>,
        grid: Entity<TableState<AnswerGrid>>,
    },
}

/// SQL over a table file or a database, reading only (`QueryView`): a data file is the table `data`, several files
/// are tables named after them. Ctrl+Enter runs; the first 1,000 rows of the answer show in a table.
pub struct QueryView {
    paths: Vec<String>,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    draft: Option<String>,
    load: Load,
    sql: Entity<TextareaState>,
    answer: Answered,
    running: bool,
    /// Numbers the runs, so an answer that arrives after a newer run was asked for is dropped (`runs`).
    generation: u64,
    _subscription: Subscription,
}

impl QueryView {
    /// The query view over `paths` (several: DuckDB's, a table each), starting with `draft` when there is one,
    /// else the first table's first rows. Loading and the first run happen in the background.
    pub fn new(
        paths: Vec<String>,
        draft: Option<String>,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sql = cx.new(|cx| TextareaState::new(window, cx).rows(3).placeholder("SELECT * FROM data"));
        let subscription = cx.subscribe_in(&sql, window, |this, _, event: &InputEvent, window, cx| match event {
            // Ctrl+Enter runs the query.
            InputEvent::PressEnter { secondary: true, .. } => this.run_current(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let mut this = Self {
            paths,
            backend,
            tooling,
            draft,
            load: Load::Waiting,
            sql,
            answer: Answered::None,
            running: false,
            generation: 0,
            _subscription: subscription,
        };
        this.ask_tables(window, cx);
        this
    }

    /// The SQL box, to type in and focus.
    pub fn sql_input(&self) -> &Entity<TextareaState> {
        &self.sql
    }

    /// The SQL now in the box.
    pub fn sql(&self, cx: &App) -> String {
        self.sql.read(cx).value().to_string()
    }

    /// The rows of the last answer, as text (empty before the first).
    pub fn rows(&self, cx: &App) -> Vec<Vec<String>> {
        match &self.answer {
            Answered::Done { answer, grid } => {
                let delegate = grid.read(cx).delegate();
                delegate.order.iter().map(|&i| answer.rows[i].clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Whether a query is running.
    pub fn running(&self) -> bool {
        self.running
    }

    /// The error of the last query, if it failed.
    pub fn error(&self) -> Option<SharedString> {
        match &self.answer {
            Answered::Failed(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    /// The tables of the file(s), once known.
    pub fn tables(&self) -> Option<&Tables> {
        match &self.load {
            Load::Ready(tables) => Some(tables),
            _ => None,
        }
    }

    /// The file is loaded (a database opened), then the first table's rows (or the query asked) run.
    fn ask_tables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.tables(self.paths.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = asked.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(found) => {
                        let first = this
                            .draft
                            .clone()
                            .or_else(|| found.tables.first().map(|table| first_rows(table)))
                            .unwrap_or_default();
                        this.load = Load::Ready(found);
                        this.set_sql(&first, window, cx);
                        if !first.is_empty() {
                            this.run(first, window, cx);
                        }
                    }
                    Err(reason) => this.load = Load::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_sql(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sql.update(cx, |sql, cx| sql.set_value(text, window, cx));
    }

    /// Runs what is in the box.
    pub fn run_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.sql(cx);
        self.run(text, window, cx);
    }

    /// Runs `text` over the file(s); the answer replaces the last one unless a newer run was asked for first.
    pub fn run(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let mine = self.generation;
        self.running = true;
        cx.notify();
        let asked = self.backend.query(self.paths.clone(), text);
        cx.spawn_in(window, async move |this, cx| {
            let result = asked.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != mine {
                    return;
                }
                this.running = false;
                this.answer = match result {
                    Err(reason) => Answered::Failed(reason.into()),
                    Ok(answer) => {
                        let table = Arc::new(answer.table);
                        let rem = theme::rem_size(cx);
                        let for_grid = table.clone();
                        let grid = cx.new(|cx| {
                            TableState::new(AnswerGrid::new(for_grid, rem), window, cx)
                                .col_movable(false)
                                .row_selectable(true)
                        });
                        // The engine and the load time come with the tables the first time, and with each answer.
                        if let Load::Ready(tables) = &mut this.load {
                            tables.engine = answer.engine;
                            if answer.load_ms >= 1. {
                                tables.load_ms = answer.load_ms;
                            }
                        }
                        Answered::Done { answer: table, grid }
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// The box, the Run button, the tables and the engine.
    fn header(&self, tables: &Tables, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = theme::mono_font(cx);
        let engine_hint = if tables.engine == "DuckDB" {
            t!("DuckDB reads the file where it is, however big")
        } else {
            t!("Loaded into SQLite in memory")
        };
        col()
            .flex_none()
            .gap(space(1.5))
            .p(space(2.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .child(Textarea::new(&self.sql).aria_label(t!("SQL"))),
            )
            .child(
                align_row(1.5)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(
                        Button::new("query-run")
                            .primary()
                            .small()
                            .icon(IconName::Play)
                            .label(t!("Run"))
                            .loading(self.running)
                            .tooltip(t!("Run it (Ctrl+Enter)"))
                            .on_click(cx.listener(|this, _, window, cx| this.run_current(window, cx))),
                    )
                    .child(
                        align_row(0.5)
                            .id("query-tables")
                            .role(Role::List)
                            .aria_label(t!("Tables"))
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .children(tables.tables.iter().enumerate().map(|(ix, table)| {
                                let table = table.clone();
                                let rows = first_rows(&table);
                                Button::new(("query-table", ix))
                                    .ghost()
                                    .compact()
                                    .font_family(mono.clone())
                                    .text_size(theme::size::XXS)
                                    .label(table.clone())
                                    .tooltip(t!("Its first rows: {sql}", sql = rows.clone()))
                                    .accessibility_label(table)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_sql(&rows, window, cx);
                                        this.run(rows.clone(), window, cx);
                                    }))
                            })),
                    )
                    .when(tables.load_ms >= 1., |this| {
                        this.child(div().flex_none().child(t!(
                            "loaded in {ms} ms",
                            ms = format!("{}", tables.load_ms.round() as i64)
                        )))
                    })
                    .child(
                        div()
                            .id("query-engine")
                            .role(Role::Group)
                            .aria_label(format!("{}. {}", tables.engine, engine_hint))
                            .test_support()
                            .flex_none()
                            .tooltip({
                                let hint = engine_hint.clone();
                                move |window, cx| crate::kit::tooltip(hint.clone(), window, cx)
                            })
                            .child(tables.engine.clone()),
                    ),
            )
    }

    /// The answer: the format badge, how many columns and rows, how long it took, the grid and a note when there
    /// were more rows.
    fn answer_view(
        &self,
        table: &TablePreview,
        grid: &Entity<TableState<AnswerGrid>>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = grid.read(cx).delegate().order.len();
        let _ = shown;
        let counts = t!(
            "{cols} cols · {total}{plus} rows",
            cols = format_count(table.columns.len()),
            total = format_count(table.total_rows_sampled),
            plus = if table.truncated { "+" } else { "" }
        );
        let green = theme::tailwind(if t.dark { "emerald-400" } else { "emerald-600" }).unwrap_or(t.primary);
        col()
            .size_full()
            .min_h_0()
            .child(
                row_start()
                    .flex_none()
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
                            .gap(space(1.))
                            .px(space(2.))
                            .py(space(0.5))
                            .rounded(theme::size::RADIUS)
                            .bg(green.opacity(0.15))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(green)
                            .child(icon(IconName::Table2).size_3())
                            .child("SQL"),
                    )
                    .child(
                        div()
                            .id("query-counts")
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
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(grid).bordered(false).accessibility_label(t!("Answer"))),
            )
            .when(table.truncated, |this| {
                let more = t!("The first 1,000 rows of the answer: LIMIT and OFFSET show others.");
                this.child(
                    div()
                        .id("query-more")
                        .role(Role::Status)
                        .aria_label(more.clone())
                        .test_support()
                        .flex_none()
                        .px(space(3.))
                        .py(space(2.))
                        .border_t_1()
                        .border_color(t.border)
                        .bg(t.muted.opacity(0.1))
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(more),
                )
            })
            .into_any_element()
    }
}

impl Render for QueryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col().id("query").size_full().min_h_0().text_start();
        let tables = match &self.load {
            Load::Waiting => return root.child(waiting("query-loading", t!("Loading it for SQL…"), &t)),
            Load::Failed(reason) => {
                // A file too big for SQLite's memory is DuckDB's to read: the card offers it.
                let card = if reason.contains("too big to load") {
                    self.tooling.card(
                        "duckdb",
                        t!("DuckDB queries data files of any size where they are, without loading them. Install it and SQL works on this one."),
                        IconName::Database,
                        cx,
                    )
                } else {
                    None
                };
                return root
                    .gap(space(3.))
                    .p(space(4.))
                    .child(note("query-failed", reason.clone(), t.muted_foreground, Role::Alert))
                    .children(card);
            }
            Load::Ready(tables) => tables.clone(),
        };
        let body = match &self.answer {
            Answered::Failed(reason) => div()
                .id("query-error")
                .role(Role::Alert)
                .aria_label(reason.clone())
                .test_support()
                .m(space(2.))
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(t.destructive.opacity(0.3))
                .bg(t.destructive.opacity(0.05))
                .px(space(3.))
                .py(space(2.))
                .font_family(theme::mono_font(cx))
                .text_size(theme::size::XS)
                .text_color(t.destructive)
                .child(reason.clone())
                .into_any_element(),
            Answered::Done { answer, grid } => div()
                .flex_1()
                .min_h_0()
                .child(self.answer_view(&answer.clone(), &grid.clone(), &t, cx))
                .into_any_element(),
            Answered::None if self.running => waiting("query-running", t!("Loading…"), &t),
            Answered::None => div().into_any_element(),
        };
        root.child(self.header(&tables, &t, cx)).child(body)
    }
}

// ---- Query together ---------------------------------------------------------------------------------------

/// "Query together": the SQL view over several data files, each a table named after its file.
pub struct TogetherDialog {
    files: Vec<String>,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    query: Option<Entity<QueryView>>,
    _subscriptions: Vec<Subscription>,
}

impl TogetherDialog {
    /// A dialog for `files`; the query view starts once DuckDB is known to be there.
    pub fn new(
        files: Vec<String>,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(tools) = tooling.model() {
            subscriptions.push(cx.observe_in(tools, window, |this, _, window, cx| {
                this.start_query(window, cx);
                cx.notify();
            }));
        }
        let mut this = Self {
            files,
            backend,
            tooling,
            query: None,
            _subscriptions: subscriptions,
        };
        this.start_query(window, cx);
        this
    }

    /// Makes the query view the first time DuckDB is there.
    fn start_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query.is_some() || !self.tooling.ready("duckdb", cx) {
            return;
        }
        let (paths, backend, tooling) = (self.files.clone(), self.backend.clone(), Tooling::new(None));
        let query = cx.new(|cx| QueryView::new(paths, None, backend, tooling, window, cx));
        self._subscriptions.push(cx.observe(&query, |_, _, cx| cx.notify()));
        self.query = Some(query);
    }

    /// The files queried.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The query view, once DuckDB is there.
    pub fn query(&self) -> Option<&Entity<QueryView>> {
        self.query.as_ref()
    }

    /// The dialog's title: "3 files, queried together".
    pub fn title(&self) -> SharedString {
        t!("{n} files, queried together", n = format_count(self.files.len()))
    }
}

impl Render for TogetherDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let names: Vec<String> = self.files.iter().map(|f| base_name(f).to_string()).collect();
        let subtitle: SharedString = names.join(", ").into();
        let body = match (
            &self.query,
            self.tooling.card("duckdb", duck_why(), IconName::Database, cx),
        ) {
            (Some(query), _) => query.clone().into_any_element(),
            (None, Some(card)) => div().p(space(4.)).child(card).into_any_element(),
            (None, None) => waiting("together-waiting", t!("Loading…"), &t),
        };
        // The TypeScript's `h-[85vh]`, less the title and folder lines.
        let height = (window.viewport_size().height * 0.85 - px(110.)).max(px(240.));
        col()
            .id("together")
            .role(Role::Group)
            .aria_label(t!("Query together"))
            .test_support()
            .gap(space(2.))
            .text_size(theme::size::SM)
            .text_start()
            .child(
                div()
                    .id("together-files")
                    .role(Role::Group)
                    .aria_label(subtitle.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .truncate()
                    .child(isolate(subtitle)),
            )
            .child(div().h(height).min_h_0().child(body))
    }
}

/// Opens Query together on the data files among `paths` in `window` (`queryTogether`): each a table named after
/// its file. Two or more are needed; otherwise it says so in a notification.
pub fn query_together(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_data_file(p)).collect();
    if files.len() < 2 {
        window.push_notification(
            Notification::info(t!("Choose two or more CSV, JSON or Parquet files")),
            cx,
        );
        return;
    }
    let backend = core_backend(cx);
    let tooling = Tooling::new(action_run::tools_model(window, cx));
    open_together_with(window, cx, backend, tooling, files);
}

/// [`query_together`] for a given backend and tools, without filtering `files` (tests pass fakes). Returns the dialog.
pub fn open_together_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    files: Vec<String>,
) -> Entity<TogetherDialog> {
    let view = cx.new(|cx| TogetherDialog::new(files, backend, tooling, window, cx));
    let title = view.read(cx).title();
    super::show(window, cx, title, px(1024.), view.clone());
    view
}

// ---- Convert data -----------------------------------------------------------------------------------------

/// The formats offered: the format, its name and what it is good for.
fn formats() -> Vec<Choice<DataFormat>> {
    vec![
        Choice::new(DataFormat::Parquet, "Parquet").hint(t!("Columns, Zstandard: far smaller and faster")),
        Choice::new(DataFormat::Csv, "CSV").hint(t!("Opens in Excel and everything")),
        Choice::new(DataFormat::Jsonl, "JSON lines").hint(t!("An object a line, for code")),
    ]
}

/// The format to start with: CSV for Parquet files, Parquet for everything else (`ConvertModal`'s first state).
pub fn first_format(files: &[String]) -> DataFormat {
    let parquet = |f: &String| {
        Path::new(f)
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("parquet"))
    };
    if files.iter().all(parquet) {
        DataFormat::Csv
    } else {
        DataFormat::Parquet
    }
}

/// "name" or "3 data files": what the dialog is called after.
pub fn what(files: &[String]) -> SharedString {
    match files {
        [one] => isolate(base_name(one).to_string()),
        _ => t!("{n} data files", n = format_count(files.len())),
    }
}

/// Converting some data files: the format, where the results go, the run and its result.
pub struct DataConvertDialog {
    files: Vec<String>,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    format: DataFormat,
    where_: Entity<WhereChooser>,
    run: Entity<ActionRun>,
    _subscriptions: Vec<Subscription>,
}

impl DataConvertDialog {
    /// A dialog for `files`, calling `backend`, its install card reading `tooling`.
    pub fn new(
        files: Vec<String>,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let where_ = cx.new(|cx| {
            WhereChooser::new(
                "data-convert-where",
                t!("file"),
                t!("A name that's taken gets “(2)”, and Ctrl+Z takes it all back."),
                true,
                ("", "Converted"),
                window,
                cx,
            )
        });
        let run = cx.new(|_| ActionRun::new());
        let mut subscriptions = vec![
            cx.observe(&where_, |_, _, cx| cx.notify()),
            cx.observe(&run, |_, _, cx| cx.notify()),
        ];
        subscriptions.extend(action_run::watch_tools(&tooling, cx));
        Self {
            format: first_format(&files),
            files,
            backend,
            tooling,
            where_,
            run,
            _subscriptions: subscriptions,
        }
    }

    /// The files being converted.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The format chosen.
    pub fn format(&self) -> DataFormat {
        self.format
    }

    /// Where the run stands.
    pub fn run_state(&self, cx: &App) -> Run {
        self.run.read(cx).state().clone()
    }

    /// The run, to put in a state by hand in tests.
    pub fn run(&self) -> &Entity<ActionRun> {
        &self.run
    }

    /// Where the results go.
    pub fn where_chooser(&self) -> &Entity<WhereChooser> {
        &self.where_
    }

    /// Whether Convert can be pressed: DuckDB is there and the names are usable.
    pub fn can_start(&self, cx: &App) -> bool {
        !self.run.read(cx).state().started()
            && self.tooling.ready("duckdb", cx)
            && !self.where_.read(cx).choice(cx).bad()
    }

    /// Starts the conversion on core's runtime; progress and the result come back here.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let output = self.where_.read(cx).choice(cx).output();
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.convert(self.files.clone(), self.format, output, channel);
        let total = self.files.len();
        self.run.update(cx, |run, cx| {
            run.start(
                total,
                call,
                receiver,
                |report| t!("conversion of {what}", what = plural(report.made.len(), "data file")).to_string(),
                cx,
            )
        });
        cx.notify();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }
}

impl Render for DataConvertDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.run.read(cx).state().clone();

        let mut parts: Vec<AnyElement> = Vec::new();
        parts.extend(self.tooling.card("duckdb", duck_why(), IconName::Database, cx));
        match state.clone() {
            Run::Setup => {
                parts.push(
                    col()
                        .gap(space(1.5))
                        .child(section_title(t!("Into"), &t))
                        .child(choices::<Self, DataFormat>(
                            "data-convert-format",
                            t!("Format"),
                            self.format,
                            formats(),
                            3,
                            |this, picked, _, _| this.format = picked,
                            &t,
                            cx,
                        ))
                        .into_any_element(),
                );
                parts.push(self.where_.clone().into_any_element());
            }
            Run::Running(progress) => parts.push(
                progress_view(
                    "data-convert-progress",
                    progress_text(&t!("Converting"), &progress, false, false),
                    whole(&progress),
                    &t,
                )
                .into_any_element(),
            ),
            Run::Done(report) => parts
                .push(result_view("data-convert-result", &report, &ResultWords::files(true), &t).into_any_element()),
            Run::Failed(reason) => {
                parts.push(error_view("data-convert-error", t!("Couldn't convert"), reason, &t).into_any_element())
            }
        }
        let body = col().gap(space(4.)).children(parts);

        let spec = FooterSpec {
            prefix: "data-convert",
            start_label: t!("Convert"),
            start_icon: IconName::FileCog,
            can_start: self.can_start(cx),
            stoppable: true,
            show: state.first_made(),
        };
        let footer = footer::<Self>(
            spec,
            &state,
            |this, _, cx| this.start(cx),
            |this, _, _| this.stop(),
            &t,
            cx,
        );
        shell(
            "data-convert",
            t!("Convert data: {what}", what = what(&self.files)),
            action_run::where_they_are(&self.files),
            body,
            footer,
            window,
            &t,
        )
    }
}

/// Opens Convert data on the data files among `paths` in `window` (`convertData`). When none is left it says so
/// in a notification instead.
pub fn convert_data(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_data_file(p)).collect();
    if files.is_empty() {
        window.push_notification(Notification::info(t!("No CSV, JSON or Parquet files chosen")), cx);
        return;
    }
    let backend = core_backend(cx);
    let tooling = Tooling::new(action_run::tools_model(window, cx));
    open_convert_with(window, cx, backend, tooling, files);
}

/// [`convert_data`] for a given backend and tools, without filtering `files` (tests pass fakes). Returns the dialog.
pub fn open_convert_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    files: Vec<String>,
) -> Entity<DataConvertDialog> {
    let shown = t!("Convert data: {what}", what = what(&files));
    let view = cx.new(|cx| DataConvertDialog::new(files, backend, tooling, window, cx));
    super::show(window, cx, shown, px(576.), view.clone());
    view
}

// ---- Profile ----------------------------------------------------------------------------------------------

/// The profile's table: each column of a data file, described.
struct ProfileGrid {
    profile: Arc<Profile>,
}

impl ProfileGrid {
    /// The text of a cell, for screen readers and tests.
    fn text(&self, row: usize, ix: usize) -> String {
        let Some(c) = self.profile.columns.get(row) else {
            return String::new();
        };
        match ix {
            0 => c.name.clone(),
            1 => c.duck_type.clone(),
            2 => missing_text(c.null_percent),
            3 => format_count(usize::try_from(c.distinct).unwrap_or(usize::MAX)),
            4 => c.min.clone(),
            5 => c.max.clone(),
            6 => c.mean.clone(),
            _ if c.q50.is_empty() => String::new(),
            _ => format!("{} · {} · {}", c.q25, c.q50, c.q75),
        }
    }
}

/// What a header of the profile's table explains when hovered (the TypeScript's `title`s), if it has one.
fn profile_header_tip(ix: usize) -> Option<SharedString> {
    match ix {
        2 => Some(t!("Rows without a value")),
        3 => Some(t!("About how many different values")),
        7 => Some(t!("Lower quartile, median, upper quartile")),
        _ => None,
    }
}

/// The tooltip of a cell of the profile's table: a cell that can be cut short shows all of it, and the mean
/// says its standard deviation.
fn profile_cell_tip(column: &ColumnProfile, ix: usize) -> Option<SharedString> {
    let all = match ix {
        0 => column.name.clone(),
        1 => column.duck_type.clone(),
        4 => column.min.clone(),
        5 => column.max.clone(),
        6 if !column.std.is_empty() => return Some(t!("Standard deviation {std}", std = column.std.clone())),
        _ => String::new(),
    };
    (!all.is_empty()).then(|| all.into())
}

/// "12%", or "0.50%" for under one percent (`toFixed(c.nullPercent && c.nullPercent < 1 ? 2 : 0)`).
pub fn missing_text(percent: f64) -> String {
    format!(
        "{}%",
        to_fixed(percent, if percent != 0. && percent < 1. { 2 } else { 0 })
    )
}

impl TableDelegate for ProfileGrid {
    fn columns_count(&self, _: &App) -> usize {
        8
    }

    fn rows_count(&self, _: &App) -> usize {
        self.profile.columns.len()
    }

    fn column(&self, ix: usize, _: &App) -> Column {
        let (name, width): (SharedString, f32) = match ix {
            0 => (t!("Column"), 176.),
            1 => (t!("Type"), 112.),
            2 => (t!("Missing"), 128.),
            3 => (t!("Distinct"), 96.),
            4 => (t!("Smallest"), 128.),
            5 => (t!("Largest"), 128.),
            6 => (t!("Mean"), 96.),
            _ => (t!("Quartiles"), 208.),
        };
        Column::new(format!("profile-{ix}"), name)
            .width(px(width))
            .movable(false)
            .resizable(true)
            .when(ix == 3 || ix == 6, |c| c.text_right())
    }

    fn perform_sort(
        &mut self,
        _: usize,
        _: gpui_kit::component::table::ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
    }

    fn render_th(&mut self, ix: usize, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let name = self.column(ix, cx).name.clone();
        match profile_header_tip(ix) {
            Some(tip) => div()
                .id(("profile-th", ix))
                .w_full()
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                .child(name)
                .into_any_element(),
            None => div().w_full().child(name).into_any_element(),
        }
    }

    fn render_td(
        &mut self,
        row: usize,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let text = self.text(row, ix);
        // Hovering a cell shows all of what the cell cuts short.
        let tip = self.profile.columns.get(row).and_then(|c| profile_cell_tip(c, ix));
        let cell = div().id(("profile-td", row * 8 + ix)).w_full();
        let cell = match tip {
            Some(tip) => cell.tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx)),
            None => cell,
        };
        match ix {
            0 => cell
                .w_full()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(isolate(text))
                .into_any_element(),
            1 => cell
                .w_full()
                .truncate()
                .font_family(theme::mono_font(cx))
                .text_color(t.muted_foreground)
                .child(text)
                .into_any_element(),
            2 => {
                let percent = self.profile.columns.get(row).map_or(0., |c| c.null_percent);
                align_row(1.5)
                    .w_full()
                    .child(
                        row_start()
                            .h(space(1.5))
                            .w(space(12.))
                            .flex_none()
                            .overflow_hidden()
                            .rounded_full()
                            .bg(t.muted)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative((percent / 100.).clamp(0., 1.) as f32))
                                    .rounded_full()
                                    .bg(if percent > 20. { t.warning } else { t.primary }),
                            ),
                    )
                    .child(div().child(text))
                    .into_any_element()
            }
            3 | 6 => cell.truncate().text_end().child(text).into_any_element(),
            _ => cell.truncate().child(isolate(text)).into_any_element(),
        }
    }

    fn cell_label(&self, row: usize, ix: usize, _: &App) -> Option<SharedString> {
        Some(self.text(row, ix).into())
    }

    fn render_empty(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let t = theme::tokens(cx);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.muted_foreground)
            .child(t!("Nothing to profile."))
    }
}

/// What the profile shows.
enum Profiled {
    Waiting,
    Failed(SharedString),
    Ready {
        profile: Arc<Profile>,
        grid: Entity<TableState<ProfileGrid>>,
    },
}

/// Each column of a data file described from all of its rows (`ProfileView`, DuckDB's `SUMMARIZE`): the preview
/// pane's Profile view.
pub struct ProfileView {
    path: String,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    profiled: Profiled,
    asked: bool,
    generation: u64,
    _subscription: Option<Subscription>,
}

impl ProfileView {
    /// The profile of the data file at `path`; it is read in the background once DuckDB is there.
    pub fn new(
        path: String,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = tooling.model().map(|tools| {
            cx.observe_in(tools, window, |this, _, window, cx| {
                this.read(window, cx);
                cx.notify();
            })
        });
        let mut this = Self {
            path,
            backend,
            tooling,
            profiled: Profiled::Waiting,
            asked: false,
            generation: 0,
            _subscription: subscription,
        };
        this.read(window, cx);
        this
    }

    /// The file profiled.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file's profile.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.asked = false;
            self.profiled = Profiled::Waiting;
            self.read(window, cx);
            cx.notify();
        }
    }

    /// The cells of the table, as text, a row per column of the file.
    pub fn cells(&self, cx: &App) -> Vec<Vec<String>> {
        match &self.profiled {
            Profiled::Ready { grid, .. } => {
                let delegate = grid.read(cx).delegate();
                (0..delegate.profile.columns.len())
                    .map(|row| (0..8).map(|ix| delegate.text(row, ix)).collect())
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Reads the profile once DuckDB is there (`useEffect([ready, path, modified])`).
    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asked || !self.tooling.ready("duckdb", cx) {
            return;
        }
        self.asked = true;
        self.generation += 1;
        let mine = self.generation;
        let asked = self.backend.profile(self.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = asked.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != mine {
                    return;
                }
                this.profiled = match result {
                    Err(reason) => Profiled::Failed(reason.into()),
                    Ok(profile) => {
                        let profile = Arc::new(profile);
                        let for_grid = profile.clone();
                        let grid = cx.new(|cx| {
                            TableState::new(ProfileGrid { profile: for_grid }, window, cx).col_movable(false)
                        });
                        Profiled::Ready { profile, grid }
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ProfileView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col().id("profile").size_full().min_h_0().text_start();
        // DuckDB isn't there: the card offers it.
        if let Some(card) = self.tooling.card("duckdb", duck_why(), IconName::Database, cx) {
            return root.p(space(4.)).child(card);
        }
        match &self.profiled {
            Profiled::Waiting => root.child(waiting("profile-waiting", t!("Reading every row…"), &t)),
            Profiled::Failed(reason) => {
                root.child(note("profile-failed", reason.clone(), t.muted_foreground, Role::Alert))
            }
            Profiled::Ready { profile, grid } => {
                let summary = t!(
                    "{rows} · {columns} · read by DuckDB in {time}",
                    rows = plural(usize::try_from(profile.rows).unwrap_or(usize::MAX), "row"),
                    columns = plural(profile.columns.len(), "column"),
                    time = format_duration(profile.elapsed_ms)
                );
                root.child(
                    div()
                        .id("profile-summary")
                        .role(Role::Status)
                        .aria_label(summary.clone())
                        .test_support()
                        .flex_none()
                        .px(space(3.))
                        .py(space(2.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(summary),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(DataTable::new(grid).bordered(false).accessibility_label(t!("Columns"))),
                )
            }
        }
    }
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::data_lens::TableColumn;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that answers from memory and remembers what it was asked.
    struct Fake {
        queries: Mutex<Vec<(Vec<String>, String)>>,
        converts: Mutex<Vec<(Vec<String>, DataFormat, Output)>>,
        tables: Result<Tables, String>,
        profile: Option<Profile>,
        report: Report,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake {
                queries: Mutex::new(Vec::new()),
                converts: Mutex::new(Vec::new()),
                tables: Ok(Tables {
                    tables: vec!["sales".into(), "my table".into()],
                    engine: "DuckDB".into(),
                    load_ms: 0.,
                }),
                profile: None,
                report: Report {
                    made: vec!["C:\\d\\sales.parquet".into()],
                    before: 4_000_000,
                    after: 1_000_000,
                    elapsed_ms: 2000.,
                    ..Default::default()
                },
                cancelled: Mutex::new(0),
            })
        }

        fn queries(&self) -> Vec<(Vec<String>, String)> {
            self.queries.lock().map(|q| q.clone()).unwrap_or_default()
        }
    }

    fn answer() -> Answer {
        Answer {
            engine: "DuckDB".into(),
            table: TablePreview {
                format: "SQL".into(),
                delimiter: String::new(),
                columns: vec![
                    TableColumn {
                        name: "city".into(),
                        kind: "text".into(),
                    },
                    TableColumn {
                        name: "total".into(),
                        kind: "number".into(),
                    },
                ],
                rows: vec![
                    vec!["Cairo".into(), "30".into()],
                    vec!["Aswan".into(), "7".into()],
                    vec!["القاهرة".into(), "100".into()],
                ],
                total_rows_sampled: 3,
                truncated: true,
                elapsed_ms: 3.5,
            },
            load_ms: 0.,
        }
    }

    impl Backend for Fake {
        fn tables(&self, _: Vec<String>) -> Fut<Tables> {
            let tables = self.tables.clone();
            Box::pin(async move { tables })
        }

        fn query(&self, paths: Vec<String>, sql: String) -> Fut<Answer> {
            if let Ok(mut queries) = self.queries.lock() {
                queries.push((paths, sql.clone()));
            }
            Box::pin(async move {
                if sql.contains("nope") {
                    Err("Catalog Error: Table with name nope does not exist!".to_string())
                } else {
                    Ok(answer())
                }
            })
        }

        fn profile(&self, _: String) -> Fut<Profile> {
            let profile = self.profile.clone();
            Box::pin(async move { profile.ok_or_else(|| "no profile".to_string()) })
        }

        fn convert(
            &self,
            files: Vec<String>,
            format: DataFormat,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut converts) = self.converts.lock() {
                converts.push((files, format, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: 1,
                current: "sales.csv".into(),
                part: 0.5,
            });
            let report = self.report.clone();
            Box::pin(async move { Ok(report) })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }
    }

    #[test]
    fn the_profile_explains_its_headers_and_cuts_nothing_short_without_saying_so() {
        let _guard = testing::lock();
        assert_eq!(profile_header_tip(2).as_deref(), Some("Rows without a value"));
        assert_eq!(
            profile_header_tip(3).as_deref(),
            Some("About how many different values")
        );
        assert_eq!(
            profile_header_tip(7).as_deref(),
            Some("Lower quartile, median, upper quartile")
        );
        for plain in [0, 1, 4, 5, 6] {
            assert_eq!(profile_header_tip(plain), None, "column {plain}");
        }
        let mut c = column("age", "number", 0.);
        c.min = "29".into();
        c.max = "41".into();
        c.std = "6.02".into();
        assert_eq!(profile_cell_tip(&c, 0).as_deref(), Some("age"));
        assert_eq!(profile_cell_tip(&c, 1).as_deref(), Some("BIGINT"));
        assert_eq!(profile_cell_tip(&c, 4).as_deref(), Some("29"));
        assert_eq!(profile_cell_tip(&c, 5).as_deref(), Some("41"));
        assert_eq!(profile_cell_tip(&c, 6).as_deref(), Some("Standard deviation 6.02"));
        // No spread known, no tip on the mean; the counts need none.
        c.std = String::new();
        assert_eq!(profile_cell_tip(&c, 6), None);
        assert_eq!(profile_cell_tip(&c, 3), None);
        assert_eq!(profile_cell_tip(&c, 7), None);
    }

    fn column(name: &str, kind: &str, nulls: f64) -> ColumnProfile {
        ColumnProfile {
            name: name.into(),
            kind: kind.into(),
            duck_type: if kind == "number" {
                "BIGINT".into()
            } else {
                "VARCHAR".into()
            },
            count: 1000,
            null_percent: nulls,
            distinct: 12_345,
            min: "1".into(),
            max: "99".into(),
            mean: "35.33".into(),
            std: "4".into(),
            q25: "10".into(),
            q50: "20".into(),
            q75: "30".into(),
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the data tools never reached the state the test waits for");
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn disabled(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
        })
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

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

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Play,
            IconName::Database,
            IconName::Sigma,
            IconName::FileCog,
            IconName::Table2,
            IconName::Hash,
            IconName::ToggleLeft,
            IconName::Type,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::CircleDot,
            IconName::Circle,
            IconName::TriangleAlert,
            IconName::Download,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn which_files_it_takes_and_how_sql_names_read_as_the_typescript_did() {
        assert!(is_data_file("C:\\d\\a.CSV") && is_data_file("C:\\d\\a.parquet") && is_data_file("C:\\d\\a.ndjson"));
        assert!(!is_data_file("C:\\d\\a.txt") && !is_data_file("C:\\d\\a.xlsx") && !is_data_file("C:\\d\\Folder"));
        assert_eq!(sql_name("sales"), "sales");
        assert_eq!(sql_name("_x1"), "_x1");
        assert_eq!(sql_name("my table"), "\"my table\"");
        assert_eq!(sql_name("1st"), "\"1st\"");
        assert_eq!(sql_name("a\"b"), "\"a\"\"b\"");
        assert_eq!(first_rows("my table"), "SELECT * FROM \"my table\" LIMIT 100");
        // Parquet files become CSV, everything else Parquet.
        assert_eq!(
            first_format(&strings(&["C:\\d\\a.parquet", "C:\\d\\b.PARQUET"])),
            DataFormat::Csv
        );
        assert_eq!(
            first_format(&strings(&["C:\\d\\a.parquet", "C:\\d\\b.csv"])),
            DataFormat::Parquet
        );
        // The missing bar's number: two places under one percent, none otherwise.
        assert_eq!(missing_text(0.), "0%");
        assert_eq!(missing_text(0.5), "0.50%");
        assert_eq!(missing_text(12.4), "12%");
        assert_eq!(missing_text(2.5), "3%");
    }

    #[gpui_kit::test]
    fn querying_together_loads_the_tables_runs_the_first_and_answers_what_is_typed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(&["C:\\d\\sales.csv", "C:\\d\\my table.csv"]);
        let view =
            visual.update(|window, cx| open_together_with(window, cx, backend, Tooling::with(&["duckdb"]), files));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Query together"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "sales.csv, my table.csv"), Some(Role::Group));
        let query = view.read_with(&visual, |v, _| v.query().cloned());
        let query = query.expect("DuckDB is there, so the query view is made");
        settle(&mut visual, |cx| !query.read_with(cx, |q, cx| q.rows(cx)).is_empty());
        // The first table's rows ran, over both files.
        let queries = fake.queries();
        assert_eq!(queries[0].0, ["C:\\d\\sales.csv", "C:\\d\\my table.csv"]);
        assert_eq!(queries[0].1, "SELECT * FROM sales LIMIT 100");
        assert_eq!(
            query.read_with(&visual, |q, cx| q.sql(cx)),
            "SELECT * FROM sales LIMIT 100"
        );
        assert_eq!(role_of(&mut visual, "Tables"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "my table"), Some(Role::Button));
        assert_eq!(role_of(&mut visual, "2 cols · 3+ rows"), Some(Role::Status));
        assert_eq!(
            role_of(
                &mut visual,
                "The first 1,000 rows of the answer: LIMIT and OFFSET show others."
            ),
            Some(Role::Status)
        );
        assert!(has(
            &mut visual,
            "DuckDB. DuckDB reads the file where it is, however big"
        ));
        // Another table: its first rows, quoted because its name isn't a plain word.
        click(&mut visual, ("query-table", 1_usize));
        settle(&mut visual, |_| fake.queries().len() == 2);
        assert_eq!(fake.queries()[1].1, "SELECT * FROM \"my table\" LIMIT 100");
        assert_eq!(
            query.read_with(&visual, |q, cx| q.sql(cx)),
            "SELECT * FROM \"my table\" LIMIT 100"
        );
        // A query typed in, run with Ctrl+Enter (the Arabic city is a cell like another).
        visual.update(|window, cx| {
            let sql = query.read(cx).sql_input().clone();
            sql.update(cx, |sql, cx| sql.focus(window, cx));
            window.press("ctrl-a", cx);
            window.input("SELECT city FROM sales", cx);
            window.press("ctrl-enter", cx);
        });
        settle(&mut visual, |_| fake.queries().len() == 3);
        assert!(fake.queries()[2].1.starts_with("SELECT city FROM sales"));
        settle(&mut visual, |cx| !query.read_with(cx, |q, cx| q.rows(cx)).is_empty());
        assert_eq!(query.read_with(&visual, |q, cx| q.rows(cx)[2][0].clone()), "القاهرة");
        // A query that fails shows the engine's words, in the table's place.
        visual.update(|window, cx| {
            query.update(cx, |q, cx| q.run("SELECT * FROM nope".into(), window, cx));
        });
        settle(&mut visual, |cx| query.read_with(cx, |q, _| q.error().is_some()));
        assert_eq!(
            role_of(&mut visual, "Catalog Error: Table with name nope does not exist!"),
            Some(Role::Alert)
        );
    }

    #[gpui_kit::test]
    fn querying_together_asks_for_duckdb_first(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        let view = visual.update(|window, cx| {
            open_together_with(
                window,
                cx,
                backend,
                Tooling::with(&[]),
                strings(&["C:\\d\\a.csv", "C:\\d\\b.csv"]),
            )
        });
        visual.run_until_parked();
        assert!(
            view.read_with(&visual, |v, _| v.query().is_none()),
            "nothing is loaded without DuckDB"
        );
        assert!(fake.queries().is_empty());
        // Fewer than two files: it says so, and nothing opens.
        visual.update(|window, cx| {
            window.close_dialog(cx);
            query_together(window, cx, strings(&["C:\\d\\a.csv"]));
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn a_file_that_cannot_be_opened_is_told_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let mut value = Fake::new();
        if let Some(inner) = Arc::get_mut(&mut value) {
            inner.tables = Err("The file is too big to load".into());
        }
        let backend: Arc<dyn Backend> = value.clone();
        let (mut visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| QueryView::new(strings(&[r"C:\d\a.csv"]), None, backend, Tooling::with(&[]), window, cx))
        });
        // Loading shows first; then the reason, as an alert, and nothing was run.
        settle(&mut visual, |cx| has(cx, "The file is too big to load"));
        assert_eq!(role_of(&mut visual, "The file is too big to load"), Some(Role::Alert));
        assert!(view.read_with(&visual, |q, _| q.tables().is_none()));
        assert!(value.queries().is_empty());
    }

    #[gpui_kit::test]
    fn converting_data_picks_a_format_and_reports(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        // DuckDB isn't there: Convert waits.
        let _missing = visual.update(|window, cx| {
            open_convert_with(
                window,
                cx,
                backend.clone(),
                Tooling::with(&[]),
                strings(&["C:\\d\\sales.csv"]),
            )
        });
        visual.run_until_parked();
        assert!(disabled(&mut visual, "data-convert-start"));
        visual.update(|window, cx| {
            window.press("escape", cx);
        });
        visual.run_until_parked();

        let view = visual.update(|window, cx| {
            open_convert_with(
                window,
                cx,
                backend,
                Tooling::with(&["duckdb"]),
                strings(&["C:\\d\\sales.csv", "C:\\d\\more.json"]),
            )
        });
        visual.run_until_parked();
        assert_eq!(role_of(&mut visual, "Convert data: 2 data files"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\d"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Format"), Some(Role::RadioGroup));
        assert_eq!(
            role_of(&mut visual, "Parquet. Columns, Zstandard: far smaller and faster"),
            Some(Role::RadioButton)
        );
        assert_eq!(view.read_with(&visual, |v, _| v.format()), DataFormat::Parquet);
        click(&mut visual, ("data-convert-format", 2_usize));
        assert_eq!(view.read_with(&visual, |v, _| v.format()), DataFormat::Jsonl);
        assert!(!disabled(&mut visual, "data-convert-start"));
        click(&mut visual, "data-convert-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let converts = fake.converts.lock().map(|c| c.clone()).unwrap_or_default();
        assert_eq!(converts[0].0, ["C:\\d\\sales.csv", "C:\\d\\more.json"]);
        assert_eq!(converts[0].1, DataFormat::Jsonl);
        assert_eq!(converts[0].2, Output::Beside { suffix: String::new() });
        visual.update(|window, _| {
            assert!(window
                .find_all_by_role(Role::Status)
                .iter()
                .any(|e| e.label().is_some_and(|l| l.contains("(\u{2212}75%)"))));
        });
        click(&mut visual, "data-convert-close");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn conversion_progress_stop_and_failure_are_drawn(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new();
        let backend: Arc<dyn Backend> = fake.clone();
        let view = visual.update(|window, cx| {
            open_convert_with(
                window,
                cx,
                backend,
                Tooling::with(&["duckdb"]),
                strings(&["C:\\d\\مبيعات.csv"]),
            )
        });
        visual.run_until_parked();
        let run = view.read_with(&visual, |v, _| v.run().clone());
        testing::set_rtl(true);
        visual.update(|_, cx| {
            run.update(cx, |run, cx| {
                run.set(
                    Run::Running(Progress {
                        done: 0,
                        total: 1,
                        current: "مبيعات.csv".into(),
                        part: 0.,
                    }),
                    cx,
                )
            })
        });
        assert_eq!(
            role_of(&mut visual, "Converting · مبيعات.csv"),
            Some(Role::ProgressIndicator)
        );
        click(&mut visual, "data-convert-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Failed("DuckDB stopped".into()), cx)));
        assert_eq!(
            role_of(&mut visual, "Couldn't convert. DuckDB stopped"),
            Some(Role::Alert)
        );
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn the_profile_describes_every_column(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let mut value = Fake::new();
        if let Some(inner) = Arc::get_mut(&mut value) {
            inner.profile = Some(Profile {
                rows: 1000,
                columns: vec![column("id", "number", 0.), column("الاسم", "text", 25.5)],
                elapsed_ms: 42.,
            });
        }
        let backend: Arc<dyn Backend> = value.clone();
        let (mut visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| {
                ProfileView::new(
                    r"C:\d\sales.csv".into(),
                    backend,
                    Tooling::with(&["duckdb"]),
                    window,
                    cx,
                )
            })
        });
        settle(&mut visual, |cx| !view.read_with(cx, |v, cx| v.cells(cx)).is_empty());
        assert_eq!(
            role_of(&mut visual, "1,000 rows · 2 columns · read by DuckDB in 42 ms"),
            Some(Role::Status)
        );
        let cells = view.read_with(&visual, |v, cx| v.cells(cx));
        assert_eq!(
            cells[0],
            ["id", "BIGINT", "0%", "12,345", "1", "99", "35.33", "10 · 20 · 30"]
        );
        // An Arabic column name stays whole, and a quarter missing is 26% (JavaScript's half-up).
        assert_eq!(cells[1][0], "الاسم");
        assert_eq!(cells[1][2], "26%");
    }

    #[gpui_kit::test]
    fn the_profile_waits_for_duckdb_and_says_when_it_fails(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let value = Fake::new();
        let backend: Arc<dyn Backend> = value.clone();
        let (mut visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| ProfileView::new(r"C:\d\sales.csv".into(), backend, Tooling::with(&[]), window, cx))
        });
        visual.run_until_parked();
        assert!(view.read_with(&visual, |v, cx| v.cells(cx)).is_empty());
        assert!(has(&mut visual, "Reading every row…"), "nothing is read without DuckDB");

        // With DuckDB there but nothing to say, the reason shows.
        let backend: Arc<dyn Backend> = value.clone();
        let (mut visual, _view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| {
                ProfileView::new(
                    r"C:\d\sales.csv".into(),
                    backend,
                    Tooling::with(&["duckdb"]),
                    window,
                    cx,
                )
            })
        });
        settle(&mut visual, |cx| has(cx, "no profile"));
        assert_eq!(role_of(&mut visual, "no profile"), Some(Role::Alert));
    }
}
