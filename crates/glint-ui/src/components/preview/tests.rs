//! Headless tests of the preview pane and Quick Look: the right previewer per kind, the states, what the host of
//! Windows' previewers is asked, Quick Look opening, moving and closing, and the same with the Arabic interface.
//! A fake disk stands behind the explorer and fakes behind the sources; the pictures are real files in a temp
//! folder.

use super::choose::{Body, View};
use super::frame::{chosen, EntryView};
use super::notebook::NotebookView;
use super::pane::{PreviewPane, Shown};
use super::picture::PictureView;
use super::probe::{Probed, TextProbe};
use super::program::ProgramPreview;
use super::quick_look;
use super::sheet::SheetPreview;
use super::shell::{self, ShellHost, State, PANE, QUICK_LOOK};
use super::small::SmallPreview;
use super::sources::{self, ArchiveFacts, Counts, Media, Native, Probe, Sources};
use super::sqlite::{SqliteEvent, SqlitePreview};
use crate::components::file_grid::thumbnail::{self, NoThumbs};
use crate::components::git::model::{Fut, Source as GitSource, View as GitView};
use crate::components::git::{self, ChangesView};
use crate::dialogs::data_tools::{ProfileView, QueryView};
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, CompressFormat, Explorer, Measured, NavOptions};
use crate::kit::prelude::*;
use crate::platform::preview_host::{Event, PreviewError};
use crate::testing;
use glint_model::entries::{attr, FileEntry, GitMark};
use glint_model::groups::GitMarks;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{BackgroundExecutor, Entity, TestAppContext, VisualTestContext};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A 24-bit BMP of one color: the simplest picture to make by hand, and one GPUI decodes.
pub(crate) fn bmp(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let mut out = Vec::new();
    out.extend(b"BM");
    out.extend((54 + row * height).to_le_bytes());
    out.extend([0; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend(width.to_le_bytes());
    out.extend(height.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0; 4]);
    out.extend((row * height).to_le_bytes());
    out.extend([0; 16]);
    for _ in 0..height {
        for _ in 0..width {
            out.extend([rgb[2], rgb[1], rgb[0]]);
        }
        out.extend(std::iter::repeat_n(0u8, (row - width * 3) as usize));
    }
    out
}

/// Core's thumbnail answer: the 8-byte header, then a BMP.
pub(crate) fn thumbnail_answer(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = vec![0, 1, 0, 0];
    out.extend(1.5f32.to_le_bytes());
    out.extend(bmp(width, height, rgb));
    out
}

/// What the fake sources answer, and what they were asked.
#[derive(Default)]
pub(crate) struct Fake {
    pub probes: RefCell<HashMap<String, Result<Probe, String>>>,
    pub counts: RefCell<Option<Result<Counts, String>>>,
    pub archive: RefCell<Option<Result<ArchiveFacts, String>>>,
    pub media: RefCell<Option<Media>>,
    pub thumbnail: RefCell<Option<Vec<u8>>>,
    pub lines: RefCell<Vec<String>>,
    pub extracted: RefCell<HashMap<String, String>>,
    /// Everything the pane asked of the sources, in order: "rotate <path>", "show 1 <path>"…
    pub log: RefCell<Vec<String>>,
    /// The channels of the previews shown, to send the host's events on.
    pub senders: RefCell<Vec<async_channel::Sender<Event>>>,
    /// The host has no window to preview in.
    pub no_window: Cell<bool>,
    /// The last rectangle given to the host.
    pub rect: Cell<Option<Bounds<Pixels>>>,
}

impl Fake {
    pub fn logged(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    pub fn count(&self, what: &str) -> usize {
        self.log.borrow().iter().filter(|line| line.starts_with(what)).count()
    }

    pub fn sources(self: &Rc<Self>) -> Sources {
        let fake = self.clone();
        let note = |fake: &Rc<Fake>, line: String| fake.log.borrow_mut().push(line);
        Sources {
            probe: {
                let fake = fake.clone();
                Rc::new(move |path| {
                    let answer = fake
                        .probes
                        .borrow()
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| Ok(Probe::default()));
                    Box::pin(async move { answer })
                })
            },
            counts: {
                let fake = fake.clone();
                Rc::new(move |_| {
                    let answer = fake.counts.borrow().clone().unwrap_or(Ok(Counts {
                        folders: 1,
                        files: 2,
                        bytes: 3000,
                        repo: false,
                    }));
                    Box::pin(async move { answer })
                })
            },
            extract: {
                let fake = fake.clone();
                Rc::new(move |path| {
                    let answer = fake
                        .extracted
                        .borrow()
                        .get(path)
                        .cloned()
                        .ok_or_else(|| "It isn't in the archive".to_string());
                    Box::pin(async move { answer })
                })
            },
            archive: {
                let fake = fake.clone();
                Rc::new(move |_| {
                    let answer = fake.archive.borrow().clone().unwrap_or(Ok(ArchiveFacts {
                        format: "ZIP".into(),
                        packed: 2048,
                        entries: Some(3),
                        unpacked: Some(5000),
                    }));
                    Box::pin(async move { answer })
                })
            },
            media: {
                let fake = fake.clone();
                Rc::new(move |_| {
                    let answer = fake.media.borrow().clone();
                    Box::pin(async move { answer })
                })
            },
            thumbnail: {
                let fake = fake.clone();
                Rc::new(move |path, _, _| {
                    note(&fake, format!("thumbnail {path}"));
                    let answer = fake.thumbnail.borrow().clone().ok_or_else(|| "none".to_string());
                    Box::pin(async move { answer })
                })
            },
            why_locked: Rc::new(|_| Box::pin(async { Some("This archive needs its other volume.".to_string()) })),
            rotate: {
                let fake = fake.clone();
                Rc::new(move |path| {
                    note(&fake, format!("rotate {path}"));
                    Box::pin(async { Ok(()) })
                })
            },
            wallpaper: {
                let fake = fake.clone();
                Rc::new(move |path| {
                    note(&fake, format!("wallpaper {path}"));
                    Box::pin(async { Ok(()) })
                })
            },
            ocr: {
                let fake = fake.clone();
                Rc::new(move |_| {
                    let lines = fake.lines.borrow().clone();
                    Box::pin(async move { Ok(lines) })
                })
            },
            reveal: {
                let fake = fake.clone();
                Rc::new(move |path| note(&fake, format!("reveal {path}")))
            },
            native: Native {
                show: {
                    let fake = fake.clone();
                    Rc::new(move |_window, claimant, bounds, path, _dark| {
                        note(&fake, format!("show {claimant} {path}"));
                        fake.rect.set(bounds);
                        if fake.no_window.get() {
                            return Err(PreviewError::NoWindow);
                        }
                        let (tx, rx) = async_channel::unbounded();
                        fake.senders.borrow_mut().push(tx);
                        Ok(rx)
                    })
                },
                set_rect: {
                    let fake = fake.clone();
                    Rc::new(move |claimant, bounds| {
                        note(&fake, format!("set_rect {claimant}"));
                        fake.rect.set(Some(bounds));
                    })
                },
                hide: {
                    let fake = fake.clone();
                    Rc::new(move |claimant| note(&fake, format!("hide {claimant}")))
                },
                reveal: {
                    let fake = fake.clone();
                    Rc::new(move |claimant| note(&fake, format!("reveal_host {claimant}")))
                },
                unload: {
                    let fake = fake.clone();
                    Rc::new(move |claimant| note(&fake, format!("unload {claimant}")))
                },
            },
        }
    }
}

/// Git's answers for the folder on screen: the marks it says its files have (none: not a repository).
#[derive(Clone, Default)]
pub(crate) struct GitFake(Arc<Mutex<Option<GitMarks>>>);

impl GitFake {
    fn marks(&self) -> Option<GitMarks> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

impl GitSource for GitFake {
    fn view(&self, folder: String, _: String, _: Vec<String>, _: bool) -> Fut<Option<GitView>> {
        let marks = self.marks();
        Box::pin(async move {
            Ok(marks.map(|marks| GitView {
                root: folder,
                branch: Some("main".into()),
                marks: Arc::new(marks),
                ..Default::default()
            }))
        })
    }

    fn cancel(&self, _: &str) {}

    fn watch(&self, _: Vec<String>) {}

    fn diff(&self, _: String) -> Fut<Option<glint_core::git::FileDiff>> {
        Box::pin(async { Ok(None) })
    }

    fn head_copy(&self, _: String) -> Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }

    fn head_discard(&self, _: String) {}

    fn card(&self, _: String) -> Fut<Option<glint_core::repo::RepoCard>> {
        Box::pin(async { Ok(None) })
    }
}

/// The pane in a window over a fake disk, with real pictures in a temp folder.
pub(crate) struct Env {
    pub visual: VisualTestContext,
    pub executor: BackgroundExecutor,
    pub fake: Rc<Fake>,
    pub disk: FakeDisk,
    pub git: GitFake,
    pub explorer: Entity<Explorer>,
    pub pane: Entity<PreviewPane>,
    pub dir: PathBuf,
    pub folder: String,
}

static FOLDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Env {
    /// The path of a file in the folder.
    pub fn path(&self, name: &str) -> String {
        format!("{}\\{name}", self.folder)
    }

    /// Selects one item, as a click would, and lets the pane settle on it.
    pub fn select(&mut self, name: &str) {
        let path = self.path(name);
        self.explorer
            .update(&mut self.visual, |e, cx| e.select_only(Some(&path), cx));
        self.settle();
    }

    /// Lets 100 ms pass (the pane's 70 ms rest) and everything that was asked of the background finish.
    pub fn settle(&mut self) {
        for _ in 0..3 {
            self.executor.advance_clock(Duration::from_millis(100));
            for _ in 0..40 {
                self.visual.run_until_parked();
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        self.visual.update(|window, cx| window.render_frame(cx));
        self.visual.run_until_parked();
    }

    /// Runs `check` after letting things settle, again and again, until it holds.
    pub fn until(&mut self, what: &str, mut check: impl FnMut(&mut Env) -> bool) {
        for _ in 0..200 {
            self.executor.advance_clock(Duration::from_millis(100));
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
            if check(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never happened: {what}");
    }

    pub fn has(&mut self, label: &str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    pub fn has_id(&mut self, id: &'static str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    pub fn click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.visual.run_until_parked();
    }

    pub fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.visual.run_until_parked();
    }

    pub fn entry_view(&mut self) -> Option<Entity<EntryView>> {
        self.pane.read_with(&self.visual, |pane, _| pane.entry_view().cloned())
    }

    pub fn body(&mut self) -> Option<Body> {
        self.entry_view()
            .and_then(|view| view.read_with(&self.visual, |v, _| v.body()))
    }

    pub fn shown(&mut self) -> Shown {
        self.pane.read_with(&self.visual, |pane, _| pane.shown().clone())
    }

    /// The previewer's view, as a view of type `T`.
    pub fn body_as<T: 'static + Render>(&mut self) -> Option<Entity<T>> {
        let view = self.entry_view()?;
        let body = view.read_with(&self.visual, |v, _| v.body_view().cloned())?;
        body.downcast::<T>().ok()
    }

    /// Says Git has these marks (lowercase name, mark) for the folder on screen, and lets the model hear it.
    pub fn git_marks(&mut self, marks: &[(&str, GitMark)]) {
        let mut all = GitMarks::new();
        for (name, mark) in marks {
            all.insert((*name).to_string(), *mark);
        }
        *self.git.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(all);
        let tab = self.explorer.read_with(&self.visual, |e, _| e.active_tab_id());
        let folder = self.folder.clone();
        // (A folder that was not a repository has no run to refresh: the model is asked outright.)
        self.visual
            .update(|_, cx| git::shared(cx).update(cx, |git, cx| git.refresh(tab, &folder, true, cx)));
        let probe = self.path("notes.txt");
        self.until("git has the marks", |env| {
            env.visual
                .update(|_, cx| git::mark_of(cx, &probe).is_some() || marks.is_empty())
        });
    }

    pub fn picture(&mut self) -> Entity<PictureView> {
        self.body_as::<PictureView>().expect("a picture is shown")
    }

    pub fn dialog_open(&mut self) -> bool {
        self.visual.update(|window, cx| window.has_active_dialog(cx))
    }
}

fn entries(dir: &str) -> Vec<FileEntry> {
    let mut docx = file(dir, "report.doc", 4096, 3);
    docx.attrs = 0;
    let mut offline = file(dir, "cloud.txt", 10, 5);
    offline.attrs |= attr::OFFLINE;
    let mut secret = file(dir, "locked.txt", 10, 6);
    secret.attrs |= attr::IN_ARCHIVE | attr::ENCRYPTED;
    let mut stuck = file(dir, "stuck.txt", 10, 7);
    stuck.attrs |= attr::IN_ARCHIVE | attr::UNREADABLE;
    let mut binned = file(dir, "gone.txt", 40, 13);
    binned.attrs |= attr::RECYCLED;
    binned.origin = Some(r"C:\Users\a\Documents".into());
    let mut binned_folder = folder(dir, "old");
    binned_folder.attrs |= attr::RECYCLED;
    binned_folder.size = 123_456;
    let mut inside = file(dir, "inside.txt", 10, 14);
    inside.attrs |= attr::IN_ARCHIVE;
    vec![
        binned,
        binned_folder,
        inside,
        folder(dir, "sub"),
        file(dir, "notes.txt", 200, 1_700_000_000_000),
        file(dir, "readme.md", 300, 1_700_000_100_000),
        file(dir, "data.json", 400, 4),
        file(dir, "photo.bmp", 500, 2),
        file(dir, "blob.zzqq", 600, 9),
        file(dir, "pack.zip", 700, 8),
        file(dir, "movie.mp4", 800, 10),
        file(dir, "book.mobi", 160, 21),
        file(dir, "تقرير الربع.txt", 900, 11),
        file(dir, "صورة.bmp", 1000, 12),
        file(dir, "table.csv", 100, 15),
        file(dir, "sheet.xlsx", 110, 16),
        file(dir, "lab.ipynb", 120, 17),
        file(dir, "tool.exe", 130, 18),
        file(dir, "base.sqlite", 140, 19),
        file(dir, "card.vcf", 150, 20),
        file(dir, "rows.parquet", 160, 21),
        docx,
        offline,
        secret,
        stuck,
    ]
}

/// The pane over a fake disk. The folder is a real temp folder holding `photo.bmp` (4 x 2, 0x336699) and
/// `صورة.bmp`.
pub(crate) fn setup(cx: &mut TestAppContext) -> Env {
    cx.update(testing::init);
    let dir = std::env::temp_dir().join(format!(
        "glint-preview-{}-{}",
        std::process::id(),
        FOLDERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("the temp folder is made");
    std::fs::write(dir.join("photo.bmp"), bmp(4, 2, [0x33, 0x66, 0x99])).expect("the picture is written");
    std::fs::write(dir.join("صورة.bmp"), bmp(8, 4, [0x10, 0x20, 0x30])).expect("the Arabic picture is written");
    let folder_path = dir.to_string_lossy().into_owned();
    let disk = FakeDisk::new();
    disk.folder(&folder_path, entries(&folder_path));
    disk.folder(&format!("{folder_path}\\sub"), vec![]);
    let fake = Rc::new(Fake::default());
    let git = GitFake::default();
    let git_source = git.clone();
    cx.update(|cx| {
        explorer::init_with(cx, disk.source());
        sources::init_with(cx, fake.sources());
        thumbnail::init_with(cx, Rc::new(NoThumbs));
        git::init_with(cx, Rc::new(git_source));
    });
    let executor = cx.executor();
    let (mut visual, pane) = testing::open_window(cx, |window, cx| cx.new(|cx| PreviewPane::new(window, cx)));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let target = folder_path.clone();
    explorer.update(&mut visual, |e, cx| {
        e.navigate(target, NavOptions::default(), cx);
    });
    visual.run_until_parked();
    let mut env = Env {
        visual,
        executor,
        fake,
        disk,
        git,
        explorer,
        pane,
        dir,
        folder: folder_path,
    };
    env.settle();
    env
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---- The pane's states --------------------------------------------------------------------------------------

#[gpui_kit::test]
fn nothing_selected_shows_the_folders_card(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    assert_eq!(env.shown(), Shown::Nothing);
    assert!(env.has("Select a file to preview it"));
    assert!(env.has("25 items"), "the folder's item count is said");
    assert!(env.entry_view().is_none());
    // The pane is a named region.
    assert!(env.has("Preview pane"));
}

#[gpui_kit::test]
fn one_file_is_shown_once_the_selection_has_rested(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let path = env.path("notes.txt");
    env.explorer
        .update(&mut env.visual, |e, cx| e.select_only(Some(&path), cx));
    env.visual.run_until_parked();
    assert_eq!(env.shown(), Shown::Nothing, "arrowing past a file doesn't load it");
    env.settle();
    assert!(matches!(env.shown(), Shown::One(ref entry) if entry.name == "notes.txt"));
    assert_eq!(env.body(), Some(Body::Text));
    assert!(env.has("notes.txt"), "the header names the file");
    // The details under it: type, size, date, place.
    assert!(env.has(&format!("Location: {}", env.folder)));
    assert!(env.has("Size: 200 B (200 bytes)"));
    // The start of the file is text, so the text previewer is up behind the probe.
    let probe = env.body_as::<TextProbe>().expect("text goes through the probe");
    probe.read_with(&env.visual, |probe, _| {
        assert!(matches!(probe.state(), Probed::Text(_)))
    });
}

#[gpui_kit::test]
fn clearing_the_selection_brings_the_folders_card_back_at_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("notes.txt");
    assert!(env.entry_view().is_some());
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.visual.run_until_parked();
    assert_eq!(env.shown(), Shown::Nothing);
    assert!(env.entry_view().is_none());
    assert!(env.has("Select a file to preview it"));
}

#[gpui_kit::test]
fn several_items_show_a_deck_and_their_totals(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let paths = [env.path("sub"), env.path("notes.txt"), env.path("readme.md")];
    env.explorer.update(&mut env.visual, |e, cx| {
        e.set_selection(paths.iter().map(String::as_str), None, None, cx);
    });
    env.settle();
    let Shown::Many(many) = env.shown() else {
        panic!("several items are selected");
    };
    assert_eq!((many.count, many.folders, many.files, many.bytes), (3, 1, 2, 500));
    assert!(many.writable);
    assert_eq!(many.deck.len(), 3);
    assert!(env.has("3 items selected"));
    assert!(env.has("1 folder, 2 files · 500 B"));
    // Rename is offered for items on disk.
    assert!(env.has_id("preview-rename"));
}

#[gpui_kit::test]
fn a_folder_shows_what_is_directly_inside_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("sub");
    assert_eq!(env.body(), Some(Body::Folder));
    assert!(env.has("1 folder, 2 files"));
    assert!(env.has("2.9 KB in files directly inside"));
    // A folder that can't be listed says why.
    *env.fake.counts.borrow_mut() = Some(Err("Access is denied.".into()));
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.select("sub");
    // (The same folder shown again keeps its card.)
    assert_eq!(env.body(), Some(Body::Folder));
}

#[gpui_kit::test]
fn an_archive_says_what_it_holds_and_opens_like_a_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("pack.zip");
    assert_eq!(env.body(), Some(Body::Archive));
    assert!(env.has("3 files and folders"));
    assert!(env.has("4.9 KB when extracted"));
    assert!(env.has("2.0 KB as a ZIP file"));
    assert!(env.has("Open like a folder"));
    // A big compressed tar keeps no list.
    *env.fake.archive.borrow_mut() = Some(Ok(ArchiveFacts {
        format: "TAR".into(),
        packed: 10_000_000,
        entries: None,
        unpacked: None,
    }));
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.select("pack.zip");
    assert!(env.has("9.5 MB as a TAR file"));
    assert!(env.has("Opening it reads it from start to end once, to list what's in it."));
}

#[gpui_kit::test]
fn files_that_cannot_be_read_say_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    // Online-only: opening it downloads it.
    env.select("cloud.txt");
    assert_eq!(env.body(), Some(Body::OnlineOnly));
    assert!(env.has("This file is online-only. Opening it downloads it."));
    // Password-protected in an archive: the password box is there.
    env.select("locked.txt");
    assert_eq!(env.body(), Some(Body::Locked));
    assert!(env.has("This file is password-protected."));
    assert!(env.has("Password"), "the password box is labelled");
    // Unreadable for another reason: the archive's own words.
    env.select("stuck.txt");
    env.until("the reason arrives", |env| {
        env.has("This archive needs its other volume.")
    });
}

#[gpui_kit::test]
fn a_file_that_is_empty_or_unreadable_is_told_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let path = env.path("notes.txt");
    env.fake.probes.borrow_mut().insert(
        path,
        Ok(Probe {
            binary: false,
            empty: true,
        }),
    );
    env.select("notes.txt");
    assert!(env.has("This file is empty."));
    let path = env.path("readme.md");
    env.fake
        .probes
        .borrow_mut()
        .insert(path, Err("Access is denied. (os error 5)".into()));
    env.select("readme.md");
    assert!(env.has("Access is denied. (os error 5)"));
}

// ---- Views --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn markdown_is_rendered_and_can_be_shown_as_written_or_as_bytes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("readme.md");
    assert_eq!(env.body(), Some(Body::Markdown));
    // The "Show" group, with Rendered chosen.
    assert!(env.has("Show"));
    assert!(env.has("Rendered") && env.has("Source") && env.has("Hex"));
    env.click(("preview-view", 1usize));
    assert_eq!(env.body(), Some(Body::Text));
    env.click(("preview-view", 2usize));
    assert_eq!(env.body(), Some(Body::Hex));
    // The choice is kept from file to file; a file without that view is shown as it is.
    env.select("notes.txt");
    assert_eq!(env.body(), Some(Body::Hex), "a text file has bytes too");
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.select("sub");
    assert_eq!(env.body(), Some(Body::Folder), "a folder has no bytes");
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

#[gpui_kit::test]
fn json_is_a_tree_with_a_text_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("data.json");
    assert_eq!(env.body(), Some(Body::Data));
    assert!(env.has("Tree") && env.has("Text"));
    env.click(("preview-view", 1usize));
    assert_eq!(env.body(), Some(Body::Text));
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

// ---- The previewers of the tables, programs and small files -------------------------------------------------

#[gpui_kit::test]
fn each_new_previewer_is_built_for_its_kind(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("sheet.xlsx");
    assert_eq!(env.body(), Some(Body::Sheet));
    assert!(env.body_as::<SheetPreview>().is_some());
    env.select("rows.parquet");
    assert_eq!(env.body(), Some(Body::Sheet));
    env.select("lab.ipynb");
    assert_eq!(env.body(), Some(Body::Notebook));
    assert!(env.body_as::<NotebookView>().is_some());
    env.select("tool.exe");
    assert_eq!(env.body(), Some(Body::Program));
    assert!(env.body_as::<ProgramPreview>().is_some());
    env.select("base.sqlite");
    assert_eq!(env.body(), Some(Body::Sqlite));
    assert!(env.body_as::<SqlitePreview>().is_some());
    env.select("card.vcf");
    assert!(matches!(env.body(), Some(Body::Small(_))));
    assert!(env.body_as::<SmallPreview>().is_some());
}

#[gpui_kit::test]
fn a_program_is_handed_the_signature_chip_for_its_header(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("tool.exe");
    let program = env.body_as::<ProgramPreview>().expect("a program is shown");
    assert!(program.read_with(&env.visual, |p, _| p.has_signature()));
}

#[gpui_kit::test]
fn a_workbook_shows_a_table_or_windows_previewer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("sheet.xlsx");
    assert!(env.has("Table") && env.has("Previewer") && env.has("Query") && env.has("Hex"));
    assert!(!env.has("Profile"), "a workbook is not a DuckDB table");
    // Previewer is the second button: Windows' own, over the icon.
    env.click(("preview-view", 1usize));
    assert_eq!(env.body(), Some(Body::Shell(super::choose::Fallback::Icon)));
    assert!(env
        .fake
        .logged()
        .contains(&format!("show {PANE} {}", env.path("sheet.xlsx"))));
    // Parquet has no Previewer.
    env.select("rows.parquet");
    assert_eq!(
        env.body(),
        Some(Body::Sheet),
        "the view is shown as the file as it is where it has none"
    );
    assert!(env.has("Profile") && !env.has("Previewer"));
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

#[gpui_kit::test]
fn a_notebook_has_a_source_view_that_is_its_json(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("lab.ipynb");
    assert!(env.has("Notebook") && env.has("Source"));
    env.click(("preview-view", 1usize));
    assert_eq!(env.body(), Some(Body::Text));
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

#[gpui_kit::test]
fn a_table_file_takes_sql_and_a_profile(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("table.csv");
    assert_eq!(env.body(), Some(Body::Data));
    assert!(env.has("Table") && env.has("Text") && env.has("Query") && env.has("Profile") && env.has("Hex"));
    // Table, Text, Query, Profile, Hex.
    env.click(("preview-view", 2usize));
    assert_eq!(env.body(), Some(Body::Query));
    assert!(env.body_as::<QueryView>().is_some());
    env.click(("preview-view", 3usize));
    assert_eq!(env.body(), Some(Body::Profile));
    assert!(env.body_as::<ProfileView>().is_some());
    // A file with no Query view is shown as it is when the choice was Profile.
    env.select("notes.txt");
    assert_eq!(env.body(), Some(Body::Text));
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

#[gpui_kit::test]
fn a_databases_rows_button_opens_the_query_view_with_its_sql(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("base.sqlite");
    assert!(env.has("Query"));
    let preview = env.body_as::<SqlitePreview>().expect("a database is shown");
    let sql = "SELECT * FROM notes LIMIT 1000".to_string();
    let path = env.path("base.sqlite");
    preview.update(&mut env.visual, |_, cx| {
        cx.emit(SqliteEvent::ShowQuery {
            path: path.clone(),
            sql: sql.clone(),
        })
    });
    env.visual.run_until_parked();
    assert_eq!(env.body(), Some(Body::Query), "the frame switched to its Query view");
    let frame = env.entry_view().expect("shown");
    frame.read_with(&env.visual, |v, _| assert_eq!(v.draft(), Some(sql.as_str())));
    assert!(env.body_as::<QueryView>().is_some());
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

#[gpui_kit::test]
fn a_file_git_has_a_change_for_has_a_changes_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("notes.txt");
    assert!(!env.has("Changes"), "outside a repository there are no changes to show");
    env.git_marks(&[("notes.txt", GitMark::Modified), ("readme.md", GitMark::Ignored)]);
    assert!(env.has("Changes"), "the button arrives with the mark");
    let index = {
        let frame = env.entry_view().expect("shown");
        frame.read_with(&env.visual, |v, _| assert!(v.has_changes()));
        // File, Changes, Hex.
        1usize
    };
    env.click(("preview-view", index));
    assert_eq!(env.body(), Some(Body::Changes));
    assert!(env.body_as::<ChangesView>().is_some());
    // An ignored file has none, and neither has a folder.
    env.select("readme.md");
    assert!(!env.has("Changes"));
    env.select("sub");
    assert!(!env.has("Changes"));
    let view = env.visual.update(|_, cx| chosen(cx));
    view.update(&mut env.visual, |c, _| c.view = View::File);
}

// ---- Compare, File X-Ray and Zip ----------------------------------------------------------------------------

#[gpui_kit::test]
fn x_ray_is_offered_for_files_on_disk_and_opens_its_dialog(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("notes.txt");
    assert!(env.has_id("preview-xray"));
    assert!(
        !env.has_id("preview-compare"),
        "one file alone has nothing to compare with"
    );
    env.click("preview-xray");
    assert!(env.dialog_open());
    // Not for a folder, a file in an archive or the Recycle Bin.
    env.visual.update(|window, cx| window.close_dialog(cx));
    for name in ["sub", "inside.txt", "gone.txt"] {
        env.select(name);
        assert!(!env.has_id("preview-xray"), "{name}");
    }
}

#[gpui_kit::test]
fn two_files_selected_can_be_compared_from_the_pane_and_from_quick_look(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let (a, b) = (env.path("notes.txt"), env.path("readme.md"));
    env.explorer.update(&mut env.visual, |e, cx| {
        e.set_selection([a.as_str(), b.as_str()], Some(&a), Some(&a), cx);
    });
    env.settle();
    assert!(matches!(env.shown(), Shown::Many(_)));
    assert!(env.has_id("preview-compare") && env.has_id("preview-zip") && env.has_id("preview-rename"));
    // Quick Look shows the focused one, with the pair to compare in its header.
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    let ql = env.visual.update(|_, cx| quick_look::current(cx)).expect("it is open");
    let frame = ql.read_with(&env.visual, |q, _| q.view().clone());
    let pair = frame
        .read_with(&env.visual, |v, _| v.compare_pair().cloned())
        .expect("a pair");
    assert_eq!((pair.left.as_str(), pair.right.as_str()), (a.as_str(), b.as_str()));
    assert!(env.has("Compare “notes.txt” and “readme.md”"));
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    // The pane's Compare opens the two files side by side.
    env.click("preview-compare");
    assert!(env.dialog_open(), "the comparison opened");
}

#[gpui_kit::test]
fn a_folder_in_the_selection_leaves_compare_out_and_zip_packs_the_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let paths = [env.path("sub"), env.path("notes.txt")];
    env.explorer.update(&mut env.visual, |e, cx| {
        e.set_selection(paths.iter().map(String::as_str), None, None, cx);
    });
    env.settle();
    assert!(env.has_id("preview-zip"));
    assert!(!env.has_id("preview-compare"), "a folder can't be compared");
    env.click("preview-zip");
    env.until("the zip was asked for", |env| !env.disk.compressions().is_empty());
    let (request, _) = env.disk.compressions().remove(0);
    assert_eq!(request.format, CompressFormat::Zip);
    assert_eq!(request.paths.len(), 2);
}

#[gpui_kit::test]
fn items_that_cannot_be_changed_are_not_offered_zip_or_compare(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let paths = [env.path("gone.txt"), env.path("notes.txt")];
    env.explorer.update(&mut env.visual, |e, cx| {
        e.set_selection(paths.iter().map(String::as_str), None, None, cx);
    });
    env.settle();
    assert!(matches!(env.shown(), Shown::Many(_)));
    assert!(!env.has_id("preview-zip") && !env.has_id("preview-compare") && !env.has_id("preview-rename"));
}

// ---- Folder sizes and the repository's card -----------------------------------------------------------------

#[gpui_kit::test]
fn a_folder_offers_to_calculate_folder_sizes_and_shows_its_size_once_measured(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.disk.set_measured(
        &env.folder.clone(),
        Measured {
            total: 8192,
            sizes: vec![("sub".to_string(), 4096)],
            elapsed: 3.0,
            engine: "pdu".to_string(),
        },
    );
    env.select("sub");
    assert!(env.has("Calculate folder sizes"));
    assert!(!env.has("4.0 KB in total"));
    env.click("preview-folder-sizes");
    env.until("the size arrives", |env| env.has("4.0 KB in total"));
    assert!(
        !env.has("Calculate folder sizes"),
        "a measured folder has its size instead"
    );
    // Nothing selected: the folder's card says what the whole folder came to.
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.until("the folder's total", |env| env.has("25 items · 8.0 KB in total"));
}

#[gpui_kit::test]
fn a_folder_in_the_recycle_bin_has_its_own_size_and_no_button(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("old");
    let size = glint_model::format::format_size(123_456);
    assert!(env.has(&format!("{size} in total")));
    assert!(!env.has("Calculate folder sizes"));
}

#[gpui_kit::test]
fn a_folder_that_is_not_where_the_tab_is_has_no_calculate_button(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let env = setup(cx);
    // A search result's folder is somewhere else than the folder on screen: sizing here would not reach it.
    let (mut visual, _card) = testing::open_window(cx, |window, cx| {
        cx.new(|cx| super::cards::FolderCard::new(folder(r"C:\elsewhere", "far"), window, cx))
    });
    visual.run_until_parked();
    let found = visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label("Calculate folder sizes").is_some()
    });
    assert!(!found);
    drop(env);
}

#[gpui_kit::test]
fn a_folder_with_a_git_folder_has_the_repository_card_under_its_counts(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    *env.fake.counts.borrow_mut() = Some(Ok(Counts {
        folders: 1,
        files: 2,
        bytes: 3000,
        repo: true,
    }));
    env.select("sub");
    let card = env.body_as::<super::cards::FolderCard>().expect("a folder");
    env.until("the card is made", |env| {
        card.read_with(&env.visual, |c, _| c.repo().is_some())
    });
    // Not for a folder in the Recycle Bin.
    env.select("old");
    let card = env.body_as::<super::cards::FolderCard>().expect("a folder");
    env.settle();
    card.read_with(&env.visual, |c, _| assert!(c.repo().is_none()));
}

#[gpui_kit::test]
fn the_folder_card_of_a_repositorys_own_folder_has_the_repository_card(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    assert!(env.pane.read_with(&env.visual, |p, _| p.repo_card().is_none()));
    // The folder shown gets a .git: the pane scans its listing for one.
    let dir = env.folder.clone();
    let mut entries = entries(&dir);
    entries.push(folder(&dir, ".git"));
    env.disk.folder(&dir, entries);
    env.explorer.update(&mut env.visual, |e, cx| e.refresh(cx));
    env.until("the pane knows it is a repository", |env| {
        env.pane.read_with(&env.visual, |p, _| p.summary().repo)
    });
    env.until("its card is made", |env| {
        env.pane.read_with(&env.visual, |p, _| p.repo_card().is_some())
    });
    // Selecting a file lets it go, and clearing the selection brings it back.
    env.select("notes.txt");
    assert!(env.pane.read_with(&env.visual, |p, _| p.repo_card().is_none()));
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.visual.run_until_parked();
    assert!(env.pane.read_with(&env.visual, |p, _| p.repo_card().is_some()));
}

// ---- Windows' own previewers --------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_binary_file_goes_to_windows_previewer_over_its_bytes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let path = env.path("blob.zzqq");
    env.fake.probes.borrow_mut().insert(
        path.clone(),
        Ok(Probe {
            binary: true,
            empty: false,
        }),
    );
    env.select("blob.zzqq");
    let probe = env.body_as::<TextProbe>().expect("an unknown extension is probed");
    let host = probe.read_with(&env.visual, |p, _| match p.state() {
        Probed::Binary(view) => view.clone().downcast::<ShellHost>().ok(),
        _ => None,
    });
    let host = host.expect("windows' previewer is asked for");
    assert!(
        env.fake.logged().contains(&format!("show {PANE} {path}")),
        "{:?}",
        env.fake.logged()
    );
    let bounds = host
        .read_with(&env.visual, |h, _| h.bounds())
        .expect("the box was measured");
    assert!(bounds.size.width > px(100.) && bounds.size.height > px(100.));
    assert_eq!(
        env.fake.rect.get().map(|r| r.size),
        Some(bounds.size),
        "the host was told the box's rectangle"
    );
}

