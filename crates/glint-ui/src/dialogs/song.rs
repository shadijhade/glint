//! A song's tags (SongDialog.tsx).
//!
//! Ported from src/components/explorer/SongDialog.tsx. The run, its progress, result and buttons, and the
//! "Where" boxes are `dialogs::media::parts` (ActionParts.tsx), so this file needs the `dialog-media` feature too.
//!
//! | TypeScript                               | Here                                                       |
//! |------------------------------------------|------------------------------------------------------------|
//! | `openSongs(entries)`                     | [`open`] (the menus' `EditSongs { paths }`)                |
//! | `SongDialog` / `SongModal`               | [`SongDialog`]                                             |
//! | `isSong`, `SONGS`                        | [`is_song`], [`SONGS`]                                     |
//! | `fromName`                               | [`from_name`]                                              |
//! | `invoke("song_tags")`, `song_write`, `song_cancel`, `pickFile` | [`Backend`] (a fake in the tests)    |
//!
//! **How the coordinator wires it.** `MenuEvent::EditSongs { paths }`: call `dialogs::song::open(window, cx,
//! paths)`. It keeps the songs among the paths and says so in a notification when none is left, as the
//! TypeScript's toast did. The menus have already checked that the paths are on disk.
//!
//! The dialog reads the songs' tags in the background; one song shows each of its details, several show what they
//! share, and what is typed applies to all (the title and track stay each song's own unless filled in from the
//! names). Each song is rewritten as a copy that takes its place, so Ctrl+Z puts them all back.

use super::media::parts;
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::actions::{Progress, Report};
use glint_core::songtags::{SongEdit, SongTags};
use glint_model::format::{format_count, plural};
use glint_model::paths::base_name;
use parts::{Fut, Run};
use std::sync::Arc;

/// What lofty reads and writes details in (songtags.rs).
pub const SONGS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "ape", "wv", "mpc", "spx",
];

/// Whether Glint edits the details of `path` (`isSong`, by its name; the menus have checked it is a file on disk).
pub fn is_song(path: &str) -> bool {
    match base_name(path).rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => SONGS.contains(&ext.to_lowercase().as_str()),
        _ => false,
    }
}

// ---- Titles and tracks from file names (fromName) ------------------------------------------------------------

/// A track number and a title read from a file's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameParts {
    /// The track number without leading zeros, "" when the name has none.
    pub track: String,
    /// The title: after the last " - ", or the whole name.
    pub title: String,
}

/// "03 - Artist - Title.mp3" gives track "3" and title "Title" (`fromName`).
pub fn from_name(name: &str) -> NameParts {
    // name.replace(/\.[^.]+$/, ""): the last dot and what follows it, when something does.
    let stem = match name.rfind('.') {
        Some(dot) if dot + 1 < name.len() => &name[..dot],
        _ => name,
    };
    // /^\s*(\d{1,3})\s*[-._)\s]\s*(.*)$/
    let body = stem.trim_start();
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let mut track = String::new();
    let mut rest = stem;
    if (1..=3).contains(&digits) {
        let after = &body[digits..];
        let spaces = after.len() - after.trim_start().len();
        let next = after.trim_start();
        // All the spaces then one of -._) ; else the last space is the separator.
        let tail = match next.chars().next() {
            Some(c) if matches!(c, '-' | '.' | '_' | ')') => Some(&next[c.len_utf8()..]),
            _ if spaces > 0 => Some(next),
            _ => None,
        };
        if let Some(tail) = tail {
            track = body[..digits]
                .parse::<u32>()
                .map_or_else(|_| body[..digits].to_string(), |n| n.to_string());
            rest = tail.trim_start();
        }
    }
    let rest = rest.trim();
    // rest.split(/\s+-\s+/)
    let mut last = rest;
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].1.is_whitespace() {
            let mut j = i;
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j].1 == '-' && j + 1 < chars.len() && chars[j + 1].1.is_whitespace() {
                let mut k = j + 1;
                while k < chars.len() && chars[k].1.is_whitespace() {
                    k += 1;
                }
                last = &rest[chars.get(k).map_or(rest.len(), |(at, _)| *at)..];
                i = k;
                continue;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    NameParts {
        track,
        title: if last.is_empty() {
            rest.to_string()
        } else {
            last.to_string()
        },
    }
}

// ---- The details ----------------------------------------------------------------------------------------------

/// One detail of a song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The title.
    Title,
    /// The artist.
    Artist,
    /// The album.
    Album,
    /// The album's artist.
    AlbumArtist,
    /// The track number.
    Track,
    /// How many tracks the album has.
    TrackTotal,
    /// The year.
    Year,
    /// The genre.
    Genre,
}

const FIELDS: [Field; 8] = [
    Field::Title,
    Field::Artist,
    Field::Album,
    Field::AlbumArtist,
    Field::Track,
    Field::TrackTotal,
    Field::Year,
    Field::Genre,
];

