//! Zero-copy SQLite 3 B-Tree schema inspector and CSV / TSV / JSONL tabular data lens.
//! Parses database headers, `sqlite_schema` B-Tree pages, and delimited/JSONL slices in sub-millisecond
//! time without external crates.

use crate::fsx::blocking;
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteObject {
    /** "table", "index", "view", or "trigger". */
    pub kind: String,
    pub name: String,
    pub table_name: String,
    pub root_page: u64,
    pub sql: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteInfo {
    pub page_size: u32,
    pub page_count: u32,
    pub freelist_pages: u32,
    pub journal_mode: String,
    pub encoding: String,
    pub sqlite_version: String,
    pub user_version: u32,
    pub application_id: u32,
    pub objects: Vec<SqliteObject>,
    pub elapsed_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableColumn {
    pub name: String,
    /** "number", "boolean", or "text". */
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePreview {
    /** "CSV", "TSV", "DSV", or "JSONL". */
    pub format: String,
    pub delimiter: String,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<Vec<String>>,
    pub total_rows_sampled: usize,
    pub truncated: bool,
    pub elapsed_ms: f64,
}

#[tauri::command]
pub async fn inspect_sqlite(path: String) -> Result<SqliteInfo, String> {
    blocking(move || parse_sqlite_file(&path)).await
}

#[tauri::command]
pub async fn inspect_tabular(path: String, max_bytes: usize) -> Result<TablePreview, String> {
    blocking(move || parse_tabular_file(&path, max_bytes)).await
}

fn u16_be(buf: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_be_bytes(buf.get(off..off + 2)?.try_into().ok()?))
}

fn u32_be(buf: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

/// Decodes a SQLite big-endian variable-length integer (1 to 9 bytes), returning `(value, bytes_read)`.
fn read_varint(buf: &[u8], off: usize) -> Option<(u64, usize)> {
    let mut val = 0u64;
    for i in 0..8 {
        let b = *buf.get(off + i)?;
        val = (val << 7) | (b & 0x7f) as u64;
        if (b & 0x80) == 0 {
            return Some((val, i + 1));
        }
    }
    let b = *buf.get(off + 8)?;
    val = (val << 8) | b as u64;
    Some((val, 9))
}

fn serial_content_len(serial: u64) -> usize {
    match serial {
        0 | 8 | 9 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        n if n >= 12 && n % 2 == 0 => ((n - 12) / 2) as usize,
        n if n >= 13 && n % 2 == 1 => ((n - 13) / 2) as usize,
        _ => 0,
    }
}

fn decode_serial_int(buf: &[u8], serial: u64) -> u64 {
    match serial {
        1 => buf.first().copied().unwrap_or(0) as u64,
        2 => u16_be(buf, 0).unwrap_or(0) as u64,
        3 if buf.len() >= 3 => ((buf[0] as u64) << 16) | ((buf[1] as u64) << 8) | (buf[2] as u64),
        4 => u32_be(buf, 0).unwrap_or(0) as u64,
        8 => 0,
        9 => 1,
        _ => 0,
    }
}

fn parse_schema_leaf_page(page: &[u8], hdr_off: usize, out: &mut Vec<SqliteObject>) {
    if page.get(hdr_off).copied() != Some(0x0d) {
        return;
    }
    let Some(cell_count) = u16_be(page, hdr_off + 3) else { return };
    let ptr_base = hdr_off + 8;

    for i in 0..(cell_count as usize).min(512) {
        let Some(cell_off) = u16_be(page, ptr_base + i * 2).map(|v| v as usize) else { continue };
        let Some((_, n1)) = read_varint(page, cell_off) else { continue };
        let Some((_, n2)) = read_varint(page, cell_off + n1) else { continue };
        let rec_off = cell_off + n1 + n2;
        let Some((hdr_len, nh)) = read_varint(page, rec_off) else { continue };
        let hdr_end = rec_off.saturating_add(hdr_len as usize);
        if hdr_end > page.len() {
            continue;
        }

        let mut pos = rec_off + nh;
        let mut serials = [0u64; 5];
        let mut ok = true;
        for s in &mut serials {
            if pos >= hdr_end {
                ok = false;
                break;
            }
            let Some((st, ns)) = read_varint(page, pos) else {
                ok = false;
                break;
            };
            *s = st;
            pos += ns;
        }
        if !ok {
            continue;
        }

        let mut body_pos = hdr_end;
        let mut cols: [&[u8]; 5] = [&[]; 5];
        for (k, &st) in serials.iter().enumerate() {
            let len = serial_content_len(st);
            let Some(slice) = page.get(body_pos..body_pos + len) else {
                ok = false;
                break;
            };
            cols[k] = slice;
            body_pos += len;
        }
        if !ok {
            continue;
        }

        let kind = String::from_utf8_lossy(cols[0]).into_owned();
        let name = String::from_utf8_lossy(cols[1]).into_owned();
        let table_name = String::from_utf8_lossy(cols[2]).into_owned();
        let root_page = decode_serial_int(cols[3], serials[3]);
        let sql = String::from_utf8_lossy(cols[4]).into_owned();
        if !name.is_empty() {
            out.push(SqliteObject {
                kind,
                name,
                table_name,
                root_page,
                sql,
            });
        }
    }
}

pub fn parse_sqlite_file(path: &str) -> Result<SqliteInfo, String> {
    let start = Instant::now();
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hdr = [0u8; 100];
    file.read_exact(&mut hdr).map_err(|_| "Not a valid SQLite 3 database".to_string())?;
    if &hdr[0..16] != b"SQLite format 3\0" {
        return Err("Missing SQLite 3 header signature".into());
    }

    let raw_page_size = u16_be(&hdr, 16).unwrap_or(4096);
    let page_size = if raw_page_size == 1 { 65536 } else { (raw_page_size as u32).clamp(512, 65536) };
    let journal_mode = if hdr[18] == 2 { "WAL (Write-Ahead Log)" } else { "Rollback Journal" }.to_string();
    let page_count = u32_be(&hdr, 28).unwrap_or(0);
    let freelist_pages = u32_be(&hdr, 36).unwrap_or(0);
    let encoding = match u32_be(&hdr, 56).unwrap_or(1) {
        2 => "UTF-16LE",
        3 => "UTF-16BE",
        _ => "UTF-8",
    }
    .to_string();
    let user_version = u32_be(&hdr, 60).unwrap_or(0);
    let application_id = u32_be(&hdr, 68).unwrap_or(0);
    let ver_num = u32_be(&hdr, 96).unwrap_or(0);
    let sqlite_version = if ver_num > 0 {
        format!("{}.{}.{}", ver_num / 1_000_000, (ver_num / 1_000) % 1000, ver_num % 1000)
    } else {
        "3.x".into()
    };

    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut page1 = vec![0u8; page_size as usize];
    let n = file.read(&mut page1).map_err(|e| e.to_string())?;
    page1.truncate(n);

    let mut objects = Vec::new();
    if page1.len() > 108 {
        match page1[100] {
            0x0d => parse_schema_leaf_page(&page1, 100, &mut objects),
            0x05 if page1.len() >= 112 => {
                // Interior table B-Tree on Page 1: visit child pages.
                let cell_count = u16_be(&page1, 103).unwrap_or(0) as usize;
                let right_child = u32_be(&page1, 108).unwrap_or(0);
                let mut child_pages = Vec::new();
                for i in 0..cell_count.min(64) {
                    if let Some(off) = u16_be(&page1, 112 + i * 2) {
                        if let Some(pg) = u32_be(&page1, off as usize) {
                            child_pages.push(pg);
                        }
                    }
                }
                if right_child > 1 {
                    child_pages.push(right_child);
                }
                let mut buf = vec![0u8; page_size as usize];
                for pg in child_pages.into_iter().take(32) {
                    if pg <= 1 {
                        continue;
                    }
                    let offset = (pg as u64 - 1) * (page_size as u64);
                    if file.seek(SeekFrom::Start(offset)).is_ok() {
                        if let Ok(rn) = file.read(&mut buf) {
                            parse_schema_leaf_page(&buf[..rn], 0, &mut objects);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(SqliteInfo {
        page_size,
        page_count,
        freelist_pages,
        journal_mode,
        encoding,
        sqlite_version,
        user_version,
        application_id,
        objects,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

fn parse_dsv_line(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' && cur.is_empty() {
            in_quotes = true;
        } else if c == delim {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(cur.trim().to_string());
    out
}

fn infer_column_kind(rows: &[Vec<String>], col_idx: usize) -> String {
    let mut seen = 0usize;
    let mut all_num = true;
    let mut all_bool = true;
    for row in rows.iter().take(100) {
        let Some(val) = row.get(col_idx) else { continue };
        let trimmed = val.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") || trimmed.eq_ignore_ascii_case("na") {
            continue;
        }
        seen += 1;
        if trimmed.parse::<f64>().is_err() {
            all_num = false;
        }
        if !matches!(trimmed.to_ascii_lowercase().as_str(), "true" | "false") {
            all_bool = false;
        }
    }
    if seen == 0 {
        "text".into()
    } else if all_bool {
        "boolean".into()
    } else if all_num {
        "number".into()
    } else {
        "text".into()
    }
}

pub fn parse_tabular_file(path: &str, max_bytes: usize) -> Result<TablePreview, String> {
    let start = Instant::now();
    let limit = max_bytes.clamp(16 * 1024, 1024 * 1024);
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);

    let text = String::from_utf8_lossy(&bytes);
    // Excel's "CSV UTF-8" starts with a byte-order mark, which would end up in the first column's name.
    let text = text.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();

    if lines.is_empty() {
        return Err("File is empty".into());
    }

    let is_jsonl = path.to_ascii_lowercase().ends_with(".jsonl")
        || path.to_ascii_lowercase().ends_with(".ndjson")
        || (lines[0].trim_start().starts_with('{') && lines[0].trim_end().ends_with('}'));

    if is_jsonl {
        let mut col_names: Vec<String> = Vec::new();
        let mut parsed_objs: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
        for line in lines.iter().take(300) {
            if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(line) {
                for k in map.keys() {
                    if !col_names.contains(k) && col_names.len() < 48 {
                        col_names.push(k.clone());
                    }
                }
                parsed_objs.push(map);
            }
        }
        if col_names.is_empty() {
            return Err("No valid JSON objects found".into());
        }
        let rows: Vec<Vec<String>> = parsed_objs
            .into_iter()
            .map(|map| {
                col_names
                    .iter()
                    .map(|k| match map.get(k) {
                        None | Some(serde_json::Value::Null) => String::new(),
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                    })
                    .collect()
            })
            .collect();
        let columns = col_names
            .into_iter()
            .enumerate()
            .map(|(i, name)| TableColumn {
                name,
                kind: infer_column_kind(&rows, i),
            })
            .collect();
        let total_rows_sampled = rows.len();
        return Ok(TablePreview {
            format: "JSONL".into(),
            delimiter: "\\n".into(),
            columns,
            rows,
            total_rows_sampled,
            truncated,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        });
    }

    // Auto-detect delimiter from the first line (preferring \t for .tsv, otherwise most frequent of `,`, `\t`, `;`, `|`).
    let first = lines[0];
    let delim = if path.to_ascii_lowercase().ends_with(".tsv") || first.contains('\t') {
        '\t'
    } else {
        [',', ';', '|']
            .into_iter()
            .max_by_key(|&d| first.chars().filter(|&c| c == d).count())
            .unwrap_or(',')
    };

    let raw_headers = parse_dsv_line(first, delim);
    let col_count = raw_headers.len().clamp(1, 64);
    let body_lines = if truncated && lines.len() > 2 { &lines[1..lines.len() - 1] } else { &lines[1..] };
    let rows: Vec<Vec<String>> = body_lines
        .iter()
        .take(400)
        .map(|l| {
            let mut r = parse_dsv_line(l, delim);
            r.resize(col_count, String::new());
            r
        })
        .collect();

    let columns: Vec<TableColumn> = raw_headers
        .into_iter()
        .take(col_count)
        .enumerate()
        .map(|(i, name)| TableColumn {
            name: if name.is_empty() { format!("col_{}", i + 1) } else { name },
            kind: infer_column_kind(&rows, i),
        })
        .collect();

    let total_rows_sampled = rows.len();
    Ok(TablePreview {
        format: if delim == '\t' { "TSV".into() } else { "CSV".into() },
        delimiter: if delim == '\t' { "TAB".into() } else { delim.to_string() },
        columns,
        rows,
        total_rows_sampled,
        truncated,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv_and_infers_column_types() {
        let dir = std::env::temp_dir().join(format!("glint-data-lens-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("metrics.csv");
        std::fs::write(&csv, "service,latency_ms,healthy\nindex,4.8,true\nrg,12.1,false\n").unwrap();

        let preview = parse_tabular_file(&csv.to_string_lossy(), 65536).unwrap();
        assert_eq!(preview.format, "CSV");
        assert_eq!(preview.columns.len(), 3);
        assert_eq!(preview.columns[0].kind, "text");
        assert_eq!(preview.columns[1].kind, "number");
        assert_eq!(preview.columns[2].kind, "boolean");
        assert_eq!(preview.rows.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