#[gpui_kit::test]
fn the_host_follows_the_box_hides_for_dialogs_and_menus_and_lets_go_when_the_item_changes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("report.doc");
    assert_eq!(env.body(), Some(Body::Shell(super::choose::Fallback::Icon)));
    let path = env.path("report.doc");
    assert_eq!(env.fake.count(&format!("show {PANE} {path}")), 1);
    let first = env.fake.rect.get().expect("shown over a rectangle");

    // A frame at the same size doesn't move it again.
    let moved = env.fake.count("set_rect");
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        env.fake.count("set_rect"),
        moved + 1,
        "set_rect is told each frame; the host ignores the same one"
    );

    // The window grows: the rectangle follows.
    env.visual.simulate_resize(size(px(1200.), px(800.)));
    env.visual.update(|window, cx| window.render_frame(cx));
    let second = env.fake.rect.get().expect("moved");
    assert!(second.size.width > first.size.width, "{first:?} -> {second:?}");

    // A dialog covers the window: hidden, then shown again.
    env.visual.update(|window, cx| {
        crate::dialogs::confirm(
            window,
            cx,
            "Delete?".into(),
            "Sure?".into(),
            "Delete".into(),
            true,
            |_, _| {},
        )
    });
    env.visual.run_until_parked();
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(env.fake.count(&format!("hide {PANE}")), 1, "{:?}", env.fake.logged());
    env.visual.update(|window, cx| window.close_dialog(cx));
    env.visual.run_until_parked();
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        env.fake.count(&format!("reveal_host {PANE}")),
        1,
        "{:?}",
        env.fake.logged()
    );

    // A menu covers it too: the workspace says so.
    env.visual.update(|_, cx| shell::cover(cx));
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(env.fake.count(&format!("hide {PANE}")), 2);
    env.visual.update(|_, cx| shell::uncover(cx));
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(env.fake.count(&format!("reveal_host {PANE}")), 2);

    // Another item: the handler is unloaded.
    let unloads = env.fake.count(&format!("unload {PANE}"));
    env.select("notes.txt");
    assert_eq!(env.fake.count(&format!("unload {PANE}")), unloads + 1);
}

