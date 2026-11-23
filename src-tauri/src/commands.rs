use crate::audio::{codec_failure_is_terminal, codec_failure_message, AUDIO_FALLBACK_CHAIN};
use crate::capabilities::Capabilities;
use crate::config::{build_scrcpy_args, validate_scrcpy_config, ScrcpyConfig};
use crate::process;
use crate::process::create_command;
#[cfg(target_os = "windows")]
use crate::process::CREATE_NO_WINDOW;
use crate::validate;
use crate::{
    bump_session_generation, RetryContext, RetryOrigin, ScrcpyState, SessionHandle, SessionRuntime,
    StopRequest,
};
use open_scrcpy_core::session::{
    display_power_is_on, reconnect_delay, should_auto_reconnect, SessionEndReason,
};
use serde_json::json;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
#[cfg(unix)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tauri::{Manager, State, Window};
#[cfg(all(test, unix))]
use tokio::process::Command as TokioCommand;
use tokio::sync::{oneshot, Notify};
use tokio::time::{timeout, Duration};

// Screen-off startup: the ADB wake must be quick, but a wireless device can
// take a little longer to report the new display state.
const ADB_WAKE_TIMEOUT: Duration = Duration::from_secs(5);
const DISPLAY_POWER_QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const DISPLAY_WAKE_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const DISPLAY_WAKE_POLL_INTERVAL: Duration = Duration::from_millis(150);

fn get_binary_path(binary_name: &str, custom_folder: Option<String>) -> String {
    crate::paths::binary_path(binary_name, custom_folder.as_deref())
}

/// Client for the GitHub release calls. The connect deadline is what keeps a
/// stalled TCP/TLS handshake from hanging the download or update UI forever;
/// individual requests still set their own total timeout.
#[tauri::command]
pub async fn check_scrcpy(custom_path: Option<String>) -> serde_json::Value {
    crate::adb::check_scrcpy(custom_path.as_deref()).await
}

#[tauri::command]
pub async fn get_devices(custom_path: Option<String>) -> serde_json::Value {
    crate::adb::list_devices(custom_path.as_deref()).await
}

#[tauri::command]
pub async fn get_mdns_devices(custom_path: Option<String>) -> serde_json::Value {
    crate::adb::list_mdns_services(custom_path.as_deref()).await
}

#[tauri::command]
pub async fn adb_connect(
    ip: String,
    custom_path: Option<String>,
    silent: Option<bool>,
) -> Result<serde_json::Value, String> {
    crate::adb::connect(
        crate::events::log_line,
        &ip,
        custom_path.as_deref(),
        silent.unwrap_or(false),
    )
    .await
}

#[tauri::command]
pub async fn adb_pair(
    ip: String,
    code: String,
    custom_path: Option<String>,
) -> Result<serde_json::Value, String> {
    crate::adb::pair(crate::events::log_line, &ip, &code, custom_path.as_deref()).await
}

#[tauri::command]
pub async fn adb_shell(
    device: String,
    command: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    crate::adb::shell(&device, &command, custom_path.as_deref()).await
}

#[tauri::command]
pub async fn run_terminal_command(
    device: Option<String>,
    cmd: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    crate::terminal::run_command(device.as_deref(), &cmd, custom_path.as_deref()).await
}

#[tauri::command]
pub async fn push_file(
    device: String,
    file_path: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    crate::adb::push_file(&device, &file_path, custom_path.as_deref()).await
}

#[tauri::command]
pub async fn install_apk(
    device: String,
    file_path: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    crate::adb::install_apk(&device, &file_path, custom_path.as_deref()).await
}

#[tauri::command]
pub async fn kill_adb(custom_path: Option<String>) -> Result<serde_json::Value, String> {
    Ok(crate::adb::kill_server(crate::events::log_line, custom_path.as_deref()).await)
}

#[tauri::command]
pub async fn list_scrcpy_options(
    device: String,
    arg: String,
    custom_path: Option<String>,
) -> serde_json::Value {
    crate::scrcpy::list_options(&device, &arg, custom_path.as_deref()).await
}

#[tauri::command]
pub async fn get_render_drivers(custom_path: Option<String>) -> serde_json::Value {
    crate::scrcpy::get_render_drivers(custom_path.as_deref()).await
}

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
/// [`resolve_window_pos_to_persist`], rather than by hardcoding decoration
/// sizes:
/// - Windows: Win32 `ClientToScreen` -> client origin (matches scrcpy/SDL).
/// - Linux/X11 (+XWayland): `xdotool` -> window origin (usually the client area).
/// - macOS: `osascript`/System Events -> frame origin (title-bar top).
pub(crate) fn try_capture_window_pos(pid: u32) -> Option<(i32, i32)> {
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
fn recenter_window(pid: u32) {
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

/// Re-centres the running mirror window for `device` on the primary screen.
/// Bound to a global OS shortcut (Ctrl+Alt+Shift+C, see `shortcuts.rs`) so a
/// window whose saved position ends up off-screen (see [`recenter_window`])
/// can be recovered without being able to see or focus it directly. A no-op
/// if `device` has no tracked process (e.g. nothing is running).
pub fn recenter_device(state: &ScrcpyState, device: &str) {
    let pid = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device)
        .map(|s| s.pid.load(Ordering::Relaxed))
        .filter(|pid| *pid != 0);
    if let Some(pid) = pid {
        recenter_window(pid);
    }
}

#[tauri::command]
pub fn recenter_scrcpy_window(
    state: State<'_, std::sync::Arc<ScrcpyState>>,
    device: String,
) -> Result<(), String> {
    recenter_device(&state, &device);
    Ok(())
}

/// Records which device the GUI currently considers selected, so the global
/// recentre shortcut (which fires with no window necessarily focused) knows
/// which mirror window to act on.
#[tauri::command]
pub fn set_active_device(
    state: State<'_, std::sync::Arc<ScrcpyState>>,
    device: Option<String>,
) -> Result<(), String> {
    *state
        .active_device
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = device;
    Ok(())
}

/// Only windowed mirror sessions have a persisted window position. Camera and
/// desktop windows must not inherit the mirror's saved position, and a
/// fullscreen session has nothing to restore.
fn tracks_window_position(config: &ScrcpyConfig) -> bool {
    config.session_mode == "mirror" && !config.fullscreen.unwrap_or(false)
}

fn session_mode_label(mode: &str) -> &'static str {
    match mode {
        "camera" => "Camera",
        "mic" => "Microphone bridge",
        "desktop" => "Desktop",
        _ => "Mirror",
    }
}

/// Fails before spawning when the selected mode needs a flag this scrcpy build
/// does not have. Cosmetic flags are filtered separately with a warning.
fn ensure_mode_supported(config: &ScrcpyConfig, caps: &Capabilities) -> Result<(), String> {
    let mut required: Vec<&str> = match config.session_mode.as_str() {
        "camera" => vec!["video-source", "camera-size"],
        "mic" => vec!["no-window", "no-video", "audio-source"],
        "desktop" => vec!["new-display"],
        _ => Vec::new(),
    };
    if config.audio_enabled.unwrap_or(true) && config.session_mode == "camera" {
        required.push("audio-source");
    }

    let hid = config.session_mode != "mic"
        && (config.hid_keyboard.unwrap_or(false) || config.hid_mouse.unwrap_or(false));
    let otg_pure = config.otg_pure.unwrap_or(false);
    let tcp = config.device.contains('.') || config.device.contains(':');
    if config.session_mode == "mirror" && hid && otg_pure {
        if tcp {
            required.extend(["no-video", "no-audio", "keyboard", "mouse"]);
        } else {
            required.push("otg");
        }
    } else if hid {
        if config.hid_keyboard.unwrap_or(false) {
            required.push("keyboard");
        }
        if config.hid_mouse.unwrap_or(false) {
            required.push("mouse");
        }
    }

    let missing: Vec<String> = required
        .iter()
        .filter(|flag| !caps.supports(flag))
        .map(|flag| format!("--{flag}"))
        .collect();
    if !missing.is_empty() {
        let hint = if config.session_mode == "mirror" {
            "Update scrcpy to enable these options."
        } else {
            "Update scrcpy or use Mirror mode."
        };
        return Err(format!(
            "{} mode requires options missing from scrcpy {}: {}. {hint}",
            session_mode_label(&config.session_mode),
            caps.version(),
            missing.join(", ")
        ));
    }

    if config.audio_enabled.unwrap_or(true) && !caps.supports("audio-codec") {
        return Err(format!(
            "Audio forwarding is not supported by scrcpy {} (missing --audio-codec). Disable audio or update scrcpy.",
            caps.version()
        ));
    }
    if config.require_audio.unwrap_or(false)
        && config.audio_enabled.unwrap_or(true)
        && !caps.supports("require-audio")
    {
        return Err(format!(
            "Require audio is not supported by scrcpy {} (missing --require-audio). Disable it or update scrcpy.",
            caps.version()
        ));
    }
    Ok(())
}

/// Replaces the denoised microphone source when the detected build does not
/// offer it, so an old custom binary keeps recording instead of failing.
fn normalize_mic_source(config: &mut ScrcpyConfig, caps: &Capabilities) -> Option<String> {
    if config.session_mode != "mic" {
        return None;
    }
    let source = validate::normalize_mic_source(config.mic_source.as_deref().unwrap_or("mic"));
    if source == "mic-voice-communication" && !caps.supports_audio_source(source) {
        config.mic_source = Some("mic".to_string());
        return Some(format!(
            "[SYSTEM] scrcpy {} does not support the denoised microphone source; using the raw microphone.",
            caps.version()
        ));
    }
    None
}

/// Falls back to the device output when the selected mirror audio source is
/// not offered by this scrcpy build. `--audio-dup` is only meaningful with
/// `--audio-source=playback`, so it is disabled when the source falls back or
/// the binary does not support the flag.
fn normalize_mirror_audio(config: &mut ScrcpyConfig, caps: &Capabilities) -> Option<String> {
    if config.session_mode != "mirror" || !config.audio_enabled.unwrap_or(true) {
        return None;
    }
    let source = config
        .mirror_audio_source
        .as_deref()
        .unwrap_or("output")
        .trim()
        .to_string();
    if source != "output" && !caps.supports_audio_source(&source) {
        config.mirror_audio_source = Some("output".to_string());
        config.audio_dup = Some(false);
        return Some(format!(
            "[AUDIO] scrcpy {} does not support audio source '{}'; using the device output instead.",
            caps.version(),
            source
        ));
    }
    if source == "playback" && config.audio_dup.unwrap_or(false) && !caps.supports("audio-dup") {
        config.audio_dup = Some(false);
        return Some(format!(
            "[AUDIO] scrcpy {} does not support --audio-dup; playback will mute the device while mirroring.",
            caps.version()
        ));
    }
    None
}

