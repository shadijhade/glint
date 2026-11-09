//! Code patterns in Text mode, through ast-grep (an optional tool): code found by its syntax tree, in
//! the 20-odd languages it parses, `console.log($$$ARGS)` or `$X.unwrap()`, whatever the spacing and
//! line breaks. Its matches go to the UI as ripgrep's JSON lines, so they show as a text search's; with a
//! rewrite, each carries what it would become, and applying it rewrites the files (--update-all).

use serde::Deserialize;
use std::path::Path;

/// A code search as Text mode asks for it; ast-grep's arguments are built here from it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeSearch {
    pub query: String,
    pub path: String,
    /// What each match becomes (`console.log($$$A)` → `logger.info($$$A)`).
    #[serde(default)]
    pub rewrite: Option<String>,
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub everything: bool,
}

/// ast-grep's arguments: a search (lines of JSON), or, with `apply`, the rewrite made in the files.
pub fn args(search: &CodeSearch, folder: &str, apply: bool) -> Vec<String> {
    // `--pattern=…`: a pattern starting with "-" is still the pattern.
    let mut args = vec!["run".to_string(), format!("--pattern={}", search.query)];
    if let Some(rewrite) = search.rewrite.as_deref().filter(|r| !r.is_empty()) {
        args.push(format!("--rewrite={rewrite}"));
    }
    args.push(if apply { "--update-all" } else { "--json=stream" }.into());
    if search.everything {
        for skip in ["hidden", "dot", "exclude", "global", "parent", "vcs"] {
            args.extend(["--no-ignore".into(), skip.into()]);
        }
    } else {
        for exclusion in &search.exclusions {
            args.push(format!("--globs=!{exclusion}"));
        }
    }
    args.extend(["--".into(), folder.into()]);
    args
}

#[derive(Deserialize)]
struct Position {
    line: usize,
}

#[derive(Deserialize)]
struct Offsets {
    start: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Range {
    byte_offset: Offsets,
    start: Position,
}

#[derive(Deserialize)]
struct CharCount {
    leading: usize,
}

/// One of ast-grep's JSON lines: a match, its lines, and where in them it starts.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    text: String,
    range: Range,
    file: String,
    lines: String,
    char_count: CharCount,
    replacement: Option<String>,
}

/// An ast-grep match as ripgrep's JSON line of one (`type: match`, its lines, its line number and
/// the match's byte range in them), with `replacement` added when there's a rewrite. None for a line
/// that isn't a match.
pub fn as_ripgrep(line: &str, folder: &str) -> Option<String> {
    let found: Found = serde_json::from_str(line).ok()?;
    let file = found.file.replace('/', "\\");
    let path = if Path::new(&file).is_absolute() { file } else { Path::new(folder).join(&file).to_string_lossy().into_owned() };
    // ast-grep counts characters; ripgrep's offsets are bytes.
    let leading = found.lines.char_indices().nth(found.char_count.leading).map_or(found.lines.len(), |(i, _)| i);
    let mut data = serde_json::json!({
        "path": { "text": path },
        "lines": { "text": format!("{}\n", found.lines) },
        "line_number": found.range.start.line + 1,
        "absolute_offset": found.range.byte_offset.start.saturating_sub(leading),
        "submatches": [{ "match": { "text": found.text }, "start": leading, "end": leading + found.text.len() }],
    });
    if let Some(replacement) = found.replacement {
        data["replacement"] = replacement.into();
    }
    Some(serde_json::json!({ "type": "match", "data": data }).to_string())
}

/// How many matches ast-grep says it rewrote ("Applied 3 changes").
pub fn applied(said: &str) -> u64 {
    said.split_whitespace().skip_while(|w| !w.eq_ignore_ascii_case("applied")).nth(1).and_then(|n| n.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_ast_greps_matches_into_ripgreps() {
        let line = r#"{"text":"console.log(é)","range":{"byteOffset":{"start":22,"end":37},"start":{"line":1,"column":0},"end":{"line":1,"column":14}},"file":"src\\unicode.ts","lines":"  é console.log(é);\r","charCount":{"leading":4,"trailing":2},"replacement":"logger.info(é)","language":"TypeScript"}"#;
        let json: serde_json::Value = serde_json::from_str(&as_ripgrep(line, r"C:\proj").unwrap()).unwrap();
        assert_eq!(json["type"], "match");
        assert_eq!(json["data"]["path"]["text"], r"C:\proj\src\unicode.ts");
        assert_eq!(json["data"]["line_number"], 2);
        // "  é " is 5 bytes: the match starts there, and ends 15 bytes on (é is two).
        assert_eq!(json["data"]["submatches"][0]["start"], 5);
        assert_eq!(json["data"]["submatches"][0]["end"], 20);
        assert_eq!(json["data"]["absolute_offset"], 17);
        assert_eq!(json["data"]["replacement"], "logger.info(é)");
        assert_eq!(as_ripgrep("not json", "C:\\"), None);
    }

    #[test]
    fn builds_its_arguments_and_reads_what_it_did() {
        let search = CodeSearch { query: "-x".into(), path: String::new(), rewrite: Some("y".into()), exclusions: vec!["node_modules".into()], everything: false };
        assert_eq!(args(&search, r"C:\proj", false), ["run", "--pattern=-x", "--rewrite=y", "--json=stream", "--globs=!node_modules", "--", r"C:\proj"]);
        assert_eq!(args(&search, r"C:\proj", true)[3], "--update-all");
        assert_eq!(applied("Applied 3 changes\n"), 3);
        assert_eq!(applied("nothing"), 0);
    }
}
