//! Deterministic programming-by-example ("FlashFill") synthesizer for batch file renaming.
//!
//! Given one or two `(from, to)` filename examples (and the remaining names in the batch for
//! disambiguation), infers a compact transformation rule built from stem tokens, token spans,
//! embedded dates (`YYYYMMDD` or `YYYY-MM-DD`), sequential numbering, literal separators, and
//! extension casing. Pure Rust, bounded search, sub-millisecond on typical batches.

use crate::paths::extension_start;
use std::collections::HashSet;

/// One user-provided before/after example at row `index` of the batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Example {
    pub index: usize,
    pub from: String,
    pub to: String,
}

/// Letter-casing transform applied to a token, span, or extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseTransform {
    Keep,
    Lower,
    Upper,
    Title,
}

/// One component of a date extracted from a filename stem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePart {
    Year4,
    Month2,
    Day2,
}

/// One step in a synthesized filename program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Atom {
    /// Fixed literal text.
    Const(String),
    /// A single non-separator token (`0..n` from start, or `-1..-n` from end).
    Token { idx: isize, case: CaseTransform },
    /// Inclusive span of tokens `[start..=end]`. When `sep` is `None`, preserves the original
    /// characters between those tokens in the source stem; otherwise joins them with `sep`.
    TokenRange {
        start: isize,
        end: isize,
        sep: Option<String>,
        case: CaseTransform,
    },
    /// Date component parsed from the source stem.
    Date { part: DatePart },
    /// Arithmetic sequence number (`start + index * step`, zero-padded to `pad` digits).
    Seq { start: i64, step: i64, pad: usize },
    /// Original extension prefixed with `.` when present (empty string if source has none).
    Ext { case: CaseTransform },
}

/// A synthesized renaming program that matches every given example.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesizedRule {
    pub atoms: Vec<Atom>,
    pub description: String,
}

