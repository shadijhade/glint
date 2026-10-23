//! SQL in the preview, reading only: a SQLite database's own tables, opened read-only and immutable
//! (nothing is locked, journaled or written), or a table file loaded into an in-memory SQLite: a CSV,
//! TSV or JSON-lines file or a Parquet file as `data`, a workbook's sheets by their names (the first one
//! as `data` too). One statement at a time, and only one that reads: SQLite's authorizer allows reading
//! and functions (and a few pragmas that describe tables), nothing else, so no ATTACH, no VACUUM INTO.
//! A query is interrupted after 10 seconds or when a newer one starts, SQLite holds at most 1 GB, and
//! 1,000 rows come back. The file loaded stays loaded for the next query, until another one is.

use crate::data_lens::TableColumn;
use crate::fsx::blocking;
use crate::sheet::{self, Cell, Kind, Loaded};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, InterruptHandle, OpenFlags};
use serde::Serialize;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant, SystemTime};

/// A table file bigger than this isn't loaded.
const MAX_LOAD_BYTES: u64 = 256 << 20;
const MAX_LOAD_ROWS: usize = 1_000_000;
const MAX_BACK: usize = 1000;
const DEADLINE: Duration = Duration::from_secs(10);
/// What SQLite may allocate in all (it's only used here).
const HEAP: i64 = 1 << 30;
const DATABASES: &[&str] = &["sqlite", "sqlite3", "db", "db3", "s3db", "sl3"];

/// The file loaded (or database open): which, as of when, its connection and its tables.
struct Session {
    key: (String, Option<SystemTime>, u64),
    conn: Connection,
    tables: Vec<String>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// The query running, for a newer one to interrupt.
static RUNNING: Mutex<Option<InterruptHandle>> = Mutex::new(None);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlTables {
    pub tables: Vec<String>,
    /// Loading the file took this long (0 when it was loaded already, or is a database).
    pub load_ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlAnswer {
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

/// The session for `path`, loading it unless it's the one loaded; and how long loading took.
fn session<'a>(slot: &'a mut Option<Session>, path: &str) -> Result<(&'a Session, f64), String> {
    let key = key_of(path)?;
    let mut load_ms = 0.0;
    if slot.as_ref().is_none_or(|s| s.key != key) {
        *slot = None;
        let started = Instant::now();
        let (conn, tables) = open(path)?;
        guard(&conn)?;
        load_ms = started.elapsed().as_secs_f64() * 1000.0;
        *slot = Some(Session { key, conn, tables });
    }
    Ok((slot.as_ref().expect("set above"), load_ms))
}

/// The tables there are to query in a file, loading it.
#[tauri::command]
pub async fn sql_tables(path: String) -> Result<SqlTables, String> {
    blocking(move || {
        let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        let (session, load_ms) = session(&mut slot, &path)?;
        Ok(SqlTables { tables: session.tables.clone(), load_ms })
    })
    .await
}

/// Runs one query that reads over a file (loading it first if it isn't), giving its first 1,000 rows.
#[tauri::command]
pub async fn sql_run(path: String, sql: String) -> Result<SqlAnswer, String> {
    // A newer query stops the one running.
    if let Some(running) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        running.interrupt();
    }
    blocking(move || {
        let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        let (session, load_ms) = session(&mut slot, &path)?;
        *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.conn.get_interrupt_handle());
        let answer = run(&session.conn, &sql);
        *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        answer.map(|answer| SqlAnswer { load_ms, ..answer })
    })
    .await
}

/// A database file's URI, read-only and immutable: `file:///C:/a%20b/x.db?mode=ro&immutable=1`.
fn database_uri(path: &str) -> String {
    let mut out = String::from("file:///");
    for byte in path.replace('\\', "/").bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out + "?mode=ro&immutable=1"
}

