//! Instant search's state for the status bar: whether its service runs, which drives it is reading, and turning it
//! on. Ported from the index parts of src/lib/explorer.ts (`refreshIndex`, `indexed`, `enableIndex`, `turnOnIndex`)
//! and the `IndexInfo` of src/lib/fs.ts.
//!
//! The explorer doesn't hold the index (it isn't a folder), so this model does: it asks glint-core in the background,
//! asks again every second while a drive is being read (every five seconds on an error), and keeps the last answer
//! when a question fails. Tests give it a [`Source`] of their own.

use crate::core_bridge;
use crate::t;
use gpui_kit::{App, Context, Global, Task};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How long to wait before asking again while a drive is being read (the TypeScript's one second).
const SCANNING_POLL: Duration = Duration::from_secs(1);
/// How long to wait before asking again after an error on a drive (the TypeScript's five seconds).
const ERROR_POLL: Duration = Duration::from_secs(5);

/// Where the service stands (`IndexInfo.state` in fs.ts). Anything the status bar doesn't offer a remedy for is
/// `Other`: the bar only says whether it can be turned on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Running, and answering.
    On,
    /// Not installed: turning it on installs it.
    Off,
    /// Installed but not running: turning it on starts it.
    Stopped,
    /// Turned on for another user of this PC.
    NoAccess,
    /// Something else holds the service's pipe.
    Untrusted,
    /// Not Windows, or no NTFS to read.
    Unsupported,
}

impl State {
    /// Core's word for it ("on", "off", "stopped"…); an unknown word is `Unsupported`.
    pub fn parse(word: &str) -> State {
        match word {
            "on" => State::On,
            "off" => State::Off,
            "stopped" => State::Stopped,
            "noAccess" => State::NoAccess,
            "untrusted" => State::Untrusted,
            _ => State::Unsupported,
        }
    }
}

/// One drive the index covers, as the service reports it (`VolumeStatus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The drive, e.g. "C:".
    pub volume: String,
    /// The service is reading the drive (its state is "scanning").
    pub scanning: bool,
    /// The service couldn't index the drive.
    pub error: bool,
    /// Records read so far while scanning, of `progress_total`.
    pub progress: u64,
    pub progress_total: u64,
}

impl Volume {
    /// How far the scan is, in whole percent (`Math.floor(100 * progress / progressTotal)`), when the total is known.
    pub fn percent(&self) -> Option<u64> {
        if self.progress_total == 0 {
            return None;
        }
        // Done in 128 bits: the product of two 64-bit counts doesn't fit in 64.
        let whole = u128::from(self.progress) * 100 / u128::from(self.progress_total);
        Some(u64::try_from(whole).unwrap_or(u64::MAX))
    }
}

/// What the service said when it was last asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub state: State,
    /// The running service's version, when it runs.
    pub version: Option<String>,
    pub volumes: Vec<Volume>,
    pub error: Option<String>,
}

impl Index {
    /// Whether the status bar offers to turn instant search on (`turnOn` in StatusBar.tsx): it isn't running.
    pub fn can_turn_on(&self) -> bool {
        matches!(self.state, State::Off | State::Stopped)
    }

    /// The first drive being read, if one is (`scanning` in StatusBar.tsx).
    pub fn scanning(&self) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.scanning)
    }

    /// When to ask again (`refreshIndex`): every second while a drive is read or none is listed yet, every five
    /// seconds after an error on one, and never otherwise. Only a running service is asked again.
    pub fn next_poll(&self) -> Option<Duration> {
        if self.state != State::On {
            None
        } else if self.volumes.is_empty() || self.volumes.iter().any(|v| v.scanning) {
            Some(SCANNING_POLL)
        } else if self.volumes.iter().any(|v| v.error) {
            Some(ERROR_POLL)
        } else {
            None
        }
    }
}

