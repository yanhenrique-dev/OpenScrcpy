//! The session monitor: start `scrcpy`, keep it alive and finalize it.
//!
//! This module owns the policy the Tauri command used to carry inline:
//! capability normalization, the virtual-mic reservation, audio-codec
//! fallback, reconnection with backoff and the final stop payload. Native
//! window tracking and process kills arrive through [`SessionHost`], and the
//! UI hears everything through the [`EventBus`].

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use serde_json::json;
use tokio::sync::{oneshot, Notify};
use tokio::time::Duration;

use crate::audio::{codec_failure_is_terminal, codec_failure_message, AUDIO_FALLBACK_CHAIN};
use crate::capabilities::Capabilities;
use crate::config::{build_scrcpy_args, ScrcpyConfig};
use crate::events::EventBus;
use crate::process::{create_command, output_with_timeout};
use crate::runtime::{RetryContext, RetryOrigin, SessionHost, SessionRuntime};
use crate::session::{
    display_power_is_on, reconnect_delay, should_auto_reconnect, SessionEndReason,
};
use crate::state::{bump_session_generation, ScrcpyState, SessionHandle, StopRequest};
use crate::validate;

// Screen-off startup: the ADB wake must be quick, but a wireless device can
// take a little longer to report the new display state.
const ADB_WAKE_TIMEOUT: Duration = Duration::from_secs(5);
const DISPLAY_POWER_QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const DISPLAY_WAKE_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const DISPLAY_WAKE_POLL_INTERVAL: Duration = Duration::from_millis(150);

fn bus_log(events: Option<&EventBus>, line: impl Into<String>) {
    if let Some(bus) = events {
        bus.log(line);
    }
}

fn bus_status(events: Option<&EventBus>, payload: serde_json::Value) {
    if let Some(bus) = events {
        bus.status(payload);
    }
}

fn bus_audio_status(events: Option<&EventBus>, payload: serde_json::Value) {
    if let Some(bus) = events {
        bus.audio_status(payload);
    }
}

