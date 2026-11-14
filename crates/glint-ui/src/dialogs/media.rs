//! Media files: convert, trim, extract, subtitles (MediaDialog.tsx).
//!
//! Ported from src/components/explorer/MediaDialog.tsx, with the parts of ActionParts.tsx, actions.ts, studio.ts and
//! tools.ts it used. The run, choosers, progress, results and buttons are `dialogs::action_run`'s (ActionParts.tsx),
//! shared with Convert, Make smaller, PDF tools and the song, metadata and media-tools views.
//!
//! | TypeScript                                     | Here                                                      |
//! |------------------------------------------------|-----------------------------------------------------------|
//! | `openMedia(entries, mode)`                     | [`open`] (the menus' `Media { paths, tool }`)             |
//! | `MediaDialog` / `MediaModal`                   | [`MediaDialog`]                                           |
//! | `mediaProbe`, `mediaEncoders`, `mediaRun`, `mediaCancel`, `whisper_*` | [`Backend`] (a fake in the tests)          |
//! | `isVideoFile`, `isAudioFile`, `isMediaFile`    | [`is_video_file`], [`is_audio_file`], [`is_media_file`]   |
//! | `seconds`, `clock`, `encoderName`              | [`seconds`], [`clock`], [`encoder_name`]                  |
//! | `last` (the choices of last time)              | `LAST`, kept while Glint runs                             |
//! | `useActionRun`, `WhereChooser`, `RunProgress`, `RunResult`, `RunFooter`, `Choices`, `ToolNeeded` | [`action_run`]   |
//!
//! **How the coordinator wires it.** `MenuEvent::Media { paths, tool }` (the Media submenu's items and the
//! actions Explorer.tsx offered): call `dialogs::media::open(window, cx, paths, tool)`. `MediaTool::Convert` and
//! `Gif` take the videos among the paths, the others the videos and recordings; when none is left it says so in
//! a notification, as the TypeScript's toast did. The menus have already checked that the paths are on disk (not
//! in an archive, closed, online-only or in the Recycle Bin).
//!
//! NOTE(port): the TypeScript's drop-downs (Height, GIF width, Frames) are rows of choices here: the kit has no
//! drop-down, and five or six short options read as well as a list does.

use super::action_run::{
    self, Choice, Choices, FooterSpec, Fut, Launch, ResultWords, Run, Tooling, Where, WhereChooser,
};
use crate::components::menus::MediaTool;
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::actions::{Output, Progress, Report};
use glint_core::host::Manager as _;
use glint_core::media_tools::{AudioFormat, Encoders, Probe, Quality, Task, VideoFormat};
use glint_model::format::{format_count, format_size, plural};
use glint_model::paths::base_name;
use std::sync::{Arc, Mutex};

// ---- What FFmpeg reads (studio.ts) ------------------------------------------------------------------------

/// Videos FFmpeg reads (`VIDEO_EXTS`).
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "ts", "mts", "m2ts", "mpg", "mpeg", "3gp", "ogv", "vob",
    "asf", "f4v",
];
/// Recordings FFmpeg reads (`AUDIO_EXTS`).
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "amr", "aiff", "aif", "ape", "mka", "ac3", "dts", "caf",
];

/// The lower-case extension of a file's name, "" when it has none.
fn ext_of(path: &str) -> String {
    match base_name(path).rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// Whether FFmpeg reads `path` as a video (`isVideoFile`, by its name; the menus have checked it is on disk).
pub fn is_video_file(path: &str) -> bool {
    VIDEO_EXTS.contains(&ext_of(path).as_str())
}

/// Whether FFmpeg reads `path` as a recording (`isAudioFile`).
pub fn is_audio_file(path: &str) -> bool {
    AUDIO_EXTS.contains(&ext_of(path).as_str())
}

/// A video or a recording (`isMediaFile`).
pub fn is_media_file(path: &str) -> bool {
    is_video_file(path) || is_audio_file(path)
}

/// What JavaScript's `Number(text)` makes of a piece of a time: an empty piece is 0, and what isn't a finite
/// number is nothing.
fn js_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.);
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// "1:05" or "12.5" as seconds; `None` when it isn't one (`seconds`).
pub fn seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut total = 0.;
    for part in text.split(':') {
        let n = js_number(part)?;
        if n < 0. {
            return None;
        }
        total = total * 60. + n;
    }
    Some(total)
}

/// 65.4 as "1:05" (`clock`).
pub fn clock(s: f64) -> String {
    let s = s.max(0.);
    format!("{}:{:02}", (s / 60.).floor() as u64, (s % 60.).floor() as u64)
}

/// What a hardware encoder is called, for people (`encoderName`).
pub fn encoder_name(encoder: Option<&str>) -> String {
    let Some(encoder) = encoder else {
        return String::new();
    };
    if encoder.ends_with("_nvenc") {
        "NVIDIA GPU".into()
    } else if encoder.ends_with("_qsv") {
        "Intel Quick Sync".into()
    } else if encoder.ends_with("_amf") {
        "AMD GPU".into()
    } else {
        match encoder {
            "libx264" => "x264".into(),
            "libx265" => "x265".into(),
            "libsvtav1" => "SVT-AV1".into(),
            other => other.into(),
        }
    }
}

// ---- Modes and the choices of last time -------------------------------------------------------------------

/// What the dialog makes (`MediaMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaMode {
    /// Another format, smaller.
    Convert,
    /// The sound alone.
    Audio,
    /// An animated GIF or WebP.
    Gif,
    /// An .srt of what is said.
    Subtitles,
    /// The volume evened out.
    Volume,
}

impl From<MediaTool> for MediaMode {
    fn from(tool: MediaTool) -> Self {
        match tool {
            MediaTool::Convert => MediaMode::Convert,
            MediaTool::Audio => MediaMode::Audio,
            MediaTool::Gif => MediaMode::Gif,
            MediaTool::Subtitles => MediaMode::Subtitles,
            MediaTool::Volume => MediaMode::Volume,
        }
    }
}

impl MediaMode {
    const ALL: [MediaMode; 5] = [
        MediaMode::Convert,
        MediaMode::Audio,
        MediaMode::Gif,
        MediaMode::Subtitles,
        MediaMode::Volume,
    ];

    /// The tab's name.
    fn label(self) -> SharedString {
        match self {
            MediaMode::Convert => t!("Convert & shrink"),
            MediaMode::Audio => t!("Sound only"),
            MediaMode::Gif => t!("GIF"),
            MediaMode::Subtitles => t!("Subtitles"),
            MediaMode::Volume => t!("Even volume"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            MediaMode::Convert => IconName::Film,
            MediaMode::Audio => IconName::AudioLines,
            MediaMode::Gif => IconName::Clapperboard,
            MediaMode::Subtitles => IconName::Captions,
            MediaMode::Volume => IconName::Volume2,
        }
    }

    /// The action button's name.
    fn start_label(self) -> SharedString {
        match self {
            MediaMode::Convert => t!("Convert"),
            MediaMode::Audio => t!("Take the sound out"),
            MediaMode::Gif => t!("Make it"),
            MediaMode::Subtitles => t!("Make subtitles"),
            MediaMode::Volume => t!("Even it out"),
        }
    }

    /// Completes "Undid the …" and the Undo offer: "conversion of 2 files".
    fn undo_verb(self) -> SharedString {
        match self {
            MediaMode::Convert => t!("conversion"),
            MediaMode::Audio => t!("sound taken out"),
            MediaMode::Gif => t!("GIF"),
            MediaMode::Subtitles => t!("subtitles"),
            MediaMode::Volume => t!("volume evened out"),
        }
    }

    /// What "Next to each file" adds to a name to begin with.
    fn suffix(self) -> &'static str {
        if self == MediaMode::Convert {
            " (converted)"
        } else {
            ""
        }
    }
}

