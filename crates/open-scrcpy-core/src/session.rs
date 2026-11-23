//! Session policy that does not depend on a UI shell.
//!
//! The monitor loop still lives in the Tauri shell for now, but the pure
//! decisions it makes (backoff, retry and terminal payloads) live here so the
//! GPUI shell can reuse them unchanged.

use std::time::Duration;

use serde_json::{json, Value};

use crate::runtime::SessionHost;
use crate::state::{ScrcpyState, StopRequest};

#[derive(Debug, Clone, Copy)]
pub enum SessionEndReason {
    Stopped,
    Exited { success: bool },
    Error,
}

pub fn session_end_payload(device: &str, reason: SessionEndReason) -> Value {
    match reason {
        SessionEndReason::Stopped => {
            json!({ "device": device, "running": false, "reason": "stopped" })
        }
        SessionEndReason::Exited { success } => json!({
            "device": device,
            "running": false,
            "reason": "exited",
            "success": success
        }),
        SessionEndReason::Error => {
            json!({ "device": device, "running": false, "reason": "error" })
        }
    }
}

/// Backoff for automatic reconnection. The last delay repeats until the
/// session is stopped or the device comes back.
const RECONNECT_DELAYS_SECS: &[u64] = &[2, 4, 8, 16, 30];

pub fn reconnect_delay(attempt: u32) -> Duration {
    let index = (attempt as usize).min(RECONNECT_DELAYS_SECS.len() - 1);
    Duration::from_secs(RECONNECT_DELAYS_SECS[index])
}

/// Only an unexpected exit with no known capture/codec failure is retried.
/// `success == true` is a deliberate window close, and capture/codec failures
/// would repeat identically.
pub fn should_auto_reconnect(
    enabled: bool,
    has_serial: bool,
    success: bool,
    capture_failed: bool,
    codec_failed: bool,
) -> bool {
    enabled && has_serial && !success && !capture_failed && !codec_failed
}

/// Reads the display state from a `dumpsys power` snapshot. The most specific
/// signal wins: on recent Android versions `mWakefulness=Awake` can coexist
/// with a display that was just powered off, so the explicit display state
/// (and the legacy `mScreenOn` flag) must be checked first.
pub fn display_power_is_on(dumpsys_power: &str) -> bool {
    if let Some(state) = dumpsys_power.split("Display Power: state=").nth(1) {
        return state.trim_start().starts_with("ON");
    }
    if let Some(state) = dumpsys_power.split("mScreenOn=").nth(1) {
        return state.trim_start().starts_with("true");
    }
    if let Some(state) = dumpsys_power.split("mWakefulness=").nth(1) {
        return state.trim_start().starts_with("Awake");
    }
    false
}

/// Converts a raw captured position into the value to persist as
/// `--window-x/--window-y` so the window round-trips exactly on the next
/// launch. Offsets larger than any plausible decoration are ignored.
pub fn resolve_window_pos_to_persist(
    requested: Option<(i32, i32)>,
    first: Option<(i32, i32)>,
    last: (i32, i32),
) -> (i32, i32) {
    if let (Some((rx, ry)), Some((fx, fy))) = (requested, first) {
        let (dx, dy) = (fx - rx, fy - ry);
        if dx.abs() <= 80 && dy.abs() <= 120 {
            return (last.0 - dx, last.1 - dy);
        }
    }
    last
}

/// Stops a registered session, capturing the window position before it dies
/// and forcing the pid if the monitor does not finish within 3 seconds.
pub async fn stop_session(
    state: &ScrcpyState,
    device: &str,
    host: &dyn SessionHost,
) -> Result<(), String> {
    let handle = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device);
    let Some(handle) = handle else {
        return Ok(());
    };

    let crate::state::SessionHandle {
        pid,
        stop,
        done,
        track_position,
        ..
    } = handle;
    let pid_value = pid.load(std::sync::atomic::Ordering::Relaxed);

    // Grab the window position while it is guaranteed to still exist. Only
    // windowed mirror sessions persist it.
    let position = if track_position && pid_value != 0 {
        host.capture_window_position(pid_value)
    } else {
        None
    };

    let notified = done.notified();
    let _ = stop.send(StopRequest { position });
    match tokio::time::timeout(Duration::from_secs(3), notified).await {
        Ok(_) => Ok(()),
        Err(_) => {
            host.force_kill(pid_value);
            Err("session did not stop within 3s".to_string())
        }
    }
}

