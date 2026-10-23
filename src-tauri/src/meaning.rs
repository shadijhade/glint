//! Meaning search (Text's Meaning, a Documents search): passages of documents and notes ranked by how
//! close their meaning is to what's typed, not by its words, with Model2Vec's potion-base-8M (MIT): static
//! embeddings, each token's vector looked up and averaged, microseconds a passage. Its model (30 MB) is
//! downloaded from Hugging Face the first time it's wanted, pinned by revision and SHA-256, into Glint's
//! data folder; its tokenizer, BERT's WordPiece as the model's tokenizer.json sets it up, runs here.

use crate::tools::{self, Installs, Progress};
use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::State;
use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

const MODEL: &str = "potion-base-8M";
/// The model's revision on Hugging Face, and its files there: name, SHA-256 and size.
const REVISION: &str = "bf8b056651a2c21b8d2565580b8569da283cab23";
const FILES: &[(&str, &str, u64)] = &[
    ("vocab.txt", "1394523a67ddd404a825428018c0582a6998bcfa044ecbcbf1f4d71adb94c61c", 219_690),
    ("model.safetensors", "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2", 30_236_760),
];
/// A passage's tokens past this aren't read (as Model2Vec's encode stops).
const MAX_TOKENS: usize = 512;
/// A word longer than this is unknown (WordPiece's max_input_chars_per_word).
const MAX_WORD: usize = 100;

fn folder() -> Option<PathBuf> {
    tools::data_dir().map(|d| d.join("models").join(MODEL))
}

/// Whether the model is downloaded (each file there, as big as pinned).
pub fn installed() -> bool {
    folder().is_some_and(|dir| FILES.iter().all(|(name, _, size)| fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size)))
}

/// The model in memory: its vocabulary (token → row) and a row of `dim` numbers per token.
pub struct Model {
    vocab: HashMap<String, u32>,
    unknown: u32,
    dim: usize,
    vectors: Vec<f32>,
}

/// Loaded once, the first time a search wants it.
static LOADED: Mutex<Option<Arc<Model>>> = Mutex::new(None);

/// The model, read from Glint's data folder the first time.
pub fn model() -> Result<Arc<Model>, String> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(model) = loaded.as_ref() {
        return Ok(model.clone());
    }
    let dir = folder().filter(|_| installed()).ok_or("Meaning search needs its model: download it from the card above the results")?;
    let model = Arc::new(Model::load(&dir)?);
    *loaded = Some(model.clone());
    Ok(model)
}

impl Model {
    fn load(dir: &Path) -> Result<Model, String> {
        let text = fs::read_to_string(dir.join("vocab.txt")).map_err(|e| e.to_string())?;
        let vocab: HashMap<String, u32> = text.lines().enumerate().map(|(i, token)| (token.to_string(), i as u32)).collect();
        let unknown = *vocab.get("[UNK]").ok_or("the model's vocabulary has no [UNK]")?;
        let (rows, dim, vectors) = embeddings(&fs::read(dir.join("model.safetensors")).map_err(|e| e.to_string())?)?;
        if rows < vocab.len() {
            return Err("the model's vectors don't cover its vocabulary".into());
        }
        Ok(Model { vocab, unknown, dim, vectors })
    }

    /// A text's tokens, as Model2Vec reads them: BERT's normalizer (control characters out, Chinese
    /// characters on their own, accents off, lowercase), words split at spaces and punctuation, then
    /// WordPiece; unknown tokens left out, at most `MAX_TOKENS`.
    pub fn tokens(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        let mut piece = String::new();
        for word in words(&normalize(text)) {
            self.word_pieces(word, &mut piece, &mut ids);
            // A word unknown whole is one [UNK], which Model2Vec leaves out before it counts.
            if ids.last() == Some(&self.unknown) {
                ids.pop();
            }
            if ids.len() >= MAX_TOKENS {
                break;
            }
        }
        ids.truncate(MAX_TOKENS);
        ids
    }

