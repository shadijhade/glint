//! Photo search (`shows:`): pictures found by what's in them, with OpenAI's CLIP (ViT-B/32, MIT; Xenova's
//! int8 ONNX export) run by ONNX Runtime (MIT), both downloaded when photo search is turned on (about
//! 240 MB with the runtime) and pinned by SHA-256. Each picture's 512 numbers (what CLIP sees in it) are
//! kept under the file's ID (media_ai.rs); a search's words get theirs from CLIP's text side, and the
//! pictures closest to them come first. Pictures are read by Windows' own codecs (WIC), so what Photos
//! opens (HEIC, AVIF and camera RAW with their extensions) is searched too.

use crate::media_ai::{self, Library, STOP};
use crate::tools::{self, Installs, Progress};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tauri::ipc::Channel;

pub const EXTS: &[&str] = &["jpg", "jpeg", "jfif", "png", "webp", "heic", "heif", "avif", "gif", "bmp", "tif", "tiff", "jxl", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2"];

/// ONNX Runtime's CPU build for Windows: the zip on its release, and the DLL in it.
const RUNTIME: &str = "onnxruntime-1.31.1";
const RUNTIME_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-win-x64-1.31.1.zip";
const RUNTIME_SHA256: &str = "5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c";
const RUNTIME_MEMBER: &str = "onnxruntime-win-x64-1.31.1/lib/onnxruntime.dll";

const MODEL: &str = "clip-vit-base-patch32";
/// The model's revision on Hugging Face, and its files there: name, SHA-256, size, and the name kept.
const REVISION: &str = "6b7c523703ae26e39be4b03774966f8571c7f523";
const FILES: &[(&str, &str, u64, &str)] = &[
    ("onnx/vision_model_quantized.onnx", "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299", 89_117_001, "vision.onnx"),
    ("onnx/text_model_quantized.onnx", "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a", 64_504_507, "text.onnx"),
    ("vocab.json", "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363", 862_328, "vocab.json"),
    ("merges.txt", "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a", 524_619, "merges.txt"),
];
/// CLIP's picture side: 224 pixels square, each channel less its mean, over its spread.
const SIDE: u32 = 224;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];
/// CLIP's tokens: start and end of text, and at most 77 in all.
const START: i64 = 49406;
const END: i64 = 49407;
const MAX_TOKENS: usize = 77;

/// What CLIP sees in a picture: its 512 numbers (made length 1), as bytes with their scale.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Seen {
    pub s: f32,
    #[serde(with = "b64")]
    pub v: Vec<i8>,
}

pub static LIBRARY: LazyLock<Library<Seen>> = LazyLock::new(|| Library::new("photos.jsonl"));

fn model_dir() -> Option<PathBuf> {
    tools::data_dir().map(|d| d.join("models").join(MODEL))
}

fn runtime_dll() -> Option<PathBuf> {
    tools::data_dir().map(|d| d.join("models").join(RUNTIME).join("onnxruntime.dll"))
}

/// Whether the runtime and the model are downloaded (each model file as big as pinned).
pub fn installed() -> bool {
    runtime_dll().is_some_and(|dll| dll.exists()) && model_dir().is_some_and(|dir| FILES.iter().all(|(_, _, size, name)| fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size)))
}

/// Downloads ONNX Runtime (its DLL out of its release's zip) and CLIP, each checked against its SHA-256.
pub async fn install(progress: &Channel<Progress>, installs: &Installs) -> Result<(), String> {
    let dll = runtime_dll().ok_or("Glint's data folder isn't available")?;
    if !dll.exists() {
        let stop = installs.begin("photos");
        let zip = tools::fetch(RUNTIME_URL, RUNTIME_SHA256, "onnxruntime", "ONNX Runtime", &stop, progress).await;
        installs.end("photos");
        let zip = zip?;
        let _ = progress.send(Progress { stage: "Unpacking".into(), done: 0, total: 0 });
        let into = dll.parent().map(Path::to_path_buf).ok_or("no folder")?;
        let file = zip.clone();
        let unpacked = crate::fsx::blocking(move || tools::unpack(&file, &[(RUNTIME_MEMBER, "onnxruntime.dll")], &into)).await;
        let _ = fs::remove_file(&zip);
        unpacked?;
    }
    let dir = model_dir().ok_or("Glint's data folder isn't available")?;
    let files: Vec<(String, &str, &str)> = FILES.iter().map(|(name, sha, _, save)| (format!("https://huggingface.co/Xenova/{MODEL}/resolve/{REVISION}/{name}"), *sha, *save)).collect();
    let files: Vec<(&str, &str, &str)> = files.iter().map(|(url, sha, save)| (url.as_str(), *sha, *save)).collect();
    media_ai::fetch_model(&dir, &files, "photos", "the photo search model", progress, installs).await?;
    *CLIP.lock().unwrap_or_else(|e| e.into_inner()) = None;
    LIBRARY.poke();
    Ok(())
}

