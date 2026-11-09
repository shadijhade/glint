//! Ported from src/lib/plain.ts.
//!
//! Plain words into the search box's filters: "pdfs from last week over 5 mb" is `ext:pdf dm:7d size:>5mb`.
//! A small lexicon, read as you type and in microseconds, nothing leaving the PC. The box offers what it
//! read under it, and Tab puts the filters in the box, so the syntax is learned by using it.
//!
//! Nothing here depends on today's date: ages come out as relative filters (`dm:7d`), so no `now` is needed.
//! The output strings are byte-identical to the TypeScript, because the rest of Glint parses them.

use crate::js::{is_js_space, js_round, number_to_string};

/// What the box would search for, and what each part means, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plain {
    /// The text to put in the box: the leftover name words, then the filters.
    pub query: String,
    /// One short phrase per recognized part, for the line shown under the box.
    pub chips: Vec<String>,
}

/// The kind of media the words so far have named, which changes how later words read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Media {
    Photo,
    Video,
    Music,
}

const PHOTOS: &str = "jpg;jpeg;png;gif;webp;heic;heif;bmp;tif;tiff;avif;jxl;raw;cr2;nef;arw;dng";
const VIDEOS: &str = "mp4;mkv;mov;avi;wmv;webm;m4v;mpg;mpeg;3gp;m2ts";
const MUSIC: &str = "mp3;flac;wav;m4a;aac;ogg;opus;wma;aiff";
const OTHER_EXTS: &str =
    "pdf;doc;docx;odt;rtf;txt;md;xls;xlsx;csv;ppt;pptx;zip;7z;rar;tar;gz;exe;msi;iso;vhd;vhdx;epub;json;log;svg;psd;dll;lnk;ttf;otf";

/// One kind of file: the words for it, its extensions, how the chip names it, and the media it implies.
struct Kind {
    forms: &'static [&'static str],
    exts: &'static str,
    chip: &'static str,
    media: Option<Media>,
}

/// Kinds of files by the words for them, as Glint's own groups have them (fileTypes.ts).
const KINDS: &[Kind] = &[
    Kind {
        forms: &["photos", "photo", "pictures", "picture", "pics", "images", "image"],
        exts: PHOTOS,
        chip: "photos",
        media: Some(Media::Photo),
    },
    Kind {
        forms: &["videos", "video", "movies", "movie", "clips", "films"],
        exts: VIDEOS,
        chip: "videos",
        media: Some(Media::Video),
    },
    Kind {
        forms: &["music", "songs", "song", "audio", "tracks"],
        exts: MUSIC,
        chip: "music",
        media: Some(Media::Music),
    },
    Kind {
        forms: &["documents", "document", "docs"],
        exts: "pdf;doc;docx;odt;rtf;txt;md",
        chip: "documents",
        media: None,
    },
    Kind {
        forms: &["spreadsheets", "spreadsheet", "excel", "workbooks"],
        exts: "xls;xlsx;xlsm;ods;csv",
        chip: "spreadsheets",
        media: None,
    },
    Kind {
        forms: &["presentations", "presentation", "slides", "powerpoints", "decks"],
        exts: "ppt;pptx;odp",
        chip: "presentations",
        media: None,
    },
    Kind {
        forms: &["archives", "archive", "zips", "compressed"],
        exts: "zip;7z;rar;tar;gz;tgz;xz;bz2;zst",
        chip: "archives",
        media: None,
    },
    Kind {
        forms: &["installers", "installer", "programs", "apps", "executables"],
        exts: "exe;msi",
        chip: "programs",
        media: None,
    },
    Kind {
        forms: &["ebooks", "e-books", "books"],
        exts: "epub;mobi;azw3",
        chip: "e-books",
        media: None,
    },
    Kind {
        forms: &["fonts", "font"],
        exts: "ttf;otf;woff;woff2",
        chip: "fonts",
        media: None,
    },
];

/// Kinds in two words (first, second, extensions, chip), read before the words alone.
const PHRASES: &[(&str, &str, &str, &str)] = &[
    ("word", "documents", "doc;docx", "Word documents"),
    ("disk", "images", "iso;img;vhd;vhdx", "disk images"),
];

