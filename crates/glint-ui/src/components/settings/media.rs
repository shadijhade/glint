//! Settings → Photos and recordings: photo search (`shows:`, CLIP sees what is in pictures) and speech search
//! (`says:`, Whisper hears what is said), each on the folders chosen here, on this PC alone
//! (`MediaSearchSection` of SettingsView.tsx).

use super::backend::{Backend, Fetch, Known};
use super::widgets::{self, bad, good};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::media_ai::{self, Config, Progress};
use glint_model::format::format_count;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// How often the models' progress is asked for (the TypeScript's 2 s).
const POLL: Duration = Duration::from_secs(2);

/// Which of the two searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Photos,
    Speech,
}

impl Kind {
    const BOTH: [Kind; 2] = [Kind::Photos, Kind::Speech];

    /// The word core's install and the element names use.
    fn word(self) -> &'static str {
        match self {
            Kind::Photos => "photos",
            Kind::Speech => "speech",
        }
    }
}

/// What core said, kept (core's own type can't be cloned).
#[derive(Clone, Debug, Default)]
pub struct State {
    pub config: Config,
    pub photos_installed: bool,
    pub speech_installed: bool,
    pub photos: Progress,
    pub speech: Progress,
}

impl From<media_ai::Status> for State {
    fn from(status: media_ai::Status) -> Self {
        State {
            config: status.config,
            photos_installed: status.photos_installed,
            speech_installed: status.speech_installed,
            photos: status.photos,
            speech: status.speech,
        }
    }
}

impl State {
    fn on(&self, kind: Kind) -> bool {
        match kind {
            Kind::Photos => self.config.photos,
            Kind::Speech => self.config.speech,
        }
    }

    fn folders(&self, kind: Kind) -> &Vec<String> {
        match kind {
            Kind::Photos => &self.config.photo_folders,
            Kind::Speech => &self.config.speech_folders,
        }
    }

    fn installed(&self, kind: Kind) -> bool {
        match kind {
            Kind::Photos => self.photos_installed,
            Kind::Speech => self.speech_installed,
        }
    }

    fn progress(&self, kind: Kind) -> &Progress {
        match kind {
            Kind::Photos => &self.photos,
            Kind::Speech => &self.speech,
        }
    }

    /// The config with `kind` switched and given `folders` (`side.set`).
    fn with(&self, kind: Kind, on: bool, folders: Vec<String>) -> Config {
        let mut config = self.config.clone();
        match kind {
            Kind::Photos => {
                config.photos = on;
                config.photo_folders = folders;
            }
            Kind::Speech => {
                config.speech = on;
                config.speech_folders = folders;
            }
        }
        config
    }
}

/// The photos and recordings card. Draws nothing until core has answered once.
pub struct MediaSearch {
    backend: Arc<dyn Backend>,
    status: Option<State>,
    /// The downloads under way, as far as they have got.
    fetching: HashMap<Kind, Fetch>,
    _poll: Task<()>,
}

impl MediaSearch {
    /// Asks how the models are, and keeps asking while Settings is open.
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let asking = backend.clone();
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let answer = asking.media_status().await;
            let alive = this
                .update(cx, |this, cx| {
                    if let Ok(status) = answer {
                        this.status = Some(status.into());
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
            cx.background_executor().timer(POLL).await;
        });
        Self {
            backend,
            status: None,
            fetching: HashMap::new(),
            _poll: poll,
        }
    }

    /// The last answer.
    pub fn status(&self) -> Option<&State> {
        self.status.as_ref()
    }

    /// Whether `kind` is downloading its model.
    pub fn fetching(&self, kind: Kind) -> Option<&Fetch> {
        self.fetching.get(&kind)
    }