/// Emits the resolved window position (when one was captured) followed by the
/// session-stopped status. Shared by every monitor-loop exit path.
fn emit_window_pos_and_stop(
    device: &str,
    requested: Option<(i32, i32)>,
    first: Option<(i32, i32)>,
    last: Option<(i32, i32)>,
    reason: SessionEndReason,
) {
    crate::runtime::emit_window_pos_and_stop(
        crate::events::bus(),
        device,
        requested,
        first,
        last,
        reason,
    );
}

/// True when this session should send audio into the shared virtual sink.
/// Camera mode already captures the microphone; opting into the sink makes it
/// selectable as a recording device instead of only playing on the default
/// output. The microphone bridge always uses the sink on Linux.
fn wants_virtual_mic(config: &ScrcpyConfig) -> bool {
    config.session_mode == "mic"
        || (config.session_mode == "camera"
            && config.camera_mic_sink.unwrap_or(false)
            && config.audio_enabled.unwrap_or(true))
}

/// Screen-off sessions are the only ones where scrcpy must power the device
/// on and then off again. Camera and mic sessions never use the flag (see
/// `build_scrcpy_args`), so they must not pay the extra ADB round-trip.
fn wakes_before_screen_off(config: &ScrcpyConfig) -> bool {
    config.turn_off.unwrap_or(false)
        && !config.device.is_empty()
        && matches!(config.session_mode.as_str(), "mirror" | "desktop")
}

async fn read_display_power_state(adb_exe_path: &str, device: &str) -> bool {
    let mut cmd = create_command(adb_exe_path);
    cmd.args(["-s", device, "shell", "dumpsys", "power"]);
    match process::output_with_timeout(cmd, DISPLAY_POWER_QUERY_TIMEOUT).await {
        Ok(output) if output.status.success() => {
            display_power_is_on(&String::from_utf8_lossy(&output.stdout))
        }
        _ => false,
    }
}

/// Wakes the device display before starting a screen-off session.
///
/// scrcpy powers the device on by itself, but it does so asynchronously and
/// then sends the `--turn-screen-off` request right after setup. On slower
/// devices the power-off can win the race and be overwritten by the pending
/// power-on, which is why the phone's screen sometimes only turns off after
/// the user wakes it by hand. Waking it here (and waiting for Android to
/// report the display as on) makes scrcpy start from a known state, so its
/// screen-off request lands last.
async fn wake_display_before_screen_off(adb_exe_path: &str, device: &str) {
    crate::events::log("[SYSTEM] Screen off requested: waking the device display...".to_string());

    let mut wake = create_command(adb_exe_path);
    wake.args(["-s", device, "shell", "input", "keyevent", "224"]);
    let wake_ok = process::output_with_timeout(wake, ADB_WAKE_TIMEOUT)
        .await
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !wake_ok {
        crate::events::log(
            "[SYSTEM] Could not wake the device display; scrcpy will try to power it on."
                .to_string(),
        );
        return;
    }

    let deadline = tokio::time::Instant::now() + DISPLAY_WAKE_SETTLE_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if read_display_power_state(adb_exe_path, device).await {
            crate::events::log("[SYSTEM] Device display is awake.".to_string());
            return;
        }
        tokio::time::sleep(DISPLAY_WAKE_POLL_INTERVAL).await;
    }

    crate::events::log(
        "[SYSTEM] Device display did not report as awake; scrcpy will try to power it on."
            .to_string(),
    );
}

enum AttemptOutcome {
    Exited { description: String, success: bool },
    WaitError(String),
}

/// Asks the child to exit, waits `grace`, then forces it. Returns whether the
/// graceful path was enough.
pub(crate) fn force_kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = StdCommand::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = StdCommand::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

// ── Microphone bridge helpers (Linux / PipeWire-PulseAudio) ─────────
// For the phone mic to become a *recording device* on the PC, we create a
// dedicated null sink ("OpenScrcpy Mic"): scrcpy's stream plays into it and
// the sink's monitor shows up to every app as a microphone input. The sink
// is service-owned: created at bridge start, unloaded when nothing uses it.
#[cfg(unix)]
const MIC_SINK_NAME: &str = "openscrcpy_mic";
#[cfg(unix)]
const MIC_SINK_LABEL: &str = "OpenScrcpy Mic";
#[cfg(unix)]
const MIC_SINK_MONITOR: &str = "openscrcpy_mic.monitor";

#[cfg(unix)]
struct PactlOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