impl Field {
    fn index(self) -> usize {
        FIELDS.iter().position(|f| *f == self).unwrap_or(0)
    }

    fn label(self) -> SharedString {
        match self {
            Field::Title => t!("Title"),
            Field::Artist => t!("Artist"),
            Field::Album => t!("Album"),
            Field::AlbumArtist => t!("Album artist"),
            Field::Track => t!("Track"),
            Field::TrackTotal => t!("Tracks in all"),
            Field::Year => t!("Year"),
            Field::Genre => t!("Genre"),
        }
    }

    /// A detail several songs have of their own (`each`): typed values don't apply to all of them.
    fn each(self) -> bool {
        matches!(self, Field::Title | Field::Track)
    }

    fn numeric(self) -> bool {
        matches!(self, Field::Track | Field::TrackTotal | Field::Year)
    }

    /// What a song says for it, as text (`show`).
    fn of(self, tags: &SongTags) -> String {
        match self {
            Field::Title => tags.title.clone(),
            Field::Artist => tags.artist.clone(),
            Field::Album => tags.album.clone(),
            Field::AlbumArtist => tags.album_artist.clone(),
            Field::Track => tags.track.map(|n| n.to_string()).unwrap_or_default(),
            Field::TrackTotal => tags.track_total.map(|n| n.to_string()).unwrap_or_default(),
            Field::Year => tags.year.map(|n| n.to_string()).unwrap_or_default(),
            Field::Genre => tags.genre.clone(),
        }
    }

    /// Puts `value` into the edit.
    fn put(self, edit: &mut SongEdit, value: String) {
        let slot = match self {
            Field::Title => &mut edit.title,
            Field::Artist => &mut edit.artist,
            Field::Album => &mut edit.album,
            Field::AlbumArtist => &mut edit.album_artist,
            Field::Track => &mut edit.track,
            Field::TrackTotal => &mut edit.track_total,
            Field::Year => &mut edit.year,
            Field::Genre => &mut edit.genre,
        };
        *slot = Some(value);
    }
}

/// The cover: as it is, taken out, or a picture file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cover {
    /// Each song keeps its own (`undefined`).
    Keep,
    /// Taken out (`null`).
    TakeOut,
    /// This picture file.
    File(String),
}

// ---- What the dialog asks of the PC ---------------------------------------------------------------------------

/// The calls into glint-core's song tags and the file picker. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// The songs' details as they are (`song_tags`).
    fn tags(&self, paths: Vec<String>) -> Fut<Vec<SongTags>>;
    /// Writes each song's new details, reporting progress on `progress` (`song_write`).
    fn write(&self, edits: Vec<SongEdit>, progress: glint_core::host::ipc::Channel<Progress>) -> Fut<Report>;
    /// Stops the run under way (`song_cancel`).
    fn cancel(&self);
    /// A picture file the person picks (`None`: cancelled). `owner` is the window's handle.
    fn pick_cover(&self, owner: isize) -> Fut<Option<String>>;
}

/// glint-core's song tags, owned by the main window as the shell dialogs it opens are.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Backend for CoreBackend {
    fn tags(&self, paths: Vec<String>) -> Fut<Vec<SongTags>> {
        parts::on_core(glint_core::songtags::song_tags(paths))
    }

    fn write(&self, edits: Vec<SongEdit>, progress: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        let app = self.app.clone();
        parts::on_core(async move {
            let window = parts::main_window(&app)?;
            glint_core::songtags::song_write(window, edits, progress).await
        })
    }

    fn cancel(&self) {
        glint_core::songtags::song_cancel();
    }

    fn pick_cover(&self, owner: isize) -> Fut<Option<String>> {
        Box::pin(pick_picture(t!("Choose a cover").to_string(), owner))
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn tags(&self, _: Vec<String>) -> Fut<Vec<SongTags>> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn write(&self, _: Vec<SongEdit>, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self) {}
    fn pick_cover(&self, _: isize) -> Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }
}

/// Windows' file picker, asked for a picture (jpg, jpeg, png, gif, bmp), owned by `owner` (a window handle, 0
/// for none). It runs a modal loop, so it is posted to the UI thread's message pump and never runs inside a GPUI
/// callback; the answer comes back through a channel.
///
/// TODO(coordinator): from a shared `platform::pickers` once it exists (Settings keeps its own, which takes
/// one extension only).
async fn pick_picture(title: String, owner: isize) -> Result<Option<String>, String> {
    let (answer, heard) = async_channel::bounded(1);
    core_bridge::post_to_ui(move || {
        let _ = answer.try_send(show_picker(&title, owner));
    });
    heard
        .recv()
        .await
        .map_err(|_| "the picker was closed without an answer".to_string())?
}

fn show_picker(title: &str, owner: isize) -> Result<Option<String>, String> {
    // SAFETY: COM calls on the UI thread, which GPUI has already put in a single-threaded apartment (a second
    // CoInitializeEx with the same mode is harmless; a different mode is reported and ignored). The owner is a
    // window handle that may be stale, which the dialog tolerates by showing unowned. Every string handed to
    // COM outlives the call, and the one COM gives back is freed once.
    unsafe {
        let _ =
            windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        picker(title, windows::Win32::Foundation::HWND(owner as *mut _)).map_err(|error| error.message())
    }
}