impl SynthesizedRule {
    /// Applies the synthesized program to `input` at 0-based batch position `index`.
    pub fn apply(&self, input: &str, index: usize) -> String {
        let parsed = ParsedName::parse(input);
        let mut out = String::new();
        for atom in &self.atoms {
            match atom {
                Atom::Const(lit) => out.push_str(lit),
                Atom::Token { idx, case } => {
                    if let Some(tok) = resolve_index(parsed.tokens.len(), *idx).and_then(|i| parsed.tokens.get(i)) {
                        out.push_str(&apply_case(&tok.text, *case));
                    }
                }
                Atom::TokenRange { start, end, sep, case } => {
                    if let Some(val) = render_token_range(&parsed, *start, *end, sep.as_deref(), *case) {
                        out.push_str(&val);
                    }
                }
                Atom::Date { part } => {
                    if let Some(date) = &parsed.date {
                        let s = match part {
                            DatePart::Year4 => &date.year4,
                            DatePart::Month2 => &date.month2,
                            DatePart::Day2 => &date.day2,
                        };
                        out.push_str(s);
                    }
                }
                Atom::Seq { start, step, pad } => {
                    let n = i128::from(*start) + i128::try_from(index).unwrap_or(0) * i128::from(*step);
                    out.push_str(&format_seq(n, *pad));
                }
                Atom::Ext { case } => {
                    if !parsed.ext.is_empty() {
                        out.push('.');
                        out.push_str(&apply_case(parsed.ext, *case));
                    }
                }
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
struct TokenSpan {
    text: String,
    byte_start: usize,
    byte_end: usize,
}

#[derive(Clone, Debug)]
struct ParsedDate {
    year4: String,
    month2: String,
    day2: String,
}

struct ParsedName<'a> {
    stem: &'a str,
    ext: &'a str,
    tokens: Vec<TokenSpan>,
    date: Option<ParsedDate>,
}

impl<'a> ParsedName<'a> {
    fn parse(name: &'a str) -> Self {
        let (stem, ext) = match extension_start(name, false) {
            Some(dot) => (&name[..dot], &name[dot + 1..]),
            None => (name, ""),
        };
        let tokens = tokenize_stem(stem);
        let date = detect_date(stem, &tokens);
        Self {
            stem,
            ext,
            tokens,
            date,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Alpha,
    Digit,
    Sep,
}

fn char_kind(c: char) -> CharKind {
    if c.is_ascii_digit() {
        CharKind::Digit
    } else if c.is_alphabetic() {
        CharKind::Alpha
    } else {
        CharKind::Sep
    }
}

fn tokenize_stem(stem: &str) -> Vec<TokenSpan> {
    let mut out = Vec::new();
    let mut cur_kind: Option<CharKind> = None;
    let mut start = 0usize;
    for (idx, ch) in stem.char_indices() {
        let kind = char_kind(ch);
        match cur_kind {
            None => {
                cur_kind = Some(kind);
                start = idx;
            }
            Some(prev) if prev != kind => {
                if prev != CharKind::Sep {
                    out.push(TokenSpan {
                        text: stem[start..idx].to_string(),
                        byte_start: start,
                        byte_end: idx,
                    });
                }
                cur_kind = Some(kind);
                start = idx;
            }
            Some(_) => {}
        }
    }
    if let Some(prev) = cur_kind {
        if prev != CharKind::Sep {
            out.push(TokenSpan {
                text: stem[start..].to_string(),
                byte_start: start,
                byte_end: stem.len(),
            });
        }
    }
    out
}

fn valid_ymd(y: u32, m: u32, d: u32) -> bool {
    (1900..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)
}

fn detect_date(stem: &str, tokens: &[TokenSpan]) -> Option<ParsedDate> {
    // Delimited YYYY-MM-DD / YYYY_MM_DD / YYYY.MM.DD across three consecutive numeric tokens.
    for win in tokens.windows(3) {
        if win[0].text.len() == 4
            && win[1].text.len() == 2
            && win[2].text.len() == 2
            && win[0].text.bytes().all(|b| b.is_ascii_digit())
            && win[1].text.bytes().all(|b| b.is_ascii_digit())
            && win[2].text.bytes().all(|b| b.is_ascii_digit())
        {
            let sep1 = &stem[win[0].byte_end..win[1].byte_start];
            let sep2 = &stem[win[1].byte_end..win[2].byte_start];
            if matches!(sep1, "-" | "_" | ".") && sep1 == sep2 {
                let y = win[0].text.parse::<u32>().unwrap_or(0);
                let m = win[1].text.parse::<u32>().unwrap_or(0);
                let d = win[2].text.parse::<u32>().unwrap_or(0);
                if valid_ymd(y, m, d) {
                    return Some(ParsedDate {
                        year4: win[0].text.clone(),
                        month2: win[1].text.clone(),
                        day2: win[2].text.clone(),
                    });
                }
            }
        }
    }
    // Compact 8-digit YYYYMMDD token.
    for tok in tokens {
        if tok.text.len() == 8 && tok.text.bytes().all(|b| b.is_ascii_digit()) {
            let y = tok.text[..4].parse::<u32>().unwrap_or(0);
            let m = tok.text[4..6].parse::<u32>().unwrap_or(0);
            let d = tok.text[6..8].parse::<u32>().unwrap_or(0);
            if valid_ymd(y, m, d) {
                return Some(ParsedDate {
                    year4: tok.text[..4].to_string(),
                    month2: tok.text[4..6].to_string(),
                    day2: tok.text[6..8].to_string(),
                });
            }
        }
    }
    None
}

fn resolve_index(len: usize, idx: isize) -> Option<usize> {
    if idx >= 0 {
        let u = usize::try_from(idx).ok()?;
        (u < len).then_some(u)
    } else {
        let back = idx.checked_neg().and_then(|v| usize::try_from(v).ok())?;
        len.checked_sub(back)
    }
}

fn apply_case(text: &str, case: CaseTransform) -> String {
    match case {
        CaseTransform::Keep => text.to_string(),
        CaseTransform::Lower => text.to_lowercase(),
        CaseTransform::Upper => text.to_uppercase(),
        CaseTransform::Title => {
            let mut chars = text.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let head: String = first.to_uppercase().collect();
                    format!("{head}{}", chars.as_str().to_lowercase())
                }
            }
        }
    }
}

fn render_token_range(
    parsed: &ParsedName<'_>,
    start: isize,
    end: isize,
    sep: Option<&str>,
    case: CaseTransform,
) -> Option<String> {
    let s = resolve_index(parsed.tokens.len(), start)?;
    let e = resolve_index(parsed.tokens.len(), end)?;
    if s > e {
        return None;
    }
    match sep {
        None => {
            let raw = &parsed.stem[parsed.tokens[s].byte_start..parsed.tokens[e].byte_end];
            Some(match case {
                CaseTransform::Keep => raw.to_string(),
                CaseTransform::Lower => raw.to_lowercase(),
                CaseTransform::Upper => raw.to_uppercase(),
                CaseTransform::Title => {
                    parsed.tokens[s..=e]
                        .iter()
                        .enumerate()
                        .fold(String::new(), |mut acc, (offset, tok)| {
                            if offset > 0 {
                                let prev_end = parsed.tokens[s + offset - 1].byte_end;
                                acc.push_str(&parsed.stem[prev_end..tok.byte_start]);
                            }
                            acc.push_str(&apply_case(&tok.text, CaseTransform::Title));
                            acc
                        })
                }
            })
        }
        Some(joiner) => {
            let mut out = String::new();
            for (offset, tok) in parsed.tokens[s..=e].iter().enumerate() {
                if offset > 0 {
                    out.push_str(joiner);
                }
                out.push_str(&apply_case(&tok.text, case));
            }
            Some(out)
        }
    }
}

fn format_seq(n: i128, pad: usize) -> String {
    if n < 0 {
        let abs = n.unsigned_abs().to_string();
        let zeros = pad.saturating_sub(abs.len() + 1);
        format!("-{}{abs}", "0".repeat(zeros))
    } else {
        let text = n.to_string();
        let zeros = pad.saturating_sub(text.len());
        format!("{}{text}", "0".repeat(zeros))
    }
}

/// Parametric atom used during single-example parsing before `Seq` step/start are resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DraftAtom {
    Fixed(Atom),
    SeqSlot { val0: i64, pad: usize },
}

#[derive(Clone, Debug)]
struct Candidate {
    atoms: Vec<DraftAtom>,
    cost: u32,
}

const MAX_BEAM_PER_POS: usize = 24;
const MAX_COMPLETE: usize = 96;

fn case_penalty(case: CaseTransform) -> u32 {
    match case {
        CaseTransform::Keep => 0,
        CaseTransform::Lower | CaseTransform::Upper | CaseTransform::Title => 1,
    }
}

fn matching_cases(raw: &str, target: &str) -> Vec<CaseTransform> {
    let mut out = Vec::new();
    for case in [
        CaseTransform::Keep,
        CaseTransform::Lower,
        CaseTransform::Upper,
        CaseTransform::Title,
    ] {
        if apply_case(raw, case) == target {
            out.push(case);
            // When Keep already matches and raw isn't uniformly cased differently, still keep case
            // variants only if helpful across examples; Keep has lowest penalty.
        }
    }
    out
}

fn parse_stem_candidates(example: &Example, max_input_tokens: usize) -> Vec<Candidate> {
    let parsed_from = ParsedName::parse(&example.from);
    let (to_stem, _) = match extension_start(&example.to, false) {
        Some(dot) => (&example.to[..dot], &example.to[dot + 1..]),
        None => (example.to.as_str(), ""),
    };

    let boundaries: Vec<usize> = to_stem.char_indices().map(|(i, _)| i).chain([to_stem.len()]).collect();
    let n_pos = boundaries.len();
    let mut buckets: Vec<Vec<Candidate>> = vec![Vec::new(); n_pos];
    buckets[0].push(Candidate {
        atoms: Vec::new(),
        cost: 0,
    });

    let push_step = |buckets: &mut [Vec<Candidate>], next_pos: usize, next_cand: Candidate| {
        let bucket = &mut buckets[next_pos];
        if let Some(existing) = bucket.iter_mut().find(|c| c.atoms == next_cand.atoms) {
            if next_cand.cost < existing.cost {
                existing.cost = next_cand.cost;
            }
            return;
        }
        bucket.push(next_cand);
        bucket.sort_by_key(|c| c.cost);
        if bucket.len() > MAX_BEAM_PER_POS {
            bucket.truncate(MAX_BEAM_PER_POS);
        }
    };

    let tok_len = parsed_from.tokens.len();

    for pos_idx in 0..n_pos.saturating_sub(1) {
        let cur_bucket = buckets[pos_idx].clone();
        if cur_bucket.is_empty() {
            continue;
        }
        let byte_at = boundaries[pos_idx];
        let rest = &to_stem[byte_at..];

        // Gather all structural transitions starting at `byte_at`.
        let mut transitions: Vec<(usize, DraftAtom, u32)> = Vec::new();

        // 1. Date parts.
        if let Some(date) = &parsed_from.date {
            for (part, slice) in [
                (DatePart::Year4, date.year4.as_str()),
                (DatePart::Month2, date.month2.as_str()),
                (DatePart::Day2, date.day2.as_str()),
            ] {
                if rest.starts_with(slice) {
                    if let Some(next_idx) = boundaries[pos_idx + 1..]
                        .iter()
                        .position(|&b| b == byte_at + slice.len())
                    {
                        transitions.push((pos_idx + 1 + next_idx, DraftAtom::Fixed(Atom::Date { part }), 2));
                    }
                }
            }
        }

        // 2. Single tokens (positive and negative indices) and trailing/multi-token spans.
        for (i, tok) in parsed_from.tokens.iter().enumerate() {
            let neg = (i as isize) - (tok_len as isize);
            for (next_rel, _) in boundaries[pos_idx + 1..].iter().enumerate() {
                let end_byte = boundaries[pos_idx + 1 + next_rel];
                let slice = &to_stem[byte_at..end_byte];
                if slice.len() > tok.text.len().saturating_mul(2).max(8) {
                    break;
                }
                for case in matching_cases(&tok.text, slice) {
                    let cp = case_penalty(case);
                    // Positive index.
                    transitions.push((
                        pos_idx + 1 + next_rel,
                        DraftAtom::Fixed(Atom::Token { idx: i as isize, case }),
                        3 + cp,
                    ));
                    // Negative index.
                    transitions.push((
                        pos_idx + 1 + next_rel,
                        DraftAtom::Fixed(Atom::Token { idx: neg, case }),
                        4 + cp,
                    ));
                    // If this is the last token in example 0 and other inputs have more tokens, offer
                    // TokenRange(i, -1) with space or original separator so multi-word tails work.
                    if i + 1 == tok_len && max_input_tokens > tok_len {
                        transitions.push((
                            pos_idx + 1 + next_rel,
                            DraftAtom::Fixed(Atom::TokenRange {
                                start: i as isize,
                                end: -1,
                                sep: Some(" ".to_string()),
                                case,
                            }),
                            2 + cp,
                        ));
                        transitions.push((
                            pos_idx + 1 + next_rel,
                            DraftAtom::Fixed(Atom::TokenRange {
                                start: i as isize,
                                end: -1,
                                sep: None,
                                case,
                            }),
                            2 + cp,
                        ));
                    }
                }
            }
        }

        // 3. Contiguous multi-token spans `[s..=e]` (e.g. "New_York" -> "New York" or "New-York").
        for s in 0..tok_len {
            for e in (s + 1)..tok_len {
                for sep in [None, Some(" "), Some("_"), Some("-"), Some("")] {
                    for case in [
                        CaseTransform::Keep,
                        CaseTransform::Lower,
                        CaseTransform::Upper,
                        CaseTransform::Title,
                    ] {
                        let Some(rendered) = render_token_range(&parsed_from, s as isize, e as isize, sep, case) else {
                            continue;
                        };
                        if !rendered.is_empty() && rest.starts_with(&rendered) {
                            let target_end = byte_at + rendered.len();
                            if let Some(rel) = boundaries[pos_idx + 1..].iter().position(|&b| b == target_end) {
                                let next_pos = pos_idx + 1 + rel;
                                let cp = case_penalty(case);
                                let sep_owned = sep.map(str::to_string);
                                transitions.push((
                                    next_pos,
                                    DraftAtom::Fixed(Atom::TokenRange {
                                        start: s as isize,
                                        end: e as isize,
                                        sep: sep_owned.clone(),
                                        case,
                                    }),
                                    3 + cp,
                                ));
                                // Also allow ending at negative index when e == tok_len - 1.
                                if e + 1 == tok_len {
                                    transitions.push((
                                        next_pos,
                                        DraftAtom::Fixed(Atom::TokenRange {
                                            start: s as isize,
                                            end: -1,
                                            sep: sep_owned,
                                            case,
                                        }),
                                        3 + cp,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Sequence slot on contiguous ASCII digit runs.
        let digit_bytes = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digit_bytes > 0 && digit_bytes <= 6 {
            let digits = &rest[..digit_bytes];
            if let Ok(val0) = digits.parse::<i64>() {
                let target_end = byte_at + digit_bytes;
                if let Some(rel) = boundaries[pos_idx + 1..].iter().position(|&b| b == target_end) {
                    let pad = if digits.len() > 1 && digits.starts_with('0') {
                        digits.len()
                    } else {
                        1
                    };
                    // Slightly cheaper than literal digits when zero-padded or 1-based at start so
                    // single-example numbering like `holiday_01.jpg` resolves as `{n:02}`.
                    let cost = if pad >= 2 || val0 == 1 { 5 } else { 8 };
                    transitions.push((pos_idx + 1 + rel, DraftAtom::SeqSlot { val0, pad }, cost));
                }
            }
        }

        // 5. Fallback: consume one character as literal Const (coalescing adjacent Consts).
        let next_pos = pos_idx + 1;
        let next_ch = &to_stem[byte_at..boundaries[next_pos]];
        for cand in &cur_bucket {
            for (target_pos, draft_atom, step_cost) in &transitions {
                let mut atoms = cand.atoms.clone();
                atoms.push(draft_atom.clone());
                push_step(
                    &mut buckets,
                    *target_pos,
                    Candidate {
                        atoms,
                        cost: cand.cost + step_cost,
                    },
                );
            }
            let mut atoms = cand.atoms.clone();
            let char_cost = match atoms.last_mut() {
                Some(DraftAtom::Fixed(Atom::Const(prev))) => {
                    prev.push_str(next_ch);
                    6
                }
                _ => {
                    atoms.push(DraftAtom::Fixed(Atom::Const(next_ch.to_string())));
                    8
                }
            };
            push_step(
                &mut buckets,
                next_pos,
                Candidate {
                    atoms,
                    cost: cand.cost + char_cost,
                },
            );
        }
    }

    let mut done = buckets.pop().unwrap_or_default();
    done.truncate(MAX_COMPLETE);
    done
}

fn infer_extension_atom(examples: &[Example]) -> Option<Atom> {
    let pairs: Vec<(&str, &str)> = examples
        .iter()
        .map(|ex| {
            let from_ext = ParsedName::parse(&ex.from).ext;
            let to_ext = match extension_start(&ex.to, false) {
                Some(dot) => &ex.to[dot + 1..],
                None => "",
            };
            (from_ext, to_ext)
        })
        .collect();

    if pairs.iter().all(|(f, t)| f.is_empty() && t.is_empty()) {
        return None;
    }

    // Prefer keeping/re-casing source extension when every example matches a case transform.
    for case in [CaseTransform::Keep, CaseTransform::Lower, CaseTransform::Upper] {
        if pairs
            .iter()
            .all(|(from_ext, to_ext)| !from_ext.is_empty() && apply_case(from_ext, case) == *to_ext)
        {
            return Some(Atom::Ext { case });
        }
    }

    // Check if every example wrote the exact same fixed non-empty extension (e.g. `.webp`).
    if let Some((_, first_to)) = pairs.first() {
        if !first_to.is_empty() && pairs.iter().all(|(_, t)| *t == *first_to) {
            return Some(Atom::Const(format!(".{first_to}")));
        }
    }

    // Check if every example explicitly stripped extensions.
    if pairs.iter().all(|(_, t)| t.is_empty()) {
        return None;
    }

    Some(Atom::Ext {
        case: CaseTransform::Keep,
    })
}

fn render_fixed_prefix(atoms: &[Atom], input: &str) -> String {
    let parsed = ParsedName::parse(input);
    let mut out = String::new();
    for atom in atoms {
        match atom {
            Atom::Const(lit) => out.push_str(lit),
            Atom::Token { idx, case } => {
                if let Some(tok) = resolve_index(parsed.tokens.len(), *idx).and_then(|i| parsed.tokens.get(i)) {
                    out.push_str(&apply_case(&tok.text, *case));
                }
            }
            Atom::TokenRange { start, end, sep, case } => {
                if let Some(val) = render_token_range(&parsed, *start, *end, sep.as_deref(), *case) {
                    out.push_str(&val);
                }
            }
            Atom::Date { part } => {
                if let Some(date) = &parsed.date {
                    out.push_str(match part {
                        DatePart::Year4 => &date.year4,
                        DatePart::Month2 => &date.month2,
                        DatePart::Day2 => &date.day2,
                    });
                }
            }
            Atom::Ext { case } => {
                if !parsed.ext.is_empty() {
                    out.push('.');
                    out.push_str(&apply_case(parsed.ext, *case));
                }
            }
            Atom::Seq { .. } => {}
        }
    }
    out
}

/// Resolves any `SeqSlot` atoms against all examples and appends `ext_atom` if present.
fn instantiate_candidate(cand: &Candidate, examples: &[Example], ext_atom: &Option<Atom>) -> Option<(Vec<Atom>, u32)> {
    let ex0 = examples.first()?;
    let seq_slots: usize = cand
        .atoms
        .iter()
        .filter(|a| matches!(a, DraftAtom::SeqSlot { .. }))
        .count();

    if seq_slots > 1 {
        // Keep synthesis simple and predictable: at most one sequence counter per rule.
        return None;
    }

    let mut atoms: Vec<Atom> = Vec::with_capacity(cand.atoms.len() + 1);
    if seq_slots == 0 {
        for draft in &cand.atoms {
            if let DraftAtom::Fixed(a) = draft {
                atoms.push(a.clone());
            }
        }
    } else {
        let slot_pos = cand.atoms.iter().position(|a| matches!(a, DraftAtom::SeqSlot { .. }))?;
        let DraftAtom::SeqSlot { val0, pad } = cand.atoms[slot_pos] else {
            return None;
        };
        let (start, step, resolved_pad) = if examples.len() == 1 {
            (val0 - i64::try_from(ex0.index).ok()?, 1i64, pad)
        } else {
            let ex1 = &examples[1];
            if ex1.index == ex0.index {
                return None;
            }
            let prefix_atoms: Vec<Atom> = cand.atoms[..slot_pos]
                .iter()
                .filter_map(|d| match d {
                    DraftAtom::Fixed(a) => Some(a.clone()),
                    DraftAtom::SeqSlot { .. } => None,
                })
                .collect();
            let suffix_atoms: Vec<Atom> = cand.atoms[slot_pos + 1..]
                .iter()
                .filter_map(|d| match d {
                    DraftAtom::Fixed(a) => Some(a.clone()),
                    DraftAtom::SeqSlot { .. } => None,
                })
                .collect();

            let prefix1 = render_fixed_prefix(&prefix_atoms, &ex1.from);
            let mut suffix1 = render_fixed_prefix(&suffix_atoms, &ex1.from);
            if let Some(ext) = ext_atom {
                suffix1.push_str(&render_fixed_prefix(std::slice::from_ref(ext), &ex1.from));
            }
            let middle = ex1.to.strip_prefix(&prefix1)?.strip_suffix(&suffix1)?;
            if middle.is_empty() || !middle.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let val1 = middle.parse::<i64>().ok()?;
            let idx_diff = i64::try_from(ex1.index).ok()? - i64::try_from(ex0.index).ok()?;
            let val_diff = val1.checked_sub(val0)?;
            if val_diff % idx_diff != 0 {
                return None;
            }
            let step = val_diff / idx_diff;
            let start = val0.checked_sub(i64::try_from(ex0.index).ok()?.checked_mul(step)?)?;
            let pad1 = if middle.len() > 1 && middle.starts_with('0') {
                middle.len()
            } else {
                1
            };
            (start, step, pad.max(pad1))
        };

        for (i, draft) in cand.atoms.iter().enumerate() {
            if i == slot_pos {
                if step == 0 {
                    atoms.push(Atom::Const(format_seq(i128::from(start), resolved_pad)));
                } else {
                    atoms.push(Atom::Seq {
                        start,
                        step,
                        pad: resolved_pad,
                    });
                }
            } else if let DraftAtom::Fixed(a) = draft {
                atoms.push(a.clone());
            }
        }
    }

    if let Some(ext) = ext_atom {
        atoms.push(ext.clone());
    }

    let rule = SynthesizedRule {
        atoms,
        description: String::new(),
    };
    if !examples.iter().all(|ex| rule.apply(&ex.from, ex.index) == ex.to) {
        return None;
    }

    Some((rule.atoms, cand.cost))
}

fn describe_atoms(atoms: &[Atom]) -> String {
    let mut parts = Vec::with_capacity(atoms.len());
    for atom in atoms {
        match atom {
            Atom::Const(lit) => parts.push(lit.clone()),
            Atom::Token { idx, case } => {
                let label = if *idx >= 0 {
                    format!("{{{}}}", idx + 1)
                } else {
                    format!("{{{idx}}}")
                };
                parts.push(match case {
                    CaseTransform::Keep => label,
                    CaseTransform::Lower => format!("{label}:lower"),
                    CaseTransform::Upper => format!("{label}:UPPER"),
                    CaseTransform::Title => format!("{label}:Title"),
                });
            }
            Atom::TokenRange { start, end, sep, .. } => {
                let s = if *start >= 0 {
                    format!("{}", start + 1)
                } else {
                    format!("{start}")
                };
                let e = if *end >= 0 {
                    format!("{}", end + 1)
                } else {
                    format!("{end}")
                };
                match sep.as_deref() {
                    None => parts.push(format!("{{{s}..{e}}}")),
                    Some(joiner) => parts.push(format!("{{{s}..{e}:'{joiner}'}}")),
                }
            }
            Atom::Date { part } => parts.push(match part {
                DatePart::Year4 => "{YYYY}".to_string(),
                DatePart::Month2 => "{MM}".to_string(),
                DatePart::Day2 => "{DD}".to_string(),
            }),
            Atom::Seq { start, step, pad } => {
                let width = if *pad > 1 { format!(":0{pad}") } else { String::new() };
                if *start == 1 && *step == 1 {
                    parts.push(format!("{{n{width}}}"));
                } else {
                    parts.push(format!("{{n={start}+{step}i{width}}}"));
                }
            }
            Atom::Ext { case } => parts.push(match case {
                CaseTransform::Keep => ".{ext}".to_string(),
                CaseTransform::Lower => ".{ext:lower}".to_string(),
                CaseTransform::Upper => ".{ext:UPPER}".to_string(),
                CaseTransform::Title => ".{ext:Title}".to_string(),
            }),
        }
    }
    parts.concat()
}

/// Infers the simplest filename transformation rule consistent with all `examples`.
///
/// `all_inputs` lets single-example synthesis prefer rules that generalize across varying token
/// counts and avoid duplicate output names. Returns `None` when `examples` is empty or mutually
/// inconsistent.
pub fn synthesize(examples: &[Example], all_inputs: &[String]) -> Option<SynthesizedRule> {
    let scored = rank(examples, all_inputs)?;
    let (_, _, _, atoms) = scored.into_iter().next()?;
    let description = describe_atoms(&atoms);
    Some(SynthesizedRule { atoms, description })
}

/// How much worse than the best rule another may score and still count as a reading of the examples that could be
/// meant: the cost of one token counted from the end instead of the start is 1, a literal letter is 6 to 8.
const DOUBT_SLACK: u32 = 4;

/// The most rival rules asked about each name.
const MAX_RIVALS: usize = 6;

/// [`synthesize`], and which names the examples leave in doubt: `doubt[i]` is true for a name that is no example
/// and that a rival rule, one as plausible as the chosen one and just as true to every example, would name
/// differently (a token counted from the start or from the end gives the same name for the examples and another
/// for a name with more words). The chosen rule is the one `synthesize` gives.
pub fn synthesize_with_doubt(examples: &[Example], all_inputs: &[String]) -> Option<(SynthesizedRule, Vec<bool>)> {
    let scored = rank(examples, all_inputs)?;
    let (best_penalty, _, _, best_atoms) = scored.first()?.clone();
    let best = SynthesizedRule {
        description: describe_atoms(&best_atoms),
        atoms: best_atoms,
    };
    let rivals: Vec<SynthesizedRule> = scored
        .iter()
        .skip(1)
        .filter(|(penalty, ..)| *penalty <= best_penalty.saturating_add(DOUBT_SLACK))
        .filter(|(_, _, _, atoms)| *atoms != best.atoms)
        .take(MAX_RIVALS)
        .map(|(_, _, _, atoms)| SynthesizedRule {
            atoms: atoms.clone(),
            description: String::new(),
        })
        .collect();
    let given: HashSet<usize> = examples.iter().map(|ex| ex.index).collect();
    let doubt = all_inputs
        .iter()
        .enumerate()
        .map(|(i, input)| {
            !given.contains(&i) && {
                let chosen = best.apply(input, i);
                rivals.iter().any(|rival| rival.apply(input, i) != chosen)
            }
        })
        .collect();
    Some((best, doubt))
}

/// A rule's atoms with what they cost: the penalty (its own cost, plus a large one for each name it collapses onto
/// another's or empties), the names it collapses, and how many atoms it has.
type Scored = (u32, usize, usize, Vec<Atom>);

/// Every rule that holds for all the examples, best first (the first is the one `synthesize` picks: ties go to the
/// one found first). `None` when there are no examples to infer from.
fn rank(examples: &[Example], all_inputs: &[String]) -> Option<Vec<Scored>> {
    let first = examples.first()?;
    if first.from.is_empty() || first.to.is_empty() {
        return None;
    }

    let max_input_tokens = all_inputs
        .iter()
        .map(|name| ParsedName::parse(name).tokens.len())
        .max()
        .unwrap_or(0);

    let ext_atom = infer_extension_atom(examples);
    let candidates = parse_stem_candidates(first, max_input_tokens);

    let mut scored: Vec<Scored> = Vec::new();
    for cand in &candidates {
        let Some((atoms, base_cost)) = instantiate_candidate(cand, examples, &ext_atom) else {
            continue;
        };
        let probe = SynthesizedRule {
            atoms: atoms.clone(),
            description: String::new(),
        };

        // Penalize rules that collapse different inputs to identical names or drop to empty.
        let mut seen = HashSet::with_capacity(all_inputs.len());
        let mut collisions = 0usize;
        let mut empty_count = 0usize;
        for (i, input) in all_inputs.iter().enumerate() {
            let out = probe.apply(input, i);
            let stem_empty = match extension_start(&out, false) {
                Some(dot) => dot == 0,
                None => out.is_empty(),
            };
            if stem_empty {
                empty_count += 1;
            }
            if !seen.insert(out.to_lowercase()) {
                collisions += 1;
            }
        }
        let penalty =
            base_cost + u32::try_from(collisions).unwrap_or(100) * 50 + u32::try_from(empty_count).unwrap_or(100) * 100;
        scored.push((penalty, collisions, atoms.len(), atoms));
    }

    // A stable sort keeps the first found first among equals, as the single best used to be picked.
    scored.sort_by_key(|(penalty, collisions, atoms, _)| (*penalty, *collisions, *atoms));
    (!scored.is_empty()).then_some(scored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_compact_date_and_city_from_one_example() {
        let inputs = vec![
            "IMG_20260412_Tokyo.jpg".to_string(),
            "IMG_20260501_Kyoto.jpg".to_string(),
            "IMG_20260515_New_York.jpg".to_string(),
        ];
        let rule = synthesize(
            &[Example {
                index: 0,
                from: inputs[0].clone(),
                to: "2026-04-12 - Tokyo.jpg".to_string(),
            }],
            &inputs,
        )
        .expect("should synthesize date + city rule");

        assert_eq!(rule.apply(&inputs[0], 0), "2026-04-12 - Tokyo.jpg");
        assert_eq!(rule.apply(&inputs[1], 1), "2026-05-01 - Kyoto.jpg");
        assert_eq!(rule.apply(&inputs[2], 2), "2026-05-15 - New York.jpg");
    }

    #[test]
    fn infers_sequence_numbering_and_lowercase_extension() {
        let inputs = vec![
            "DSC_0042_holiday.JPG".to_string(),
            "DSC_0043_beach.JPG".to_string(),
            "DSC_0044_sunset.JPG".to_string(),
        ];
        let rule = synthesize(
            &[
                Example {
                    index: 0,
                    from: inputs[0].clone(),
                    to: "holiday_01.jpg".to_string(),
                },
                Example {
                    index: 1,
                    from: inputs[1].clone(),
                    to: "beach_02.jpg".to_string(),
                },
            ],
            &inputs,
        )
        .expect("should synthesize sequence rule");

        assert_eq!(rule.apply(&inputs[0], 0), "holiday_01.jpg");
        assert_eq!(rule.apply(&inputs[1], 1), "beach_02.jpg");
        assert_eq!(rule.apply(&inputs[2], 2), "sunset_03.jpg");
    }

    #[test]
    fn infers_custom_arithmetic_sequence_from_two_examples() {
        let inputs = vec![
            "shot_a.png".to_string(),
            "shot_b.png".to_string(),
            "shot_c.png".to_string(),
        ];
        let rule = synthesize(
            &[
                Example {
                    index: 0,
                    from: inputs[0].clone(),
                    to: "scene_010.png".to_string(),
                },
                Example {
                    index: 1,
                    from: inputs[1].clone(),
                    to: "scene_020.png".to_string(),
                },
            ],
            &inputs,
        )
        .expect("should synthesize step-10 sequence");

        assert_eq!(rule.apply(&inputs[2], 2), "scene_030.png");
    }

    #[test]
    fn swaps_tokens_and_preserves_compound_tar_extension() {
        let inputs = vec!["Artist - Song.mp3".to_string(), "Bach - Fugue.mp3".to_string()];
        let rule = synthesize(
            &[Example {
                index: 0,
                from: inputs[0].clone(),
                to: "Song (Artist).mp3".to_string(),
            }],
            &inputs,
        )
        .expect("should swap tokens");
        assert_eq!(rule.apply(&inputs[1], 1), "Fugue (Bach).mp3");

        let tars = vec![
            "proj_20260102_alpha.tar.gz".to_string(),
            "proj_20260103_beta.tar.gz".to_string(),
        ];
        let tar_rule = synthesize(
            &[Example {
                index: 0,
                from: tars[0].clone(),
                to: "alpha_2026-01-02.tar.gz".to_string(),
            }],
            &tars,
        )
        .expect("should preserve .tar.gz");
        assert_eq!(tar_rule.apply(&tars[1], 1), "beta_2026-01-03.tar.gz");
    }

    #[test]
    fn handles_unicode_and_arabic_names() {
        let inputs = vec![
            "ملف_20260412_تقرير.pdf".to_string(),
            "ملف_20260501_فاتورة.pdf".to_string(),
        ];
        let rule = synthesize(
            &[Example {
                index: 0,
                from: inputs[0].clone(),
                to: "2026-04-12 - تقرير.pdf".to_string(),
            }],
            &inputs,
        )
        .expect("should synthesize Arabic example");
        assert_eq!(rule.apply(&inputs[1], 1), "2026-05-01 - فاتورة.pdf");
    }

    #[test]
    fn contradictory_examples_return_none() {
        let inputs = vec!["a_1.txt".to_string(), "b_2.txt".to_string()];
        // Completely unrelated target strings with no common structural program.
        let rule = synthesize(
            &[
                Example {
                    index: 0,
                    from: inputs[0].clone(),
                    to: "zebra.txt".to_string(),
                },
                Example {
                    index: 1,
                    from: inputs[1].clone(),
                    to: "mountain.txt".to_string(),
                },
            ],
            &inputs,
        );
        assert!(rule.is_none());
    }

    #[test]
    fn rows_a_rival_reading_would_name_differently_are_in_doubt() {
        let inputs: Vec<String> = ["a_b.txt", "c_d.txt", "x_y_z.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let example = Example {
            index: 0,
            from: inputs[0].clone(),
            to: "b.txt".to_string(),
        };
        let (rule, doubt) = synthesize_with_doubt(std::slice::from_ref(&example), &inputs).expect("a rule");
        // The same rule `synthesize` gives.
        assert_eq!(Some(rule.clone()), synthesize(std::slice::from_ref(&example), &inputs));
        // The second token counted from the start or the last counted from the end: the same for names with two
        // words, different for the one with three.
        assert_eq!(doubt, vec![false, false, true]);
    }

    #[test]
    fn nothing_is_in_doubt_when_every_name_is_an_example_or_one_shape() {
        let inputs: Vec<String> = ["a_b.txt", "c_d.txt"].iter().map(|s| s.to_string()).collect();
        let example = Example {
            index: 0,
            from: inputs[0].clone(),
            to: "b.txt".to_string(),
        };
        let (_, doubt) = synthesize_with_doubt(&[example], &inputs).expect("a rule");
        assert_eq!(doubt, vec![false, false]);
        assert!(synthesize_with_doubt(&[], &inputs).is_none());
    }
}
