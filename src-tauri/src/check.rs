//! Checks with optional tools: YARA rules (YARA-X) and secrets left in files (Kingfisher). Both run on
//! this PC only: Kingfisher's checking of the keys it finds against their services is turned off, so
//! nothing is sent anywhere, and the secrets it finds reach the UI masked.

use crate::fsx::blocking;
use crate::proc::{self, Slot};
use serde::Serialize;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Instant;

/// The check running (one at a time).
static RUN: LazyLock<Slot> = LazyLock::new(Slot::default);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub path: String,
    /// Where in the file (secrets), 0 when it's the whole file (YARA rules).
    pub line: u32,
    /// What matched: a rule's name.
    pub rule: String,
    /// A secret's kind in words, or a YARA rule's namespace.
    pub title: String,
    /// The secret found, masked ("AKIA••••K4PD"); empty for YARA rules.
    pub value: String,
    /// "high", "medium" or "low" for secrets; empty for YARA rules.
    pub confidence: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub findings: Vec<Finding>,
    pub elapsed_ms: f64,
}

/// Scans `target` (a file, or a folder and what's in it) with the YARA rules in `rules` (a file, or a
/// folder of .yar and .yara files). Rejects with "cancelled" when stopped.
#[tauri::command]
pub async fn yara_scan(rules: String, target: String) -> Result<CheckReport, String> {
    blocking(move || {
        let mut yr = crate::tools::command("yara-x").map_err(|_| "YARA-X isn't installed".to_string())?;
        yr.args(["scan", "--output-format", "ndjson", "--disable-console-logs", "--recursive", "--"]).arg(&rules).arg(&target);
        let started = Instant::now();
        let (code, out, err) = run(yr)?;
        // It prints only the files that matched; rules that don't compile end it with what's wrong.
        if code != Some(0) && out.trim().is_empty() {
            return Err(first_lines(&err).unwrap_or_else(|| format!("YARA-X stopped with exit code {code:?}")));
        }
        Ok(CheckReport { findings: yara_matches(&out), elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
    })
    .await
}

/// Looks for passwords, keys and tokens in `target`'s files as they are (not Git's history),
/// skipping files over 25 MB. Rejects with "cancelled" when stopped.
#[tauri::command]
pub async fn secrets_scan(target: String) -> Result<CheckReport, String> {
    blocking(move || {
        let mut kingfisher = crate::tools::command("kingfisher").map_err(|_| "Kingfisher isn't installed".to_string())?;
        kingfisher.arg("scan").arg(&target).args(["--format", "json", "--no-validate", "--no-update-check", "--quiet", "--git-history", "none", "--max-file-size", "25"]);
        let started = Instant::now();
        let (code, out, err) = run(kingfisher)?;
        // 0: nothing found, 200 and up: found some; its report comes either way.
        if !out.trim_start().starts_with('{') {
            return Err(first_lines(&err).unwrap_or_else(|| format!("Kingfisher stopped with exit code {code:?}")));
        }
        Ok(CheckReport { findings: secrets(&out)?, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
    })
    .await
}

/// Stops the check running.
#[tauri::command]
pub fn cancel_check() {
    RUN.kill();
}

/// Runs `command` as the check, with no window; its exit code and what it said on stdout and stderr.
fn run(mut command: Command) -> Result<(Option<i32>, String, String), String> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    proc::no_window(&mut command);
    let mut child = command.spawn().map_err(|e| format!("Couldn't start it: {e}"))?;
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let pid = RUN.replace(child);
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let mut out = String::new();
    let _ = stdout.read_to_string(&mut out);
    let err = errors.join().unwrap_or_default();
    let status = RUN.finish(pid).ok_or("cancelled")?;
    Ok((status.code(), out, err))
}

fn first_lines(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// YARA-X's JSON lines: each file that matched, with the rules it matched.
fn yara_matches(out: &str) -> Vec<Finding> {
    let mut found = Vec::new();
    for line in out.lines().filter(|l| l.starts_with('{')) {
        let Ok(file) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let path = file["path"].as_str().unwrap_or_default();
        for rule in file["rules"].as_array().into_iter().flatten() {
            let name = rule["identifier"].as_str().unwrap_or_default();
            let namespace = rule["namespace"].as_str().filter(|n| *n != "default").unwrap_or_default();
            found.push(Finding { path: path.into(), line: 0, rule: name.into(), title: namespace.into(), value: String::new(), confidence: String::new() });
        }
    }
    found
}

/// Kingfisher's JSON report: each secret with its rule, file and line, its value masked.
fn secrets(out: &str) -> Result<Vec<Finding>, String> {
    let report: serde_json::Value = serde_json::from_str(out).map_err(|e| format!("Kingfisher said something unexpected: {e}"))?;
    Ok(report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| {
            let (rule, finding) = (&f["rule"], &f["finding"]);
            let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
            Finding {
                path: text(&finding["path"]),
                line: finding["line"].as_u64().unwrap_or(0) as u32,
                rule: text(&rule["id"]),
                title: text(&rule["description"]),
                value: mask(finding["snippet"].as_str().unwrap_or_default()),
                confidence: text(&finding["confidence"]),
            }
        })
        .collect())
}

/// A secret with most of it hidden: its first four and last two characters (less of a short one).
pub fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.trim().chars().collect();
    let (head, tail) = if chars.len() >= 12 { (4, 2) } else if chars.len() >= 6 { (2, 1) } else { (0, 0) };
    let hidden = (chars.len() - head - tail).clamp(1, 8);
    let mut masked: String = chars[..head].iter().collect();
    masked.extend(std::iter::repeat_n('•', hidden));
    masked.extend(&chars[chars.len() - tail..]);
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_secrets() {
        assert_eq!(mask("AKIAZ7Q3VQ5LWXJ2K4PD"), "AKIA••••••••PD");
        assert_eq!(mask("hunter2"), "hu••••2");
        assert_eq!(mask("abc"), "•••");
        assert_eq!(mask(""), "•");
    }

    #[test]
    fn reads_what_yara_x_and_kingfisher_report() {
        let yara = "{\"path\":\"C:\\\\x\\\\a.txt\",\"rules\":[{\"identifier\":\"has_glint\"}]}\n{\"path\":\"C:\\\\x\\\\c.exe\",\"rules\":[{\"identifier\":\"is_mz\",\"namespace\":\"pe\"},{\"identifier\":\"other\"}]}\n";
        let found = yara_matches(yara);
        assert_eq!(found.iter().map(|f| (f.path.as_str(), f.rule.as_str(), f.title.as_str())).collect::<Vec<_>>(), [(r"C:\x\a.txt", "has_glint", ""), (r"C:\x\c.exe", "is_mz", "pe"), (r"C:\x\c.exe", "other", "")]);

        let kingfisher = r#"{"findings":[{"rule":{"title":"GITHUB-PAT","name":"github-pat","id":"betterleaks.github-pat","description":"Uncovered a GitHub Personal Access Token"},"finding":{"snippet":"ghp_8sK2mN4pQ7rT1vX9zB3cD5fG6hJ0kL2wY4uE","confidence":"high","line":4,"path":"C:\\x\\settings.cfg"}}],"metadata":{}}"#;
        let found = secrets(kingfisher).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].path.as_str(), found[0].line, found[0].rule.as_str(), found[0].value.as_str(), found[0].confidence.as_str()), (r"C:\x\settings.cfg", 4, "betterleaks.github-pat", "ghp_••••••••uE", "high"));
        assert!(!found[0].value.contains("8sK2"));
        assert!(secrets("{}").unwrap().is_empty());
    }

    /// A real YARA-X scan, when it's installed.
    #[test]
    fn scans_with_yara_rules() {
        if crate::tools::command("yara-x").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("glint-yara-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("target").join("sub")).unwrap();
        std::fs::write(dir.join("rules.yar"), "rule has_glint { strings: $a = \"Glint\" condition: $a }").unwrap();
        std::fs::write(dir.join("target").join("a.txt"), "Hello from Glint").unwrap();
        std::fs::write(dir.join("target").join("sub").join("b.txt"), "nothing here").unwrap();
        let scan = |rules: &str| tauri::async_runtime::block_on(yara_scan(dir.join(rules).to_string_lossy().into(), dir.join("target").to_string_lossy().into()));
        let report = scan("rules.yar");
        std::fs::write(dir.join("broken.yar"), "rule { nope").unwrap();
        let broken = scan("broken.yar");
        let _ = std::fs::remove_dir_all(&dir);
        let found = report.unwrap().findings;
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].path.ends_with("a.txt") && found[0].rule == "has_glint");
        assert!(broken.is_err());
    }
}