/// # Safety
/// UI thread; see [`show_picker`].
unsafe fn picker(title: &str, owner: windows::Win32::Foundation::HWND) -> windows::core::Result<Option<String>> {
    use windows::core::{Interface as _, HSTRING, PCWSTR};
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, SIGDN_FILESYSPATH,
    };

    let dialog: IFileDialog =
        CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?.cast()?;
    dialog.SetTitle(&HSTRING::from(title))?;
    dialog.SetOptions(dialog.GetOptions()? | FOS_FORCEFILESYSTEM)?;
    let name = HSTRING::from(t!("Pictures").as_ref());
    let spec = HSTRING::from("*.jpg;*.jpeg;*.png;*.gif;*.bmp");
    let types = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(name.as_ptr()),
        pszSpec: PCWSTR(spec.as_ptr()),
    }];
    dialog.SetFileTypes(&types)?;
    match dialog.Show(Some(owner)) {
        Ok(()) => {}
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) => return Ok(None),
        Err(error) => return Err(error),
    }
    let item = dialog.GetResult()?;
    let path = item.GetDisplayName(SIGDN_FILESYSPATH)?;
    let text = path.to_string().ok();
    CoTaskMemFree(Some(path.0 as *const _));
    Ok(text)
}

// ---- The dialog -----------------------------------------------------------------------------------------------

/// "Edit song details" for some songs: their details, what to change, the run and its result.
pub struct SongDialog {
    /// The songs, in the order they were chosen.
    files: Vec<String>,
    /// Their details once read.
    tags: Option<Vec<SongTags>>,
    /// Why they couldn't be read, when they couldn't.
    read_error: Option<SharedString>,
    inputs: Vec<Entity<InputState>>,
    /// Which details were typed in (`touched`).
    touched: [bool; 8],
    from_names: bool,
    numbered: bool,
    cover: Cover,
    run: Run,
    backend: Arc<dyn Backend>,
    _subscriptions: Vec<Subscription>,
}

