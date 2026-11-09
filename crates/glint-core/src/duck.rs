//! DuckDB (MIT, a tool Glint installs) for data files of any size: once it's there, Query reads CSV, TSV,
//! JSON and Parquet files where they are, however big, instead of loading them into SQLite (sql.rs, which
//! stops at 256 MB); several files are queried together, each a table named after it; Profile describes
//! each column (DuckDB's SUMMARIZE); and Convert data writes CSV, Parquet (Zstandard) or JSON lines.
//!
//! Reading only, as sql.rs: what's typed runs inside DuckDB's `query()`, which takes a single SELECT and
//! nothing else, after the session has been locked down: no extensions downloaded or loaded, no files but
//! the ones chosen (`allowed_paths`, then `enable_external_access = false`), and the configuration locked.
//! The whole script is one `-c` argument, so none of the CLI's dot commands can run. Nothing is sent anywhere.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::data_lens::TableColumn;
use crate::fsx::blocking;
use crate::sql::{SqlAnswer, SqlTables};
use serde::{Deserialize, Serialize};
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

/// Whether DuckDB is there to read `path` (else sql.rs loads it).
pub fn takes(path: &str) -> bool {
    READS.contains(&extension(path).as_str()) && crate::tools::command("duckdb").is_ok()
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

/// The locked-down session over `paths` (each a view by its table name, the first also `data`), then `body`.
fn script(paths: &[String], body: &str) -> String {
    let allowed = paths.iter().map(|p| literal(p)).collect::<Vec<_>>().join(", ");
    let mut out = format!(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; SET allowed_paths = [{allowed}]; SET enable_external_access = false; SET lock_configuration = true; "
    );
    for (path, name) in paths.iter().zip(table_names(paths)) {
        out.push_str(&format!("CREATE VIEW {} AS SELECT * FROM {}; ", ident(&name), reader(path)));
    }
    if let Some(first) = paths.first() {
        out.push_str(&format!("CREATE VIEW data AS SELECT * FROM {}; ", reader(first)));
    }
    out.push_str(body);
    out
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

/// Runs `sql` (one SELECT) over `paths`: its columns, typed, and its first 1,000 rows.
pub fn query(paths: &[String], sql: &str) -> Result<SqlAnswer, String> {
    let started = Instant::now();
    let inner = literal(sql.trim().trim_end_matches(';'));
    let body = format!("DESCRIBE SELECT * FROM query({inner}); SELECT * FROM query({inner}) LIMIT {};", MAX_BACK + 1);
    let results = run_script(&script(paths, &body), &AtomicBool::new(false))?;
    let described = results.first().and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let columns: Vec<TableColumn> = described
        .iter()
        .map(|c| TableColumn { name: c["column_name"].as_str().unwrap_or_default().to_string(), kind: kind(c["column_type"].as_str().unwrap_or_default()).to_string() })
        .collect();
    let rows_json = results.get(1).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let truncated = rows_json.len() > MAX_BACK;
    let rows = rows_json.iter().take(MAX_BACK).map(|row| columns.iter().map(|c| cell(&row[&c.name])).collect()).collect();
    Ok(SqlAnswer { engine: "DuckDB", columns, rows, truncated, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0, load_ms: 0.0 })
}

pub async fn duck_tables(paths: Vec<String>) -> Result<SqlTables, String> {
    Ok(tables(&paths))
}

pub async fn duck_run(paths: Vec<String>, sql: String) -> Result<SqlAnswer, String> {
    blocking(move || query(&paths, &sql)).await
}

/// A column described: its type, how many values are missing, distinct (about), and the spread of numbers.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
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

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
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

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
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
