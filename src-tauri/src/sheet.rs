//! Spreadsheets and Parquet files as the preview's tables, without Office: Excel's workbooks (.xlsx,
//! .xlsm, .xlsb, .xls) and OpenDocument's (.ods) through calamine, a sheet at a time, and Parquet
//! through the parquet crate, with its columns' types. The first 1,000 rows and 200 columns are read;
//! an .xlsx or .xlsb sheet is streamed, so a big one isn't read whole.

use crate::data_lens::{TableColumn, TablePreview};
use crate::fsx::blocking;
use calamine::{Data, DataRef, Dimensions, ExcelDateTime, Ods, Reader, SheetVisible, Xls, Xlsb, Xlsx};
use serde::Serialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Instant;

const MAX_ROWS: usize = 1000;
const MAX_COLS: usize = 200;
/// A cell's text past this many characters is cut.
const MAX_CELL: usize = 2000;
/// .xls and .ods sheets are read whole, so not past this size.
const MAX_WHOLE: u64 = 64 << 20;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetPreview {
    pub table: TablePreview,
    /// A workbook's sheets that aren't hidden, in order (none for Parquet), and the one shown.
    pub sheets: Vec<String>,
    pub sheet: usize,
    /// Parquet: each column's type ("INT64", "BYTE_ARRAY · String"…), in order.
    pub types: Vec<String>,
    /// Rows and columns in all: the sheet's used range, or the Parquet file's rows.
    pub total_rows: u64,
    pub total_cols: u64,
}

/// A spreadsheet's sheet (the first one not hidden, unless `sheet` says which), or a Parquet file, as a table.
#[tauri::command]
pub async fn inspect_sheet(path: String, sheet: Option<usize>) -> Result<SheetPreview, String> {
    blocking(move || {
        let started = Instant::now();
        let ext = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let mut preview = if ext == "parquet" { parquet(&path)? } else { workbook(&path, &ext, sheet)? };
        preview.table.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(preview)
    })
    .await
}

