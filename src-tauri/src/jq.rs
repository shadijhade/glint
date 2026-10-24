//! JSON, YAML and TOML files as a tree, and jq filters over them.
//!
//! The file is read in a worker: Glint itself, started with `--data-worker`, reading it with jaq's
//! readers (jaq-json, jaq-fmts) and writing each document as a line of JSON. A filter runs in jaq, the jq
//! clone in Rust, an optional tool installed on demand (it would add 4 MB to Glint compiled in), fed the
//! file through its standard input. A file can be anything and a filter is typed by hand, so neither may
//! take the app down (a stack overflow aborts the process it happens in, and `[range(1e9)]` eats memory):
//! both run in a job object that holds them to 1 GB of memory and no child processes and dies with Glint,
//! they're stopped after 10 seconds, and a newer query (or Stop) stops the one before.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::proc::Job;

const WORKER_ARG: &str = "--data-worker";
/// The biggest file read (the preview offers the tree up to this size).
const MAX_INPUT: u64 = 64 << 20;
/// The most values kept, and the most JSON: past either, the rest is cut.
const MAX_VALUES: usize = 100_000;
const MAX_OUTPUT: usize = 64 << 20;
const DEADLINE: Duration = Duration::from_secs(10);
const MEMORY: usize = 1 << 30;

/// What the worker is asked: a file and its format.
#[derive(Serialize, Deserialize)]
struct Ask {
    path: String,
    /// "json", "yaml" or "toml".
    format: String,
}

/// How a query ended, sent before its values.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Head {
    truncated: bool,
    /// The file couldn't be read as its format, the filter didn't compile, or it stopped with an
    /// error (after the values before it).
    error: Option<String>,
    elapsed: f64,
}

/// Glint started as the data worker: reads the one file asked on its standard input, writes its
/// documents as lines of JSON and exits, before anything of the app starts (main.rs).
pub fn worker() -> Option<i32> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(WORKER_ARG)) {
        return None;
    }
    let mut line = String::new();
    let ask = std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string()).and_then(|_| serde_json::from_str::<Ask>(&line).map_err(|e| e.to_string()));
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let read = ask.and_then(|ask| read_values(&ask, &mut out));
    let _ = out.flush();
    Some(match read {
        Ok(()) => 0,
        Err(e) => {
            eprint!("{}", e.chars().take(4000).collect::<String>());
            2
        }
    })
}

/// A file's text: UTF-8 without its byte order mark, or UTF-16 by its mark.
fn decode(bytes: Vec<u8>) -> Result<String, String> {
    let utf16 = |bytes: &[u8], le: bool| {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&pair| if le { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) }).collect();
        String::from_utf16(&units).map_err(|_| "This file isn't valid UTF-16 text.".to_string())
    };
    match bytes.as_slice() {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).map_err(|_| "This file isn't valid UTF-8 text.".into()),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8(bytes).map_err(|_| "This file isn't valid UTF-8 text.".into()),
    }
}

fn read_text(path: &str) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_INPUT + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err(format!("This file is over {} MB, too big for the tree.", MAX_INPUT >> 20));
    }
    decode(bytes)
}

/// Reads the file as its format and writes each document in it as a line of JSON to `out`; the
/// documents before one that can't be read are written.
fn read_values(ask: &Ask, out: &mut impl Write) -> Result<(), String> {
    use jaq_json::Val;
    let text = read_text(&ask.path)?;
    let pp = jaq_json::write::Pp::default();
    let unreadable = |e: &dyn std::fmt::Display| format!("This file can't be read as {}: {e}", ask.format.to_uppercase());
    let mut write = |value: Val| -> Result<(), String> {
        jaq_json::write::write(out, &pp, 0, &value).and_then(|_| out.write_all(b"\n")).map_err(|e| e.to_string())
    };
    match ask.format.as_str() {
        "json" => jaq_json::read::parse_many(text.as_bytes()).try_for_each(|value| write(value.map_err(|e| unreadable(&e))?)),
        "yaml" => jaq_fmts::read::yaml::parse_many(&text).try_for_each(|value| write(value.map_err(|e| unreadable(&e))?)),
        "toml" => write(jaq_fmts::read::toml::parse(&text).map_err(|e| unreadable(&e))?),
        other => Err(format!("Not a format a tree is shown for: {other}")),
    }
}

