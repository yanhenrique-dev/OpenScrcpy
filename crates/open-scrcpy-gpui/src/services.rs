//! Bridge between the GPUI shell and `open-scrcpy-core`.
//!
//! Core async work runs on a dedicated Tokio runtime: the core uses
//! `tokio::process` and its own monitor tasks, while GPUI owns a different
//! executor. Events come back through the core [`EventBus`] and are consumed by
//! a GPUI task that updates the shell entity.

use std::sync::Arc;

use open_scrcpy_core::config::ScrcpyConfig;
use open_scrcpy_core::events::{EventBus, EventReceiver};
use open_scrcpy_core::monitor::{SessionRequest, run_session};
use open_scrcpy_core::runtime::{SessionHost, SessionIntegration};
use open_scrcpy_core::session::{stop_session, teardown_sessions};
use open_scrcpy_core::state::ScrcpyState;
use open_scrcpy_core::{adb, process};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

/// Native window/process operations the core asks the shell for.
pub struct GpuiSessionHost;

impl SessionIntegration for GpuiSessionHost {
    fn track_window(&self, pid: u32, position: Option<(i32, i32)>) {
        #[cfg(target_os = "linux")]
        open_scrcpy_platform::kwin::track_window(pid, position);
        #[cfg(not(target_os = "linux"))]
        let _ = (pid, position);
    }

    fn stop_tracking(&self, pid: u32) {
        #[cfg(target_os = "linux")]
        open_scrcpy_platform::kwin::stop_tracking(pid);
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }
}

impl SessionHost for GpuiSessionHost {
    fn capture_window_position(&self, pid: u32) -> Option<(i32, i32)> {
        open_scrcpy_platform::window::try_capture_window_pos(pid)
    }

    fn force_kill(&self, pid: u32) {
        process::force_kill_pid(pid);
    }
}

/// A newer native release, as offered by the signed manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppUpdate {
    pub version: String,
    pub url: String,
}

const UPDATE_MANIFEST_URL: &str =
    "https://github.com/yanhenrique-dev/OpenScrcpy/releases/latest/download/latest-gpui.json";

/// Shared application services. Cloning is cheap: the runtime, the session
/// registry and the host are `Arc`s and the bus is a channel handle.
#[derive(Clone)]
pub struct AppServices {
    runtime: Arc<Runtime>,
    state: Arc<ScrcpyState>,
    events: EventBus,
    host: Arc<dyn SessionHost>,
    video_dir: Option<String>,
    /// Disabled by the UI tests so they never spawn `adb`.
    poll_devices: bool,
}

impl AppServices {
    pub fn new() -> (Self, EventReceiver) {
        let (events, receiver) = EventBus::channel();
        let services = Self {
            runtime: build_runtime(),
            state: Arc::new(ScrcpyState::new()),
            events,
            host: Arc::new(GpuiSessionHost),
            video_dir: user_video_dir(),
            poll_devices: true,
        };
        (services, receiver)
    }

    /// Services for a widget test: same seams, no background `adb` polling.
    pub fn for_test() -> (Self, EventReceiver) {
        let (mut services, receiver) = Self::new();
        services.poll_devices = false;
        (services, receiver)
    }

    pub fn polls_devices(&self) -> bool {
        self.poll_devices
    }

    pub fn state(&self) -> &Arc<ScrcpyState> {
        &self.state
    }

    /// Starts a baseline windowed mirror session. The session monitor owns the
    /// child from here; the returned task only reports start-up errors.
    pub fn start(&self, config: ScrcpyConfig) -> JoinHandle<Result<(), String>> {
        let request = SessionRequest {
            state: self.state.clone(),
            config,
            video_dir: self.video_dir.clone(),
            events: Some(self.events.clone()),
            host: self.host.clone(),
        };
        self.runtime.spawn(run_session(request))
    }

    pub fn stop(&self, device: &str) -> JoinHandle<Result<(), String>> {
        let state = self.state.clone();
        let host = self.host.clone();
        let device = device.to_string();
        self.runtime
            .spawn(async move { stop_session(&state, &device, host.as_ref()).await })
    }