impl SongDialog {
    /// A dialog for `files`, calling `backend`; the details are read at once.
    pub fn new(files: Vec<String>, backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs: Vec<Entity<InputState>> = FIELDS
            .iter()
            .map(|_| cx.new(|cx| InputState::new(window, cx)))
            .collect();
        let mut subscriptions = Vec::new();
        for (ix, input) in inputs.iter().enumerate() {
            subscriptions.push(
                cx.subscribe_in(
                    input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => {
                            this.touched[ix] = true;
                            input.update(cx, |input, cx| input.set_placeholder("", window, cx));
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        _ => {}
                    },
                ),
            );
        }
        let read = backend.tags(files.clone());
        cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            let _ = this.update_in(cx, |this, window, cx| match read {
                Ok(tags) => this.fill(tags, window, cx),
                Err(reason) => {
                    window.push_notification(
                        Notification::error(SharedString::from(reason.clone()))
                            .title(t!("Couldn't read the songs' details")),
                        cx,
                    );
                    this.read_error = Some(reason.into());
                    cx.notify();
                }
            });
        })
        .detach();
        Self {
            files,
            tags: None,
            read_error: None,
            inputs,
            touched: [false; 8],
            from_names: false,
            numbered: false,
            cover: Cover::Keep,
            run: Run::Setup,
            backend,
            _subscriptions: subscriptions,
        }
    }

    /// Shows what was read: one song's own details, or what all the songs share.
    fn fill(&mut self, tags: Vec<SongTags>, window: &mut Window, cx: &mut Context<Self>) {
        let one = self.files.len() == 1;
        for field in FIELDS {
            let shared = {
                let mut values: Vec<String> = tags.iter().map(|t| field.of(t)).collect();
                values.dedup();
                values.sort();
                values.dedup();
                if values.len() == 1 {
                    values.remove(0)
                } else {
                    String::new()
                }
            };
            let text = if one || !field.each() { shared } else { String::new() };
            let empty = text.is_empty();
            self.inputs[field.index()].update(cx, |input, cx| {
                input.set_value(text, window, cx);
                input.set_placeholder(
                    if !one && empty {
                        t!("(as each is)")
                    } else {
                        SharedString::default()
                    },
                    window,
                    cx,
                );
            });
        }
        self.tags = Some(tags);
        cx.notify();
    }

    /// The songs being edited.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Where the run stands.
    pub fn run_state(&self) -> &Run {
        &self.run
    }

    /// The box of a detail.
    pub fn input(&self, field: Field) -> &Entity<InputState> {
        &self.inputs[field.index()]
    }

    /// Whether a detail was typed in.
    pub fn touched(&self, field: Field) -> bool {
        self.touched[field.index()]
    }

    /// Whether a number box holds something that isn't one (`badNumber`).
    pub fn bad_number(&self, cx: &App) -> bool {
        FIELDS.iter().filter(|f| f.numeric()).any(|f| {
            let text = self.inputs[f.index()].read(cx).value();
            let text = text.trim();
            self.touched[f.index()] && !text.is_empty() && !text.bytes().all(|b| b.is_ascii_digit())
        })
    }

    /// Whether nothing would change (`nothing`).
    pub fn nothing(&self) -> bool {
        !self.touched.iter().any(|t| *t) && !self.from_names && !self.numbered && self.cover == Cover::Keep
    }

    /// Whether Save can be pressed: the details are read, something changes, and the numbers are numbers.
    pub fn can_start(&self, cx: &App) -> bool {
        matches!(self.run, Run::Setup) && self.tags.is_some() && !self.nothing() && !self.bad_number(cx)
    }

    /// Each song's edit: what was typed, then the names and numbering when asked (`start`).
    pub fn edits(&self, cx: &App) -> Vec<SongEdit> {
        let mut ordered: Vec<&String> = self.files.iter().collect();
        ordered.sort_by(|a, b| glint_model::entries::natural_compare(base_name(a), base_name(b)));
        let total = ordered.len();
        ordered
            .into_iter()
            .enumerate()
            .map(|(i, file)| {
                let mut edit = SongEdit {
                    path: file.clone(),
                    ..SongEdit::default()
                };
                for field in FIELDS {
                    if self.touched[field.index()] {
                        field.put(&mut edit, self.inputs[field.index()].read(cx).value().to_string());
                    }
                }
                if self.from_names {
                    let parsed = from_name(base_name(file));
                    edit.title = Some(parsed.title);
                    if !parsed.track.is_empty() && !self.numbered {
                        edit.track = Some(parsed.track);
                    }
                }
                if self.numbered {
                    edit.track = Some((i + 1).to_string());
                    edit.track_total = Some(total.to_string());
                }
                match &self.cover {
                    Cover::Keep => {}
                    Cover::TakeOut => edit.cover = Some(String::new()),
                    Cover::File(path) => edit.cover = Some(path.clone()),
                }
                edit
            })
            .collect()
    }

    fn run_mut(&mut self) -> &mut Run {
        &mut self.run
    }

    /// Enter in a box: Save, when it can be pressed (the Modal's `onSubmit`).
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start(window, cx);
    }

    /// Starts the run on core's runtime; progress and the result come back here.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let edits = self.edits(cx);
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.write(edits, channel);
        parts::drive(
            self,
            window,
            cx,
            parts::Launch {
                total: self.files.len(),
                call,
                progress: receiver,
            },
            Self::run_mut,
            |report| t!("details of {what}", what = plural(report.made.len(), "song")).to_string(),
        );
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// Asks for a picture to be the cover (`pickFile`).
    fn choose_cover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |hwnd| hwnd.0 as isize);
        let pick = self.backend.pick_cover(owner);
        cx.spawn_in(window, async move |this, cx| {
            let picked = pick.await;
            let _ = this.update_in(cx, |this, window, cx| match picked {
                Ok(Some(path)) => {
                    this.cover = Cover::File(path);
                    cx.notify();
                }
                Ok(None) => {}
                Err(reason) => window.push_notification(
                    Notification::error(SharedString::from(reason)).title(t!("Couldn't choose a cover")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// What the cover row says.
    fn cover_text(&self) -> SharedString {
        match (&self.cover, &self.tags) {
            (Cover::Keep, Some(tags)) if tags.iter().all(|t| t.cover) => t!("Each has one"),
            (Cover::Keep, Some(tags)) if tags.iter().any(|t| t.cover) => t!("Some have one"),
            (Cover::Keep, _) => t!("None"),
            (Cover::TakeOut, _) => t!("Taken out"),
            (Cover::File(path), _) => base_name(path).to_string().into(),
        }
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------------

impl SongDialog {
    /// The fields shown: all of them for one song; for several, not the ones each song keeps while the names or
    /// numbering fill them in.
    fn visible(&self) -> Vec<Field> {
        let one = self.files.len() == 1;
        FIELDS
            .into_iter()
            .filter(|f| {
                one || !f.each()
                    || match f {
                        Field::Track => !self.numbered && !self.from_names,
                        Field::Title => !self.from_names,
                        _ => true,
                    }
            })
            .collect()
    }

    /// "“03 - A - B.mp3” becomes “B”, track 3."
    fn example_from_names(&self) -> Option<SharedString> {
        let first = base_name(self.files.first()?);
        let parsed = from_name(first);
        let mut text = t!("“{name}” becomes “{title}”", name = first, title = parsed.title.clone()).to_string();
        if !parsed.track.is_empty() {
            text.push_str(&t!(", track {n}", n = parsed.track.clone()));
        }
        text.push('.');
        Some(text.into())
    }

    fn fields(&self, t: &theme::Tokens) -> impl IntoElement {
        col().gap(space(1.5)).children(self.visible().into_iter().map(|field| {
            parts::field(
                field.label(),
                Input::new(&self.inputs[field.index()])
                    .small()
                    .aria_label(field.label()),
                t,
            )
        }))
    }

    fn cover_row(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.cover_text();
        align_row(2.)
            .child(
                div()
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child(t!("Cover")),
            )
            .child(
                div()
                    .id("song-cover")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(text)),
            )
            .child(
                Button::new("song-cover-choose")
                    .outline()
                    .compact()
                    .label(t!("Choose…"))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_cover(window, cx))),
            )
            .child(
                Button::new("song-cover-remove")
                    .ghost()
                    .compact()
                    .label(t!("Take out"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cover = Cover::TakeOut;
                        cx.notify();
                    })),
            )
    }

    fn setup(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let Some(tags) = &self.tags else {
            let (text, role) = match &self.read_error {
                Some(reason) => (
                    t!("Couldn't read the songs' details: {why}", why = reason.clone()),
                    Role::Alert,
                ),
                None => (t!("Reading their details…"), Role::Status),
            };
            return div()
                .id("song-reading")
                .role(role)
                .aria_label(text.clone())
                .test_support()
                .text_color(t.muted_foreground)
                .child(text)
                .into_any_element();
        };
        let unreadable = tags.iter().filter(|t| t.error.is_some()).count();
        let many = self.files.len() > 1;
        col()
            .gap(space(3.))
            .when(unreadable > 0, |this| {
                let text = t!(
                    "{what} couldn't be read, and won't change.",
                    what = plural(unreadable, "song")
                );
                this.child(
                    div()
                        .id("song-unreadable")
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.warning)
                        .child(text),
                )
            })
            .child(self.fields(t))
            .when(many, |this| {
                this.child(
                    col()
                        .gap(space(1.))
                        .child(
                            align_row(2.)
                                .child(icon(IconName::WandSparkles).size_4().flex_none().text_color(t.muted_foreground))
                                .child(
                                    Checkbox::new("song-from-names")
                                        .label(t!(
                                            "Titles (and track numbers) from the file names, as in “03 - Artist - Title”"
                                        ))
                                        .checked(self.from_names)
                                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                            this.from_names = *checked;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            Checkbox::new("song-numbered")
                                .label(t!(
                                    "Number the tracks 1 to {n}, in the files' name order",
                                    n = self.files.len()
                                ))
                                .checked(self.numbered)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.numbered = *checked;
                                    cx.notify();
                                })),
                        )
                        .when(self.from_names, |this| {
                            this.children(self.example_from_names().map(|example| {
                                div()
                                    .id("song-example")
                                    .role(Role::Status)
                                    .aria_label(example.clone())
                                    .test_support()
                                    .ps(6.)
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(isolate(example))
                            }))
                        }),
                )
            })
            .child(self.cover_row(t, cx))
            .child(parts::note(
                t!("An empty field takes that detail out. Each song is saved as a copy with its new details that takes its place, so Ctrl+Z puts them all back."),
                t,
            ))
            .into_any_element()
    }
}

