//! The application shell: device hub, mode tabs, session controls and console.
//!
//! Presentation state lives here; device and session state come from
//! `open-scrcpy-core` through [`AppServices`]. The event loop and the device
//! poller are spawned in `new` and mutate this entity through its weak handle.

use std::time::Duration;

use gpui_kit::base::Selectable as _;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, button::Button};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TestSupportExt as _, Window, WindowBounds, div,
};
use open_scrcpy_core::events::{AppEvent, EventReceiver};
use rust_i18n::t;
use serde_json::Value;

use crate::i18n;
use crate::services::AppServices;
use crate::settings::{AppSettings, MirrorSettings, SettingsStore, ThemePreference};
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

#[derive(Clone, Copy, PartialEq, Eq)]
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

    fn is_implemented(self) -> bool {
        matches!(self, Self::Screen)
    }
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
}

impl AppShell {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        settings: AppSettings,
        store: SettingsStore,
        services: AppServices,
        events: EventReceiver,
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
        if services.polls_devices() {
            spawn_device_poll(cx, services.clone());
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
                }
            }
            AppEvent::AudioStatus(_) | AppEvent::WindowPos(_) => {}
            AppEvent::MicRoute { .. } | AppEvent::MicLevel(_) => {}
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

        let config = self.settings.mirror.to_config(&device);
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

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_64()
            .flex_shrink_0()
            .p_3()
            .border_r_1()
            .border_color(cx.theme().border)
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
        format!(
            "{bitrate} Mbps · {} fps · {resolution} · {codec} · {audio}",
            self.settings.mirror.fps
        )
        .into()
    }

    fn screen_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_start = self.active_device.is_some() && !self.starting && !self.running;
        let can_stop = self.running;
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
            .child(
                div()
                    .id("panel-title")
                    .role(Role::Heading)
                    .aria_label(self.mode.label().to_string())
                    .text_xl()
                    .child(self.mode.label())
                    .test_support(),
            )
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
                            .label(t!("controlPanel.startMission").to_string())
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

    fn pending_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_6()
            .child(
                div()
                    .id("panel-title")
                    .role(Role::Heading)
                    .aria_label(self.mode.label().to_string())
                    .text_xl()
                    .child(self.mode.label())
                    .test_support(),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("controlPanel.modeUnavailable").to_string()),
            )
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
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("logPanel.systemConsole").to_string()),
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
                            .child(if self.mode.is_implemented() {
                                self.screen_panel(cx).into_any_element()
                            } else {
                                self.pending_panel(cx).into_any_element()
                            }),
                    ),
            )
            .child(self.console(cx))
    }
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
    fn empty_payload_yields_no_devices() {
        assert!(Device::parse(&json!({})).is_empty());
        assert!(Device::parse(&json!({ "error": true })).is_empty());
    }
}