/// How good Convert makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualityKind {
    Good,
    High,
    Size,
}

/// The choices made in the dialog, kept for the next one while Glint runs (`last`).
#[derive(Clone, Debug, PartialEq)]
struct Picked {
    format: VideoFormat,
    quality: QualityKind,
    mb: f64,
    height: u32,
    audio: AudioFormat,
    gif_width: u32,
    fps: u32,
    webp: bool,
    translate: bool,
    music: bool,
}

impl Default for Picked {
    fn default() -> Self {
        Self {
            format: VideoFormat::H264,
            quality: QualityKind::Good,
            mb: 25.,
            height: 0,
            audio: AudioFormat::Same,
            gif_width: 480,
            fps: 12,
            webp: false,
            translate: false,
            music: false,
        }
    }
}

static LAST: Mutex<Option<Picked>> = Mutex::new(None);

/// What `Number(text) || 1` makes of the megabytes box.
fn parse_mb(text: &str) -> f64 {
    js_number(text).filter(|n| *n != 0.).unwrap_or(1.)
}

/// The megabytes as the box shows them: whole numbers without a point.
fn mb_text(mb: f64) -> String {
    if mb.fract() == 0. {
        format!("{}", mb as i64)
    } else {
        mb.to_string()
    }
}

// ---- What the dialog asks of the PC ------------------------------------------------------------------------

/// The calls into glint-core's media studio and the speech model. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// What the first file holds, and its size (`mediaProbe`; the size was the entry's).
    fn probe(&self, path: String) -> Fut<(Probe, u64)>;
    /// The encoders this FFmpeg has (`mediaEncoders`).
    fn encoders(&self) -> Fut<Encoders>;
    /// Runs `task` on each of `files` as `output` says, reporting progress on `progress` (`mediaRun`).
    fn run(
        &self,
        files: Vec<String>,
        task: Task,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the run under way (`mediaCancel`).
    fn cancel(&self);
    /// Whether whisper.cpp and its model are there (`mediaStatus().speechInstalled`).
    fn speech_installed(&self) -> Fut<bool>;
    /// Installs whisper.cpp and downloads its model, telling `progress` how far it is ("Downloading 40%"); whether
    /// they are there now (`whisper_install`).
    fn install_speech(&self, progress: async_channel::Sender<String>) -> Fut<bool>;
    /// What the install cards ask about tools (none: every tool counts as there).
    fn tools(&self) -> Option<Arc<dyn SettingsBackend>>;
}