fn e2s(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// ONNX Runtime, loaded once (one environment a process).
fn environment() -> Result<&'static ort::environment::Environment, String> {
    static ENV: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();
    static INIT: Mutex<()> = Mutex::new(());
    let _one = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let dll = runtime_dll().filter(|d| d.exists()).ok_or("Photo search needs ONNX Runtime: download it in Settings → Photos and recordings")?;
    let env = ort::init_from(&dll).map_err(|e| format!("ONNX Runtime didn't load: {e}"))?.with_name("glint").build().map_err(e2s)?;
    Ok(ENV.get_or_init(|| env))
}

fn session(file: &str, threads: usize) -> Result<ort::session::Session, String> {
    let path = model_dir().ok_or("no data folder")?.join(file);
    ort::session::Session::builder(environment()?)
        .map_err(e2s)?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .map_err(e2s)?
        .with_intra_threads(threads)
        .map_err(e2s)?
        .commit_from_file(path)
        .map_err(e2s)
}

/// CLIP's text side and tokenizer, loaded by the first search.
struct Clip {
    text: Mutex<ort::session::Session>,
    tokenizer: Bpe,
}

static CLIP: Mutex<Option<Arc<Clip>>> = Mutex::new(None);

fn clip() -> Result<Arc<Clip>, String> {
    let mut loaded = CLIP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(clip) = loaded.as_ref() {
        return Ok(clip.clone());
    }
    let dir = model_dir().filter(|_| installed()).ok_or("Photo search needs its model: download it in Settings → Photos and recordings")?;
    let clip = Arc::new(Clip { text: Mutex::new(session("text.onnx", 2)?), tokenizer: Bpe::load(&dir)? });
    *loaded = Some(clip.clone());
    Ok(clip)
}

/// CLIP's picture side, for the background pass (its thread only).
static VISION: Mutex<Option<ort::session::Session>> = Mutex::new(None);

fn length_one(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter().map(|x| x / n).collect()
}

fn quantize(v: &[f32]) -> Seen {
    let s = v.iter().fold(0f32, |m, x| m.max(x.abs())).max(1e-9) / 127.0;
    Seen { s, v: v.iter().map(|x| (x / s).round().clamp(-127.0, 127.0) as i8).collect() }
}

/// What CLIP sees in a picture (the background pass's `learn`).
pub fn learn(path: &Path) -> Result<Seen, String> {
    let pixels = pixels(path)?;
    let mut vision = VISION.lock().unwrap_or_else(|e| e.into_inner());
    if vision.is_none() {
        match session("vision.onnx", 2) {
            Ok(session) => *vision = Some(session),
            Err(e) => {
                LIBRARY.fail(&e);
                return Err(STOP.into());
            }
        }
    }
    let session = vision.as_mut().ok_or(STOP)?;
    let input = ort::value::Tensor::from_array((vec![1i64, 3, SIDE as i64, SIDE as i64], pixels)).map_err(e2s)?;
    let outputs = session.run(ort::inputs!["pixel_values" => input]).map_err(e2s)?;
    let (_, seen) = outputs["image_embeds"].try_extract_tensor::<f32>().map_err(e2s)?;
    Ok(quantize(&length_one(seen)))
}