fn open(path: &str) -> Result<(Connection, Vec<String>), String> {
    static HEAP_LIMIT: Once = Once::new();
    // SQLite's own allocations, process-wide: nothing else in Glint uses it.
    HEAP_LIMIT.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_hard_heap_limit64(HEAP);
    });
    let ext = sheet::extension(path);
    if DATABASES.contains(&ext.as_str()) {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        // A share's path isn't a URI SQLite takes; read-only is enough there.
        let conn = if path.starts_with("\\\\") { Connection::open_with_flags(path, flags) } else { Connection::open_with_flags(database_uri(path), flags | OpenFlags::SQLITE_OPEN_URI) };
        let conn = conn.map_err(|e| format!("This database can't be opened: {e}"))?;
        let tables = conn
            .prepare("SELECT name FROM sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type = 'view', name")
            .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("This database can't be read: {e}"))?;
        return Ok((conn, tables));
    }
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_LOAD_BYTES {
        return Err(format!("This file is over {} MB, too big to load for SQL.", MAX_LOAD_BYTES >> 20));
    }
    let tables = match ext.as_str() {
        "csv" | "tsv" | "tab" | "psv" | "txt" => vec![delimited(path, &ext)?],
        "jsonl" | "ndjson" => vec![json_lines(path)?],
        "parquet" => vec![sheet::load_parquet(path, "data", MAX_LOAD_ROWS)?],
        _ => sheet::load_workbook(path, MAX_LOAD_ROWS)?,
    };
    load(tables)
}

/// Lets a statement only read: tables, functions, recursion, and the pragmas that describe tables.
fn guard(conn: &Connection) -> Result<(), String> {
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true).map_err(|e| e.to_string())?;
    conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Function { .. } | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Pragma { pragma_name, .. }
            if ["table_info", "table_xinfo", "table_list", "index_list", "index_info", "index_xinfo", "foreign_key_list"].contains(&pragma_name.to_ascii_lowercase().as_str()) =>
        {
            Authorization::Allow
        }
        _ => Authorization::Deny,
    }))
    .map_err(|e| e.to_string())
}

/// A name SQL takes as one, in double quotes.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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

/// Tables into an in-memory database: number columns NUMERIC, the rest TEXT (booleans as 1 and 0).
fn load(tables: Vec<Loaded>) -> Result<(Connection, Vec<String>), String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    let names = distinct(&tables.iter().map(|t| t.name.clone()).collect::<Vec<_>>());
    let too_big = |e: rusqlite::Error| {
        if e.sqlite_error_code() == Some(rusqlite::ErrorCode::OutOfMemory) {
            "This file needs more memory than SQL in a preview has (1 GB).".to_string()
        } else {
            e.to_string()
        }
    };
    for (table, name) in tables.iter().zip(&names) {
        let columns = distinct(&table.columns);
        let kinds: Vec<Kind> = (0..columns.len()).map(|c| sheet::kind_of(table.rows.iter().filter_map(|r| r.get(c)))).collect();
        let definition = columns.iter().zip(&kinds).map(|(c, k)| format!("{} {}", quoted(c), if *k == Kind::Number { "NUMERIC" } else { "TEXT" })).collect::<Vec<_>>().join(", ");
        conn.execute(&format!("CREATE TABLE {} ({definition})", quoted(name)), []).map_err(too_big)?;
        let insert = format!("INSERT INTO {} VALUES ({})", quoted(name), vec!["?"; columns.len()].join(", "));
        conn.execute("BEGIN", []).map_err(too_big)?;
        {
            let mut statement = conn.prepare(&insert).map_err(too_big)?;
            for row in &table.rows {
                let values = (0..columns.len()).map(|c| match row.get(c) {
                    None | Some(Cell::Empty) => Value::Null,
                    Some(Cell::Text(s)) => Value::Text(s.clone()),
                    Some(Cell::Int(i)) => Value::Integer(*i),
                    Some(Cell::Float(f)) => Value::Real(*f),
                    Some(Cell::Bool(b)) => Value::Integer(*b as i64),
                });
                statement.execute(rusqlite::params_from_iter(values)).map_err(too_big)?;
            }
        }
        conn.execute("COMMIT", []).map_err(too_big)?;
    }
    if names.first().is_some_and(|first| first != "data") && !names.iter().any(|n| n.eq_ignore_ascii_case("data")) {
        conn.execute(&format!("CREATE VIEW data AS SELECT * FROM {}", quoted(&names[0])), []).map_err(|e| e.to_string())?;
    }
    Ok((conn, names))
}

