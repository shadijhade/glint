//! A fake engine and fake sources for the player's tests: what was asked of the engine is recorded, and the test
//! says what the engine answers.

use super::engine::{Command, Engine, Event, Facts, Frame, Kind, Opened};
use super::scrub::Frames;
use super::sources::PlayerSources;
use crate::components::preview::sources::Fut;
use gpui_kit::RenderImage;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// What the tests see of the engine the view opened.
#[derive(Default)]
pub struct Opening {
    /// Every command sent, in order.
    pub commands: Mutex<Vec<Command>>,
    /// The picture [`Engine::take_frame`] gives next.
    pub frame: Mutex<Option<Frame>>,
    /// Where the test speaks for the engine.
    pub events: RefCell<Option<async_channel::Sender<Event>>>,
    /// The path and kind it was opened with, one entry for each open.
    pub opened: RefCell<Vec<(String, Kind)>>,
    /// Set when the view let the engine go.
    pub dropped: Mutex<bool>,
}

impl Opening {
    /// Speaks for the engine.
    pub fn say(&self, event: Event) {
        if let Some(events) = self.events.borrow().as_ref() {
            events.try_send(event).ok();
        }
    }

    /// Hands the view a picture and says there is one.
    pub fn show(&self, image: Arc<RenderImage>, width: u32, height: u32) {
        if let Ok(mut slot) = self.frame.lock() {
            *slot = Some(Frame { image, width, height });
        }
        self.say(Event::Frame);
    }

    /// The commands sent so far.
    pub fn sent(&self) -> Vec<Command> {
        self.commands.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Whether this command was ever sent.
    pub fn was_sent(&self, command: &Command) -> bool {
        self.sent().contains(command)
    }

    /// The last command of a kind, tested by `pick`.
    pub fn last<T>(&self, pick: impl Fn(&Command) -> Option<T>) -> Option<T> {
        self.sent().iter().rev().find_map(pick)
    }

    /// Whether the engine has been let go of.
    pub fn is_dropped(&self) -> bool {
        self.dropped.lock().map(|d| *d).unwrap_or(false)
    }
}

struct Fake {
    opening: Rc<Opening>,
}

impl Engine for Fake {
    fn send(&self, command: Command) {
        if let Ok(mut sent) = self.opening.commands.lock() {
            sent.push(command);
        }
    }

    fn take_frame(&self) -> Option<Frame> {
        self.opening.frame.lock().ok().and_then(|mut f| f.take())
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        if let Ok(mut dropped) = self.opening.dropped.lock() {
            *dropped = true;
        }
    }
}

/// A picture of one colour, for a frame.
pub fn picture(width: u32, height: u32, bgra: [u8; 4]) -> Arc<RenderImage> {
    let bytes: Vec<u8> = bgra
        .iter()
        .copied()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect();
    super::mf::bgra_image(width, height, bytes).expect("a picture")
}

/// A song's facts.
pub fn song(duration: f64) -> Facts {
    Facts {
        duration,
        width: 0,
        height: 0,
        has_video: false,
        has_audio: true,
    }
}

/// A video's facts.
pub fn film(duration: f64, width: u32, height: u32) -> Facts {
    Facts {
        duration,
        width,
        height,
        has_video: true,
        has_audio: true,
    }
}

/// What the fake sources answer.
#[derive(Default)]
pub struct Answers {
    /// The waveform's bars.
    pub peaks: RefCell<Option<Vec<f32>>>,
    /// A song's cover.
    pub cover: RefCell<Option<Arc<RenderImage>>>,
    /// The temp copy an archived file is extracted to.
    pub extracted: RefCell<Option<Result<String, String>>>,
    /// The frames a tile's scrub reads.
    pub frames: RefCell<Option<Arc<Frames>>>,
}

/// Fake sources that open [`Opening`]s and answer from [`Answers`].
pub fn sources(opening: Rc<Opening>, answers: Rc<Answers>) -> PlayerSources {
    let open_in = opening.clone();
    let (peaks, cover, extract, frames) = (answers.clone(), answers.clone(), answers.clone(), answers);
    PlayerSources {
        open: Rc::new(move |path, kind| {
            open_in.opened.borrow_mut().push((path.to_string(), kind));
            let (sender, events) = async_channel::unbounded();
            *open_in.events.borrow_mut() = Some(sender);
            if let Ok(mut dropped) = open_in.dropped.lock() {
                *dropped = false;
            }
            Opened {
                engine: Box::new(Fake {
                    opening: open_in.clone(),
                }),
                events,
            }
        }),
        peaks: Rc::new(move |_| {
            let bars = peaks.peaks.borrow().clone();
            Box::pin(async move { bars })
        }),
        cover: Rc::new(move |_| {
            let cover = cover.cover.borrow().clone();
            Box::pin(async move { cover })
        }),
        extract: Rc::new(move |_| -> Fut<Result<String, String>> {
            let made = extract
                .extracted
                .borrow()
                .clone()
                .unwrap_or_else(|| Err("no extractor".into()));
            Box::pin(async move { made })
        }),
        frames: Rc::new(move |_, _| {
            let read = frames.frames.borrow().clone();
            Box::pin(async move { read })
        }),
    }
}
