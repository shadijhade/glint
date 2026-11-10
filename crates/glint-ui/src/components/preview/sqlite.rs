//! SQLite databases: their tables and SQL (`SqlitePreview` in DataPreview.tsx, `isSqliteCandidate`).
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `SqlitePreview`                              | [`SqlitePreview`]: the header's facts, three tabs, a filter, a card per schema object |
//! | `inspectSqlite`                              | [`SqliteInspect`] (glint-core's `inspect_sqlite`, which reads the header and the schema page, not the data) |
//! | the `grouped` memo, `match`                  | [`Group`], [`matching`], [`totals`]                           |
//! | `CopySqlButton`                              | the copy button on a card, with its check mark for 1.4 s      |
//! | the card's Rows button, `showQuery`          | [`SqlitePreview::show_rows`]: the table's rows in a [`QueryView`] (see below) |
//! | `isSqliteCandidate`                          | [`is_sqlite_candidate`]                                       |
//!
//! **A table's rows.** In the TypeScript the Rows button switched the whole preview to its Query view with
//! `SELECT * FROM "table" LIMIT 1000` written and run. The frame has no Query view yet, so the preview shows the
//! [`QueryView`] itself (the one `dialogs::data_tools` holds: a SQL box, Ctrl+Enter or Run, the tables as
//! buttons, and the answer in the kit's virtualized `DataTable`) in place of the schema, under a Back button.
//! When the frame does have a Query view, call [`SqlitePreview::set_query_in_frame`]`(true)`: Rows then emits
//! [`SqliteEvent::ShowQuery`] and the frame does what `showQuery` did (the file's draft, the Query view).
//!
//! The schema is read from the file's first pages by glint-core (up to 512 objects), in the background. The cards
//! differ in height (a table's `CREATE` statement is as long as it is), so a `list` draws only the ones on screen.

use super::text::{big_icon, name_of, redraw_after_delay, waiting};
use crate::dialogs::action_run::{self, Tooling};
use crate::dialogs::data_tools::{core_backend, sql_name, Backend, QueryView};
use crate::kit::prelude::*;
use crate::{actions, core_bridge, t, theme};
use glint_core::data_lens::{SqliteInfo, SqliteObject};
use glint_model::format::{format_count, format_size};
use gpui_kit::{list, ClipboardItem, ListAlignment, ListState, Task};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The extensions a database has (`SQLITE_EXTS`).
const SQLITE_EXTS: [&str; 6] = ["sqlite", "sqlite3", "db", "db3", "s3db", "sl3"];

/// How long the check mark of a copied statement stays.
const COPIED_FOR: Duration = Duration::from_millis(1400);

/// Whether the file is a database the preview reads, by its extension (`isSqliteCandidate`, for a file on disk).
pub fn is_sqlite_candidate(path: &str) -> bool {
    SQLITE_EXTS.contains(&super::text::ext_of(path).as_str())
}

/// Reads a database's header and schema: the path in, what glint-core finds out.
pub type SqliteInspect =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<SqliteInfo, String>> + Send>> + Send + Sync>;

/// glint-core's `inspect_sqlite`.
pub fn core_sqlite_inspect() -> SqliteInspect {
    Arc::new(|path| Box::pin(glint_core::data_lens::inspect_sqlite(path)))
}

// ---- The schema, grouped -----------------------------------------------------------------------------------

/// The three tabs: what the schema holds, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Tables,
    Indexes,
    /// Views and triggers.
    Other,
}

/// Which tab an object belongs to: tables and indexes have their own, views, triggers and anything else share one.
pub fn group_of(object: &SqliteObject) -> Group {
    match object.kind.as_str() {
        "table" => Group::Tables,
        "index" => Group::Indexes,
        _ => Group::Other,
    }
}

/// How many tables, indexes and other objects the database has (the numbers on the tabs).
pub fn totals(info: &SqliteInfo) -> (usize, usize, usize) {
    let count = |group| info.objects.iter().filter(|o| group_of(o) == group).count();
    (count(Group::Tables), count(Group::Indexes), count(Group::Other))
}