/// The dialog's title: the song's file name, or how many songs.
pub fn title(files: &[String]) -> SharedString {
    match files {
        [one] => base_name(one).to_string().into(),
        _ => t!("Details of {n} songs", n = format_count(files.len())),
    }
}

impl Render for SongDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let body = match self.run.clone() {
            Run::Setup => self.setup(&t, cx),
            Run::Running(progress) => parts::progress_view("song", &t!("Saving"), &progress, &t).into_any_element(),
            Run::Done(report) => parts::result_view("song", "song", false, &report, &t).into_any_element(),
            Run::Failed(reason) => {
                parts::error_view("song", t!("Couldn't save their details"), &reason, &t).into_any_element()
            }
        };
        let footer = parts::Footer::<Self> {
            slot: "song",
            run: self.run.clone(),
            label: t!("Save"),
            icon: IconName::Music,
            can_start: self.can_start(cx),
            start: Self::start,
            stop: Self::stop,
        }
        .render(&t, cx);
        col()
            .id("song")
            .role(Role::Group)
            .aria_label(t!("Edit song details"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .child(parts::folder_line("song-folder", &self.files, &t))
            .child(
                div()
                    .id("song-body")
                    .max_h(parts::body_height(window))
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .child(footer)
    }
}

// ---- Opening it -----------------------------------------------------------------------------------------------

/// Opens Edit song details on the songs among `paths` (`openSongs`). When none is left it says so in a
/// notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_song(p)).collect();
    if files.is_empty() {
        window.push_notification(
            Notification::info(t!(
                "Edit song details works on MP3, FLAC, M4A, Ogg, Opus, WAV and AIFF files"
            )),
            cx,
        );
        return;
    }
    let backend: Arc<dyn Backend> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    };
    open_with(window, cx, backend, files);
}