    fn configure(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.backend.media_configure(config);
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(status) => this.status = Some(status.into()),
                    Err(reason) => bad(window, cx, t!("Couldn't change photo and speech search"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Downloads what `kind` needs, showing how far it has got (`install`).
    pub fn install(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.fetching.insert(
            kind,
            Fetch {
                stage: t!("Starting").to_string(),
                done: 0,
                total: 0,
            },
        );
        cx.notify();
        let (progress, heard) = async_channel::unbounded();
        let asked = self.backend.media_install(kind.word(), progress);
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(fetch) = heard.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.fetching.insert(kind, fetch);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await;
            this.update_in(cx, |this, window, cx| {
                this.fetching.remove(&kind);
                match answer {
                    Ok(status) => {
                        this.status = Some(status.into());
                        good(
                            window,
                            cx,
                            match kind {
                                Kind::Photos => t!("Photo search is ready: it reads the photos in the background"),
                                Kind::Speech => {
                                    t!("Speech search is ready: it listens to the recordings in the background")
                                }
                            },
                            None,
                        );
                    }
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => bad(window, cx, t!("Couldn't download it"), reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Turns a search on or off; turning it on with no folders chosen picks the usual ones, and downloads its
    /// model if it hasn't one.
    pub fn toggle(&mut self, kind: Kind, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = self.status.clone() else { return };
        let mut folders = status.folders(kind).clone();
        if on && folders.is_empty() {
            let usual: &[Known] = match kind {
                Kind::Photos => &[Known::Pictures],
                Kind::Speech => &[Known::Music, Known::Videos],
            };
            folders = usual.iter().filter_map(|k| self.backend.known_folder(*k)).collect();
        }
        let config = status.with(kind, on, folders);
        // The model comes after the choice is saved, as the person asked for the search.
        let wants_model = on && !status.installed(kind) && !self.fetching.contains_key(&kind);
        self.configure(config, window, cx);
        if wants_model {
            self.install(kind, window, cx);
        }
    }

    fn add_folder(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let title = match kind {
            Kind::Photos => t!("A folder of photos"),
            Kind::Speech => t!("A folder of recordings or videos"),
        };
        let picked = self.backend.pick_folder(title.to_string(), None, owner);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(folder)) = picked.await else { return };
            this.update_in(cx, |this, window, cx| {
                let Some(status) = this.status.clone() else { return };
                if !status.folders(kind).contains(&folder) {
                    let mut folders = status.folders(kind).clone();
                    folders.push(folder);
                    let config = status.with(kind, true, folders);
                    this.configure(config, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_folder(&mut self, kind: Kind, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = self.status.clone() else { return };
        let folders = status.folders(kind).iter().filter(|f| *f != folder).cloned().collect();
        let config = status.with(kind, true, folders);
        self.configure(config, window, cx);
    }

    /// What a side says under its name (the TypeScript's `hint`).
    fn hint(&self, kind: Kind, status: &State) -> SharedString {
        let progress = status.progress(kind);
        let count = |n: usize| format_count(n);
        if !status.on(kind) {
            return match kind {
                Kind::Photos => t!("Finds pictures by what's in them: type shows:dog on a beach, shows:receipt or shows:whiteboard. OpenAI's CLIP with ONNX Runtime (about 240 MB to download, MIT)."),
                Kind::Speech => t!("Finds recordings and videos by what's said in them, in any language: type says:budget meeting. whisper.cpp with Whisper's base model (about 70 MB to download, MIT); FFmpeg, if installed, reads videos and every format."),
            };
        }
        if let Some(busy) = self.fetching.get(&kind) {
            return if busy.total != 0 {
                let percent = (busy.done as f64 / busy.total as f64 * 100.).round();
                t!("{stage}: {percent}%", stage = busy.stage, percent = percent)
            } else {
                t!("{stage}…", stage = busy.stage)
            };
        }
        if !status.installed(kind) {
            return t!("Its model isn't downloaded yet.");
        }
        if !progress.error.is_empty() {
            return t!("Stopped: {error}", error = progress.error);
        }
        match (kind, progress.working) {
            (Kind::Photos, true) => t!(
                "{known} photos read, {pending} to go…",
                known = count(progress.known),
                pending = count(progress.pending)
            ),
            (Kind::Speech, true) => t!(
                "{known} recordings heard, {pending} to go…",
                known = count(progress.known),
                pending = count(progress.pending)
            ),
            (Kind::Photos, false) => t!(
                "{known} photos read. New ones are read every two hours, and when the folders change here.",
                known = count(progress.known)
            ),
            (Kind::Speech, false) => t!(
                "{known} recordings heard. New ones are read every two hours, and when the folders change here.",
                known = count(progress.known)
            ),
        }
    }
}

impl Render for MediaSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let Some(status) = self.status.clone() else {
            return div().into_any_element();
        };
        let mut rows: Vec<AnyElement> = vec![widgets::note(
            t!("Both run on this PC, at low priority: no picture, recording or word typed is sent anywhere. What they learn is kept in Glint's data folder, under each file's ID, so moving or renaming a file keeps it."),
            &t,
        )
        .into_any_element()];
        for kind in Kind::BOTH {
            let word = kind.word();
            let on = status.on(kind);
            let busy = self.fetching.contains_key(&kind);
            let label = match kind {
                Kind::Photos => t!("Photo search (shows:)"),
                Kind::Speech => t!("Speech search (says:)"),
            };
            let mut side = col().child(widgets::switch_row(
                word,
                label,
                Some(self.hint(kind, &status)),
                on,
                false,
                widgets::toggler(cx, move |this, on, window, cx| this.toggle(kind, on, window, cx)),
                &t,
            ));
            if on {
                side = side.child(
                    col()
                        .gap(space(1.))
                        .px(space(4.))
                        .pb(space(3.))
                        .children(status.folders(kind).iter().enumerate().map(|(i, folder)| {
                            let removed = folder.clone();
                            widgets::path_line(
                                word,
                                i,
                                folder.clone(),
                                widgets::quiet_button(&format!("{word}-folder-{i}"), t!("Remove"))
                                    .accessibility_label(t!("Remove {name}", name = folder))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_folder(kind, &removed, window, cx)
                                    })),
                            )
                        }))
                        .child(
                            row_start()
                                .gap(space(2.))
                                .child(
                                    widgets::button(&format!("{word}-add"), t!("Add a folder"), Some(IconName::Plus))
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| this.add_folder(kind, window, cx)),
                                        ),
                                )
                                .when(!status.installed(kind) && !busy, |this| {
                                    this.child(
                                        widgets::button(
                                            &format!("{word}-download"),
                                            t!("Download"),
                                            Some(IconName::Download),
                                        )
                                        .tooltip(t!("Downloads it and checks it against its pinned checksums"))
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| this.install(kind, window, cx)),
                                        ),
                                    )
                                })
                                .when(busy, |this| {
                                    let backend = self.backend.clone();
                                    this.child(widgets::quiet_button(&format!("{word}-stop"), t!("Stop")).on_click(
                                        move |_, _, _| {
                                            for id in ["photos", "speech", "whisper", "onnxruntime"] {
                                                backend.cancel_install(id.to_string());
                                            }
                                        },
                                    ))
                                }),
                        ),
                );
            }
            rows.push(side.into_any_element());
        }
        widgets::section("photos-and-recordings", t!("Photos and recordings"), None, rows, &t).into_any_element()
    }
}
