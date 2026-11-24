//! Retained session state shared by the monitor and its retry helper.
//!
//! These types are UI-agnostic: they hold the child process, audio state and
//! the latest window-position samples. The Tauri monitor still drives them.

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;

use crate::audio::{is_audio_capture_error, is_audio_codec_error, AudioErrorState};
use crate::capabilities;
use crate::capabilities::Capabilities;
use crate::config::build_scrcpy_args;
use crate::config::ScrcpyConfig;
use crate::events::EventBus;
use crate::process::create_command;
#[cfg(target_os = "windows")]
use crate::process::CREATE_NO_WINDOW;
use crate::session::{resolve_window_pos_to_persist, session_end_payload, SessionEndReason};
use crate::state::ScrcpyState;

/// Mutable state owned by the session monitor between retries.
pub struct SessionRuntime {
    pub child: Child,
    pub audio_state: AudioErrorState,
    pub pid: Arc<AtomicU32>,
    pub first_window_pos: Option<(i32, i32)>,
    pub last_window_pos: Option<(i32, i32)>,
    pub chain_index: usize,
    pub capture_reported: bool,
    pub codec_terminal_reported: bool,
}

/// Immutable session context needed to respawn scrcpy with another codec.
pub struct RetryContext<'a> {
    pub device: &'a str,
    pub exe_path: &'a str,
    pub adb_exe_path: &'a str,
    pub server_path: Option<&'a str>,
    pub config: &'a ScrcpyConfig,
    pub video_dir: &'a Option<String>,
    pub pulse_sink: Option<&'a str>,
    pub capabilities: &'a Capabilities,
    // Only read by the KWin bridge; the field stays for a single context shape.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub track_position: bool,
}

#[derive(Clone, Copy)]
pub enum RetryOrigin {
    Running,
    Exited,
}

/// Spawns scrcpy with both streams piped and starts the batching readers.
/// The shell passes its bus so log lines keep the existing event contract.
pub async fn spawn_scrcpy_streams(
    events: Option<EventBus>,
    exe_path: &str,
    adb_exe_path: &str,
    server_path: Option<&str>,
    args: &[String],
    vsync: bool,
    pulse_sink: Option<&str>,
) -> Result<(Child, AudioErrorState), String> {
    if let Some(bus) = &events {
        bus.log(format!("> scrcpy {}", args.join(" ")));
    }

    let mut command = create_command(exe_path);
    command.args(args);
    command.env("ADB", adb_exe_path);
    // KDE matches the launcher's desktop entry (icon, grouping, pinning)
    // instead of logging a missing "scrcpy.desktop". SDL3 reads SDL_APP_ID;
    // the legacy names cover SDL2.
    #[cfg(target_os = "linux")]
    {
        command.env("SDL_APP_ID", "openscrcpy");
        command.env("SDL_VIDEO_WAYLAND_WMCLASS", "openscrcpy");
        command.env("SDL_VIDEO_X11_WMCLASS", "openscrcpy");
    }
    if let Some(sink) = pulse_sink {
        command.env("PULSE_SINK", sink);
    }
    if let Some(server_path) = server_path {
        if Path::new(server_path).exists() {
            command.env("SCRCPY_SERVER_PATH", server_path);
        }
    }
    // scrcpy 4.0/SDL3 leaves VSync off by default; re-assert it unless the
    // user opted out in-app.
    command.env("SDL_RENDER_VSYNC", if vsync { "1" } else { "0" });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let stderr = child.stderr.take().expect("Failed to capture stderr");
    let audio_state = AudioErrorState::default();
    spawn_log_reader(stdout, events.clone(), None);
    spawn_log_reader(stderr, events, Some(audio_state.clone()));
    Ok((child, audio_state))
}

/// Buffers scrcpy output for 100 ms so a burst reaches the UI as one log
/// event. The stderr reader also classifies audio failures for the fallback
/// chain.
fn spawn_log_reader<R>(reader: R, events: Option<EventBus>, audio_state: Option<AudioErrorState>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        let mut buffer = Vec::new();
        let mut interval = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                line_res = lines.next_line() => {
                    match line_res {
                        Ok(Some(line)) => {
                            if let Some(state) = &audio_state {
                                if is_audio_capture_error(&line) {
                                    state.note_capture();
                                } else if is_audio_codec_error(&line) {
                                    state.note_codec();
                                }
                            }
                            buffer.push(line);
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        if let Some(bus) = &events {
                            bus.log(buffer.join("\n"));
                        }
                        buffer.clear();
                    }
                }
            }
        }
        if !buffer.is_empty() {
            if let Some(bus) = &events {
                bus.log(buffer.join("\n"));
            }
        }
    });
}

/// Native window tracking needed when scrcpy is respawned with another codec.
/// The Tauri shell implements this with KWin; other shells can pass
/// [`NoopSessionIntegration`].
pub trait SessionIntegration: Send + Sync {
    fn track_window(&self, _pid: u32, _position: Option<(i32, i32)>) {}
    fn stop_tracking(&self, _pid: u32) {}
}

pub struct NoopSessionIntegration;

impl SessionIntegration for NoopSessionIntegration {}

/// Everything the core needs from the native shell to stop or monitor a
/// session. KWin/Windows details stay in the shell.
pub trait SessionHost: SessionIntegration {
    fn capture_window_position(&self, _pid: u32) -> Option<(i32, i32)> {
        None
    }
    fn force_kill(&self, _pid: u32) {}
}

