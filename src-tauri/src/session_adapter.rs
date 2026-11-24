//! Tauri implementation of the core session host seam.
//!
//! The core owns process/session policy; only the shell knows KWin, Win32 and
//! the Tauri AppHandle. This adapter exposes those native details.

use open_scrcpy_core::runtime::{SessionHost, SessionIntegration};

pub struct TauriSessionHost;

impl SessionIntegration for TauriSessionHost {
    fn track_window(&self, pid: u32, position: Option<(i32, i32)>) {
        #[cfg(target_os = "linux")]
        crate::kwin::track_window(pid, position);
        #[cfg(not(target_os = "linux"))]
        let _ = (pid, position);
    }

    fn stop_tracking(&self, pid: u32) {
        #[cfg(target_os = "linux")]
        crate::kwin::stop_tracking(pid);
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }
}

impl SessionHost for TauriSessionHost {
    fn capture_window_position(&self, pid: u32) -> Option<(i32, i32)> {
        open_scrcpy_platform::window::try_capture_window_pos(pid)
    }

    fn force_kill(&self, pid: u32) {
        open_scrcpy_core::process::force_kill_pid(pid);
    }
}