/// Lets the picture side go once a pass is done (it holds 100 MB).
pub fn rest() {
    *VISION.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// What CLIP's text side makes of words.
fn text_vector(text: &str) -> Result<Vec<f32>, String> {
    let clip = clip()?;
    let ids = clip.tokenizer.encode(text);
    let n = ids.len() as i64;
    let mut session = clip.text.lock().unwrap_or_else(|e| e.into_inner());
    let masked = session.inputs().iter().any(|input| input.name() == "attention_mask");
    let input_ids = ort::value::Tensor::from_array((vec![1i64, n], ids)).map_err(e2s)?;
    let outputs = if masked {
        let mask = ort::value::Tensor::from_array((vec![1i64, n], vec![1i64; n as usize])).map_err(e2s)?;
        session.run(ort::inputs!["input_ids" => input_ids, "attention_mask" => mask]).map_err(e2s)?
    } else {
        session.run(ort::inputs!["input_ids" => input_ids]).map_err(e2s)?
    };
    let (_, vector) = outputs["text_embeds"].try_extract_tensor::<f32>().map_err(e2s)?;
    Ok(length_one(vector))
}

/// What any picture is, to CLIP: how close a picture is to these is how much it's just a picture.
const GENERIC: &[&str] = &["a photo", "a picture", "an image", "a screenshot", "a photo of something"];
static GENERIC_VECTORS: Mutex<Option<Arc<Vec<Vec<f32>>>>> = Mutex::new(None);

fn generic_vectors() -> Result<Arc<Vec<Vec<f32>>>, String> {
    if let Some(vectors) = GENERIC_VECTORS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return Ok(vectors.clone());
    }
    let vectors = Arc::new(GENERIC.iter().map(|text| text_vector(text)).collect::<Result<Vec<_>, _>>()?);
    *GENERIC_VECTORS.lock().unwrap_or_else(|e| e.into_inner()) = Some(vectors.clone());
    Ok(vectors)
}

/// How close (0.25 is close for CLIP) and how much closer than to "a photo" and the like.
fn closeness(seen: &Seen, query: &[f32], generic: &[Vec<f32>]) -> (f32, f32) {
    let dot = |v: &[f32]| seen.v.iter().zip(v).map(|(a, b)| *a as f32 * b).sum::<f32>() * seen.s;
    let close = dot(query);
    (close, close - generic.iter().map(|g| dot(g)).fold(f32::MIN, f32::max))
}

/// What's kept of pictures scored (closeness, how much closer than to a plain picture), closest first:
/// none when even the closest is no closer to the words than to "a photo", or doesn't stand out from
/// the rest; else those about as close as it.
fn kept<T>(mut scored: Vec<(f32, f32, T)>, max: usize) -> Vec<T> {
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let Some(&(best, margin, _)) = scored.first() else { return Vec::new() };
    let median = scored[scored.len() / 2].0;
    if margin < -0.012 || (scored.len() >= 5 && best - median < 0.01) {
        return Vec::new();
    }
    scored.into_iter().take_while(|(close, _, _)| *close >= best - 0.012).filter(|(_, margin, _)| *margin >= -0.015).take(max).map(|(_, _, t)| t).collect()
}

/// The pictures below `roots` that show `text`, closest first.
pub fn search(text: &str, roots: &[String], max: usize) -> Result<Vec<String>, String> {
    let config = media_ai::config();
    if !config.photos || !installed() {
        return Err("Turn on photo search first, in Settings → Photos and recordings".into());
    }
    LIBRARY.load();
    let query = text_vector(text)?;
    let generic = generic_vectors()?;
    let items = LIBRARY.items.read().unwrap_or_else(|e| e.into_inner());
    let scored: Vec<(f32, f32, &str)> = items
        .values()
        .filter(|item| media_ai::below(&item.path, roots))
        .filter_map(|item| {
            let (close, margin) = closeness(item.data.as_ref()?, &query, &generic);
            Some((close, margin, item.path.as_str()))
        })
        .collect();
    if scored.is_empty() {
        let progress = LIBRARY.progress();
        return Err(if progress.working { format!("Photo search is still reading the photos ({} to go)", progress.pending) } else { "Photo search hasn't read any photos there: add the folder in Settings → Photos and recordings".into() });
    }
    Ok(kept(scored, max).into_iter().map(str::to_string).collect())
}

