//! Read aloud: speaking text with a voice on this PC (`speechSynthesis` in BookView.tsx).
//!
//! The web app asked WebView2's `speechSynthesis` for a local voice in the book's language and spoke one
//! utterance. Here the book view talks to a small [`Speaker`]; [`system`] is the one for Windows, which synthesizes
//! with `Windows.Media.SpeechSynthesis.SpeechSynthesizer` (the same installed voices Windows' Narrator and the
//! Settings page list, online voices not included) and plays the result with `Windows.Media.Playback.MediaPlayer`.
//!
//! Nothing here blocks the caller: [`Speaker::speak`] and [`Speaker::stop`] only send a message to a worker thread
//! that does the Windows calls. Long text is cut into pieces at paragraph and sentence ends; the next piece is
//! synthesized while the one before is playing, so the voice doesn't stop between them. When a [`Speaker`] is
//! dropped it stops.
//!
//! | TypeScript                                          | Here                                              |
//! |-----------------------------------------------------|---------------------------------------------------|
//! | `speechSynthesis.getVoices()` (local ones)          | `SpeechSynthesizer::AllVoices()`, all local       |
//! | the voice whose `lang` starts like the book's       | [`voice_index`]                                   |
//! | the first voice, when none matches                  | the system's default voice (`DefaultVoice()`)     |
//! | `text.slice(0, 32_000)`                             | [`MAX_CHARS`] in [`Speaker::speak`]'s text        |
//! | `speechSynthesis.cancel()`                          | [`Speaker::stop`]                                 |
//! | `utterance.onend`, `onerror`                        | [`Speaker::is_speaking`] turns false              |

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Foundation::TypedEventHandler;
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;
use windows::Media::SpeechSynthesis::{SpeechSynthesisStream, SpeechSynthesizer};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// The most characters spoken at once (the TypeScript cut the text at 32,000).
pub const MAX_CHARS: usize = 32_000;

/// The most characters synthesized as one piece.
const PIECE: usize = 2_500;

/// How often the worker looks for a stop, a new text, or the end of the piece being played.
const TICK: Duration = Duration::from_millis(25);

/// Something that speaks text aloud: Windows' voices in the app, a recording fake in tests.
pub trait Speaker: Send + Sync {
    /// Starts speaking `text` (what was being spoken stops), with a voice for `language` (a code like `ar` or
    /// `en-US`; empty for the system's own voice). Returns at once.
    fn speak(&self, text: &str, language: &str);
    /// Stops speaking. Returns at once; does nothing when nothing is spoken.
    fn stop(&self);
    /// Whether text is being spoken now: from [`Speaker::speak`] until the text ends, [`Speaker::stop`], or an
    /// error (no voice, no sound device).
    fn is_speaking(&self) -> bool;
}

/// The speaker for this PC: Windows' speech synthesis. Its worker thread starts with the first text.
pub fn system() -> Arc<dyn Speaker> {
    Arc::new(WindowsSpeaker::new(1.0))
}

// ---- Choosing the voice and cutting the text -------------------------------------------------------------------

/// The voice to use for a book in `wanted` (a language code), out of `voices` (their language tags, like `en-US`),
/// where `default` is the system's voice. `None` leaves the system's voice: nothing was asked for, or no voice
/// speaks the language. A voice is chosen by the language before the first dash (the TypeScript compared the first
/// two letters), the system's own voice first when it speaks it.
pub fn voice_index(voices: &[String], wanted: &str, default: Option<usize>) -> Option<usize> {
    let primary = |tag: &str| tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    let want = primary(wanted.trim());
    if want.is_empty() {
        return None;
    }
    let wanted_full = wanted.trim().replace('_', "-").to_ascii_lowercase();
    // The exact language and region first (`pt-BR`), then the system's voice, then any voice of the language.
    if wanted_full.contains('-') {
        if let Some(ix) = voices
            .iter()
            .position(|v| v.replace('_', "-").to_ascii_lowercase() == wanted_full)
        {
            return Some(ix);
        }
    }
    if let Some(ix) = default.filter(|ix| voices.get(*ix).is_some_and(|v| primary(v) == want)) {
        return Some(ix);
    }
    voices.iter().position(|v| primary(v) == want)
}

