//! SQL in the preview and for agents, reading only, answered by DuckDB (duck.rs, a tool Glint installs). What a file
//! is decides how DuckDB gets at it: a CSV, TSV, JSON-lines, JSON or Parquet file is read where it is (the table
//! `data`); a SQLite database is attached read-only through DuckDB's `sqlite` extension (its own tables); a
//! workbook's sheets are written as CSV files for the length of the query and read from those (a table named after
//! each sheet, the first also `data`), then the copies are deleted. Without DuckDB there is no SQL, and the error
//! says so, for the window to offer the install; the schema and the rows of a database, a workbook or a table file
//! are shown without it (data_lens.rs, sheet.rs).
//!
//! One statement at a time, and only one that reads (duck.rs locks the session down). A newer query stops the one
//! running, and 1,000 rows come back. A workbook read stays in memory for the next query, until another one is.

use crate::data_lens::TableColumn;
use crate::duck::{self, Source};
use crate::fsx::blocking;
use crate::sheet::{self, Cell, Loaded};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

/// A workbook bigger than this isn't loaded.
const MAX_LOAD_BYTES: u64 = 256 << 20;
const MAX_LOAD_ROWS: usize = 1_000_000;
const DATABASES: &[&str] = &["sqlite", "sqlite3", "db", "db3", "s3db", "sl3"];

/// The workbook read (which file, as of when), kept for the next query.
struct Book {
    key: (String, Option<SystemTime>, u64),
    sheets: Arc<Vec<Loaded>>,
}

static BOOK: Mutex<Option<Book>> = Mutex::new(None);
/// The query running, for a newer one to stop.
static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

pub struct SqlTables {
    pub tables: Vec<String>,
    /// Who answers: always "DuckDB" now.
    pub engine: &'static str,
    /// Loading the file took this long (0 when it was loaded already, or needed no loading).
    pub load_ms: f64,
}

pub struct SqlAnswer {
    pub engine: &'static str,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<Vec<String>>,
    /// There were more rows than came back.
    pub truncated: bool,
    pub elapsed_ms: f64,
    pub load_ms: f64,
}

fn key_of(path: &str) -> Result<(String, Option<SystemTime>, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    Ok((path.to_lowercase(), meta.modified().ok(), meta.len()))
}