/// A picture as CLIP takes it: its shorter side made 224 (bicubic), the middle 224 square, RGB, each
/// channel normalized, channel by channel. Read by WIC.
#[cfg(windows)]
fn pixels(path: &Path) -> Result<Vec<f32>, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone, WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    thread_local!(static COM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    COM.with(|com| {
        if !com.replace(true) {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        }
    });
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(e2s)?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let decoder = factory.CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).map_err(e2s)?;
        let frame = decoder.GetFrame(0).map_err(e2s)?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height).map_err(e2s)?;
        if width < 16 || height < 16 {
            return Err("too small to see".into());
        }
        let scale = SIDE as f64 / width.min(height) as f64;
        let (w, h) = (((width as f64 * scale).round() as u32).max(SIDE), ((height as f64 * scale).round() as u32).max(SIDE));
        let scaler = factory.CreateBitmapScaler().map_err(e2s)?;
        scaler.Initialize(&frame, w, h, WICBitmapInterpolationModeHighQualityCubic).map_err(e2s)?;
        let converter = factory.CreateFormatConverter().map_err(e2s)?;
        converter.Initialize(&scaler, &GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, None::<&IWICPalette>, 0.0, WICBitmapPaletteTypeCustom).map_err(e2s)?;
        let square = WICRect { X: ((w - SIDE) / 2) as i32, Y: ((h - SIDE) / 2) as i32, Width: SIDE as i32, Height: SIDE as i32 };
        let mut rgb = vec![0u8; (SIDE * SIDE * 3) as usize];
        converter.CopyPixels(&square, SIDE * 3, &mut rgb).map_err(e2s)?;
        Ok(planes(&rgb))
    }
}

#[cfg(not(windows))]
fn pixels(_: &Path) -> Result<Vec<f32>, String> {
    Err("Photo search reads pictures with Windows' codecs".into())
}

/// RGB bytes, row by row, as CLIP's three normalized planes.
fn planes(rgb: &[u8]) -> Vec<f32> {
    let n = (SIDE * SIDE) as usize;
    let mut out = vec![0f32; n * 3];
    for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            out[c * n + i] = (px[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    out
}

/// CLIP's tokenizer: byte-level BPE, words lowercased and split as CLIP splits them, each ending in
/// "</w>".
struct Bpe {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), u32>,
    bytes: [char; 256],
    words: regex::Regex,
}

impl Bpe {
    fn load(dir: &Path) -> Result<Bpe, String> {
        let vocab: HashMap<String, i64> = serde_json::from_slice(&fs::read(dir.join("vocab.json")).map_err(e2s)?).map_err(e2s)?;
        let merges = fs::read_to_string(dir.join("merges.txt")).map_err(e2s)?;
        Ok(Bpe::new(vocab, &merges))
    }

    fn new(vocab: HashMap<String, i64>, merges: &str) -> Bpe {
        // As CLIP's own tokenizer: the merges after the header line, 49,152 - 256 - 2 of them.
        let ranks = merges.lines().skip(1).take(49_152 - 256 - 2).enumerate().filter_map(|(i, line)| line.split_once(' ').map(|(a, b)| ((a.to_string(), b.to_string()), i as u32))).collect();
        let words = regex::Regex::new(r"<\|startoftext\|>|<\|endoftext\|>|'s|'t|'re|'ve|'m|'ll|'d|\p{L}+|\p{N}|[^\s\p{L}\p{N}]+").expect("CLIP's pattern");
        Bpe { vocab, ranks, bytes: bytes_to_unicode(), words }
    }

    fn encode(&self, text: &str) -> Vec<i64> {
        let clean = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let mut ids = vec![START];
        for word in self.words.find_iter(&clean) {
            let token: String = word.as_str().bytes().map(|b| self.bytes[b as usize]).collect();
            ids.extend(self.pieces(&token).iter().filter_map(|piece| self.vocab.get(piece)));
        }
        ids.truncate(MAX_TOKENS - 1);
        ids.push(END);
        ids
    }

    /// A word's pieces: its characters, the last with "</w>", pairs merged lowest rank first.
    fn pieces(&self, token: &str) -> Vec<String> {
        let mut word: Vec<String> = token.chars().map(String::from).collect();
        if let Some(last) = word.last_mut() {
            last.push_str("</w>");
        }
        while word.len() > 1 {
            let Some((_, at)) = word.windows(2).enumerate().filter_map(|(i, pair)| self.ranks.get(&(pair[0].clone(), pair[1].clone())).map(|rank| (*rank, i))).min() else { break };
            let (a, b) = (word[at].clone(), word[at + 1].clone());
            let mut merged = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == a && word[i + 1] == b {
                    merged.push(format!("{a}{b}"));
                    i += 2;
                } else {
                    merged.push(std::mem::take(&mut word[i]));
                    i += 1;
                }
            }
            word = merged;
        }
        word
    }
}

