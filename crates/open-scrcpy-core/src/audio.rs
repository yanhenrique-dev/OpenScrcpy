//! Audio failure classification and fallback policy.
//!
//! scrcpy degrades to video-only when audio capture or encoding fails, so the
//! stderr stream is the only signal. The classifiers below separate a codec
//! failure (repairable by trying the next codec) from a device-side capture
//! failure (not repairable), and decide when the fallback chain is exhausted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const AUDIO_FALLBACK_CHAIN: &[&str] = &["aac", "flac", "raw"];

/// True when a codec failure cannot be repaired by trying another codec:
/// automatic fallback is disabled, or the chain is exhausted.
pub fn codec_failure_is_terminal(should_auto_fallback: bool, chain_index: usize) -> bool {
    !should_auto_fallback || chain_index >= AUDIO_FALLBACK_CHAIN.len()
}

pub fn codec_failure_message(should_auto_fallback: bool) -> &'static str {
    if should_auto_fallback {
        "[SYSTEM] No compatible audio codec found. Consider disabling audio forwarding."
    } else {
        "[SYSTEM] The selected audio codec failed. Choose Automatic or another codec."
    }
}

pub fn is_audio_codec_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("could not create default audio encoder")
        || lower.contains("failed to initialize audio")
    {
        return true;
    }
    let mentions_codec = lower.contains("audio encoder") || lower.contains("audio codec");
    let mentions_failure = lower.contains("fail")
        || lower.contains("error")
        || lower.contains("could not")
        || lower.contains("not available")
        || lower.contains("not supported");
    mentions_codec && mentions_failure
}

/// A capture failure is a device-side limitation (Android version, locked
/// screen, an app opting out of capture). It is *not* repaired by switching
/// audio codecs, so it must be recognised separately from
/// [`is_audio_codec_error`] and reported to the user.
pub fn is_audio_capture_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    // scrcpy logs a transient DEBUG line while it retries capture itself.
    // Flagging it would produce a false warning shortly before success.
    if lower.contains("retrying") {
        return false;
    }
    lower.contains("failed to start audio capture")
        || lower.contains("audio capture failed")
        || lower.contains("failed to capture audio")
        || lower.contains("could not capture audio")
        || lower.contains("audio capture is not supported")
        || (lower.contains("demuxer 'audio'") && lower.contains("disabled"))
        || (lower.contains("audio") && lower.contains("foreground") && lower.contains("unlocked"))
}

/// Audio failures observed on scrcpy's stderr. The two flags drive different
/// responses: a codec error can be repaired by retrying with another codec,
/// while a capture failure is reported and left alone.
#[derive(Clone, Default)]
pub struct AudioErrorState {
    codec_failed: Arc<AtomicBool>,
    capture_failed: Arc<AtomicBool>,
}

impl AudioErrorState {
    pub fn codec_failed(&self) -> bool {
        self.codec_failed.load(Ordering::SeqCst)
    }

    pub fn capture_failed(&self) -> bool {
        self.capture_failed.load(Ordering::SeqCst)
    }

    pub fn note_codec(&self) {
        self.codec_failed.store(true, Ordering::SeqCst);
    }

    pub fn note_capture(&self) {
        self.capture_failed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_error_matches_known_patterns() {
        assert!(is_audio_capture_error(
            "[server] ERROR: Failed to start audio capture"
        ));
        assert!(is_audio_capture_error(
            "On Android 11, audio capture must be started in the foreground, make sure that the device is unlocked"
        ));
        assert!(is_audio_capture_error(
            "WARN: Demuxer 'audio': stream explicitly disabled by the device"
        ));
        assert!(is_audio_capture_error("audio capture failed"));
        // Transient DEBUG line: scrcpy retries on its own, do not warn.
        assert!(!is_audio_capture_error(
            "[server] DEBUG: Failed to start audio capture, retrying..."
        ));
    }

    #[test]
    fn codec_error_matches_known_patterns() {
        assert!(is_audio_codec_error(
            "ERROR: Could not create default audio encoder for opus"
        ));
        assert!(is_audio_codec_error("Failed to initialize audio/opus"));
        assert!(is_audio_codec_error("audio codec not supported on device"));
        assert!(is_audio_codec_error("audio encoder failed to start"));
    }

    #[test]
    fn codec_error_ignores_unrelated() {
        assert!(!is_audio_codec_error("ADB disconnected"));
        assert!(!is_audio_codec_error("device offline"));
        assert!(!is_audio_codec_error("video encoder error"));
        assert!(!is_audio_codec_error("permission issue"));
        assert!(!is_audio_codec_error("INFO Audio codec selected: opus"));
    }

    #[test]
    fn capture_error_ignores_codec_errors() {
        assert!(!is_audio_capture_error("Failed to initialize audio/opus"));
        assert!(!is_audio_capture_error("audio encoder failed to start"));
        assert!(!is_audio_capture_error("ADB disconnected"));
        assert!(!is_audio_capture_error("INFO Audio codec selected: opus"));
        assert!(!is_audio_codec_error(
            "[server] ERROR: Failed to start audio capture"
        ));
    }

    #[test]
    fn error_state_tracks_failures_independently() {
        let state = AudioErrorState::default();
        assert!(!state.codec_failed());
        assert!(!state.capture_failed());

        state.note_codec();
        assert!(state.codec_failed());
        assert!(!state.capture_failed());

        state.note_capture();
        assert!(state.codec_failed());
        assert!(state.capture_failed());
    }

    #[test]
    fn codec_failure_is_terminal_only_without_a_next_codec() {
        // A codec chosen by the user must never trigger the automatic chain.
        assert!(codec_failure_is_terminal(false, 0));

        // Automatic fallback is repairable until the chain is exhausted.
        assert!(!codec_failure_is_terminal(true, 0));
        assert!(!codec_failure_is_terminal(true, 1));
        assert!(!codec_failure_is_terminal(true, 2));
        assert!(codec_failure_is_terminal(true, 3));
    }
}