/// `text` cut into pieces of at most `max` characters, at line ends where it can, else at the end of a sentence,
/// else at a space. Blank lines are dropped; the lines of one piece stay on separate lines.
pub fn pieces(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut length = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut rest: Vec<char> = line.chars().collect();
        // A line longer than a piece is cut first.
        while rest.len() > max {
            let cut = cut_point(&rest, max);
            let head: String = rest[..cut].iter().collect();
            let head = head.trim();
            if !head.is_empty() {
                if length + head.chars().count() + 1 > max && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                    length = 0;
                }
                if !current.is_empty() {
                    current.push('\n');
                    length += 1;
                }
                length += head.chars().count();
                current.push_str(head);
            }
            rest.drain(..cut);
            while rest.first().is_some_and(|c| c.is_whitespace()) {
                rest.remove(0);
            }
        }
        if rest.is_empty() {
            continue;
        }
        let tail: String = rest.iter().collect();
        if length + rest.len() + 1 > max && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            length = 0;
        }
        if !current.is_empty() {
            current.push('\n');
            length += 1;
        }
        length += rest.len();
        current.push_str(&tail);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Where to cut `chars` (longer than `max`) so the head is at most `max` characters: after the last sentence end in
/// the second half, else after the last space, else at `max`.
fn cut_point(chars: &[char], max: usize) -> usize {
    let window = &chars[..max.min(chars.len())];
    let half = window.len() / 2;
    let sentence = window
        .iter()
        .rposition(|c| matches!(c, '.' | '!' | '?' | '؟' | '。' | '！' | '？' | '…' | ';' | '؛'))
        .filter(|ix| *ix >= half);
    if let Some(ix) = sentence {
        return ix + 1;
    }
    match window.iter().rposition(|c| c.is_whitespace()) {
        Some(ix) if ix > 0 => ix + 1,
        _ => window.len().max(1),
    }
}

// ---- Windows ---------------------------------------------------------------------------------------------------

/// What the callers and the worker both see.
struct Shared {
    /// Whether text is being spoken: set by `speak`, cleared when the worker finishes that text, or by `stop`.
    speaking: AtomicBool,
    /// Counts the texts asked for, so a text that ended late can't clear a newer one's flag.
    generation: AtomicU64,
}

enum Command {
    Speak {
        text: String,
        language: String,
        generation: u64,
    },
    Stop,
}

/// Windows' speech synthesis, on a worker thread of its own.
pub struct WindowsSpeaker {
    shared: Arc<Shared>,
    /// Playback volume, 0 to 1 (tests listen to silence).
    volume: f64,
    /// The channel to the worker, once it has been started. Dropping it ends the worker.
    worker: Mutex<Option<Sender<Command>>>,
}

impl WindowsSpeaker {
    /// A speaker playing at `volume` (0 to 1).
    pub fn new(volume: f64) -> Self {
        Self {
            shared: Arc::new(Shared {
                speaking: AtomicBool::new(false),
                generation: AtomicU64::new(0),
            }),
            volume: volume.clamp(0., 1.),
            worker: Mutex::new(None),
        }
    }

    /// Sends a command, starting the worker if it isn't running (or has gone). False when it could not be sent.
    fn send(&self, command: Command) -> bool {
        let Ok(mut worker) = self.worker.lock() else {
            return false;
        };
        let mut command = Some(command);
        // A second try covers a worker that ended on its own (its channel is closed).
        for _ in 0..2 {
            if worker.is_none() {
                let (tx, rx) = mpsc::channel();
                let shared = self.shared.clone();
                let volume = self.volume;
                if std::thread::Builder::new()
                    .name("glint-speech".into())
                    .spawn(move || work(rx, shared, volume))
                    .is_err()
                {
                    return false;
                }
                *worker = Some(tx);
            }
            let Some(tx) = worker.as_ref() else {
                return false;
            };
            match tx.send(command.take().unwrap_or(Command::Stop)) {
                Ok(()) => return true,
                Err(mpsc::SendError(back)) => {
                    command = Some(back);
                    *worker = None;
                }
            }
        }
        false
    }
}