/// GPT-2's map of bytes to printable characters, which CLIP's vocabulary is written in.
fn bytes_to_unicode() -> [char; 256] {
    let mut bytes: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut chars = bytes.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bytes.contains(&b) {
            bytes.push(b);
            chars.push(256 + n);
            n += 1;
        }
    }
    let mut table = ['\0'; 256];
    for (b, c) in bytes.into_iter().zip(chars) {
        table[b as usize] = char::from_u32(c).unwrap_or('?');
    }
    table
}

mod b64 {
    use base64::Engine;

    pub fn serialize<S: serde::Serializer>(v: &[i8], s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = v.iter().map(|&x| x as u8).collect();
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<i8>, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(text).map(|b| b.into_iter().map(|x| x as i8).collect()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_as_clip_does() {
        let vocab: HashMap<String, i64> = [("a</w>", 320), ("dog</w>", 1929), ("d", 67), ("o", 78), ("g</w>", 333), ("do", 1417), ("!</w>", 256)].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let bpe = Bpe::new(vocab, "#version: 0.2\nd o\ndo g</w>\n");
        assert_eq!(bpe.pieces("dog"), ["dog</w>"]);
        assert_eq!(bpe.encode("  A   DOG! "), [START, 320, 1929, 256, END]);
        assert_eq!(bytes_to_unicode()[b' ' as usize], 'Ġ');
        assert_eq!(bytes_to_unicode()[b'a' as usize], 'a');
    }

    #[test]
    fn keeps_what_it_sees_small() {
        let v = length_one(&[3.0, -4.0, 0.0]);
        let seen = quantize(&v);
        let back: Vec<f32> = seen.v.iter().map(|&x| x as f32 * seen.s).collect();
        assert!(back.iter().zip(&v).all(|(a, b)| (a - b).abs() < 0.01), "{back:?}");
        let json = serde_json::to_string(&seen).unwrap();
        let again: Seen = serde_json::from_str(&json).unwrap();
        assert_eq!(again.v, seen.v);
    }

    /// With the model downloaded (Settings → Photos and recordings): a picture of red is closer to "red"
    /// than to "blue", and the real tokenizer gives CLIP's ids.
    #[test]
    fn sees_what_a_picture_shows() {
        if !installed() {
            return;
        }
        let tokenizer = Bpe::load(&model_dir().unwrap()).unwrap();
        assert_eq!(tokenizer.encode("a photo of a dog"), [START, 320, 1125, 539, 320, 1929, END]);
        let dir = std::env::temp_dir().join(format!("glint-photos-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let red = dir.join("red.bmp");
        fs::write(&red, bmp(64, 64, [220, 20, 20])).unwrap();
        let seen = learn(&red).unwrap();
        let score = |text: &str| {
            let q = text_vector(text).unwrap();
            seen.v.iter().zip(&q).map(|(a, b)| *a as f32 * b).sum::<f32>() * seen.s
        };
        assert!(score("a red square") > score("a blue square"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Scores of what photo search has read, and what it keeps, for tuning: GLINT_DATA names a Glint data
    /// folder (`cargo test --lib -- --ignored prints_scores --nocapture`).
    #[test]
    #[ignore]
    fn prints_scores() {
        let Ok(data) = std::env::var("GLINT_DATA") else { return };
        crate::tools::set_dir(std::path::PathBuf::from(data).join("tools"));
        LIBRARY.load();
        let generic = generic_vectors().unwrap();
        for query in ["red", "a receipt", "mountains and a lake", "a blue square", "text on paper", "a dog", "a city at night", "a sunset", "water"] {
            let q = text_vector(query).unwrap();
            let items = LIBRARY.items.read().unwrap();
            let name = |path: &str| path.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or(path).to_string();
            let scored: Vec<(f32, f32, String)> = items.values().filter_map(|i| i.data.as_ref().map(|d| (closeness(d, &q, &generic), name(&i.path)))).map(|((c, m), n)| (c, m, n)).collect();
            let mut shown = scored.clone();
            shown.sort_by(|a, b| b.0.total_cmp(&a.0));
            eprintln!("{query:22} kept {:?} of {}", kept(scored, 100), shown.iter().map(|(c, m, n)| format!("{n} {c:.3}/{m:+.3}")).collect::<Vec<_>>().join(" | "));
        }
    }

    #[test]
    fn keeps_what_stands_out() {
        // Measured with CLIP on nine test pictures (prints_scores): closeness, and how much closer than to "a photo".
        let red = vec![(0.276, 0.044, "red"), (0.237, -0.009, "screen2"), (0.230, -0.002, "screen5"), (0.227, -0.008, "blue"), (0.226, -0.013, "screen3"), (0.218, -0.023, "screen1"), (0.214, -0.021, "screen0"), (0.202, -0.014, "receipt"), (0.199, -0.030, "screen4")];
        assert_eq!(kept(red, 100), ["red"]);
        let lake = vec![(0.236, -0.010, "screen2"), (0.221, -0.008, "screen4"), (0.200, -0.033, "screen5"), (0.198, -0.037, "blue"), (0.191, -0.048, "screen3"), (0.190, -0.042, "red"), (0.182, -0.059, "screen1"), (0.181, -0.054, "screen0"), (0.158, -0.058, "receipt")];
        assert_eq!(kept(lake, 100), ["screen2"]);
        let blue = vec![(0.290, 0.056, "blue"), (0.286, 0.053, "screen5"), (0.266, 0.031, "screen0"), (0.265, 0.036, "screen4"), (0.264, 0.023, "screen1"), (0.234, 0.002, "red"), (0.216, -0.030, "screen2"), (0.211, -0.028, "screen3"), (0.200, -0.016, "receipt")];
        assert_eq!(kept(blue.clone(), 1), ["blue"], "at most max");
        assert_eq!(kept(blue, 100), ["blue", "screen5"]);
        let dog = vec![(0.215, -0.018, "screen5"), (0.214, -0.020, "blue"), (0.213, -0.019, "red"), (0.210, -0.037, "screen2"), (0.210, -0.029, "screen3"), (0.205, -0.036, "screen1"), (0.200, -0.016, "receipt"), (0.197, -0.039, "screen0"), (0.192, -0.037, "screen4")];
        assert!(kept(dog, 100).is_empty(), "no closer to a dog than to a photo");
        let paper = vec![(0.228, -0.004, "screen5"), (0.227, -0.005, "red"), (0.227, -0.008, "blue"), (0.226, 0.010, "receipt"), (0.225, -0.014, "screen3"), (0.222, -0.013, "screen0"), (0.221, -0.020, "screen1"), (0.216, -0.013, "screen4"), (0.211, -0.036, "screen2")];
        assert!(kept(paper, 100).is_empty(), "nothing stands out");
        assert!(kept(Vec::<(f32, f32, &str)>::new(), 10).is_empty());
    }

    /// A plain 24-bit BMP of one color.
    fn bmp(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let size = 54 + row * h;
        let mut out = Vec::new();
        out.extend(b"BM");
        out.extend(size.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(54u32.to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend(w.to_le_bytes());
        out.extend(h.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(24u16.to_le_bytes());
        out.extend([0u8; 24]);
        for _ in 0..h {
            let mut line: Vec<u8> = (0..w).flat_map(|_| [rgb[2], rgb[1], rgb[0]]).collect();
            line.resize(row as usize, 0);
            out.extend(line);
        }
        out
    }
}