#[gpui_kit::test]
fn a_type_with_no_previewer_keeps_its_icon_and_the_hosts_reason(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("report.doc");
    let sender = env
        .fake
        .senders
        .borrow()
        .last()
        .cloned()
        .expect("a preview was asked for");
    sender
        .try_send(Event::Unavailable {
            reason: "Windows has no previewer for this type.".into(),
        })
        .expect("sent");
    env.until("the reason shows", |env| {
        env.has("Windows has no previewer for this type.")
    });
    // Ready is quiet.
    sender.try_send(Event::Ready).expect("sent");
    env.visual.run_until_parked();
    let host = env.body_as::<ShellHost>().expect("the host is the body");
    env.until("the host is ready", |env| {
        host.read_with(&env.visual, |h, _| *h.state() == State::Ready)
    });
}

#[gpui_kit::test]
fn a_window_without_a_handle_has_no_previewer_and_says_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.fake.no_window.set(true);
    env.select("report.doc");
    let host = env.body_as::<ShellHost>().expect("the host is the body");
    host.read_with(&env.visual, |h, _| {
        assert_eq!(*h.state(), State::Unavailable(String::new()))
    });
    assert!(!host.read_with(&env.visual, |h, _| h.is_shown()));
    assert!(!env.has_id("preview-shell-note"));
}

