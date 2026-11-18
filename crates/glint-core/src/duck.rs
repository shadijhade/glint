//! DuckDB (MIT, a tool Glint installs) answers every SQL query: Query reads CSV, TSV, JSON and Parquet files where
//! they are, however big; a SQLite database is attached read-only through DuckDB's `sqlite` extension; a workbook's
//! sheets are read from copies Glint writes for the query (sql.rs); several files are queried together, each a table
//! named after it; Profile describes each column (DuckDB's SUMMARIZE); and Convert data writes CSV, Parquet
//! (Zstandard) or JSON lines.
//!
//! Reading only: what's typed runs inside DuckDB's `query()`, which takes a single SELECT and nothing else, after
//! the session has been locked down: no extensions downloaded or loaded, no files but the ones chosen
//! (`allowed_paths`, then `enable_external_access = false`), and the configuration locked. The whole script is one
//! `-c` argument, so none of the CLI's dot commands can run. Nothing of yours is sent anywhere. The one download
//! besides the tool itself is DuckDB's own `sqlite` extension (a program from extensions.duckdb.org that DuckDB keeps
//! in its own folder), fetched the first time a SQLite file is queried, on its own before any query runs.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::data_lens::TableColumn;
use crate::fsx::blocking;
use crate::sql::{SqlAnswer, SqlTables};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use crate::host::ipc::Channel;
use crate::host::Window;

/// What DuckDB reads itself (its CSV, JSON and Parquet readers are built in).
pub const READS: &[&str] = &["csv", "tsv", "tab", "psv", "txt", "json", "jsonl", "ndjson", "parquet"];
/// Rows a query gives back.
const MAX_BACK: usize = 1000;

static STOP: AtomicBool = AtomicBool::new(false);

fn extension(path: &str) -> String {
    Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// A string as a SQL literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A name as a SQL identifier.
fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The table function that reads `path`, by its extension.
fn reader(path: &str) -> String {
    let at = literal(path);
    match extension(path).as_str() {
        "parquet" => format!("read_parquet({at})"),
        "json" | "jsonl" | "ndjson" => format!("read_json_auto({at})"),
        "tsv" | "tab" => format!("read_csv({at}, delim = '\\t')"),
        "psv" => format!("read_csv({at}, delim = '|')"),
        _ => format!("read_csv({at})"),
    }
}

/// Each file's table name: its name without the extension, made distinct ("sales", "sales_2").
fn table_names(paths: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    paths
        .iter()
        .map(|p| {
            let stem = Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let base: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>().trim_matches('_').to_string();
            let base = if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) { format!("t_{base}") } else { base };
            let mut name = base.clone();
            let mut n = 2;
            while !seen.insert(name.to_lowercase()) {
                name = format!("{base}_{n}");
                n += 1;
            }
            name
        })
        .collect()
}

/// What a query reads.
pub(crate) enum Source {
    /// Data files DuckDB reads itself, a table named after each (the first also `data`).
    Files(Vec<String>),
    /// A SQLite database, attached read-only through DuckDB's `sqlite` extension: its own tables.
    Sqlite(String),
    /// Tables DuckDB reads from CSV files Glint wrote for it (a workbook's sheets): each table's name and file.
    Sheets(Vec<(String, String)>),
}

/// The part of a script that ends the session's freedom to reach anything: no extensions fetched, no files
/// outside `allowed` (none at all once they are), and the configuration locked.
fn lock_down(allowed: &[String]) -> String {
    let allowed = allowed.iter().map(|p| literal(p)).collect::<Vec<_>>().join(", ");
    format!("SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; SET allowed_paths = [{allowed}]; SET enable_external_access = false; SET lock_configuration = true; ")
}

/// Locks the session down over `allowed`, then makes a view of each of `tables` (name and the expression that reads
/// it), the first also `data` unless one is called that.
fn views(allowed: &[String], tables: &[(String, String)]) -> String {
    let mut out = lock_down(allowed);
    for (name, from) in tables {
        out.push_str(&format!("CREATE VIEW {} AS SELECT * FROM {}; ", ident(name), from));
    }
    if let Some((_, first)) = tables.first().filter(|_| !tables.iter().any(|(name, _)| name.eq_ignore_ascii_case("data"))) {
        out.push_str(&format!("CREATE VIEW data AS SELECT * FROM {first}; "));
    }
    out
}