/// Kills every registered child and unloads the virtual microphone sink.
///
/// Synchronous by design: shell exit handlers run outside the async runtime,
/// so this cannot await a graceful stop. Every step is best effort.
pub fn teardown_sessions(state: &ScrcpyState, host: &dyn SessionHost) {
    let handles: Vec<crate::state::SessionHandle> = {
        let mut sessions = state
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        sessions.drain().map(|(_, handle)| handle).collect()
    };
    for handle in handles {
        let pid = handle.pid.load(std::sync::atomic::Ordering::Relaxed);
        if handle.track_position {
            host.stop_tracking(pid);
        }
        let _ = handle
            .stop
            .send(crate::state::StopRequest { position: None });
        host.force_kill(pid);
    }
    #[cfg(unix)]
    {
        crate::mic::restore_default_source_sync(state);
        crate::mic::teardown_mic_sink();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_payload_distinguishes_stop_exit_and_error() {
        let stopped = session_end_payload("device1", SessionEndReason::Stopped);
        assert_eq!(stopped["device"], "device1");
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["reason"], "stopped");
        assert!(stopped.get("success").is_none());

        let crashed = session_end_payload("device1", SessionEndReason::Exited { success: false });
        assert_eq!(crashed["reason"], "exited");
        assert_eq!(crashed["success"], false);

        let closed = session_end_payload("device1", SessionEndReason::Exited { success: true });
        assert_eq!(closed["success"], true);

        let error = session_end_payload("device1", SessionEndReason::Error);
        assert_eq!(error["reason"], "error");
        assert!(error.get("success").is_none());
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(2));
        assert_eq!(reconnect_delay(1), Duration::from_secs(4));
        assert_eq!(reconnect_delay(2), Duration::from_secs(8));
        assert_eq!(reconnect_delay(3), Duration::from_secs(16));
        assert_eq!(reconnect_delay(4), Duration::from_secs(30));
        assert_eq!(reconnect_delay(99), Duration::from_secs(30));
    }

    #[test]
    fn auto_reconnect_only_for_unexpected_plain_exits() {
        assert!(should_auto_reconnect(true, true, false, false, false));
        assert!(!should_auto_reconnect(false, true, false, false, false));
        assert!(!should_auto_reconnect(true, false, false, false, false));
        assert!(!should_auto_reconnect(true, true, true, false, false));
        assert!(!should_auto_reconnect(true, true, false, true, false));
        assert!(!should_auto_reconnect(true, true, false, false, true));
    }

    #[test]
    fn parses_dumpsys_power_variants() {
        assert!(display_power_is_on("Display Power: state=ON\n"));
        assert!(!display_power_is_on("Display Power: state=OFF\n"));
        assert!(!display_power_is_on(
            "Display Power: state=OFF\nmWakefulness=Awake\n"
        ));
        assert!(display_power_is_on("mScreenOn=true\nmWakefulness=Awake\n"));
        assert!(!display_power_is_on("mScreenOn=false\n"));
        assert!(display_power_is_on("mWakefulness=Awake\n"));
        assert!(!display_power_is_on("mWakefulness=Dozing\n"));
        assert!(!display_power_is_on("unrecognized dump"));
    }

    #[test]
    fn first_launch_uses_raw_position() {
        assert_eq!(
            resolve_window_pos_to_persist(None, Some((100, 72)), (300, 222)),
            (300, 222)
        );
        assert_eq!(
            resolve_window_pos_to_persist(None, None, (300, 222)),
            (300, 222)
        );
    }

    #[test]
    fn zero_offset_is_a_noop() {
        let requested = Some((300, 222));
        let first = Some((300, 222));
        assert_eq!(
            resolve_window_pos_to_persist(requested, first, (640, 480)),
            (640, 480)
        );
    }

    #[test]
    fn cancels_titlebar_offset() {
        assert_eq!(
            resolve_window_pos_to_persist(Some((300, 222)), Some((300, 250)), (400, 350)),
            (400, 322)
        );
    }

    #[test]
    fn cancels_full_frame_offset() {
        assert_eq!(
            resolve_window_pos_to_persist(Some((300, 222)), Some((292, 219)), (400, 350)),
            (408, 353)
        );
    }

    #[test]
    fn ignores_implausible_offsets() {
        assert_eq!(
            resolve_window_pos_to_persist(Some((0, 0)), Some((200, 0)), (300, 222)),
            (300, 222)
        );
    }
}
