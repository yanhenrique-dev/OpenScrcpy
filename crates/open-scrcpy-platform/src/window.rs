//! Native geometry for the external `scrcpy` mirror window.
//!
//! The mirror is a separate process with its own SDL window, so the shell has
//! to read and move it through OS tooling: Win32 on Windows, `xdotool` plus the
//! KWin D-Bus bridge on Linux and System Events on macOS. Every failure mode is
//! non-fatal: a missing window, missing permission or missing tooling simply
//! returns `None` and the session keeps running.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command as StdCommand;

/// Parses `KEY=value` shell output (as printed by `xdotool ... --shell`) and
/// returns the integer value for `key`. Only compiled where it is used (the
/// Linux capture path, and the unit tests).
#[cfg(any(test, target_os = "linux"))]
fn parse_shell_var_int(text: &str, key: &str) -> Option<i32> {
    let prefix = format!("{}=", key);
    text.lines()
        .find(|l| l.starts_with(&prefix))
        .and_then(|l| l[prefix.len()..].trim().parse().ok())
}

/// Windows: finds the first visible, unowned top-level window belonging to
/// process `pid` (matches .NET's `MainWindowHandle` heuristic), i.e. scrcpy's
/// SDL mirror window rather than some owned tool/popup window. Shared by the
/// position-capture and recenter paths so both agree on which window they
/// mean.
#[cfg(target_os = "windows")]
fn find_window_by_pid(pid: u32) -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
    };

    // State threaded through EnumWindows via LPARAM.
    struct Search {
        target_pid: u32,
        found: HWND,
    }

    // WNDENUMPROC: BOOL(1) keeps enumerating, BOOL(0) stops.
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if GetWindow(hwnd, GW_OWNER)
            .map(|o| !o.0.is_null())
            .unwrap_or(false)
        {
            return BOOL(1);
        }
        let mut wnd_pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut wnd_pid as *mut u32));
        if wnd_pid == search.target_pid {
            search.found = hwnd;
            return BOOL(0); // found it -> stop enumeration
        }
        BOOL(1)
    }

    let mut search = Search {
        target_pid: pid,
        found: HWND::default(),
    };
    // EnumWindows returns Err both on real failure and when the callback
    // returns BOOL(0) (our intentional early stop), so ignore the Result and
    // read the captured handle instead.
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut Search as isize));
    }
    (!search.found.0.is_null()).then_some(search.found)
}