    /// `adb connect <address>`. The core logs every step to the bus.
    pub fn connect(&self, address: String) -> JoinHandle<Result<serde_json::Value, String>> {
        let bus = self.events.clone();
        self.runtime.spawn(async move {
            adb::connect(move |line| bus.log(line), &address, None, false).await
        })
    }

    /// `adb pair <address> <code>`.
    pub fn pair(
        &self,
        address: String,
        code: String,
    ) -> JoinHandle<Result<serde_json::Value, String>> {
        let bus = self.events.clone();
        self.runtime
            .spawn(async move { adb::pair(move |line| bus.log(line), &address, &code, None).await })
    }

    /// Fetches the signed native manifest and compares it with this build.
    /// `OPENSCRCPY_UPDATE_URL` overrides the release URL for tests and forks.
    pub fn check_scrcpy(&self) -> JoinHandle<serde_json::Value> {
        self.runtime.spawn(async { adb::check_scrcpy(None).await })
    }

    /// Downloads the scrcpy/adb bundle into the resolved install directory.
    /// Progress arrives through the EventBus.
    pub fn download_scrcpy(&self) -> JoinHandle<Result<(), String>> {
        let events = self.events.clone();
        let state = self.state.clone();
        self.runtime.spawn(async move {
            let install = open_scrcpy_core::download::install_dir()
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| open_scrcpy_core::download::init_install_dir(None));
            open_scrcpy_core::download::download_latest(Some(&events), &state, &install).await
        })
    }

    pub fn check_app_update(&self) -> JoinHandle<Result<Option<AppUpdate>, String>> {
        self.runtime.spawn(async move {
            let url = std::env::var("OPENSCRCPY_UPDATE_URL")
                .unwrap_or_else(|_| UPDATE_MANIFEST_URL.to_string());
            let manifest = open_scrcpy_core::app_update::fetch_manifest(&url)
                .await
                .map_err(|error| error.to_string())?;
            let (status, asset) = open_scrcpy_core::app_update::check(
                &manifest,
                env!("CARGO_PKG_VERSION"),
                open_scrcpy_core::app_update::current_target(),
            )
            .map_err(|error| error.to_string())?;
            match status {
                open_scrcpy_core::app_update::UpdateStatus::UpToDate => Ok(None),
                open_scrcpy_core::app_update::UpdateStatus::Available { version, .. } => {
                    Ok(Some(AppUpdate {
                        version,
                        url: asset.map(|asset| asset.url).unwrap_or_default(),
                    }))
                }
            }
        })
    }

    pub fn list_cameras(&self, device: String) -> JoinHandle<serde_json::Value> {
        self.runtime.spawn(async move {
            open_scrcpy_core::scrcpy::list_options(&device, "--list-cameras", None).await
        })
    }

    pub fn run_terminal(
        &self,
        device: Option<String>,
        command: String,
    ) -> JoinHandle<serde_json::Value> {
        self.runtime.spawn(async move {
            open_scrcpy_core::terminal::run_command(device.as_deref(), &command, None).await
        })
    }

    pub fn list_mdns(&self) -> JoinHandle<serde_json::Value> {
        self.runtime
            .spawn(async { adb::list_mdns_services(None).await })
    }

    pub fn get_render_drivers(&self) -> JoinHandle<serde_json::Value> {
        self.runtime
            .spawn(async { open_scrcpy_core::scrcpy::get_render_drivers(None).await })
    }

    pub fn list_devices(&self) -> JoinHandle<serde_json::Value> {
        self.runtime.spawn(async { adb::list_devices(None).await })
    }

    pub fn push_file(&self, device: String, path: String) -> JoinHandle<serde_json::Value> {
        self.runtime
            .spawn(async move { adb::push_file(&device, &path, None).await })
    }

    pub fn install_apk(&self, device: String, path: String) -> JoinHandle<serde_json::Value> {
        self.runtime
            .spawn(async move { adb::install_apk(&device, &path, None).await })
    }

    /// Kills every tracked session synchronously; used from the window close
    /// handler before the process exits.
    pub fn teardown(&self) {
        teardown_sessions(&self.state, self.host.as_ref());
    }
}

fn build_runtime() -> Arc<Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime"),
    )
}

fn user_video_dir() -> Option<String> {
    directories::UserDirs::new().and_then(|dirs| {
        dirs.video_dir()
            .map(|path| path.to_string_lossy().into_owned())
    })
}