/// A table file's text: UTF-8 (or UTF-16 by its mark), without a byte order mark.
fn text_of(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|&p| u16::from_le_bytes(p)).collect::<Vec<_>>()),
        [0xFE, 0xFF, rest @ ..] => String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|&p| u16::from_be_bytes(p)).collect::<Vec<_>>()),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// A CSV-like file's records: a quoted field may hold the delimiter, doubled quotes and line breaks.
fn records(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let (mut record, mut field) = (Vec::new(), String::new());
    let (mut quoted, mut was_quoted) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.trim().is_empty() {
            field.clear();
            quoted = true;
            was_quoted = true;
        } else if c == delim {
            record.push(if was_quoted { std::mem::take(&mut field) } else { std::mem::take(&mut field).trim().to_string() });
            was_quoted = false;
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            record.push(if was_quoted { std::mem::take(&mut field) } else { std::mem::take(&mut field).trim().to_string() });
            was_quoted = false;
            if record.iter().any(|f| !f.is_empty()) || record.len() > 1 {
                records.push(std::mem::take(&mut record));
            } else {
                record.clear();
            }
            if records.len() > MAX_LOAD_ROWS {
                return records;
            }
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(if was_quoted { field } else { field.trim().to_string() });
        records.push(record);
    }
    records
}

/// Text as the number it says, if it says one.
fn numeric(text: &str) -> Option<Cell> {
    let t = text.trim();
    if t.is_empty() || t.starts_with('+') || (t.len() > 1 && t.starts_with('0') && !t.starts_with("0.")) {
        return None;
    }
    t.parse::<i64>().map(Cell::Int).ok().or_else(|| t.parse::<f64>().ok().filter(|f| f.is_finite()).map(Cell::Float))
}

/// A CSV, TSV or other delimited file as `data`, its first record the header; a column that's all
/// numbers is numbers.
fn delimited(path: &str, ext: &str) -> Result<Loaded, String> {
    let text = text_of(path)?;
    let first = text.lines().next().unwrap_or("");
    let delim = match ext {
        "tsv" | "tab" => '\t',
        "psv" => '|',
        _ if first.contains('\t') => '\t',
        _ => [',', ';', '|'].into_iter().max_by_key(|&d| first.matches(d).count()).unwrap_or(','),
    };
    let mut records = records(&text, delim).into_iter();
    let columns = records.next().ok_or("This file is empty.")?;
    let width = columns.len().max(1);
    let mut rows: Vec<Vec<Cell>> = records
        .take(MAX_LOAD_ROWS)
        .map(|r| {
            let mut row: Vec<Cell> = r.into_iter().take(width).map(|f| if f.is_empty() { Cell::Empty } else { Cell::Text(f) }).collect();
            row.resize(width, Cell::Empty);
            row
        })
        .collect();
    for c in 0..width {
        let all_numbers = rows.iter().all(|r| match &r[c] {
            Cell::Text(t) => numeric(t).is_some(),
            _ => true,
        });
        if all_numbers {
            for row in &mut rows {
                if let Cell::Text(t) = &row[c] {
                    row[c] = numeric(t).unwrap_or(Cell::Empty);
                }
            }
        }
    }
    Ok(Loaded { name: "data".into(), columns, rows })
}

