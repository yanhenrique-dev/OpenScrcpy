//! The application shell: device hub, mode tabs, session controls and console.
//!
//! Presentation state lives here; device and session state come from
//! `open-scrcpy-core` through [`AppServices`]. The event loop and the device
//! poller are spawned in `new` and mutate this entity through its weak handle.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::Selectable as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, button::Button};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, ExternalPaths, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, SystemNotification,
    TestSupportExt as _, Window, WindowBounds, div, relative,
};
use open_scrcpy_core::events::{AppEvent, EventReceiver};
use open_scrcpy_core::scrcpy::CameraInfo;
use open_scrcpy_platform::shortcuts::PlatformCommand;
use rust_i18n::t;
use serde_json::Value;

use crate::i18n;
use crate::services::AppServices;
use crate::settings::{
    AppSettings, CameraSettings, DesktopSettings, MicSettings, MirrorSettings, SettingsStore,
    ThemePreference,
};
use crate::theme;

const MAX_LOG_LINES: usize = 100;
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(3);

fn theme_label(preference: ThemePreference) -> SharedString {
    match preference {
        ThemePreference::Light => t!("header.colorModes.light"),
        ThemePreference::Dark => t!("header.colorModes.dark"),
        ThemePreference::System => t!("header.colorModes.system"),
    }
    .to_string()
    .into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureMode {
    Screen,
    Camera,
    Desktop,
    Microphone,
}

impl CaptureMode {
    const ALL: [Self; 4] = [Self::Screen, Self::Camera, Self::Desktop, Self::Microphone];

    fn id(self) -> &'static str {
        match self {
            Self::Screen => "mode-screen",
            Self::Camera => "mode-camera",
            Self::Desktop => "mode-desktop",
            Self::Microphone => "mode-microphone",
        }
    }

    fn label(self) -> SharedString {
        match self {
            Self::Screen => t!("controlPanel.screen"),
            Self::Camera => t!("controlPanel.camera"),
            Self::Desktop => t!("controlPanel.desktop"),
            Self::Microphone => t!("controlPanel.microphone"),
        }
        .to_string()
        .into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscoveredService {
    name: String,
    service: String,
    address: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceState {
    Online,
    Unauthorized,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Device {
    serial: String,
    model: String,
    state: DeviceState,
}

impl Device {
    fn label(&self) -> SharedString {
        let name = if self.model.is_empty() {
            self.serial.as_str()
        } else {
            self.model.as_str()
        };
        match self.state {
            DeviceState::Online => name.to_string().into(),
            DeviceState::Unauthorized => {
                format!("{name} · {}", t!("sidebar.deviceUnauthorized")).into()
            }
            DeviceState::Offline => format!("{name} · {}", t!("sidebar.deviceOffline")).into(),
        }
    }

    /// Reads the `list_devices` payload. `devices` are ready to use; the
    /// `unavailable` array keeps phones that are physically present but
    /// unauthorized or offline visible instead of silently dropping them.
    fn parse(value: &Value) -> Vec<Self> {
        let mut devices = Vec::new();

        if let Some(serials) = value.get("devices").and_then(Value::as_array) {
            let models = value.get("deviceModels").and_then(Value::as_object);
            for serial in serials.iter().filter_map(Value::as_str) {
                let model = models
                    .and_then(|map| map.get(serial))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                devices.push(Self {
                    serial: serial.to_string(),
                    model: model.to_string(),
                    state: DeviceState::Online,
                });
            }
        }

        if let Some(unavailable) = value.get("unavailable").and_then(Value::as_array) {
            for entry in unavailable {
                let Some(serial) = entry.get("serial").and_then(Value::as_str) else {
                    continue;
                };
                let model = entry
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let state = match entry.get("state").and_then(Value::as_str) {
                    Some("unauthorized") => DeviceState::Unauthorized,
                    _ => DeviceState::Offline,
                };
                devices.push(Self {
                    serial: serial.to_string(),
                    model: model.to_string(),
                    state,
                });
            }
        }

        devices
    }
}

pub struct AppShell {
    settings: AppSettings,
    store: SettingsStore,
    services: AppServices,
    mode: CaptureMode,
    devices: Vec<Device>,
    active_device: Option<String>,
    running: bool,
    starting: bool,
    logs: Vec<String>,
    bitrate: Entity<SliderState>,
    _bitrate_subscription: Subscription,
    camera_zoom: Entity<SliderState>,
    _camera_zoom_subscription: Subscription,
    wireless_address: Entity<InputState>,
    pairing_address: Entity<InputState>,
    pairing_code: Entity<InputState>,
    wireless_busy: bool,
    mic_route: Option<(String, bool)>,
    mic_db: Option<f64>,
    renderers: Vec<(String, String)>,
    discovered: Vec<DiscoveredService>,
    terminal_input: Entity<InputState>,
    terminal_busy: bool,
    cameras: Vec<CameraInfo>,
    cameras_busy: bool,
}

impl AppShell {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        settings: AppSettings,
        store: SettingsStore,
        services: AppServices,
        events: EventReceiver,
        platform_commands: tokio::sync::mpsc::UnboundedReceiver<PlatformCommand>,
    ) -> Self {
        theme::apply(settings.theme, Some(window), cx);

        let close_services = services.clone();
        let close_store = store.clone();
        window.on_window_should_close(cx, move |window, _cx| {
            // The GPUI process must never leave a mirror or the virtual sink
            // behind; the core teardown is synchronous for exactly this path.
            close_services.teardown();
            let maximized = matches!(window.window_bounds(), WindowBounds::Maximized(_));
            close_store.save_window(window.bounds(), maximized);
            true
        });

        spawn_event_loop(cx, events);
        spawn_platform_commands(cx, platform_commands);
        if services.polls_devices() {
            spawn_device_poll(cx, services.clone());
            spawn_renderer_load(cx, services.clone());
            spawn_wireless_discovery(cx, services.clone());
        }

        let bitrate = cx.new(|_| {
            SliderState::new()
                .min(1.0)
                .max(50.0)
                .step(1.0)
                .default_value(settings.mirror.bitrate as f32)
        });
        let bitrate_subscription = cx.subscribe(&bitrate, |this, _, event: &SliderEvent, cx| {
            let SliderEvent::Release(value) = event else {
                return;
            };
            let bitrate = value.end().round().clamp(1.0, 50.0) as u32;
            if this.settings.mirror.bitrate != bitrate {
                this.settings.mirror.bitrate = bitrate;
                this.persist();
                cx.notify();
            }
        });

        let camera_zoom = cx.new(|_| {
            SliderState::new()
                .min(1.0)
                .max(10.0)
                .step(0.1)
                .default_value(settings.camera.zoom)
        });
        let camera_zoom_subscription =
            cx.subscribe(&camera_zoom, |this, _, event: &SliderEvent, cx| {
                let SliderEvent::Release(value) = event else {
                    return;
                };
                let zoom = value.end().clamp(1.0, 10.0);
                if (this.settings.camera.zoom - zoom).abs() > f32::EPSILON {
                    this.settings.camera.zoom = zoom;
                    this.persist();
                    cx.notify();
                }
            });

        let wireless_address = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("sidebar.ipPlaceholder").to_string())
        });
        let pairing_address = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("sidebar.ipPortPlaceholder").to_string())
        });
        let pairing_code = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("sidebar.pairingCodePlaceholder").to_string())
        });
        let terminal_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("logPanel.terminalPlaceholder").to_string())
        });

        Self {
            settings,
            store,
            services,
            mode: CaptureMode::Screen,
            devices: Vec::new(),
            active_device: None,
            running: false,
            starting: false,
            logs: Vec::new(),
            bitrate,
            _bitrate_subscription: bitrate_subscription,
            camera_zoom,
            _camera_zoom_subscription: camera_zoom_subscription,
            wireless_address,
            pairing_address,
            pairing_code,
            wireless_busy: false,
            mic_route: None,
            mic_db: None,
            renderers: Vec::new(),
            discovered: Vec::new(),
            terminal_input,
            terminal_busy: false,
            cameras: Vec::new(),
            cameras_busy: false,
        }
    }

    fn persist(&self) {
        if let Err(error) = self.store.save(&self.settings) {
            eprintln!("[settings] could not save: {error}");
        }
    }

    fn set_theme(&mut self, theme: ThemePreference, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.theme == theme {
            return;
        }
        self.settings.theme = theme;
        theme::apply(theme, Some(window), cx);
        self.persist();
        cx.notify();
    }

    fn cycle_locale(&mut self, cx: &mut Context<Self>) {
        let next = i18n::next(&self.settings.locale);
        self.settings.locale = next.to_string();
        i18n::apply(next);
        self.persist();
        cx.refresh_windows();
    }

    fn switch_mode(&mut self, mode: CaptureMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    fn select_device(&mut self, serial: String, cx: &mut Context<Self>) {
        if self.active_device.as_deref() != Some(serial.as_str()) {
            self.active_device = Some(serial);
            cx.notify();
        }
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.logs.push(line.into());
        if self.logs.len() > MAX_LOG_LINES {
            let overflow = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(..overflow);
        }
    }

    fn push_error(&mut self, error: impl Into<String>) {
        self.push_log(format!("[ERROR] {}", error.into()));
    }

    fn apply_event(&mut self, event: AppEvent, cx: &mut Context<Self>) {
        match event {
            AppEvent::Log(line) => self.push_log(line),
            AppEvent::Status(payload) => {
                if let Some(running) = payload.get("running").and_then(Value::as_bool) {
                    self.running = running;
                    self.starting = false;
                    if !running {
                        self.mic_route = None;
                        self.mic_db = None;
                        notify_unexpected_end(cx, &payload);
                    }
                }
            }
            AppEvent::AudioStatus(_) | AppEvent::WindowPos(_) => {}
            AppEvent::MicRoute { display, routed } => {
                self.mic_route = Some((display, routed));
            }
            AppEvent::MicLevel(db) => {
                self.mic_db = Some(db);
            }
        }
        cx.notify();
    }

    fn apply_devices(&mut self, devices: Vec<Device>, cx: &mut Context<Self>) {
        self.devices = devices;
        if self
            .active_device
            .as_deref()
            .is_some_and(|serial| !self.devices.iter().any(|device| device.serial == serial))
        {
            self.active_device = None;
        }
        if self.active_device.is_none() {
            self.active_device = self
                .devices
                .iter()
                .find(|device| device.state == DeviceState::Online)
                .map(|device| device.serial.clone());
        }
        cx.notify();
    }

    fn recenter_active(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            return;
        };
        let pid = self
            .services
            .state()
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&device)
            .map(|handle| handle.pid.load(std::sync::atomic::Ordering::Relaxed));
        let Some(pid) = pid.filter(|pid| *pid != 0) else {
            return;
        };
        open_scrcpy_platform::window::recenter_window(pid);
        self.push_log(format!(
            "[SYSTEM] Recentred the mirror window for {device}."
        ));
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let prompt: SharedString = t!("sidebar.flashPushFiles").to_string().into();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(prompt),
        });
        cx.spawn(async move |this, cx| {
            let Ok(selection) = receiver.await else {
                return;
            };
            let Ok(Some(paths)) = selection else {
                return;
            };
            let _ = this.update(cx, |shell, cx| shell.transfer_files(paths, cx));
        })
        .detach();
    }

    fn handle_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let files: Vec<PathBuf> = paths.0.iter().cloned().collect();
        if files.is_empty() {
            return;
        }
        self.transfer_files(files, cx);
    }

    fn transfer_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            self.push_error("[WARN] No device selected for push/install.");
            cx.notify();
            return;
        };
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            for path in paths {
                let is_apk = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"));
                let path = path.to_string_lossy().to_string();
                let outcome = if is_apk {
                    services.install_apk(device.clone(), path.clone()).await
                } else {
                    services.push_file(device.clone(), path.clone()).await
                };
                let done = this
                    .update(cx, |shell, cx| {
                        shell.log_transfer(&path, is_apk, outcome);
                        cx.notify();
                    })
                    .is_ok();
                if !done {
                    break;
                }
            }
        })
        .detach();
    }

    fn log_transfer(
        &mut self,
        path: &str,
        is_apk: bool,
        outcome: Result<Value, tokio::task::JoinError>,
    ) {
        match outcome {
            Ok(value) if value.get("success").and_then(Value::as_bool) == Some(true) => {
                let action = if is_apk { "Installed" } else { "Pushed" };
                self.push_log(format!("[SYSTEM] {action} {path}"));
            }
            Ok(value) => self.push_error(
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("transfer failed")
                    .to_string(),
            ),
            Err(error) => self.push_error(error.to_string()),
        }
    }

    fn connect_wireless(&mut self, cx: &mut Context<Self>) {
        let address = self.wireless_address.read(cx).value().trim().to_string();
        if address.is_empty() || self.wireless_busy {
            return;
        }
        self.wireless_busy = true;
        let services = self.services.clone();
        let recent = address.clone();
        cx.spawn(async move |this, cx| {
            let result = services.connect(address).await;
            let _ = this.update(cx, |shell, cx| {
                shell.wireless_busy = false;
                match result {
                    Ok(Ok(value)) => {
                        if value.get("success").and_then(Value::as_bool) == Some(true) {
                            shell.remember_wireless(&recent);
                            shell.push_log(format!("[SYSTEM] Connected to {recent}."));
                        } else if let Some(message) = value.get("message").and_then(Value::as_str) {
                            shell.push_error(message.to_string());
                        }
                    }
                    Ok(Err(error)) => shell.push_error(error),
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remember_wireless(&mut self, address: &str) {
        self.settings.recent_wireless.retain(|item| item != address);
        self.settings.recent_wireless.insert(0, address.to_string());
        self.settings.recent_wireless.truncate(5);
        self.persist();
    }

    fn use_discovered(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.discovered.get(index).cloned() else {
            return;
        };
        if service.service.contains("pairing") {
            self.pairing_address.update(cx, |state, cx| {
                state.set_value(service.address.clone(), window, cx);
            });
            self.push_log(format!(
                "[SYSTEM] Pairing address set to {}.",
                service.address
            ));
        } else {
            self.wireless_address.update(cx, |state, cx| {
                state.set_value(service.address.clone(), window, cx);
            });
            self.push_log(format!(
                "[SYSTEM] Connect address set to {}.",
                service.address
            ));
        }
        cx.notify();
    }

    fn use_recent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(address) = self.settings.recent_wireless.get(index).cloned() else {
            return;
        };
        self.wireless_address.update(cx, |state, cx| {
            state.set_value(address, window, cx);
        });
        cx.notify();
    }

    fn pair_wireless(&mut self, cx: &mut Context<Self>) {
        let address = self.pairing_address.read(cx).value().trim().to_string();
        let code = self.pairing_code.read(cx).value().trim().to_string();
        if address.is_empty() || code.is_empty() || self.wireless_busy {
            return;
        }
        self.wireless_busy = true;
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.pair(address, code).await;
            let _ = this.update(cx, |shell, cx| {
                shell.wireless_busy = false;
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => shell.push_error(error),
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_session(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            return;
        };
        if self.starting || self.running {
            return;
        }
        self.starting = true;
        self.push_log(format!("[SYSTEM] Starting mirror session for {device}..."));
        cx.notify();

        let config = self.session_config(&device);
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.start(config).await;
            if this.upgrade().is_none() {
                return;
            }
            let _ = this.update(cx, |shell, cx| {
                shell.starting = false;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => shell.push_error(error),
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_session(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            return;
        };
        self.push_log(format!("[SYSTEM] Stopping the session for {device}..."));
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.stop(&device).await;
            if this.upgrade().is_none() {
                return;
            }
            let _ = this.update(cx, |shell, cx| {
                if let Ok(Err(error)) = result {
                    shell.push_error(error);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn active_device_label(&self) -> SharedString {
        self.active_device
            .as_deref()
            .and_then(|serial| self.devices.iter().find(|device| device.serial == serial))
            .map(Device::label)
            .unwrap_or_else(|| t!("sidebar.noDevicesDetected").to_string().into())
    }

    fn session_config(&self, device: &str) -> open_scrcpy_core::config::ScrcpyConfig {
        match self.mode {
            CaptureMode::Desktop => self
                .settings
                .desktop
                .to_config(device, &self.settings.mirror),
            CaptureMode::Microphone => self.settings.mic.to_config(device, &self.settings.mirror),
            CaptureMode::Camera => self
                .settings
                .camera
                .to_config(device, &self.settings.mirror),
            _ => self.settings.mirror.to_config(device),
        }
    }

    fn panel_heading(&self) -> impl IntoElement {
        div()
            .id("panel-title")
            .role(Role::Heading)
            .aria_label(self.mode.label().to_string())
            .text_xl()
            .child(self.mode.label())
            .test_support()
    }

    fn session_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_start = self.active_device.is_some() && !self.starting && !self.running;
        let can_stop = self.running;
        let start_label = match self.mode {
            CaptureMode::Desktop => t!("controlPanel.ejectToDesktop").to_string(),
            CaptureMode::Microphone => t!("controlPanel.startMicBridge").to_string(),
            CaptureMode::Camera => t!("controlPanel.engageCamera").to_string(),
            _ => t!("controlPanel.startMission").to_string(),
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.active_device_label()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("start-mission")
                            .label(start_label)
                            .disabled(!can_start)
                            .on_click(cx.listener(|this, _, _, cx| this.start_session(cx))),
                    )
                    .child(
                        Button::new("stop-session")
                            .label(t!("controlPanel.stopSession").to_string())
                            .disabled(!can_stop)
                            .on_click(cx.listener(|this, _, _, cx| this.stop_session(cx))),
                    ),
            )
    }

    fn theme_modes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .children(ThemePreference::ALL.map(|preference| {
                Button::new(preference.id())
                    .label(theme_label(preference))
                    .selected(self.settings.theme == preference)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme(preference, window, cx)
                    }))
            }))
    }

    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale_label = format!(
            "{}: {}",
            t!("header.languageLabel"),
            i18n::label(&self.settings.locale)
        );
        let version = env!("CARGO_PKG_VERSION");

        div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().id("app-title").text_lg().child("OpenScrcpy"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("v{version}")),
                    ),
            )
            .child(div().flex_1())
            .child(self.theme_modes(cx))
            .child(
                Button::new("locale-toggle")
                    .label(locale_label)
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_locale(cx))),
            )
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = Vec::with_capacity(self.devices.len());
        for device in &self.devices {
            let serial = device.serial.clone();
            rows.push(
                Button::new(format!("device-{}", device.serial))
                    .label(device.label())
                    .selected(self.active_device.as_deref() == Some(device.serial.as_str()))
                    .w_full()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_device(serial.clone(), cx)),
                    ),
            );
        }

        let busy = self.wireless_busy;

        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .gap_2()
            .w_64()
            .flex_shrink_0()
            .p_3()
            .border_r_1()
            .border_color(cx.theme().border)
            .overflow_y_scroll()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sidebar.deviceHub").to_string()),
            )
            .when(self.devices.is_empty(), |this| {
                this.child(
                    div()
                        .id("device-empty")
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("sidebar.noDevicesDetected").to_string()),
                )
            })
            .children(rows)
            .child(
                Button::new("push-files")
                    .label(t!("sidebar.flashPushFiles").to_string())
                    .disabled(self.active_device.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
            )
            .child(
                div()
                    .id("push-hint")
                    .aria_label(t!("sidebar.flashPushSubtitle").to_string())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sidebar.flashPushSubtitle").to_string())
                    .test_support(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sidebar.wireless").to_string()),
                    )
                    .child(Input::new(&self.wireless_address).w_full())
                    .child(
                        Button::new("wireless-connect")
                            .label(t!("sidebar.connect").to_string())
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.connect_wireless(cx))),
                    )
                    .when(!self.discovered.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("sidebar.discoveredDevices").to_string()),
                        )
                        .children(self.discovered.iter().enumerate().map(|(index, service)| {
                            Button::new(format!("mdns-{index}"))
                                .label(format!("{} · {}", service.name, service.address))
                                .w_full()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.use_discovered(index, window, cx)
                                }))
                        }))
                    })
                    .when(!self.settings.recent_wireless.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("sidebar.recentDevices").to_string()),
                        )
                        .children(
                            self.settings.recent_wireless.iter().enumerate().map(
                                |(index, address)| {
                                    Button::new(format!("recent-{index}"))
                                        .label(address.clone())
                                        .w_full()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.use_recent(index, window, cx)
                                        }))
                                },
                            ),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sidebar.startPairing").to_string()),
                    )
                    .child(Input::new(&self.pairing_address).w_full())
                    .child(Input::new(&self.pairing_code).w_full())
                    .child(
                        Button::new("pair-start")
                            .label(t!("sidebar.startPairing").to_string())
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.pair_wireless(cx))),
                    ),
            )
    }

    fn mode_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(CaptureMode::ALL.map(|mode| {
                Button::new(mode.id())
                    .label(mode.label())
                    .selected(self.mode == mode)
                    .on_click(cx.listener(move |this, _, _, cx| this.switch_mode(mode, cx)))
            }))
    }

    fn mirror_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.mirror.clone();
        let bitrate = self.bitrate.read(cx).value().end().round() as u32;

        let fps_index = MirrorSettings::FPS
            .iter()
            .position(|fps| *fps == settings.fps)
            .unwrap_or(1);
        let size_index = MirrorSettings::SIZES
            .iter()
            .position(|size| *size == settings.max_size)
            .unwrap_or(0);
        let codec_index = MirrorSettings::CODECS
            .iter()
            .position(|codec| *codec == settings.video_codec)
            .unwrap_or(0);
        let renderer_index = if settings.renderer == "auto" {
            Some(0)
        } else {
            self.renderers
                .iter()
                .position(|(id, _)| *id == settings.renderer)
                .map(|index| index + 1)
        };
        let mut renderer_options =
            vec![Radio::new("renderer-auto").label(t!("controlPanel.rendererAuto").to_string())];
        for (id, label) in &self.renderers {
            renderer_options.push(Radio::new(format!("renderer-{id}")).label(label.clone()));
        }
        let rotation_index = DesktopSettings::ROTATIONS
            .iter()
            .position(|rotation| *rotation == settings.rotation)
            .unwrap_or(0);
        let rotation_options = DesktopSettings::ROTATIONS.map(|rotation| {
            Radio::new(format!("mirror-rotation-{rotation}")).label(format!("{rotation}°"))
        });

        let fps_options =
            MirrorSettings::FPS.map(|fps| Radio::new(format!("fps-{fps}")).label(fps.to_string()));
        let size_options = MirrorSettings::SIZES.map(|size| {
            let label = if size == 0 {
                t!("controlPanel.resolutionOriginal").to_string()
            } else {
                size.to_string()
            };
            Radio::new(format!("size-{size}")).label(label)
        });
        let codec_options = MirrorSettings::CODECS.map(|codec| {
            let label = match codec {
                "h265" => "H.265",
                "av1" => "AV1",
                _ => "H.264",
            };
            Radio::new(format!("codec-{codec}")).label(label)
        });

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.bitrate").to_string()),
                    )
                    .child(
                        div().w_64().child(
                            Slider::new(&self.bitrate)
                                .horizontal()
                                .accessibility_label(t!("controlPanel.bitrate").to_string()),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{bitrate} Mbps")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.fps").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mirror-fps")
                            .selected_index(Some(fps_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.mirror.fps = MirrorSettings::FPS[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(fps_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.resolution").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mirror-size")
                            .selected_index(Some(size_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.mirror.max_size = MirrorSettings::SIZES[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(size_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.codec").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mirror-codec")
                            .selected_index(Some(codec_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                let codec = MirrorSettings::CODECS
                                    .get(*index)
                                    .copied()
                                    .unwrap_or("h264");
                                this.settings.mirror.video_codec = codec.to_string();
                                this.persist();
                                cx.notify();
                            }))
                            .children(codec_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(
                        Switch::new("mirror-audio")
                            .checked(settings.audio)
                            .label(t!("sessionBehavior.forwardAudio").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.audio = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-vsync")
                            .checked(settings.vsync)
                            .label(t!("controlPanel.vsync").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.vsync = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-stay-awake")
                            .checked(settings.stay_awake)
                            .label(t!("sessionBehavior.stayAwake").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.stay_awake = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-screen-off")
                            .checked(settings.turn_screen_off)
                            .label(t!("sessionBehavior.screenOff").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.turn_screen_off = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-reconnect")
                            .checked(settings.auto_reconnect)
                            .label(t!("appBehavior.autoReconnect").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.auto_reconnect = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("controlPanel.inputEnhancements").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(
                        Switch::new("mirror-hid-keyboard")
                            .checked(settings.hid_keyboard)
                            .label(t!("controlPanel.hidKeyboard").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.hid_keyboard = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-hid-mouse")
                            .checked(settings.hid_mouse)
                            .label(t!("controlPanel.hidMouse").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.hid_mouse = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-pure-hid")
                            .checked(settings.otg_pure)
                            .label(t!("controlPanel.pureHid").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.otg_pure = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(
                        Switch::new("mirror-always-on-top")
                            .checked(settings.always_on_top)
                            .label(t!("sessionBehavior.alwaysOnTop").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.always_on_top = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-borderless")
                            .checked(settings.borderless)
                            .label(t!("sessionBehavior.borderless").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.borderless = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-fullscreen")
                            .checked(settings.fullscreen)
                            .label(t!("sessionBehavior.fullScreen").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.fullscreen = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-record")
                            .checked(settings.record)
                            .label(t!("sessionBehavior.recordFeed").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.record = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.graphicsRenderer").to_string()),
                    )
                    .child(
                        RadioGroup::vertical("mirror-renderer")
                            .selected_index(renderer_index)
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.mirror.renderer = if *index == 0 {
                                    "auto".to_string()
                                } else {
                                    this.renderers
                                        .get(*index - 1)
                                        .map(|entry: &(String, String)| entry.0.clone())
                                        .unwrap_or_else(|| "auto".to_string())
                                };
                                this.persist();
                                cx.notify();
                            }))
                            .children(renderer_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.rotation").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mirror-rotation")
                            .selected_index(Some(rotation_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.mirror.rotation = DesktopSettings::ROTATIONS[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(rotation_options),
                    ),
            )
    }

    fn mirror_summary(&self, cx: &Context<Self>) -> SharedString {
        let bitrate = self.bitrate.read(cx).value().end().round() as u32;
        let resolution = if self.settings.mirror.max_size == 0 {
            t!("controlPanel.resolutionOriginal").to_string()
        } else {
            self.settings.mirror.max_size.to_string()
        };
        let codec = match self.settings.mirror.video_codec.as_str() {
            "h265" => "H.265",
            "av1" => "AV1",
            _ => "H.264",
        };
        let audio = if self.settings.mirror.audio {
            t!("sessionBehavior.forwardAudio").to_string()
        } else {
            t!("controlPanel.audioOff").to_string()
        };
        let mut summary = format!(
            "{bitrate} Mbps · {} fps · {resolution} · {codec} · {audio}",
            self.settings.mirror.fps
        );
        if self.settings.mirror.record {
            summary.push_str(" · Record");
        }
        if self.settings.mirror.otg_pure {
            summary.push_str(" · OTG");
        } else if self.settings.mirror.hid_keyboard || self.settings.mirror.hid_mouse {
            summary.push_str(" · HID");
        }
        summary.into()
    }

    fn desktop_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let desktop = self.settings.desktop.clone();
        let size_index = DesktopSettings::SIZES
            .iter()
            .position(|&(width, height)| width == desktop.width && height == desktop.height)
            .unwrap_or(0);
        let dpi_index = DesktopSettings::DPI
            .iter()
            .position(|dpi| *dpi == desktop.dpi)
            .unwrap_or(3);
        let rotation_index = DesktopSettings::ROTATIONS
            .iter()
            .position(|rotation| *rotation == desktop.rotation)
            .unwrap_or(0);
        let max_size_index = MirrorSettings::SIZES
            .iter()
            .position(|size| *size == desktop.max_size)
            .unwrap_or(0);

        let size_options = DesktopSettings::SIZES.map(|(width, height)| {
            let label = match (width, height) {
                (1920, 1080) => t!("controlPanel.preset1080p").to_string(),
                (2560, 1440) => t!("controlPanel.preset1440p").to_string(),
                (3840, 2160) => t!("controlPanel.preset4k").to_string(),
                _ => t!("controlPanel.presetUltrawide").to_string(),
            };
            Radio::new(format!("desktop-size-{width}x{height}")).label(label)
        });
        let dpi_options = DesktopSettings::DPI
            .map(|dpi| Radio::new(format!("desktop-dpi-{dpi}")).label(dpi.to_string()));
        let rotation_options = DesktopSettings::ROTATIONS.map(|rotation| {
            Radio::new(format!("desktop-rotation-{rotation}")).label(format!("{rotation}°"))
        });
        let max_size_options = MirrorSettings::SIZES.map(|size| {
            let label = if size == 0 {
                t!("controlPanel.resolutionOriginal").to_string()
            } else {
                size.to_string()
            };
            Radio::new(format!("desktop-size-max-{size}")).label(label)
        });

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.virtualDisplayEngine").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("desktop-size")
                            .selected_index(Some(size_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                let (width, height) = DesktopSettings::SIZES[*index];
                                this.settings.desktop.width = width;
                                this.settings.desktop.height = height;
                                this.persist();
                                cx.notify();
                            }))
                            .children(size_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.uiScaling").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("desktop-dpi")
                            .selected_index(Some(dpi_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.desktop.dpi = DesktopSettings::DPI[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(dpi_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.rotation").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("desktop-rotation")
                            .selected_index(Some(rotation_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.desktop.rotation = DesktopSettings::ROTATIONS[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(rotation_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.resolution").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("desktop-max-size")
                            .selected_index(Some(max_size_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.desktop.max_size = MirrorSettings::SIZES[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(max_size_options),
                    ),
            )
            .child(
                Switch::new("desktop-flex")
                    .checked(desktop.flex_display)
                    .label(t!("controlPanel.flexDisplay").to_string())
                    .on_click(cx.listener(|this, checked, _, cx| {
                        this.settings.desktop.flex_display = *checked;
                        this.persist();
                        cx.notify();
                    })),
            )
    }

    fn desktop_summary(&self) -> SharedString {
        let desktop = &self.settings.desktop;
        let resolution = if desktop.max_size == 0 {
            t!("controlPanel.resolutionOriginal").to_string()
        } else {
            desktop.max_size.to_string()
        };
        let flex = if desktop.flex_display { " · Flex" } else { "" };
        format!(
            "{}×{} @ {} DPI · {}° · {}{}",
            desktop.width, desktop.height, desktop.dpi, desktop.rotation, resolution, flex
        )
        .into()
    }

    fn mic_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mic = self.settings.mic.clone();
        let source_index = MicSettings::SOURCES
            .iter()
            .position(|source| *source == mic.source)
            .unwrap_or(0);
        let codec_index = MicSettings::CODECS
            .iter()
            .position(|codec| *codec == mic.codec)
            .unwrap_or(0);
        let bitrate_index = MicSettings::BITRATES
            .iter()
            .position(|bitrate| *bitrate == mic.bitrate)
            .unwrap_or(2);

        let source_options = MicSettings::SOURCES.map(|source| {
            let label = if source == "mic-voice-communication" {
                t!("controlPanel.micSourceVoiceComm").to_string()
            } else {
                t!("controlPanel.micSourceMic").to_string()
            };
            Radio::new(format!("mic-source-{source}")).label(label)
        });
        let codec_options = MicSettings::CODECS.map(|codec| {
            let label = if codec == "aac" { "AAC" } else { "Opus" };
            Radio::new(format!("mic-codec-{codec}")).label(label)
        });
        let bitrate_options = MicSettings::BITRATES
            .map(|bitrate| Radio::new(format!("mic-bitrate-{bitrate}")).label(bitrate.to_string()));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.micSourceLabel").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mic-source")
                            .selected_index(Some(source_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                let source =
                                    MicSettings::SOURCES.get(*index).copied().unwrap_or("mic");
                                this.settings.mic.source = source.to_string();
                                this.persist();
                                cx.notify();
                            }))
                            .children(source_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.micCodecLabel").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mic-codec")
                            .selected_index(Some(codec_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                let codec =
                                    MicSettings::CODECS.get(*index).copied().unwrap_or("opus");
                                this.settings.mic.codec = codec.to_string();
                                this.persist();
                                cx.notify();
                            }))
                            .children(codec_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.micQualityLabel").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mic-bitrate")
                            .selected_index(Some(bitrate_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.mic.bitrate = MicSettings::BITRATES[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(bitrate_options),
                    ),
            )
            .child(div().flex().flex_wrap().gap_6().when(cfg!(unix), |this| {
                this.child(
                    Switch::new("mic-denoise")
                        .checked(mic.denoise)
                        .label(t!("controlPanel.micDenoiseLabel").to_string())
                        .on_click(cx.listener(|this, checked, _, cx| {
                            this.settings.mic.denoise = *checked;
                            this.persist();
                            cx.notify();
                        })),
                )
                .child(
                    Switch::new("mic-default-source")
                        .checked(mic.set_default_source)
                        .label(t!("controlPanel.micSetDefaultSource").to_string())
                        .on_click(cx.listener(|this, checked, _, cx| {
                            this.settings.mic.set_default_source = *checked;
                            this.persist();
                            cx.notify();
                        })),
                )
            }))
    }

    fn mic_summary(&self) -> SharedString {
        let mic = &self.settings.mic;
        let source = if mic.source == "mic-voice-communication" {
            t!("controlPanel.micSourceVoiceComm").to_string()
        } else {
            t!("controlPanel.micSourceMic").to_string()
        };
        let codec = if mic.codec == "aac" { "AAC" } else { "Opus" };
        let mut summary = format!("{source} · {codec} · {} kbps", mic.bitrate);
        if mic.denoise {
            summary.push_str(" · RNNoise");
        }
        if mic.set_default_source {
            summary.push_str(" · Default input");
        }
        summary.into()
    }

    fn mic_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let destination = match &self.mic_route {
            Some((display, _)) if !display.is_empty() => display.clone(),
            Some(_) => t!("controlPanel.micDestSearching").to_string(),
            None => t!("controlPanel.micDestIdle").to_string(),
        };
        let percent = self.mic_db.map(mic_level_percent).unwrap_or(0.0);
        let db_label = self
            .mic_db
            .map(|db| format!("{db:.0} dB"))
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id("mic-route")
                    .aria_label(format!(
                        "{}: {destination}",
                        t!("controlPanel.micDestLabel")
                    ))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.micDestLabel").to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(destination),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.micLevelLabel").to_string()),
                    )
                    .child(
                        div()
                            .w_64()
                            .h_2()
                            .rounded_full()
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(cx.theme().primary)
                                    .w(relative(percent)),
                            ),
                    )
                    .child(
                        div()
                            .id("mic-level")
                            .aria_label(format!("{}: {db_label}", t!("controlPanel.micLevelLabel")))
                            .test_support()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(db_label),
                    ),
            )
    }

    fn mic_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.mic_summary();

        div()
            .id("mic-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_6()
            .overflow_y_scroll()
            .child(self.panel_heading())
            .child(self.session_bar(cx))
            .child(self.mic_settings(cx))
            .child(self.mic_status(cx))
            .child(
                div()
                    .id("mic-summary")
                    .aria_label(summary.to_string())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary)
                    .test_support(),
            )
    }

    fn facing_label(facing: &str) -> SharedString {
        match facing {
            "front" => t!("controlPanel.facingFront"),
            "external" => t!("controlPanel.facingExternal"),
            _ => t!("controlPanel.facingBack"),
        }
        .to_string()
        .into()
    }

    fn refresh_lenses(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            return;
        };
        if self.cameras_busy {
            return;
        }
        self.cameras_busy = true;
        self.push_log(format!(
            "[SYSTEM] Running scrcpy --list-cameras for {device}..."
        ));
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.list_cameras(device).await;
            let _ = this.update(cx, |shell, cx| {
                shell.cameras_busy = false;
                match result {
                    Ok(value) => {
                        let output = value
                            .get("output")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let cameras = open_scrcpy_core::scrcpy::parse_cameras(output);
                        shell.push_log(if cameras.is_empty() {
                            "[SYSTEM] No cameras parsed from the output.".to_string()
                        } else {
                            format!("[SYSTEM] {} camera(s) found.", cameras.len())
                        });
                        shell.cameras = cameras;
                        if !shell
                            .cameras
                            .iter()
                            .any(|camera| camera.id == shell.settings.camera.camera_id)
                        {
                            shell.settings.camera.camera_id.clear();
                            shell.persist();
                        }
                    }
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn camera_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let camera = self.settings.camera.clone();
        let zoom = self.camera_zoom.read(cx).value().end().clamp(1.0, 10.0);
        let facing_index = CameraSettings::FACINGS
            .iter()
            .position(|facing| *facing == camera.facing)
            .unwrap_or(0);
        let aspect_index = CameraSettings::ASPECTS
            .iter()
            .position(|aspect| *aspect == camera.aspect)
            .unwrap_or(0);
        let max_size_index = MirrorSettings::SIZES
            .iter()
            .position(|size| *size == camera.max_size)
            .unwrap_or(0);

        let facing_options = CameraSettings::FACINGS.map(|facing| {
            Radio::new(format!("camera-facing-{facing}")).label(Self::facing_label(facing))
        });
        let aspect_options = CameraSettings::ASPECTS.map(|aspect| {
            let label = if aspect == "0" {
                t!("controlPanel.aspectDefault").to_string()
            } else {
                aspect.to_string()
            };
            Radio::new(format!("camera-aspect-{aspect}")).label(label)
        });
        let max_size_options = MirrorSettings::SIZES.map(|size| {
            let label = if size == 0 {
                t!("controlPanel.resolutionOriginal").to_string()
            } else {
                size.to_string()
            };
            Radio::new(format!("camera-size-max-{size}")).label(label)
        });

        let lens_index = if camera.camera_id.is_empty() {
            Some(0)
        } else {
            self.cameras
                .iter()
                .position(|camera_info| camera_info.id == camera.camera_id)
                .map(|index| index + 1)
        };
        let mut lens_options =
            vec![Radio::new("camera-lens-auto").label(t!("controlPanel.autoSelect").to_string())];
        for camera_info in &self.cameras {
            lens_options.push(
                Radio::new(format!("camera-lens-{}", camera_info.id))
                    .label(camera_info.name.clone()),
            );
        }
        let refresh_label = t!("controlPanel.refreshLenses").to_string();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.cameraDevice").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("camera-lens")
                            .selected_index(lens_index)
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.camera.camera_id = if *index == 0 {
                                    String::new()
                                } else {
                                    this.cameras
                                        .get(*index - 1)
                                        .map(|camera: &CameraInfo| camera.id.clone())
                                        .unwrap_or_default()
                                };
                                this.persist();
                                cx.notify();
                            }))
                            .children(lens_options),
                    )
                    .child(
                        Button::new("refresh-lenses")
                            .label(refresh_label)
                            .disabled(self.active_device.is_none() || self.cameras_busy)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_lenses(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.facing").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("camera-facing")
                            .selected_index(Some(facing_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                let facing = CameraSettings::FACINGS
                                    .get(*index)
                                    .copied()
                                    .unwrap_or("back");
                                this.settings.camera.facing = facing.to_string();
                                this.persist();
                                cx.notify();
                            }))
                            .children(facing_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.aspect").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("camera-aspect")
                            .selected_index(Some(aspect_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                let aspect =
                                    CameraSettings::ASPECTS.get(*index).copied().unwrap_or("0");
                                this.settings.camera.aspect = aspect.to_string();
                                this.persist();
                                cx.notify();
                            }))
                            .children(aspect_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.cameraZoom").to_string()),
                    )
                    .child(
                        div().w_64().child(
                            Slider::new(&self.camera_zoom)
                                .horizontal()
                                .accessibility_label(t!("controlPanel.cameraZoom").to_string()),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{zoom:.1}×")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.resolution").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("camera-max-size")
                            .selected_index(Some(max_size_index))
                            .on_change(cx.listener(|this, index, _, cx| {
                                this.settings.camera.max_size = MirrorSettings::SIZES[*index];
                                this.persist();
                                cx.notify();
                            }))
                            .children(max_size_options),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(
                        Switch::new("camera-torch")
                            .checked(camera.torch)
                            .label(t!("controlPanel.cameraTorch").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.camera.torch = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("camera-high-speed")
                            .checked(camera.high_speed)
                            .label(t!("controlPanel.cameraHighSpeed").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.camera.high_speed = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("camera-mic-sink")
                            .checked(camera.mic_sink)
                            .label(t!("controlPanel.cameraMicSink").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.camera.mic_sink = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    ),
            )
    }

    fn camera_summary(&self, cx: &Context<Self>) -> SharedString {
        let camera = &self.settings.camera;
        let zoom = self.camera_zoom.read(cx).value().end().clamp(1.0, 10.0);
        let aspect = if camera.aspect == "0" {
            t!("controlPanel.aspectDefault").to_string()
        } else {
            camera.aspect.clone()
        };
        let mut summary = format!(
            "{} · {} · {:.1}×",
            Self::facing_label(&camera.facing),
            aspect,
            zoom
        );
        if camera.torch {
            summary.push_str(" · Torch");
        }
        if camera.high_speed {
            summary.push_str(" · High-speed");
        }
        if camera.mic_sink {
            summary.push_str(" · Virtual mic");
        }
        summary.into()
    }

    fn camera_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.camera_summary(cx);

        div()
            .id("camera-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_6()
            .overflow_y_scroll()
            .child(self.panel_heading())
            .child(self.session_bar(cx))
            .child(self.camera_settings(cx))
            .child(
                div()
                    .id("camera-summary")
                    .aria_label(summary.to_string())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary)
                    .test_support(),
            )
    }

    fn screen_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.mirror_summary(cx);

        div()
            .id("mirror-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_6()
            .overflow_y_scroll()
            .child(self.panel_heading())
            .child(self.session_bar(cx))
            .child(self.mirror_settings(cx))
            .child(
                div()
                    .id("mirror-summary")
                    .aria_label(summary.to_string())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary)
                    .test_support(),
            )
    }

    fn desktop_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.desktop_summary();

        div()
            .id("desktop-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_6()
            .overflow_y_scroll()
            .child(self.panel_heading())
            .child(self.session_bar(cx))
            .child(self.desktop_settings(cx))
            .child(
                div()
                    .id("desktop-summary")
                    .aria_label(summary.to_string())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary)
                    .test_support(),
            )
    }

    fn run_terminal_command(&mut self, cx: &mut Context<Self>) {
        let command = self.terminal_input.read(cx).value().trim().to_string();
        if command.is_empty() || self.terminal_busy {
            return;
        }
        self.terminal_busy = true;
        self.push_log(format!("> {command}"));
        cx.notify();

        let device = self.active_device.clone();
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.run_terminal(device, command).await;
            let _ = this.update(cx, |shell, cx| {
                shell.terminal_busy = false;
                match result {
                    Ok(value) => {
                        let stdout = value
                            .get("stdout")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let stderr = value
                            .get("stderr")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        for line in stdout.lines().chain(stderr.lines()) {
                            if !line.trim().is_empty() {
                                shell.push_log(format!("[TERMINAL] {line}"));
                            }
                        }
                        if value.get("success").and_then(Value::as_bool) != Some(true)
                            && let Some(message) = value.get("message").and_then(Value::as_str)
                        {
                            shell.push_error(message.to_string());
                        }
                    }
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_console(&mut self, cx: &mut Context<Self>) {
        if !self.logs.is_empty() {
            self.logs.clear();
            cx.notify();
        }
    }

    fn copy_console(&mut self, cx: &mut Context<Self>) {
        if self.logs.is_empty() {
            return;
        }
        let text = self.logs.join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.push_log(t!("logPanel.copied").to_string());
        cx.notify();
    }

    fn save_report(&mut self, cx: &mut Context<Self>) {
        let Some(downloads) = downloads_dir() else {
            self.push_error("downloads directory is unavailable");
            cx.notify();
            return;
        };
        let report = build_report(ReportContext {
            version: env!("CARGO_PKG_VERSION"),
            locale: &self.settings.locale,
            theme: &format!("{:?}", self.settings.theme),
            devices: &self.devices,
            active_device: self.active_device.as_deref(),
            mode: self.mode,
            running: self.running,
            logs: &self.logs,
        });
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let name = format!("opensrcpy-report-{timestamp}.json");
        match serde_json::to_string_pretty(&report)
            .map_err(|error| error.to_string())
            .and_then(|content| open_scrcpy_core::report::write_report(&downloads, &content, &name))
        {
            Ok(path) => {
                self.push_log(t!("logPanel.diagnosticReportSaved", fileName = path).to_string());
            }
            Err(error) => self.push_error(error),
        }
        cx.notify();
    }

    fn console(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut lines = Vec::with_capacity(self.logs.len());
        for line in &self.logs {
            lines.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(line.clone()),
            );
        }

        div()
            .flex()
            .flex_col()
            .h_40()
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("logPanel.systemConsole").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("console-clear")
                            .label(t!("logPanel.clear").to_string())
                            .disabled(self.logs.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.clear_console(cx))),
                    )
                    .child(
                        Button::new("console-copy")
                            .label(t!("logPanel.copy").to_string())
                            .disabled(self.logs.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.copy_console(cx))),
                    )
                    .child(
                        Button::new("console-report")
                            .label(t!("logPanel.report").to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.save_report(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pb_2()
                    .child(Input::new(&self.terminal_input).flex_1())
                    .child(
                        Button::new("terminal-run")
                            .label(t!("logPanel.run").to_string())
                            .disabled(self.terminal_busy)
                            .on_click(cx.listener(|this, _, _, cx| this.run_terminal_command(cx))),
                    ),
            )
            .child(
                div()
                    .id("console-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .gap_1()
                    .px_4()
                    .py_2()
                    .when(self.logs.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("logPanel.waitingForSequence").to_string()),
                        )
                    })
                    .children(lines),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.handle_drop(paths, cx)))
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(self.mode_tabs(cx))
                            .child(match self.mode {
                                CaptureMode::Screen => self.screen_panel(cx).into_any_element(),
                                CaptureMode::Desktop => self.desktop_panel(cx).into_any_element(),
                                CaptureMode::Microphone => self.mic_panel(cx).into_any_element(),
                                CaptureMode::Camera => self.camera_panel(cx).into_any_element(),
                            }),
                    ),
            )
            .child(self.console(cx))
    }
}

fn parse_renderers(value: &Value) -> Vec<(String, String)> {
    value
        .get("supportedDrivers")
        .and_then(Value::as_array)
        .map(|drivers| {
            drivers
                .iter()
                .filter_map(|driver| {
                    let id = driver.get("id").and_then(Value::as_str)?;
                    let label = driver.get("label").and_then(Value::as_str).unwrap_or(id);
                    Some((id.to_string(), label.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_discovered(value: &Value) -> Vec<DiscoveredService> {
    let mut services = Vec::new();
    let Some(items) = value.get("services").and_then(Value::as_array) else {
        return services;
    };
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(service) = item.get("service").and_then(Value::as_str) else {
            continue;
        };
        let Some(address) = item.get("address").and_then(Value::as_str) else {
            continue;
        };
        if address.is_empty()
            || services.iter().any(|entry: &DiscoveredService| {
                entry.address == address && entry.service == service
            })
        {
            continue;
        }
        services.push(DiscoveredService {
            name: name.to_string(),
            service: service.to_string(),
            address: address.to_string(),
        });
    }
    services
}

fn spawn_wireless_discovery(cx: &mut Context<AppShell>, services: AppServices) {
    cx.spawn(async move |this, cx| {
        loop {
            let value = services.list_mdns().await;
            let discovered = value
                .ok()
                .map(|value| parse_discovered(&value))
                .unwrap_or_default();
            if this
                .update(cx, |shell, cx| {
                    shell.discovered = discovered;
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(5)).await;
        }
    })
    .detach();
}

fn spawn_renderer_load(cx: &mut Context<AppShell>, services: AppServices) {
    cx.spawn(async move |this, cx| {
        let value = services.get_render_drivers().await;
        let renderers = value
            .ok()
            .map(|value| parse_renderers(&value))
            .unwrap_or_default();
        let _ = this.update(cx, |shell, cx| {
            shell.renderers = renderers;
            cx.notify();
        });
    })
    .detach();
}

fn spawn_platform_commands(
    cx: &mut Context<AppShell>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<PlatformCommand>,
) {
    cx.spawn(async move |this, cx| {
        while let Some(command) = commands.recv().await {
            match command {
                PlatformCommand::Recenter => {
                    if this
                        .update(cx, |shell, cx| shell.recenter_active(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    })
    .detach();
}

fn spawn_event_loop(cx: &mut Context<AppShell>, mut events: EventReceiver) {
    cx.spawn(async move |this, cx| {
        while let Some(event) = events.recv().await {
            if this
                .update(cx, |shell, cx| shell.apply_event(event, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn spawn_device_poll(cx: &mut Context<AppShell>, services: AppServices) {
    cx.spawn(async move |this, cx| {
        loop {
            let devices = services.list_devices().await;
            let parsed = devices
                .ok()
                .map(|value| Device::parse(&value))
                .unwrap_or_default();
            if this
                .update(cx, |shell, cx| shell.apply_devices(parsed, cx))
                .is_err()
            {
                break;
            }
            cx.background_executor().timer(DEVICE_POLL_INTERVAL).await;
        }
    })
    .detach();
}

fn downloads_dir() -> Option<std::path::PathBuf> {
    directories::UserDirs::new().and_then(|dirs| dirs.download_dir().map(|path| path.to_path_buf()))
}

/// Everything the diagnostic report needs, so the builder stays a pure one-arg
/// function and can be unit tested without a window.
struct ReportContext<'a> {
    version: &'a str,
    locale: &'a str,
    theme: &'a str,
    devices: &'a [Device],
    active_device: Option<&'a str>,
    mode: CaptureMode,
    running: bool,
    logs: &'a [String],
}

fn build_report(context: ReportContext<'_>) -> Value {
    serde_json::json!({
        "app": "OpenScrcpy",
        "version": context.version,
        "locale": context.locale,
        "theme": context.theme,
        "mode": format!("{:?}", context.mode),
        "running": context.running,
        "activeDevice": context.active_device,
        "devices": context.devices.iter().map(|device| serde_json::json!({
            "serial": device.serial,
            "model": device.model,
            "state": format!("{:?}", device.state),
        })).collect::<Vec<_>>(),
        "logs": context.logs,
    })
}

/// Native notification for a session that ended without the user asking.
fn notify_unexpected_end(cx: &mut Context<AppShell>, payload: &Value) {
    let reason = payload.get("reason").and_then(Value::as_str).unwrap_or("");
    let success = payload.get("success").and_then(Value::as_bool);
    let unexpected = reason == "error" || (reason == "exited" && success == Some(false));
    if !unexpected {
        return;
    }
    let device = payload.get("device").and_then(Value::as_str).unwrap_or("");
    cx.show_system_notification(SystemNotification {
        tag: format!("session-ended-{device}").into(),
        title: t!("notifications.sessionEndedTitle").to_string().into(),
        body: t!("notifications.sessionEndedBody", device = device)
            .to_string()
            .into(),
        actions: Vec::new(),
    });
}

/// Maps the core's dBFS meter to a 0..=1 bar width; -60 dB (or lower) is empty.
fn mic_level_percent(db: f64) -> f32 {
    (((db.clamp(-60.0, 0.0)) + 60.0) / 60.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_ready_and_unavailable_devices() {
        let payload = json!({
            "error": false,
            "devices": ["emulator-5554"],
            "deviceModels": { "emulator-5554": "Pixel 9" },
            "unavailable": [
                { "serial": "10.0.0.2:5555", "state": "unauthorized", "model": "Pixel 7" }
            ],
        });
        let devices = Device::parse(&payload);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].serial, "emulator-5554");
        assert_eq!(devices[0].model, "Pixel 9");
        assert_eq!(devices[0].state, DeviceState::Online);
        assert_eq!(devices[1].state, DeviceState::Unauthorized);
    }

    #[test]
    fn diagnostic_report_carries_state_and_logs() {
        let devices = vec![Device {
            serial: "device1".into(),
            model: "Pixel".into(),
            state: DeviceState::Online,
        }];
        let logs = vec!["[SYSTEM] hello".to_string()];
        let report = build_report(ReportContext {
            version: "3.0.0",
            locale: "pt-BR",
            theme: "Dark",
            devices: &devices,
            active_device: Some("device1"),
            mode: CaptureMode::Screen,
            running: true,
            logs: &logs,
        });
        assert_eq!(report["version"], "3.0.0");
        assert_eq!(report["locale"], "pt-BR");
        assert_eq!(report["devices"][0]["serial"], "device1");
        assert_eq!(report["logs"][0], "[SYSTEM] hello");
    }

    #[test]
    fn parses_and_dedupes_discovered_services() {
        let payload = json!({
            "services": [
                { "name": "phone", "service": "_adb-tls-pairing._tcp", "address": "10.0.0.2:37123" },
                { "name": "phone", "service": "_adb-tls-pairing._tcp", "address": "10.0.0.2:37123" },
                { "name": "phone-connect", "service": "_adb-tls-connect._tcp", "address": "10.0.0.2:5555" }
            ]
        });
        let services = parse_discovered(&payload);
        assert_eq!(services.len(), 2);
        assert!(services[0].service.contains("pairing"));
        assert!(parse_discovered(&json!({})).is_empty());
    }

    #[test]
    fn parses_render_drivers() {
        let payload = json!({
            "supportsRenderDriver": true,
            "supportedDrivers": [
                { "id": "opengl", "label": "OpenGL" },
                { "id": "vulkan", "label": "Vulkan" }
            ]
        });
        let drivers = parse_renderers(&payload);
        assert_eq!(drivers.len(), 2);
        assert_eq!(drivers[0], ("opengl".to_string(), "OpenGL".to_string()));
        assert!(parse_renderers(&json!({})).is_empty());
    }

    #[test]
    fn mic_level_percent_is_clamped() {
        assert_eq!(mic_level_percent(0.0), 1.0);
        assert_eq!(mic_level_percent(-60.0), 0.0);
        assert_eq!(mic_level_percent(-120.0), 0.0);
        assert!((mic_level_percent(-30.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn empty_payload_yields_no_devices() {
        assert!(Device::parse(&json!({})).is_empty());
        assert!(Device::parse(&json!({ "error": true })).is_empty());
    }
}