/// Queries the on-screen position of the scrcpy SDL window owned by process
/// `pid`, returning the client-area top-left in screen pixels. Returns `None`
/// silently whenever the window can't be read (not created yet, minimized,
/// tooling missing, permission denied); every failure mode is non-fatal.
///
/// The reference frame differs per platform and is reconciled afterwards by
/// `open_scrcpy_core::session::resolve_window_pos_to_persist`, rather than by
/// hardcoding decoration sizes:
/// - Windows: Win32 `ClientToScreen` -> client origin (matches scrcpy/SDL).
/// - Linux/X11 (+XWayland): `xdotool` -> window origin (usually the client area).
/// - macOS: `osascript`/System Events -> frame origin (title-bar top).
pub fn try_capture_window_pos(pid: u32) -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    {
        // Native Win32: this replaces spawning PowerShell + `Add-Type` (a C#
        // JIT) on every poll, it is a couple of direct syscalls with no child
        // process.
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};

        if let Some(hwnd) = find_window_by_pid(pid) {
            // A minimized window reports an off-screen sentinel origin
            // (~ -32000,-32000) and a maximized one the monitor edge; neither is
            // the windowed position we want, so skip them and keep the last good
            // sample.
            let special = unsafe { IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() };
            if !special {
                // ClientToScreen maps the client-area origin (0,0) into screen
                // pixels; GetWindowRect would return the outer frame and creep
                // the window up by the title-bar height on every relaunch.
                let mut pt = POINT { x: 0, y: 0 };
                if unsafe { ClientToScreen(hwnd, &mut pt) }.as_bool() {
                    return Some((pt.x, pt.y));
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let pid_str = pid.to_string();
        // X11/XWayland: xdotool can see the window directly. On native
        // Wayland it finds nothing, so fall through to KDE's reported cache.
        if let Ok(output) = StdCommand::new("xdotool")
            .args(["search", "--pid", &pid_str, "getwindowgeometry", "--shell"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let (Some(x), Some(y)) = (
                    parse_shell_var_int(&text, "X"),
                    parse_shell_var_int(&text, "Y"),
                ) {
                    return Some((x, y));
                }
            }
        }
        if let Some(position) = crate::kwin::cached_position(pid) {
            return Some(position);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // System Events needs Accessibility permission for the app (one-time
        // grant in System Settings). The try block swallows every error (no
        // window yet, permission denied, ...) so this stays a silent no-op
        // until the permission is granted.
        let script = format!(
            concat!(
                "tell application \"System Events\"\n",
                "try\n",
                "set proc to first process whose unix id is {}\n",
                "set pos to position of first window of proc\n",
                "((item 1 of pos) as string) & \" \" & ((item 2 of pos) as string)\n",
                "end try\n",
                "end tell"
            ),
            pid
        );
        let output = StdCommand::new("osascript")
            .args(["-e", &script])
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut parts = text.split_whitespace();
            let x: i32 = parts.next()?.parse().ok()?;
            let y: i32 = parts.next()?.parse().ok()?;
            return Some((x, y));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let _ = pid;
    None
}

/// Moves the scrcpy mirror window owned by `pid` to the centre of the primary
/// screen and brings it to the front. A safety net for a persisted position
/// that no longer lands on any connected monitor (e.g. it was saved while a
/// second monitor was attached, which has since been unplugged) -- the window
/// can reopen fully off-screen, with no title bar to drag back if borderless.
/// Best-effort and silent: does nothing if the window can't be found, is
/// already maximized (it fills its monitor; there is nothing to recover), or
/// the move fails.
pub fn recenter_window(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, GetWindowRect, IsIconic, IsZoomed, SetForegroundWindow, SetWindowPos,
            ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SWP_NOSIZE, SWP_NOZORDER, SW_RESTORE,
        };

        let Some(hwnd) = find_window_by_pid(pid) else {
            return;
        };
        if unsafe { IsZoomed(hwnd) }.as_bool() {
            return;
        }
        if unsafe { IsIconic(hwnd) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let (screen_w, screen_h) =
            unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                (screen_w - w) / 2,
                (screen_h - h) / 2,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER,
            )
        };
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }
    #[cfg(target_os = "linux")]
    {
        // X11/XWayland: xdotool is the direct path. Native Wayland hides the
        // window from X11 tooling, so hand it to KWin's scripting interface.
        if !recenter_window_xdotool(pid) {
            crate::kwin::recenter(pid);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Same Accessibility-permission requirement as the position capture
        // above; a silent no-op until it is granted.
        let script = format!(
            concat!(
                "tell application \"System Events\"\n",
                "try\n",
                "set proc to first process whose unix id is {}\n",
                "set win to first window of proc\n",
                "set {{winW, winH}} to size of win\n",
                "tell application \"Finder\" to set screenBounds to bounds of window of desktop\n",
                "set screenW to (item 3 of screenBounds)\n",
                "set screenH to (item 4 of screenBounds)\n",
                "set position of win to {{((screenW - winW) / 2), ((screenH - winH) / 2)}}\n",
                "set frontmost of proc to true\n",
                "end try\n",
                "end tell"
            ),
            pid
        );
        let _ = StdCommand::new("osascript").args(["-e", &script]).output();
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let _ = pid;
}

/// X11/XWayland recentre via xdotool. Returns false when the window is not
/// visible to X11 (native Wayland), so the caller can try KWin instead.
#[cfg(target_os = "linux")]
fn recenter_window_xdotool(pid: u32) -> bool {
    let pid_str = pid.to_string();
    let Ok(search_out) = StdCommand::new("xdotool")
        .args(["search", "--pid", &pid_str])
        .output()
    else {
        return false;
    };
    let Some(window_id) = String::from_utf8_lossy(&search_out.stdout)
        .lines()
        .next()
        .map(str::to_string)
    else {
        return false;
    };

    let Ok(geom_out) = StdCommand::new("xdotool")
        .args(["getwindowgeometry", "--shell", &window_id])
        .output()
    else {
        return false;
    };
    let geom_text = String::from_utf8_lossy(&geom_out.stdout);
    let (Some(w), Some(h)) = (
        parse_shell_var_int(&geom_text, "WIDTH"),
        parse_shell_var_int(&geom_text, "HEIGHT"),
    ) else {
        return false;
    };

    let Ok(display_out) = StdCommand::new("xdotool")
        .arg("getdisplaygeometry")
        .output()
    else {
        return false;
    };
    let display_text = String::from_utf8_lossy(&display_out.stdout);
    let mut parts = display_text.split_whitespace();
    let (Some(Ok(screen_w)), Some(Ok(screen_h))) = (
        parts.next().map(str::parse::<i32>),
        parts.next().map(str::parse::<i32>),
    ) else {
        return false;
    };

    let x = ((screen_w - w).max(0) / 2).to_string();
    let y = ((screen_h - h).max(0) / 2).to_string();
    let _ = StdCommand::new("xdotool")
        .args(["windowmove", &window_id, &x, &y])
        .output();
    let _ = StdCommand::new("xdotool")
        .args(["windowactivate", &window_id])
        .output();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xdotool_shell_geometry() {
        let text = "WINDOW=123\nX=10\nY=-20\nWIDTH=800\nHEIGHT=600\n";
        assert_eq!(parse_shell_var_int(text, "X"), Some(10));
        assert_eq!(parse_shell_var_int(text, "Y"), Some(-20));
        assert_eq!(parse_shell_var_int(text, "WIDTH"), Some(800));
        assert_eq!(parse_shell_var_int(text, "Z"), None);
    }
}
