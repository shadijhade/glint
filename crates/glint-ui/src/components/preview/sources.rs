//! Where the preview gets what takes work: glint-core in the app, fakes in tests.
//!
//! Every method returns a future the view awaits in a GPUI task (as the explorer's `Source` does), so the UI
//! thread never waits. [`Sources::core`] runs them on core's runtime through `core_bridge::run`. The last field,
//! [`Native`], is the preview host (Windows' own preview handlers, `platform::preview_host`), behind the same
//! seam so a test can see what the pane asked of it.

use crate::core_bridge;
use crate::platform::preview_host::{self, Event, PreviewError};
use gpui_kit::{App, Bounds, Global, Pixels, Window};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

/// A future the view awaits on the UI thread (so it needn't be `Send`).
pub type Fut<T> = Pin<Box<dyn Future<Output = T>>>;

/// One thing asked of the outside world about a path.
pub type Ask<T> = Rc<dyn Fn(&str) -> Fut<T>>;

/// Windows' thumbnail of a picture, as core answers: the path, its date and its size.
pub type ThumbFn = Rc<dyn Fn(&str, i64, u64) -> Fut<Result<Vec<u8>, String>>>;

/// `preview_host::show`: the window, the claimant, the pane's bounds, the file, whether the theme is dark.
pub type ShowFn = Rc<
    dyn Fn(&Window, u32, Option<Bounds<Pixels>>, &str, bool) -> Result<async_channel::Receiver<Event>, PreviewError>,
>;

/// What the start of a file says about it (`readTextPreview`'s flags).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// Not text: the preview shows Windows' previewer for it, or its bytes.
    pub binary: bool,
    /// Nothing in it (or only blanks, and the whole file was read).
    pub empty: bool,
}

/// What is directly inside a folder (`FolderPreview`'s `listDir`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub folders: usize,
    pub files: usize,
    /// The files' sizes added up.
    pub bytes: u64,
    /// It has a `.git` (a file in a worktree): a repository's own folder.
    pub repo: bool,
}

/// What an archive holds, from its list of entries (`ArchivePeek`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchiveFacts {
    /// "ZIP", "7z"…
    pub format: String,
    pub packed: u64,
    /// `None` for a big compressed tar, which keeps no list: it is read when opened, not on selection.
    pub entries: Option<usize>,
    pub unpacked: Option<u64>,
}

/// A photo's, video's or song's details (`MediaDetails`): label and value, in the order shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub rows: Vec<(String, String)>,
    /// Latitude and longitude, where it was taken.
    pub place: Option<[f64; 2]>,
}

/// The preview host (Windows' preview handlers in an owned pop-up over the pane), as the views call it.
#[derive(Clone)]
pub struct Native {
    /// `preview_host::show`.
    pub show: ShowFn,
    /// `preview_host::set_rect`.
    pub set_rect: Rc<dyn Fn(u32, Bounds<Pixels>)>,
    /// `preview_host::hide`.
    pub hide: Rc<dyn Fn(u32)>,
    /// `preview_host::reveal`.
    pub reveal: Rc<dyn Fn(u32)>,
    /// `preview_host::unload`.
    pub unload: Rc<dyn Fn(u32)>,
}

impl Native {
    /// `platform::preview_host` itself.
    pub fn host() -> Self {
        Native {
            show: Rc::new(|window, claimant, bounds, path, dark| {
                preview_host::show(window, claimant, bounds, path, dark)
            }),
            set_rect: Rc::new(preview_host::set_rect),
            hide: Rc::new(preview_host::hide),
            reveal: Rc::new(preview_host::reveal),
            unload: Rc::new(preview_host::unload),
        }
    }
}

/// Everything the preview asks of the outside world.
#[derive(Clone)]
pub struct Sources {
    /// The start of a file, to tell text from bytes (`readTextPreview`).
    pub probe: Ask<Result<Probe, String>>,
    /// What is directly inside a folder.
    pub counts: Ask<Result<Counts, String>>,
    /// A file in an archive (or on a remote), copied out to a temp file to be shown: its path (`archiveExtract`).
    pub extract: Ask<Result<String, String>>,
    /// What an archive holds.
    pub archive: Ask<Result<ArchiveFacts, String>>,
    /// A photo's, video's or song's details; `None` for other files or when it says nothing.
    pub media: Ask<Option<Media>>,
    /// Windows' own thumbnail of a picture the preview can't decode itself, large: core's answer (the 8-byte
    /// header, then a BMP). Arguments: the path, its date and size.
    pub thumbnail: ThumbFn,
    /// Why a file in an archive can't be read, as the archive says (a missing volume, a method Glint lacks…).
    pub why_locked: Ask<Option<String>>,
    /// Turns a picture a quarter clockwise, in place.
    pub rotate: Ask<Result<(), String>>,
    /// Sets a picture as the desktop background.
    pub wallpaper: Ask<Result<(), String>>,
    /// The text in a picture, read by Windows' OCR, a line each.
    pub ocr: Ask<Result<Vec<String>, String>>,
    /// Shows a file in File Explorer, selected (`revealItemInDir`).
    pub reveal: Rc<dyn Fn(&str)>,
    /// Windows' preview handlers.
    pub native: Native,
}

/// A core future's answer when it is itself a `Result`: both layers' errors as one.
fn flat<T>(answer: Result<Result<T, String>, String>) -> Result<T, String> {
    answer.and_then(|inner| inner)
}

