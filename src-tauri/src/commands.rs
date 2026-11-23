use crate::config::ScrcpyConfig;
use crate::validate;
use crate::ScrcpyState;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Manager, State};

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

/// Re-centres the running mirror window for `device` on the primary screen.
/// Bound to a global OS shortcut (Ctrl+Alt+Shift+C, see `shortcuts.rs`) so a
/// window whose saved position ends up off-screen (see
/// [`open_scrcpy_platform::window::recenter_window`]) can be recovered without
/// being able to see or focus it directly. A no-op if `device` has no tracked
/// process (e.g. nothing is running).
pub fn recenter_device(state: &ScrcpyState, device: &str) {
    let pid = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device)
        .map(|s| s.pid.load(Ordering::Relaxed))
        .filter(|pid| *pid != 0);
    if let Some(pid) = pid {
        open_scrcpy_platform::window::recenter_window(pid);
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

/// Starts (or replaces) the `scrcpy` session for `config.device`.
///
/// The command is a thin adapter: it resolves the desktop video directory and
/// the native window bridge, then hands the start to the UI-agnostic monitor.
#[tauri::command]
pub async fn run_scrcpy(
    state: State<'_, Arc<ScrcpyState>>,
    config: ScrcpyConfig,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    use tauri::Manager;

    let video_dir = app_handle
        .path()
        .video_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    open_scrcpy_core::monitor::run_session(open_scrcpy_core::monitor::SessionRequest {
        state: state.inner().clone(),
        config,
        video_dir,
        events: crate::events::bus().cloned(),
        host: Arc::new(crate::session_adapter::TauriSessionHost),
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{build_scrcpy_args, sanitize_filename_component};
    #[cfg(unix)]
    use crate::runtime::terminate_child;
    use crate::{bump_session_generation, session_generation_is_current};
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::process::{Command as StdCommand, Stdio};
    #[cfg(unix)]
    use std::time::Duration;
    #[cfg(unix)]
    use tokio::io::{AsyncBufReadExt, BufReader};
    #[cfg(unix)]
    use tokio::process::Command as TokioCommand;

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

        crate::process::force_kill_pid(pid);

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
// or a mirror running in the background. Delegates to the core so the GPUI
// shell performs exactly the same teardown.
pub fn teardown_sessions(app_handle: &tauri::AppHandle) {
    let state = app_handle.state::<std::sync::Arc<ScrcpyState>>();
    open_scrcpy_core::session::teardown_sessions(
        state.inner(),
        &crate::session_adapter::TauriSessionHost,
    );
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