/// A line of jaq's JSON as JSON: it writes NaN and ±Infinity bare, which JSON has no words for, so
/// those (outside strings) become null.
fn json_safe(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains("NaN") && !line.contains("Infinity") {
        return line.into();
    }
    let mut out = String::with_capacity(line.len());
    let (mut in_string, mut escaped) = (false, false);
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if in_string {
            in_string = escaped || c != '"';
            escaped = !escaped && c == '\\';
        } else if c == '"' {
            in_string = true;
        } else if let Some(word) = ["-Infinity", "Infinity", "NaN"].iter().find(|w| rest.starts_with(**w)) {
            out.push_str("null");
            rest = &rest[word.len()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.into()
}

/// The query running, which a newer one (or Stop) ends by dropping its job.
static RUNNING: Mutex<Option<(u64, Job)>> = Mutex::new(None);
static QUERIES: AtomicU64 = AtomicU64::new(0);

fn running() -> std::sync::MutexGuard<'static, Option<(u64, Job)>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a run printed: its values, one per line (made JSON), up to the caps.
struct Printed {
    values: Vec<u8>,
    count: usize,
    truncated: bool,
}

/// Reads the values a run prints, a line each, into a JSON array; past the caps it stops reading (the
/// run is ended then, with its job).
fn values_of(pipe: impl Read + Send + 'static) -> mpsc::Receiver<Printed> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut printed = Printed { values: b"[".to_vec(), count: 0, truncated: false };
        for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if printed.count == MAX_VALUES || printed.values.len() + line.len() > MAX_OUTPUT {
                printed.truncated = true;
                break;
            }
            if printed.count > 0 {
                printed.values.push(b',');
            }
            printed.values.extend_from_slice(json_safe(line).as_bytes());
            printed.count += 1;
        }
        printed.values.push(b']');
        let _ = tx.send(printed);
    });
    rx
}

/// Reads all of a pipe on a thread, up to `cap` bytes (past that, the rest is drained unread).
fn text_of(mut pipe: impl Read + Send + 'static, cap: usize) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = (&mut pipe).take(cap as u64).read_to_end(&mut kept);
        let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        let _ = tx.send(String::from_utf8_lossy(&kept).trim().to_string());
    });
    rx
}

fn query(path: String, format: String, filter: String) -> Result<Vec<u8>, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
    let started = Instant::now();
    if !matches!(format.as_str(), "json" | "yaml" | "toml") {
        return Err(format!("Not a format a tree is shown for: {format}"));
    }
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if size > MAX_INPUT {
        return Err(format!("This file is over {} MB, too big for the tree.", MAX_INPUT >> 20));
    }
    // The file as it is: Glint's worker reads it. A filter: jaq runs it over the file, given on its
    // standard input, so neither starts on anything before it's in its job.
    let filter = filter.trim();
    let as_is = filter.is_empty() || filter == ".";
    let mut command = if as_is {
        let mut worker = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        worker.arg(WORKER_ARG);
        worker
    } else {
        let mut jaq = crate::tools::command("jaq")?;
        jaq.args(["--from", &format, "--compact-output", "--monochrome-output", "--", filter]);
        jaq
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .spawn()
        .map_err(|e| e.to_string())?;
    let job = match Job::new(MEMORY).and_then(|job| job.assign(&child).map(|_| job)) {
        Ok(job) => job,
        Err(e) => {
            let _ = child.kill();
            return Err(e);
        }
    };
    let me = QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    // The query before this one, if any, ends here.
    *running() = Some((me, job));
    let values = values_of(child.stdout.take().expect("piped"));
    let message = text_of(child.stderr.take().expect("piped"), 16 << 10);
    let mut stdin = child.stdin.take().expect("piped");
    std::thread::spawn(move || {
        // Either way the pipe closes after, so the run sees the end of its input.
        let _ = if as_is {
            serde_json::to_vec(&Ask { path, format }).map_err(std::io::Error::other).and_then(|mut line| {
                line.push(b'\n');
                stdin.write_all(&line)
            })
        } else {
            std::fs::File::open(&path).and_then(|file| std::io::copy(&mut file.take(MAX_INPUT), &mut stdin).map(|_| ()))
        };
    });
    let printed = values.recv_timeout(DEADLINE.saturating_sub(started.elapsed()));
    let stopped = {
        let mut r = running();
        let still = r.as_ref().is_some_and(|(id, _)| *id == me);
        if still {
            // Done, cut or out of time: either way its job closes now, ending the run if it's still going.
            *r = None;
        }
        !still
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    let message = message.recv().unwrap_or_default();
    if stopped {
        return Err("stopped".into());
    }
    let Ok(printed) = printed else {
        return Err(format!("The filter was stopped after {} seconds.", DEADLINE.as_secs()));
    };
    let error = match status.code() {
        _ if printed.truncated => None,
        Some(0) => None,
        // An exit code of its own, with why on its standard error ("Error: cannot index…").
        Some(code) if (1..256).contains(&code) => Some(message.strip_prefix("Error: ").unwrap_or(&message).to_string()),
        // Ended by its job (out of memory), or a stack overflow.
        _ if as_is => return Err("This file needs more memory than a preview has (1 GB), or is nested too deep.".into()),
        _ => return Err("The filter needed more memory than a preview has (1 GB), or went too deep.".into()),
    };
    let head = Head { truncated: printed.truncated, error, elapsed: started.elapsed().as_secs_f64() * 1000.0 };
    let mut answer = serde_json::to_vec(&head).map_err(|e| e.to_string())?;
    answer.push(b'\n');
    answer.extend_from_slice(&printed.values);
    Ok(answer)
}

/// A JSON, YAML or TOML file's documents through a jq filter ("." for the file as it is): a line of
/// JSON saying how it went (truncated, error, elapsed), then the values as a JSON array.
#[tauri::command]
pub async fn jq_query(path: String, format: String, filter: String) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || query(path, format, filter)).await.map_err(|e| e.to_string())?.map(tauri::ipc::Response::new)
}

