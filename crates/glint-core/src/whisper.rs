//! Whisper for Media's Subtitles: whisper.cpp (MIT, a tool Glint installs) with Whisper's base model (MIT, 60 MB,
//! any language), downloaded the first time Subtitles are made and checked against its pinned SHA-256. (Speech
//! search used the same two until 0.12; the model stays where it put it.)

use crate::tools::{self, Installs, Progress};
use std::fs;
use std::path::{Path, PathBuf};
use crate::host::ipc::Channel;
use crate::host::State;

const MODEL: &str = "whisper-base";
const MODEL_FILE: &str = "ggml-base-q5_1.bin";
const MODEL_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
const MODEL_SHA256: &str = "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898";
const MODEL_SIZE: u64 = 59_707_625;

fn model() -> Option<PathBuf> {
    tools::data_dir().map(|d| d.join("models").join(MODEL).join(MODEL_FILE))
}

/// The model's file, when it's there whole.
pub fn model_path() -> Option<PathBuf> {
    model().filter(|m| fs::metadata(m).is_ok_and(|m| m.len() == MODEL_SIZE))
}

/// Whether whisper.cpp and its model are there.
pub fn installed() -> bool {
    model_path().is_some() && tools::command("whisper").is_ok()
}

/// Whether whisper.cpp and its model are there, off the UI thread.
pub async fn whisper_installed() -> Result<bool, String> {
    crate::fsx::blocking(|| Ok(installed())).await
}

/// Installs whisper.cpp and downloads its model, telling `progress` how far it is; whether both are there now.
pub async fn whisper_install(progress: Channel<Progress>, installs: State<'_, Installs>) -> Result<bool, String> {
    if tools::command("whisper").is_err() {
        tools::install("whisper", progress.clone(), &installs).await?;
    }
    let file = model().ok_or("Glint's data folder isn't available")?;
    if !fs::metadata(&file).is_ok_and(|m| m.len() == MODEL_SIZE) {
        let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/{MODEL_REVISION}/{MODEL_FILE}");
        let dir = file.parent().ok_or("no folder")?.to_path_buf();
        fetch_model(&dir, &url, &progress, &installs).await?;
    }
    whisper_installed().await
}

/// The model's file from Hugging Face, checked against its pinned SHA-256, into a fresh folder that then replaces
/// `dir`.
async fn fetch_model(dir: &Path, url: &str, progress: &Channel<Progress>, installs: &Installs) -> Result<(), String> {
    let fresh = dir.with_extension("new");
    let _ = fs::remove_dir_all(&fresh);
    fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    let id = "whisper-model";
    let stop = installs.begin(id);
    let result = match tools::fetch(url, MODEL_SHA256, &format!("{id}-{MODEL_FILE}"), "Whisper's model", &stop, progress).await {
        Ok(file) => {
            let moved = fs::rename(&file, fresh.join(MODEL_FILE)).or_else(|_| fs::copy(&file, fresh.join(MODEL_FILE)).map(drop));
            let _ = fs::remove_file(&file);
            moved.map_err(|e| e.to_string())
        }
        Err(e) => Err(e),
    };
    installs.end(id);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&fresh);
        return Err(e);
    }
    let _ = fs::remove_dir_all(dir);
    fs::rename(&fresh, dir).map_err(|e| e.to_string())
}