fn get_binary_path(binary_name: &str, custom_folder: Option<String>) -> String {
    crate::paths::binary_path(binary_name, custom_folder.as_deref())
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
    match output_with_timeout(cmd, DISPLAY_POWER_QUERY_TIMEOUT).await {
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
async fn wake_display_before_screen_off(
    events: Option<&EventBus>,
    adb_exe_path: &str,
    device: &str,
) {
    bus_log(
        events,
        "[SYSTEM] Screen off requested: waking the device display...",
    );

    let mut wake = create_command(adb_exe_path);
    wake.args(["-s", device, "shell", "input", "keyevent", "224"]);
    let wake_ok = output_with_timeout(wake, ADB_WAKE_TIMEOUT)
        .await
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !wake_ok {
        bus_log(
            events,
            "[SYSTEM] Could not wake the device display; scrcpy will try to power it on.",
        );
        return;
    }

    let deadline = tokio::time::Instant::now() + DISPLAY_WAKE_SETTLE_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if read_display_power_state(adb_exe_path, device).await {
            bus_log(events, "[SYSTEM] Device display is awake.");
            return;
        }
        tokio::time::sleep(DISPLAY_WAKE_POLL_INTERVAL).await;
    }

    bus_log(
        events,
        "[SYSTEM] Device display did not report as awake; scrcpy will try to power it on.",
    );
}

enum AttemptOutcome {
    Exited { description: String, success: bool },
    WaitError(String),
}

/// Parses `adb devices` output. Only a ready device is online; `offline` and
/// `unauthorized` entries keep the reconnect loop waiting. `parse_adb_device_line`
/// handles mDNS serials that contain spaces.
async fn device_is_online(adb_exe_path: &str, serial: &str) -> bool {
    let mut command = create_command(adb_exe_path);
    command.arg("devices");
    let Ok(output) = output_with_timeout(command, Duration::from_secs(2)).await else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    output.status.success() && crate::adb::device_is_online(&stdout, serial)
}

pub struct SessionRequest {
    pub state: Arc<ScrcpyState>,
    pub config: ScrcpyConfig,
    /// Output directory for `--record`, resolved by the shell (desktop video
    /// dir), so the core never asks a UI toolkit for a path.
    pub video_dir: Option<String>,
    pub events: Option<EventBus>,
    pub host: Arc<dyn SessionHost>,
}

/// Starts `scrcpy` for one device and owns it until it stops.
///
/// This is the body the `run_scrcpy` IPC command used to own. The shell only
/// resolves native details (binary paths, video dir, window tracking); every
/// decision below is domain policy.
pub async fn run_session(request: SessionRequest) -> Result<(), String> {
    let SessionRequest {
        state,
        mut config,
        video_dir,
        events,
        host,
    } = request;
    crate::config::validate_scrcpy_config(&config)?;

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

    let exe_path = get_binary_path("scrcpy", config.scrcpy_path.clone());

    // Ask this exact binary which options it supports. If the help output is
    // unrecognizable, detection stays permissive and the real scrcpy error is
    // allowed through.
    let capabilities = crate::capabilities::detect(&exe_path).await;
    if let Err(error) = ensure_mode_supported(&config, &capabilities) {
        state.release_starting_device(&config.device);
        return Err(error);
    }
    if let Some(message) = normalize_mic_source(&mut config, &capabilities) {
        bus_log(events.as_ref(), message);
    }
    if let Some(message) = normalize_mirror_audio(&mut config, &capabilities) {
        bus_log(events.as_ref(), message);
    }
    let capabilities = Arc::new(capabilities);

    // Log the session details for the user
    let mode_label = match config.session_mode.as_str() {
        "camera" => "Camera Mode",
        "desktop" => "Desktop Mode",
        "mic" => "Microphone Bridge",
        _ => "Screen Mirroring",
    };

    bus_log(
        events.as_ref(),
        format!("[SYSTEM] Starting {} session...", mode_label),
    );
    if config.session_mode == "mic" {
        bus_log(
            events.as_ref(),
            format!("[SYSTEM] Target: {}", config.device),
        );
        let source = config.mic_source.as_deref().unwrap_or("").trim();
        let source = if source.is_empty() { "mic" } else { source };
        let source = validate::normalize_mic_source(source);
        bus_log(
            events.as_ref(),
            format!(
                "[SYSTEM] Audio-only bridge (--no-window --no-video --audio-source={}).",
                source
            ),
        );
    } else {
        let res_label = config
            .res
            .as_ref()
            .map(|r| if r == "0" { "Original" } else { r })
            .unwrap_or("Original");
        let bitrate_label = format!("{}Mbps", config.bitrate.unwrap_or(8));
        let fps_label = format!("{}fps", config.fps.unwrap_or(60));
        bus_log(
            events.as_ref(),
            format!(
                "[SYSTEM] Target: {} | Config: {} @ {}, {}",
                config.device, res_label, bitrate_label, fps_label
            ),
        );
        if config.session_mode == "mirror" && config.audio_enabled.unwrap_or(true) {
            let source = config
                .mirror_audio_source
                .as_deref()
                .unwrap_or("output")
                .trim();
            let dup = source == "playback" && config.audio_dup.unwrap_or(false);
            bus_log(
                events.as_ref(),
                format!(
                    "[SYSTEM] Audio source: {}{}.",
                    source,
                    if dup { " (kept on device)" } else { "" }
                ),
            );
        }
    }

    if config.record.unwrap_or(false) {
        let path = config
            .record_path
            .as_ref()
            .map(|p| if p.is_empty() { "Videos" } else { p })
            .unwrap_or("Videos");
        bus_log(
            events.as_ref(),
            format!("[SYSTEM] Recording enabled -> output to {}", path),
        );
    }

    let adb_exe_path = get_binary_path("adb", config.scrcpy_path.clone());
    let server_path = if !exe_path.is_empty() && exe_path != "scrcpy" {
        Path::new(&exe_path)
            .parent()
            .map(|p| p.join("scrcpy-server").to_string_lossy().to_string())
    } else {
        None
    };

    bus_log(
        events.as_ref(),
        format!("[SYSTEM] Using scrcpy: {}", exe_path),
    );
    bus_log(
        events.as_ref(),
        format!("[SYSTEM] Using adb: {}", adb_exe_path),
    );

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
        bus_log(
            events.as_ref(),
            "[SYSTEM] Replacing the previous session for this device.".to_string(),
        );
        let notified = existing.done.notified();
        let _ = existing.stop.send(StopRequest { position: None });
        if tokio::time::timeout(Duration::from_secs(3), notified)
            .await
            .is_err()
        {
            host.force_kill(existing.pid.load(Ordering::Relaxed));
        }
    }

    // `--turn-screen-off` is applied right after the connection is set up, so
    // the display must already be on when scrcpy starts. Otherwise, on slower
    // devices, scrcpy's own asynchronous power-on can land after its
    // power-off request and leave the phone screen on until the user wakes it
    // manually. Wake it here, after any previous session has been stopped, so
    // the power-off request is the last one to reach the device.
    if wakes_before_screen_off(&config) {
        wake_display_before_screen_off(events.as_ref(), &adb_exe_path, &config.device).await;
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
                crate::mic::unload_mic_sink(events.as_ref()).await;
            }
            let denoise = config.session_mode == "mic" && config.mic_denoise.unwrap_or(false);
            if crate::mic::ensure_mic_sink(events.as_ref(), denoise).await {
                crate::mic::reserve_mic_sink_user(&state.mic_sink_users);
                let uses_default = config.session_mode == "mic"
                    && config.mic_set_default_source.unwrap_or(false)
                    && crate::mic::ensure_default_mic_source(events.as_ref(), &state).await;
                (
                    Some(crate::mic::MIC_SINK_NAME.to_string()),
                    true,
                    uses_default,
                )
            } else {
                (None, false, false)
            }
        } else {
            (None, false, false)
        };

    #[cfg(not(unix))]
    let pulse_sink: Option<String> = if wants_virtual_mic {
        bus_log(
            events.as_ref(),
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
        bus_log(events.as_ref(), message);
    }

    let spawned = crate::runtime::spawn_scrcpy_streams(
        events.clone(),
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
                crate::mic::release_default_mic_source(events.as_ref(), &state).await;
            }
            #[cfg(unix)]
            if uses_mic_sink {
                crate::mic::release_mic_sink(events.as_ref(), &state).await;
            }
            state.release_starting_device(&config.device);
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
        host.track_window(initial_scrcpy_pid, config.window_x.zip(config.window_y));
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
    state.release_starting_device(&config.device);
    bus_status(
        events.as_ref(),
        json!({ "device": config.device, "running": true }),
    );
    // The audio badge starts optimistic; log detection flips it to a warning
    // (or an error with --require-audio) if capture/codec trouble shows up.
    bus_audio_status(
        events.as_ref(),
        json!({
            "device": config.device,
            "status": if audio_enabled { "ok" } else { "off" }
        }),
    );

    // Virtual-sink sidecars. Started only after the registry insert above, so
    // their contains_key liveness checks see this session on the first poll
    // (spawning them earlier raced the insert and the loops died instantly).
    // The route watcher is needed for camera sessions too: SDL3 does not
    // reliably honour PULSE_SINK, so the stream is moved after it appears.
    #[cfg(unix)]
    if uses_mic_sink {
        crate::mic::spawn_mic_route_watcher(
            events.clone(),
            config.device.clone(),
            state.clone(),
            session_pid.clone(),
            session_generation,
            if config.session_mode == "camera" {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(2)
            },
            config.session_mode == "mic",
        );
        if config.session_mode == "mic" {
            crate::mic::spawn_mic_level_meter(
                events.clone(),
                config.device.clone(),
                state.clone(),
                session_generation,
            );
        }
    }

    // Monitor for exit (and orchestrate the audio codec fallback chain)
    let device_mon = config.device.clone();
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
            bus_audio_status(
                events.as_ref(),
                json!({ "device": &device_mon, "status": status }),
            );
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
                    crate::runtime::finish_stopped_session(
                        events.as_ref(),
                        &mut runtime,
                        &device_mon,
                        requested_pos,
                        position,
                        state.generation_is_current(&device_mon, session_generation),
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
                    bus_log(events.as_ref(), capture_message().to_string());
                    emit_audio_status(if config_mon.require_audio.unwrap_or(false) {
                        "error"
                    } else {
                        "warning"
                    });
                }
            } else if runtime.audio_state.codec_failed() {
                if !codec_failure_is_terminal(should_auto_fallback, runtime.chain_index) {
                    let next_codec = AUDIO_FALLBACK_CHAIN[runtime.chain_index];
                    match crate::runtime::retry_audio_codec(
                        &mut runtime,
                        &retry_ctx,
                        RetryOrigin::Running,
                        next_codec,
                        events.as_ref(),
                        host.as_ref(),
                    )
                    .await
                    {
                        Ok(()) => continue,
                        Err(e) => {
                            bus_log(
                                events.as_ref(),
                                format!("[SYSTEM] Failed to spawn audio retry: {}", e),
                            );
                            emit_audio_status("error");
                            crate::runtime::emit_window_pos_and_stop(
                                events.as_ref(),
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
                    bus_log(
                        events.as_ref(),
                        codec_failure_message(should_auto_fallback).to_string(),
                    );
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
                    if let Some(pos) = host.capture_window_position(pid) {
                        if runtime.first_window_pos.is_none() {
                            runtime.first_window_pos = Some(pos);
                        }
                        runtime.last_window_pos = Some(pos);
                    }
                }
            }

            let outcome = match runtime.child.try_wait() {
                Ok(Some(status)) => AttemptOutcome::Exited {
                    description: status
                        .code()
                        .map(|code| format!("code {code}"))
                        .unwrap_or_else(|| "signal".to_string()),
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
                    bus_log(
                        events.as_ref(),
                        format!("[SYSTEM] Error waiting for scrcpy: {}", e),
                    );
                    emit_audio_status("off");
                    crate::runtime::emit_window_pos_and_stop(
                        events.as_ref(),
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
                    bus_log(
                        events.as_ref(),
                        format!("[SYSTEM] Scrcpy process exited with {status}."),
                    );

                    // Give stream reader tasks a moment to flush remaining stderr
                    // lines (the audio error pattern may still be in flight).
                    tokio::time::sleep(Duration::from_millis(250)).await;

                    let capture_failed = runtime.audio_state.capture_failed();
                    let codec_failed = runtime.audio_state.codec_failed();

                    if capture_failed {
                        // Changing codecs cannot fix a device-side capture
                        // failure, so stop instead of burning the fallback chain.
                        if !runtime.capture_reported {
                            bus_log(events.as_ref(), capture_message().to_string());
                        }
                        emit_audio_status(if config_mon.require_audio.unwrap_or(false) {
                            "error"
                        } else {
                            "warning"
                        });
                        crate::runtime::emit_window_pos_and_stop(
                            events.as_ref(),
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
                            match crate::runtime::retry_audio_codec(
                                &mut runtime,
                                &retry_ctx,
                                RetryOrigin::Exited,
                                next_codec,
                                events.as_ref(),
                                host.as_ref(),
                            )
                            .await
                            {
                                Ok(()) => continue,
                                Err(e) => {
                                    bus_log(
                                        events.as_ref(),
                                        format!("[SYSTEM] Failed to spawn audio retry: {}", e),
                                    );
                                    emit_audio_status("error");
                                    crate::runtime::emit_window_pos_and_stop(
                                        events.as_ref(),
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
                            bus_log(
                                events.as_ref(),
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
                        if !crate::runtime::finish_if_session_gone(
                            events.as_ref(),
                            &mut runtime,
                            &state,
                            &device_mon,
                            session_generation,
                            requested_pos,
                        )
                        .await
                        {
                            break 'monitor;
                        }
                        if reconnect_attempt == 0 {
                            bus_log(events.as_ref(), "[RECONNECT] scrcpy exited unexpectedly; trying to restore the session."
                                    .to_string(),);
                        }
                        loop {
                            let attempt = reconnect_attempt.saturating_add(1);
                            let delay = reconnect_delay(reconnect_attempt);
                            reconnect_attempt = attempt;
                            bus_log(
                                events.as_ref(),
                                format!(
                                    "[RECONNECT] Retrying in {}s (attempt {}).",
                                    delay.as_secs(),
                                    attempt
                                ),
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                request = &mut stop_rx => {
                                    let position = match request {
                                        Ok(request) => request.position,
                                        Err(_) => None,
                                    };
                                    crate::runtime::finish_stopped_session(
                                        events.as_ref(),
                                        &mut runtime,
                                        &device_mon,
                                        requested_pos,
                                        position,
                                        state.generation_is_current(
                                            &device_mon,
                                            session_generation,
                                        ),
                                    )
                                    .await;
                                    break 'monitor;
                                }
                            }
                            if !crate::runtime::finish_if_session_gone(
                                events.as_ref(),
                                &mut runtime,
                                &state,
                                &device_mon,
                                session_generation,
                                requested_pos,
                            )
                            .await
                            {
                                break 'monitor;
                            }
                            if !device_is_online(&adb_exe_path_mon, &device_mon).await {
                                bus_log(
                                    events.as_ref(),
                                    format!(
                                        "[RECONNECT] Device {} is not available yet; waiting.",
                                        device_mon
                                    ),
                                );
                                continue;
                            }
                            // A replacement or a manual stop may have happened
                            // while `adb devices` was running.
                            if !crate::runtime::finish_if_session_gone(
                                events.as_ref(),
                                &mut runtime,
                                &state,
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
                                bus_log(events.as_ref(), message);
                            }
                            match crate::runtime::spawn_scrcpy_streams(
                                events.clone(),
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
                                        host.stop_tracking(runtime.pid.load(Ordering::Relaxed));
                                        host.track_window(
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
                                    bus_log(
                                        events.as_ref(),
                                        format!(
                                            "[RECONNECT] Session restored after {} attempt(s).",
                                            attempt
                                        ),
                                    );
                                    break;
                                }
                                Err(e) => {
                                    bus_log(
                                        events.as_ref(),
                                        format!("[RECONNECT] Attempt {} failed: {}", attempt, e),
                                    );
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
                    crate::runtime::emit_window_pos_and_stop(
                        events.as_ref(),
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
            let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
            if sessions.get(&device_mon).map(|s| s.generation) == Some(session_generation) {
                sessions.remove(&device_mon);
            }
        }
        // Restore the default source before the sink is unloaded. A replacement
        // has its own reservation, so both counts decide when each step runs.
        #[cfg(unix)]
        if uses_mic_default_source {
            crate::mic::release_default_mic_source(events.as_ref(), &state).await;
        }
        // Release this session's sink reservation even when a replacement
        // already bumped the generation: the replacement has its own
        // reservation, so the count decides when the sink can be unloaded.
        #[cfg(unix)]
        if uses_mic_sink {
            crate::mic::release_mic_sink(events.as_ref(), &state).await;
        }
        #[cfg(target_os = "linux")]
        if track_position {
            host.stop_tracking(runtime.pid.load(Ordering::Relaxed));
        }
        runtime.pid.store(0, Ordering::Relaxed);
        session_done.notify_one();
    });

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::validate_scrcpy_config;

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
    fn test_window_position_tracking_is_windowed_mirror_only() {
        assert!(tracks_window_position(&base_config("mirror")));

        let mut fullscreen = base_config("mirror");
        fullscreen.fullscreen = Some(true);
        assert!(!tracks_window_position(&fullscreen));

        for mode in ["camera", "desktop", "mic"] {
            assert!(!tracks_window_position(&base_config(mode)));
        }
    }
}