/// Stops the query running, if any (its answer is "stopped").
#[tauri::command]
pub fn jq_stop() {
    running().take();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &[u8], format: &str) -> Result<String, (String, String)> {
        let dir = std::env::temp_dir().join(format!("glint-jq-{}-{}", std::process::id(), QUERIES.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc");
        std::fs::write(&path, text).unwrap();
        let mut out = Vec::new();
        let read = read_values(&Ask { path: path.to_string_lossy().into(), format: format.into() }, &mut out);
        let _ = std::fs::remove_dir_all(&dir);
        let out = String::from_utf8(out).unwrap();
        read.map(|_| out.clone()).map_err(|e| (out, e))
    }

    #[test]
    fn reads_json_yaml_and_toml_as_lines_of_json() {
        assert_eq!(read(b"{\"a\": [1, 2, {\"b\": null}], \"big\": 123456789012345678901234567890}", "json").unwrap(), "{\"a\":[1,2,{\"b\":null}],\"big\":123456789012345678901234567890}\n");
        assert_eq!(read(b"\xEF\xBB\xBF{\"a\": 1} [2]", "json").unwrap(), "{\"a\":1}\n[2]\n");
        assert_eq!(read(b"name: glint\ntools: [fd, rg]\nbase: &b {x: 1}\nuse: *b\n---\nname: second\n", "yaml").unwrap(), "{\"name\":\"glint\",\"tools\":[\"fd\",\"rg\"],\"base\":{\"x\":1},\"use\":{\"x\":1}}\n{\"name\":\"second\"}\n");
        assert_eq!(read(b"[package]\nname = \"glint\"\nversion = \"0.6.0\"\n", "toml").unwrap(), "{\"package\":{\"name\":\"glint\",\"version\":\"0.6.0\"}}\n");
        // UTF-16, as some Windows tools write JSON.
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("[\"é\"]".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(read(&utf16, "json").unwrap(), "[\"é\"]\n");
    }

    #[test]
    fn says_why_a_file_cant_be_read() {
        let (out, e) = read(b"[1] {", "json").unwrap_err();
        assert_eq!(out, "[1]\n", "the documents before stay");
        assert!(e.starts_with("This file can't be read as JSON"), "{e}");
        assert!(read(b"a: [", "yaml").unwrap_err().1.contains("YAML"));
        assert!(read(b"a = ", "toml").unwrap_err().1.contains("TOML"));
        assert!(read(b"\xFF\xFF", "json").is_err());
    }

    #[test]
    fn makes_jaqs_nan_and_infinity_json() {
        assert_eq!(json_safe("[NaN,Infinity,-Infinity,1]"), "[null,null,null,1]");
        assert_eq!(json_safe(r#"{"NaN":"Infinity \" NaN","x":NaN}"#), r#"{"NaN":"Infinity \" NaN","x":null}"#);
        assert_eq!(json_safe(r#""a\\""#), r#""a\\""#);
        assert_eq!(json_safe("123"), "123");
    }
}