pub struct NoopSessionHost;

impl SessionIntegration for NoopSessionHost {}
impl SessionHost for NoopSessionHost {}

/// Asks the child to exit, waits `grace`, then forces it. Returns whether the
/// graceful path was enough.
pub async fn terminate_child(child: &mut Child, grace: Duration) -> bool {
    #[cfg(target_os = "windows")]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
    }

    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(_)) => true,
        Ok(Err(_)) | Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            false
        }
    }
}

/// Replaces the current child with one using `next_codec`, updating the PID,
/// window tracking and position samples. Callers own the stop notification.
pub async fn retry_audio_codec(
    runtime: &mut SessionRuntime,
    ctx: &RetryContext<'_>,
    origin: RetryOrigin,
    next_codec: &'static str,
    events: Option<&EventBus>,
    integration: &dyn SessionIntegration,
) -> Result<(), String> {
    if let Some(bus) = events {
        bus.log(format!(
            "[SYSTEM] Default audio codec failed, retrying with {}...",
            next_codec.to_uppercase()
        ));
        bus.audio_status(serde_json::json!({ "device": ctx.device, "status": "warning" }));
    }

    let old_pid = match origin {
        RetryOrigin::Running => runtime.child.id().unwrap_or(0),
        RetryOrigin::Exited => runtime.pid.load(Ordering::Relaxed),
    };
    if matches!(origin, RetryOrigin::Running) {
        let _ = terminate_child(&mut runtime.child, Duration::from_secs(2)).await;
    }

    let new_args = build_scrcpy_args(ctx.config, ctx.video_dir.clone(), Some(next_codec));
    let (new_args, _) = capabilities::filter_args(new_args, ctx.capabilities);
    let (new_child, new_state) = spawn_scrcpy_streams(
        events.cloned(),
        ctx.exe_path,
        ctx.adb_exe_path,
        ctx.server_path,
        &new_args,
        ctx.config.vsync.unwrap_or(true),
        ctx.pulse_sink,
    )
    .await?;

    let new_pid = new_child.id().unwrap_or(0);
    runtime.pid.store(new_pid, Ordering::Relaxed);
    if ctx.track_position {
        integration.stop_tracking(old_pid);
        integration.track_window(new_pid, ctx.config.window_x.zip(ctx.config.window_y));
    }
    runtime.child = new_child;
    runtime.audio_state = new_state;
    // New window: re-learn its decoration offset by resampling from scratch.
    runtime.first_window_pos = None;
    runtime.last_window_pos = None;
    runtime.capture_reported = false;
    runtime.chain_index += 1;
    if let Some(bus) = events {
        bus.audio_status(serde_json::json!({ "device": ctx.device, "status": "ok" }));
    }
    Ok(())
}

/// Publishes the persisted window position (when one was captured) followed by
/// the session-stopped status.
pub fn emit_window_pos_and_stop(
    events: Option<&EventBus>,
    device: &str,
    requested: Option<(i32, i32)>,
    first: Option<(i32, i32)>,
    last: Option<(i32, i32)>,
    reason: SessionEndReason,
) {
    let Some(bus) = events else {
        return;
    };
    if let Some(last) = last {
        let (x, y) = resolve_window_pos_to_persist(requested, first, last);
        bus.window_pos(serde_json::json!({ "device": device, "x": x, "y": y }));
    }
    bus.status(session_end_payload(device, reason));
}

/// Finalizes a user-initiated stop. Also used while the monitor is in the
/// reconnect backoff, where the child has already exited.
pub async fn finish_stopped_session(
    events: Option<&EventBus>,
    runtime: &mut SessionRuntime,
    device: &str,
    requested: Option<(i32, i32)>,
    stop_position: Option<(i32, i32)>,
    publish: bool,
) {
    // Dropping a Tokio Child does not kill the process, so a replacement must
    // still reap the previous scrcpy even when it must not publish a stop.
    let graceful = match runtime.child.try_wait() {
        Ok(Some(_)) => true,
        _ => terminate_child(&mut runtime.child, Duration::from_secs(2)).await,
    };
    if !publish {
        return;
    }
    let Some(bus) = events else {
        return;
    };
    bus.audio_status(serde_json::json!({ "device": device, "status": "off" }));
    emit_window_pos_and_stop(
        Some(bus),
        device,
        requested,
        runtime.first_window_pos,
        stop_position.or(runtime.last_window_pos),
        SessionEndReason::Stopped,
    );
    bus.log(format!(
        "[SYSTEM] Session stopped ({}).",
        if graceful { "graceful" } else { "forced" }
    ));
}

/// Returns true while this generation still owns the session. When the handle
/// was removed by a normal Stop, publishes the stop event; when a replacement
/// bumped the generation, leaves the UI untouched for the new session.
pub async fn finish_if_session_gone(
    events: Option<&EventBus>,
    runtime: &mut SessionRuntime,
    state: &ScrcpyState,
    device: &str,
    generation: u64,
    requested: Option<(i32, i32)>,
) -> bool {
    if state.session_alive(device, generation) {
        return true;
    }
    finish_stopped_session(
        events,
        runtime,
        device,
        requested,
        None,
        state.generation_is_current(device, generation),
    )
    .await;
    false
}
