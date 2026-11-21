//! Precision Touchpad and Windows haptic feedback via `Windows::Devices::Haptics::SimpleHapticsController`.
//!
//! Runs lazily on one bounded background thread (`sync_channel(4)`, non-blocking `try_send`) with a
//! 25 ms rate limit so rapid zooming or scrolling never blocks the UI thread or floods the touchpad.
//! Silently no-ops on non-haptic hardware and in unit tests (`cfg!(test)`).

use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MIN_INTERVAL: Duration = Duration::from_millis(25);

/// Tactile feedback cue families triggered by explorer interactions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapticCue {
    ZoomDetent,
    BumpLimit,
    BasketSnap,
    ChecksumMatch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WaveformKind {
    Click,
    Press,
    Success,
    Buzz,
}

/// Preferred WinRT waveform sequence and intensity `(0.0..=1.0)` for each cue.
pub(crate) fn cue_profile(cue: HapticCue) -> (&'static [WaveformKind], f64) {
    match cue {
        HapticCue::ZoomDetent => (&[WaveformKind::Click], 0.45),
        HapticCue::BumpLimit => (&[WaveformKind::Press, WaveformKind::Buzz, WaveformKind::Click], 0.70),
        HapticCue::BasketSnap => (&[WaveformKind::Click, WaveformKind::Press], 0.55),
        HapticCue::ChecksumMatch => (&[WaveformKind::Success, WaveformKind::Click], 0.65),
    }
}

/// Enforces the minimum 25 ms spacing between haptic cues to avoid buzzing storms.
#[inline]
pub(crate) fn should_fire(last: Option<Instant>, now: Instant) -> bool {
    match last {
        Some(prev) => now.saturating_duration_since(prev) >= MIN_INTERVAL,
        None => true,
    }
}

static LAST_FIRED: Mutex<Option<Instant>> = Mutex::new(None);
static SENDER: OnceLock<mpsc::SyncSender<HapticCue>> = OnceLock::new();

/// Fires a non-blocking subtle haptic cue if `enabled` and hardware supports it.
pub fn trigger(cue: HapticCue, enabled: bool) {
    if !enabled || cfg!(test) {
        return;
    }
    let now = Instant::now();
    {
        let mut last = LAST_FIRED.lock().unwrap_or_else(|e| e.into_inner());
        if !should_fire(*last, now) {
            return;
        }
        *last = Some(now);
    }

    #[cfg(windows)]
    {
        let tx = SENDER.get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<HapticCue>(4);
            std::thread::Builder::new()
                .name("glint-haptics".to_string())
                .spawn(move || win::worker(rx))
                .ok();
            tx
        });
        let _ = tx.try_send(cue);
    }
    #[cfg(not(windows))]
    {
        let _ = cue;
        let _ = &SENDER;
    }
}

#[cfg(windows)]
mod win {
    use super::{cue_profile, HapticCue, WaveformKind};
    use std::sync::mpsc::Receiver;
    use windows::Devices::Haptics::{
        KnownSimpleHapticsControllerWaveforms, SimpleHapticsController, SimpleHapticsControllerFeedback,
        VibrationDevice,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    fn resolve_waveform_id(kind: WaveformKind) -> Option<u16> {
        match kind {
            WaveformKind::Click => KnownSimpleHapticsControllerWaveforms::Click().ok(),
            WaveformKind::Press => KnownSimpleHapticsControllerWaveforms::Press().ok(),
            WaveformKind::Success => KnownSimpleHapticsControllerWaveforms::Success().ok(),
            WaveformKind::Buzz => KnownSimpleHapticsControllerWaveforms::BuzzContinuous()
                .or_else(|_| KnownSimpleHapticsControllerWaveforms::RumbleContinuous())
                .ok(),
        }
    }

    fn probe_controller() -> Option<SimpleHapticsController> {
        if let Ok(op) = VibrationDevice::GetDefaultAsync() {
            if let Ok(dev) = op.join() {
                if let Ok(ctrl) = dev.SimpleHapticsController() {
                    return Some(ctrl);
                }
            }
        }
        if let Ok(op) = VibrationDevice::FindAllAsync() {
            if let Ok(list) = op.join() {
                for dev in list {
                    if let Ok(ctrl) = dev.SimpleHapticsController() {
                        return Some(ctrl);
                    }
                }
            }
        }
        None
    }

    fn pick_feedback(
        supported: &[SimpleHapticsControllerFeedback],
        cue: HapticCue,
    ) -> Option<&SimpleHapticsControllerFeedback> {
        if supported.is_empty() {
            return None;
        }
        let (order, _) = cue_profile(cue);
        for &kind in order {
            let Some(wanted) = resolve_waveform_id(kind) else {
                continue;
            };
            if let Some(fb) = supported.iter().find(|f| f.Waveform().ok() == Some(wanted)) {
                return Some(fb);
            }
        }
        supported.first()
    }

    pub(super) fn worker(rx: Receiver<HapticCue>) {
        // SAFETY: Initialize WinRT MTA apartment once on this dedicated background haptics thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let controller = probe_controller();
        let supported: Vec<SimpleHapticsControllerFeedback> = controller
            .as_ref()
            .and_then(|c| c.SupportedFeedback().ok())
            .map(|view| view.into_iter().collect())
            .unwrap_or_default();

        while let Ok(cue) = rx.recv() {
            let Some(ctrl) = controller.as_ref() else {
                continue;
            };
            let Some(fb) = pick_feedback(&supported, cue) else {
                continue;
            };
            let (_, intensity) = cue_profile(cue);
            if ctrl.SendHapticFeedbackWithIntensity(fb, intensity).is_err() {
                let _ = ctrl.SendHapticFeedback(fb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_enforces_25ms_window() {
        let t0 = Instant::now();
        assert!(should_fire(None, t0));
        assert!(!should_fire(Some(t0), t0 + Duration::from_millis(10)));
        assert!(!should_fire(Some(t0), t0 + Duration::from_millis(24)));
        assert!(should_fire(Some(t0), t0 + Duration::from_millis(25)));
        assert!(should_fire(Some(t0), t0 + Duration::from_millis(100)));
    }

    #[test]
    fn disabled_trigger_and_profiles_are_clean() {
        trigger(HapticCue::ZoomDetent, false);
        trigger(HapticCue::BumpLimit, true);
        for cue in [
            HapticCue::ZoomDetent,
            HapticCue::BumpLimit,
            HapticCue::BasketSnap,
            HapticCue::ChecksumMatch,
        ] {
            let (waves, intensity) = cue_profile(cue);
            assert!(!waves.is_empty());
            assert!((0.1..=1.0).contains(&intensity));
        }
    }
}
