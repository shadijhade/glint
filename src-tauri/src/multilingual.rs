//! Meaning search in 101 languages: Model2Vec's potion-multilingual-128M (MIT), distilled from BGE-M3,
//! as an option beside the English model (meaning.rs). Its tokenizer is a Unigram one (SentencePiece's):
//! the text normalized (NFKC, spaces around ASCII punctuation), each word with "▁" in front, then cut
//! into the most likely pieces of its vocabulary. Its 500,000 rows of 256 numbers are kept as 8-bit
//! numbers with a scale per row (128 MB in memory rather than 512), which ranks passages the same.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const MODEL: &str = "potion-multilingual-128M";
pub const REVISION: &str = "73908c3438cf03b6a01bcb9611d62b23d0726f08";
pub const FILES: &[(&str, &str, u64)] = &[
    ("tokenizer.json", "19f1909063da3cfe3bd83a782381f040dccea475f4816de11116444a73e1b6a1", 18_616_131),
    ("model.safetensors", "14b5eb39cb4ce5666da8ad1f3dc6be4346e9b2d601c073302fa0a31bf7943397", 512_361_560),
];

pub fn source(name: &str) -> String {
    format!("https://huggingface.co/minishlab/{MODEL}/resolve/{REVISION}/{name}")
}

pub fn folder() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("models").join(MODEL))
}

pub fn installed() -> bool {
    folder().is_some_and(|dir| FILES.iter().all(|(name, _, size)| fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size)))
}

/// The tokenizer: each piece's id and score, and the longest piece (in characters).
pub struct Unigram {
    pieces: HashMap<String, (u32, f32)>,
    max_chars: usize,
    unknown: u32,
}