    /// WordPiece: the longest pieces in the vocabulary, left to right, the ones after the first with
    /// "##"; a word with any part not there is unknown as a whole.
    fn word_pieces(&self, word: &str, piece: &mut String, ids: &mut Vec<u32>) {
        if word.chars().count() > MAX_WORD {
            ids.push(self.unknown);
            return;
        }
        let first = ids.len();
        let mut start = 0;
        while start < word.len() {
            let mut end = word.len();
            let found = loop {
                piece.clear();
                if start > 0 {
                    piece.push_str("##");
                }
                piece.push_str(&word[start..end]);
                if let Some(&id) = self.vocab.get(piece.as_str()) {
                    break Some(id);
                }
                // One character shorter.
                match word[start..end].char_indices().next_back() {
                    Some((at, _)) if at > 0 => end = start + at,
                    _ => break None,
                }
            };
            let Some(id) = found else {
                ids.truncate(first);
                ids.push(self.unknown);
                return;
            };
            ids.push(id);
            start = end;
        }
    }

    /// A text's meaning as a vector of length 1: its tokens' rows averaged (and scaled); None when it has
    /// no token the model knows.
    pub fn embed(&self, text: &str) -> Option<Vec<f32>> {
        let ids = self.tokens(text);
        if ids.is_empty() {
            return None;
        }
        let mut sum = vec![0f32; self.dim];
        for id in ids {
            let row = &self.vectors[id as usize * self.dim..(id as usize + 1) * self.dim];
            sum.iter_mut().zip(row).for_each(|(s, v)| *s += v);
        }
        let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
        (norm > 0.0).then(|| sum.into_iter().map(|v| v / norm).collect())
    }
}

/// How alike two meanings are: 1 the same, 0 unrelated (cosine; the vectors are of length 1).
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The `embeddings` table of a safetensors file: its rows, their length, and the numbers (F32).
fn embeddings(file: &[u8]) -> Result<(usize, usize, Vec<f32>), String> {
    let bad = || "the model file isn't a safetensors file Glint reads".to_string();
    let header_len = u64::from_le_bytes(file.get(..8).ok_or_else(bad)?.try_into().map_err(|_| bad())?) as usize;
    let header: serde_json::Value = serde_json::from_slice(file.get(8..8 + header_len).ok_or_else(bad)?).map_err(|_| bad())?;
    let table = &header["embeddings"];
    let shape: Vec<usize> = table["shape"].as_array().ok_or_else(bad)?.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect();
    let offsets: Vec<usize> = table["data_offsets"].as_array().ok_or_else(bad)?.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect();
    let (&[rows, dim], &[from, to]) = (shape.as_slice(), offsets.as_slice()) else { return Err(bad()) };
    if table["dtype"] != "F32" || to - from != rows * dim * 4 {
        return Err(bad());
    }
    let data = file.get(8 + header_len + from..8 + header_len + to).ok_or_else(bad)?;
    Ok((rows, dim, data.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()))
}

/// CJK ideographs, which BERT's normalizer puts spaces around.
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F | 0x2B740..=0x2B81F | 0x2B820..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}

/// Punctuation as BERT splits at it: ASCII's, and Unicode's (its P categories).
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || c.general_category_group() == GeneralCategoryGroup::Punctuation
}

/// BERT's normalizer, as the model's tokenizer.json sets it: control, formatting, private-use and
/// surrogate characters out (a zero-width space joins the words either side, as it shows; unassigned
/// ones stay, as Hugging Face's tokenizers keeps them), tabs and line breaks as spaces, Chinese
/// characters spaced apart, accents off (only the marks that take no space: a Devanagari vowel sign
/// stays), lowercase.
fn normalize(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.chars() {
        let line = matches!(c, '\t' | '\n' | '\r');
        let invisible = matches!(c.general_category(), GeneralCategory::Control | GeneralCategory::Format | GeneralCategory::PrivateUse | GeneralCategory::Surrogate);
        if c == '\0' || c == '\u{FFFD}' || (!line && invisible) {
            continue;
        }
        if line || c.is_whitespace() {
            cleaned.push(' ');
        } else if is_cjk(c) {
            cleaned.extend([' ', c, ' ']);
        } else {
            cleaned.push(c);
        }
    }
    cleaned.nfd().filter(|c| c.general_category() != GeneralCategory::NonspacingMark).flat_map(char::to_lowercase).collect()
}

/// BERT's pre-tokenizer: words between spaces, and each punctuation character a word of its own.
fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        if c == ' ' || is_punctuation(c) {
            if let Some(from) = start.take() {
                words.push(&text[from..at]);
            }
            if c != ' ' {
                words.push(&text[at..at + c.len_utf8()]);
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        words.push(&text[from..]);
    }
    words
}