impl Source {
    /// Everything a script does before the query itself: the session locked down and its tables set up.
    fn setup(&self) -> String {
        match self {
            Source::Files(paths) => {
                let tables: Vec<(String, String)> = table_names(paths).into_iter().zip(paths.iter().map(|p| reader(p))).collect();
                views(paths, &tables)
            }
            // The extension is loaded and the file attached while the session may still reach them; what follows
            // takes the reach away, and the attached database goes on being read.
            Source::Sqlite(db) => format!("LOAD sqlite; ATTACH {} AS src (TYPE sqlite, READ_ONLY); USE src; {}", literal(db), lock_down(&[])),
            Source::Sheets(sheets) => {
                let tables: Vec<(String, String)> = sheets.iter().map(|(name, file)| (name.clone(), format!("read_csv({}, header = true)", literal(file)))).collect();
                let files: Vec<String> = sheets.iter().map(|(_, file)| file.clone()).collect();
                views(&files, &tables)
            }
        }
    }
}

/// The locked-down session over `paths` (each a view by its table name, the first also `data`), then `body`.
fn script(paths: &[String], body: &str) -> String {
    format!("{}{body}", Source::Files(paths.to_vec()).setup())
}

fn duckdb() -> Result<Command, String> {
    let mut command = crate::tools::command("duckdb")?;
    // DuckDB 2 changes its output for AI agents it detects; Glint isn't one.
    command.env_remove("AI_AGENT");
    command.arg("-json");
    Ok(command)
}

/// Runs a script in a fresh in-memory DuckDB: its JSON results, one array a statement that gave rows.
fn run_script(script: &str, stop: &AtomicBool) -> Result<Vec<serde_json::Value>, String> {
    let mut command = duckdb()?;
    command.arg("-c").arg(script);
    let said = actions::run_tool(command, "DuckDB", stop, |_| {}).map_err(|e| plain(&e))?;
    Ok(serde_json::Deserializer::from_str(&said.stdout).into_iter::<serde_json::Value>().filter_map(Result::ok).collect())
}

/// DuckDB's errors without what's only for programmers.
fn plain(said: &str) -> String {
    said.replace("Parser Error: Expected a single SELECT statement", "Only a query that reads (SELECT, one at a time) runs here").trim().to_string()
}

/// What sql.rs calls a column of this DuckDB type.
fn kind(duck_type: &str) -> &'static str {
    let t = duck_type.to_ascii_uppercase();
    if t == "BOOLEAN" {
        "boolean"
    } else if ["INT", "DOUBLE", "FLOAT", "DECIMAL", "REAL", "NUMERIC", "HUGEINT", "BIGINT", "SMALLINT", "TINYINT"].iter().any(|n| t.contains(n)) && !t.contains("[]") && !t.starts_with("STRUCT") && !t.starts_with("MAP") {
        "number"
    } else {
        "text"
    }
}