#[gpui_kit::test]
fn a_mobi_book_gets_the_book_view_not_windows_previewer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    // The MOBI family is read here (preview::mobi); Windows' previewer is not asked.
    env.select("book.mobi");
    assert_eq!(env.body(), Some(Body::Book));
    assert!(!env
        .fake
        .logged()
        .contains(&format!("show {PANE} {}", env.path("book.mobi"))));
}

// ---- Pictures -----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_picture_is_drawn_here_and_says_its_size(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("photo.bmp");
    assert_eq!(env.body(), Some(Body::Picture));
    let picture = env.picture();
    env.until("the picture decodes", |env| {
        picture.read_with(&env.visual, |p, _| p.dimensions().is_some())
    });
    assert_eq!(picture.read_with(&env.visual, |p, _| p.dimensions()), Some((4, 2)));
    assert!(env.has("4 × 2 px"));
    assert!(env.has("Picture: photo.bmp"));
    // Windows' tools for a picture on disk: BMPs rotate and are wallpaper.
    assert!(env.has_id("preview-copy-text") && env.has_id("preview-rotate") && env.has_id("preview-wallpaper"));
}

#[gpui_kit::test]
fn zoom_backdrop_and_windows_tools_work_on_a_picture(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("photo.bmp");
    let picture = env.picture();
    env.until("the picture decodes", |env| {
        picture.read_with(&env.visual, |p, _| p.dimensions().is_some())
    });
    env.visual.update(|window, cx| window.render_frame(cx));
    // The wheel zooms by steps of 1.2; up zooms in.
    env.visual.update(|window, cx| {
        window.scroll(
            "preview-picture",
            gpui_kit::ScrollDelta::Pixels(point(px(0.), px(40.))),
            cx,
        )
    });
    assert_eq!(picture.read_with(&env.visual, |p, _| p.zoom()), 1.2);
    assert!(env.has("Zoom: 1.2×"));
    // Clicking the zoom chip resets it; a double-click toggles 2×.
    env.click("preview-zoom");
    assert_eq!(picture.read_with(&env.visual, |p, _| p.zoom()), 1.);
    env.visual
        .update(|window, cx| window.double_click("preview-picture", cx));
    assert_eq!(picture.read_with(&env.visual, |p, _| p.zoom()), 2.);
    // The wheel down to 1 and no further.
    for _ in 0..8 {
        env.visual.update(|window, cx| {
            window.scroll(
                "preview-picture",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-40.))),
                cx,
            )
        });
    }
    assert_eq!(picture.read_with(&env.visual, |p, _| p.zoom()), 1.);
    // What shows through: checks, then dark, then light.
    assert!(env.has("Backdrop: Checks"));
    env.click("preview-backdrop");
    assert!(env.has("Backdrop: Dark"));
    // Rotate and Wallpaper ask Windows.
    env.click("preview-rotate");
    env.click("preview-wallpaper");
    assert_eq!(env.fake.count("rotate"), 1);
    assert_eq!(env.fake.count("wallpaper"), 1);
}