/// A passage of a document: its lines there (from 1), its first line's place and its text.
pub type Passage = (Range<usize>, String, String);

/// A document's lines as passages to rank: consecutive lines together, about a paragraph's worth (a new
/// one at 400 characters, or once there are 150 at a new page or slide or the end of a sentence).
pub fn passages(lines: &[(String, String)]) -> Vec<Passage> {
    let mut passages: Vec<Passage> = Vec::new();
    for (i, (label, text)) in lines.iter().enumerate() {
        match passages.last_mut() {
            Some((at, first, passage)) if passage.len() < 150 || (passage.len() < 400 && first == label && !passage.trim_end().ends_with(['.', '!', '?', '。', '！', '？'])) => {
                passage.push(' ');
                passage.push_str(text);
                at.end = i + 2;
            }
            _ => passages.push((i + 1..i + 2, label.clone(), text.clone())),
        }
    }
    passages
}

/// Downloads the model (each file checked against its pinned SHA-256), into a fresh folder that then
/// replaces any old one.
#[tauri::command]
pub async fn meaning_install(on_progress: Channel<Progress>, installs: State<'_, Installs>) -> Result<(), String> {
    let dir = folder().ok_or("Glint's data folder isn't available")?;
    let fresh = dir.with_extension("new");
    let _ = fs::remove_dir_all(&fresh);
    fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    let stop = installs.begin("meaning");
    let mut result = Ok(());
    for (name, sha256, _) in FILES {
        let url = format!("https://huggingface.co/minishlab/{MODEL}/resolve/{REVISION}/{name}");
        match tools::fetch(&url, sha256, &format!("meaning-{name}"), "the meaning model", &stop, &on_progress).await {
            Ok(file) => {
                let moved = fs::rename(&file, fresh.join(name)).or_else(|_| fs::copy(&file, fresh.join(name)).map(|_| ()));
                let _ = fs::remove_file(&file);
                if let Err(e) = moved {
                    result = Err(e.to_string());
                    break;
                }
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    installs.end("meaning");
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&fresh);
        return Err(e);
    }
    let _ = fs::remove_dir_all(&dir);
    fs::rename(&fresh, &dir).map_err(|e| e.to_string())?;
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

/// Whether the model is downloaded.
#[tauri::command]
pub fn meaning_status() -> bool {
    installed()
}

/// Deletes the downloaded model.
#[tauri::command]
pub fn meaning_remove() -> Result<(), String> {
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    match folder().map(fs::remove_dir_all) {
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model of a few tokens, with two-number vectors that say what they're about.
    fn tiny() -> Model {
        let tokens = ["[PAD]", "[UNK]", "cat", "##s", "kitten", "spread", "##sheet", "cafe", ",", "!", "un", "##aff", "##able"];
        let vectors = [[0., 0.], [0., 0.], [1., 0.], [0.9, 0.1], [1., 0.1], [0., 1.], [0.1, 1.], [0.5, 0.5], [0., 0.], [0., 0.], [0.2, 0.], [0.2, 0.], [0.2, 0.]];
        Model {
            vocab: tokens.iter().enumerate().map(|(i, t)| (t.to_string(), i as u32)).collect(),
            unknown: 1,
            dim: 2,
            vectors: vectors.iter().flatten().copied().collect(),
        }
    }

    #[test]
    fn tokenizes_as_bert_does() {
        let model = tiny();
        let names = |text: &str| model.tokens(text).iter().map(|&id| model.vocab.iter().find(|(_, &v)| v == id).unwrap().0.clone()).collect::<Vec<_>>();
        assert_eq!(names("Cats, kitten!"), ["cat", "##s", ",", "kitten", "!"]);
        assert_eq!(names("unaffable"), ["un", "##aff", "##able"]);
        assert_eq!(names("Café"), ["cafe"], "accents off, lowercase");
        assert_eq!(names("dog cat"), ["cat"], "unknown words are left out");
        assert_eq!(names("catz"), Vec::<String>::new(), "a word with a part not known is unknown whole");
        assert_eq!(words("a,b  c!"), ["a", ",", "b", "c", "!"]);
        assert_eq!(normalize("A\u{7}b\tÉ中"), "ab e 中 ");
    }

    #[test]
    fn close_meanings_are_closer() {
        let model = tiny();
        let cat = model.embed("cats").unwrap();
        assert!((similarity(&cat, &cat) - 1.0).abs() < 1e-5);
        assert!(similarity(&cat, &model.embed("kitten").unwrap()) > similarity(&cat, &model.embed("spreadsheet").unwrap()));
        assert!(model.embed("dog").is_none(), "nothing known");
    }

    /// The real model, from a folder named by GLINT_MEANING_DIR holding its vocab.txt and model.safetensors,
    /// and reference.json: sentences and their token ids from Hugging Face's own tokenizer (without special
    /// tokens, [UNK] left out). Run with --ignored --nocapture.
    #[test]
    #[ignore]
    fn tokenizes_as_hugging_face_does_and_ranks_meanings() {
        let dir = PathBuf::from(std::env::var("GLINT_MEANING_DIR").expect("GLINT_MEANING_DIR"));
        let model = Model::load(&dir).unwrap();
        let reference: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("reference.json")).unwrap()).unwrap();
        let mut differ = 0;
        let mut seen = Vec::new();
        for (index, case) in reference.as_array().unwrap().iter().enumerate() {
            let text = case["text"].as_str().unwrap();
            // Punctuation only since Unicode 14, which Hugging Face's character tables are older than.
            if text.contains('\u{61D}') {
                continue;
            }
            let mut expected: Vec<u32> = case["ids"].as_array().unwrap().iter().map(|id| id.as_u64().unwrap() as u32).collect();
            // Model2Vec reads a passage's first 512 tokens.
            expected.truncate(MAX_TOKENS);
            let got = model.tokens(text);
            if got != expected {
                differ += 1;
                println!("DIFFERS {text:?}\n  hugging face {expected:?}\n  glint        {got:?}");
                seen.push(serde_json::json!({ "index": index, "normalized": normalize(text), "words": words(&normalize(text)) }));
            }
        }
        fs::write(dir.join("glint-differs.json"), serde_json::to_vec(&seen).unwrap()).unwrap();
        println!("{} of {} sentences tokenized the same", reference.as_array().unwrap().len() - differ, reference.as_array().unwrap().len());
        let alike = |a: &str, b: &str| similarity(&model.embed(a).unwrap(), &model.embed(b).unwrap());
        for (a, b) in [("cat", "kitten"), ("cat", "spreadsheet"), ("invoice", "bill to pay"), ("invoice", "holiday photos"), ("how to reset my password", "forgot login credentials"), ("how to reset my password", "quarterly revenue grew")] {
            println!("{a:?} ~ {b:?}: {:.2}", alike(a, b));
        }
        assert_eq!(differ, 0);
        assert!(alike("cat", "kitten") > alike("cat", "spreadsheet"));
    }

    #[test]
    fn reads_safetensors_and_makes_passages() {
        let mut file = Vec::new();
        let header = br#"{"embeddings":{"dtype":"F32","shape":[2,3],"data_offsets":[0,24]}}"#;
        file.extend((header.len() as u64).to_le_bytes());
        file.extend(header);
        file.extend([1f32, 2., 3., 4., 5., 6.].iter().flat_map(|f| f.to_le_bytes()));
        assert_eq!(embeddings(&file).unwrap(), (2, 3, vec![1., 2., 3., 4., 5., 6.]));
        assert!(embeddings(&file[..20]).is_err());
        assert!(embeddings(b"not a model").is_err());

        let line = |label: &str, text: &str| (label.to_string(), text.to_string());
        let (long, middling) = ("x".repeat(396), "y".repeat(200));
        let lines = [line("p. 1", "Title"), line("p. 1", &long), line("p. 1", "more"), line("p. 2", &middling), line("p. 3", "Next"), line("¶ 9", "short")];
        let made = passages(&lines);
        // Title and the long line make 402 characters: "more" starts another, which takes page 2 (it's short)
        // and is long enough for page 3 to start one of its own.
        assert_eq!(made.iter().map(|(at, label, _)| (at.clone(), label.as_str())).collect::<Vec<_>>(), [(1..3, "p. 1"), (3..5, "p. 1"), (5..7, "p. 3")]);
        assert!(made[2].2.ends_with("short"), "a short paragraph joins the passage before it");
        let page = [line("p. 4", &format!("{}.", "z".repeat(160))), line("p. 4", "Another"), line("p. 4", "and its end")];
        assert_eq!(passages(&page).into_iter().map(|(at, ..)| at).collect::<Vec<_>>(), [1..2, 2..4], "one long enough ends with its sentence");
    }
}
