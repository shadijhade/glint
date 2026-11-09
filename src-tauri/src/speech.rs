//! Speech search (`says:`): recordings and videos found by what's said in them, transcribed on this PC by
//! whisper.cpp (MIT, a tool Glint installs) with Whisper's base model (MIT, 60 MB, any language), both
//! downloaded when speech search is turned on. FFmpeg (another of Glint's tools) reads the sound out of
//! videos and any format; without it, WAV, MP3, FLAC and Ogg files are read as they are.

use crate::media_ai::{self, Library};
use crate::tools::{self, Installs, Progress};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tauri::ipc::Channel;

pub const EXTS: &[&str] = &["mp3", "m4a", "wav", "flac", "ogg", "opus", "aac", "wma", "amr", "mp4", "m4v", "mkv", "mov", "webm", "avi", "wmv", "3gp"];
/// What whisper.cpp reads itself.
const DIRECT: &[&str] = &["wav", "mp3", "flac", "ogg"];

const MODEL: &str = "whisper-base";
const MODEL_FILE: &str = "ggml-base-q5_1.bin";
const MODEL_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
const MODEL_SHA256: &str = "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898";
const MODEL_SIZE: u64 = 59_707_625;

/// What's said in a recording: the language heard, and each part with when it starts (ms).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Transcript {
    #[serde(default)]
    pub lang: String,
    pub parts: Vec<(u64, String)>,
}

pub static LIBRARY: LazyLock<Library<Transcript>> = LazyLock::new(|| Library::new("speech.jsonl"));

fn model() -> Option<PathBuf> {
    tools::data_dir().map(|d| d.join("models").join(MODEL).join(MODEL_FILE))
}

/// The model's file, when it's there (Media studio's subtitles use it too).
pub fn model_path() -> Option<PathBuf> {
    model().filter(|m| fs::metadata(m).is_ok_and(|m| m.len() == MODEL_SIZE))
}

/// Whether whisper.cpp and its model are there.
pub fn installed() -> bool {
    model().is_some_and(|m| fs::metadata(m).is_ok_and(|m| m.len() == MODEL_SIZE)) && tools::command("whisper").is_ok()
}

/// Installs whisper.cpp (Glint's tool) and downloads its model, checked against its SHA-256.
pub async fn install(progress: &Channel<Progress>, installs: &Installs) -> Result<(), String> {
    if tools::command("whisper").is_err() {
        tools::install("whisper", progress.clone(), installs).await?;
    }
    let file = model().ok_or("Glint's data folder isn't available")?;
    if !fs::metadata(&file).is_ok_and(|m| m.len() == MODEL_SIZE) {
        let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/{MODEL_REVISION}/{MODEL_FILE}");
        let dir = file.parent().ok_or("no folder")?.to_path_buf();
        media_ai::fetch_model(&dir, &[(url.as_str(), MODEL_SHA256, MODEL_FILE)], "speech", "the speech model", progress, installs).await?;
    }
    LIBRARY.poke();
    Ok(())
}

/// What's said in a recording or video (the background pass's `learn`).
pub fn learn(path: &Path) -> Result<Transcript, String> {
    let work = std::env::temp_dir().join("Glint").join(format!("speech-{}", std::process::id()));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = transcribe(path, &work);
    let _ = fs::remove_dir_all(&work);
    result
}

fn transcribe(path: &Path, work: &Path) -> Result<Transcript, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    // The sound alone, 16 kHz mono, as Whisper hears.
    let input = match tools::command("ffmpeg") {
        Ok(mut ffmpeg) => {
            let wav = work.join("sound.wav");
            ffmpeg.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(path).args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"]).arg(&wav);
            quiet(&mut ffmpeg);
            let done = ffmpeg.output().map_err(|e| e.to_string())?;
            if !done.status.success() || !wav.exists() {
                return Err("no sound to read".into());
            }
            wav
        }
        Err(_) if DIRECT.contains(&ext.as_str()) => path.to_path_buf(),
        Err(_) => return Err("needs FFmpeg to read its sound".into()),
    };
    let model = model().ok_or("no model")?;
    let out = work.join("said");
    let threads = std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(1, 8));
    let mut whisper = tools::command("whisper")?;
    whisper.arg("-m").arg(&model).arg("-f").arg(&input).args(["-l", "auto", "-oj", "-np", "-t", &threads.to_string()]).arg("-of").arg(&out);
    quiet(&mut whisper);
    let done = whisper.output().map_err(|e| e.to_string())?;
    let json = fs::read(out.with_extension("json")).map_err(|_| format!("whisper.cpp said: {}", String::from_utf8_lossy(&done.stderr).lines().last().unwrap_or("nothing")))?;
    parse(&json)
}