/// glint-core's answer, as the status bar's [`Index`].
impl From<&glint_core::index::IndexInfo> for Index {
    fn from(info: &glint_core::index::IndexInfo) -> Self {
        Index {
            state: State::parse(info.state),
            version: info.version.clone(),
            volumes: info
                .volumes
                .iter()
                .map(|v| Volume {
                    volume: v.volume.clone(),
                    scanning: v.state == "scanning",
                    error: v.state == "error",
                    progress: v.progress,
                    progress_total: v.progress_total,
                })
                .collect(),
            error: info.error.clone(),
        }
    }
}

/// A question the model asks, answered on another thread.
pub type Fut = Pin<Box<dyn Future<Output = Result<Index, String>> + Send>>;

/// Where the model's answers come from: glint-core in the app, a fake in tests.
#[derive(Clone)]
pub struct Source {
    /// Asks how the service does (`index_status`).
    pub status: Arc<dyn Fn() -> Fut + Send + Sync>,
    /// Turns it on, or starts it (`index_enable`, after Windows asks for administrator approval).
    pub enable: Arc<dyn Fn() -> Fut + Send + Sync>,
}

impl Source {
    /// glint-core's index, through the app's handle. Turning on needs this app's main window (its owner for
    /// Windows' approval prompt); without one the answer says so.
    pub fn core(app: glint_core::host::AppHandle) -> Source {
        use glint_core::host::Manager as _;
        let for_enable = app;
        Source {
            status: Arc::new(|| {
                Box::pin(async { glint_core::index::index_status().await.map(|info| Index::from(&info)) })
            }),
            enable: Arc::new(move || {
                let app = for_enable.clone();
                Box::pin(async move {
                    let window = app
                        .get_webview_window(crate::workspace::MAIN)
                        .ok_or_else(|| t!("Glint's window isn't open").to_string())?;
                    glint_core::index::index_enable(window)
                        .await
                        .map(|info| Index::from(&info))
                })
            }),
        }
    }

    /// For a status bar made before the app's index is wired: it says it can't be reached.
    pub fn unavailable() -> Source {
        let failed = || -> Fut { Box::pin(async { Err(t!("Instant search isn't available here").to_string()) }) };
        Source {
            status: Arc::new(failed),
            enable: Arc::new(failed),
        }
    }
}

/// The source the status bars use, in a GPUI global.
struct SharedSource(Source);

impl Global for SharedSource {}

/// Start-up: the status bars read instant search through glint-core. Call after `core_bridge::init`; without core
/// the bars report the index as unavailable.
pub fn init(cx: &mut App) {
    if let Some(core) = cx.try_global::<core_bridge::Core>() {
        let app = core.app.clone();
        init_with(cx, Source::core(app));
    }
}

/// [`init`] with a source of your own (tests, tools).
pub fn init_with(cx: &mut App, source: Source) {
    cx.set_global(SharedSource(source));
}

/// The source the status bars use: the one [`init`] set, or [`Source::unavailable`] before it.
pub fn shared_source(cx: &App) -> Source {
    cx.try_global::<SharedSource>()
        .map_or_else(Source::unavailable, |shared| shared.0.clone())
}

/// The index as the status bar knows it: the last answer, and the questions under way.
pub struct IndexModel {
    source: Source,
    info: Option<Index>,
    /// A question is under way; another one waits for it.
    reading: bool,
    /// The next question, when the last answer asked for one. Replacing it cancels the older timer.
    poll: Option<Task<()>>,
}

impl IndexModel {
    /// A model that reads nothing until [`IndexModel::refresh`].
    pub fn new(source: Source) -> Self {
        IndexModel {
            source,
            info: None,
            reading: false,
            poll: None,
        }
    }

    /// The last answer; `None` until the service has been asked once.
    pub fn info(&self) -> Option<&Index> {
        self.info.as_ref()
    }