#[gpui_kit::test]
fn copy_text_puts_the_text_on_the_clipboard(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    *env.fake.lines.borrow_mut() = vec!["Invoice".into(), "2026-10-24".into()];
    env.select("photo.bmp");
    let picture = env.picture();
    env.until("the picture decodes", |env| {
        picture.read_with(&env.visual, |p, _| p.dimensions().is_some())
    });
    env.click("preview-copy-text");
    env.until("the text is copied", |env| {
        env.visual.read_from_clipboard().and_then(|item| item.text()).as_deref() == Some("Invoice\n2026-10-24")
    });
}

#[gpui_kit::test]
fn the_color_picker_reads_the_pixel_under_the_pointer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("photo.bmp");
    let picture = env.picture();
    env.until("the picture decodes", |env| {
        picture.read_with(&env.visual, |p, _| p.dimensions().is_some())
    });
    env.visual.update(|window, cx| window.render_frame(cx));
    env.click("preview-colors");
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.hover("preview-picture", cx);
    });
    env.visual.update(|window, cx| window.render_frame(cx));
    assert!(env.has("#336699"), "the BMP is one color");
    // Clicking the picture copies it.
    env.click("preview-picture");
    assert_eq!(
        env.visual.read_from_clipboard().and_then(|item| item.text()).as_deref(),
        Some("#336699")
    );
}