impl Speaker for WindowsSpeaker {
    fn speak(&self, text: &str, language: &str) {
        let text: String = text.chars().take(MAX_CHARS).collect();
        if text.trim().is_empty() {
            self.stop();
            return;
        }
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.speaking.store(true, Ordering::SeqCst);
        let sent = self.send(Command::Speak {
            text,
            language: language.to_string(),
            generation,
        });
        if !sent {
            self.shared.speaking.store(false, Ordering::SeqCst);
        }
    }

    fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.speaking.store(false, Ordering::SeqCst);
        // Nothing to stop when the worker was never started.
        let started = self.worker.lock().is_ok_and(|w| w.is_some());
        if started {
            self.send(Command::Stop);
        }
    }

    fn is_speaking(&self) -> bool {
        self.shared.speaking.load(Ordering::SeqCst)
    }
}

impl Drop for WindowsSpeaker {
    fn drop(&mut self) {
        // The worker sees its channel close, silences what plays, and ends.
        self.shared.speaking.store(false, Ordering::SeqCst);
        if let Ok(mut worker) = self.worker.lock() {
            worker.take();
        }
    }
}

/// A piece being played: the player, and whether it has ended (or failed).
struct Playing {
    player: MediaPlayer,
    ended: Arc<AtomicBool>,
}

impl Drop for Playing {
    fn drop(&mut self) {
        self.player.Pause().ok();
        self.player.Close().ok();
    }
}

/// One text being spoken.
struct Job {
    generation: u64,
    synth: SpeechSynthesizer,
    volume: f64,
    /// The pieces not yet sent to the synthesizer.
    queue: VecDeque<String>,
    /// The piece being synthesized: its sound arrives here (a helper thread waits for the synthesizer).
    pending: Option<mpsc::Receiver<windows::core::Result<SpeechSynthesisStream>>>,
    playing: Option<Playing>,
}

impl Job {
    /// Gets the synthesizer ready with the voice for `language`.
    fn start(text: &str, language: &str, generation: u64, volume: f64) -> windows::core::Result<Self> {
        let synth = SpeechSynthesizer::new()?;
        if let Ok(voices) = SpeechSynthesizer::AllVoices() {
            let size = voices.Size().unwrap_or(0);
            let list: Vec<_> = (0..size).filter_map(|i| voices.GetAt(i).ok()).collect();
            let tags: Vec<String> = list
                .iter()
                .map(|v| v.Language().map(|l| l.to_string()).unwrap_or_default())
                .collect();
            let default = SpeechSynthesizer::DefaultVoice()
                .and_then(|d| d.Id())
                .ok()
                .and_then(|id| list.iter().position(|v| v.Id().is_ok_and(|other| other == id)));
            if let Some(voice) = voice_index(&tags, language, default).and_then(|ix| list.get(ix)) {
                synth.SetVoice(voice).ok();
            }
        }
        Ok(Self {
            generation,
            synth,
            volume,
            queue: pieces(text, PIECE).into(),
            pending: None,
            playing: None,
        })
    }

    /// Starts synthesizing the next piece, if there is one.
    fn synthesize_next(&mut self) -> windows::core::Result<()> {
        if let Some(piece) = self.queue.pop_front() {
            let op = self.synth.SynthesizeTextToStreamAsync(&HSTRING::from(piece))?;
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("glint-speech-piece".into())
                .spawn(move || {
                    // SAFETY: plain COM initialisation of this short-lived thread; it ends with the thread.
                    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
                    tx.send(op.join()).ok();
                })
                .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            self.pending = Some(rx);
        }
        Ok(())
    }

    /// Plays a synthesized piece.
    fn play(&mut self, stream: &SpeechSynthesisStream) -> windows::core::Result<()> {
        let source = MediaSource::CreateFromStream(stream, &stream.ContentType()?)?;
        let player = MediaPlayer::new()?;
        player.SetVolume(self.volume).ok();
        player.SetSource(&source)?;
        let ended = Arc::new(AtomicBool::new(false));
        let on_end = ended.clone();
        player.MediaEnded(&TypedEventHandler::new(move |_, _| {
            on_end.store(true, Ordering::SeqCst);
            Ok(())
        }))?;
        let on_fail = ended.clone();
        player.MediaFailed(&TypedEventHandler::new(move |_, _| {
            on_fail.store(true, Ordering::SeqCst);
            Ok(())
        }))?;
        player.Play()?;
        self.playing = Some(Playing { player, ended });
        Ok(())
    }

