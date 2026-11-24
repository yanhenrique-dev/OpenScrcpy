//! Session registry shared by every UI shell.
//!
//! These types are deliberately UI-agnostic: they hold the process handles,
//! stop channels and counters the commands need, not windows or app handles.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};
use std::sync::{Arc, Mutex};

use tokio::sync::{oneshot, Notify};

/// One running session. The monitor task owns the `Child`; the registry keeps
/// only what other commands need: the PID to target, a channel to ask the
/// task to stop, and a notification that it finished.
pub struct SessionHandle {
    pub generation: u64,
    /// Whether this session owns the persisted mirror-window position. Only
    /// windowed mirror sessions do; camera/desktop/fullscreen must not read or
    /// overwrite it.
    pub track_position: bool,
    pub pid: Arc<AtomicU32>,
    pub stop: oneshot::Sender<StopRequest>,
    pub done: Arc<Notify>,
}

#[derive(Debug)]
pub struct StopRequest {
    /// Position captured by `stop_scrcpy` while the window still exists.
    pub position: Option<(i32, i32)>,
}

/// Bump the logical generation for `device` and return the new value.
pub fn bump_session_generation(generations: &mut HashMap<String, u64>, device: &str) -> u64 {
    let generation = generations.entry(device.to_string()).or_insert(0);
    *generation += 1;
    *generation
}

/// True when `generation` is still the current one for `device`.
pub fn session_generation_is_current(
    generations: &HashMap<String, u64>,
    device: &str,
    generation: u64,
) -> bool {
    generations.get(device) == Some(&generation)
}

pub struct ScrcpyState {
    pub sessions: Mutex<HashMap<String, SessionHandle>>,
    /// Devices with a run_scrcpy call currently preparing a child. Keeps two
    /// concurrent starts from both spawning before either registers.
    pub starting_devices: Mutex<HashSet<String>>,
    /// Serializes the binary update swap with the "reserve a device" step of
    /// run_scrcpy, closing the check-then-start race in download_scrcpy.
    pub binary_swap: tokio::sync::Mutex<()>,
    /// Serializes virtual-sink creation/unload with the per-session user
    /// count. Without it, a stopping session could unload the sink between
    /// another session's `ensure_mic_sink` and its reservation, silently
    /// sending the new audio stream to the default output.
    pub mic_sink_lock: tokio::sync::Mutex<()>,
    /// Number of sessions holding a reservation on the shared OpenScrcpy Mic
    /// sink (microphone bridge or camera with virtual mic). A start reserves
    /// before spawning, so the count can briefly include a session that is
    /// still preparing. The sink is unloaded when it reaches zero.
    pub mic_sink_users: AtomicUsize,
    /// Number of microphone bridges that asked to become the system default
    /// input. Mutated under `mic_sink_lock`; teardown reads it synchronously.
    pub mic_default_source_users: AtomicUsize,
    /// Default source captured before the first opt-in bridge changed it.
    /// Restored when the last opt-in bridge stops.
    pub mic_previous_default_source: Mutex<Option<String>>,
    /// Logical session generation per device serial, bumped by every
    /// run_scrcpy. Mic sidecar loops (route watcher, level meter) capture the
    /// value at spawn and exit once it moves past theirs: a replaced session
    /// reuses the same registry key, so the plain `contains_key` check alone
    /// cannot tell the previous session's loops apart from the new one.
    pub session_generations: Mutex<HashMap<String, u64>>,
    pub active_device: Mutex<Option<String>>,
    /// Mirrors the persisted "keep running in the background" preference.
    /// When set, closing the main window hides it instead of exiting.
    pub background_mode: AtomicBool,
}

impl Default for ScrcpyState {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrcpyState {
    /// Empty registry with every counter at its neutral value. Shells create
    /// one and share it as an `Arc`.
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            starting_devices: Mutex::new(HashSet::new()),
            binary_swap: tokio::sync::Mutex::new(()),
            mic_sink_lock: tokio::sync::Mutex::new(()),
            mic_sink_users: AtomicUsize::new(0),
            mic_default_source_users: AtomicUsize::new(0),
            mic_previous_default_source: Mutex::new(None),
            session_generations: Mutex::new(HashMap::new()),
            active_device: Mutex::new(None),
            background_mode: AtomicBool::new(false),
        }
    }
}

impl ScrcpyState {
    /// True when no replacement has taken over this device. A replaced session
    /// must not publish a stop event: the new session already owns the UI.
    pub fn generation_is_current(&self, device: &str, generation: u64) -> bool {
        let generations = self
            .session_generations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        session_generation_is_current(&generations, device, generation)
    }

    /// Sidecar liveness: still registered AND this generation is current.
    /// Locks are taken one after the other, never together.
    pub fn session_alive(&self, device: &str, generation: u64) -> bool {
        let in_registry = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(device);
        in_registry && self.generation_is_current(device, generation)
    }

    pub fn release_starting_device(&self, device: &str) {
        self.starting_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(device);
    }
}