impl Sources {
    /// glint-core and the real preview host.
    pub fn core(cx: &App) -> Self {
        let app = cx.try_global::<core_bridge::Core>().map(|core| core.app.clone());
        Sources {
            probe: Rc::new(|path| {
                let running = core_bridge::run(glint_core::fsx::read_text_preview(path.to_string(), 4096, None));
                Box::pin(async move {
                    let read = flat(running.await)?;
                    // Blanks alone are "empty" only when that was the whole file.
                    let blank = read.text.trim().is_empty() && !read.truncated;
                    Ok(Probe {
                        binary: read.binary,
                        empty: !read.binary && blank,
                    })
                })
            }),
            counts: Rc::new(|path| {
                let path = path.to_string();
                let running = core_bridge::run(async move {
                    let asked = path.clone();
                    let response = glint_core::fsx::list_dir(path).await.map_err(|e| e.message)?;
                    let bytes = response.into_bytes();
                    glint_core::fsx::blocking(move || {
                        let listing = glint_model::entries::decode_listing(&bytes)
                            .map_err(|e| format!("Glint couldn't read the folder's listing of {asked}: {e}"))?;
                        let mut counts = Counts::default();
                        for entry in &listing.entries {
                            if entry.dir {
                                counts.folders += 1;
                            } else {
                                counts.files += 1;
                                counts.bytes = counts.bytes.saturating_add(entry.size);
                            }
                            counts.repo |= entry.lower == ".git";
                        }
                        Ok(counts)
                    })
                    .await
                });
                Box::pin(async move { flat(running.await) })
            }),
            extract: Rc::new(|path| {
                let running = core_bridge::run(glint_core::archive::archive_extract(
                    path.to_string(),
                    "preview".to_string(),
                ));
                Box::pin(async move { flat(running.await) })
            }),
            archive: Rc::new(|path| {
                let path = path.to_string();
                let running = core_bridge::run(async move {
                    let peek = glint_core::archive::archive_peek(path).await?;
                    serde_json::to_value(peek).map_err(|e| e.to_string())
                });
                Box::pin(async move {
                    let value = flat(running.await)?;
                    let number = |key: &str| value.get(key).and_then(serde_json::Value::as_u64);
                    Ok(ArchiveFacts {
                        format: value
                            .get("format")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        packed: number("packed").unwrap_or(0),
                        entries: number("entries").and_then(|n| usize::try_from(n).ok()),
                        unpacked: number("unpacked"),
                    })
                })
            }),
            media: Rc::new(|path| {
                let running = core_bridge::run(glint_core::media::media_details(path.to_string()));
                Box::pin(async move {
                    let details = flat(running.await).ok().flatten()?;
                    Some(Media {
                        rows: details.rows.into_iter().map(|[label, value]| (label, value)).collect(),
                        place: details.place,
                    })
                })
            }),
            thumbnail: Rc::new(move |path, modified, bytes| {
                let path = path.to_string();
                let app = app.clone();
                let running = core_bridge::run(async move {
                    use glint_core::host::Manager as _;
                    let core = app.ok_or("Glint's engine isn't up")?;
                    let state = core.state::<glint_core::thumbs::Thumbs>();
                    glint_core::thumbs::thumbnail(
                        state,
                        path,
                        1600,
                        modified as f64,
                        bytes as f64,
                        false,
                        "preview".to_string(),
                        0,
                    )
                    .await
                    .map(glint_core::host::ipc::Response::into_bytes)
                });
                Box::pin(async move { flat(running.await) })
            }),
            why_locked: Rc::new(|path| {
                let running = core_bridge::run(glint_core::fsx::read_text_preview(path.to_string(), 1, None));
                Box::pin(async move {
                    match running.await {
                        Ok(Err(reason)) => Some(reason),
                        _ => None,
                    }
                })
            }),
            rotate: Rc::new(|path| {
                let running = core_bridge::run(glint_core::shell::rotate_image(path.to_string(), true));
                Box::pin(async move { flat(running.await) })
            }),
            wallpaper: Rc::new(|path| {
                let running = core_bridge::run(glint_core::shell::set_desktop_wallpaper(path.to_string()));
                Box::pin(async move { flat(running.await) })
            }),
            ocr: Rc::new(|path| {
                let running = core_bridge::run(glint_core::ocr::ocr_text(path.to_string()));
                Box::pin(async move { flat(running.await) })
            }),
            reveal: Rc::new(reveal_in_file_explorer),
            native: Native::host(),
        }
    }
}

/// Opens File Explorer with `path` selected (`/select,`), as `revealItemInDir` did.
fn reveal_in_file_explorer(path: &str) {
    let path = path.to_string();
    // Explorer is a separate process; starting it waits for nothing, but still not on the UI thread.
    std::thread::spawn(move || {
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{path}"))
            .spawn();
    });
}

/// The sources in use, in a GPUI global: [`sources`] reads them.
struct Shared(Sources);

impl Global for Shared {}

/// Sets the sources the preview uses (tests; the app uses [`Sources::core`] without this).
pub fn init_with(cx: &mut App, sources: Sources) {
    cx.set_global(Shared(sources));
}

/// The sources in use: glint-core's unless [`init_with`] set others.
pub fn sources(cx: &mut App) -> Sources {
    if let Some(shared) = cx.try_global::<Shared>() {
        return shared.0.clone();
    }
    let core = Sources::core(cx);
    init_with(cx, core);
    cx.global::<Shared>().0.clone()
}