    /// One look at how the job stands. Returns true when it is over (all spoken, or something went wrong).
    fn tick(&mut self) -> bool {
        if let Some(playing) = &self.playing {
            if !playing.ended.load(Ordering::SeqCst) {
                return false;
            }
            self.playing = None;
        }
        if self.pending.is_none() && self.synthesize_next().is_err() {
            return true;
        }
        let Some(pending) = &self.pending else {
            // Nothing playing, nothing coming: everything has been said.
            return true;
        };
        let stream = match pending.try_recv() {
            Ok(stream) => stream,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        };
        self.pending = None;
        let Ok(stream) = stream else {
            return true;
        };
        if self.play(&stream).is_err() {
            return true;
        }
        // While this piece plays, the next one is made.
        self.synthesize_next().is_err()
    }
}

/// The worker thread: takes commands, and steps the job being spoken.
fn work(commands: mpsc::Receiver<Command>, shared: Arc<Shared>, volume: f64) {
    // SAFETY: plain COM initialisation of this thread; undone below only when it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let mut job: Option<Job> = None;
    loop {
        let command = if job.is_some() {
            match commands.recv_timeout(TICK) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        match command {
            Some(Command::Speak {
                text,
                language,
                generation,
            }) => {
                // What was playing is let go of (and goes quiet) before the new text starts.
                job = None;
                match Job::start(&text, &language, generation, volume) {
                    Ok(started) => job = Some(started),
                    Err(_) => finish(&shared, generation),
                }
            }
            Some(Command::Stop) => job = None,
            None => {}
        }
        if let Some(running) = &mut job {
            if running.tick() {
                let generation = running.generation;
                job = None;
                finish(&shared, generation);
            }
        }
    }
    drop(job);
    if com {
        // SAFETY: balances the successful CoInitializeEx above, on the same thread.
        unsafe { CoUninitialize() };
    }
}

/// A text has ended: it isn't being spoken any more, unless a newer one was asked for meanwhile.
fn finish(shared: &Shared, generation: u64) {
    if shared.generation.load(Ordering::SeqCst) == generation {
        shared.speaking.store(false, Ordering::SeqCst);
    }
}

// ---- A speaker for tests ---------------------------------------------------------------------------------------

/// A speaker that makes no sound and records what it was asked: for the tests of whatever has a Read aloud.
#[cfg(test)]
#[derive(Default)]
pub struct FakeSpeaker {
    state: Mutex<FakeState>,
}

#[cfg(test)]
#[derive(Default)]
struct FakeState {
    spoken: Vec<(String, String)>,
    stops: usize,
    speaking: bool,
}

#[cfg(test)]
impl FakeSpeaker {
    /// A speaker that has not been asked anything.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Every text asked for, with its language, in order.
    pub fn spoken(&self) -> Vec<(String, String)> {
        self.state.lock().map(|s| s.spoken.clone()).unwrap_or_default()
    }

    /// How many times it was told to stop.
    pub fn stops(&self) -> usize {
        self.state.lock().map(|s| s.stops).unwrap_or(0)
    }

    /// The voice reaches the end of the text.
    pub fn finish(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.speaking = false;
        }
    }
}