/// What a query over one file reads, and what has to be cleaned up afterwards: the folder a workbook's sheets were
/// written to, deleted when this goes.
struct Prepared {
    source: Source,
    folder: Option<PathBuf>,
    load_ms: f64,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(folder) = &self.folder {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// How a file is read for SQL, by its extension.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    /// DuckDB reads it where it is.
    Data,
    Database,
    Workbook,
}

fn kind_of_file(path: &str) -> Kind {
    let ext = sheet::extension(path);
    if DATABASES.contains(&ext.as_str()) {
        Kind::Database
    } else if duck::READS.contains(&ext.as_str()) {
        Kind::Data
    } else {
        Kind::Workbook
    }
}

/// `path` made ready to query. DuckDB must be there (the caller checked); nothing here runs it.
fn prepare(path: &str) -> Result<Prepared, String> {
    match kind_of_file(path) {
        Kind::Database => Ok(Prepared { source: Source::Sqlite(path.to_string()), folder: None, load_ms: 0.0 }),
        Kind::Data => Ok(Prepared { source: Source::Files(vec![path.to_string()]), folder: None, load_ms: 0.0 }),
        Kind::Workbook => {
            let started = Instant::now();
            let (sheets, loaded_now) = workbook(path)?;
            let folder = scratch_folder()?;
            // The folder is the guard's from here on: whatever goes wrong next, it is deleted.
            let mut prepared = Prepared { source: Source::Sheets(Vec::new()), folder: Some(folder.clone()), load_ms: 0.0 };
            let names = distinct(&sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>());
            let mut files = Vec::new();
            for (n, (sheet, name)) in sheets.iter().zip(names).enumerate() {
                let file = folder.join(format!("sheet-{n}.csv"));
                std::fs::write(&file, csv_of(sheet)).map_err(|e| e.to_string())?;
                files.push((name, file.to_string_lossy().into_owned()));
            }
            prepared.source = Source::Sheets(files);
            prepared.load_ms = if loaded_now { started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
            Ok(prepared)
        }
    }
}

/// The workbook at `path`, read unless it's the one kept; and whether it was read just now.
fn workbook(path: &str) -> Result<(Arc<Vec<Loaded>>, bool), String> {
    let key = key_of(path)?;
    let mut slot = BOOK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(book) = slot.as_ref().filter(|b| b.key == key) {
        return Ok((book.sheets.clone(), false));
    }
    *slot = None;
    if key.2 > MAX_LOAD_BYTES {
        return Err(format!("This file is over {} MB, too big to load for SQL.", MAX_LOAD_BYTES >> 20));
    }
    let sheets = Arc::new(sheet::load_workbook(path, MAX_LOAD_ROWS)?);
    *slot = Some(Book { key, sheets: sheets.clone() });
    Ok((sheets, true))
}

/// A new folder in the temp folder for one query's copies of a workbook's sheets.
fn scratch_folder() -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let folder = std::env::temp_dir().join(format!("glint-sql-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    Ok(folder)
}

/// Names made distinct (a second "id" is "id_2") and never empty.
fn distinct(names: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let base = if name.trim().is_empty() { format!("col_{}", i + 1) } else { name.trim().to_string() };
            let mut candidate = base.clone();
            let mut n = 2;
            while !seen.insert(candidate.to_lowercase()) {
                candidate = format!("{base}_{n}");
                n += 1;
            }
            candidate
        })
        .collect()
}

/// One field of a CSV file: in double quotes when it holds a comma, a quote, a line break or spaces at either end.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) || text.starts_with(' ') || text.ends_with(' ') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// A sheet as a CSV file: its column names (made distinct) and then its rows, numbers as they are.
fn csv_of(sheet: &Loaded) -> String {
    let columns = distinct(&sheet.columns);
    let mut out = columns.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for row in &sheet.rows {
        let line = (0..columns.len())
            .map(|c| match row.get(c) {
                None | Some(Cell::Empty) => String::new(),
                Some(Cell::Text(s)) => csv_field(s),
                Some(Cell::Int(i)) => i.to_string(),
                Some(Cell::Float(f)) => f.to_string(),
                Some(Cell::Bool(b)) => b.to_string(),
            })
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The tables there are to query in a file, loading it.
pub async fn sql_tables(path: String) -> Result<SqlTables, String> {
    blocking(move || {
        duck::available()?;
        let prepared = prepare(&path)?;
        let mut found = duck::tables_of(&prepared.source)?;
        found.load_ms = found.load_ms.max(prepared.load_ms);
        Ok(found)
    })
    .await
}

/// Runs one query that reads over a file (loading it first if it isn't), giving its first 1,000 rows.
pub async fn sql_run(path: String, sql: String) -> Result<SqlAnswer, String> {
    // A newer query stops the one running.
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(older) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).replace(stop.clone()) {
        older.store(true, Ordering::SeqCst);
    }
    blocking(move || {
        let answer = run(&path, &sql, &stop);
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.as_ref().is_some_and(|ours| Arc::ptr_eq(ours, &stop)) {
            *running = None;
        }
        answer
    })
    .await
}

/// One query over a file, outside the preview's session (the MCP server's query_data).
pub(crate) fn query_file(path: &str, sql: &str) -> Result<SqlAnswer, String> {
    run(path, sql, &AtomicBool::new(false))
}

fn run(path: &str, sql: &str, stop: &AtomicBool) -> Result<SqlAnswer, String> {
    if sql.trim().trim_end_matches(';').trim().is_empty() {
        return Err("Type a query, such as SELECT * FROM data".into());
    }
    duck::available()?;
    let prepared = prepare(path)?;
    duck::query_over(&prepared.source, sql, stop)
        .map(|answer| SqlAnswer { load_ms: prepared.load_ms, ..answer })
        .map_err(|e| if e == "cancelled" { "The query was stopped for a newer one.".to_string() } else { e })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(columns: &[&str], rows: Vec<Vec<Cell>>) -> Loaded {
        Loaded { name: "Sheet1".into(), columns: columns.iter().map(|c| c.to_string()).collect(), rows, first: 1, header: true }
    }

    #[test]
    fn a_file_is_read_by_what_its_extension_says() {
        assert_eq!(kind_of_file(r"C:\d\people.DB"), Kind::Database);
        assert_eq!(kind_of_file(r"C:\d\a.sqlite3"), Kind::Database);
        for data in ["a.csv", "a.tsv", "a.jsonl", "a.parquet", "a.TXT"] {
            assert_eq!(kind_of_file(data), Kind::Data, "{data}");
        }
        for book in ["a.xlsx", "a.xls", "a.ods"] {
            assert_eq!(kind_of_file(book), Kind::Workbook, "{book}");
        }
        assert_eq!(distinct(&["id".into(), "".into(), "ID".into(), "id".into()]), ["id", "col_2", "ID_2", "id_3"], "as SQL compares names: without case");
    }

    #[test]
    fn a_sheet_is_written_as_csv_with_quotes_where_they_are_needed() {
        let sheet = loaded(
            &["name", "amount", "name", "note"],
            vec![
                vec![Cell::Text("Ada, the first".into()), Cell::Float(2.5), Cell::Bool(true), Cell::Text("say \"hi\"\nthere".into())],
                vec![Cell::Text(" padded ".into()), Cell::Int(7), Cell::Empty],
            ],
        );
        assert_eq!(csv_of(&sheet), "name,amount,name_2,note\n\"Ada, the first\",2.5,true,\"say \"\"hi\"\"\nthere\"\n\" padded \",7,,\n");
    }

    #[test]
    fn a_workbook_query_leaves_no_copy_of_the_sheets_behind() {
        let folder = scratch_folder().unwrap();
        std::fs::write(folder.join("sheet-0.csv"), "a\n1\n").unwrap();
        let other = scratch_folder().unwrap();
        assert_ne!(folder, other, "each query has a folder of its own");
        let prepared = Prepared { source: Source::Sheets(Vec::new()), folder: Some(folder.clone()), load_ms: 0.0 };
        assert!(folder.join("sheet-0.csv").exists());
        drop(prepared);
        assert!(!folder.exists(), "the copies went with the query");
        let _ = std::fs::remove_dir_all(other);
    }

    #[test]
    fn an_empty_query_is_asked_for_before_anything_starts() {
        assert_eq!(run(r"C:\d\a.csv", "  ; ", &AtomicBool::new(false)).err().as_deref(), Some("Type a query, such as SELECT * FROM data"));
    }
}