impl Unigram {
    pub fn load(path: &Path) -> Result<Unigram, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("the tokenizer can't be read: {e}"))?;
        let model = &json["model"];
        if model["type"] != "Unigram" {
            return Err("the tokenizer isn't a Unigram one".into());
        }
        let unknown = model["unk_id"].as_u64().unwrap_or(1) as u32;
        let mut pieces = HashMap::new();
        let mut max_chars = 1;
        for (i, entry) in model["vocab"].as_array().ok_or("the tokenizer has no vocabulary")?.iter().enumerate() {
            let (Some(piece), Some(score)) = (entry[0].as_str(), entry[1].as_f64()) else { continue };
            max_chars = max_chars.max(piece.chars().count());
            pieces.insert(piece.to_string(), (i as u32, score as f32));
        }
        Ok(Unigram { pieces, max_chars: max_chars.min(32), unknown })
    }

    /// The tokenizer's normalizer: NFKC (what its precompiled map does, near enough), ASCII punctuation
    /// spaced apart, whitespace run together, the ends trimmed.
    fn normalize(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 16);
        for c in text.nfkc() {
            if c.is_ascii_punctuation() {
                out.push(' ');
                out.push(c);
                out.push(' ');
            } else if c.is_whitespace() || c.is_control() {
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// A text's pieces: each word ("▁" in front) cut by Viterbi into its most likely pieces; unknown
    /// ones left out (as Model2Vec leaves out the unknown token), at most `max`.
    pub fn tokens(&self, text: &str, max: usize) -> Vec<u32> {
        let mut ids = Vec::new();
        for word in Unigram::normalize(text).split(' ').filter(|w| !w.is_empty()) {
            let word: Vec<char> = std::iter::once('▁').chain(word.chars()).collect();
            let n = word.len();
            // best[i]: the best score of the text up to i, and where its last piece starts.
            let mut best: Vec<(f32, usize, u32)> = vec![(f32::NEG_INFINITY, 0, self.unknown); n + 1];
            best[0].0 = 0.0;
            let mut piece = String::new();
            for end in 1..=n {
                for start in end.saturating_sub(self.max_chars)..end {
                    if best[start].0 == f32::NEG_INFINITY {
                        continue;
                    }
                    piece.clear();
                    piece.extend(&word[start..end]);
                    let (id, score) = match self.pieces.get(piece.as_str()) {
                        Some(&found) => found,
                        // A single character the vocabulary lacks: unknown, at a price.
                        None if end - start == 1 => (self.unknown, -30.0),
                        None => continue,
                    };
                    let total = best[start].0 + score;
                    if total > best[end].0 {
                        best[end] = (total, start, id);
                    }
                }
            }
            let mut found = Vec::new();
            let mut at = n;
            while at > 0 {
                let (_, start, id) = best[at];
                found.push(id);
                at = start;
            }
            ids.extend(found.into_iter().rev().filter(|&id| id != self.unknown));
            if ids.len() >= max {
                break;
            }
        }
        ids.truncate(max);
        ids
    }
}

/// The rows as 8-bit numbers, each row with its scale.
pub struct Quantized {
    pub dim: usize,
    pub rows: usize,
    data: Vec<i8>,
    scales: Vec<f32>,
}

impl Quantized {
    /// Reads the `embeddings` table of a big safetensors file a piece at a time, so it never sits in
    /// memory as F32 all at once.
    pub fn load(path: &Path) -> Result<Quantized, String> {
        let bad = || "the model file isn't a safetensors file Glint reads".to_string();
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut len = [0u8; 8];
        file.read_exact(&mut len).map_err(|_| bad())?;
        let header_len = u64::from_le_bytes(len) as usize;
        let mut header = vec![0u8; header_len];
        file.read_exact(&mut header).map_err(|_| bad())?;
        let header: serde_json::Value = serde_json::from_slice(&header).map_err(|_| bad())?;
        let table = &header["embeddings"];
        let shape: Vec<usize> = table["shape"].as_array().ok_or_else(bad)?.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect();
        let offsets: Vec<u64> = table["data_offsets"].as_array().ok_or_else(bad)?.iter().filter_map(|n| n.as_u64()).collect();
        let (&[rows, dim], &[from, to]) = (shape.as_slice(), offsets.as_slice()) else { return Err(bad()) };
        if table["dtype"] != "F32" || (to - from) as usize != rows * dim * 4 {
            return Err(bad());
        }
        file.seek(SeekFrom::Start(8 + header_len as u64 + from)).map_err(|e| e.to_string())?;
        let mut data = vec![0i8; rows * dim];
        let mut scales = vec![0f32; rows];
        let chunk_rows = (16 << 20) / (dim * 4);
        let mut buf = vec![0u8; chunk_rows * dim * 4];
        let mut row = 0;
        while row < rows {
            let take = chunk_rows.min(rows - row);
            let bytes = &mut buf[..take * dim * 4];
            file.read_exact(bytes).map_err(|e| e.to_string())?;
            for r in 0..take {
                let values: Vec<f32> = bytes[r * dim * 4..(r + 1) * dim * 4].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
                let max = values.iter().fold(0f32, |m, v| m.max(v.abs()));
                let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
                scales[row + r] = scale;
                for (d, v) in values.iter().enumerate() {
                    data[(row + r) * dim + d] = (v / scale).round().clamp(-127.0, 127.0) as i8;
                }
            }
            row += take;
        }
        Ok(Quantized { dim, rows, data, scales })
    }

    /// Adds row `id` to `sum`.
    pub fn add(&self, id: usize, sum: &mut [f32]) {
        let scale = self.scales[id];
        for (s, &v) in sum.iter_mut().zip(&self.data[id * self.dim..(id + 1) * self.dim]) {
            *s += v as f32 * scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_words_into_their_likeliest_pieces() {
        let pieces: HashMap<String, (u32, f32)> = [("▁", -4.0), ("▁mer", -6.0), ("▁merhaba", -7.0), ("haba", -7.5), ("▁ب", -5.0), ("يت", -6.0), ("▁بيت", -6.5), ("!", -3.0), ("▁!", -3.0)]
            .iter()
            .enumerate()
            .map(|(i, (p, s))| (p.to_string(), (i as u32 + 2, *s)))
            .collect();
        let tokenizer = Unigram { pieces, max_chars: 8, unknown: 1 };
        // "▁merhaba" whole beats "▁mer" + "haba" (-7 > -13.5); Arabic the same way.
        assert_eq!(tokenizer.tokens("Merhaba".to_lowercase().as_str(), 10), [4]);
        assert_eq!(tokenizer.tokens("بيت", 10), [8]);
        assert_eq!(Unigram::normalize("a,b  ｃ"), "a , b c", "NFKC turns the full-width c into c");
        // Unknown characters are left out.
        assert!(tokenizer.tokens("zzz", 10).is_empty() || tokenizer.tokens("zzz", 10).iter().all(|&id| id != 1));
    }
}
