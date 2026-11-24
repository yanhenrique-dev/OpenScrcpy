//! Virtual microphone bridge (Linux / PipeWire-PulseAudio).
//!
//! For the phone mic to become a *recording device* on the PC, the app creates
//! a dedicated null sink ("OpenScrcpy Mic"): scrcpy's stream plays into it and
//! the sink's monitor shows up to every app as a microphone input. The sink is
//! service-owned: created at bridge start, unloaded when nothing uses it.
//!
//! Everything here is UI-agnostic: logs and meter levels go through the
//! [`EventBus`], and liveness is answered by [`ScrcpyState`]. The whole module
//! is `unix`-only; on other targets it compiles to nothing and the session
//! monitor logs that routing needs Linux.

#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command as StdCommand, Stdio};
#[cfg(unix)]
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use tokio::io::AsyncReadExt;
#[cfg(unix)]
use tokio::time::{timeout, Duration};

#[cfg(unix)]
use crate::events::EventBus;
#[cfg(unix)]
use crate::process::{create_command, output_with_timeout};
#[cfg(unix)]
use crate::state::ScrcpyState;

#[cfg(unix)]
pub const MIC_SINK_NAME: &str = "openscrcpy_mic";
#[cfg(unix)]
const MIC_SINK_LABEL: &str = "OpenScrcpy Mic";
#[cfg(unix)]
const MIC_SINK_MONITOR: &str = "openscrcpy_mic.monitor";

#[cfg(unix)]
fn log(events: Option<&EventBus>, line: impl Into<String>) {
    if let Some(bus) = events {
        bus.log(line);
    }
}

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
    let output = output_with_timeout(cmd, Duration::from_secs(4))
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
pub async fn ensure_default_mic_source(events: Option<&EventBus>, state: &ScrcpyState) -> bool {
    let users = state.mic_default_source_users.load(Ordering::Relaxed);
    if users > 0 {
        state
            .mic_default_source_users
            .fetch_add(1, Ordering::Relaxed);
        return true;
    }

    let Some(previous) = pactl_get_default_source().await else {
        log(
            events,
            "[MIC] Could not read the current default source; leaving it unchanged.",
        );
        return false;
    };
    if previous == MIC_SINK_MONITOR {
        log(
            events,
            "[MIC] The virtual monitor source is already the default; leaving it unchanged.",
        );
        return false;
    }
    if !pactl_set_default_source(MIC_SINK_MONITOR).await {
        log(
            events,
            "[MIC] Could not set Monitor of OpenScrcpy Mic as the default source.",
        );
        return false;
    }

    *state
        .mic_previous_default_source
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(previous.clone());
    state.mic_default_source_users.store(1, Ordering::Relaxed);
    log(
        events,
        format!(
            "[MIC] Monitor of OpenScrcpy Mic is now the default source; will restore \"{}\" when the bridge stops.",
            previous
        ),
    );
    true
}

/// Releases one opt-in default-source user and restores the saved source when
/// the last one stops. It only restores if the default is still the virtual
/// source, so a manual change by the user wins.
#[cfg(unix)]
pub async fn release_default_mic_source(events: Option<&EventBus>, state: &ScrcpyState) {
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
        log(
            events,
            "[MIC] Default source changed by another app; not restoring the previous one.",
        );
        return;
    }
    if pactl_set_default_source(&previous).await {
        log(
            events,
            format!("[MIC] Default source restored to \"{}\".", previous),
        );
    } else {
        log(
            events,
            format!(
                "[MIC] Could not restore the previous default source \"{}\".",
                previous
            ),
        );
    }
}

/// Synchronous teardown variant for `RunEvent::Exit`, where the async runtime
/// is not available. Best effort, like the rest of the exit path.
#[cfg(unix)]
pub fn restore_default_source_sync(state: &ScrcpyState) {
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

/// Returns true when the virtual sink exists (created now, if absent).
/// With `denoise` (experimental) the sink is born from a PipeWire RNNoise
/// filter-chain, so apps recording its monitor pick up suppressed noise.
#[cfg(unix)]
pub async fn ensure_mic_sink(events: Option<&EventBus>, denoise: bool) -> bool {
    let Some(sinks) = pactl_run(&["list", "short", "sinks"]).await else {
        log(
            events,
            "[MIC] pactl not found; routing skipped (audio goes to the default output).",
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
        log(events, message);
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
                        log(
                            events,
                            format!(
                                "[MIC] Noise-suppressed sink \"OpenScrcpy Mic\" created (RNNoise: {}).",
                                conf
                            ),
                        );
                        return true;
                    }
                    Some(o) => {
                        log(
                            events,
                            format!("[MIC] RNNoise sink rejected by PipeWire: {}.", o.stderr),
                        );
                    }
                    None => {
                        log(
                            events,
                            "[MIC] RNNoise sink failed to load; trying the plain sink.",
                        );
                    }
                }
            }
            None => {
                log(
                    events,
                    "[MIC] RNNoise filter-chain file not found; using the unfiltered sink.",
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
            log(
                events,
                format!(
                    "[MIC] Virtual sink \"{}\" created — select \"Monitor of {}\" (or the matching monitor source) as the microphone in Discord/OBS/Recorder.",
                    MIC_SINK_LABEL, MIC_SINK_LABEL
                ),
            );
            true
        }
        Some(o) => {
            log(
                events,
                format!("[MIC] Could not create the virtual sink: {}.", o.stderr),
            );
            false
        }
        None => {
            log(
                events,
                "[MIC] Could not create the virtual sink; audio goes to the default output.",
            );
            false
        }
    }
}

