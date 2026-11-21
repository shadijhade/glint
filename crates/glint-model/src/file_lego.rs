//! Contextual file-on-file drop recipes ("File Lego").
//!
//! Determines what deterministic action makes sense when dragging one or more files onto a target
//! file row or tile (merge PDFs, mux audio/subtitles into a video, verify a checksum file, pack
//! into an archive, or open a side-by-side / pixel comparison).

use crate::archives::archive_in_path;
use crate::paths::{
    base_name, is_gallery, is_recent, is_recycle_bin, is_remote_path, is_shadow_path, parent_path, same_path,
};

/// Which kind of sidecar media track is being muxed into a video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuxKind {
    Subtitles,
    Audio,
}

/// Deterministic action proposed when dropping file(s) onto another file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegoAction {
    MergePdf {
        files: Vec<String>,
        dest_dir: String,
    },
    MuxMedia {
        video: String,
        track: String,
        kind: MuxKind,
    },
    CompareFiles {
        left: String,
        right: String,
    },
    VerifyChecksum {
        file: String,
        sums_file: String,
    },
    PackArchive {
        files: Vec<String>,
        target_archive: String,
    },
}

/// A matched drop recipe with short verb and user-facing description (used on the drag picture
/// and hover pill).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegoRecipe {
    pub action: LegoAction,
    pub verb: &'static str,
    pub summary: String,
}