/// glint-core's media studio, owned by the main window as the shell dialogs it opens are.
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
    fn probe(&self, path: String) -> Fut<(Probe, u64)> {
        action_run::on_core(async move {
            let probe = glint_core::media_tools::media_probe(path.clone()).await?;
            let size = glint_core::fsx::blocking(move || Ok(std::fs::metadata(&path).map_or(0, |m| m.len()))).await?;
            Ok((probe, size))
        })
    }

    fn encoders(&self) -> Fut<Encoders> {
        action_run::on_core(glint_core::media_tools::media_encoders())
    }

    fn run(
        &self,
        files: Vec<String>,
        task: Task,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        let app = self.app.clone();
        action_run::on_core(async move {
            let window = action_run::main_window(&app)?;
            glint_core::media_tools::media_run(window, files, task, output, progress).await
        })
    }

    fn cancel(&self) {
        glint_core::media_tools::media_cancel();
    }

    fn speech_installed(&self) -> Fut<bool> {
        action_run::on_core(glint_core::whisper::whisper_installed())
    }

    fn install_speech(&self, progress: async_channel::Sender<String>) -> Fut<bool> {
        let (fetch, heard) = async_channel::unbounded();
        let app = self.app.clone();
        let install = action_run::on_core(async move {
            let installs = app.state::<glint_core::tools::Installs>();
            let channel = crate::components::settings::fetch_channel(fetch);
            glint_core::whisper::whisper_install(channel, installs).await
        });
        // "Downloading 40%": the stage and how far, as the TypeScript's `setFetching` worded it.
        glint_core::host::async_runtime::spawn(async move {
            while let Ok(p) = heard.recv().await {
                let text = if p.total > 0 {
                    format!("{} {}%", p.stage, (100. * p.done as f64 / p.total as f64).round())
                } else {
                    p.stage
                };
                if progress.send(text).await.is_err() {
                    break;
                }
            }
        });
        install
    }

    fn tools(&self) -> Option<Arc<dyn SettingsBackend>> {
        Some(Arc::new(SettingsCoreBackend::new(self.app.clone())))
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn probe(&self, _: String) -> Fut<(Probe, u64)> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn encoders(&self) -> Fut<Encoders> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn run(&self, _: Vec<String>, _: Task, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self) {}
    fn speech_installed(&self) -> Fut<bool> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn install_speech(&self, _: async_channel::Sender<String>) -> Fut<bool> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn tools(&self) -> Option<Arc<dyn SettingsBackend>> {
        None
    }
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// Media studio on some videos or recordings: what to make, its choices, where it goes, the run and its result.
pub struct MediaDialog {
    files: Vec<String>,
    mode: MediaMode,
    picked: Picked,
    encoders: Option<Encoders>,
    probe: Option<(Probe, u64)>,
    /// FFmpeg is there and the encoders and the first file have been asked for.
    asked: bool,
    /// Whether the speech model is there, once known.
    speech: Option<bool>,
    speech_asked: bool,
    /// How far the speech download is, while it runs.
    fetching: Option<SharedString>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    mb: Entity<InputState>,
    place: Entity<WhereChooser>,
    run: Run,
    backend: Arc<dyn Backend>,
    gate: Tooling,
    _subscriptions: Vec<Subscription>,
}

impl MediaDialog {
    /// A dialog for `files` (all usable for `mode`), starting at `mode`, calling `backend`.
    pub fn new(
        files: Vec<String>,
        mode: MediaMode,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picked = LAST.lock().ok().and_then(|last| last.clone()).unwrap_or_default();
        let from = cx.new(|cx| InputState::new(window, cx).placeholder("0:00"));
        let to = cx.new(|cx| InputState::new(window, cx).placeholder(t!("the end")));
        let mb = cx.new(|cx| InputState::new(window, cx).default_value(mb_text(picked.mb)));
        let start = Where::Beside {
            suffix: mode.suffix().to_string(),
        };
        let place = cx.new(|cx| WhereChooser::starting_at("media-where", &start, window, cx));
        let gate = Tooling::from_backend(backend.tools(), window, cx);
        let mut subscriptions = vec![cx.observe(&place, |_, _, cx| cx.notify())];
        for input in [&from, &to] {
            subscriptions.push(cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.subscribe_in(&mb, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let mb = parse_mb(&input.read(cx).value());
                this.choose(|p| p.mb = mb, cx);
            }
        }));
        subscriptions.extend(gate.observe(cx, |this, cx| {
            this.look(cx);
            cx.notify();
        }));
        let mut this = Self {
            files,
            mode,
            picked,
            encoders: None,
            probe: None,
            asked: false,
            speech: None,
            speech_asked: false,
            fetching: None,
            from,
            to,
            mb,
            place,
            run: Run::Setup,
            backend,
            gate,
            _subscriptions: subscriptions,
        };
        this.sync_place(false, window, cx);
        this.look(cx);
        this.look_for_speech(cx);
        this
    }

    /// The files being worked on.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// What the dialog makes now.
    pub fn mode(&self) -> MediaMode {
        self.mode
    }

    /// Where the run stands.
    #[cfg(test)]
    pub fn run_state(&self) -> &Run {
        &self.run
    }

    /// The "Part" boxes of a GIF: from and to.
    #[cfg(test)]
    pub fn range_inputs(&self) -> (&Entity<InputState>, &Entity<InputState>) {
        (&self.from, &self.to)
    }

    /// The chooser of where results go.
    pub fn place(&self) -> &Entity<WhereChooser> {
        &self.place
    }

    fn run_mut(&mut self) -> &mut Run {
        &mut self.run
    }

    /// Changes a choice and keeps it for the next dialog (`choose`).
    fn choose(&mut self, change: impl FnOnce(&mut Picked), cx: &mut Context<Self>) {
        change(&mut self.picked);
        if let Ok(mut last) = LAST.lock() {
            *last = Some(self.picked.clone());
        }
        cx.notify();
    }

    /// Asks for the encoders and what the first file holds, once FFmpeg is there (the TypeScript's effect on
    /// `ready` and `files`).
    fn look(&mut self, cx: &mut Context<Self>) {
        if self.asked || !self.gate.ready("ffmpeg", cx) {
            return;
        }
        self.asked = true;
        let encoders = self.backend.encoders();
        let first = self.files.first().cloned().unwrap_or_default();
        let probe = self.backend.probe(first);
        cx.spawn(async move |this, cx| {
            let encoders = encoders.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(encoders) = encoders {
                    this.encoders = Some(encoders);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let probe = probe.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(probe) = probe {
                    this.probe = Some(probe);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Asks whether the speech model is there when Subtitles is the mode.
    fn look_for_speech(&mut self, cx: &mut Context<Self>) {
        if self.mode != MediaMode::Subtitles || self.speech_asked {
            return;
        }
        self.speech_asked = true;
        let installed = self.backend.speech_installed();
        cx.spawn(async move |this, cx| {
            let installed = installed.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(installed) = installed {
                    this.speech = Some(installed);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Tells the chooser of where results go what this mode needs: its suffix when it is "Next to", the word for
    /// what is made, and whether "In their place" is offered.
    fn sync_place(&self, suffix: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.mode;
        self.place.update(cx, |place, cx| {
            place.set_noun(
                if mode == MediaMode::Convert {
                    t!("video")
                } else {
                    t!("file")
                },
                cx,
            );
            place.offer_replace(matches!(mode, MediaMode::Convert | MediaMode::Volume), cx);
            if suffix {
                place.set_suffix(mode.suffix(), window, cx);
            }
        });
    }

    fn set_mode(&mut self, mode: MediaMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode || !matches!(self.run, Run::Setup) {
            return;
        }
        self.mode = mode;
        self.sync_place(true, window, cx);
        self.look_for_speech(cx);
        cx.notify();
    }

    fn videos(&self) -> Vec<String> {
        self.files.iter().filter(|f| is_video_file(f)).cloned().collect()
    }

    /// The files the mode can work on: videos for Convert and GIF, videos and recordings for the rest.
    pub fn usable(&self) -> Vec<String> {
        let wants_video = matches!(self.mode, MediaMode::Convert | MediaMode::Gif);
        self.files
            .iter()
            .filter(|f| {
                if wants_video {
                    is_video_file(f)
                } else {
                    is_media_file(f)
                }
            })
            .cloned()
            .collect()
    }

    /// The part of a GIF: its start and end in seconds, where given.
    fn range(&self, cx: &App) -> (Option<f64>, Option<f64>) {
        (seconds(&self.from.read(cx).value()), seconds(&self.to.read(cx).value()))
    }

    /// Whether the part typed isn't one (`rangeBad`).
    pub fn range_bad(&self, cx: &App) -> bool {
        let (from, to) = (self.from.read(cx).value(), self.to.read(cx).value());
        let (start, end) = self.range(cx);
        (!from.trim().is_empty() && start.is_none())
            || (!to.trim().is_empty() && end.is_none())
            || matches!((start, end), (Some(start), Some(end)) if end <= start)
    }

    fn speech_missing(&self) -> bool {
        self.mode == MediaMode::Subtitles && self.speech == Some(false)
    }

    /// What the mode asks core to do with the choices made.
    pub fn task(&self, cx: &App) -> Task {
        let p = &self.picked;
        match self.mode {
            MediaMode::Convert => Task::Convert {
                format: p.format,
                quality: match p.quality {
                    QualityKind::Good => Quality::Good,
                    QualityKind::High => Quality::High,
                    QualityKind::Size => Quality::Size { mb: p.mb.max(1.) },
                },
                height: (p.height != 0).then_some(p.height),
            },
            MediaMode::Audio => Task::Audio { format: p.audio },
            MediaMode::Gif => {
                let (start, end) = self.range(cx);
                Task::Gif {
                    width: p.gif_width,
                    fps: p.fps,
                    start,
                    end,
                    webp: p.webp,
                }
            }
            MediaMode::Volume => Task::Loudness { music: p.music },
            MediaMode::Subtitles => Task::Subtitles { translate: p.translate },
        }
    }

    /// Whether the action can be pressed: FFmpeg is there, there is something to work on, the output and the
    /// part are usable, and Subtitles has its model.
    pub fn can_start(&self, cx: &App) -> bool {
        matches!(self.run, Run::Setup)
            && self.gate.ready("ffmpeg", cx)
            && !self.usable().is_empty()
            && !self.place.read(cx).bad(cx)
            && !(self.mode == MediaMode::Gif && self.range_bad(cx))
            && !self.speech_missing()
    }

    /// Starts the run on core's runtime; progress and the result come back here (`start`).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let task = self.task(cx);
        let output = self.place.read(cx).choice(cx).output();
        let usable = self.usable();
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.run(usable.clone(), task, output, channel);
        let verb = self.mode.undo_verb();
        action_run::drive(
            self,
            window,
            cx,
            Launch {
                total: usable.len(),
                call,
                progress: receiver,
            },
            Self::run_mut,
            move |report| {
                t!(
                    "{verb} of {what}",
                    verb = verb,
                    what = plural(report.made.len(), "file")
                )
                .to_string()
            },
        );
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// Downloads the speech model and whisper.cpp (`fetchSpeech`).
    fn fetch_speech(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fetching.is_some() {
            return;
        }
        self.fetching = Some(t!("Starting"));
        cx.notify();
        let (progress, heard) = async_channel::unbounded();
        let call = self.backend.install_speech(progress);
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(text) = heard.recv().await {
                let alive = this.update(cx, |this, cx| {
                    this.fetching = Some(text.into());
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.fetching = None;
                match result {
                    Ok(installed) => this.speech = Some(installed),
                    Err(reason) if reason != "cancelled" => window.push_notification(
                        Notification::error(SharedString::from(reason)).title(t!("Couldn't download the speech model")),
                        cx,
                    ),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ---- Drawing ----------------------------------------------------------------------------------------------

impl MediaDialog {
    /// The row of tabs that says what to make.
    fn tabs(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let no_videos = self.videos().is_empty();
        row_start()
            .id("media-modes")
            .role(Role::TabList)
            .aria_label(t!("What to make"))
            .test_support()
            .gap(space(1.))
            .p(space(1.))
            .rounded(theme::size::RADIUS_LG)
            .bg(t.muted)
            .children(MediaMode::ALL.into_iter().enumerate().map(|(ix, mode)| {
                let chosen = self.mode == mode;
                Button::new(("media-mode", ix))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen { t.background } else { t.background.opacity(0.) })
                            .foreground(if chosen { t.foreground } else { t.muted_foreground })
                            .hover(if chosen { t.background } else { t.accent })
                            .active(t.accent),
                    )
                    .flex_1()
                    .min_w_0()
                    .compact()
                    .text_size(theme::size::XS)
                    .role(Role::Tab)
                    .toggled(chosen)
                    .disabled(matches!(mode, MediaMode::Convert | MediaMode::Gif) && no_videos)
                    .icon(mode.icon())
                    .label(mode.label())
                    .on_click(cx.listener(move |this, _, window, cx| this.set_mode(mode, window, cx)))
            }))
    }

    /// "H264 1920×1080 at 30 fps · AAC sound · 1:05 · 12 MB" for one file.
    pub fn probe_line(&self) -> Option<SharedString> {
        let (probe, size) = self.probe.as_ref().filter(|_| self.files.len() == 1)?;
        let mut line = Vec::new();
        if !probe.video.is_empty() {
            let fps = if probe.fps > 0. {
                t!(" at {fps} fps", fps = (probe.fps + 0.5).floor() as u64).to_string()
            } else {
                String::new()
            };
            line.push(format!(
                "{} {}×{}{fps}",
                probe.video.to_uppercase(),
                probe.width,
                probe.height
            ));
        }
        if !probe.audio.is_empty() {
            line.push(t!("{codec} sound", codec = probe.audio.to_uppercase()).to_string());
        }
        if probe.duration > 0. {
            line.push(clock(probe.duration));
        }
        line.push(format_size(*size));
        Some(line.join(" · ").into())
    }

    fn convert_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let encoders = self.encoders.as_ref();
        let chosen_encoder = encoders.and_then(|e| match self.picked.format {
            VideoFormat::H264 => e.h264.as_deref(),
            VideoFormat::Hevc => e.hevc.as_deref(),
            VideoFormat::Av1 => e.av1.as_deref(),
        });
        let no = |has: fn(&Encoders) -> bool| encoders.is_some_and(|e| !has(e));
        let sizes = [8., 10., 25., 50., 100.];
        col()
            .gap(space(4.))
            .child(
                col()
                    .gap(space(1.5))
                    .child(action_run::section_title(t!("Into"), t))
                    .child(
                        Choices::new("media-format", t!("Format"), self.picked.format)
                            .columns(3)
                            .options(vec![
                                Choice::new(VideoFormat::H264, t!("MP4 (H.264)"))
                                    .hint(t!("Plays everywhere"))
                                    .disabled(no(|e| e.h264.is_some())),
                                Choice::new(VideoFormat::Hevc, t!("MP4 (HEVC)"))
                                    .hint(t!("About half the size"))
                                    .disabled(no(|e| e.hevc.is_some())),
                                Choice::new(VideoFormat::Av1, t!("MP4 (AV1)"))
                                    .hint(if no(|e| e.av1.is_some()) {
                                        t!("No AV1 encoder here")
                                    } else {
                                        t!("Smallest, newer devices")
                                    })
                                    .disabled(no(|e| e.av1.is_some())),
                            ])
                            .on_pick(
                                cx.listener(|this, format: &VideoFormat, _, cx| {
                                    this.choose(|p| p.format = *format, cx)
                                }),
                            ),
                    )
                    .when_some(chosen_encoder, |this, encoder| {
                        this.child(action_run::help(
                            t!("Encoded with {name}.", name = encoder_name(Some(encoder))),
                            t,
                        ))
                    }),
            )
            .child(
                col()
                    .gap(space(1.5))
                    .child(action_run::section_title(t!("Quality"), t))
                    .child(
                        Choices::new("media-quality", t!("Quality"), self.picked.quality)
                            .columns(3)
                            .options(vec![
                                Choice::new(QualityKind::Good, t!("Good")).hint(t!("Small, fine to share")),
                                Choice::new(QualityKind::High, t!("High")).hint(t!("Close to the original")),
                                Choice::new(QualityKind::Size, t!("Fit under a size")).hint(t!("For mail and chats")),
                            ])
                            .on_pick(cx.listener(|this, quality: &QualityKind, _, cx| {
                                this.choose(|p| p.quality = *quality, cx)
                            })),
                    )
                    .when(self.picked.quality == QualityKind::Size, |this| {
                        this.child(
                            align_row(1.5)
                                .flex_wrap()
                                .pt(space(1.))
                                .children(sizes.into_iter().enumerate().map(|(ix, mb)| {
                                    let on = self.picked.mb == mb;
                                    let button = Button::new(("media-size", ix))
                                        .compact()
                                        .label(t!("{mb} MB", mb = mb as u32))
                                        .toggled(on)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            // Setting the box's text says nothing to the dialog, so it is told.
                                            this.mb.update(cx, |input, cx| input.set_value(mb_text(mb), window, cx));
                                            this.choose(|p| p.mb = mb, cx);
                                        }));
                                    if on {
                                        button.primary()
                                    } else {
                                        button.outline()
                                    }
                                }))
                                .child(
                                    div()
                                        .w(space(20.))
                                        .child(Input::new(&self.mb).small().aria_label(t!("Megabytes"))),
                                )
                                .child(action_run::help(t!("MB each"), t)),
                        )
                    }),
            )
            .child(action_run::field(
                t!("Height"),
                Choices::new("media-height", t!("Height"), self.picked.height)
                    .columns(3)
                    .options(vec![
                        Choice::new(0, t!("Keep it")),
                        Choice::new(2160, t!("2160p (4K) at most")),
                        Choice::new(1080, t!("1080p at most")),
                        Choice::new(720, t!("720p at most")),
                        Choice::new(480, t!("480p at most")),
                    ])
                    .on_pick(cx.listener(|this, height: &u32, _, cx| this.choose(|p| p.height = *height, cx))),
                t,
            ))
    }

    fn audio_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        col()
            .gap(space(1.5))
            .child(action_run::section_title(t!("The sound, as"), t))
            .child(
                Choices::new("media-audio", t!("Format"), self.picked.audio)
                    .columns(3)
                    .options(vec![
                        Choice::new(AudioFormat::Same, t!("As it is")).hint(t!("No quality lost, at once")),
                        Choice::new(AudioFormat::Mp3, t!("MP3")).hint(t!("Plays everywhere")),
                        Choice::new(AudioFormat::M4a, t!("M4A (AAC)")).hint(t!("Phones and iTunes")),
                        Choice::new(AudioFormat::Opus, t!("Opus")).hint(t!("Smallest for speech")),
                        Choice::new(AudioFormat::Flac, t!("FLAC")).hint(t!("Lossless")),
                        Choice::new(AudioFormat::Wav, t!("WAV")).hint(t!("Uncompressed, for editing")),
                    ])
                    .on_pick(cx.listener(|this, audio: &AudioFormat, _, cx| this.choose(|p| p.audio = *audio, cx))),
            )
    }

    fn gif_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let no_webp = self.encoders.as_ref().is_some_and(|e| !e.webp);
        let bad = self.range_bad(cx);
        col()
            .gap(space(3.))
            .child(
                Choices::new("media-kind", t!("Kind"), self.picked.webp)
                    .options(vec![
                        Choice::new(false, t!("GIF")).hint(t!("Works in every chat and page")),
                        Choice::new(true, t!("Animated WebP"))
                            .hint(if no_webp {
                                t!("This FFmpeg can't make WebP")
                            } else {
                                t!("Far smaller, modern browsers")
                            })
                            .disabled(no_webp),
                    ])
                    .on_pick(cx.listener(|this, webp: &bool, _, cx| this.choose(|p| p.webp = *webp, cx))),
            )
            .child(action_run::field(
                t!("Width"),
                Choices::new("media-gif-width", t!("Width"), self.picked.gif_width)
                    .columns(3)
                    .options(
                        [320, 480, 640, 800, 1080]
                            .into_iter()
                            .map(|w| Choice::new(w, t!("{w} pixels", w = w)))
                            .collect(),
                    )
                    .on_pick(cx.listener(|this, w: &u32, _, cx| this.choose(|p| p.gif_width = *w, cx))),
                t,
            ))
            .child(action_run::field(
                t!("Frames"),
                Choices::new("media-gif-fps", t!("Frames a second"), self.picked.fps)
                    .columns(3)
                    .options(
                        [8, 10, 12, 15, 20, 24]
                            .into_iter()
                            .map(|f| Choice::new(f, t!("{f} a second", f = f)))
                            .collect(),
                    )
                    .on_pick(cx.listener(|this, f: &u32, _, cx| this.choose(|p| p.fps = *f, cx))),
                t,
            ))
            .child(
                align_row(2.)
                    .child(
                        div()
                            .w(space(20.))
                            .flex_none()
                            .text_size(theme::size::XS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.muted_foreground)
                            .child(t!("Part")),
                    )
                    .child(
                        div()
                            .w(space(20.))
                            .child(Input::new(&self.from).small().aria_label(t!("From"))),
                    )
                    .child(div().text_color(t.muted_foreground).child(t!("to")))
                    .child(
                        div()
                            .w(space(20.))
                            .child(Input::new(&self.to).small().aria_label(t!("To"))),
                    )
                    .when(bad, |this| {
                        this.child(
                            div()
                                .id("media-range-bad")
                                .role(Role::Alert)
                                .aria_label(t!("Not a part of it"))
                                .test_support()
                                .text_size(theme::size::XS)
                                .text_color(t.destructive)
                                .child(t!("Not a part of it")),
                        )
                    }),
            )
            .child(action_run::help(
                t!("Keep it short: a GIF of a few seconds is already a few megabytes."),
                t,
            ))
    }

    fn volume_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        col()
            .gap(space(1.5))
            .child(
                Choices::new("media-loud", t!("Loud as"), self.picked.music)
                    .options(vec![
                        Choice::new(false, t!("Speech and podcasts")).hint(t!("−16 LUFS, as podcast apps play")),
                        Choice::new(true, t!("Music")).hint(t!("−14 LUFS, as streaming services play")),
                    ])
                    .on_pick(cx.listener(|this, music: &bool, _, cx| this.choose(|p| p.music = *music, cx))),
            )
            .child(action_run::help(
                t!("Measured first, then turned up or down in one even step (EBU R128), so recordings from different places sound as loud as each other without clipping. A video's picture is kept as it is."),
                t,
            ))
    }

    fn subtitle_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let fetching = self.fetching.clone();
        col()
            .gap(space(3.))
            .child(
                Choices::new("media-language", t!("Language"), self.picked.translate)
                    .options(vec![
                        Choice::new(false, t!("As spoken")).hint(t!("In the language heard")),
                        Choice::new(true, t!("In English")).hint(t!("Translated as it's heard")),
                    ])
                    .on_pick(cx.listener(|this, translate: &bool, _, cx| {
                        this.choose(|p| p.translate = *translate, cx)
                    })),
            )
            .child(action_run::help(
                t!("Heard on this PC by whisper.cpp (speech search's model); nothing is sent anywhere. Players find an .srt named like its video by themselves."),
                t,
            ))
            .when(self.speech_missing(), |this| {
                let why = t!("Subtitles need whisper.cpp and its speech model (60 MB).");
                this.child(
                    row_start()
                        .id("media-speech")
                        .role(Role::Group)
                        .aria_label(why.clone())
                        .test_support()
                        .gap(space(3.))
                        .px(space(4.))
                        .py(space(3.))
                        .rounded(theme::size::RADIUS_LG)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.primary.opacity(0.05))
                        .child(icon(IconName::Captions).size_4().flex_none().text_color(t.primary))
                        .child(div().flex_1().min_w_0().text_start().child(why))
                        .child(
                            Button::new("media-speech-download")
                                .primary()
                                .icon(IconName::Download)
                                .label(fetching.clone().unwrap_or_else(|| t!("Download")))
                                .loading(fetching.is_some())
                                .disabled(fetching.is_some())
                                .on_click(cx.listener(|this, _, window, cx| this.fetch_speech(window, cx))),
                        ),
                )
            })
    }

    /// The choices, before the run starts.
    fn setup(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let usable = self.usable().len();
        let left_out = self.files.len().saturating_sub(usable);
        col()
            .gap(space(4.))
            .child(self.tabs(t, cx))
            .when_some(self.probe_line(), |this, line| {
                this.child(
                    div()
                        .id("media-probe")
                        .role(Role::Status)
                        .aria_label(line.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(line),
                )
            })
            .child(match self.mode {
                MediaMode::Convert => self.convert_options(t, cx).into_any_element(),
                MediaMode::Audio => self.audio_options(t, cx).into_any_element(),
                MediaMode::Gif => self.gif_options(t, cx).into_any_element(),
                MediaMode::Volume => self.volume_options(t, cx).into_any_element(),
                MediaMode::Subtitles => self.subtitle_options(t, cx).into_any_element(),
            })
            .when(left_out > 0, |this| {
                this.child(
                    div()
                        .id("media-left-out")
                        .role(Role::Status)
                        .aria_label(t!(
                            "{what} without a video will be left out.",
                            what = plural(left_out, "file")
                        ))
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.warning)
                        .child(t!(
                            "{what} without a video will be left out.",
                            what = plural(left_out, "file")
                        )),
                )
            })
            .child(self.place.clone())
            .into_any_element()
    }
}

/// The dialog's title: the file's name, or how many videos or files.
pub fn title(files: &[String]) -> SharedString {
    match files {
        [one] => base_name(one).to_string().into(),
        _ if files.iter().all(|f| is_video_file(f)) => t!("{n} videos", n = format_count(files.len())),
        _ => t!("{n} files", n = format_count(files.len())),
    }
}

impl Render for MediaDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let body = match self.run.clone() {
            Run::Setup => self.setup(&t, cx),
            Run::Running(progress) => {
                let verb = if self.mode == MediaMode::Subtitles {
                    t!("Listening to")
                } else {
                    t!("Working on")
                };
                action_run::run_progress("media-progress", &verb, &progress, &t).into_any_element()
            }
            Run::Done(report) => action_run::result_view(
                "media-result",
                &report,
                &ResultWords::files(self.mode == MediaMode::Convert),
                &t,
            )
            .into_any_element(),
            Run::Failed(reason) => {
                action_run::error_view("media-error", t!("Couldn't make it"), reason, &t).into_any_element()
            }
        };
        let card = self.gate.card(
            "ffmpeg",
            t!("FFmpeg converts, shrinks and cuts videos and sound, with your graphics card when it can."),
            IconName::Film,
            cx,
        );
        let footer = action_run::footer(
            FooterSpec {
                prefix: "media",
                start_label: self.mode.start_label(),
                start_icon: self.mode.icon(),
                can_start: self.can_start(cx),
                stoppable: true,
                show: self.run.first_made(),
            },
            &self.run,
            Self::start,
            |this, _, _| this.stop(),
            &t,
            cx,
        );
        col()
            .id("media")
            .role(Role::Group)
            .aria_label(t!("Media: {title}", title = title(&self.files)))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .child(action_run::folder_line("media-folder", &self.files, &t))
            .child(
                div()
                    .id("media-body")
                    .max_h(action_run::body_height(window))
                    .overflow_y_scrollbar()
                    .child(col().gap(space(4.)).children(card).child(body)),
            )
            .child(footer)
    }
}

// ---- Opening it -------------------------------------------------------------------------------------------

/// Opens Media studio on the videos and recordings among `paths`, at the mode `tool` names (`openMedia`). When
/// none is left it says so in a notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, tool: MediaTool) {
    let mode = MediaMode::from(tool);
    let wants_video = matches!(mode, MediaMode::Convert | MediaMode::Gif);
    let files: Vec<String> = paths
        .into_iter()
        .filter(|p| {
            if wants_video {
                is_video_file(p)
            } else {
                is_media_file(p)
            }
        })
        .collect();
    if files.is_empty() {
        let message = if wants_video {
            t!("No videos chosen")
        } else {
            t!("No videos or recordings chosen")
        };
        window.push_notification(Notification::info(message), cx);
        return;
    }
    let backend: Arc<dyn Backend> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    };
    open_with(window, cx, backend, files, mode);
}

