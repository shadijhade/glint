//! Scan with Microsoft Defender: its command-line scanner (MpCmdRun), a custom scan of a file or a
//! folder, as File Explorer's own "Scan with Microsoft Defender" asks for. What it finds, it deals
//! with as its settings say (and lists in Windows Security's Protection history).

use crate::fsx::blocking;
use crate::proc::{self, Slot};
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Instant;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DefenderReport {
    /// It found nothing.
    pub clean: bool,
    pub threats: Vec<Threat>,
    pub elapsed_ms: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Threat {
    /// As Defender names it, e.g. "Virus:DOS/EICAR_Test_File".
    pub name: String,
    /// Where it found it.
    pub resources: Vec<String>,
}

/// The scan running (one at a time, as Defender runs them).
static SCAN: LazyLock<Slot> = LazyLock::new(Slot::default);

/// MpCmdRun.exe: the current platform's (Defender updates it there), else the one Windows ships.
fn scanner() -> Option<PathBuf> {
    let platform = Path::new(&std::env::var_os("ProgramData")?).join(r"Microsoft\Windows Defender\Platform");
    let current = platform.join("CurrentVersion").join("MpCmdRun.exe");
    if current.exists() {
        return Some(current);
    }
    let version = |name: &str| name.split(['.', '-']).map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let newest = std::fs::read_dir(&platform)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("MpCmdRun.exe").exists())
        .max_by_key(|p| version(&p.file_name().unwrap_or_default().to_string_lossy()));
    newest.map(|p| p.join("MpCmdRun.exe")).or_else(|| {
        let shipped = Path::new(&std::env::var_os("ProgramFiles")?).join(r"Windows Defender\MpCmdRun.exe");
        shipped.exists().then_some(shipped)
    })
}

/// Scans `path` (a file, or a folder with what's in it). Rejects with "cancelled" when stopped.
#[tauri::command]
pub async fn defender_scan(path: String) -> Result<DefenderReport, String> {
    blocking(move || scan(&path)).await
}

/// Stops the scan running.
#[tauri::command]
pub fn cancel_defender_scan() {
    SCAN.kill();
}

fn scan(path: &str) -> Result<DefenderReport, String> {
    let started = Instant::now();
    let exe = scanner().ok_or("Microsoft Defender isn't on this PC")?;
    let mut command = Command::new(exe);
    command.args(["-Scan", "-ScanType", "3", "-File"]).arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    proc::no_window(&mut command);
    let mut child = command.spawn().map_err(|e| format!("Couldn't start Microsoft Defender's scanner: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("Couldn't read Microsoft Defender's scanner")?;
    let pid = SCAN.replace(child);
    let mut said = String::new();
    let _ = stdout.read_to_string(&mut said);
    let status = SCAN.finish(pid).ok_or("cancelled")?;
    let threats = threats_in(&said);
    // 0: nothing found; 2: threats found (and dealt with); anything else: it couldn't scan.
    match status.code() {
        Some(0) | Some(2) => Ok(DefenderReport { clean: threats.is_empty() && status.code() == Some(0), threats, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 }),
        code => {
            let why = said.lines().rev().find(|l| l.contains("hr =") || l.contains("failed")).map(str::trim).unwrap_or_default().to_string();
            Err(format!(
                "Microsoft Defender couldn't scan (it may be turned off, with another antivirus in its place){}",
                if why.is_empty() { code.map(|c| format!(": exit code {c:#x}")).unwrap_or_default() } else { format!(": {why}") }
            ))
        }
    }
}

/// The threats MpCmdRun lists: each "Threat : name", then the "file : path" lines under it.
fn threats_in(said: &str) -> Vec<Threat> {
    let mut threats: Vec<Threat> = Vec::new();
    for line in said.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if key.eq_ignore_ascii_case("Threat") {
            threats.push(Threat { name: value.to_string(), resources: Vec::new() });
        } else if let Some(threat) = threats.last_mut().filter(|_| key.eq_ignore_ascii_case("file") || key.eq_ignore_ascii_case("containerfile")) {
            // The value is a path with its own colon (C:\…), so it's what follows the first one.
            threat.resources.push(line.split_once(" : ").map_or(value, |(_, v)| v.trim()).to_string());
        }
    }
    threats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_found() {
        let said = "Scan starting...\r\nScan finished.\r\nScanning C:\\Users\\me\\Downloads found 1 threats.\r\n\r\n<===========================LIST OF DETECTED THREATS==========================>\r\n----------------------------- Threat information ------------------------------\r\nThreat                  : Virus:DOS/EICAR_Test_File\r\nResources               : 1 total\r\n    file                : C:\\Users\\me\\Downloads\\eicar.com\r\n-------------------------------------------------------------------------------\r\n";
        assert_eq!(threats_in(said), [Threat { name: "Virus:DOS/EICAR_Test_File".into(), resources: vec![r"C:\Users\me\Downloads\eicar.com".into()] }]);
        assert!(threats_in("Scan starting...\nScan finished.\nScanning C:\\x found no threats.\n").is_empty());
    }

    /// A clean file, scanned for real when Defender is there.
    #[test]
    fn scans_a_clean_file() {
        if scanner().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("glint-defender-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, "Hello from Glint").unwrap();
        let report = scan(&file.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
        match report {
            Ok(report) => assert!(report.clean && report.threats.is_empty(), "{report:?}"),
            // Turned off for another antivirus: said so.
            Err(e) => assert!(e.contains("couldn't scan"), "{e}"),
        }
    }
}