fn ext_lower(path: &str) -> String {
    let name = base_name(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn is_pdf(path: &str) -> bool {
    ext_lower(path) == "pdf"
}

fn is_video(path: &str) -> bool {
    matches!(ext_lower(path).as_str(), "mp4" | "mkv" | "mov" | "webm" | "avi" | "m4v")
}

fn is_subtitle(path: &str) -> bool {
    matches!(ext_lower(path).as_str(), "srt" | "ass" | "ssa" | "vtt")
}

fn is_audio(path: &str) -> bool {
    matches!(
        ext_lower(path).as_str(),
        "mp3" | "wav" | "flac" | "m4a" | "aac" | "opus" | "ogg"
    )
}

fn is_checksum(path: &str) -> bool {
    matches!(
        ext_lower(path).as_str(),
        "sha256" | "sha512" | "sha1" | "md5" | "b3sum" | "sha256sum" | "md5sum"
    )
}

fn is_packable_archive(path: &str) -> bool {
    matches!(ext_lower(path).as_str(), "zip" | "7z")
}

fn is_image(path: &str) -> bool {
    matches!(
        ext_lower(path).as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "avif" | "ico" | "jxl"
    )
}

fn is_text_or_code(path: &str) -> bool {
    matches!(
        ext_lower(path).as_str(),
        "txt"
            | "md"
            | "markdown"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "html"
            | "css"
            | "scss"
            | "py"
            | "go"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "java"
            | "kt"
            | "sh"
            | "ps1"
            | "sql"
            | "csv"
            | "tsv"
            | "log"
            | "ini"
            | "cfg"
            | "env"
    )
}

fn is_virtual_or_readonly_location(path: &str) -> bool {
    is_remote_path(path)
        || is_recycle_bin(path)
        || is_gallery(path)
        || is_recent(path)
        || is_shadow_path(path)
        || archive_in_path(&parent_path(path))
}

/// Classifies dropping `sources` onto a non-directory item at `target_path`.
///
/// Returns `None` if `target_is_dir`, `sources` is empty, `target_path` is among `sources`, or
/// the target resides in an archive, remote, or virtual shell location.
pub fn classify_drop(sources: &[String], target_path: &str, target_is_dir: bool) -> Option<LegoRecipe> {
    if target_is_dir || sources.is_empty() || target_path.is_empty() {
        return None;
    }
    if is_virtual_or_readonly_location(target_path) {
        return None;
    }
    if sources.iter().any(|s| same_path(s, target_path)) {
        return None;
    }

    let target_name = base_name(target_path);

    // 1. Merge PDFs: target + all sources are `.pdf`.
    if is_pdf(target_path) && sources.iter().all(|s| is_pdf(s)) {
        let mut files = Vec::with_capacity(sources.len() + 1);
        files.push(target_path.to_string());
        files.extend(sources.iter().cloned());
        let dest_dir = parent_path(target_path);
        return Some(LegoRecipe {
            action: LegoAction::MergePdf { files, dest_dir },
            verb: "Merge PDFs",
            summary: format!("Merge into {target_name}"),
        });
    }

    // 2. Single-source pair recipes.
    if sources.len() == 1 {
        let src = &sources[0];

        // 2a. Video + subtitle/audio mux (either drop direction).
        if is_video(target_path) && is_subtitle(src) {
            return Some(LegoRecipe {
                action: LegoAction::MuxMedia {
                    video: target_path.to_string(),
                    track: src.clone(),
                    kind: MuxKind::Subtitles,
                },
                verb: "Mux subtitles",
                summary: format!("Add subtitles to {target_name}"),
            });
        }
        if is_subtitle(target_path) && is_video(src) {
            let vid_name = base_name(src);
            return Some(LegoRecipe {
                action: LegoAction::MuxMedia {
                    video: src.clone(),
                    track: target_path.to_string(),
                    kind: MuxKind::Subtitles,
                },
                verb: "Mux subtitles",
                summary: format!("Add subtitles to {vid_name}"),
            });
        }
        if is_video(target_path) && is_audio(src) {
            return Some(LegoRecipe {
                action: LegoAction::MuxMedia {
                    video: target_path.to_string(),
                    track: src.clone(),
                    kind: MuxKind::Audio,
                },
                verb: "Mux audio",
                summary: format!("Add audio to {target_name}"),
            });
        }
        if is_audio(target_path) && is_video(src) {
            let vid_name = base_name(src);
            return Some(LegoRecipe {
                action: LegoAction::MuxMedia {
                    video: src.clone(),
                    track: target_path.to_string(),
                    kind: MuxKind::Audio,
                },
                verb: "Mux audio",
                summary: format!("Add audio to {vid_name}"),
            });
        }

        // 2b. Checksum verification (either direction).
        if is_checksum(target_path) && !is_checksum(src) {
            let src_name = base_name(src);
            return Some(LegoRecipe {
                action: LegoAction::VerifyChecksum {
                    file: src.clone(),
                    sums_file: target_path.to_string(),
                },
                verb: "Verify checksum",
                summary: format!("Verify {src_name} against {target_name}"),
            });
        }
        if is_checksum(src) && !is_checksum(target_path) {
            let src_name = base_name(src);
            return Some(LegoRecipe {
                action: LegoAction::VerifyChecksum {
                    file: target_path.to_string(),
                    sums_file: src.clone(),
                },
                verb: "Verify checksum",
                summary: format!("Verify {target_name} against {src_name}"),
            });
        }
    }

    // 3. Drop onto `.zip` / `.7z` archive -> pack together beside it.
    if is_packable_archive(target_path) {
        return Some(LegoRecipe {
            action: LegoAction::PackArchive {
                files: sources.to_vec(),
                target_archive: target_path.to_string(),
            },
            verb: "Pack archive",
            summary: format!("Pack with {target_name}"),
        });
    }

    // 4. Compare two comparable files (images or text/code).
    if sources.len() == 1 {
        let src = &sources[0];
        let comparable =
            (is_image(target_path) && is_image(src)) || (is_text_or_code(target_path) && is_text_or_code(src));
        if comparable {
            return Some(LegoRecipe {
                action: LegoAction::CompareFiles {
                    left: target_path.to_string(),
                    right: src.clone(),
                },
                verb: "Compare",
                summary: format!("Compare with {target_name}"),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_multiple_pdfs() {
        let recipe = classify_drop(
            &[r"C:\docs\p2.pdf".into(), r"C:\docs\p3.PDF".into()],
            r"C:\docs\p1.pdf",
            false,
        )
        .expect("should merge PDFs");
        assert_eq!(recipe.verb, "Merge PDFs");
        assert_eq!(
            recipe.action,
            LegoAction::MergePdf {
                files: vec![
                    r"C:\docs\p1.pdf".into(),
                    r"C:\docs\p2.pdf".into(),
                    r"C:\docs\p3.PDF".into(),
                ],
                dest_dir: r"C:\docs".into(),
            }
        );
    }

    #[test]
    fn muxes_subtitles_and_audio_in_either_direction() {
        let sub = classify_drop(&[r"C:\v\subs.SRT".into()], r"C:\v\movie.mp4", false).unwrap();
        assert_eq!(
            sub.action,
            LegoAction::MuxMedia {
                video: r"C:\v\movie.mp4".into(),
                track: r"C:\v\subs.SRT".into(),
                kind: MuxKind::Subtitles,
            }
        );

        let rev = classify_drop(&[r"C:\v\movie.mkv".into()], r"C:\v\score.flac", false).unwrap();
        assert_eq!(
            rev.action,
            LegoAction::MuxMedia {
                video: r"C:\v\movie.mkv".into(),
                track: r"C:\v\score.flac".into(),
                kind: MuxKind::Audio,
            }
        );
    }

    #[test]
    fn verifies_checksum_and_packs_archives() {
        let sum = classify_drop(&[r"C:\dl\setup.exe".into()], r"C:\dl\SHA256.sha256", false).unwrap();
        assert_eq!(sum.verb, "Verify checksum");
        assert!(matches!(sum.action, LegoAction::VerifyChecksum { .. }));

        let pack = classify_drop(
            &[r"C:\a\notes.txt".into(), r"C:\a\photo.png".into()],
            r"C:\a\bundle.zip",
            false,
        )
        .unwrap();
        assert_eq!(pack.verb, "Pack archive");
        assert!(matches!(pack.action, LegoAction::PackArchive { .. }));
    }

    #[test]
    fn compares_text_or_images_and_rejects_self_or_dirs() {
        let cmp_txt = classify_drop(&[r"C:\a\v2.rs".into()], r"C:\a\v1.rs", false).unwrap();
        assert_eq!(cmp_txt.verb, "Compare");

        let cmp_img = classify_drop(&[r"C:\a\b.webp".into()], r"C:\a\a.png", false).unwrap();
        assert_eq!(cmp_img.verb, "Compare");

        assert!(classify_drop(&[r"C:\a\a.png".into()], r"C:\A\a.png", false).is_none());
        assert!(classify_drop(&[r"C:\a\a.png".into()], r"C:\a\folder", true).is_none());
        assert!(classify_drop(&[r"C:\a\a.png".into()], r"C:\a\pack.zip\b.png", false).is_none());
    }
}