    /// Asks the service how it does, in the background. A question already under way isn't repeated.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        self.reading = true;
        let question = (self.source.status)();
        cx.spawn(async move |this, cx| {
            // Two layers, as core's runs go: the task may panic (outer Err), the service may not answer (inner Err).
            let answer = core_bridge::run(question).await.and_then(|inner| inner);
            this.update(cx, |this, cx| {
                this.reading = false;
                // A failed question keeps the last answer, as the TypeScript did, and stops asking.
                let next = match answer {
                    Ok(info) => {
                        let next = info.next_poll();
                        this.info = Some(info);
                        next
                    }
                    Err(_) => None,
                };
                this.schedule(next, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Turns instant search on (or starts it). The task resolves with what went wrong, if anything; the answer
    /// replaces the last one when it works.
    pub fn enable(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let question = (self.source.enable)();
        cx.spawn(async move |this, cx| {
            let answer = core_bridge::run(question).await.and_then(|inner| inner)?;
            this.update(cx, |this, cx| {
                let next = answer.next_poll();
                this.info = Some(answer);
                this.schedule(next, cx);
                cx.notify();
            })
            .ok();
            Ok(())
        })
    }

    /// Asks again after `delay`, or not at all when there is no delay.
    fn schedule(&mut self, delay: Option<Duration>, cx: &mut Context<Self>) {
        self.poll = delay.map(|delay| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, cx| this.refresh(cx)).ok();
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(state: &str, progress: u64, total: u64) -> Volume {
        Volume {
            volume: "C:".into(),
            scanning: state == "scanning",
            error: state == "error",
            progress,
            progress_total: total,
        }
    }

    fn on(volumes: Vec<Volume>) -> Index {
        Index {
            state: State::On,
            version: Some("0.9.5".into()),
            volumes,
            error: None,
        }
    }

    #[test]
    fn core_words_map_to_states_and_only_off_or_stopped_can_be_turned_on() {
        assert_eq!(State::parse("on"), State::On);
        assert_eq!(State::parse("off"), State::Off);
        assert_eq!(State::parse("stopped"), State::Stopped);
        assert_eq!(State::parse("noAccess"), State::NoAccess);
        assert_eq!(State::parse("nonsense"), State::Unsupported);
        let offered = |state| Index {
            state,
            version: None,
            volumes: Vec::new(),
            error: None,
        };
        assert!(offered(State::Off).can_turn_on());
        assert!(offered(State::Stopped).can_turn_on());
        assert!(!offered(State::On).can_turn_on());
        assert!(!offered(State::Untrusted).can_turn_on());
    }

    #[test]
    fn progress_is_whole_percent_and_unknown_without_a_total() {
        assert_eq!(volume("scanning", 50, 100).percent(), Some(50));
        assert_eq!(volume("scanning", 2, 3).percent(), Some(66));
        assert_eq!(volume("scanning", 5, 0).percent(), None);
        assert_eq!(volume("scanning", u64::MAX, u64::MAX).percent(), Some(100));
    }

    #[test]
    fn the_scanning_drive_is_found_and_asked_about_every_second() {
        let index = on(vec![volume("ready", 0, 0), volume("scanning", 1, 4)]);
        assert_eq!(index.scanning().map(|v| v.volume.as_str()), Some("C:"));
        assert_eq!(index.next_poll(), Some(SCANNING_POLL));
        // No drive listed yet is still being read.
        assert_eq!(on(Vec::new()).next_poll(), Some(SCANNING_POLL));
    }

    #[test]
    fn an_error_is_asked_about_every_five_seconds_and_a_quiet_service_not_at_all() {
        assert_eq!(on(vec![volume("error", 0, 0)]).next_poll(), Some(ERROR_POLL));
        assert_eq!(on(vec![volume("ready", 0, 0)]).next_poll(), None);
        let stopped = Index {
            state: State::Stopped,
            version: None,
            volumes: vec![volume("scanning", 1, 2)],
            error: None,
        };
        assert_eq!(stopped.next_poll(), None, "only a running service is asked again");
    }
}