/// Words that say nothing a filter needs.
const FILLER: &[&str] = &[
    "all", "any", "every", "my", "the", "a", "an", "of", "that", "which", "are", "is", "were", "was", "files", "file",
    "items", "stuff", "things", "show", "find", "get", "list", "search", "me", "please", "and", "or", "with", "from",
    "in", "on", "at", "since", "changed", "modified", "edited", "updated", "saved", "made", "created",
];

// NOTE(port): the TypeScript looked these words up in plain objects, so a word like "constructor" or
// "toString" found a property of Object.prototype and was treated as a unit or comparison. That is a bug;
// here only the listed words match.

/// Size units: the word to (filter unit, how the chip writes it).
fn size_unit(word: &str) -> Option<(&'static str, &'static str)> {
    Some(match word {
        "b" | "bytes" => ("b", "bytes"),
        "kb" | "k" | "kilobytes" => ("kb", "KB"),
        "mb" | "m" | "megabytes" => ("mb", "MB"),
        "gb" | "g" | "gigabytes" => ("gb", "GB"),
        "tb" | "terabytes" => ("tb", "TB"),
        _ => return None,
    })
}

/// The next unit down, for a size given with decimals (the filter takes whole numbers).
fn smaller_unit(unit: &str) -> Option<&'static str> {
    match unit {
        "tb" => Some("gb"),
        "gb" => Some("mb"),
        "mb" => Some("kb"),
        "kb" => Some("b"),
        _ => None,
    }
}

/// dm:'s units: the word to (filter unit, multiplier, how the chip writes it). A month counts as 30 days.
fn age_unit(word: &str) -> Option<(&'static str, f64, &'static str)> {
    Some(match word {
        "hour" => ("h", 1.0, "hour"),
        "hours" => ("h", 1.0, "hours"),
        "day" => ("d", 1.0, "day"),
        "days" => ("d", 1.0, "days"),
        "week" => ("w", 1.0, "week"),
        "weeks" => ("w", 1.0, "weeks"),
        "month" => ("d", 30.0, "month"),
        "months" => ("d", 30.0, "months"),
        "year" => ("y", 1.0, "year"),
        "years" => ("y", 1.0, "years"),
        _ => return None,
    })
}

/// Duration units: the word to (filter unit, how the chip writes it).
fn duration_unit(word: &str) -> Option<(&'static str, &'static str)> {
    Some(match word {
        "seconds" | "second" | "secs" | "sec" | "s" => ("s", "s"),
        "minutes" | "minute" | "mins" | "min" => ("m", "min"),
        "hours" | "hour" | "hrs" => ("h", "h"),
        _ => return None,
    })
}

/// Comparisons that mean "more than", with how each reads.
fn more_word(word: &str) -> Option<&'static str> {
    Some(match word {
        "over" | "above" | "bigger" | "larger" | "more" | "greater" => "over",
        "longer" => "longer than",
        "wider" => "wider than",
        "taller" => "taller than",
        "least" => "at least",
        _ => return None,
    })
}

/// Comparisons that mean "less than", with how each reads.
fn less_word(word: &str) -> Option<&'static str> {
    Some(match word {
        "under" | "below" | "smaller" | "less" => "under",
        "shorter" => "shorter than",
        "narrower" => "narrower than",
        "most" => "at most",
        _ => return None,
    })
}

/// Whether a word is an extension that "pngs" or "png files" can name.
fn is_known_ext(word: &str) -> bool {
    [PHOTOS, VIDEOS, MUSIC, OTHER_EXTS]
        .iter()
        .any(|list| list.split(';').any(|e| e == word))
}

/// Whether a word is filler that says nothing a filter needs.
fn is_filler(word: &str) -> bool {
    FILLER.contains(&word)
}

/// `/^(19|20)\d\d$/`: a four-digit year from 1900 to 2099.
fn is_year(word: &str) -> bool {
    let b = word.as_bytes();
    b.len() == 4 && (word.starts_with("19") || word.starts_with("20")) && b[2].is_ascii_digit() && b[3].is_ascii_digit()
}