/// A JSON-lines file as `data`: each line an object, its keys the columns (in the order first seen).
fn json_lines(path: &str) -> Result<Loaded, String> {
    let text = text_of(path)?;
    let mut columns: Vec<String> = Vec::new();
    let mut objects = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(MAX_LOAD_ROWS) {
        let Ok(serde_json::Value::Object(object)) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        for key in object.keys() {
            if columns.len() < 200 && !columns.contains(key) {
                columns.push(key.clone());
            }
        }
        objects.push(object);
    }
    if columns.is_empty() {
        return Err("No JSON objects in this file.".into());
    }
    let rows = objects
        .iter()
        .map(|o| {
            columns
                .iter()
                .map(|k| match o.get(k) {
                    None | Some(serde_json::Value::Null) => Cell::Empty,
                    Some(serde_json::Value::Bool(b)) => Cell::Bool(*b),
                    Some(serde_json::Value::Number(n)) => n.as_i64().map(Cell::Int).unwrap_or_else(|| Cell::Float(n.as_f64().unwrap_or(f64::NAN))),
                    Some(serde_json::Value::String(s)) => Cell::Text(sheet::cut(s)),
                    Some(other) => Cell::Text(sheet::cut(&other.to_string())),
                })
                .collect()
        })
        .collect();
    Ok(Loaded { name: "data".into(), columns, rows })
}