/// whisper.cpp's JSON (-oj): the language, and each part's start (ms) and text.
fn parse(json: &[u8]) -> Result<Transcript, String> {
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let lang = value.pointer("/result/language").and_then(|l| l.as_str()).unwrap_or_default().to_string();
    let parts = value
        .get("transcription")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .filter_map(|part| {
            let text = part.get("text")?.as_str()?.trim().to_string();
            let from = part.pointer("/offsets/from").and_then(|f| f.as_u64()).unwrap_or(0);
            (!text.is_empty() && !text.starts_with('[')).then_some((from, text))
        })
        .collect();
    Ok(Transcript { lang, parts })
}

/// A tool run out of sight, at low priority.
fn quiet(command: &mut std::process::Command) {
    crate::proc::no_window(command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
        command.creation_flags(0x0800_0000 | 0x0000_4000);
    }
}

/// Words as they're matched: lowercase, letters and digits, anything else a space.
fn plain(text: &str) -> String {
    text.chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' }).collect()
}

/// The parts of a transcript that say all of `words` (in one part and the next).
fn hits(transcript: &Transcript, words: &[String]) -> Vec<usize> {
    let parts: Vec<String> = transcript.parts.iter().map(|(_, text)| plain(text)).collect();
    (0..parts.len())
        .filter(|&i| {
            let near = match parts.get(i + 1) {
                Some(next) => format!(" {} {} ", parts[i], next),
                None => format!(" {} ", parts[i]),
            };
            words.iter().all(|w| near.contains(&format!(" {w}")))
        })
        .collect()
}

/// The recordings below `roots` that say `text`: most mentions first, then newest.
pub fn search(text: &str, roots: &[String], max: usize) -> Result<Vec<String>, String> {
    if !media_ai::config().speech || !installed() {
        return Err("Turn on speech search first, in Settings → Photos and recordings".into());
    }
    let words: Vec<String> = plain(text).split_whitespace().map(String::from).collect();
    LIBRARY.load();
    let items = LIBRARY.items.read().unwrap_or_else(|e| e.into_inner());
    let mut found: Vec<(usize, i64, &str)> = items
        .values()
        .filter(|item| media_ai::below(&item.path, roots))
        .filter_map(|item| {
            let count = hits(item.data.as_ref()?, &words).len();
            (count > 0).then_some((count, item.modified, item.path.as_str()))
        })
        .collect();
    if found.is_empty() && !items.values().any(|i| i.data.is_some()) {
        let progress = LIBRARY.progress();
        return Err(if progress.working { format!("Speech search is still listening to the recordings ({} to go)", progress.pending) } else { "Speech search hasn't heard any recordings there: add the folder in Settings → Photos and recordings".into() });
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    Ok(found.into_iter().take(max).map(|(_, _, path)| path.to_string()).collect())
}

/// A recording's transcript, if speech search has heard it, with the parts that say `words` marked: for
/// its preview.
#[derive(Serialize)]
pub struct Heard {
    pub lang: String,
    pub parts: Vec<(u64, String)>,
    pub hits: Vec<usize>,
}

#[tauri::command]
pub async fn speech_transcript(path: String, words: Option<String>) -> Result<Option<Heard>, String> {
    crate::fsx::blocking(move || {
        let Some(key) = media_ai::file_key(Path::new(&path)) else { return Ok(None) };
        LIBRARY.load();
        let items = LIBRARY.items.read().unwrap_or_else(|e| e.into_inner());
        let Some(transcript) = items.get(&key).and_then(|item| item.data.clone()) else { return Ok(None) };
        let words: Vec<String> = words.map(|w| plain(&w).split_whitespace().map(String::from).collect()).unwrap_or_default();
        let hits = if words.is_empty() { Vec::new() } else { hits(&transcript, &words) };
        Ok(Some(Heard { lang: transcript.lang, parts: transcript.parts, hits }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_whisper_heard() {
        let json = br#"{"result":{"language":"en"},"transcription":[{"offsets":{"from":0,"to":3000},"text":" And so, my fellow Americans,"},{"offsets":{"from":3000,"to":5000},"text":" [MUSIC]"},{"offsets":{"from":5000,"to":9000},"text":" ask not what your country can do"}]}"#;
        let heard = parse(json).unwrap();
        assert_eq!(heard.lang, "en");
        assert_eq!(heard.parts, vec![(0, "And so, my fellow Americans,".to_string()), (5000, "ask not what your country can do".to_string())]);
        let words = |text: &str| plain(text).split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(hits(&heard, &words("fellow americans")), [0]);
        assert_eq!(hits(&heard, &words("Americans, ask")), [0], "across a part and the next");
        assert_eq!(hits(&heard, &words("country")), [0, 1]);
        assert!(hits(&heard, &words("mericans")).is_empty(), "words from their start");
    }
}