#[cfg(test)]
impl Speaker for FakeSpeaker {
    fn speak(&self, text: &str, language: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.spoken.push((text.to_string(), language.to_string()));
            state.speaking = true;
        }
    }

    fn stop(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stops += 1;
            state.speaking = false;
        }
    }

    fn is_speaking(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.speaking)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_voice_is_chosen_by_the_books_language() {
        let voices = tags(&["en-US", "en-GB", "ar-SA", "pt-PT", "pt-BR"]);
        // Nothing asked for: the system's voice stays.
        assert_eq!(voice_index(&voices, "", Some(0)), None);
        assert_eq!(voice_index(&voices, "  ", Some(0)), None);
        // The language, whatever the region.
        assert_eq!(voice_index(&voices, "ar", Some(0)), Some(2));
        assert_eq!(voice_index(&voices, "AR-EG", Some(0)), Some(2));
        // The system's own voice when it speaks the language.
        assert_eq!(voice_index(&voices, "en", Some(1)), Some(1));
        assert_eq!(voice_index(&voices, "en", Some(2)), Some(0));
        // The exact region when there is a voice for it.
        assert_eq!(voice_index(&voices, "pt-BR", Some(0)), Some(4));
        assert_eq!(voice_index(&voices, "pt_br", None), Some(4));
        assert_eq!(voice_index(&voices, "pt", None), Some(3));
        // A language no voice speaks leaves the system's voice.
        assert_eq!(voice_index(&voices, "ja", Some(0)), None);
        assert_eq!(voice_index(&[], "en", None), None);
    }

    #[test]
    fn text_is_cut_at_lines_then_sentences_then_spaces() {
        // Short text stays in one piece, blank lines dropped, lines kept apart.
        assert_eq!(
            pieces("One.\n\n  Two.  \nمرحبا", 100),
            vec!["One.\nTwo.\nمرحبا".to_string()]
        );
        assert!(pieces("  \n \n", 100).is_empty());
        // Lines are not split across pieces when they fit.
        assert_eq!(
            pieces("aaaa\nbbbb\ncccc", 9),
            vec!["aaaa\nbbbb".to_string(), "cccc".to_string()]
        );
        // A long line is cut after a sentence end.
        let long = format!("{} {}", "a".repeat(30) + ".", "b".repeat(30));
        let cut = pieces(&long, 40);
        assert_eq!(
            cut.first().map(String::as_str),
            Some(format!("{}.", "a".repeat(30)).as_str())
        );
        assert!(cut.iter().all(|p| p.chars().count() <= 40));
        // Else at a space; else wherever it must be, on a character boundary.
        let spaced = pieces("alpha beta gamma delta", 11);
        assert_eq!(spaced, vec!["alpha beta".to_string(), "gamma delta".to_string()]);
        let solid = "ي".repeat(25);
        let cut = pieces(&solid, 10);
        assert_eq!(cut.len(), 3);
        assert!(cut.iter().all(|p| p.chars().count() <= 10));
        assert_eq!(cut.concat(), solid);
        // Nothing is lost: every word comes back, in order.
        let words: String = (0..400).map(|i| format!("word{i} ")).collect();
        let cut = pieces(&words, 120);
        assert!(cut.iter().all(|p| p.chars().count() <= 120));
        assert_eq!(
            cut.join(" ").split_whitespace().collect::<Vec<_>>(),
            words.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_fake_records_and_the_trait_object_works() {
        let fake = FakeSpeaker::new();
        let speaker: Arc<dyn Speaker> = fake.clone();
        assert!(!speaker.is_speaking());
        speaker.speak("hello", "en");
        assert!(speaker.is_speaking());
        assert_eq!(fake.spoken(), vec![("hello".to_string(), "en".to_string())]);
        fake.finish();
        assert!(!speaker.is_speaking());
        speaker.speak("again", "");
        speaker.stop();
        assert!(!speaker.is_speaking());
        assert_eq!(fake.stops(), 1);
    }

    #[test]
    fn a_windows_speaker_that_is_stopped_or_given_nothing_never_starts_a_thread() {
        let speaker = WindowsSpeaker::new(0.);
        speaker.stop();
        speaker.speak("   \n ", "en");
        assert!(!speaker.is_speaking());
        assert!(speaker.worker.lock().is_ok_and(|w| w.is_none()));
    }

    /// Needs a voice on this PC: speaks a short text at volume 0 (silent) and waits for the end, which goes through
    /// the synthesizer, the player and the ended event. `cargo test -p glint-ui -- --ignored silent_speech`.
    #[test]
    #[ignore = "uses this PC's speech voices"]
    fn silent_speech_reaches_its_end() {
        let speaker = WindowsSpeaker::new(0.);
        speaker.speak("Hello. This is a test of reading aloud.", "en");
        assert!(speaker.is_speaking());
        let start = std::time::Instant::now();
        while speaker.is_speaking() && start.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!speaker.is_speaking(), "the speech never ended");
        // Stopping part way works, and a new text can follow.
        speaker.speak(&"A long sentence to say. ".repeat(40), "");
        std::thread::sleep(Duration::from_millis(300));
        speaker.stop();
        assert!(!speaker.is_speaking());
    }
}