#[cfg(unix)]
async fn pactl_run(args: &[&str]) -> Option<PactlOut> {
    let mut cmd = create_command("pactl");
    cmd.args(args);
    let output = process::output_with_timeout(cmd, Duration::from_secs(4))
        .await
        .ok()?;
    Some(PactlOut {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

#[cfg(unix)]
async fn pactl_get_default_source() -> Option<String> {
    let out = pactl_run(&["get-default-source"]).await?;
    if out.ok && !out.stdout.trim().is_empty() {
        Some(out.stdout.trim().to_string())
    } else {
        None
    }
}

#[cfg(unix)]
async fn pactl_set_default_source(source: &str) -> bool {
    pactl_run(&["set-default-source", source])
        .await
        .map(|out| out.ok)
        .unwrap_or(false)
}

/// Makes the virtual monitor source the system default input. The caller holds
/// `ScrcpyState::mic_sink_lock`, so the saved previous source and the user
/// count cannot race with a stop. Returns false when nothing changed.
#[cfg(unix)]
async fn ensure_default_mic_source(state: &ScrcpyState) -> bool {
    let users = state.mic_default_source_users.load(Ordering::Relaxed);
    if users > 0 {
        state
            .mic_default_source_users
            .fetch_add(1, Ordering::Relaxed);
        return true;
    }

    let Some(previous) = pactl_get_default_source().await else {
        crate::events::log(
            "[MIC] Could not read the current default source; leaving it unchanged.".to_string(),
        );
        return false;
    };
    if previous == MIC_SINK_MONITOR {
        crate::events::log(
            "[MIC] The virtual monitor source is already the default; leaving it unchanged."
                .to_string(),
        );
        return false;
    }
    if !pactl_set_default_source(MIC_SINK_MONITOR).await {
        crate::events::log(
            "[MIC] Could not set Monitor of OpenScrcpy Mic as the default source.".to_string(),
        );
        return false;
    }

    *state
        .mic_previous_default_source
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(previous.clone());
    state.mic_default_source_users.store(1, Ordering::Relaxed);
    crate::events::log(format!(
            "[MIC] Monitor of OpenScrcpy Mic is now the default source; will restore \"{}\" when the bridge stops.",
            previous
        ),);
    true
}

/// Releases one opt-in default-source user and restores the saved source when
/// the last one stops. It only restores if the default is still the virtual
/// source, so a manual change by the user wins.
#[cfg(unix)]
async fn release_default_mic_source(state: &ScrcpyState) {
    let _guard = state.mic_sink_lock.lock().await;
    let users = state.mic_default_source_users.load(Ordering::Relaxed);
    if users == 0 {
        return;
    }
    let remaining = users - 1;
    state
        .mic_default_source_users
        .store(remaining, Ordering::Relaxed);
    if remaining > 0 {
        return;
    }

    let previous = state
        .mic_previous_default_source
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let Some(previous) = previous else {
        return;
    };
    if pactl_get_default_source().await.as_deref() != Some(MIC_SINK_MONITOR) {
        crate::events::log(
            "[MIC] Default source changed by another app; not restoring the previous one."
                .to_string(),
        );
        return;
    }
    if pactl_set_default_source(&previous).await {
        crate::events::log(format!(
            "[MIC] Default source restored to \"{}\".",
            previous
        ));
    } else {
        crate::events::log(format!(
            "[MIC] Could not restore the previous default source \"{}\".",
            previous
        ));
    }
}

/// Synchronous teardown variant for `RunEvent::Exit`, where the async runtime
/// is not available. Best effort, like the rest of the exit path.
#[cfg(unix)]
fn restore_default_source_sync(state: &ScrcpyState) {
    let previous = state
        .mic_previous_default_source
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    state.mic_default_source_users.store(0, Ordering::Relaxed);
    let Some(previous) = previous else {
        return;
    };
    let current = StdCommand::new("pactl")
        .args(["get-default-source"])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    if current.as_deref() == Some(MIC_SINK_MONITOR) {
        let _ = StdCommand::new("pactl")
            .args(["set-default-source", &previous])
            .output();
    }
}

// Returns true when the virtual sink exists (created now, if absent).
// With mic_denoise (experimental) the sink is born from a PipeWire RNNoise
// filter-chain, so apps recording its monitor pick up suppressed noise.
#[cfg(unix)]
async fn ensure_mic_sink(denoise: bool) -> bool {
    let Some(sinks) = pactl_run(&["list", "short", "sinks"]).await else {
        crate::events::log(
            "[MIC] pactl not found; routing skipped (audio goes to the default output)."
                .to_string(),
        );
        return false;
    };
    if sinks
        .stdout
        .lines()
        .any(|l| l.split('\t').nth(1) == Some(MIC_SINK_NAME))
    {
        let message = if denoise {
            "[MIC] Shared virtual sink OpenScrcpy Mic already loaded; reusing it (noise suppression is only applied when the bridge creates the sink)."
        } else {
            "[MIC] Virtual sink OpenScrcpy Mic already loaded."
        };
        crate::events::log(message.to_string());
        return true;
    }

    if denoise {
        // RNNoise ships with PipeWire's example filter-chain configs; try the
        // known install paths before falling back to the plain sink.
        let candidates = [
            "/usr/share/pipewire/filter-chain/sink-rnnoise.conf",
            "/usr/share/pipewire/filter-chain/rnnoise-sink.conf",
        ];
        match candidates.iter().find(|p| Path::new(p).exists()) {
            Some(conf) => {
                let load = pactl_run(&[
                    "load-module",
                    "module-filter-chain",
                    &format!("x={}", conf),
                    &format!("sink_name={}", MIC_SINK_NAME),
                    &format!("sink_properties=device.description=\"{}\"", MIC_SINK_LABEL),
                ])
                .await;
                match load {
                    Some(o) if o.ok => {
                        crate::events::log(format!(
                            "[MIC] Noise-suppressed sink \"OpenScrcpy Mic\" created (RNNoise: {}).",
                            conf
                        ));
                        return true;
                    }
                    Some(o) => {
                        crate::events::log(format!(
                            "[MIC] RNNoise sink rejected by PipeWire: {}.",
                            o.stderr
                        ));
                    }
                    None => {
                        crate::events::log(
                            "[MIC] RNNoise sink failed to load; trying the plain sink.".to_string(),
                        );
                    }
                }
            }
            None => {
                crate::events::log(
                    "[MIC] RNNoise filter-chain file not found; using the unfiltered sink."
                        .to_string(),
                );
            }
        }
    }

    let created = pactl_run(&[
        "load-module",
        "module-null-sink",
        &format!("sink_name={}", MIC_SINK_NAME),
        &format!("sink_properties=device.description=\"{}\"", MIC_SINK_LABEL),
    ])
    .await;
    match created {
        Some(o) if o.ok => {
            crate::events::log(format!(
                    "[MIC] Virtual sink \"{}\" created — select \"Monitor of {}\" (or the matching monitor source) as the microphone in Discord/OBS/Recorder.",
                    MIC_SINK_LABEL, MIC_SINK_LABEL
                ),);
            true
        }
        Some(o) => {
            crate::events::log(format!(
                "[MIC] Could not create the virtual sink: {}.",
                o.stderr
            ));
            false
        }
        None => {
            crate::events::log(
                "[MIC] Could not create the virtual sink; audio goes to the default output."
                    .to_string(),
            );
            false
        }
    }
}

#[cfg(unix)]
async fn unload_mic_sink() {
    let Some(sinks) = pactl_run(&["list", "short", "sinks"]).await else {
        return;
    };
    if !sinks
        .stdout
        .lines()
        .any(|l| l.split('\t').nth(1) == Some(MIC_SINK_NAME))
    {
        return;
    }
    let Some(modules) = pactl_run(&["list", "short", "modules"]).await else {
        return;
    };
    let module_idx = modules
        .stdout
        .lines()
        .find(|l| l.contains(&format!("sink_name={}", MIC_SINK_NAME)))
        .and_then(|l| l.split('\t').next().map(str::to_string));
    if let Some(idx) = module_idx {
        let _ = pactl_run(&["unload-module", &idx]).await;
        crate::events::log(
            "[MIC] Virtual sink unloaded (no active session is using it).".to_string(),
        );
    }
}

/// Reserves one user of the shared virtual sink. The caller must hold
/// `ScrcpyState::mic_sink_lock` across creation and this reservation, so a
/// session stopping at the same time cannot unload the sink in between.
#[cfg(unix)]
fn reserve_mic_sink_user(users: &AtomicUsize) {
    users.fetch_add(1, Ordering::Relaxed);
}

/// Releases one user. Returns true when the caller must unload the sink
/// because this was the last one; never underflows on a stray release.
/// Callers must hold `ScrcpyState::mic_sink_lock`, like the reservation path.
#[cfg(unix)]
fn release_mic_sink_user(users: &AtomicUsize) -> bool {
    let current = users.load(Ordering::Relaxed);
    if current == 0 {
        return false;
    }
    users.store(current - 1, Ordering::Relaxed);
    current == 1
}

/// Releases this session's reservation and unloads the sink when it was the
/// last user. Serialized with start-up, so a replacement can reserve its own
/// user before the previous session's monitor gets here.
#[cfg(unix)]
async fn release_mic_sink(state: &ScrcpyState) {
    let _guard = state.mic_sink_lock.lock().await;
    if release_mic_sink_user(&state.mic_sink_users) {
        unload_mic_sink().await;
    }
}

/// Bump the logical session generation for a device. run_scrcpy calls this
/// before replacing a previous process, so sidecars spawned for the old
/// session can see they are stale even when the registry key is re-occupied
/// by the replacement before their next liveness check.
#[cfg_attr(not(unix), allow(dead_code))]
/// True when no replacement has taken over this device. A replaced session
/// must not publish a stop event: the new session already owns the UI state.
fn generation_is_current(app_handle: &tauri::AppHandle, device: &str, generation: u64) -> bool {
    app_handle
        .state::<std::sync::Arc<ScrcpyState>>()
        .generation_is_current(device, generation)
}

/// Sidecar liveness: the device is still registered AND the generation this
/// loop was spawned with is still the current one. Both locks are taken one
/// after the other, never together, so this cannot deadlock with the
/// run_scrcpy paths (which also never hold both at once).
#[cfg_attr(not(unix), allow(dead_code))]
fn session_alive(app_handle: &tauri::AppHandle, device: &str, generation: u64) -> bool {
    app_handle
        .state::<std::sync::Arc<ScrcpyState>>()
        .session_alive(device, generation)
}

// Watcher that keeps the scrcpy stream inside the virtual sink for the
// whole session. The microphone bridge also reports the current destination
// to the GUI; camera sessions only move the stream, because the `mic-route`
// event belongs to the bridge's MicMeter and must not carry a camera's state.
// PULSE_SINK does not reach SDL3 clients and the stream shows up only after
// scrcpy's server handshake (seconds later), so a one-shot check is not enough.
// Camera sessions pass a slower steady interval because they can stay open
// for hours; the mic bridge keeps the tighter default.
#[cfg(unix)]
async fn spawn_mic_route_watcher(
    device: String,
    app_handle: tauri::AppHandle,
    session_pid: Arc<AtomicU32>,
    generation: u64,
    steady_interval: Duration,
    report_route: bool,
) {
    tokio::spawn(async move {
        let mut attempts: u32 = 0;
        // Route state of the latest poll we saw (None = nothing yet): drives
        // the pacing exactly like the change-only version did, without
        // gating the emit.
        let mut last_routed: Option<bool> = None;
        loop {
            if !session_alive(&app_handle, &device, generation) {
                break;
            }
            attempts += 1;
            let pid = session_pid.load(Ordering::Relaxed);
            if let Some((display, routed)) = poll_mic_route(pid).await {
                last_routed = Some(routed);
                // Emit every pass instead of only on change: the GUI only
                // subscribes while the mic tab is mounted, so a remounted tab
                // would otherwise stay without destination info until the
                // route actually changes. Camera sessions skip this event:
                // a bridge on another device owns the MicMeter, and a camera
                // "searching" state must not overwrite its destination.
                if report_route {
                    crate::events::mic_route(display, routed);
                }
            }
            // Fast cadence while hunting the fresh stream (the handshake
            // lands in ~1.3s) or while a routed stream is MIA (<10 attempts),
            // slow down once healthy or after the hunt window closes.
            let steady = last_routed == Some(true) || attempts > 10;
            if steady {
                tokio::time::sleep(steady_interval).await;
            } else {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });
}

/// True when a `pactl list sink-inputs` block belongs to the scrcpy process
/// that owns the mic bridge. The PID is unique; the application name is not
/// reliable when the user points the app at a renamed custom binary.
#[cfg(unix)]
fn sink_input_is_scrcpy_pid(block: &str, pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let process = format!("application.process.id = \"{}\"", pid);
    block.lines().any(|line| line.trim() == process)
}

// One routing pass. Returns the destination for the GUI: (text, in_virtual)
// where empty text means "no scrcpy stream yet". Maintains the route when the
// stream landed on some other output.
#[cfg(unix)]
async fn poll_mic_route(scrcpy_pid: u32) -> Option<(String, bool)> {
    if scrcpy_pid == 0 {
        return Some((String::new(), false));
    }
    // Verbose sinks: entries start at "Sink #N" and carry Name/Description.
    let mut sink_map: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let sinks = pactl_run(&["list", "sinks"]).await?;
    for block in sinks.stdout.split("Sink #").skip(1) {
        let idx = block.lines().next().unwrap_or("").trim().to_string();
        if idx.is_empty() {
            continue;
        }
        let name = block
            .lines()
            .find(|l| l.trim().starts_with("Name: "))
            .and_then(|l| l.split_once("Name: ").map(|(_, v)| v.trim().to_string()));
        let desc = block
            .lines()
            .find(|l| l.trim().starts_with("Description: "))
            .and_then(|l| {
                l.split_once("Description: ")
                    .map(|(_, v)| v.trim().to_string())
            });
        if let Some(name) = name {
            sink_map.insert(idx, (name, desc.unwrap_or_default()));
        }
    }
    let (target_idx, _) = sink_map
        .iter()
        .find(|(_, (name, _))| name == MIC_SINK_NAME)
        .map(|(k, v)| (k.clone(), v.1.clone()))?;

    let inputs = pactl_run(&["list", "sink-inputs"]).await?;
    for block in inputs.stdout.split("\n\n") {
        if !sink_input_is_scrcpy_pid(block, scrcpy_pid) {
            continue;
        }
        // move-sink-input takes the CLIENT (stream) index first, then the
        // sink. Stream index lives in the block header: "Sink Input #1340".
        let stream_idx = block
            .lines()
            .next()
            .and_then(|l| l.trim().strip_prefix("Sink Input #"))
            .map(|v| v.trim().to_string());
        let current_idx = block
            .lines()
            .find(|l| l.trim().starts_with("Sink: "))
            .and_then(|l| l.split(':').nth(1).map(|v| v.trim().to_string()));
        let Some(stream_idx) = stream_idx else {
            return Some((String::new(), false));
        };
        let Some(current_idx) = current_idx else {
            return Some((String::new(), false));
        };
        let (cur_name, cur_desc) = sink_map
            .get(&current_idx)
            .cloned()
            .unwrap_or_else(|| (String::new(), String::new()));

        if cur_name == MIC_SINK_NAME {
            return Some((MIC_SINK_LABEL.to_string(), true));
        }
        // The stream went somewhere else: bring it into the virtual sink.
        let moved = pactl_run(&["move-sink-input", &stream_idx, &target_idx]).await;
        let ok = moved.as_ref().map(|m| m.ok).unwrap_or(false);
        crate::events::log(if ok {
            "[MIC] Audio stream moved into the virtual sink.".to_string()
        } else {
            format!(
                "[MIC] move-sink-input failed: {}.",
                moved.map(|m| m.stderr).unwrap_or_default()
            )
        });
        return Some((
            if ok {
                MIC_SINK_LABEL.to_string()
            } else {
                cur_desc
            },
            ok,
        ));
    }
    // scrcpy stream not there yet (server handshake in progress).
    Some((String::new(), false))
}

// Live level feedback: parec taps the virtual sink's monitor and the RMS of
// each 100 ms chunk is emitted to the GUI as dB. The loop ends when parec
// exits or the owning scrcpy session leaves the process registry.
#[cfg(unix)]
async fn spawn_mic_level_meter(device: String, app_handle: tauri::AppHandle, generation: u64) {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let monitor = format!("{}.monitor", MIC_SINK_NAME);
        let Some(mut child) = create_command("parec")
            .args([
                "-d",
                &monitor,
                "--format=s16le",
                "--channels=1",
                "--rate=44100",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()
        else {
            crate::events::log("[MIC] Level meter unavailable (parec not found).".to_string());
            return;
        };
        crate::events::log(format!("[MIC] Level meter started (parec on {}).", monitor));
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill().await;
            crate::events::log("[MIC] Level meter ended (no stdout).".to_string());
            return;
        };
        // 88200 bytes/s -> one 100 ms chunk at s16le mono 44.1 kHz.
        let mut buf = vec![0u8; 8820];
        let mut chunks: u32 = 0;
        loop {
            if !session_alive(&app_handle, &device, generation) {
                break;
            }
            // Generous timeout: chunks arrive ~every 100 ms while the stream
            // plays; a 3s drought means parec stalled.
            let read_next = timeout(Duration::from_secs(3), stdout.read(&mut buf));
            let n = match read_next.await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => n,
                Ok(Err(_)) | Err(_) => break,
            };
            chunks += 1;
            let samples: Vec<f64> = buf[..(n / 2) * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    let v = i16::from_le_bytes(*c) as f64 / 32768.0;
                    v * v
                })
                .collect();
            let mean_sq = if samples.is_empty() {
                0.0
            } else {
                samples.iter().sum::<f64>() / samples.len() as f64
            };
            let rms = mean_sq.sqrt();
            let db = if rms < 0.0016 {
                -60.0
            } else {
                (20.0 * rms.log10()).floor().clamp(-60.0, 0.0)
            };
            crate::events::mic_level(db);
        }
        let _ = child.kill().await;
        crate::events::log(format!("[MIC] Level meter ended after {} chunks.", chunks));
    });
}

/// Parses `adb devices` output. Only a ready device is online; `offline` and
/// `unauthorized` entries keep the reconnect loop waiting. `parse_adb_device_line`
/// handles mDNS serials that contain spaces.
async fn device_is_online(adb_exe_path: &str, serial: &str) -> bool {
    let mut command = create_command(adb_exe_path);
    command.arg("devices");
    let Ok(output) = process::output_with_timeout(command, Duration::from_secs(2)).await else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    output.status.success() && crate::adb::device_is_online(&stdout, serial)
}

/// Finalizes a user-initiated stop. Also used while the monitor is in the
/// reconnect backoff, where the child has already exited.
async fn finish_stopped_session(
    runtime: &mut SessionRuntime,
    device: &str,
    requested: Option<(i32, i32)>,
    stop_position: Option<(i32, i32)>,
    publish: bool,
) {
    crate::runtime::finish_stopped_session(
        crate::events::bus(),
        runtime,
        device,
        requested,
        stop_position,
        publish,
    )
    .await;
}

/// Returns true while this generation still owns the session. When the handle
/// was removed by a normal Stop, publishes the stop event; when a replacement
/// bumped the generation, leaves the UI untouched for the new session.
async fn finish_if_session_gone(
    runtime: &mut SessionRuntime,
    app_handle: &tauri::AppHandle,
    device: &str,
    generation: u64,
    requested: Option<(i32, i32)>,
) -> bool {
    let state = app_handle.state::<std::sync::Arc<ScrcpyState>>();
    crate::runtime::finish_if_session_gone(
        crate::events::bus(),
        runtime,
        state.inner(),
        device,
        generation,
        requested,
    )
    .await
}

/// Immutable session context needed to respawn scrcpy with another codec.
/// Replaces the current child with one using `next_codec`, updating the PID,
/// KWin tracking and position samples. Callers handle the error path because
/// they own the stop notification.
async fn retry_audio_codec(
    runtime: &mut SessionRuntime,
    ctx: &RetryContext<'_>,
    origin: RetryOrigin,
    next_codec: &'static str,
) -> Result<(), String> {
    crate::runtime::retry_audio_codec(
        runtime,
        ctx,
        origin,
        next_codec,
        crate::events::bus(),
        &crate::session_adapter::TauriSessionHost,
    )
    .await
}

#[tauri::command]
pub async fn run_scrcpy(
    window: Window,
    state: State<'_, std::sync::Arc<ScrcpyState>>,
    config: ScrcpyConfig,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    validate_scrcpy_config(&config)?;
    let mut config = config;

    // Reserve the device before any await. Without this, two concurrent
    // invocations can both see no session, spawn two children, and let the
    // last registry insert overwrite (and orphan) the first. The same lock
    // lets download_scrcpy refuse a swap while a start is being prepared.
    {
        let _swap_guard = state.binary_swap.lock().await;
        let mut starting = state
            .starting_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !starting.insert(config.device.clone()) {
            return Err("A session for this device is already starting".to_string());
        }
    }

    let video_dir = app_handle
        .path()
        .video_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let exe_path = get_binary_path("scrcpy", config.scrcpy_path.clone());

    // Ask this exact binary which options it supports. If the help output is
    // unrecognizable, detection stays permissive and the real scrcpy error is
    // allowed through.
    let capabilities = crate::capabilities::detect(&exe_path).await;
    if let Err(error) = ensure_mode_supported(&config, &capabilities) {
        state.inner().release_starting_device(&config.device);
        return Err(error);
    }
    if let Some(message) = normalize_mic_source(&mut config, &capabilities) {
        crate::events::log(message);
    }
    if let Some(message) = normalize_mirror_audio(&mut config, &capabilities) {
        crate::events::log(message);
    }
    let capabilities = Arc::new(capabilities);

    // Log the session details for the user
    let mode_label = match config.session_mode.as_str() {
        "camera" => "Camera Mode",
        "desktop" => "Desktop Mode",
        "mic" => "Microphone Bridge",
        _ => "Screen Mirroring",
    };

    crate::events::log(format!("[SYSTEM] Starting {} session...", mode_label));
    if config.session_mode == "mic" {
        crate::events::log(format!("[SYSTEM] Target: {}", config.device));
        let source = config.mic_source.as_deref().unwrap_or("").trim();
        let source = if source.is_empty() { "mic" } else { source };
        let source = validate::normalize_mic_source(source);
        crate::events::log(format!(
            "[SYSTEM] Audio-only bridge (--no-window --no-video --audio-source={}).",
            source
        ));
    } else {
        let res_label = config
            .res
            .as_ref()
            .map(|r| if r == "0" { "Original" } else { r })
            .unwrap_or("Original");
        let bitrate_label = format!("{}Mbps", config.bitrate.unwrap_or(8));
        let fps_label = format!("{}fps", config.fps.unwrap_or(60));
        crate::events::log(format!(
            "[SYSTEM] Target: {} | Config: {} @ {}, {}",
            config.device, res_label, bitrate_label, fps_label
        ));
        if config.session_mode == "mirror" && config.audio_enabled.unwrap_or(true) {
            let source = config
                .mirror_audio_source
                .as_deref()
                .unwrap_or("output")
                .trim();
            let dup = source == "playback" && config.audio_dup.unwrap_or(false);
            crate::events::log(format!(
                "[SYSTEM] Audio source: {}{}.",
                source,
                if dup { " (kept on device)" } else { "" }
            ));
        }
    }

    if config.record.unwrap_or(false) {
        let path = config
            .record_path
            .as_ref()
            .map(|p| if p.is_empty() { "Videos" } else { p })
            .unwrap_or("Videos");
        crate::events::log(format!("[SYSTEM] Recording enabled -> output to {}", path));
    }

    let adb_exe_path = get_binary_path("adb", config.scrcpy_path.clone());
    let server_path = if !exe_path.is_empty() && exe_path != "scrcpy" {
        Path::new(&exe_path)
            .parent()
            .map(|p| p.join("scrcpy-server").to_string_lossy().to_string())
    } else {
        None
    };

    crate::events::log(format!("[SYSTEM] Using scrcpy: {}", exe_path));
    crate::events::log(format!("[SYSTEM] Using adb: {}", adb_exe_path));

    // Bump the logical session generation BEFORE replacing the previous
    // process. Mic sidecars check this once per pass; the monitor task also
    // uses it to avoid deleting a newer session's registry entry.
    let session_generation = {
        let mut generations = state
            .session_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        bump_session_generation(&mut generations, &config.device)
    };

    // One session per device: stop the previous one before spawning the new
    // child, so a fast double-start cannot leave an untracked process behind.
    let existing_to_replace = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&config.device);
    if let Some(existing) = existing_to_replace {
        crate::events::log("[SYSTEM] Replacing the previous session for this device.".to_string());
        let notified = existing.done.notified();
        let _ = existing.stop.send(StopRequest { position: None });
        if tokio::time::timeout(Duration::from_secs(3), notified)
            .await
            .is_err()
        {
            force_kill_pid(existing.pid.load(Ordering::Relaxed));
        }
    }

    // `--turn-screen-off` is applied right after the connection is set up, so
    // the display must already be on when scrcpy starts. Otherwise, on slower
    // devices, scrcpy's own asynchronous power-on can land after its
    // power-off request and leave the phone screen on until the user wakes it
    // manually. Wake it here, after any previous session has been stopped, so
    // the power-off request is the last one to reach the device.
    if wakes_before_screen_off(&config) {
        wake_display_before_screen_off(&adb_exe_path, &config.device).await;
    }

    let audio_enabled = config.audio_enabled.unwrap_or(true);
    let wants_virtual_mic = wants_virtual_mic(&config);

    // Virtual microphone on Linux: create/refresh the shared sink and hand
    // the scrcpy client to it through the libpulse default-sink env. Camera
    // mode can use the same route, so webcam video and a selectable phone
    // microphone come from one session. Other platforms get an honest note.
    // Start-up and teardown take `mic_sink_lock` so the user count cannot be
    // overtaken by a session stopping at the same time.
    #[cfg(unix)]
    let (pulse_sink, uses_mic_sink, uses_mic_default_source): (Option<String>, bool, bool) =
        if wants_virtual_mic {
            let _sink_guard = state.mic_sink_lock.lock().await;
            // Remove a sink left behind by a crashed session. Only the first
            // active user does this; later users reuse the live sink.
            if state.mic_sink_users.load(Ordering::Relaxed) == 0 {
                unload_mic_sink().await;
            }
            let denoise = config.session_mode == "mic" && config.mic_denoise.unwrap_or(false);
            if ensure_mic_sink(denoise).await {
                reserve_mic_sink_user(&state.mic_sink_users);
                let uses_default = config.session_mode == "mic"
                    && config.mic_set_default_source.unwrap_or(false)
                    && ensure_default_mic_source(state.inner()).await;
                (Some(MIC_SINK_NAME.to_string()), true, uses_default)
            } else {
                (None, false, false)
            }
        } else {
            (None, false, false)
        };

    #[cfg(not(unix))]
    let pulse_sink: Option<String> = if wants_virtual_mic {
        crate::events::log(
            "[MIC] Virtual-microphone routing requires Linux; audio goes to the default output."
                .to_string(),
        );
        None
    } else {
        None
    };

    // First attempt: honour the user's audio codec choice. In "auto" mode this
    // launches without --audio-codec, matching the previous default behaviour.
    let (initial_args, skipped) = crate::capabilities::filter_args(
        build_scrcpy_args(&config, video_dir.clone(), None),
        &capabilities,
    );
    if let Some(message) = crate::capabilities::skipped_message(&capabilities, &skipped) {
        crate::events::log(message);
    }

    let spawned = crate::runtime::spawn_scrcpy_streams(
        crate::events::bus().cloned(),
        &exe_path,
        &adb_exe_path,
        server_path.as_deref(),
        &initial_args,
        config.vsync.unwrap_or(true),
        pulse_sink.as_deref(),
    )
    .await;
    let (child, audio_state) = match spawned {
        Ok(spawned) => spawned,
        Err(e) => {
            // The sink was reserved before the spawn attempt; release it
            // before allowing another start for this device.
            #[cfg(unix)]
            if uses_mic_default_source {
                release_default_mic_source(state.inner()).await;
            }
            #[cfg(unix)]
            if uses_mic_sink {
                release_mic_sink(state.inner()).await;
            }
            state.inner().release_starting_device(&config.device);
            return Err(e);
        }
    };

    // Decide whether automatic audio codec fallback should kick in on errors.
    let audio_codec_mode = config.audio_codec.as_deref().unwrap_or("auto").trim();
    let should_auto_fallback = audio_enabled
        && (audio_codec_mode.is_empty() || audio_codec_mode.eq_ignore_ascii_case("auto"));

    let initial_scrcpy_pid = child.id().unwrap_or(0);
    // Only windowed mirror sessions have a persisted position; camera, desktop
    // and fullscreen windows must not inherit it.
    let track_position = tracks_window_position(&config);
    // KDE Wayland: let KWin report (and, on launch, apply) the mirror window
    // position. On X11 this is a no-op and xdotool remains in charge.
    #[cfg(target_os = "linux")]
    if track_position {
        crate::kwin::track_window(initial_scrcpy_pid, config.window_x.zip(config.window_y));
    }
    let session_pid = Arc::new(AtomicU32::new(initial_scrcpy_pid));
    let session_done = Arc::new(Notify::new());
    let (stop_tx, stop_rx) = oneshot::channel();
    state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            config.device.clone(),
            SessionHandle {
                generation: session_generation,
                track_position,
                pid: session_pid.clone(),
                stop: stop_tx,
                done: session_done.clone(),
            },
        );
    state.inner().release_starting_device(&config.device);
    crate::events::status(json!({ "device": config.device, "running": true }));
    // The audio badge starts optimistic; log detection flips it to a warning
    // (or an error with --require-audio) if capture/codec trouble shows up.
    crate::events::audio_status(json!({
        "device": config.device,
        "status": if audio_enabled { "ok" } else { "off" }
    }));

    // Virtual-sink sidecars. Started only after the registry insert above, so
    // their contains_key liveness checks see this session on the first poll
    // (spawning them earlier raced the insert and the loops died instantly).
    // The route watcher is needed for camera sessions too: SDL3 does not
    // reliably honour PULSE_SINK, so the stream is moved after it appears.
    #[cfg(unix)]
    if uses_mic_sink {
        tokio::spawn(spawn_mic_route_watcher(
            config.device.clone(),
            app_handle.clone(),
            session_pid.clone(),
            session_generation,
            if config.session_mode == "camera" {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(2)
            },
            config.session_mode == "mic",
        ));
        if config.session_mode == "mic" {
            tokio::spawn(spawn_mic_level_meter(
                config.device.clone(),
                app_handle.clone(),
                session_generation,
            ));
        }
    }

    // Monitor for exit (and orchestrate the audio codec fallback chain)
    let device_mon = config.device.clone();
    let app_handle_mon = window.app_handle().clone();
    let video_dir_mon = video_dir;
    let exe_path_mon = exe_path;
    let adb_exe_path_mon = adb_exe_path;
    let server_path_mon = server_path;
    let config_mon = config.clone();
    let pulse_sink_mon = pulse_sink;

    // Position we asked scrcpy to open at this launch (if any). Used to learn
    // the decoration offset so the saved position round-trips exactly. See
    // resolve_window_pos_to_persist.
    let requested_pos: Option<(i32, i32)> = match (config_mon.window_x, config_mon.window_y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    };
    // `track_position` (computed before the child was tracked) keeps sampling
    // limited to windowed mirror sessions.

    tokio::spawn(async move {
        let mut runtime = SessionRuntime {
            child,
            audio_state,
            pid: session_pid.clone(),
            first_window_pos: None,
            last_window_pos: None,
            chain_index: 0,
            // Capture and terminal codec failures do not improve on retry, so
            // report each one once per attempt instead of every 2 s tick.
            capture_reported: false,
            codec_terminal_reported: false,
        };
        let retry_ctx = RetryContext {
            device: &device_mon,
            exe_path: &exe_path_mon,
            adb_exe_path: &adb_exe_path_mon,
            server_path: server_path_mon.as_deref(),
            config: &config_mon,
            video_dir: &video_dir_mon,
            pulse_sink: pulse_sink_mon.as_deref(),
            capabilities: &capabilities,
            track_position,
        };
        let mut stop_rx = stop_rx;
        let mut reconnect_attempt: u32 = 0;
        let mut healthy_ticks: u32 = 0;

        let emit_audio_status = |status: &str| {
            crate::events::audio_status(json!({ "device": &device_mon, "status": status }));
        };
        let capture_message = || {
            if config_mon.require_audio.unwrap_or(false) {
                "[AUDIO] Audio capture failed and \"require audio\" is enabled: the session stops. Unlock the device (Android 11) and use Android 12+; some apps block audio capture."
            } else {
                "[AUDIO] Audio capture failed on the device; video keeps running without sound. Android 11+ is required, unlock the device on Android 11, and some apps block audio capture."
            }
        };

        'monitor: loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(2000)) => {}
                request = &mut stop_rx => {
                    let position = match request {
                        Ok(request) => request.position,
                        Err(_) => None,
                    };
                    finish_stopped_session(
                        &mut runtime,
                        &device_mon,
                        requested_pos,
                        position,
                        generation_is_current(&app_handle_mon, &device_mon, session_generation),
                    )
                    .await;
                    break 'monitor;
                }
            }

            // Audio can fail while scrcpy keeps mirroring the video, so the
            // process may never exit. React while it is still running instead
            // of only checking on exit (which made the failure silent).
            if runtime.audio_state.capture_failed() {
                if !runtime.capture_reported {
                    runtime.capture_reported = true;
                    crate::events::log(capture_message().to_string());
                    emit_audio_status(if config_mon.require_audio.unwrap_or(false) {
                        "error"
                    } else {
                        "warning"
                    });
                }
            } else if runtime.audio_state.codec_failed() {
                if !codec_failure_is_terminal(should_auto_fallback, runtime.chain_index) {
                    let next_codec = AUDIO_FALLBACK_CHAIN[runtime.chain_index];
                    match retry_audio_codec(
                        &mut runtime,
                        &retry_ctx,
                        RetryOrigin::Running,
                        next_codec,
                    )
                    .await
                    {
                        Ok(()) => continue,
                        Err(e) => {
                            crate::events::log(format!(
                                "[SYSTEM] Failed to spawn audio retry: {}",
                                e
                            ));
                            emit_audio_status("error");
                            emit_window_pos_and_stop(
                                &device_mon,
                                requested_pos,
                                runtime.first_window_pos,
                                runtime.last_window_pos,
                                SessionEndReason::Error,
                            );
                            break;
                        }
                    }
                } else if !runtime.codec_terminal_reported {
                    runtime.codec_terminal_reported = true;
                    crate::events::log(codec_failure_message(should_auto_fallback).to_string());
                    emit_audio_status("error");
                }
            }

            // Sample the live window position on every tick. This is the only
            // path that can catch a move made right before the user closes the
            // mirror window itself (rather than through stop_scrcpy, which
            // grabs its own guaranteed-fresh sample): SDL destroys the window
            // before the process exits, so by the time that exit is detected
            // below the position is unreadable, and this periodic sample is
            // all that is left to persist. Two seconds is a good trade: on
            // Linux the sample spawns xdotool, on macOS an osascript call,
            // and the stop path takes its own fresh sample anyway.
            if track_position {
                if let Some(pid) = runtime.child.id() {
                    if let Some(pos) = try_capture_window_pos(pid) {
                        if runtime.first_window_pos.is_none() {
                            runtime.first_window_pos = Some(pos);
                        }
                        runtime.last_window_pos = Some(pos);
                    }
                }
            }

            let outcome = match runtime.child.try_wait() {
                Ok(Some(status)) => AttemptOutcome::Exited {
                    description: status.to_string(),
                    success: status.success(),
                },
                Ok(None) => {
                    healthy_ticks = healthy_ticks.saturating_add(1);
                    if healthy_ticks >= 5 {
                        reconnect_attempt = 0;
                    }
                    continue;
                }
                Err(e) => AttemptOutcome::WaitError(e.to_string()),
            };

            match outcome {
                AttemptOutcome::WaitError(e) => {
                    crate::events::log(format!("[SYSTEM] Error waiting for scrcpy: {}", e));
                    emit_audio_status("off");
                    emit_window_pos_and_stop(
                        &device_mon,
                        requested_pos,
                        runtime.first_window_pos,
                        runtime.last_window_pos,
                        SessionEndReason::Error,
                    );
                    break;
                }
                AttemptOutcome::Exited {
                    description: status,
                    success,
                } => {
                    crate::events::log(format!(
                        "[SYSTEM] Scrcpy process exited with status: {}",
                        status
                    ));

                    // Give stream reader tasks a moment to flush remaining stderr
                    // lines (the audio error pattern may still be in flight).
                    tokio::time::sleep(Duration::from_millis(250)).await;

                    let capture_failed = runtime.audio_state.capture_failed();
                    let codec_failed = runtime.audio_state.codec_failed();

                    if capture_failed {
                        // Changing codecs cannot fix a device-side capture
                        // failure, so stop instead of burning the fallback chain.
                        if !runtime.capture_reported {
                            crate::events::log(capture_message().to_string());
                        }
                        emit_audio_status(if config_mon.require_audio.unwrap_or(false) {
                            "error"
                        } else {
                            "warning"
                        });
                        emit_window_pos_and_stop(
                            &device_mon,
                            requested_pos,
                            runtime.first_window_pos,
                            runtime.last_window_pos,
                            SessionEndReason::Error,
                        );
                        break;
                    }

                    if codec_failed {
                        if !codec_failure_is_terminal(should_auto_fallback, runtime.chain_index) {
                            let next_codec = AUDIO_FALLBACK_CHAIN[runtime.chain_index];
                            match retry_audio_codec(
                                &mut runtime,
                                &retry_ctx,
                                RetryOrigin::Exited,
                                next_codec,
                            )
                            .await
                            {
                                Ok(()) => continue,
                                Err(e) => {
                                    crate::events::log(format!(
                                        "[SYSTEM] Failed to spawn audio retry: {}",
                                        e
                                    ));
                                    emit_audio_status("error");
                                    emit_window_pos_and_stop(
                                        &device_mon,
                                        requested_pos,
                                        runtime.first_window_pos,
                                        runtime.last_window_pos,
                                        SessionEndReason::Error,
                                    );
                                    break;
                                }
                            }
                        }

                        if !runtime.codec_terminal_reported {
                            crate::events::log(
                                codec_failure_message(should_auto_fallback).to_string(),
                            );
                        }
                        emit_audio_status("error");
                    } else {
                        emit_audio_status("off");
                    }

                    healthy_ticks = 0;
                    if should_auto_reconnect(
                        config_mon.auto_reconnect.unwrap_or(false),
                        !device_mon.is_empty(),
                        success,
                        capture_failed,
                        codec_failed,
                    ) {
                        if !finish_if_session_gone(
                            &mut runtime,
                            &app_handle_mon,
                            &device_mon,
                            session_generation,
                            requested_pos,
                        )
                        .await
                        {
                            break 'monitor;
                        }
                        if reconnect_attempt == 0 {
                            crate::events::log("[RECONNECT] scrcpy exited unexpectedly; trying to restore the session."
                                    .to_string(),);
                        }
                        loop {
                            let attempt = reconnect_attempt.saturating_add(1);
                            let delay = reconnect_delay(reconnect_attempt);
                            reconnect_attempt = attempt;
                            crate::events::log(format!(
                                "[RECONNECT] Retrying in {}s (attempt {}).",
                                delay.as_secs(),
                                attempt
                            ));
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                request = &mut stop_rx => {
                                    let position = match request {
                                        Ok(request) => request.position,
                                        Err(_) => None,
                                    };
                                    finish_stopped_session(
                                        &mut runtime,
                                        &device_mon,
                                        requested_pos,
                                        position,
                                        generation_is_current(
                                            &app_handle_mon,
                                            &device_mon,
                                            session_generation,
                                        ),
                                    )
                                    .await;
                                    break 'monitor;
                                }
                            }
                            if !finish_if_session_gone(
                                &mut runtime,
                                &app_handle_mon,
                                &device_mon,
                                session_generation,
                                requested_pos,
                            )
                            .await
                            {
                                break 'monitor;
                            }
                            if !device_is_online(&adb_exe_path_mon, &device_mon).await {
                                crate::events::log(format!(
                                    "[RECONNECT] Device {} is not available yet; waiting.",
                                    device_mon
                                ));
                                continue;
                            }
                            // A replacement or a manual stop may have happened
                            // while `adb devices` was running.
                            if !finish_if_session_gone(
                                &mut runtime,
                                &app_handle_mon,
                                &device_mon,
                                session_generation,
                                requested_pos,
                            )
                            .await
                            {
                                break 'monitor;
                            }

                            let reconnect_args =
                                build_scrcpy_args(&config_mon, video_dir_mon.clone(), None);
                            let (reconnect_args, skipped) =
                                crate::capabilities::filter_args(reconnect_args, &capabilities);
                            if let Some(message) =
                                crate::capabilities::skipped_message(&capabilities, &skipped)
                            {
                                crate::events::log(message);
                            }
                            match crate::runtime::spawn_scrcpy_streams(
                                crate::events::bus().cloned(),
                                &exe_path_mon,
                                &adb_exe_path_mon,
                                server_path_mon.as_deref(),
                                &reconnect_args,
                                config_mon.vsync.unwrap_or(true),
                                pulse_sink_mon.as_deref(),
                            )
                            .await
                            {
                                Ok((new_child, new_state)) => {
                                    let new_pid = new_child.id().unwrap_or(0);
                                    #[cfg(target_os = "linux")]
                                    if track_position {
                                        crate::kwin::stop_tracking(
                                            runtime.pid.load(Ordering::Relaxed),
                                        );
                                        crate::kwin::track_window(
                                            new_pid,
                                            config_mon.window_x.zip(config_mon.window_y),
                                        );
                                    }
                                    runtime.pid.store(new_pid, Ordering::Relaxed);
                                    runtime.child = new_child;
                                    runtime.audio_state = new_state;
                                    runtime.first_window_pos = None;
                                    runtime.last_window_pos = None;
                                    runtime.capture_reported = false;
                                    runtime.codec_terminal_reported = false;
                                    runtime.chain_index = 0;
                                    emit_audio_status(if audio_enabled { "ok" } else { "off" });
                                    crate::events::log(format!(
                                        "[RECONNECT] Session restored after {} attempt(s).",
                                        attempt
                                    ));
                                    break;
                                }
                                Err(e) => {
                                    crate::events::log(format!(
                                        "[RECONNECT] Attempt {} failed: {}",
                                        attempt, e
                                    ));
                                }
                            }
                        }
                        continue 'monitor;
                    }

                    // A terminal codec failure is an error even when the
                    // process happens to exit with status 0.
                    let reason = if codec_failed {
                        SessionEndReason::Error
                    } else {
                        SessionEndReason::Exited { success }
                    };
                    emit_window_pos_and_stop(
                        &device_mon,
                        requested_pos,
                        runtime.first_window_pos,
                        runtime.last_window_pos,
                        reason,
                    );
                    break;
                }
            }
        }

        // Remove only this generation's entry: a newer session may already own
        // the key after a replacement.
        {
            let state_lock = app_handle_mon.state::<std::sync::Arc<ScrcpyState>>();
            let mut sessions = state_lock
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if sessions.get(&device_mon).map(|s| s.generation) == Some(session_generation) {
                sessions.remove(&device_mon);
            }
        }
        // Restore the default source before the sink is unloaded. A replacement
        // has its own reservation, so both counts decide when each step runs.
        #[cfg(unix)]
        if uses_mic_default_source {
            let state_lock = app_handle_mon.state::<std::sync::Arc<ScrcpyState>>();
            release_default_mic_source(state_lock.inner()).await;
        }
        // Release this session's sink reservation even when a replacement
        // already bumped the generation: the replacement has its own
        // reservation, so the count decides when the sink can be unloaded.
        #[cfg(unix)]
        if uses_mic_sink {
            let state_lock = app_handle_mon.state::<std::sync::Arc<ScrcpyState>>();
            release_mic_sink(state_lock.inner()).await;
        }
        #[cfg(target_os = "linux")]
        if track_position {
            crate::kwin::stop_tracking(runtime.pid.load(Ordering::Relaxed));
        }
        runtime.pid.store(0, Ordering::Relaxed);
        session_done.notify_one();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::sanitize_filename_component;
    use crate::runtime::terminate_child;
    use crate::session_generation_is_current;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[test]
    fn session_generation_bumps_per_run_and_rejects_stale_sidecars() {
        let mut generations = HashMap::new();
        let first = bump_session_generation(&mut generations, "device1");
        assert_eq!(first, 1);
        assert!(session_generation_is_current(
            &generations,
            "device1",
            first
        ));

        // A replacement bumps the generation: the previous session's sidecars
        // are stale even though the registry key gets reused right away.
        let second = bump_session_generation(&mut generations, "device1");
        assert_eq!(second, 2);
        assert!(!session_generation_is_current(
            &generations,
            "device1",
            first
        ));
        assert!(session_generation_is_current(
            &generations,
            "device1",
            second
        ));

        // Generations are per device.
        assert_eq!(bump_session_generation(&mut generations, "device2"), 1);
        assert!(session_generation_is_current(
            &generations,
            "device1",
            second
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sink_input_filter_requires_the_scrcpy_pid() {
        // A custom binary can call itself "scrcpy-custom"; the PID is still
        // the authoritative link to the session.
        let ours = "Sink Input #12\n  Properties:\n    application.name = \"scrcpy-custom\"\n    application.process.id = \"4242\"\n";
        let other = "Sink Input #13\n  Properties:\n    application.name = \"scrcpy\"\n    application.process.id = \"9999\"\n";
        assert!(sink_input_is_scrcpy_pid(ours, 4242));
        assert!(!sink_input_is_scrcpy_pid(other, 4242));
        assert!(!sink_input_is_scrcpy_pid(ours, 0));
    }

    #[test]
    fn session_generation_rejects_unknown_devices() {
        let generations = HashMap::new();
        assert!(!session_generation_is_current(&generations, "device1", 1));
    }

    #[cfg(unix)]
    async fn pid_alive(pid: u32) -> bool {
        StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_child_escalates_when_sigterm_is_ignored() {
        // `exec` keeps the ignored SIGTERM disposition, so only the forced
        // kill can end this process. The readiness line proves the trap is
        // installed before the test signals: killing too early can beat the
        // shell's startup and make the test pass or fail by luck.
        let mut child = TokioCommand::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 30"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let pid = child.id().expect("pid");
        let stdout = child.stdout.take().expect("stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .await
            .expect("readiness line");
        assert_eq!(line.trim(), "ready");

        let graceful = terminate_child(&mut child, Duration::from_millis(300)).await;

        assert!(!graceful);
        assert!(!pid_alive(pid).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn force_kill_pid_terminates_the_process() {
        let mut child = TokioCommand::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let pid = child.id().expect("pid");

        force_kill_pid(pid);

        let status = child.wait().await.expect("wait");
        assert!(!status.success());
        assert!(!pid_alive(pid).await);
    }

    fn base_config(session_mode: &str) -> ScrcpyConfig {
        ScrcpyConfig {
            device: "device1".to_string(),
            session_mode: session_mode.to_string(),
            bitrate: None,
            fps: None,
            stay_awake: None,
            turn_off: None,
            audio_enabled: None,
            audio_codec: None,
            mirror_audio_source: None,
            audio_dup: None,
            require_audio: None,
            always_on_top: None,
            fullscreen: None,
            borderless: None,
            record: None,
            record_path: None,
            scrcpy_path: None,
            otg_pure: None,
            camera_facing: None,
            camera_id: None,
            codec: None,
            camera_ar: None,
            camera_high_speed: None,
            vd_width: None,
            vd_height: None,
            vd_dpi: None,
            rotation: None,
            res: None,
            hid_keyboard: None,
            hid_mouse: None,
            render_driver: None,
            flex_display: None,
            camera_torch: None,
            camera_zoom: None,
            camera_mic_sink: None,
            background_color: None,
            keep_active: None,
            auto_reconnect: None,
            vsync: None,
            mic_source: None,
            mic_codec: None,
            mic_bitrate: None,
            mic_denoise: None,
            mic_set_default_source: None,
            window_x: None,
            window_y: None,
            ignore_video_encoder_constraints: None,
        }
    }

    #[test]
    fn camera_can_route_its_microphone_into_the_virtual_sink() {
        let mut camera = base_config("camera");
        assert!(!wants_virtual_mic(&camera));

        camera.camera_mic_sink = Some(true);
        assert!(wants_virtual_mic(&camera));

        // Turning audio off removes the route even when the saved toggle is on.
        camera.audio_enabled = Some(false);
        assert!(!wants_virtual_mic(&camera));

        assert!(!wants_virtual_mic(&base_config("mirror")));
        assert!(wants_virtual_mic(&base_config("mic")));
    }

    #[cfg(unix)]
    #[test]
    fn virtual_sink_unloads_only_after_the_last_user_releases_it() {
        let users = AtomicUsize::new(0);
        // A stray release must not underflow or ask for an unload.
        assert!(!release_mic_sink_user(&users));
        assert_eq!(users.load(Ordering::Relaxed), 0);

        reserve_mic_sink_user(&users);
        reserve_mic_sink_user(&users);
        assert!(!release_mic_sink_user(&users));
        assert_eq!(users.load(Ordering::Relaxed), 1);
        assert!(release_mic_sink_user(&users));
        assert_eq!(users.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_validate_scrcpy_config_accepts_defaults_and_rejects_bad_values() {
        assert!(validate_scrcpy_config(&base_config("mirror")).is_ok());

        let mut config = base_config("mirror");
        config.codec = Some("vp9".to_string());
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mirror");
        config.device = "-s".to_string();
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mic");
        config.mic_codec = Some("flac".to_string());
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mirror");
        config.fps = Some(0);
        assert!(validate_scrcpy_config(&config).is_err());

        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("speaker".to_string());
        assert!(validate_scrcpy_config(&config).is_err());
    }

    #[test]
    fn test_build_scrcpy_args_mirror_defaults() {
        let config = base_config("mirror");
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"-s".to_string()));
        assert!(args.contains(&"device1".to_string()));
        assert!(args.contains(&"--video-codec=h264".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_turn_off_allows_wake_before_screen_off() {
        let mut config = base_config("mirror");
        config.turn_off = Some(true);

        let args = build_scrcpy_args(&config, None, None);

        assert!(args.contains(&"--turn-screen-off".to_string()));
        assert!(!args.contains(&"--no-power-on".to_string()));
    }

    #[test]
    fn test_wakes_before_screen_off_only_for_windowed_control_sessions() {
        let mut config = base_config("mirror");
        assert!(!wakes_before_screen_off(&config));

        config.turn_off = Some(true);
        assert!(wakes_before_screen_off(&config));

        // No serial means scrcpy picks the device itself; do not run a wake
        // command against an empty `-s`.
        config.device.clear();
        assert!(!wakes_before_screen_off(&config));

        config.device = "device1".to_string();
        for mode in ["camera", "mic"] {
            config.session_mode = mode.to_string();
            assert!(!wakes_before_screen_off(&config), "{mode}");
        }

        config.session_mode = "desktop".to_string();
        assert!(wakes_before_screen_off(&config));
    }

    #[test]
    fn test_build_scrcpy_args_camera_mode() {
        let mut config = base_config("camera");
        config.fps = Some(30);
        config.camera_facing = Some("front".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--video-source=camera".to_string()));
        assert!(args.contains(&"--camera-facing=front".to_string()));
        assert!(args.contains(&"--camera-fps".to_string()));
        assert!(args.contains(&"30".to_string()));
    }

    #[test]
    fn test_camera_mode_forwards_the_phone_microphone() {
        let config = base_config("camera");
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic".to_string()));

        // Disabling audio must not leave an orphan --audio-source.
        let mut muted = base_config("camera");
        muted.audio_enabled = Some(false);
        let args = build_scrcpy_args(&muted, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));

        // Mirror keeps the default device-output capture.
        let mirror = base_config("mirror");
        let args = build_scrcpy_args(&mirror, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));
    }

    #[test]
    fn test_mirror_audio_source_and_dup_flags() {
        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=playback".to_string()));
        assert!(args.contains(&"--audio-dup".to_string()));

        // --audio-dup is only valid with playback.
        config.mirror_audio_source = Some("mic".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic".to_string()));
        assert!(!args.contains(&"--audio-dup".to_string()));

        // The default source is implicit, matching scrcpy's default.
        config.mirror_audio_source = Some("output".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));
        assert!(!args.contains(&"--audio-dup".to_string()));

        config.mirror_audio_source = Some("voice-performance".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=voice-performance".to_string()));

        // Disabling audio removes both flags.
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        config.audio_enabled = Some(false);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--audio-source")));
        assert!(!args.contains(&"--audio-dup".to_string()));
    }

    #[test]
    fn test_normalize_mirror_audio_falls_back_on_unsupported_values() {
        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("voice-call".to_string());
        let caps = Capabilities::for_test(&["audio-source"], &["output", "playback", "mic"]);
        assert!(normalize_mirror_audio(&mut config, &caps).is_some());
        assert_eq!(config.mirror_audio_source.as_deref(), Some("output"));

        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        let caps = Capabilities::for_test(&["audio-source"], &["output", "playback"]);
        assert!(normalize_mirror_audio(&mut config, &caps).is_some());
        assert_eq!(config.audio_dup, Some(false));

        let mut config = base_config("mirror");
        config.mirror_audio_source = Some("playback".to_string());
        config.audio_dup = Some(true);
        let caps = Capabilities::for_test(&["audio-source", "audio-dup"], &["output", "playback"]);
        assert!(normalize_mirror_audio(&mut config, &caps).is_none());
        assert_eq!(config.audio_dup, Some(true));
    }

    #[test]
    fn test_build_scrcpy_args_bitrate_and_fps() {
        let mut config = base_config("mirror");
        config.bitrate = Some(8);
        config.fps = Some(60);

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--video-bit-rate".to_string()));
        assert!(args.contains(&"8M".to_string()));
        assert!(args.contains(&"--max-fps".to_string()));
        assert!(args.contains(&"60".to_string()));
    }

    #[test]
    fn test_audio_codec_auto_adds_no_flag() {
        let mut config = base_config("mirror");
        config.audio_codec = Some("auto".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--audio-codec")));
    }

    #[test]
    fn test_audio_codec_manual_adds_flag() {
        let mut config = base_config("mirror");
        config.audio_codec = Some("aac".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-codec=aac".to_string()));
    }

    #[test]
    fn test_audio_codec_override_takes_precedence() {
        let mut config = base_config("mirror");
        config.audio_codec = Some("auto".to_string());

        let args = build_scrcpy_args(&config, None, Some("flac"));
        assert!(args.contains(&"--audio-codec=flac".to_string()));
    }

    #[test]
    fn test_audio_codec_skipped_when_audio_disabled() {
        let mut config = base_config("mirror");
        config.audio_enabled = Some(false);
        config.audio_codec = Some("aac".to_string());

        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--audio-codec")));
    }

    #[test]
    fn test_require_audio_flag_is_opt_in_and_only_with_audio() {
        let config = base_config("mirror");
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a == "--require-audio"));

        let mut config = base_config("mirror");
        config.require_audio = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.iter().any(|a| a == "--require-audio"));

        // Contradictory combination must never reach scrcpy.
        let mut config = base_config("mirror");
        config.audio_enabled = Some(false);
        config.require_audio = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-audio".to_string()));
        assert!(!args.iter().any(|a| a == "--require-audio"));

        // Mic mode has its own minimal arg set (audio is mandatory there).
        let mut config = base_config("mic");
        config.require_audio = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a == "--require-audio"));
    }

    #[test]
    fn test_capability_guard_protects_modes_and_audio() {
        let caps = Capabilities::for_test(
            &[
                "video-codec",
                "audio-codec",
                "video-source",
                "camera-size",
                "audio-source",
                "no-window",
                "no-video",
                "new-display",
                "require-audio",
            ],
            &["mic", "mic-voice-communication"],
        );
        for mode in ["mirror", "camera", "mic", "desktop"] {
            assert!(
                ensure_mode_supported(&base_config(mode), &caps).is_ok(),
                "{mode}"
            );
        }

        let old = Capabilities::for_test(&["video-codec", "audio-codec", "audio-source"], &["mic"]);
        let camera = ensure_mode_supported(&base_config("camera"), &old).unwrap_err();
        assert!(camera.contains("Camera mode"));
        assert!(camera.contains("--video-source"));
        let mic = ensure_mode_supported(&base_config("mic"), &old).unwrap_err();
        assert!(mic.contains("--no-window"));
        let desktop = ensure_mode_supported(&base_config("desktop"), &old).unwrap_err();
        assert!(desktop.contains("--new-display"));
        assert!(ensure_mode_supported(&base_config("mirror"), &old).is_ok());

        let no_audio = Capabilities::for_test(&["video-codec"], &[]);
        let audio = ensure_mode_supported(&base_config("mirror"), &no_audio).unwrap_err();
        assert!(audio.contains("Audio forwarding"));

        let mut require = base_config("mirror");
        require.require_audio = Some(true);
        let audio_only = Capabilities::for_test(&["video-codec", "audio-codec"], &[]);
        let require_err = ensure_mode_supported(&require, &audio_only).unwrap_err();
        assert!(require_err.contains("Require audio"));

        let mut hid = base_config("mirror");
        hid.hid_keyboard = Some(true);
        let no_hid = Capabilities::for_test(&["video-codec", "audio-codec"], &[]);
        let hid_err = ensure_mode_supported(&hid, &no_hid).unwrap_err();
        assert!(hid_err.contains("--keyboard"));

        // The audio-only bridge ignores HID flags, so they must not block it.
        let mut mic_hid = base_config("mic");
        mic_hid.hid_keyboard = Some(true);
        assert!(ensure_mode_supported(&mic_hid, &caps).is_ok());
    }

    #[test]
    fn test_mic_source_falls_back_when_denoise_is_unsupported() {
        let mut config = base_config("mic");
        config.mic_source = Some("mic-voice-communication".to_string());
        let old = Capabilities::for_test(&["audio-source"], &["mic"]);
        assert!(normalize_mic_source(&mut config, &old).is_some());
        assert_eq!(config.mic_source.as_deref(), Some("mic"));

        let mut config = base_config("mic");
        config.mic_source = Some("mic-voice-communication".to_string());
        let new = Capabilities::for_test(&["audio-source"], &["mic", "mic-voice-communication"]);
        assert!(normalize_mic_source(&mut config, &new).is_none());
        assert_eq!(
            config.mic_source.as_deref(),
            Some("mic-voice-communication")
        );
    }

    #[test]
    fn test_build_scrcpy_args_mic_defaults() {
        let config = base_config("mic");
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-window".to_string()));
        assert!(args.contains(&"--no-video".to_string()));
        assert!(args.contains(&"--audio-source=mic".to_string()));
        assert!(args.contains(&"--audio-codec=opus".to_string()));
        assert!(args.contains(&"--audio-bit-rate=128k".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_mic_uses_voice_communication_source() {
        let mut config = base_config("mic");
        config.mic_source = Some("mic-voice-communication".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic-voice-communication".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_mic_normalizes_legacy_source() {
        let mut config = base_config("mic");
        config.mic_source = Some("voice-communication".to_string());
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic-voice-communication".to_string()));
        assert!(!args.contains(&"--audio-source=voice-communication".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_mic_codec_override_wins() {
        let mut config = base_config("mic");
        config.mic_codec = Some("aac".to_string());
        let args = build_scrcpy_args(&config, None, Some("opus"));
        assert!(args.contains(&"--audio-codec=opus".to_string()));
        assert!(!args.contains(&"--audio-codec=aac".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_window_position() {
        let mut config = base_config("mirror");
        config.window_x = Some(100);
        config.window_y = Some(200);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--window-x=100".to_string()));
        assert!(args.contains(&"--window-y=200".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_window_position_negative_coords() {
        // Multi-monitor: a display left of / above the primary yields negative
        // coordinates, which scrcpy accepts.
        let mut config = base_config("mirror");
        config.window_x = Some(-1920);
        config.window_y = Some(0);
        let args = build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--window-x=-1920".to_string()));
        assert!(args.contains(&"--window-y=0".to_string()));
    }

    #[test]
    fn test_build_scrcpy_args_window_position_skipped_in_fullscreen() {
        let mut config = base_config("mirror");
        config.window_x = Some(100);
        config.window_y = Some(200);
        config.fullscreen = Some(true);
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--window-x")));
        assert!(!args.iter().any(|a| a.starts_with("--window-y")));
    }

    #[test]
    fn test_build_scrcpy_args_window_position_only_mirror() {
        // Camera (and desktop) sessions must not receive the mirror window's
        // remembered position, it is scoped to mirror mode.
        let mut config = base_config("camera");
        config.window_x = Some(100);
        config.window_y = Some(200);
        let args = build_scrcpy_args(&config, None, None);
        assert!(!args.iter().any(|a| a.starts_with("--window-x")));
        assert!(!args.iter().any(|a| a.starts_with("--window-y")));
    }

    #[test]
    fn test_parse_shell_var_int() {
        let text = "WINDOW=12345\nX=959\nY=24\nWIDTH=428\nHEIGHT=928\n";
        assert_eq!(parse_shell_var_int(text, "X"), Some(959));
        assert_eq!(parse_shell_var_int(text, "Y"), Some(24));
        assert_eq!(parse_shell_var_int(text, "MISSING"), None);
    }

    #[test]
    fn test_window_position_tracking_is_windowed_mirror_only() {
        assert!(tracks_window_position(&base_config("mirror")));

        let mut fullscreen = base_config("mirror");
        fullscreen.fullscreen = Some(true);
        assert!(!tracks_window_position(&fullscreen));

        for mode in ["camera", "desktop", "mic"] {
            assert!(!tracks_window_position(&base_config(mode)));
        }
    }

    #[test]
    fn test_sanitize_filename_component_allowlists() {
        assert_eq!(
            sanitize_filename_component("192.168.1.50:5555"),
            "192.168.1.50_5555"
        );
        assert_eq!(
            sanitize_filename_component("emulator-5554"),
            "emulator-5554"
        );
        assert_eq!(sanitize_filename_component("../../evil"), "_.._evil");
        assert_eq!(sanitize_filename_component("..."), "device");
        assert_eq!(sanitize_filename_component(""), "device");
    }
}

/// Applies the persisted background preference and (re)builds the tray menu
/// with the labels for the active locale. Keeping the flag in backend state
/// lets the window close handler decide without a round-trip to the webview.
#[tauri::command]
pub fn configure_background_mode(
    app: tauri::AppHandle,
    state: State<'_, std::sync::Arc<ScrcpyState>>,
    enabled: bool,
    show_label: String,
    quit_label: String,
) -> Result<(), String> {
    // Turning background mode off must never depend on labels: otherwise a
    // bad translation could leave the backend hiding a window without a tray.
    let labels = if enabled {
        crate::tray::TrayLabels {
            show: validate::tray_label(&show_label)?,
            quit: validate::tray_label(&quit_label)?,
        }
    } else {
        crate::tray::TrayLabels {
            show: String::new(),
            quit: String::new(),
        }
    };
    match crate::tray::configure(&app, enabled, &labels) {
        Ok(()) => {
            state.background_mode.store(enabled, Ordering::Relaxed);
            Ok(())
        }
        Err(error) if !enabled => {
            // Never keep hiding the window if the preference was turned off,
            // even if hiding the tray icon itself failed.
            state.background_mode.store(false, Ordering::Relaxed);
            Err(error)
        }
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub async fn stop_scrcpy(
    state: State<'_, std::sync::Arc<ScrcpyState>>,
    device: String,
) -> Result<(), String> {
    crate::session::stop_session(
        state.inner(),
        &device,
        &crate::session_adapter::TauriSessionHost,
    )
    .await
}

// App exit teardown: kill every registered child (mirror, camera, mic
// bridge) and unload the mic sink, so closing the app cannot leave audio
// or a mirror running in the background. This handler is synchronous, so it
// uses SIGKILL/TerminateProcess instead of waiting for a graceful stop.
pub fn teardown_sessions(app_handle: &tauri::AppHandle) {
    let state = app_handle.state::<std::sync::Arc<ScrcpyState>>();
    let handles: Vec<SessionHandle> = {
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.drain().map(|(_, handle)| handle).collect()
    };
    for handle in handles {
        let pid = handle.pid.load(Ordering::Relaxed);
        #[cfg(target_os = "linux")]
        if handle.track_position {
            crate::kwin::stop_tracking(pid);
        }
        let _ = handle.stop.send(StopRequest { position: None });
        force_kill_pid(pid);
    }
    #[cfg(unix)]
    {
        restore_default_source_sync(state.inner());
        teardown_mic_sink();
    }
}

// Synchronous sink teardown for process exit: event handlers run outside
// the async runtime, so this cannot reuse the window-based helper.
#[cfg(unix)]
fn teardown_mic_sink() {
    let Ok(modules) = StdCommand::new("pactl")
        .args(["list", "short", "modules"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    else {
        return;
    };
    let module_idx = modules
        .lines()
        .find(|l| l.contains(&format!("sink_name={}", MIC_SINK_NAME)))
        .and_then(|l| l.split('\t').next().map(str::to_string));
    if let Some(idx) = module_idx {
        let _ = StdCommand::new("pactl")
            .args(["unload-module", &idx])
            .output();
    }
}

#[tauri::command]
pub async fn download_scrcpy(app_handle: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;

    let install_path = match crate::download::install_dir() {
        Some(dir) => dir.to_path_buf(),
        None => crate::download::init_install_dir(&app_handle),
    };
    let state = app_handle.state::<std::sync::Arc<ScrcpyState>>();
    crate::download::download_latest(crate::events::bus(), state.inner(), &install_path).await
}

#[tauri::command]
pub async fn get_videos_dir(app_handle: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    app_handle
        .path()
        .video_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_report(
    app_handle: tauri::AppHandle,
    content: String,
    name: String,
) -> Result<String, String> {
    use tauri::Manager;

    let downloads = app_handle
        .path()
        .download_dir()
        .map_err(|error| error.to_string())?;
    crate::report::write_report(&downloads, &content, &name)
}

#[tauri::command]
pub async fn get_scrcpy_bin_dir() -> Result<String, String> {
    crate::paths::scrcpy_bin_dir()
}

#[tauri::command]
pub async fn check_scrcpy_update(custom_path: Option<String>) -> serde_json::Value {
    crate::update::check_scrcpy_update(custom_path.as_deref()).await
}

// "View release" from the update modal: open the releases page in the
// user's browser. Rust-side open keeps it outside the webview popup rules;
// the HTTPS check keeps the shell handler from being abused with anything
// else even if a future caller passes this command a value.
#[tauri::command]
pub async fn open_app_release(app: tauri::AppHandle) -> Result<(), String> {
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_url(
            "https://github.com/yanhenrique-dev/OpenScrcpy/releases/latest",
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

// One-click USB -> wireless switch: read the phone's Wi-Fi IP from its own
// routing table, lift adbd into TCPIP mode, then point the daemon back at
// the phone over the network. Every step is logged to the console like
// the rest of the adb surface, so a failure is diagnosable in-app.
#[tauri::command]
pub async fn goto_wireless(
    device: String,
    custom_path: Option<String>,
) -> Result<serde_json::Value, String> {
    crate::wireless::goto_wireless(crate::events::log_line, &device, custom_path.as_deref()).await
}