/// The objects of a tab that match the filter, as indices into `info.objects`, in the schema's order: the text
/// is in the name, the statement or the table's name, in any case.
pub fn matching(info: &SqliteInfo, group: Group, query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    info.objects
        .iter()
        .enumerate()
        .filter(|(_, o)| group_of(o) == group)
        .filter(|(_, o)| {
            q.is_empty()
                || o.name.to_lowercase().contains(&q)
                || o.sql.to_lowercase().contains(&q)
                || o.table_name.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// The query the Rows button writes (`showQuery(path, \`SELECT * FROM ${sqlName(name)} LIMIT 1000\`)`).
pub fn rows_query(table: &str) -> String {
    format!("SELECT * FROM {} LIMIT 1000", sql_name(table))
}

// ---- The previewer -------------------------------------------------------------------------------------------

/// What the preview asks of its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqliteEvent {
    /// The Rows button, when [`SqlitePreview::set_query_in_frame`] is on: show the file's Query view with `sql`
    /// written and run (`showQuery`).
    ShowQuery { path: String, sql: String },
}

impl EventEmitter<SqliteEvent> for SqlitePreview {}

enum Load {
    Waiting,
    Failed(SharedString),
    Ready(Arc<SqliteInfo>),
}

/// A database's tables, indexes, views and triggers.
pub struct SqlitePreview {
    path: String,
    inspect: SqliteInspect,
    backend: Arc<dyn Backend>,
    load: Load,
    tab: Group,
    filter: Entity<InputState>,
    /// The objects the tab and the filter leave, as indices into the schema.
    shown: Vec<usize>,
    cards: ListState,
    /// The table's rows, in place of the schema.
    rows: Option<Entity<QueryView>>,
    query_in_frame: bool,
    /// The card whose statement was copied a moment ago.
    copied: Option<usize>,
    started: Instant,
    generation: u64,
    _read: Task<()>,
    _reset: Option<Task<()>>,
    _filter_events: Subscription,
}

impl SqlitePreview {
    /// The previewer for the database at `path`. Reading happens in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = core_backend(cx);
        Self::with_source(path, core_sqlite_inspect(), backend, window, cx)
    }

    /// [`SqlitePreview::new`] reading through `inspect` and querying through `backend` (tests pass fakes).
    pub fn with_source(
        path: String,
        inspect: SqliteInspect,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter schema…")));
        let events = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.apply_filter(cx);
            }
        });
        let mut this = Self {
            path,
            inspect,
            backend,
            load: Load::Waiting,
            tab: Group::Tables,
            filter,
            shown: Vec::new(),
            cards: ListState::new(0, ListAlignment::Top, px(400.)),
            rows: None,
            query_in_frame: false,
            copied: None,
            started: Instant::now(),
            generation: 0,
            _read: Task::ready(()),
            _reset: None,
            _filter_events: events,
        };
        this.read(cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another database; the tab and the filter start over.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.tab = Group::Tables;
            self.rows = None;
            self.filter.update(cx, |filter, cx| filter.set_value("", window, cx));
            self.read(cx);
        }
    }

    /// Reads the database again (it changed on disk: the TypeScript read again when `entry.modified` changed).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.read(cx);
    }

    /// Whether the frame has a Query view for the Rows button to switch to (it doesn't yet).
    pub fn set_query_in_frame(&mut self, in_frame: bool) {
        self.query_in_frame = in_frame;
    }

    /// The database, once read.
    pub fn info(&self) -> Option<&SqliteInfo> {
        match &self.load {
            Load::Ready(info) => Some(info),
            _ => None,
        }
    }

    /// The tab showing.
    pub fn tab(&self) -> Group {
        self.tab
    }

    /// The names of the objects the tab and the filter leave.
    pub fn shown_names(&self) -> Vec<String> {
        match &self.load {
            Load::Ready(info) => self
                .shown
                .iter()
                .filter_map(|&i| info.objects.get(i).map(|o| o.name.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The table's rows, when they are showing in place of the schema.
    pub fn rows_view(&self) -> Option<&Entity<QueryView>> {
        self.rows.as_ref()
    }

    /// Switches tab.
    pub fn set_tab(&mut self, tab: Group, cx: &mut Context<Self>) {
        self.tab = tab;
        self.apply_filter(cx);
    }

    /// Scrolls the schema a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if self.rows.is_some() {
            return false;
        }
        let step = self.cards.viewport_bounds().size.height * 0.85;
        self.cards.scroll_by(if down { step } else { -step });
        cx.notify();
        true
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Waiting;
        self.started = Instant::now();
        let read = (self.inspect)(self.path.clone());
        self._read = cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.load = match result {
                        Ok(info) => Load::Ready(Arc::new(info)),
                        Err(reason) => Load::Failed(reason.into()),
                    };
                    this.apply_filter(cx);
                }
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The tab or the filter changed, or the schema arrived: the cards to draw.
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().to_string();
        self.shown = match &self.load {
            Load::Ready(info) => matching(info, self.tab, &query),
            _ => Vec::new(),
        };
        self.cards.reset(self.shown.len());
        self.copied = None;
        cx.notify();
    }

    /// The Rows button: the table's first 1,000 rows, run.
    pub fn show_rows(&mut self, table: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sql = rows_query(table);
        if self.query_in_frame {
            cx.emit(SqliteEvent::ShowQuery {
                path: self.path.clone(),
                sql,
            });
            return;
        }
        let (paths, backend) = (vec![self.path.clone()], self.backend.clone());
        let tooling = Tooling::new(action_run::tools_model(window, cx));
        self.rows = Some(cx.new(|cx| QueryView::new(paths, Some(sql), backend, tooling, window, cx)));
        cx.notify();
    }

    /// Back from the rows to the schema.
    pub fn show_schema(&mut self, cx: &mut Context<Self>) {
        if self.rows.take().is_some() {
            cx.notify();
        }
    }

    /// Copies a statement; its button shows a check mark for a moment (`CopySqlButton`).
    fn copy_sql(&mut self, index: usize, cx: &mut Context<Self>) {
        let Load::Ready(info) = &self.load else { return };
        let Some(object) = info.objects.get(index) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(object.sql.clone()));
        self.copied = Some(index);
        cx.notify();
        if super::text::SPINNER_DELAY.is_zero() {
            // Tests: no timer; the mark stays so it can be looked at.
            return;
        }
        self._reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// A schema object's card: name, table, root page, Rows and Copy, and the statement.
    fn card(&mut self, position: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let mono = theme::mono_font(cx);
        let Load::Ready(info) = &self.load else {
            return div().into_any_element();
        };
        let Some(&index) = self.shown.get(position) else {
            return div().into_any_element();
        };
        let Some(object) = info.objects.get(index).cloned() else {
            return div().into_any_element();
        };
        let name: SharedString = object.name.clone().into();
        let on_table = !object.table_name.is_empty() && object.table_name != object.name;
        let has_rows = object.kind == "table" || object.kind == "view";
        let copied = self.copied == Some(index);
        let object_name = object.name.clone();
        let header = row_start()
            .flex_none()
            .items_center()
            .gap(space(2.))
            .px(space(2.5))
            .py(space(1.5))
            .border_b_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.2))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(mono.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.foreground)
                    .child(isolate(name.clone())),
            )
            .when(on_table, |this| {
                this.child(
                    div()
                        .flex_none()
                        .px(space(1.5))
                        .py(space(0.5))
                        .rounded(theme::size::RADIUS_SM)
                        .bg(t.muted)
                        .font_family(mono.clone())
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(t!("on {table}", table = isolate(object.table_name.clone()))),
                )
            })
            .child(
                div()
                    .ms(0.)
                    .flex_1()
                    .flex()
                    .justify_end()
                    .font_family(mono.clone())
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(t!("page #{page}", page = object.root_page)),
            )
            .when(has_rows, |this| {
                this.child(
                    Button::new(("sqlite-rows", index))
                        .ghost()
                        .compact()
                        .icon(IconName::Rows3)
                        .label(t!("Rows"))
                        .tooltip(t!("Its rows, in the Query view"))
                        .accessibility_label(t!("Rows of {name}", name = object.name.clone()))
                        .on_click(cx.listener({
                            let table = object_name.clone();
                            move |this, _, window, cx| this.show_rows(&table, window, cx)
                        })),
                )
            })
            .when(!object.sql.is_empty(), |this| {
                this.child(
                    Button::new(("sqlite-copy", index))
                        .ghost()
                        .compact()
                        .icon(if copied { IconName::Check } else { IconName::Copy })
                        .tooltip(t!("Copy DDL statement"))
                        .accessibility_label(t!("Copy DDL statement of {name}", name = object.name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.copy_sql(index, cx))),
                )
            });
        let statement = if object.sql.is_empty() {
            div()
                .p(space(2.5))
                .italic()
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(t!("Internal auto-index (no SQL statement)"))
        } else {
            div()
                .p(space(2.5))
                .font_family(mono.clone())
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(SharedString::from(object.sql.clone()))
        };
        div()
            .w_full()
            .pb(space(2.5))
            .child(
                col()
                    .id(("sqlite-object", index))
                    .role(Role::ListItem)
                    .aria_label(format!("{}: {}", object.kind, object.name))
                    .test_support()
                    .w_full()
                    .overflow_hidden()
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.background.opacity(0.5))
                    .child(header)
                    .child(statement),
            )
            .into_any_element()
    }

    /// The header: the version, encoding and journal mode, and the numbers.
    fn telemetry(&self, info: &SqliteInfo, t: &theme::Tokens, mono: &SharedString) -> impl IntoElement {
        let sky = theme::tailwind(if t.dark { "sky-400" } else { "sky-600" }).unwrap_or(t.primary);
        let green = theme::tailwind(if t.dark { "emerald-400" } else { "emerald-600" }).unwrap_or(t.primary);
        let wal = info.journal_mode == "WAL";
        let version = t!("SQLite {version}", version = info.sqlite_version);
        let mode = t!("{mode} mode", mode = info.journal_mode);
        let chip = |id: &'static str, label: SharedString, color: Hsla, ground: Hsla| {
            div()
                .id(id)
                .role(Role::Group)
                .aria_label(label.clone())
                .test_support()
                .px(space(2.))
                .py(space(0.5))
                .rounded(theme::size::RADIUS)
                .bg(ground)
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(label)
        };
        let fact = |id: &'static str, label: SharedString, value: String| {
            row_start()
                .id(id)
                .role(Role::Group)
                .aria_label(format!("{label}: {value}"))
                .test_support()
                .min_w_0()
                .justify_between()
                .gap(space(2.))
                .child(div().text_color(t.muted_foreground).child(label))
                .child(div().font_family(mono.clone()).child(value))
        };
        col()
            .flex_none()
            .gap(space(2.5))
            .px(space(3.5))
            .py(space(3.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.2))
            .child(
                row_start()
                    .flex_wrap()
                    .items_center()
                    .gap(space(1.5))
                    .child(
                        row_start()
                            .id("sqlite-version")
                            .role(Role::Group)
                            .aria_label(version.clone())
                            .test_support()
                            .gap(space(1.))
                            .px(space(2.))
                            .py(space(0.5))
                            .rounded(theme::size::RADIUS)
                            .bg(sky.opacity(0.15))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(sky)
                            .child(icon(IconName::Database).size_3())
                            .child(version),
                    )
                    .child(chip(
                        "sqlite-encoding",
                        info.encoding.clone().into(),
                        t.foreground,
                        t.muted,
                    ))
                    .child(chip(
                        "sqlite-journal",
                        mode,
                        if wal { green } else { t.muted_foreground },
                        if wal { green.opacity(0.15) } else { t.muted },
                    ))
                    .child(
                        div()
                            .ms(0.)
                            .flex_1()
                            .flex()
                            .justify_end()
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(format!("{:.2} ms", info.elapsed_ms)),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_x(space(4.))
                    .gap_y(space(1.))
                    .text_size(theme::size::XXS)
                    .child(fact(
                        "sqlite-page-size",
                        t!("Page size"),
                        format_size(u64::from(info.page_size)),
                    ))
                    .child(fact(
                        "sqlite-pages",
                        t!("Pages"),
                        format_count(info.page_count as usize),
                    ))
                    .child(fact(
                        "sqlite-freelist",
                        t!("Freelist"),
                        format!(
                            "{} ({})",
                            format_count(info.freelist_pages as usize),
                            format_size(u64::from(info.freelist_pages) * u64::from(info.page_size))
                        ),
                    ))
                    .child(fact(
                        "sqlite-user-version",
                        t!("User version"),
                        info.user_version.to_string(),
                    )),
            )
    }

    /// One of the three tabs.
    fn tab_button(
        &self,
        id: &'static str,
        group: Group,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.tab == group;
        let t = theme::tokens(cx).clone();
        Button::new(id)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(if active {
                        t.primary.opacity(0.15)
                    } else {
                        t.background.opacity(0.)
                    })
                    .foreground(if active { t.primary } else { t.muted_foreground })
                    .hover(t.accent)
                    .active(t.accent),
            )
            .compact()
            .h(px(24.))
            .px(space(2.))
            .text_size(theme::size::XS)
            .font_weight(FontWeight::MEDIUM)
            .label(label.clone())
            .selected(active)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, _, cx| this.set_tab(group, cx)))
    }
}

