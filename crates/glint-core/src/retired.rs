//! What features 0.12 removed left on users' PCs: photo, speech and meaning search's downloads and what they
//! learned, paired devices' key and pairings, and the built-in Typst's previews. [`sweep`] deletes exactly those,
//! once per start, on a background thread (startup.rs in glint-ui), and nothing else. Whisper's model
//! (`models\whisper-base`) stays: Media's Subtitles still uses it (whisper.rs).

use std::fs;
use std::path::{Path, PathBuf};

/// Under Glint's local data folder ([`crate::tools::data_dir`]). A `.new` is a download that was being put in
/// place (`Path::with_extension("new")`, so ONNX Runtime's is `onnxruntime-1.31.new`) or a library being
/// saved when Glint stopped.
const RETIRED: &[&str] = &[
    // Photo search (photos.rs): OpenAI's CLIP and ONNX Runtime, about 240 MB.
    "models/clip-vit-base-patch32",
    "models/clip-vit-base-patch32.new",
    "models/onnxruntime-1.31.1",
    "models/onnxruntime-1.31.new",
    // Meaning search (meaning.rs, multilingual.rs): Model2Vec's potion models, 30 and 128 MB.
    "models/potion-base-8M",
    "models/potion-base-8M.new",
    "models/potion-multilingual-128M",
    "models/potion-multilingual-128M.new",
    // What photo and speech search learned of each file (media_ai.rs), and their settings.
    "photos.jsonl",
    "photos.new",
    "speech.jsonl",
    "speech.new",
    "media-ai.json",
    // Paired devices (devices.rs): this PC's secret key and the PCs paired with it.
    "devices.json",
    // The built-in Typst's pictures of .typ files' pages (docconvert.rs).
    "typst-previews",
];

/// Deletes what the removed features left in Glint's data folder, if it's there.
pub fn sweep() {
    if let Some(data) = crate::tools::data_dir() {
        sweep_in(&data);
    }
}

/// Deletes [`RETIRED`]'s paths under `data`; returns those it deleted. A link is deleted, never what it points to.
fn sweep_in(data: &Path) -> Vec<PathBuf> {
    let mut gone = Vec::new();
    for relative in RETIRED {
        let path = relative
            .split('/')
            .fold(data.to_path_buf(), |path, part| path.join(part));
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let removed = if meta.file_type().is_symlink() {
            fs::remove_dir(&path).or_else(|_| fs::remove_file(&path))
        } else if meta.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if removed.is_ok() {
            gone.push(path);
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_what_the_removed_features_left_and_nothing_else() {
        let data = std::env::temp_dir().join(format!("glint-retired-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let models = data.join("models");
        for folder in [
            "clip-vit-base-patch32",
            "onnxruntime-1.31.1",
            "potion-base-8M",
            "potion-multilingual-128M.new",
        ] {
            fs::create_dir_all(models.join(folder).join("sub")).unwrap();
            fs::write(models.join(folder).join("sub").join("weights.bin"), "w").unwrap();
        }
        fs::create_dir_all(data.join("typst-previews").join("ab12")).unwrap();
        fs::write(data.join("typst-previews").join("ab12").join("1.png"), "p").unwrap();
        for file in ["photos.jsonl", "speech.jsonl", "media-ai.json", "devices.json"] {
            fs::write(data.join(file), "{}").unwrap();
        }
        // What stays: Subtitles' model, the tools, the state, the content index, a model of someone else's.
        fs::create_dir_all(models.join("whisper-base")).unwrap();
        fs::write(models.join("whisper-base").join("ggml-base-q5_1.bin"), "m").unwrap();
        fs::create_dir_all(models.join("mine")).unwrap();
        fs::create_dir_all(data.join("tools").join("jaq")).unwrap();
        fs::create_dir_all(data.join("content")).unwrap();
        fs::write(data.join("state.json"), "{}").unwrap();
        fs::write(data.join("ondemand.json"), "[]").unwrap();

        let gone = sweep_in(&data);

        assert_eq!(gone.len(), 9, "{gone:?}");
        for relative in RETIRED {
            assert!(!data.join(relative).exists(), "{relative} is still there");
        }
        for kept in [
            "models/whisper-base/ggml-base-q5_1.bin",
            "models/mine",
            "tools/jaq",
            "content",
            "state.json",
            "ondemand.json",
        ] {
            assert!(data.join(kept).exists(), "{kept} went");
        }
        assert!(sweep_in(&data).is_empty(), "a second start has nothing to delete");
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_data_folder_without_them_is_left_as_it_is() {
        let data = std::env::temp_dir().join(format!("glint-retired-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        assert!(sweep_in(&data).is_empty(), "no data folder at all");
        fs::create_dir_all(data.join("models")).unwrap();
        assert!(sweep_in(&data).is_empty());
        assert!(data.join("models").is_dir());
        fs::remove_dir_all(&data).unwrap();
    }
}