#[gpui_kit::test]
fn a_picture_gpui_cannot_decode_is_shown_from_windows_thumbnail(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    // A "picture" of a kind GPUI can't read (HEIC); Windows has a thumbnail of it.
    std::fs::write(env.dir.join("shot.heic"), b"\0\0\0\x18ftypheic").expect("written");
    *env.fake.thumbnail.borrow_mut() = Some(thumbnail_answer(6, 3, [1, 2, 3]));
    let entry = file(&env.folder, "shot.heic", 14, 20);
    let view = env
        .visual
        .update(|window, cx| cx.new(|cx| PictureView::new(entry, window, cx)));
    env.until("the thumbnail stands in", |env| {
        view.read_with(&env.visual, |p, _| p.dimensions().is_some())
    });
    assert_eq!(view.read_with(&env.visual, |p, _| p.dimensions()), Some((6, 3)));
    assert_eq!(env.fake.count("thumbnail"), 1);
}

#[gpui_kit::test]
fn a_picture_nobody_can_draw_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    std::fs::write(env.dir.join("broken.heic"), b"nothing a picture").expect("written");
    let entry = file(&env.folder, "broken.heic", 17, 21);
    let view = env
        .visual
        .update(|window, cx| cx.new(|cx| PictureView::new(entry, window, cx)));
    env.until("it fails", |env| {
        view.read_with(&env.visual, |p, _| p.failure().is_some())
    });
    assert_eq!(
        view.read_with(&env.visual, |p, _| p.failure().map(str::to_string))
            .as_deref(),
        Some("This image can't be shown here")
    );
}