/// Runs one statement that only reads, giving its first 1,000 rows.
fn run(conn: &Connection, sql: &str) -> Result<SqlAnswer, String> {
    let started = Instant::now();
    let sql = sql.trim().trim_end_matches(';').trim();
    if sql.is_empty() {
        return Err("Type a query, such as SELECT * FROM data".into());
    }
    let explain = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::OperationInterrupted) => format!("The query was stopped (after {} seconds, or for a newer one).", DEADLINE.as_secs()),
        Some(rusqlite::ErrorCode::AuthorizationForStatementDenied) => "Only queries that read run here: SELECT, WITH and the like.".to_string(),
        Some(rusqlite::ErrorCode::OutOfMemory) => "The query needed more memory than SQL in a preview has (1 GB).".to_string(),
        _ => match e {
            rusqlite::Error::MultipleStatement => "One statement at a time.".to_string(),
            rusqlite::Error::SqliteFailure(_, Some(message)) => message,
            other => other.to_string(),
        },
    };
    let mut statement = conn.prepare(sql).map_err(explain)?;
    if !statement.readonly() {
        return Err("Only queries that read run here: SELECT, WITH and the like.".into());
    }
    let deadline = started + DEADLINE;
    conn.progress_handler(1000, Some(move || Instant::now() > deadline)).map_err(|e| e.to_string())?;
    let names: Vec<String> = statement.column_names().into_iter().map(String::from).collect();
    let mut kinds = vec![Kind::Empty; names.len()];
    let mut rows = Vec::new();
    let mut truncated = false;
    let read = (|| -> Result<(), rusqlite::Error> {
        let mut found = statement.query([])?;
        while let Some(row) = found.next()? {
            if rows.len() == MAX_BACK {
                truncated = true;
                break;
            }
            let mut line = Vec::with_capacity(names.len());
            for (c, kind) in kinds.iter_mut().enumerate() {
                let (text, k) = match row.get_ref(c)? {
                    ValueRef::Null => (String::new(), Kind::Empty),
                    ValueRef::Integer(i) => (i.to_string(), Kind::Number),
                    ValueRef::Real(f) => (sheet::number(f), Kind::Number),
                    ValueRef::Text(t) => (sheet::cut(&String::from_utf8_lossy(t)), Kind::Text),
                    ValueRef::Blob(b) => (format!("({} bytes)", b.len()), Kind::Text),
                };
                *kind = match (*kind, k) {
                    (seen, Kind::Empty) => seen,
                    (Kind::Empty, k) => k,
                    (seen, k) if seen == k => seen,
                    _ => Kind::Text,
                };
                line.push(text);
            }
            rows.push(line);
        }
        Ok(())
    })();
    let _ = conn.progress_handler(0, None::<fn() -> bool>);
    read.map_err(explain)?;
    Ok(SqlAnswer {
        columns: names.into_iter().zip(&kinds).map(|(name, k)| TableColumn { name, kind: sheet::kind_name(*k).into() }).collect(),
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        load_ms: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, text: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("glint-sql-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        (dir, path.to_string_lossy().into())
    }

    #[test]
    fn reads_csv_records_with_quotes_and_line_breaks() {
        assert_eq!(records("a,b\n1,\"x, \"\"y\"\"\nz\"\n\n2, w \r\n", ','), [vec!["a", "b"], vec!["1", "x, \"y\"\nz"], vec!["2", "w"]]);
        assert_eq!(numeric("12"), Some(Cell::Int(12)));
        assert_eq!(numeric("-1.5"), Some(Cell::Float(-1.5)));
        assert_eq!(numeric("007"), None, "a code with leading zeros stays text");
        assert_eq!(numeric("inf"), None);
        assert_eq!(distinct(&["id".into(), "".into(), "ID".into(), "id".into()]), ["id", "col_2", "ID_2", "id_3"], "as SQL compares names: without case");
        assert_eq!(database_uri("C:\\My Data\\x#1.db"), "file:///C:/My%20Data/x%231.db?mode=ro&immutable=1");
    }

    #[test]
    fn queries_a_csv_file_loaded_into_memory() {
        let (dir, path) = temp("sales.csv", "region,amount,code\nNorth,10,007\nSouth,2.5,010\nNorth,7,011\n");
        let (conn, tables) = open(&path).unwrap();
        guard(&conn).unwrap();
        assert_eq!(tables, ["data"]);
        let answer = run(&conn, "SELECT region, sum(amount) AS total FROM data GROUP BY region ORDER BY total DESC;").unwrap();
        assert_eq!(answer.rows, [["North", "17"], ["South", "2.5"]]);
        assert_eq!(answer.columns.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), ["text", "number"]);
        assert_eq!(run(&conn, "SELECT code FROM data").unwrap().rows[0], ["007"]);
        // Nothing but reading.
        for refused in ["DELETE FROM data", "CREATE TABLE x(a)", "ATTACH DATABASE 'x.db' AS x", "VACUUM INTO 'x.db'", "PRAGMA writable_schema = ON", "SELECT 1; SELECT 2"] {
            assert!(run(&conn, refused).is_err(), "{refused} ran");
        }
        assert!(!std::path::Path::new("x.db").exists());
        assert!(run(&conn, "PRAGMA table_info(data)").unwrap().rows.len() == 3);
        let many = run(&conn, "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i FROM n").unwrap();
        assert_eq!((many.rows.len(), many.truncated), (MAX_BACK, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stops_a_query_that_never_ends() {
        let conn = Connection::open_in_memory().unwrap();
        guard(&conn).unwrap();
        let handle = conn.get_interrupt_handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            handle.interrupt();
        });
        let started = Instant::now();
        let e = run(&conn, "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT count(*) FROM n").err().unwrap();
        assert!(e.contains("stopped"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn opens_a_database_without_writing_to_it() {
        let (dir, path) = temp("t.db", "");
        std::fs::remove_file(&path).unwrap();
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE people(name TEXT, age INTEGER); INSERT INTO people VALUES ('Ada', 36), ('Alan', 41);").unwrap();
        }
        let before: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        let (conn, tables) = open(&path).unwrap();
        guard(&conn).unwrap();
        assert_eq!(tables, ["people"]);
        assert_eq!(run(&conn, "SELECT name FROM people WHERE age > 40").unwrap().rows, [["Alan"]]);
        assert!(run(&conn, "INSERT INTO people VALUES ('x', 1)").is_err());
        drop(conn);
        let after: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(before, after, "no journal, WAL or lock file appeared");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