/// A value as the table view shows it.
fn cell(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The tables a query over `paths` has: each file's, and `data` for the first.
pub fn tables(paths: &[String]) -> SqlTables {
    let mut tables = vec!["data".to_string()];
    if paths.len() > 1 {
        tables = table_names(paths);
    }
    SqlTables { tables, engine: "DuckDB", load_ms: 0.0 }
}

/// DuckDB itself, or why it isn't there: "DuckDB isn't installed. Settings → Tools installs it."
pub(crate) fn available() -> Result<(), String> {
    duckdb().map(drop)
}

/// Makes sure DuckDB has its `sqlite` extension: it downloads it once, from extensions.duckdb.org, into its own
/// folder, and later runs find it there without going online. Done on its own, before the query's session is
/// locked, so that a failure to fetch it (offline, a proxy) is told as that and not as a SQL error.
fn ensure_sqlite_extension() -> Result<(), String> {
    static READY: AtomicBool = AtomicBool::new(false);
    if READY.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut command = duckdb()?;
    command.arg("-c").arg("INSTALL sqlite; LOAD sqlite;");
    actions::run_tool(command, "DuckDB", &AtomicBool::new(false), |_| {}).map_err(|said| sqlite_extension_failure(&said))?;
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// What it says when the `sqlite` extension couldn't be had, with DuckDB's own words for why on a line of their own.
fn sqlite_extension_failure(said: &str) -> String {
    let why = plain(said).lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim().to_string();
    format!(
        "DuckDB reads SQLite files with its SQLite extension, which it downloads once from extensions.duckdb.org the first time. It couldn't be fetched: check that this PC is online and try again.\n{why}"
    )
}

/// The tables of a SQLite database, tables first and then views, as DuckDB lists them.
fn sqlite_tables(db: &str) -> Result<SqlTables, String> {
    let started = Instant::now();
    ensure_sqlite_extension()?;
    let body = "SELECT name FROM (SELECT table_name AS name, 0 AS k FROM duckdb_tables() WHERE database_name = 'src' UNION ALL SELECT view_name, 1 FROM duckdb_views() WHERE database_name = 'src' AND NOT internal) WHERE NOT starts_with(name, 'sqlite_') ORDER BY k, name;";
    let results = run_script(&format!("{}{body}", Source::Sqlite(db.to_string()).setup()), &AtomicBool::new(false))?;
    let tables = results.iter().filter_map(|v| v.as_array()).flatten().filter_map(|row| row["name"].as_str().map(str::to_string)).collect();
    Ok(SqlTables { tables, engine: "DuckDB", load_ms: started.elapsed().as_secs_f64() * 1000.0 })
}

/// The tables there are to query in `source`.
pub(crate) fn tables_of(source: &Source) -> Result<SqlTables, String> {
    available()?;
    match source {
        Source::Files(paths) => Ok(tables(paths)),
        Source::Sqlite(db) => sqlite_tables(db),
        Source::Sheets(sheets) => Ok(SqlTables { tables: sheets.iter().map(|(name, _)| name.clone()).collect(), engine: "DuckDB", load_ms: 0.0 }),
    }
}

/// Runs `sql` (one SELECT) over `source`: its columns, typed, and its first 1,000 rows. `stop` ends it ("cancelled").
pub(crate) fn query_over(source: &Source, sql: &str, stop: &AtomicBool) -> Result<SqlAnswer, String> {
    let started = Instant::now();
    available()?;
    if matches!(source, Source::Sqlite(_)) {
        ensure_sqlite_extension()?;
    }
    let inner = literal(sql.trim().trim_end_matches(';'));
    let body = format!("DESCRIBE SELECT * FROM query({inner}); SELECT * FROM query({inner}) LIMIT {};", MAX_BACK + 1);
    let results = run_script(&format!("{}{body}", source.setup()), stop)?;
    // The description is the first result that describes columns; the rows (if the CLI prints any) follow it.
    let at = results.iter().position(|v| v.as_array().is_some_and(|rows| rows.first().is_some_and(|row| row.get("column_name").is_some() && row.get("column_type").is_some())));
    let described = at.and_then(|at| results[at].as_array()).cloned().unwrap_or_default();
    let columns: Vec<TableColumn> = described
        .iter()
        .map(|c| TableColumn { name: c["column_name"].as_str().unwrap_or_default().to_string(), kind: kind(c["column_type"].as_str().unwrap_or_default()).to_string() })
        .collect();
    let rows_json = at.and_then(|at| results.get(at + 1)).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let truncated = rows_json.len() > MAX_BACK;
    let rows = rows_json.iter().take(MAX_BACK).map(|row| columns.iter().map(|c| cell(&row[&c.name])).collect()).collect();
    Ok(SqlAnswer { engine: "DuckDB", columns, rows, truncated, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0, load_ms: 0.0 })
}

/// Runs `sql` (one SELECT) over `paths`: its columns, typed, and its first 1,000 rows.
pub fn query(paths: &[String], sql: &str) -> Result<SqlAnswer, String> {
    query_over(&Source::Files(paths.to_vec()), sql, &AtomicBool::new(false))
}

pub async fn duck_tables(paths: Vec<String>) -> Result<SqlTables, String> {
    Ok(tables(&paths))
}

pub async fn duck_run(paths: Vec<String>, sql: String) -> Result<SqlAnswer, String> {
    blocking(move || query(&paths, &sql)).await
}

/// A column described: its type, how many values are missing, distinct (about), and the spread of numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnProfile {
    pub name: String,
    pub kind: String,
    pub duck_type: String,
    pub count: u64,
    pub null_percent: f64,
    pub distinct: u64,
    pub min: String,
    pub max: String,
    pub mean: String,
    pub std: String,
    pub q25: String,
    pub q50: String,
    pub q75: String,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub rows: u64,
    pub columns: Vec<ColumnProfile>,
    pub elapsed_ms: f64,
}

/// Each column of the file at `path`, described from all of its rows.
pub async fn duck_profile(path: String) -> Result<Profile, String> {
    blocking(move || {
        let started = Instant::now();
        let results = run_script(&script(std::slice::from_ref(&path), "SUMMARIZE data;"), &AtomicBool::new(false))?;
        let rows = results.first().and_then(|v| v.as_array()).cloned().unwrap_or_default();
        Ok(read_profile(&rows, started.elapsed().as_secs_f64() * 1000.0))
    })
    .await
}

fn read_profile(rows: &[serde_json::Value], elapsed_ms: f64) -> Profile {
    let text = |v: &serde_json::Value| cell(v);
    let columns: Vec<ColumnProfile> = rows
        .iter()
        .map(|r| {
            let duck_type = r["column_type"].as_str().unwrap_or_default().to_string();
            ColumnProfile {
                name: r["column_name"].as_str().unwrap_or_default().to_string(),
                kind: kind(&duck_type).to_string(),
                duck_type,
                count: r["count"].as_u64().unwrap_or(0),
                null_percent: r["null_percentage"].as_str().and_then(|p| p.parse().ok()).or_else(|| r["null_percentage"].as_f64()).unwrap_or(0.0),
                distinct: r["approx_unique"].as_u64().unwrap_or(0),
                min: text(&r["min"]),
                max: text(&r["max"]),
                mean: short_number(&text(&r["avg"])),
                std: short_number(&text(&r["std"])),
                q25: text(&r["q25"]),
                q50: text(&r["q50"]),
                q75: text(&r["q75"]),
            }
        })
        .collect();
    Profile { rows: columns.first().map_or(0, |c| c.count), columns, elapsed_ms }
}

/// "35.333333333333336" as "35.33".
fn short_number(text: &str) -> String {
    match text.parse::<f64>() {
        Ok(n) if n.fract() != 0.0 => format!("{:.2}", n).trim_end_matches('0').trim_end_matches('.').to_string(),
        _ => text.to_string(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DataFormat {
    Csv,
    Parquet,
    Jsonl,
}

pub fn duck_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Writes each of `files` as `format`, through actions.rs (beside it, in a folder or in its place).
pub async fn duck_convert(window: Window, files: Vec<String>, format: DataFormat, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        crate::tools::command("duckdb")?;
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        actions::run(owner, "data", &jobs, &output, 2, &STOP, &|p| drop(on_progress.send(p)), |job, _, work| convert_one(&job.inputs[0], format, work).map(|f| vec![f]))
    })
    .await
}

fn convert_one(file: &Path, format: DataFormat, work: &Path) -> Result<PathBuf, String> {
    let (ext, options) = match format {
        DataFormat::Csv => ("csv", "FORMAT csv, HEADER true"),
        DataFormat::Parquet => ("parquet", "FORMAT parquet, COMPRESSION zstd"),
        DataFormat::Jsonl => ("jsonl", "FORMAT json"),
    };
    let input = file.to_string_lossy().into_owned();
    if extension(&input) == ext {
        return Err(format!("It's {} already", ext.to_uppercase()));
    }
    let out = actions::renamed(work, file, ext);
    let out_text = out.to_string_lossy().into_owned();
    // Our own statement, not what was typed: the file read and the one written are the only ones allowed.
    let script = format!(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; SET allowed_paths = [{}, {}]; SET enable_external_access = false; SET lock_configuration = true; COPY (SELECT * FROM {}) TO {} ({options});",
        literal(&input),
        literal(&out_text),
        reader(&input),
        literal(&out_text)
    );
    let mut command = duckdb()?;
    command.arg("-c").arg(script);
    actions::run_tool(command, "DuckDB", &STOP, |_| {}).map_err(|e| plain(&e))?;
    if out.is_file() {
        Ok(out)
    } else {
        Err("DuckDB made nothing".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_the_session_down_before_anything_runs() {
        let paths = vec![r"C:\data\sales 2024.csv".to_string(), r"C:\data\it's.parquet".to_string()];
        let s = script(&paths, "SELECT 1;");
        let lock = s.find("lock_configuration = true").unwrap();
        assert!(s.find("enable_external_access = false").unwrap() < lock);
        assert!(s.find("CREATE VIEW").unwrap() > lock, "{s}");
        assert!(s.contains(r#"CREATE VIEW "sales_2024" AS SELECT * FROM read_csv('C:\data\sales 2024.csv');"#), "{s}");
        assert!(s.contains(r#"CREATE VIEW "it_s" AS SELECT * FROM read_parquet('C:\data\it''s.parquet');"#), "{s}");
        assert!(s.contains("allowed_paths = ['C:\\data\\sales 2024.csv', 'C:\\data\\it''s.parquet']"), "{s}");
        assert!(s.ends_with("SELECT 1;"));
    }

    #[test]
    fn a_sqlite_file_is_attached_read_only_before_the_session_is_locked() {
        let s = format!("{}SELECT 1;", Source::Sqlite(r"C:\my data\it's.db".into()).setup());
        let (load, attach, lock) = (s.find("LOAD sqlite;").unwrap(), s.find("ATTACH ").unwrap(), s.find("lock_configuration = true").unwrap());
        assert!(load < attach && attach < lock, "{s}");
        assert!(s.contains(r"ATTACH 'C:\my data\it''s.db' AS src (TYPE sqlite, READ_ONLY); USE src;"), "{s}");
        assert!(!s.contains("INSTALL"), "nothing is downloaded once a query's session starts: {s}");
        assert!(s.contains("enable_external_access = false") && s.contains("autoinstall_known_extensions = false"), "{s}");
        assert!(s.ends_with("SELECT 1;"));
    }

    #[test]
    fn a_workbooks_sheets_are_tables_named_for_them_and_the_first_is_data() {
        let sheets = vec![("Sales 2024".to_string(), r"C:\t\sheet-0.csv".to_string()), ("Notes".to_string(), r"C:\t\sheet-1.csv".to_string())];
        let s = Source::Sheets(sheets).setup();
        assert!(s.contains(r#"CREATE VIEW "Sales 2024" AS SELECT * FROM read_csv('C:\t\sheet-0.csv', header = true);"#), "{s}");
        assert!(s.contains(r#"CREATE VIEW "Notes" AS SELECT * FROM read_csv('C:\t\sheet-1.csv', header = true);"#), "{s}");
        assert!(s.contains("CREATE VIEW data AS SELECT * FROM read_csv('C:\\t\\sheet-0.csv', header = true);"), "{s}");
        assert!(s.contains(r"allowed_paths = ['C:\t\sheet-0.csv', 'C:\t\sheet-1.csv']"), "{s}");
        // A sheet called data is `data` itself.
        let own = Source::Sheets(vec![("Data".to_string(), r"C:\t\sheet-0.csv".to_string())]).setup();
        assert_eq!(own.matches("CREATE VIEW").count(), 1, "{own}");
    }

    #[test]
    fn a_sqlite_extension_that_cannot_be_fetched_is_told_as_that() {
        let said = sqlite_extension_failure("IO Error: Failed to download extension \"sqlite_scanner\" at URL \"http://extensions.duckdb.org/v2.0.0/windows_amd64/sqlite_scanner.duckdb_extension.gz\"
(HTTP error 0)");
        assert!(said.starts_with("DuckDB reads SQLite files with its SQLite extension, which it downloads once from extensions.duckdb.org"), "{said}");
        assert!(said.contains("check that this PC is online"), "{said}");
        assert!(said.lines().nth(1).is_some_and(|why| why.starts_with("IO Error: Failed to download extension")), "{said}");
    }

    #[test]
    fn names_tables_after_their_files() {
        let names = table_names(&[r"C:\a\sales.csv".into(), r"C:\b\sales.json".into(), r"C:\c\2024.csv".into(), r"C:\d\---.csv".into()]);
        assert_eq!(names, ["sales", "sales_2", "t_2024", "t_"]);
        assert_eq!(reader(r"C:\x\a.tsv"), r"read_csv('C:\x\a.tsv', delim = '\t')");
        assert_eq!(kind("DECIMAL(18,3)"), "number");
        assert_eq!(kind("INTEGER[]"), "text");
        assert_eq!(kind("BOOLEAN"), "boolean");
        assert_eq!(short_number("35.333333333333336"), "35.33");
        assert_eq!(short_number("41.0"), "41.0");
    }

    #[test]
    fn reads_a_profile() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(r#"[{"column_name":"age","column_type":"BIGINT","min":"29","max":"41","approx_unique":3,"avg":"35.333333333333336","std":"6.02","q25":"31","q50":"36","q75":"40","count":3,"null_percentage":"0.00"}]"#).unwrap();
        let p = read_profile(&rows, 1.0);
        assert_eq!(p.rows, 3);
        assert_eq!((p.columns[0].kind.as_str(), p.columns[0].mean.as_str(), p.columns[0].distinct), ("number", "35.33", 3));
    }
}