#[gpui_kit::test]
fn a_picture_too_big_to_decode_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    std::fs::write(env.dir.join("huge.bmp"), bmp(2, 2, [0, 0, 0])).expect("written");
    let mut entry = file(&env.folder, "huge.bmp", 65 << 20, 22);
    entry.size = 65 << 20;
    let view = env
        .visual
        .update(|window, cx| cx.new(|cx| PictureView::new(entry, window, cx)));
    env.until("it is refused", |env| {
        view.read_with(&env.visual, |p, _| p.failure().is_some())
    });
    assert_eq!(
        view.read_with(&env.visual, |p, _| p.failure().map(str::to_string))
            .as_deref(),
        Some("Too big to preview here: open it to read it.")
    );
}

// ---- Quick Look ---------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn space_opens_quick_look_on_the_focused_item_and_space_or_escape_closes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let first = env.path("notes.txt");
    env.explorer
        .update(&mut env.visual, |e, cx| e.select_only(Some(&first), cx));
    env.visual.run_until_parked();
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    assert!(env.dialog_open());
    assert!(env.explorer.read_with(&env.visual, |e, _| e.quick_look));
    let ql = env.visual.update(|_, cx| quick_look::current(cx)).expect("it is open");
    assert_eq!(ql.read_with(&env.visual, |q, cx| q.entry(cx).name), "notes.txt");
    assert!(env.has("Quick Look: notes.txt"));
    // The same previewer, large, with a Close button and Maximize.
    assert!(env.has_id("quick-look-close") || env.has("Close"));
    assert!(env.has("Maximize preview"));

    // Space closes it.
    env.press("space");
    assert!(!env.dialog_open(), "Space closes it");
    assert!(!env.explorer.read_with(&env.visual, |e, _| e.quick_look));
    // Escape too.
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    assert!(env.dialog_open());
    env.press("escape");
    assert!(!env.dialog_open(), "Escape closes it");
    assert!(!env.explorer.read_with(&env.visual, |e, _| e.quick_look));
}

#[gpui_kit::test]
fn the_arrows_in_quick_look_move_through_the_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let first = env.path("notes.txt");
    env.explorer
        .update(&mut env.visual, |e, cx| e.select_only(Some(&first), cx));
    env.visual.run_until_parked();
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    let ql = env.visual.update(|_, cx| quick_look::current(cx)).expect("it is open");
    let rows: Vec<String> = env.explorer.read_with(&env.visual, |e, _| {
        e.active_tab()
            .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
            .unwrap_or_default()
    });
    let at = rows.iter().position(|n| n == "notes.txt").expect("listed");
    env.press("down");
    assert_eq!(ql.read_with(&env.visual, |q, cx| q.entry(cx).name), rows[at + 1]);
    // The list underneath moved with it.
    let selected = env.explorer.read_with(&env.visual, |e, _| {
        e.active_tab()
            .and_then(|t| t.selection.iter().next().map(str::to_string))
    });
    assert_eq!(selected, Some(env.path(&rows[at + 1])));
    env.press("right");
    assert_eq!(ql.read_with(&env.visual, |q, cx| q.entry(cx).name), rows[at + 2]);
    env.press("up");
    env.press("left");
    assert_eq!(ql.read_with(&env.visual, |q, cx| q.entry(cx).name), "notes.txt");
    // At the first row there is no previous: it stays.
    env.explorer.update(&mut env.visual, |e, cx| {
        e.select_only(Some(&format!("{}\\{}", env.folder, rows[0])), cx)
    });
    env.visual.run_until_parked();
    env.press("up");
    assert_eq!(ql.read_with(&env.visual, |q, cx| q.entry(cx).name), rows[0]);
    // The pane beside it settles on the same item.
    env.settle();
    assert!(matches!(env.shown(), Shown::One(ref entry) if entry.name == rows[0]));
}

#[gpui_kit::test]
fn quick_look_can_be_maximized_and_closed_with_its_buttons(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("notes.txt");
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    let ql = env.visual.update(|_, cx| quick_look::current(cx)).expect("open");
    assert!(!ql.read_with(&env.visual, |q, _| q.is_maximized()));
    env.click("quick-look-maximize");
    assert!(ql.read_with(&env.visual, |q, _| q.is_maximized()));
    assert!(env.has("Restore size"));
    env.click("quick-look-close");
    assert!(!env.dialog_open());
}

#[gpui_kit::test]
fn quick_look_takes_windows_previewer_from_the_pane_and_gives_it_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("report.doc");
    let path = env.path("report.doc");
    assert_eq!(env.fake.count(&format!("show {PANE} {path}")), 1);
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    env.visual.update(|window, cx| window.render_frame(cx));
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        env.fake.count(&format!("show {QUICK_LOOK} {path}")),
        1,
        "{:?}",
        env.fake.logged()
    );
    // The pane's own is covered by the dialog and doesn't fight for the host.
    assert_eq!(env.fake.count(&format!("show {PANE} {path}")), 1);
    env.press("space");
    env.visual.update(|window, cx| window.render_frame(cx));
    env.visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(env.fake.count(&format!("unload {QUICK_LOOK}")), 1);
    assert_eq!(
        env.fake.count(&format!("show {PANE} {path}")),
        2,
        "the pane's previewer is shown again: {:?}",
        env.fake.logged()
    );
}

#[gpui_kit::test]
fn the_models_flag_opens_and_closes_quick_look_through_sync(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("notes.txt");
    env.explorer.update(&mut env.visual, |e, _| e.quick_look = true);
    env.visual.update(quick_look::sync);
    env.visual.run_until_parked();
    assert!(env.dialog_open());
    // Another tab coming forward clears the flag: Quick Look goes.
    env.explorer.update(&mut env.visual, |e, cx| {
        e.quick_look = false;
        cx.notify();
    });
    env.visual.run_until_parked();
    assert!(!env.dialog_open());
}

#[gpui_kit::test]
fn quick_look_does_nothing_without_an_item(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    assert!(!env.dialog_open());
    assert!(!env.explorer.read_with(&env.visual, |e, _| e.quick_look));
}