/// [`open`] for a given backend, without filtering `files` (tests pass a fake). Returns the dialog's view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    files: Vec<String>,
    mode: MediaMode,
) -> Entity<MediaDialog> {
    let shown = isolate(title(&files));
    let view = cx.new(|cx| MediaDialog::new(files, mode, backend, window, cx));
    super::show(window, cx, shown, px(576.), view.clone());
    view
}

// ---- Tests ------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::actions::Recycled;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::time::Duration;

    /// What the fake was asked to run.
    type RunCall = (Vec<String>, Task, Output);

    /// A backend with canned answers; a run waits for `release` when it has one.
    struct Fake {
        probe: Probe,
        size: u64,
        encoders: Encoders,
        report: Report,
        error: Option<String>,
        release: Option<async_channel::Receiver<()>>,
        speech: Mutex<bool>,
        downloads: Mutex<usize>,
        calls: Mutex<Vec<RunCall>>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(report: Report) -> Self {
            Self {
                probe: Probe {
                    duration: 65.4,
                    width: 1920,
                    height: 1080,
                    fps: 29.97,
                    video: "h264".into(),
                    audio: "aac".into(),
                    container: "mov,mp4".into(),
                    bitrate: 1,
                    playable: true,
                },
                size: 12 * 1024 * 1024,
                encoders: Encoders {
                    h264: Some("h264_nvenc".into()),
                    hevc: None,
                    av1: None,
                    webp: false,
                    hardware: vec!["nvenc".into()],
                },
                report,
                error: None,
                release: None,
                speech: Mutex::new(true),
                downloads: Mutex::new(0),
                calls: Mutex::new(Vec::new()),
                cancelled: Mutex::new(0),
            }
        }

        fn calls(&self) -> Vec<RunCall> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn probe(&self, _: String) -> Fut<(Probe, u64)> {
            let answer = (self.probe.clone(), self.size);
            Box::pin(async move { Ok(answer) })
        }

        fn encoders(&self) -> Fut<Encoders> {
            let encoders = self.encoders.clone();
            Box::pin(async move { Ok(encoders) })
        }

        fn run(
            &self,
            files: Vec<String>,
            task: Task,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((files.clone(), task, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: files.len(),
                current: "clip.mp4".into(),
                part: 0.5,
            });
            let (report, error, release) = (self.report.clone(), self.error.clone(), self.release.clone());
            Box::pin(async move {
                if let Some(release) = release {
                    let _ = release.recv().await;
                }
                match error {
                    Some(error) => Err(error),
                    None => Ok(report),
                }
            })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }

        fn speech_installed(&self) -> Fut<bool> {
            let there = self.speech.lock().map(|s| *s).unwrap_or(true);
            Box::pin(async move { Ok(there) })
        }

        fn install_speech(&self, progress: async_channel::Sender<String>) -> Fut<bool> {
            if let Ok(mut n) = self.downloads.lock() {
                *n += 1;
            }
            if let Ok(mut there) = self.speech.lock() {
                *there = true;
            }
            Box::pin(async move {
                let _ = progress.send("Downloading 50%".into()).await;
                Ok(true)
            })
        }

        fn tools(&self) -> Option<Arc<dyn SettingsBackend>> {
            None
        }
    }

    fn report(made: &[&str]) -> Report {
        Report {
            made: made.iter().map(|s| (*s).to_string()).collect(),
            before: 12 * 1024 * 1024,
            after: 4 * 1024 * 1024,
            elapsed_ms: 1234.,
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
        panic!("the media dialog never reached the state the test waits for");
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

    /// Whether the button is disabled (an enabled one says nothing at all).
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
            window.input(&text, cx);
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
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn open_view(cx: &mut VisualTestContext, files: &[&str], mode: MediaMode, fake: &Arc<Fake>) -> Entity<MediaDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let view = cx.update(|window, cx| open_with(window, cx, backend, files, mode));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Film,
            IconName::AudioLines,
            IconName::Clapperboard,
            IconName::Captions,
            IconName::Volume2,
            IconName::Download,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::CircleDot,
            IconName::Circle,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn times_clocks_encoders_and_kinds_read_as_the_typescript_did() {
        // seconds(): "1:05" and "12.5", and what isn't a time.
        assert_eq!(seconds("1:05"), Some(65.));
        assert_eq!(seconds("  12.5 "), Some(12.5));
        assert_eq!(seconds("1:02:03"), Some(3723.));
        assert_eq!(seconds(""), None);
        assert_eq!(seconds("abc"), None);
        assert_eq!(seconds("-5"), None);
        assert_eq!(seconds("1:-5"), None);
        assert_eq!(seconds("inf"), None);
        // Number("") is 0, so a piece left empty counts as 0, as it did.
        assert_eq!(seconds("1:"), Some(60.));
        assert_eq!(seconds(":30"), Some(30.));
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(0.), "0:00");
        assert_eq!(clock(3599.9), "59:59");
        assert_eq!(clock(-4.), "0:00");
        assert_eq!(encoder_name(Some("h264_nvenc")), "NVIDIA GPU");
        assert_eq!(encoder_name(Some("hevc_qsv")), "Intel Quick Sync");
        assert_eq!(encoder_name(Some("av1_amf")), "AMD GPU");
        assert_eq!(encoder_name(Some("libx264")), "x264");
        assert_eq!(encoder_name(Some("libx265")), "x265");
        assert_eq!(encoder_name(Some("libsvtav1")), "SVT-AV1");
        assert_eq!(encoder_name(Some("libaom-av1")), "libaom-av1");
        assert_eq!(encoder_name(None), "");
        assert!(is_video_file("C:\\a\\Clip.MP4") && !is_video_file("C:\\a\\song.mp3"));
        assert!(is_audio_file("C:\\a\\song.FLAC") && is_media_file("C:\\a\\song.flac"));
        assert!(!is_media_file("C:\\a\\notes.txt") && !is_media_file("C:\\a\\mp4"));
        // Number(text) || 1 for the megabytes, and how they are shown.
        assert_eq!(
            (parse_mb("25"), parse_mb("0"), parse_mb(""), parse_mb("x")),
            (25., 1., 1., 1.)
        );
        assert_eq!((mb_text(25.), mb_text(1.5)), ("25".to_string(), "1.5".to_string()));
    }

    #[test]
    fn progress_lines_sizes_and_shares_read_as_the_typescript_did() {
        let _guard = testing::lock();
        let at = |done, total, current: &str, part| Progress {
            done,
            total,
            current: current.into(),
            part,
        };
        assert_eq!(
            action_run::progress_text("Working on", &at(0, 1, "", 0.), false, false),
            "Working on"
        );
        assert_eq!(
            action_run::progress_text("Working on", &at(1, 3, "a.mp4", 0.5), false, false),
            "Working on 2 of 3 · a.mp4 · 50%"
        );
        // The count never runs past the total.
        assert_eq!(
            action_run::progress_text("Saving", &at(3, 3, "", 0.), false, false),
            "Saving 3 of 3"
        );
        assert!((action_run::whole(&at(1, 4, "", 0.5)) - 0.375).abs() < 1e-6);
        assert_eq!(action_run::whole(&at(0, 0, "", 0.5)), 0.);
        assert_eq!(
            action_run::sizes_line(12 * 1024 * 1024, 4 * 1024 * 1024).as_deref(),
            Some("12 MB → 4.0 MB (−67%)")
        );
        assert_eq!(
            action_run::sizes_line(1000, 1500).as_deref(),
            Some("1000 B → 1.5 KB (+50%)")
        );
        assert_eq!(action_run::sizes_line(0, 5), None);
    }

    #[test]
    fn the_title_names_the_file_or_counts_videos_and_files() {
        let _guard = testing::lock();
        assert_eq!(title(&strings(&["C:\\a\\clip.mp4"])), "clip.mp4");
        assert_eq!(title(&strings(&["C:\\a\\a.mp4", "C:\\a\\b.mkv"])), "2 videos");
        assert_eq!(title(&strings(&["C:\\a\\a.mp4", "C:\\a\\b.mp3"])), "2 files");
        assert_eq!(MediaMode::from(MediaTool::Gif), MediaMode::Gif);
        assert_eq!(MediaMode::from(MediaTool::Volume), MediaMode::Volume);
    }

    #[gpui_kit::test]
    fn opening_with_nothing_it_takes_says_so_instead_of_opening(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        // Convert and GIF want videos; a recording and a text file give none.
        visual.update(|window, cx| {
            open(
                window,
                cx,
                strings(&["C:\\a\\song.mp3", "C:\\a\\x.txt"]),
                MediaTool::Convert,
            )
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        visual.update(|window, cx| open(window, cx, strings(&["C:\\a\\x.txt"]), MediaTool::Subtitles));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn converting_a_video_shows_what_it_has_runs_with_the_choices_and_reports(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip (converted).mp4"])));
        let view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Media: clip.mp4"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "What to make"), Some(Role::TabList));
        assert_eq!(role_of(&mut visual, "Format"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Where"), Some(Role::RadioGroup));
        // What the file holds, once the probe answers.
        settle(&mut visual, |cx| {
            has(cx, "H264 1920×1080 at 30 fps · AAC sound · 1:05 · 12 MB")
        });
        // Only the encoder this FFmpeg has can be picked.
        assert!(!disabled(&mut visual, ("media-format", 0_usize)));
        assert!(disabled(&mut visual, ("media-format", 1_usize)));
        assert!(disabled(&mut visual, ("media-format", 2_usize)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find(("media-format", 0_usize)).checked(), Some(true));
            assert_eq!(window.find(("media-quality", 0_usize)).checked(), Some(true));
        });
        // Fit under a size shows the megabytes; 10 MB is picked from the row.
        click(&mut visual, ("media-quality", 2_usize));
        click(&mut visual, ("media-size", 1_usize));
        click(&mut visual, ("media-height", 2_usize));
        assert!(!disabled(&mut visual, "media-start"));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ["C:\\a\\clip.mp4"]);
        assert_eq!(
            calls[0].1,
            Task::Convert {
                format: VideoFormat::H264,
                quality: Quality::Size { mb: 10. },
                height: Some(1080),
            }
        );
        assert_eq!(
            calls[0].2,
            Output::Beside {
                suffix: " (converted)".into()
            }
        );
        assert_eq!(role_of(&mut visual, "Made 1 file"), Some(Role::Status));
        // After a run: Show and Close, no Cancel or Convert.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("media-show").is_some());
            assert!(window.try_find("media-start").is_none() && window.try_find("media-cancel").is_none());
        });
        assert!(matches!(
            view.read_with(&visual, |v, _| v.run_state().clone()),
            Run::Done(_)
        ));
        // The choices are kept for the next dialog while Glint runs.
        let last = LAST.lock().ok().and_then(|l| l.clone()).unwrap_or_default();
        assert_eq!((last.quality, last.mb, last.height), (QualityKind::Size, 10., 1080));
        click(&mut visual, "media-close");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn the_tabs_change_what_is_made_and_a_gif_needs_a_real_part(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip.gif"])));
        let view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        let suffix = |cx: &VisualTestContext, view: &Entity<MediaDialog>| {
            view.read_with(cx, |v, cx| v.place().read(cx).choice(cx))
        };
        assert_eq!(
            suffix(&visual, &view),
            Where::Beside {
                suffix: " (converted)".into()
            }
        );
        // The GIF tab: the suffix goes (only Convert adds one) and "In their place" is no longer offered.
        click(&mut visual, ("media-mode", 2_usize));
        assert_eq!(view.read_with(&visual, |v, _| v.mode()), MediaMode::Gif);
        assert_eq!(suffix(&visual, &view), Where::Beside { suffix: String::new() });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find(("media-where", 2_usize)).is_none(),
                "no in-place radio for a GIF"
            );
            assert!(window.try_find(("media-where", 1_usize)).is_some());
        });
        let (from, to) = view.read_with(&visual, |v, _| {
            let (from, to) = v.range_inputs();
            (from.clone(), to.clone())
        });
        // An end before the start is not a part.
        type_into(&mut visual, &from, "5");
        type_into(&mut visual, &to, "3");
        assert_eq!(role_of(&mut visual, "Not a part of it"), Some(Role::Alert));
        assert!(disabled(&mut visual, "media-start"));
        // Words are not times either.
        type_into(&mut visual, &to, "soon");
        assert!(has(&mut visual, "Not a part of it") && disabled(&mut visual, "media-start"));
        type_into(&mut visual, &to, "0:09");
        assert!(!has(&mut visual, "Not a part of it"));
        click(&mut visual, ("media-gif-width", 2_usize));
        click(&mut visual, ("media-gif-fps", 4_usize));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(
            fake.calls()[0].1,
            Task::Gif {
                width: 640,
                fps: 20,
                start: Some(5.),
                end: Some(9.),
                webp: false
            }
        );
        assert_eq!(fake.calls()[0].2, Output::Beside { suffix: String::new() });
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn recordings_get_sound_and_volume_and_videos_without_one_are_left_out(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\talk.opus"])));
        let view = open_view(&mut visual, &["C:\\a\\talk.wav"], MediaMode::Audio, &fake);
        // A recording has no picture to convert or make a GIF of.
        assert!(disabled(&mut visual, ("media-mode", 0_usize)));
        assert!(disabled(&mut visual, ("media-mode", 2_usize)));
        assert!(!disabled(&mut visual, ("media-mode", 1_usize)));
        click(&mut visual, ("media-audio", 3_usize));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(
            fake.calls()[0].1,
            Task::Audio {
                format: AudioFormat::Opus
            }
        );
        drop(view);
        click(&mut visual, "media-close");

        // Two files, one a recording: Convert leaves it out and says so; the run gets the video only.
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip (converted).mp4"])));
        let _view = open_view(
            &mut visual,
            &["C:\\a\\clip.mp4", "C:\\a\\talk.wav"],
            MediaMode::Convert,
            &fake,
        );
        assert_eq!(
            role_of(&mut visual, "1 file without a video will be left out."),
            Some(Role::Status)
        );
        assert!(has(&mut visual, "Media: 2 files"));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(fake.calls()[0].0, ["C:\\a\\clip.mp4"]);
        assert_eq!(action_run::folders_of(&strings(&["C:\\a\\x", "C:\\A\\y"])).len(), 1);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn subtitles_wait_for_the_speech_model_which_the_card_downloads(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&["C:\\a\\clip.srt"])));
        if let Ok(mut there) = fake.speech.lock() {
            *there = false;
        }
        let _view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Subtitles, &fake);
        settle(&mut visual, |cx| {
            has(cx, "Subtitles need whisper.cpp and its speech model (60 MB).")
        });
        assert_eq!(
            role_of(&mut visual, "Subtitles need whisper.cpp and its speech model (60 MB)."),
            Some(Role::Group)
        );
        assert!(disabled(&mut visual, "media-start"));
        click(&mut visual, "media-speech-download");
        settle(&mut visual, |cx| {
            !has(cx, "Subtitles need whisper.cpp and its speech model (60 MB).")
        });
        assert_eq!(fake.downloads.lock().map(|n| *n).unwrap_or(0), 1);
        assert!(!disabled(&mut visual, "media-start"));
        click(&mut visual, ("media-language", 1_usize));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(fake.calls()[0].1, Task::Subtitles { translate: true });
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn progress_shows_while_it_runs_stop_cancels_and_a_stopped_run_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (release, held) = async_channel::bounded(1);
        let mut fake = Fake::new(Report {
            stopped: true,
            ..Default::default()
        });
        fake.release = Some(held);
        let fake = Arc::new(fake);
        let _view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Volume, &fake);
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Working on · clip.mp4 · 50%"));
        assert_eq!(
            role_of(&mut visual, "Working on · clip.mp4 · 50%"),
            Some(Role::ProgressIndicator)
        );
        assert!(disabled(&mut visual, "media-cancel"));
        click(&mut visual, "media-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        // Core ends the run and says it was stopped.
        release.try_send(()).ok();
        settle(&mut visual, |cx| has(cx, "Nothing was made before it was stopped"));
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn start_waits_for_ffmpeg_and_a_failed_run_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(&[])));
        let view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        // Before the tools are known (or with FFmpeg missing) nothing starts.
        view.update(&mut visual, |v, cx| {
            v.gate = Tooling::never();
            cx.notify();
        });
        assert!(disabled(&mut visual, "media-start"));
        visual.update(|window, cx| view.update(cx, |v, cx| v.start(window, cx)));
        assert!(fake.calls().is_empty());
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();

        let mut broken = Fake::new(report(&[]));
        broken.error = Some("FFmpeg isn't installed".into());
        let broken = Arc::new(broken);
        let _view = open_view(&mut visual, &["C:\\a\\other.mp4"], MediaMode::Convert, &broken);
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Couldn't make it. FFmpeg isn't installed"));
        assert_eq!(
            role_of(&mut visual, "Couldn't make it. FFmpeg isn't installed"),
            Some(Role::Alert)
        );
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_work_through_a_run(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let mut partial = report(&["C:\\a\\تقرير (converted).mp4"]);
        partial.failed = (0..8)
            .map(|i| (format!("C:\\a\\ملف{i}.mp4"), "Couldn't read it".to_string()))
            .collect();
        let fake = Arc::new(Fake::new(partial));
        let _view = open_view(&mut visual, &["C:\\a\\تقرير.mp4"], MediaMode::Convert, &fake);
        // The accessible names carry the file name as it is, without direction marks.
        assert_eq!(role_of(&mut visual, "Media: تقرير.mp4"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\a"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "What to make"), Some(Role::TabList));
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(role_of(&mut visual, "8 files not done:"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "ملف0.mp4: Couldn't read it"), Some(Role::ListItem));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find_all_by_role(Role::ListItem).len(),
                6,
                "six shown, the rest counted"
            );
        });
        testing::set_rtl(false);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
    }

    #[gpui_kit::test]
    fn what_a_run_made_can_be_taken_back_with_the_files_it_replaced(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        if let Ok(mut last) = LAST.lock() {
            *last = None;
        }
        cx.update(|cx| crate::explorer::init_with(cx, crate::explorer::fake::FakeDisk::new().source()));
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let mut done = report(&["C:\\a\\clip.mp4"]);
        done.recycled = vec![Recycled {
            path: "C:\\a\\clip.mp4".into(),
            bin: "C:\\$Recycle.Bin\\S-1\\$R1.mp4".into(),
        }];
        let fake = Arc::new(Fake::new(done));
        let _view = open_view(&mut visual, &["C:\\a\\clip.mp4"], MediaMode::Convert, &fake);
        click(&mut visual, "media-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let explorer = visual.update(|_, cx| crate::explorer::explorer(cx));
        // One step: what was made recycled and the original restored together.
        let label = explorer.read_with(&visual, |e, _| e.next_undo().map(|(_, label)| label.to_string()));
        assert_eq!(label.as_deref(), Some("conversion of 1 file"));
        visual.update(|_, cx| explorer.update(cx, |e, cx| e.undo(None, cx).detach()));
        visual.run_until_parked();
        assert!(explorer.read_with(&visual, |e, _| e.next_undo().is_none()));
    }
}