/// [`open`] for a given backend, without filtering `files` (tests pass a fake). Returns the dialog's view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    files: Vec<String>,
) -> Entity<SongDialog> {
    let shown = isolate(title(&files));
    let view = cx.new(|cx| SongDialog::new(files, backend, window, cx));
    super::show(window, cx, shown, px(512.), view.clone());
    view
}

// ---- Tests ----------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend with canned answers.
    struct Fake {
        tags: Result<Vec<SongTags>, String>,
        report: Report,
        cover: Option<String>,
        writes: Mutex<Vec<Vec<SongEdit>>>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(tags: Vec<SongTags>, made: &[&str]) -> Self {
            Self {
                tags: Ok(tags),
                report: Report {
                    made: made.iter().map(|s| (*s).to_string()).collect(),
                    elapsed_ms: 800.,
                    ..Default::default()
                },
                cover: Some("C:\\pics\\front.jpg".into()),
                writes: Mutex::new(Vec::new()),
                cancelled: Mutex::new(0),
            }
        }

        fn written(&self) -> Vec<Vec<SongEdit>> {
            self.writes.lock().map(|w| w.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn tags(&self, _: Vec<String>) -> Fut<Vec<SongTags>> {
            let answer = self.tags.clone();
            Box::pin(async move { answer })
        }

        fn write(&self, edits: Vec<SongEdit>, progress: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
            let _ = progress.send(Progress {
                done: 0,
                total: edits.len(),
                current: "a.mp3".into(),
                part: 0.,
            });
            if let Ok(mut writes) = self.writes.lock() {
                writes.push(edits);
            }
            let report = self.report.clone();
            Box::pin(async move { Ok(report) })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }

        fn pick_cover(&self, _: isize) -> Fut<Option<String>> {
            let cover = self.cover.clone();
            Box::pin(async move { Ok(cover) })
        }
    }

    fn song(path: &str, title: &str, artist: &str) -> SongTags {
        SongTags {
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            album: "Abbey Road".into(),
            track: Some(3),
            year: Some(1969),
            cover: true,
            ..Default::default()
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the song dialog never reached the state the test waits for");
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn disabled(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
        })
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press("ctrl-a", cx);
            if text.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(&text, cx);
            }
        });
        cx.run_until_parked();
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn open_view(cx: &mut VisualTestContext, files: &[&str], fake: &Arc<Fake>) -> Entity<SongDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let view = cx.update(|window, cx| open_with(window, cx, backend, files));
        settle(cx, |cx| {
            view.read_with(cx, |v, _| v.tags.is_some() || v.read_error.is_some())
        });
        view
    }

    fn value(cx: &VisualTestContext, view: &Entity<SongDialog>, field: Field) -> String {
        view.read_with(cx, |v, cx| v.input(field).read(cx).value().to_string())
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Music,
            IconName::WandSparkles,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn songs_are_told_by_their_names() {
        for yes in [
            "C:\\m\\a.mp3",
            "C:\\m\\a.FLAC",
            "C:\\m\\x.y.opus",
            "C:\\m\\a.m4b",
            "C:\\m\\a.spx",
        ] {
            assert!(is_song(yes), "{yes}");
        }
        for no in ["C:\\m\\a.txt", "C:\\m\\mp3", "C:\\m\\.mp3", "C:\\m\\a.mp3x", "C:\\m\\a"] {
            assert!(!is_song(no), "{no}");
        }
    }

    #[test]
    fn titles_and_tracks_come_out_of_names_as_the_typescript_read_them() {
        let parts = |name: &str| {
            let p = from_name(name);
            (p.track, p.title)
        };
        let is = |track: &str, title: &str| (track.to_string(), title.to_string());
        // "03 - Artist - Title": the number, then the last part.
        assert_eq!(parts("03 - Artist - Title.mp3"), is("3", "Title"));
        assert_eq!(parts("03. Title.mp3"), is("3", "Title"));
        assert_eq!(parts("7)Seven.mp3"), is("7", "Seven"));
        assert_eq!(parts("007_Bond.flac"), is("7", "Bond"));
        assert_eq!(parts("01 Song Name.flac"), is("1", "Song Name"));
        assert_eq!(parts("  12 -  Spaced  .mp3"), is("12", "Spaced"));
        // No number: the title alone, after the last " - ".
        assert_eq!(parts("Title.mp3"), is("", "Title"));
        assert_eq!(parts("Artist - Title.mp3"), is("", "Title"));
        assert_eq!(parts("A - B - C.mp3"), is("", "C"));
        assert_eq!(parts("A-B.mp3"), is("", "A-B"));
        // Four digits are a year, not a track: no number is taken, the dash still splits.
        assert_eq!(parts("1999 - Song.mp3"), is("", "Song"));
        // A number then a space then more digits: the space is the separator.
        assert_eq!(parts("12 3.mp3"), is("12", "3"));
        // Digits stuck to the title are part of it.
        assert_eq!(parts("03Song.mp3"), is("", "03Song"));
        // Only the last extension goes; a name without one stays whole.
        assert_eq!(parts("02 - A.B.mp3"), is("2", "A.B"));
        assert_eq!(parts("Song"), is("", "Song"));
        assert_eq!(parts("a - .mp3"), is("", "a -"));
        // Arabic and Hebrew names work the same.
        assert_eq!(parts("04 - أغنية - عنوان.mp3"), is("4", "عنوان"));
        assert_eq!(parts("קובץ.mp3"), is("", "קובץ"));
    }

    #[gpui_kit::test]
    fn one_song_shows_its_details_and_saves_only_what_was_typed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(
            vec![song("C:\\m\\a.mp3", "Come Together", "The Beatles")],
            &["C:\\m\\a.mp3"],
        ));
        let backend: Arc<dyn Backend> = fake.clone();
        let view = visual.update(|window, cx| open_with(window, cx, backend, strings(&["C:\\m\\a.mp3"])));
        // Reading first.
        assert_eq!(role_of(&mut visual, "Reading their details…"), Some(Role::Status));
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.tags.is_some()));
        assert_eq!(role_of(&mut visual, "Edit song details"), Some(Role::Group));
        assert_eq!(value(&visual, &view, Field::Title), "Come Together");
        assert_eq!(value(&visual, &view, Field::Artist), "The Beatles");
        assert_eq!(value(&visual, &view, Field::Track), "3");
        assert_eq!(value(&visual, &view, Field::Year), "1969");
        assert_eq!(value(&visual, &view, Field::AlbumArtist), "");
        assert!(has(&mut visual, "Each has one"), "its cover is there");
        // Nothing changed yet: Save waits. Filling the boxes didn't count as typing.
        assert!(disabled(&mut visual, "song-start"));
        assert!(!view.read_with(&visual, |v, _| v.touched(Field::Title)));
        let year = view.read_with(&visual, |v, _| v.input(Field::Year).clone());
        let title = view.read_with(&visual, |v, _| v.input(Field::Title).clone());
        type_into(&mut visual, &year, "19x9");
        assert!(disabled(&mut visual, "song-start"), "a year that isn't a number");
        type_into(&mut visual, &year, "1970");
        type_into(&mut visual, &title, "");
        assert!(!disabled(&mut visual, "song-start"));
        click(&mut visual, "song-start");
        settle(&mut visual, |cx| has(cx, "Made 1 song"));
        assert_eq!(role_of(&mut visual, "Made 1 song"), Some(Role::Status));
        let writes = fake.written();
        assert_eq!(writes.len(), 1);
        let edit = &writes[0][0];
        assert_eq!(edit.path, "C:\\m\\a.mp3");
        // Typed: an empty title (taken out) and the year. The rest stays as it is.
        assert_eq!(edit.title.as_deref(), Some(""));
        assert_eq!(edit.year.as_deref(), Some("1970"));
        assert!(edit.artist.is_none() && edit.track.is_none() && edit.cover.is_none());
        assert!(view.read_with(&visual, |v, _| matches!(v.run_state(), Run::Done(_))));
    }

    #[gpui_kit::test]
    fn several_songs_share_what_they_share_and_number_and_name_themselves(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let tags = vec![
            song("C:\\m\\10 - X - Ten.mp3", "a", "The Beatles"),
            song("C:\\m\\2 - X - Two.mp3", "b", "The Beatles"),
            song("C:\\m\\1 - X - One.mp3", "c", "Wings"),
        ];
        let fake = Arc::new(Fake::new(tags, &["a", "b", "c"]));
        let view = open_view(
            &mut visual,
            &[
                "C:\\m\\10 - X - Ten.mp3",
                "C:\\m\\2 - X - Two.mp3",
                "C:\\m\\1 - X - One.mp3",
            ],
            &fake,
        );
        assert!(has(&mut visual, "Edit song details"));
        // What they share is shown; what differs, and each song's own title and track, is empty.
        assert_eq!(value(&visual, &view, Field::Album), "Abbey Road");
        assert_eq!(value(&visual, &view, Field::Artist), "");
        assert_eq!(value(&visual, &view, Field::Title), "");
        assert_eq!(value(&visual, &view, Field::Track), "");
        assert_eq!(value(&visual, &view, Field::Year), "1969");
        assert!(has(&mut visual, "Title") && has(&mut visual, "Track"));
        // Names fill the titles and the numbering fills the tracks: their boxes go.
        click(&mut visual, "song-from-names");
        assert!(
            !has(&mut visual, "Title"),
            "the title box is gone while the names fill it in"
        );
        assert!(
            !has(&mut visual, "Track"),
            "and the track box with it: the names carry the numbers"
        );
        assert!(has(&mut visual, "“10 - X - Ten.mp3” becomes “Ten”, track 10."));
        click(&mut visual, "song-numbered");
        assert!(!has(&mut visual, "Track") && !has(&mut visual, "Title"));
        // Unticked, the boxes come back.
        click(&mut visual, "song-from-names");
        assert!(
            has(&mut visual, "Title") && !has(&mut visual, "Track"),
            "numbering still fills the tracks"
        );
        click(&mut visual, "song-from-names");
        click(&mut visual, "song-start");
        settle(&mut visual, |cx| has(cx, "Made 3 songs"));
        let edits = &fake.written()[0];
        // In the files' natural name order: 1, 2, 10.
        let paths: Vec<&str> = edits.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "C:\\m\\1 - X - One.mp3",
                "C:\\m\\2 - X - Two.mp3",
                "C:\\m\\10 - X - Ten.mp3"
            ]
        );
        let titles: Vec<_> = edits.iter().map(|e| e.title.clone().unwrap_or_default()).collect();
        assert_eq!(titles, ["One", "Two", "Ten"]);
        let tracks: Vec<_> = edits.iter().map(|e| e.track.clone().unwrap_or_default()).collect();
        assert_eq!(tracks, ["1", "2", "3"], "numbering wins over the numbers in the names");
        assert!(edits
            .iter()
            .all(|e| e.track_total.as_deref() == Some("3") && e.artist.is_none()));
    }

    #[gpui_kit::test]
    fn the_cover_is_chosen_or_taken_out_and_unreadable_songs_are_counted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut bad = song("C:\\m\\b.mp3", "", "");
        bad.error = Some("Not a song".into());
        bad.cover = false;
        let fake = Arc::new(Fake::new(vec![song("C:\\m\\a.mp3", "A", "X"), bad], &["C:\\m\\a.mp3"]));
        let _view = open_view(&mut visual, &["C:\\m\\a.mp3", "C:\\m\\b.mp3"], &fake);
        assert_eq!(
            role_of(&mut visual, "1 song couldn't be read, and won't change."),
            Some(Role::Status)
        );
        assert_eq!(role_of(&mut visual, "Some have one"), Some(Role::Status));
        click(&mut visual, "song-cover-choose");
        settle(&mut visual, |cx| has(cx, "front.jpg"));
        click(&mut visual, "song-cover-remove");
        assert!(has(&mut visual, "Taken out"));
        click(&mut visual, "song-cover-choose");
        settle(&mut visual, |cx| has(cx, "front.jpg"));
        assert!(!disabled(&mut visual, "song-start"), "a new cover is a change");
        click(&mut visual, "song-start");
        settle(&mut visual, |cx| has(cx, "Made 1 song"));
        let edits = &fake.written()[0];
        assert!(edits.iter().all(|e| e.cover.as_deref() == Some("C:\\pics\\front.jpg")));
    }

    #[gpui_kit::test]
    fn enter_in_a_box_saves_and_a_read_failure_is_shown(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(vec![song("C:\\m\\a.mp3", "A", "X")], &["C:\\m\\a.mp3"]));
        let view = open_view(&mut visual, &["C:\\m\\a.mp3"], &fake);
        let artist = view.read_with(&visual, |v, _| v.input(Field::Artist).clone());
        type_into(&mut visual, &artist, "Wings");
        visual.update(|window, cx| window.press("enter", cx));
        settle(&mut visual, |_| !fake.written().is_empty());
        assert_eq!(fake.written()[0][0].artist.as_deref(), Some("Wings"));
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();

        let mut broken = Fake::new(vec![], &[]);
        broken.tags = Err("boom".into());
        let broken = Arc::new(broken);
        let _view = open_view(&mut visual, &["C:\\m\\a.mp3"], &broken);
        assert_eq!(
            role_of(&mut visual, "Couldn't read the songs' details: boom"),
            Some(Role::Alert)
        );
        assert!(disabled(&mut visual, "song-start"));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_work_through_a_save(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Arc::new(Fake::new(
            vec![
                song("C:\\م\\04 - فيروز - سألتك حبيبي.mp3", "سألتك حبيبي", "فيروز"),
                song("C:\\م\\05 - فيروز - ليلية.mp3", "ليلية", "فيروز"),
            ],
            &["a", "b"],
        ));
        let view = open_view(
            &mut visual,
            &["C:\\م\\04 - فيروز - سألتك حبيبي.mp3", "C:\\م\\05 - فيروز - ليلية.mp3"],
            &fake,
        );
        assert_eq!(value(&visual, &view, Field::Artist), "فيروز");
        assert_eq!(role_of(&mut visual, "C:\\م"), Some(Role::Group));
        click(&mut visual, "song-from-names");
        assert!(has(
            &mut visual,
            "“04 - فيروز - سألتك حبيبي.mp3” becomes “سألتك حبيبي”, track 4."
        ));
        click(&mut visual, "song-start");
        settle(&mut visual, |cx| has(cx, "Made 2 songs"));
        let edits = &fake.written()[0];
        assert_eq!(edits[0].title.as_deref(), Some("سألتك حبيبي"));
        assert_eq!(edits[0].track.as_deref(), Some("4"));
        assert_eq!(edits[1].title.as_deref(), Some("ليلية"));
        testing::set_rtl(false);
    }
}