// ---- Arabic -------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_arabic_interface_previews_arabic_names(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    testing::set_rtl(true);
    env.select("تقرير الربع.txt");
    assert_eq!(env.body(), Some(Body::Text));
    assert!(env.has("تقرير الربع.txt"), "the header names the file");
    assert!(env.has(&format!("Location: {}", env.folder)));
    // An Arabic picture is decoded and drawn.
    env.select("صورة.bmp");
    let picture = env.picture();
    env.until("the picture decodes", |env| {
        picture.read_with(&env.visual, |p, _| p.dimensions().is_some())
    });
    assert_eq!(picture.read_with(&env.visual, |p, _| p.dimensions()), Some((8, 4)));
    assert!(env.has("Picture: صورة.bmp"));
    // Quick Look names it too.
    env.visual.update(quick_look::toggle);
    env.visual.run_until_parked();
    assert!(env.has("Quick Look: صورة.bmp"));
    env.press("escape");
    // Nothing selected: the folder's card in Arabic mirrors.
    env.explorer.update(&mut env.visual, |e, cx| e.select_only(None, cx));
    env.visual.run_until_parked();
    assert!(env.has("Select a file to preview it"));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_photos_details_and_place_show_under_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    *env.fake.media.borrow_mut() = Some(Media {
        rows: vec![("Camera".into(), "Canon EOS".into()), ("Place".into(), "London".into())],
        place: Some([51.5, -0.12]),
    });
    env.select("photo.bmp");
    env.until("the details arrive", |env| env.has("Camera: Canon EOS"));
    assert!(env.has("Place: London"));
}

#[gpui_kit::test]
fn the_actions_open_reveal_and_restore(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("notes.txt");
    env.click("preview-reveal");
    assert!(
        env.fake.logged().contains(&format!("reveal {}", env.path("notes.txt"))),
        "{:?}",
        env.fake.logged()
    );
}

// ---- The pane's rules ---------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_pane_shows_for_folders_and_searches_but_not_a_bare_this_pc(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    assert!(env.visual.update(|_, cx| super::should_show(cx)));
    // The setting off.
    env.visual.update(|_, cx| super::toggle_pane(cx));
    assert!(!env.visual.update(|_, cx| super::should_show(cx)));
    env.visual.update(|_, cx| super::toggle_pane(cx));
    assert!(env.visual.update(|_, cx| super::should_show(cx)));
    // This PC has nothing to preview.
    env.explorer.update(&mut env.visual, |e, cx| {
        e.navigate(String::new(), NavOptions::default(), cx);
    });
    env.visual.run_until_parked();
    assert!(!env.visual.update(|_, cx| super::should_show(cx)));
}

#[test]
fn the_panes_width_follows_the_saved_width_inside_its_limits() {
    let rem = 16.;
    let near = |got: f32, want: f32| assert!((got - want).abs() < 0.01, "{got} is not {want}");
    // Unsaved: 28% of the container, at most 28 rem.
    near(super::pane_width(None, 1000., rem, false), 280.);
    near(super::pane_width(None, 3000., rem, false), 448.);
    // Saved: kept, unless it leaves the list less than its share (19 rem here) or goes past 60%.
    near(super::pane_width(Some(400.), 1200., rem, false), 400.);
    near(super::pane_width(Some(2000.), 1200., rem, false), 720.);
    near(super::pane_width(Some(2000.), 800., rem, false), 480.);
    near(super::pane_width(Some(2000.), 600., rem, false), 296.);
    // A narrow container: the minimum, 16 rem, or what is left, wins.
    near(super::pane_width(Some(100.), 1200., rem, false), 256.);
    near(super::pane_width(Some(100.), 400., rem, false), 96.);
    // From md the list keeps 30 rem.
    near(super::pane_width(Some(2000.), 1000., rem, true), 520.);
}

#[test]
fn resolve_tells_one_item_from_several_and_ignores_what_is_gone() {
    use crate::explorer::{Rows, Selection};
    use std::sync::Arc;
    let rows = Rows::identity(Arc::from(vec![
        folder(r"C:\x", "a"),
        file(r"C:\x", "b.txt", 100, 20),
        file(r"C:\x", "c.txt", 50, 10),
        file(r"C:\x", "d.txt", 1, 0),
    ]));
    assert_eq!(super::resolve(&rows, &Selection::default()), Shown::Nothing);
    let one = super::resolve(&rows, &Selection::of([r"C:\x\b.txt"]));
    assert!(matches!(one, Shown::One(ref e) if e.name == "b.txt"));
    // A path the rows don't have (a filter hides it) is nothing.
    assert_eq!(
        super::resolve(&rows, &Selection::of([r"C:\x\gone.txt"])),
        Shown::Nothing
    );
    let Shown::Many(many) = super::resolve(
        &rows,
        &Selection::of([r"C:\x\a", r"C:\x\b.txt", r"C:\x\c.txt", r"C:\x\d.txt"]),
    ) else {
        panic!("four items");
    };
    assert_eq!((many.count, many.folders, many.files, many.bytes), (4, 1, 3, 151));
    // Dates: those with one only.
    assert_eq!((many.oldest, many.newest), (10, 20));
    assert_eq!(many.deck.len(), 3);
    assert_eq!(many.paths.len(), 4);
    // Two selected but one of them hidden: the one left.
    let left = super::resolve(&rows, &Selection::of([r"C:\x\b.txt", r"C:\x\gone.txt"]));
    assert!(matches!(left, Shown::One(ref e) if e.name == "b.txt"));
}

#[gpui_kit::test]
fn an_item_in_the_recycle_bin_is_restored_not_opened(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    env.select("gone.txt");
    assert!(env.has("Restore"));
    assert!(!env.has_id("preview-open") && !env.has_id("preview-reveal"));
    // It was deleted from somewhere: said under it.
    assert!(
        env.has(r"Deleted from: C:\Users\a\Documents"),
        "the place it goes back to"
    );
    // A folder there comes with its size, as Windows noted it.
    env.select("old");
    assert!(env.has("121 KB in total"));
}

#[gpui_kit::test]
fn a_file_in_an_archive_that_asks_for_a_password_shows_the_password_box(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx);
    let path = env.path("inside.txt");
    env.fake
        .probes
        .borrow_mut()
        .insert(path, Err("This file is password-protected.".into()));
    env.select("inside.txt");
    assert!(env.has("This file is password-protected."));
    assert!(env.has("Password"), "the box that asks for it");
    // Inside an archive Windows' previewer is never asked for, and it is shown from the archive: there is no
    // "Show in File Explorer" for the item itself (it shows the archive).
    assert_eq!(env.fake.count("show"), 0);
}

#[test]
fn every_icon_the_preview_draws_is_embedded() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    // A missing icon draws nothing and says nothing, so each one this module uses is checked here.
    for name in [
        IconName::Cloud,
        IconName::Clock,
        IconName::ExternalLink,
        IconName::FileArchive,
        IconName::FileLock,
        IconName::FolderLock,
        IconName::FolderOpen,
        IconName::FileArchive,
        IconName::FingerprintPattern,
        IconName::FolderSearch,
        IconName::GitCompareArrows,
        IconName::HardDrive,
        IconName::Images,
        IconName::LoaderCircle,
        IconName::Maximize2,
        IconName::Minimize2,
        IconName::PenLine,
        IconName::Pipette,
        IconName::RotateCcw,
        IconName::RotateCwSquare,
        IconName::ScanText,
        IconName::Trash,
        IconName::Wallpaper,
        IconName::X,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}