impl Render for SqlitePreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("sqlite-preview")
            .role(Role::Group)
            .aria_label(t!("Database {name}", name = name_of(&self.path)))
            .test_support()
            .size_full()
            .min_h_0()
            .text_start()
            .text_size(theme::size::XS)
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let info = match &self.load {
            Load::Waiting => return root.child(waiting(self.started, Some(t!("Reading the database…")), cx)),
            Load::Failed(reason) => return root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            Load::Ready(info) => info.clone(),
        };
        // The table's rows, under a way back to the schema.
        if let Some(rows) = &self.rows {
            return root
                .child(
                    row_start()
                        .flex_none()
                        .px(space(2.))
                        .py(space(1.5))
                        .border_b_1()
                        .border_color(t.border)
                        .child(
                            Button::new("sqlite-back")
                                .ghost()
                                .small()
                                .icon(back_icon())
                                .label(t!("Schema"))
                                .tooltip(t!("Back to the tables and indexes"))
                                .on_click(cx.listener(|this, _, _, cx| this.show_schema(cx))),
                        ),
                )
                .child(div().flex_1().min_h_0().child(rows.clone()));
        }
        let mono = theme::mono_font(cx);
        let (tables, indexes, other) = totals(&info);
        let tabs = row_start()
            .id("sqlite-tabs")
            .role(Role::TabList)
            .aria_label(t!("Schema"))
            .test_support()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(space(1.))
            .px(space(2.5))
            .py(space(1.5))
            .border_b_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.1))
            .child(self.tab_button("sqlite-tab-tables", Group::Tables, t!("Tables ({n})", n = tables), cx))
            .child(self.tab_button(
                "sqlite-tab-indexes",
                Group::Indexes,
                t!("Indexes ({n})", n = indexes),
                cx,
            ))
            .when(other > 0, |this| {
                this.child(self.tab_button(
                    "sqlite-tab-other",
                    Group::Other,
                    t!("Views & Triggers ({n})", n = other),
                    cx,
                ))
            })
            .child(
                div().ms(0.).flex_1().min_w(space(28.)).child(
                    Input::new(&self.filter)
                        .small()
                        .cleanable(true)
                        .aria_label(t!("Filter schema…"))
                        .prefix(icon(IconName::Search).size_3().text_color(t.muted_foreground)),
                ),
            );
        let body = if self.shown.is_empty() {
            div()
                .id("sqlite-none")
                .role(Role::Status)
                .aria_label(t!("No matching schema objects found."))
                .test_support()
                .py(space(6.))
                .text_center()
                .text_color(t.muted_foreground)
                .child(t!("No matching schema objects found."))
                .into_any_element()
        } else {
            let cards = list(self.cards.clone(), cx.processor(Self::card)).size_full();
            div()
                .id("sqlite-objects")
                .role(Role::List)
                .aria_label(t!("Schema objects"))
                .test_support()
                .relative()
                .size_full()
                .child(cards)
                .vertical_scrollbar(&self.cards)
                .into_any_element()
        };
        root.child(self.telemetry(&info, &t, &mono))
            .child(tabs)
            .child(div().flex_1().min_h_0().px(space(3.)).pt(space(3.)).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::action_run::Fut;
    use crate::dialogs::data_tools::{Answer, Tables};
    use crate::testing;
    use glint_core::actions::{Output, Progress, Report};
    use glint_core::data_lens::{TableColumn, TablePreview};
    use glint_core::duck::{DataFormat, Profile};
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;

    fn object(kind: &str, name: &str, table: &str, page: u64, sql: &str) -> SqliteObject {
        SqliteObject {
            kind: kind.into(),
            name: name.into(),
            table_name: table.into(),
            root_page: page,
            sql: sql.into(),
        }
    }

    /// A database with an Arabic table among its objects.
    fn info() -> SqliteInfo {
        SqliteInfo {
            page_size: 4096,
            page_count: 120,
            freelist_pages: 3,
            journal_mode: "WAL".into(),
            encoding: "UTF-8".into(),
            sqlite_version: "3.45.1".into(),
            user_version: 7,
            application_id: 0,
            objects: vec![
                object(
                    "table",
                    "users",
                    "users",
                    2,
                    "CREATE TABLE users (\n  id INTEGER PRIMARY KEY,\n  name TEXT\n)",
                ),
                object("table", "عملاء", "عملاء", 3, "CREATE TABLE \"عملاء\" (اسم TEXT)"),
                object(
                    "index",
                    "users_name",
                    "users",
                    4,
                    "CREATE INDEX users_name ON users(name)",
                ),
                object("index", "sqlite_autoindex_users_1", "users", 5, ""),
                object(
                    "view",
                    "active_users",
                    "active_users",
                    0,
                    "CREATE VIEW active_users AS SELECT * FROM users",
                ),
                object(
                    "trigger",
                    "users_audit",
                    "users",
                    0,
                    "CREATE TRIGGER users_audit AFTER INSERT ON users BEGIN SELECT 1; END",
                ),
            ],
            elapsed_ms: 0.31,
        }
    }

    fn inspect(answer: Result<SqliteInfo, String>) -> SqliteInspect {
        Arc::new(move |_| {
            let answer = answer.clone();
            Box::pin(async move { answer })
        })
    }

    /// A backend that answers from memory and remembers what it was asked.
    struct Fake {
        queries: Mutex<Vec<(Vec<String>, String)>>,
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake {
                queries: Mutex::new(Vec::new()),
            })
        }

        fn queries(&self) -> Vec<(Vec<String>, String)> {
            self.queries.lock().map(|q| q.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn tables(&self, _: Vec<String>) -> Fut<Tables> {
            Box::pin(async {
                Ok(Tables {
                    tables: vec!["users".into(), "عملاء".into()],
                    engine: "SQLite".into(),
                    load_ms: 0.,
                })
            })
        }

        fn query(&self, paths: Vec<String>, sql: String) -> Fut<Answer> {
            if let Ok(mut queries) = self.queries.lock() {
                queries.push((paths, sql));
            }
            Box::pin(async {
                Ok(Answer {
                    engine: "SQLite".into(),
                    table: TablePreview {
                        format: "SQL".into(),
                        delimiter: String::new(),
                        columns: vec![TableColumn {
                            name: "اسم".into(),
                            kind: "text".into(),
                        }],
                        rows: vec![vec!["أحمد".into()], vec!["سارة".into()]],
                        total_rows_sampled: 2,
                        truncated: false,
                        elapsed_ms: 1.5,
                    },
                    load_ms: 0.,
                })
            })
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

    fn open(
        cx: &mut TestAppContext,
        source: SqliteInspect,
        backend: Arc<dyn Backend>,
    ) -> (Entity<SqlitePreview>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| SqlitePreview::with_source("C:\\data\\shop.sqlite".into(), source, backend, window, cx))
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
        panic!("the database never arrived");
    }

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..3 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    fn ready(view: &Entity<SqlitePreview>, visual: &mut VisualTestContext) {
        settle(visual, |cx| view.read_with(cx, |v, _| v.info().is_some()));
        frames(visual);
    }

    #[test]
    fn databases_are_told_by_their_extension() {
        for path in [
            "C:\\a\\x.sqlite",
            "C:\\a\\x.SQLITE3",
            "C:\\a\\x.db",
            "C:\\a\\x.s3db",
            "C:\\a\\x.sl3",
            "C:\\a\\x.db3",
        ] {
            assert!(is_sqlite_candidate(path), "{path}");
        }
        assert!(!is_sqlite_candidate("C:\\a\\x.csv"));
        assert!(!is_sqlite_candidate("C:\\a\\db"));
    }

    #[test]
    fn the_schema_is_grouped_and_filtered_by_name_statement_or_table() {
        let info = info();
        assert_eq!(totals(&info), (2, 2, 2));
        assert_eq!(matching(&info, Group::Tables, ""), vec![0, 1]);
        assert_eq!(matching(&info, Group::Indexes, ""), vec![2, 3]);
        // Views and triggers share a tab.
        assert_eq!(matching(&info, Group::Other, ""), vec![4, 5]);
        // The name, the statement or the table it is on, in any case; the tab still decides.
        assert_eq!(matching(&info, Group::Tables, "  USERS "), vec![0]);
        assert_eq!(matching(&info, Group::Indexes, "users"), vec![2, 3]);
        assert_eq!(matching(&info, Group::Tables, "primary key"), vec![0]);
        assert_eq!(matching(&info, Group::Tables, "اسم"), vec![1]);
        assert_eq!(matching(&info, Group::Other, "insert"), vec![5]);
        assert!(matching(&info, Group::Tables, "nothing like it").is_empty());
    }

    #[test]
    fn the_rows_button_writes_the_query_the_typescript_wrote() {
        assert_eq!(rows_query("users"), "SELECT * FROM users LIMIT 1000");
        assert_eq!(rows_query("my table"), "SELECT * FROM \"my table\" LIMIT 1000");
        assert_eq!(rows_query("عملاء"), "SELECT * FROM \"عملاء\" LIMIT 1000");
        assert_eq!(rows_query("a\"b"), "SELECT * FROM \"a\"\"b\" LIMIT 1000");
    }

    #[gpui_kit::test]
    fn the_header_and_the_tables_show_with_an_arabic_name_among_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (view, mut visual) = open(cx, inspect(Ok(info())), Fake::new());
        // The read is under way: the UI thread didn't wait for it.
        assert!(view.read_with(&visual, |v, _| v.info().is_none()));
        assert!(has(&mut visual, "Reading the database…"));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "SQLite 3.45.1"));
        assert!(has(&mut visual, "UTF-8"));
        assert!(has(&mut visual, "WAL mode"));
        assert!(has(&mut visual, "Page size: 4.0 KB"));
        assert!(has(&mut visual, "Pages: 120"));
        assert!(has(&mut visual, "Freelist: 3 (12 KB)"));
        assert!(has(&mut visual, "User version: 7"));
        assert!(has(&mut visual, "Tables (2)"));
        assert!(has(&mut visual, "Indexes (2)"));
        assert!(has(&mut visual, "Views & Triggers (2)"));
        assert!(has(&mut visual, "table: users"));
        assert!(
            has(&mut visual, "table: عملاء"),
            "an Arabic table is named for what it is"
        );
        assert_eq!(view.read_with(&visual, |v, _| v.shown_names()), ["users", "عملاء"]);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn the_tabs_and_the_filter_choose_what_is_drawn(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, inspect(Ok(info())), Fake::new());
        ready(&view, &mut visual);
        visual.update(|window, cx| window.click("sqlite-tab-indexes", cx));
        frames(&mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.tab()), Group::Indexes);
        assert_eq!(
            view.read_with(&visual, |v, _| v.shown_names()),
            ["users_name", "sqlite_autoindex_users_1"]
        );
        assert!(
            has(&mut visual, "Internal auto-index (no SQL statement)")
                || has(&mut visual, "index: sqlite_autoindex_users_1")
        );
        visual.update(|window, cx| window.click("sqlite-tab-other", cx));
        frames(&mut visual);
        assert_eq!(
            view.read_with(&visual, |v, _| v.shown_names()),
            ["active_users", "users_audit"]
        );
        // A filter that matches nothing says so.
        visual.update(|window, cx| {
            let filter = view.read(cx).filter.clone();
            filter.update(cx, |f, cx| f.focus(window, cx));
            window.input("zzz", cx);
        });
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.shown_names().is_empty()));
        assert!(has(&mut visual, "No matching schema objects found."));
    }

    #[gpui_kit::test]
    fn rows_run_the_query_in_the_query_view_and_back_returns_to_the_schema(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let backend = Fake::new();
        let (view, mut visual) = open(cx, inspect(Ok(info())), backend.clone());
        ready(&view, &mut visual);
        visual.update(|window, cx| window.click(("sqlite-rows", 1_usize), cx));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.rows_view().is_some()));
        let rows = view.read_with(&visual, |v, _| v.rows_view().cloned());
        let Some(rows) = rows else {
            panic!("the rows are showing")
        };
        settle(&mut visual, |cx| {
            !backend.queries().is_empty() && rows.read_with(cx, |q, _| !q.running())
        });
        // The draft runs instead of the first table's rows.
        assert_eq!(
            backend.queries(),
            vec![(
                vec!["C:\\data\\shop.sqlite".to_string()],
                "SELECT * FROM \"عملاء\" LIMIT 1000".to_string()
            )]
        );
        frames(&mut visual);
        assert_eq!(
            rows.read_with(&visual, |q, cx| q.rows(cx)),
            vec![vec!["أحمد".to_string()], vec!["سارة".to_string()]]
        );
        assert!(has(&mut visual, "Schema") || has(&mut visual, "sqlite-back"));
        visual.update(|window, cx| window.click("sqlite-back", cx));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.rows_view().is_none()));
        assert!(has(&mut visual, "table: users"));
    }

    #[gpui_kit::test]
    fn with_a_query_view_in_the_frame_rows_asks_it_instead(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, inspect(Ok(info())), Fake::new());
        ready(&view, &mut visual);
        view.update(&mut visual, |v, _| v.set_query_in_frame(true));
        let heard = Arc::new(Mutex::new(Vec::new()));
        let log = heard.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &SqliteEvent, _| {
                if let Ok(mut log) = log.lock() {
                    log.push(event.clone());
                }
            })
        });
        visual.update(|window, cx| window.click(("sqlite-rows", 0_usize), cx));
        frames(&mut visual);
        assert_eq!(
            heard.lock().map(|h| h.clone()).unwrap_or_default(),
            vec![SqliteEvent::ShowQuery {
                path: "C:\\data\\shop.sqlite".into(),
                sql: "SELECT * FROM users LIMIT 1000".into()
            }]
        );
        assert!(view.read_with(&visual, |v, _| v.rows_view().is_none()));
    }

    #[gpui_kit::test]
    fn the_copy_button_copies_the_statement_and_shows_a_check(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, inspect(Ok(info())), Fake::new());
        ready(&view, &mut visual);
        visual.update(|window, cx| window.click(("sqlite-copy", 1_usize), cx));
        frames(&mut visual);
        let copied = visual.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(copied.as_deref(), Some("CREATE TABLE \"عملاء\" (اسم TEXT)"));
        assert_eq!(view.read_with(&visual, |v, _| v.copied), Some(1));
        // An index with no statement has nothing to copy.
        visual.update(|window, cx| window.click("sqlite-tab-indexes", cx));
        frames(&mut visual);
        assert!(!has(&mut visual, "Copy DDL statement of sqlite_autoindex_users_1"));
        assert!(has(&mut visual, "Copy DDL statement of users_name"));
    }

    #[gpui_kit::test]
    fn a_file_that_is_not_a_database_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, inspect(Err("This isn't an SQLite database".into())), Fake::new());
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Failed(_)))
        });
        assert!(has(&mut visual, "This isn't an SQLite database"));
    }

    #[gpui_kit::test]
    fn a_long_schema_only_draws_the_cards_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut big = info();
        big.objects = (0..500)
            .map(|i| {
                object(
                    "table",
                    &format!("table_{i}"),
                    &format!("table_{i}"),
                    i + 2,
                    "CREATE TABLE t (a, b, c)",
                )
            })
            .collect();
        let (view, mut visual) = open(cx, inspect(Ok(big)), Fake::new());
        ready(&view, &mut visual);
        assert!(has(&mut visual, "Tables (500)"));
        let drawn = visual.update(|window, _| window.find_all_by_role(Role::ListItem).len());
        assert!(drawn > 0 && drawn < 60, "{drawn} cards drawn");
    }

    #[test]
    fn every_icon_the_schema_view_draws_is_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Database,
            IconName::Rows3,
            IconName::Copy,
            IconName::Check,
            IconName::Search,
            IconName::ArrowLeft,
            IconName::ArrowRight,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
