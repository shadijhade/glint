use std::{
    sync::{
        Condvar, LazyLock, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use gpui::FrameRequestSource;
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self { interval, f }
    }

    pub(crate) fn wait_for_vsync(&self) -> FrameRequestSource {
        let vsync_start = Instant::now();
        let wait_succeeded = (self.f)();
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            std::thread::sleep(self.interval);
            FrameRequestSource::LocalSchedule
        } else {
            FrameRequestSource::NativeCallback
        }
    }
}

/// Glint: lets the vsync thread rest while nothing is drawn.
///
/// The thread invalidated every window on every tick of the display, for as long as the app ran: 144 times a
/// second here, a window on screen woken as often to find nothing to draw, and the thread itself still ticking
/// with every window hidden in the tray. After `IDLE_TICKS` ticks in which no window drew a frame, and while
/// nothing holds the ticks (a touchpad gesture, which DirectManipulation advances on each tick), it now waits
/// until it is woken: a window that has something to draw wakes it (its frame waker, called by gpui whenever
/// there is frame demand: the window dirtied, a next-frame callback, a throttled frame), and so does whatever
/// has to reach it (stopping, devices to make again). It also looks once in a while by itself (`look_after`).
#[derive(Default)]
pub(crate) struct VsyncRest {
    /// Frames drawn by every window (`WindowsWindow::draw`).
    drawn: AtomicU64,
    /// What needs the ticks though it draws nothing yet.
    held: AtomicU32,
    /// Set by `wake`, taken by `rest`: a wake while the thread ticks makes its next rest end at once.
    woken: Mutex<bool>,
    wakes: Condvar,
}

impl VsyncRest {
    /// Ticks in a row with no frame drawn before the thread rests: about 110 ms at 144 Hz, 270 ms at 60 Hz.
    pub(crate) const IDLE_TICKS: u32 = 16;

    pub(crate) fn note_draw(&self) {
        self.drawn.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn drawn(&self) -> u64 {
        self.drawn.load(Ordering::Relaxed)
    }

    pub(crate) fn wake(&self) {
        let mut woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        *woken = true;
        self.wakes.notify_one();
    }

    /// Something needs the ticks until `release`.
    pub(crate) fn hold(&self) {
        self.held.fetch_add(1, Ordering::SeqCst);
        self.wake();
    }

    pub(crate) fn release(&self) {
        let mut held = self.held.load(Ordering::SeqCst);
        while held > 0 {
            match self
                .held
                .compare_exchange_weak(held, held - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(now) => held = now,
            }
        }
    }

    pub(crate) fn held(&self) -> bool {
        self.held.load(Ordering::SeqCst) > 0
    }

    /// Waits until woken, or `look_after` passes. True when woken.
    pub(crate) fn rest(&self, look_after: Duration) -> bool {
        let woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        let (mut woken, _) = self
            .wakes
            .wait_timeout_while(woken, look_after, |woken| !*woken)
            .unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *woken, false)
    }
}

/// Glint: the display's refresh interval, asked of the compositor at most every two seconds (the rate
/// changes with the display the window is on and with battery saver). For the UI thread.
pub(crate) fn refresh_interval() -> Duration {
    thread_local! {
        static KNOWN: std::cell::Cell<Option<(Instant, Duration)>> = const { std::cell::Cell::new(None) };
    }
    KNOWN.with(|known| match known.get() {
        Some((at, interval)) if at.elapsed() < Duration::from_secs(2) => interval,
        _ => {
            let interval = get_dwm_interval().unwrap_or(DEFAULT_VSYNC_INTERVAL);
            known.set(Some((Instant::now(), interval)));
            interval
        }
    })
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_compositor_wait_reports_native_callback() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                true
            }),
        };

        assert_eq!(
            provider.wait_for_vsync(),
            FrameRequestSource::NativeCallback
        );
    }

    #[test]
    fn a_rest_ends_when_woken_or_after_its_look() {
        let rest = std::sync::Arc::new(VsyncRest::default());
        // A wake while ticking: the next rest ends at once.
        rest.wake();
        assert!(rest.rest(Duration::from_secs(5)));
        // Nothing wakes it: it looks again after a while.
        let started = Instant::now();
        assert!(!rest.rest(Duration::from_millis(30)));
        assert!(started.elapsed() >= Duration::from_millis(25));
        // Woken from another thread while it rests.
        let waker = {
            let rest = rest.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                rest.wake();
            })
        };
        let started = Instant::now();
        assert!(rest.rest(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(2));
        waker.join().unwrap();
        // Holding needs the ticks, and wakes it.
        assert!(!rest.held());
        rest.hold();
        assert!(rest.held() && rest.rest(Duration::ZERO));
        rest.release();
        rest.release();
        assert!(!rest.held(), "a release too many isn't held against it");
        let drawn = rest.drawn();
        rest.note_draw();
        assert_eq!(rest.drawn(), drawn + 1);
    }

    #[test]
    fn failed_compositor_wait_reports_local_schedule_even_after_threshold() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                false
            }),
        };

        assert_eq!(provider.wait_for_vsync(), FrameRequestSource::LocalSchedule);
    }
}