#[cfg(unix)]
pub async fn unload_mic_sink(events: Option<&EventBus>) {
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
        log(
            events,
            "[MIC] Virtual sink unloaded (no active session is using it).",
        );
    }
}

/// Reserves one user of the shared virtual sink. The caller must hold
/// `ScrcpyState::mic_sink_lock` across creation and this reservation, so a
/// session stopping at the same time cannot unload the sink in between.
#[cfg(unix)]
pub fn reserve_mic_sink_user(users: &AtomicUsize) {
    users.fetch_add(1, Ordering::Relaxed);
}

/// Releases one user. Returns true when the caller must unload the sink
/// because this was the last one; never underflows on a stray release.
/// Callers must hold `ScrcpyState::mic_sink_lock`, like the reservation path.
#[cfg(unix)]
pub fn release_mic_sink_user(users: &AtomicUsize) -> bool {
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
pub async fn release_mic_sink(events: Option<&EventBus>, state: &ScrcpyState) {
    let _guard = state.mic_sink_lock.lock().await;
    if release_mic_sink_user(&state.mic_sink_users) {
        unload_mic_sink(events).await;
    }
}

/// Watcher that keeps the scrcpy stream inside the virtual sink for the whole
/// session. The microphone bridge also reports the current destination to the
/// GUI; camera sessions only move the stream, because the `mic-route` event
/// belongs to the bridge's MicMeter and must not carry a camera's state.
/// PULSE_SINK does not reach SDL3 clients and the stream shows up only after
/// scrcpy's server handshake (seconds later), so a one-shot check is not
/// enough. Camera sessions pass a slower steady interval because they can stay
/// open for hours; the mic bridge keeps the tighter default.
#[cfg(unix)]
pub fn spawn_mic_route_watcher(
    events: Option<EventBus>,
    device: String,
    state: Arc<ScrcpyState>,
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
            if !state.session_alive(&device, generation) {
                break;
            }
            attempts += 1;
            let pid = session_pid.load(Ordering::Relaxed);
            if let Some((display, routed)) = poll_mic_route(events.as_ref(), pid).await {
                last_routed = Some(routed);
                // Emit every pass instead of only on change: the GUI only
                // subscribes while the mic tab is mounted, so a remounted tab
                // would otherwise stay without destination info until the
                // route actually changes. Camera sessions skip this event:
                // a bridge on another device owns the MicMeter, and a camera
                // "searching" state must not overwrite its destination.
                if report_route {
                    if let Some(bus) = &events {
                        bus.mic_route(display, routed);
                    }
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
async fn poll_mic_route(events: Option<&EventBus>, scrcpy_pid: u32) -> Option<(String, bool)> {
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
        log(
            events,
            if ok {
                "[MIC] Audio stream moved into the virtual sink.".to_string()
            } else {
                format!(
                    "[MIC] move-sink-input failed: {}.",
                    moved.map(|m| m.stderr).unwrap_or_default()
                )
            },
        );
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

/// Live level feedback: parec taps the virtual sink's monitor and the RMS of
/// each 100 ms chunk is emitted to the GUI as dB. The loop ends when parec
/// exits or the owning scrcpy session leaves the process registry.
#[cfg(unix)]
pub fn spawn_mic_level_meter(
    events: Option<EventBus>,
    device: String,
    state: Arc<ScrcpyState>,
    generation: u64,
) {
    tokio::spawn(async move {
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
            log(
                events.as_ref(),
                "[MIC] Level meter unavailable (parec not found).",
            );
            return;
        };
        log(
            events.as_ref(),
            format!("[MIC] Level meter started (parec on {}).", monitor),
        );
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill().await;
            log(events.as_ref(), "[MIC] Level meter ended (no stdout).");
            return;
        };
        // 88200 bytes/s -> one 100 ms chunk at s16le mono 44.1 kHz.
        let mut buf = vec![0u8; 8820];
        let mut chunks: u32 = 0;
        loop {
            if !state.session_alive(&device, generation) {
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
            if let Some(bus) = &events {
                bus.mic_level(db);
            }
        }
        let _ = child.kill().await;
        log(
            events.as_ref(),
            format!("[MIC] Level meter ended after {} chunks.", chunks),
        );
    });
}

/// Synchronous sink teardown for process exit: event handlers run outside the
/// async runtime, so this cannot reuse the async helper.
#[cfg(unix)]
pub fn teardown_mic_sink() {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

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
}