/// A cell, whichever way calamine hands it over.
enum Value<'a> {
    Empty,
    Text(&'a str),
    Iso(&'a str),
    Int(i64),
    Float(f64),
    Bool(bool),
    Date(ExcelDateTime),
    Error(String),
}

impl<'a> From<&'a DataRef<'a>> for Value<'a> {
    fn from(d: &'a DataRef<'a>) -> Self {
        match d {
            DataRef::Empty => Value::Empty,
            DataRef::String(s) => Value::Text(s),
            DataRef::DateTimeIso(s) | DataRef::DurationIso(s) => Value::Iso(s),
            DataRef::SharedString(s) => Value::Text(s),
            DataRef::Int(i) => Value::Int(*i),
            DataRef::Float(f) => Value::Float(*f),
            DataRef::Bool(b) => Value::Bool(*b),
            DataRef::DateTime(d) => Value::Date(*d),
            DataRef::Error(e) => Value::Error(e.to_string()),
        }
    }
}

impl<'a> From<&'a Data> for Value<'a> {
    fn from(d: &'a Data) -> Self {
        match d {
            Data::Empty => Value::Empty,
            Data::String(s) => Value::Text(s),
            Data::DateTimeIso(s) | Data::DurationIso(s) => Value::Iso(s),
            Data::Int(i) => Value::Int(*i),
            Data::Float(f) => Value::Float(*f),
            Data::Bool(b) => Value::Bool(*b),
            Data::DateTime(d) => Value::Date(*d),
            Data::Error(e) => Value::Error(e.to_string()),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Empty,
    Number,
    Boolean,
    Text,
}

/// A number as a sheet shows it: whole ones without a point, others to 10 places without trailing zeros.
fn number(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    if f != 0.0 && (f.abs() < 1e-6 || f.abs() >= 1e15) {
        return format!("{f:e}");
    }
    let fixed = format!("{f:.10}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A date as 2026-10-23, a time as 14:05:00, both when both are there; a duration as hours.
fn date(d: &ExcelDateTime) -> String {
    if d.is_duration() {
        let seconds = (d.as_f64() * 86400.0).round() as i64;
        return format!("{}:{:02}:{:02}", seconds / 3600, seconds % 3600 / 60, seconds % 60);
    }
    let (y, mo, da, h, mi, s, _) = d.to_ymd_hms_milli();
    let has_time = h != 0 || mi != 0 || s != 0;
    if d.as_f64() < 1.0 && has_time {
        format!("{h:02}:{mi:02}:{s:02}")
    } else if has_time {
        format!("{y:04}-{mo:02}-{da:02} {h:02}:{mi:02}:{s:02}")
    } else {
        format!("{y:04}-{mo:02}-{da:02}")
    }
}

fn cut(text: &str) -> String {
    match text.char_indices().nth(MAX_CELL) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

fn cell(value: Value) -> (String, Kind) {
    match value {
        Value::Empty => (String::new(), Kind::Empty),
        Value::Text(s) => (cut(s), if s.is_empty() { Kind::Empty } else { Kind::Text }),
        // 2026-01-02T00:00:00 as 2026-01-02, a time after the date with a space.
        Value::Iso(s) => (s.strip_suffix("T00:00:00").map(str::to_string).unwrap_or_else(|| s.replacen('T', " ", 1)), Kind::Text),
        Value::Int(i) => (i.to_string(), Kind::Number),
        Value::Float(f) => (number(f), Kind::Number),
        Value::Bool(b) => ((if b { "TRUE" } else { "FALSE" }).to_string(), Kind::Boolean),
        Value::Date(d) => (date(&d), Kind::Text),
        Value::Error(e) => (e, Kind::Text),
    }
}

/// "A", "Z", "AA"…: a column's letters, from its index.
fn letters(mut col: u32) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

/// Cells gathered into rows (relative to the sheet's used range), at most the first rows and columns.
struct Grid {
    start: (u32, u32),
    rows: Vec<Vec<(String, Kind)>>,
}

impl Grid {
    fn new(start: (u32, u32)) -> Self {
        Grid { start, rows: Vec::new() }
    }

    /// Takes a cell; false once it's past the rows kept (cells come row by row).
    fn put(&mut self, (row, col): (u32, u32), value: Value) -> bool {
        let (r, c) = ((row.saturating_sub(self.start.0)) as usize, (col.saturating_sub(self.start.1)) as usize);
        if r > MAX_ROWS {
            return false;
        }
        if c >= MAX_COLS {
            return true;
        }
        if self.rows.len() <= r {
            self.rows.resize_with(r + 1, Vec::new);
        }
        let line = &mut self.rows[r];
        if line.len() <= c {
            line.resize_with(c + 1, || (String::new(), Kind::Empty));
        }
        line[c] = cell(value);
        true
    }

    /// The table, and how many rows of data the sheet has (`total_rows` but its header): the first row
    /// is its header when it's all text, else the columns are named A, B, C…
    fn table(self, format: &str, total_rows: u64) -> (TablePreview, u64) {
        let width = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        let header = self.rows.first().is_some_and(|first| first.iter().any(|(_, k)| *k == Kind::Text) && first.iter().all(|(_, k)| matches!(k, Kind::Text | Kind::Empty)));
        let mut rows = self.rows.into_iter();
        let names: Vec<String> = match header.then(|| rows.next()).flatten() {
            Some(first) => (0..width).map(|c| first.get(c).map(|(t, _)| t.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| letters(self.start.1 + c as u32))).collect(),
            None => (0..width).map(|c| letters(self.start.1 + c as u32)).collect(),
        };
        let rows: Vec<Vec<(String, Kind)>> = rows.take(MAX_ROWS).collect();
        let columns = names
            .into_iter()
            .enumerate()
            .map(|(c, name)| {
                let kinds = rows.iter().filter_map(|r| r.get(c)).map(|(_, k)| *k).filter(|k| *k != Kind::Empty);
                let kind = kinds.fold(None, |seen, k| match seen {
                    None => Some(k),
                    Some(s) if s == k => Some(s),
                    _ => Some(Kind::Text),
                });
                TableColumn { name, kind: (match kind { Some(Kind::Number) => "number", Some(Kind::Boolean) => "boolean", _ => "text" }).into() }
            })
            .collect();
        let shown = rows.len();
        let data_rows = total_rows.saturating_sub(header as u64);
        let table = TablePreview {
            format: format.into(),
            delimiter: String::new(),
            columns,
            rows: rows.into_iter().map(|r| (0..width).map(|c| r.get(c).map(|(t, _)| t.clone()).unwrap_or_default()).collect()).collect(),
            total_rows_sampled: shown,
            truncated: data_rows > shown as u64,
            elapsed_ms: 0.0,
        };
        (table, data_rows)
    }
}

fn rows_in(d: &Dimensions) -> (u64, u64) {
    if d.start > d.end {
        return (0, 0);
    }
    ((d.end.0 - d.start.0) as u64 + 1, (d.end.1 - d.start.1) as u64 + 1)
}

fn workbook(path: &str, ext: &str, sheet: Option<usize>) -> Result<SheetPreview, String> {
    let file = || File::open(path).map(BufReader::new).map_err(|e| e.to_string());
    let too_big = || -> Result<(), String> {
        match std::fs::metadata(path) {
            Ok(m) if m.len() > MAX_WHOLE => Err(format!("This workbook is over {} MB, too big to preview: open it to read it.", MAX_WHOLE >> 20)),
            _ => Ok(()),
        }
    };
    // The sheets one can see, as Excel shows them (hidden ones aren't offered).
    let visible = |metadata: &[calamine::Sheet]| -> Vec<String> { metadata.iter().filter(|s| s.visible == SheetVisible::Visible).map(|s| s.name.clone()).collect() };
    let pick = |sheets: &[String]| -> Result<(usize, String), String> {
        let at = sheet.unwrap_or(0).min(sheets.len().saturating_sub(1));
        sheets.get(at).map(|name| (at, name.clone())).ok_or_else(|| "This workbook has no sheet to show.".to_string())
    };
    let read_err = |e: &dyn std::fmt::Display| format!("This workbook can't be read: {e}");
    macro_rules! streamed {
        ($book:expr, $format:expr) => {{
            let mut book = $book.map_err(|e| read_err(&e))?;
            let sheets = visible(book.sheets_metadata());
            let (at, name) = pick(&sheets)?;
            let mut cells = book.worksheet_cells_reader(&name).map_err(|e| read_err(&e))?;
            let dims = cells.dimensions();
            let (total_rows, total_cols) = rows_in(&dims);
            let mut grid = Grid::new(dims.start);
            while let Some(c) = cells.next_cell().map_err(|e| read_err(&e))? {
                if !grid.put(c.get_position(), Value::from(c.get_value())) {
                    break;
                }
            }
            let (table, total_rows) = grid.table($format, total_rows);
            SheetPreview { table, sheets, sheet: at, types: Vec::new(), total_rows, total_cols }
        }};
    }
    macro_rules! whole {
        ($book:expr, $format:expr) => {{
            too_big()?;
            let mut book = $book.map_err(|e| read_err(&e))?;
            let sheets = visible(book.sheets_metadata());
            let (at, name) = pick(&sheets)?;
            let range = book.worksheet_range(&name).map_err(|e| read_err(&e))?;
            let (rows, cols) = range.get_size();
            let mut grid = Grid::new(range.start().unwrap_or((0, 0)));
            'cells: for (r, row) in range.rows().enumerate() {
                for (c, value) in row.iter().enumerate() {
                    if !grid.put((grid.start.0 + r as u32, grid.start.1 + c as u32), Value::from(value)) {
                        break 'cells;
                    }
                }
            }
            let (table, total_rows) = grid.table($format, rows as u64);
            SheetPreview { table, sheets, sheet: at, types: Vec::new(), total_rows, total_cols: cols as u64 }
        }};
    }
    Ok(match ext {
        "xlsx" | "xlsm" | "xltx" | "xltm" | "xlam" => streamed!(Xlsx::new(file()?), "Excel"),
        "xlsb" => streamed!(Xlsb::new(file()?), "Excel binary"),
        "xls" | "xla" | "xlt" => whole!(Xls::new(file()?), "Excel 97–2003"),
        "ods" => whole!(Ods::new(file()?), "OpenDocument"),
        other => return Err(format!("Not a spreadsheet: .{other}")),
    })
}

/// A Parquet column's type: its physical type, and what it holds when the file says ("INT32 · Date").
fn type_name(t: &parquet::schema::types::Type) -> String {
    use parquet::basic::ConvertedType;
    if !t.is_primitive() {
        return match t.get_basic_info().converted_type() {
            ConvertedType::LIST => "list".into(),
            ConvertedType::MAP | ConvertedType::MAP_KEY_VALUE => "map".into(),
            _ => "group".into(),
        };
    }
    let physical = format!("{:?}", t.get_physical_type());
    match t.get_basic_info().logical_type_ref() {
        Some(logical) => format!("{physical} · {logical:?}").split(" {").next().unwrap_or(&physical).to_string(),
        None => match t.get_basic_info().converted_type() {
            ConvertedType::NONE => physical,
            // Older writers mark text UTF8, newer ones String: shown alike.
            ConvertedType::UTF8 => format!("{physical} · String"),
            converted => format!("{physical} · {converted}"),
        },
    }
}

fn parquet(path: &str) -> Result<SheetPreview, String> {
    use parquet::basic::Type as Physical;
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use parquet::record::Field;
    let friendly = |e: parquet::errors::ParquetError| {
        let text = e.to_string();
        if text.to_ascii_lowercase().contains("brotli") {
            "This Parquet file is compressed with Brotli, which Glint doesn't read (Snappy, gzip, LZ4 and Zstandard it does).".to_string()
        } else {
            format!("This Parquet file can't be read: {text}")
        }
    };
    let reader = SerializedFileReader::new(File::open(path).map_err(|e| e.to_string())?).map_err(friendly)?;
    let meta = reader.metadata().file_metadata();
    let total_rows = meta.num_rows().max(0) as u64;
    let fields = meta.schema_descr().root_schema().get_fields().to_vec();
    let columns: Vec<TableColumn> = fields
        .iter()
        .take(MAX_COLS)
        .map(|f| {
            let plain = f.get_basic_info().logical_type_ref().is_none() && f.get_basic_info().converted_type() == parquet::basic::ConvertedType::NONE;
            let kind = match (f.is_primitive().then(|| f.get_physical_type()), plain) {
                (Some(Physical::BOOLEAN), _) => "boolean",
                (Some(Physical::INT32 | Physical::INT64 | Physical::FLOAT | Physical::DOUBLE), true) => "number",
                _ => "text",
            };
            TableColumn { name: f.name().to_string(), kind: kind.into() }
        })
        .collect();
    let text = |field: &Field| -> String {
        match field {
            Field::Null => String::new(),
            Field::Str(s) => cut(s),
            Field::Bytes(b) => match std::str::from_utf8(b.data()) {
                Ok(s) => cut(s),
                Err(_) => b.data().iter().take(64).map(|x| format!("{x:02x}")).collect::<String>() + if b.data().len() > 64 { "…" } else { "" },
            },
            Field::Float(f) => number(*f as f64),
            Field::Double(f) => number(*f),
            other => cut(&other.to_string()),
        }
    };
    let mut rows = Vec::new();
    for row in reader.get_row_iter(None).map_err(friendly)?.take(MAX_ROWS) {
        let row = row.map_err(friendly)?;
        rows.push(row.get_column_iter().take(MAX_COLS).map(|(_, field)| text(field)).collect::<Vec<_>>());
    }
    let shown = rows.len();
    Ok(SheetPreview {
        table: TablePreview { format: "Parquet".into(), delimiter: String::new(), columns, rows, total_rows_sampled: shown, truncated: total_rows > shown as u64, elapsed_ms: 0.0 },
        sheets: Vec::new(),
        sheet: 0,
        types: fields.iter().take(MAX_COLS).map(|f| type_name(f)).collect(),
        total_rows,
        total_cols: fields.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_numbers_dates_and_column_names_as_a_sheet_does() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.1 + 0.2), "0.3");
        assert_eq!(number(1234.5678), "1234.5678");
        assert_eq!(number(-2.5), "-2.5");
        assert_eq!(number(1e20), "1e20");
        assert_eq!(number(1.5e-9), "1.5e-9");
        assert_eq!(letters(0), "A");
        assert_eq!(letters(25), "Z");
        assert_eq!(letters(26), "AA");
        assert_eq!(letters(701), "ZZ");
        assert_eq!(letters(702), "AAA");
        use calamine::ExcelDateTimeType::{DateTime, TimeDelta};
        assert_eq!(date(&ExcelDateTime::new(45943.0, DateTime, false)), "2025-10-13");
        assert_eq!(date(&ExcelDateTime::new(45943.5, DateTime, false)), "2025-10-13 12:00:00");
        assert_eq!(date(&ExcelDateTime::new(0.25, DateTime, false)), "06:00:00");
        assert_eq!(date(&ExcelDateTime::new(1.5, TimeDelta, false)), "36:00:00");
    }

    #[test]
    fn makes_a_header_of_a_first_row_of_text() {
        let mut grid = Grid::new((0, 0));
        for (pos, value) in [((0, 0), Value::Text("Name")), ((0, 1), Value::Text("Size")), ((1, 0), Value::Text("a.txt")), ((1, 1), Value::Int(12)), ((2, 0), Value::Text("b.png")), ((2, 1), Value::Float(3.5))] {
            assert!(grid.put(pos, value));
        }
        let (table, total) = grid.table("Excel", 3);
        assert_eq!(total, 2, "the rows of data, without the header");
        assert_eq!(table.columns.iter().map(|c| (c.name.as_str(), c.kind.as_str())).collect::<Vec<_>>(), [("Name", "text"), ("Size", "number")]);
        assert_eq!(table.rows, [["a.txt", "12"], ["b.png", "3.5"]]);
        assert!(!table.truncated);

        let mut grid = Grid::new((4, 2));
        grid.put((4, 2), Value::Int(1));
        grid.put((5, 3), Value::Bool(true));
        let (table, total) = grid.table("Excel", 900);
        assert_eq!(total, 900);
        assert_eq!(table.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["C", "D"], "no header: the columns' letters");
        assert_eq!(table.rows, [["1", ""], ["", "TRUE"]]);
        assert!(table.truncated);
    }

    #[test]
    fn reads_a_parquet_file_with_its_types() {
        use parquet::data_type::{ByteArray, ByteArrayType, DoubleType, Int32Type};
        use parquet::file::properties::WriterProperties;
        use parquet::file::writer::SerializedFileWriter;
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("glint-sheet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.parquet");
        let schema = Arc::new(parquet::schema::parser::parse_message_type("message t { REQUIRED BYTE_ARRAY name (UTF8); REQUIRED DOUBLE amount; REQUIRED INT32 day (DATE); }").unwrap());
        let props = Arc::new(WriterProperties::builder().set_compression(parquet::basic::Compression::SNAPPY).build());
        let mut writer = SerializedFileWriter::new(File::create(&path).unwrap(), schema, props).unwrap();
        let mut group = writer.next_row_group().unwrap();
        let mut col = group.next_column().unwrap().unwrap();
        col.typed::<ByteArrayType>().write_batch(&[ByteArray::from("a"), ByteArray::from("b")], None, None).unwrap();
        col.close().unwrap();
        let mut col = group.next_column().unwrap().unwrap();
        col.typed::<DoubleType>().write_batch(&[0.0, 12.5], None, None).unwrap();
        col.close().unwrap();
        let mut col = group.next_column().unwrap().unwrap();
        col.typed::<Int32Type>().write_batch(&[20454, 20455], None, None).unwrap();
        col.close().unwrap();
        group.close().unwrap();
        writer.close().unwrap();
        let read = parquet(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read.types, ["BYTE_ARRAY · String", "DOUBLE", "INT32 · Date"]);
        assert_eq!(read.table.columns.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), ["text", "number", "text"]);
        assert_eq!(read.table.rows, [["a", "0", "2026-01-01"], ["b", "12.5", "2026-01-02"]]);
        assert_eq!((read.total_rows, read.table.truncated), (2, false));
    }

    #[test]
    fn stops_after_the_rows_it_keeps() {
        let mut grid = Grid::new((0, 0));
        let kept = (0..5000u32).take_while(|&r| grid.put((r, 0), Value::Int(r as i64))).count();
        assert_eq!(kept, MAX_ROWS + 1);
        assert!(grid.put((0, MAX_COLS as u32 + 5), Value::Int(1)), "columns past the cap are skipped, not the end");
    }
}