/// `"5"`, `"1.5"` or `"5mb"`: the number and what's stuck to it (`/^(\d+(?:\.\d+)?)([a-z]*)$/`).
fn number_at(word: Option<&str>) -> Option<(&str, &str)> {
    let w = word?;
    let b = w.as_bytes();
    let mut end = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if end == 0 {
        return None;
    }
    if b.get(end) == Some(&b'.') {
        let frac = b[end + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if frac > 0 {
            end += 1 + frac;
        }
    }
    let unit = &w[end..];
    if unit.bytes().all(|c| c.is_ascii_lowercase()) {
        Some((&w[..end], unit))
    } else {
        None
    }
}

/// Whether files of this kind have a date they were taken.
fn taken(media: Option<Media>) -> bool {
    matches!(media, Some(Media::Photo) | Some(Media::Video))
}

/// The filters and chips found so far: one filter of each kind, except extensions, which add up.
struct Found {
    filters: Vec<String>,
    chips: Vec<String>,
}

impl Found {
    fn add(&mut self, filter: &str, chip: &str) {
        let key_end = filter.find(':').unwrap_or(filter.len());
        let key = &filter[..key_end];
        let at = self
            .filters
            .iter()
            .position(|f| f.strip_prefix(key).is_some_and(|rest| rest.starts_with(':')));
        match at {
            None => {
                self.filters.push(filter.to_string());
                self.chips.push(chip.to_string());
            }
            Some(at) if key == "ext" => {
                self.filters[at].push(';');
                self.filters[at].push_str(&filter[4..]);
                self.chips[at].push_str(" or ");
                self.chips[at].push_str(chip);
            }
            Some(_) => {}
        }
    }
}

/// What plain words in the box mean as filters, or `None` when they read as names: nothing recognized,
/// or the box holds filters, quotes or wildcards already.
pub fn plain_query(text: &str) -> Option<Plain> {
    if text.trim_matches(is_js_space).is_empty() || text.contains([':', '"', '*', '?']) {
        return None;
    }
    let words: Vec<&str> = text.split(is_js_space).filter(|w| !w.is_empty()).collect();
    let lower: Vec<String> = words
        .iter()
        .map(|w| {
            let l = w.to_lowercase();
            match l.chars().last() {
                Some(',' | ';' | '.' | '!') => l[..l.len() - 1].to_string(),
                _ => l,
            }
        })
        .collect();
    let at = |i: usize| lower.get(i).map(String::as_str);
    let mut names: Vec<&str> = Vec::new();
    let mut found = Found {
        filters: Vec::new(),
        chips: Vec::new(),
    };
    let mut media: Option<Media> = None;

    let mut i = 0;
    while i < lower.len() {
        let w = lower[i].as_str();
        let next = at(i + 1);
        let after = at(i + 2);
        let prev = i.checked_sub(1).and_then(at);
        // How many more words this step used up, beyond the one at `i`.
        let skip: usize = 'step: {
            if let Some(p) = PHRASES
                .iter()
                .find(|p| p.0 == w && (next == Some(p.1) || p.1.strip_suffix('s').is_some_and(|s| next == Some(s))))
            {
                found.add(&format!("ext:{}", p.2), p.3);
                break 'step 1;
            }
            if let Some(kind) = KINDS.iter().find(|k| k.forms.contains(&w)) {
                found.add(&format!("ext:{}", kind.exts), kind.chip);
                media = kind.media.or(media);
                break 'step 0;
            }
            // "pngs", "mp3s", "png files".
            let ext = match w.strip_suffix('s') {
                Some(stem) if is_known_ext(stem) => Some(stem),
                _ if is_known_ext(w) && matches!(next, Some("files" | "file")) => Some(w),
                _ => None,
            };
            if let Some(ext) = ext {
                found.add(&format!("ext:{ext}"), &format!("{} files", ext.to_uppercase()));
                break 'step 0;
            }
            if w == "folders" || w == "folder" {
                found.add("folder:", "folders");
                break 'step 0;
            }
            // When it changed: today, yesterday, recently, this week, the last 3 days; older than 2 years.
            match w {
                "today" => {
                    found.add("dm:1d", "changed in the last day");
                    break 'step 0;
                }
                "yesterday" => {
                    found.add("dm:2d", "changed since yesterday");
                    break 'step 0;
                }
                "recent" | "recently" | "lately" => {
                    found.add("dm:7d", "changed in the last 7 days");
                    break 'step 0;
                }
                _ => {}
            }
            if matches!(w, "this" | "last" | "past") && matches!(next, Some("week" | "month" | "year")) {
                let (age, span) = match next {
                    Some("week") => ("7d", "7 days"),
                    Some("month") => ("30d", "30 days"),
                    _ => ("1y", "year"),
                };
                if taken(media) && matches!(prev, Some("taken" | "shot")) {
                    found.add(&format!("taken:{age}"), &format!("taken in the last {span}"));
                } else {
                    found.add(&format!("dm:{age}"), &format!("changed in the last {span}"));
                }
                break 'step 1;
            }
            let count = number_at(next);
            if matches!(w, "last" | "past") {
                if let (Some((num, "")), Some((unit, times, span))) = (count, after.and_then(age_unit)) {
                    let n = js_round(num.parse::<f64>().unwrap_or(0.0) * times);
                    found.add(
                        &format!("dm:{}{unit}", number_to_string(n)),
                        &format!("changed in the last {num} {span}"),
                    );
                    break 'step 2;
                }
            }
            if w == "older" && next == Some("than") {
                let n = number_at(after);
                // The unit is stuck to the number ("2y" style words) or the word after it.
                let unit_word = match n {
                    Some((_, u)) if !u.is_empty() => Some(u),
                    _ => at(i + 3),
                };
                if let (Some((num, stuck)), Some((unit, times, span))) = (n, unit_word.and_then(age_unit)) {
                    let v = js_round(num.parse::<f64>().unwrap_or(0.0) * times);
                    found.add(
                        &format!("dm:>{}{unit}", number_to_string(v)),
                        &format!("older than {num} {span}"),
                    );
                    break 'step if stuck.is_empty() { 3 } else { 2 };
                }
            }
            if w == "old" {
                found.add("dm:>1y", "older than a year");
                break 'step 0;
            }
            // When a photo or video was taken: taken in 2024, from 2024.
            if w == "taken" || w == "shot" || ((w == "from" || w == "in") && taken(media)) {
                let year = match (next, after) {
                    (Some(n), _) if is_year(n) => Some((n, 1)),
                    (Some("in"), Some(a)) if is_year(a) => Some((a, 2)),
                    _ => None,
                };
                if let Some((year, used)) = year {
                    found.add(&format!("taken:{year}"), &format!("taken in {year}"));
                    break 'step used;
                }
            }
            // Sizes, lengths and pixels: bigger than 5 mb, over 1.5gb, at least 10kb, longer than 5 minutes, wider than 1920.
            let more = more_word(w);
            if let Some(said) = more.or_else(|| less_word(w)) {
                let pos = if next == Some("than") { i + 2 } else { i + 1 };
                if let Some((num, stuck)) = number_at(at(pos)) {
                    let op = match w {
                        "least" => ">=",
                        "most" => "<=",
                        _ if more.is_some() => ">",
                        _ => "<",
                    };
                    let unit_word = if stuck.is_empty() { at(pos + 1) } else { Some(stuck) };
                    let used = if stuck.is_empty() { pos + 1 - i } else { pos - i };
                    if matches!(w, "wider" | "taller" | "narrower") && stuck.is_empty() {
                        let dim = if w == "taller" { "height" } else { "width" };
                        found.add(&format!("{dim}:{op}{num}"), &format!("{said} {num} pixels"));
                        break 'step pos - i;
                    }
                    if let Some(uw) = unit_word {
                        let is_length =
                            matches!(w, "longer" | "shorter") || matches!(media, Some(Media::Video | Media::Music));
                        if let (Some((unit, name)), true) = (duration_unit(uw), is_length) {
                            found.add(&format!("duration:{op}{num}{unit}"), &format!("{said} {num} {name}"));
                            break 'step used;
                        }
                        if let Some((mut unit, name)) = size_unit(uw) {
                            let mut value = num.parse::<f64>().unwrap_or(0.0);
                            while !(value.is_finite() && value.fract() == 0.0) {
                                match smaller_unit(unit) {
                                    Some(down) => {
                                        value *= 1024.0;
                                        unit = down;
                                    }
                                    None => break,
                                }
                            }
                            found.add(
                                &format!("size:{op}{}{unit}", number_to_string(js_round(value))),
                                &format!("{said} {num} {name}"),
                            );
                            break 'step used;
                        }
                    }
                }
            }
            match w {
                "huge" => {
                    found.add("size:>1gb", "over 1 GB");
                    break 'step 0;
                }
                "small" | "tiny" => {
                    found.add("size:<100kb", "under 100 KB");
                    break 'step 0;
                }
                "big" | "large" => {
                    found.add("size:>100mb", "over 100 MB");
                    break 'step 0;
                }
                _ => {}
            }
            // Who made it: songs by an artist, photos taken with (or shot on) a camera.
            if w == "by" && media == Some(Media::Music) {
                if let Some(next_word) = next {
                    let who = if next_word == "the" && after.is_some() {
                        i + 2
                    } else {
                        i + 1
                    };
                    if let (Some(name), Some(shown)) = (at(who), words.get(who)) {
                        found.add(&format!("artist:{name}"), &format!("by {shown}"));
                        break 'step who - i;
                    }
                }
            }
            if w == "with" || w == "on" {
                if let Some(next_word) = next {
                    if !is_filler(next_word) && (matches!(prev, Some("taken" | "shot")) || media == Some(Media::Photo))
                    {
                        found.add(&format!("camera:{next_word}"), &format!("taken with {}", words[i + 1]));
                        break 'step 1;
                    }
                }
            }
            // "named budget", "called budget": what follows is the name.
            if (w == "named" || w == "called") && next.is_some() {
                names.push(words[i + 1]);
                break 'step 1;
            }
            if is_filler(w) || w == "taken" || w == "shot" || w == "than" {
                break 'step 0;
            }
            names.push(words[i]);
            0
        };
        i += 1 + skip;
    }
    if found.filters.is_empty() {
        return None;
    }
    if !names.is_empty() {
        found.chips.push(format!("named \u{201C}{}\u{201D}", names.join(" ")));
    }
    let query = names
        .iter()
        .copied()
        .chain(found.filters.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    Some(Plain {
        query,
        chips: found.chips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check file's `plain` helper: just the query, or None.
    fn plain(text: &str) -> Option<String> {
        plain_query(text).map(|p| p.query)
    }

    fn chips(text: &str) -> Option<Vec<String>> {
        plain_query(text).map(|p| p.chips)
    }

    fn strs(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn plain_words_read_as_filters() {
        assert_eq!(
            plain("pdfs from last week over 5 mb").as_deref(),
            Some("ext:pdf dm:7d size:>5mb")
        );
        assert_eq!(
            chips("pdfs from last week over 5 mb"),
            strs(&["PDF files", "changed in the last 7 days", "over 5 MB"])
        );
        // What isn't a filter is the name.
        assert_eq!(plain("invoice pdfs").as_deref(), Some("invoice ext:pdf"));
        assert_eq!(
            plain("documents named budget"),
            Some("budget ext:pdf;doc;docx;odt;rtf;txt;md".to_string())
        );
        assert_eq!(
            chips("documents named budget"),
            strs(&["documents", "named \u{201C}budget\u{201D}"])
        );
        assert_eq!(plain("photos taken in 2024"), Some(format!("ext:{PHOTOS} taken:2024")));
        // Kinds add up, and from a year is when they were taken.
        assert_eq!(
            plain("photos and videos from 2024"),
            Some(format!("ext:{PHOTOS};{VIDEOS} taken:2024"))
        );
        assert_eq!(
            chips("photos and videos from 2024"),
            strs(&["photos or videos", "taken in 2024"])
        );
        // A year, for files without a date taken, is part of the name.
        assert_eq!(plain("pdfs from 2024").as_deref(), Some("2024 ext:pdf"));
        assert!(plain("videos longer than 10 minutes")
            .unwrap()
            .ends_with(" duration:>10m"));
        assert!(plain("songs by the beatles").unwrap().ends_with(" artist:beatles"));
        assert!(plain("photos with pixel").unwrap().ends_with(" camera:pixel"));
        assert!(plain("pictures wider than 1920").unwrap().ends_with(" width:>1920"));
        assert_eq!(plain("big zip files").as_deref(), Some("size:>100mb ext:zip"));
        assert_eq!(plain("folders changed today").as_deref(), Some("folder: dm:1d"));
        // The filter takes whole numbers.
        assert_eq!(plain("files bigger than 1.5 gb").as_deref(), Some("size:>1536mb"));
        assert_eq!(plain("at least 2gb").as_deref(), Some("size:>=2gb"));
        assert_eq!(plain("files smaller than 10kb").as_deref(), Some("size:<10kb"));
        assert_eq!(plain("logs older than 2 years").as_deref(), Some("ext:log dm:>2y"));
        assert_eq!(plain("old isos").as_deref(), Some("dm:>1y ext:iso"));
        assert_eq!(
            plain("word documents from the last 3 days").as_deref(),
            Some("ext:doc;docx dm:3d")
        );
        assert_eq!(plain("mp3s").as_deref(), Some("ext:mp3"));
    }

    #[test]
    fn names_and_existing_filters_stay_as_typed() {
        assert_eq!(plain_query("report"), None);
        assert_eq!(plain_query("quarterly report 2024"), None);
        assert_eq!(plain_query("ext:pdf report"), None);
        assert_eq!(plain_query("\"big brother\""), None);
        assert_eq!(plain_query("*.log"), None);
    }

    #[test]
    fn edge_cases_do_not_panic() {
        assert_eq!(plain_query(""), None);
        assert_eq!(plain_query("   "), None);
        assert_eq!(plain("over"), None);
        assert_eq!(plain("pdfs over"), Some("over ext:pdf".to_string()));
        assert_eq!(plain("pdfs by"), Some("by ext:pdf".to_string()));
        assert_eq!(plain("pdfs older than"), Some("older ext:pdf".to_string()));
        assert_eq!(plain("pdfs named"), Some("named ext:pdf".to_string()));
        assert_eq!(plain("PDFS, أحمد"), Some("أحمد ext:pdf".to_string()));
        assert_eq!(plain("pdfs over 1.5b").as_deref(), Some("ext:pdf size:>2b"));
        assert_eq!(plain("constructor pdfs"), Some("constructor ext:pdf".to_string()));
    }

    #[test]
    fn huge_sizes_are_written_the_way_javascript_writes_them() {
        // String(1e23) is "1e+23" in JavaScript, so the filter carries that text.
        assert_eq!(
            plain("over 100000000000000000000000 b").as_deref(),
            Some("size:>1e+23b")
        );
        assert_eq!(plain("over 5 bytes").as_deref(), Some("size:>5b"));
    }

    #[test]
    fn whole_numbers_at_the_top_of_the_safe_range_are_not_rounded_up() {
        // Math.round(9007199254740991) is 9007199254740991 in JavaScript; the +0.5 trick alone would give ...992.
        assert_eq!(
            plain("over 9007199254740991 b").as_deref(),
            Some("size:>9007199254740991b")
        );
        assert_eq!(
            plain("last 9007199254740991 days").as_deref(),
            Some("dm:9007199254740991d")
        );
    }

    #[test]
    fn fractional_sizes_still_round_half_up() {
        // 2.5 kb is 2560 bytes, a whole number, so it passes through unchanged.
        assert_eq!(plain("over 2.5 kb").as_deref(), Some("size:>2560b"));
        // 1.5 b is not whole, so it rounds half up like Math.round: 2.
        assert_eq!(plain("over 1.5 b").as_deref(), Some("size:>2b"));
    }
}
