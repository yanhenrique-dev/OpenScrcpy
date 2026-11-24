//! The application shell: device hub, mode tabs, session controls and console.
//!
//! Presentation state lives here; device and session state come from
//! `open-scrcpy-core` through [`AppServices`]. The event loop and the device
//! poller are spawned in `new` and mutate this entity through its weak handle.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::LogicalStyledExt as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::IndexPath;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::TitleBar;
use gpui_kit::component::avatar::Avatar;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::resizable::{resizable_panel, v_resizable};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::px;
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, ExternalPaths, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, SystemNotification,
    TestSupportExt as _, Window, WindowBounds, div, relative,
};
use open_scrcpy_core::capabilities::Capabilities;
use open_scrcpy_core::events::{AppEvent, EventReceiver};
use open_scrcpy_core::scrcpy::CameraInfo;
use open_scrcpy_platform::shortcuts::PlatformCommand;
use rust_i18n::t;
use serde_json::Value;

use crate::i18n;
use crate::layout;
use crate::services::{AppServices, AppUpdate};
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

fn shortcut_label(key: &str) -> SharedString {
    match key {
        "F" => t!("shortcuts.full"),
        "H" => t!("shortcuts.home"),
        "B" => t!("shortcuts.back"),
        "S" => t!("shortcuts.recents"),
        "P" => t!("shortcuts.power"),
        "R" => t!("shortcuts.rotate"),
        "V" => t!("shortcuts.paste"),
        _ => t!("shortcuts.off"),
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

    fn icon(self) -> IconName {
        match self {
            Self::Screen => IconName::Monitor,
            Self::Camera => IconName::Camera,
            Self::Desktop => IconName::AppWindow,
            Self::Microphone => IconName::Mic,
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

impl SearchableListItem for ThemePreference {
    type Value = ThemePreference;

    fn title(&self) -> SharedString {
        theme_label(*self)
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

#[derive(Clone)]
struct ChoiceItem {
    value: String,
    label: SharedString,
}

impl SearchableListItem for ChoiceItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

fn choice_select(items: Vec<(&str, String)>) -> SearchableVec<ChoiceItem> {
    let items: Vec<ChoiceItem> = items
        .into_iter()
        .map(|(value, label)| ChoiceItem {
            value: value.to_string(),
            label: label.into(),
        })
        .collect();
    SearchableVec::new(items)
}

fn choice_select_owned(items: Vec<(String, String)>) -> SearchableVec<ChoiceItem> {
    let items: Vec<ChoiceItem> = items
        .into_iter()
        .map(|(value, label)| ChoiceItem {
            value,
            label: label.into(),
        })
        .collect();
    SearchableVec::new(items)
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
    fn display_name(&self) -> SharedString {
        if self.model.is_empty() {
            self.serial.clone().into()
        } else {
            self.model.clone().into()
        }
    }

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
    log_filter: LogFilter,
    theme_select: Entity<SelectState<SearchableVec<ThemePreference>>>,
    _theme_subscription: Subscription,
    fps_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    size_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    codec_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    renderer_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    rotation_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    camera_facing_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    camera_aspect_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    camera_max_size_select: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    _mirror_selects_subscriptions: Vec<Subscription>,
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
    console_scroll: ScrollHandle,
    cameras: Vec<CameraInfo>,
    cameras_busy: bool,
    cameras_for: Option<String>,
    cameras_error: bool,
    cameras_generation: u64,
    camera_cache: HashMap<String, Vec<CameraInfo>>,
    capabilities: Option<Capabilities>,
    desktop_width: Entity<SliderState>,
    desktop_height: Entity<SliderState>,
    desktop_dpi: Entity<SliderState>,
    _desktop_subscriptions: Vec<Subscription>,
    update_available: Option<String>,
    pending_update: Option<AppUpdate>,
    installing_update: bool,
    checking_update: bool,
    pairing_attempt: Option<u32>,
    scrcpy_ready: Option<bool>,
    android_versions: HashMap<String, String>,
    android_version_requests: HashSet<String>,
    downloading: bool,
    download_percent: Option<u64>,
    download_error: bool,
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
            spawn_core_check(cx, services.clone());
            spawn_capability_load(cx, services.clone());
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
        let desktop_width = cx.new(|_| {
            SliderState::new()
                .min(480.0)
                .max(3840.0)
                .step(10.0)
                .default_value(settings.desktop.width as f32)
        });
        let desktop_height = cx.new(|_| {
            SliderState::new()
                .min(320.0)
                .max(2160.0)
                .step(10.0)
                .default_value(settings.desktop.height as f32)
        });
        let desktop_dpi = cx.new(|_| {
            SliderState::new()
                .min(120.0)
                .max(640.0)
                .step(10.0)
                .default_value(settings.desktop.dpi as f32)
        });
        let desktop_subscriptions = vec![
            cx.subscribe(&desktop_width, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Release(value) = event {
                    let width = value.end().round().clamp(480.0, 3840.0) as u32;
                    if this.settings.desktop.width != width {
                        this.settings.desktop.width = width;
                        this.persist();
                        cx.notify();
                    }
                }
            }),
            cx.subscribe(&desktop_height, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Release(value) = event {
                    let height = value.end().round().clamp(320.0, 2160.0) as u32;
                    if this.settings.desktop.height != height {
                        this.settings.desktop.height = height;
                        this.persist();
                        cx.notify();
                    }
                }
            }),
            cx.subscribe(&desktop_dpi, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Release(value) = event {
                    let dpi = value.end().round().clamp(120.0, 640.0) as u32;
                    if this.settings.desktop.dpi != dpi {
                        this.settings.desktop.dpi = dpi;
                        this.persist();
                        cx.notify();
                    }
                }
            }),
        ];

        let theme_index = ThemePreference::ALL
            .iter()
            .position(|preference| *preference == settings.theme)
            .unwrap_or(2);
        let theme_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(ThemePreference::ALL.to_vec()),
                Some(IndexPath::new(theme_index)),
                window,
                cx,
            )
            .searchable(true)
        });
        let theme_subscription = cx.subscribe_in(
            &theme_select,
            window,
            |this, _, event: &SelectEvent<SearchableVec<ThemePreference>>, window, cx| {
                if let SelectEvent::Confirm(Some(theme)) = event {
                    this.set_theme(*theme, window, cx);
                }
            },
        );

        let fps_values = ["30", "60", "90"];
        let fps_index = fps_values
            .iter()
            .position(|value| value.parse::<u32>().ok() == Some(settings.mirror.fps))
            .unwrap_or(1);
        let fps_select = cx.new(|cx| {
            let items = fps_values
                .iter()
                .map(|value| (*value, (*value).to_string()))
                .collect();
            SelectState::new(
                choice_select(items),
                Some(IndexPath::new(fps_index)),
                window,
                cx,
            )
        });

        let size_values = ["0", "1920", "1280", "720"];
        let size_index = size_values
            .iter()
            .position(|value| value.parse::<u32>().ok() == Some(settings.mirror.max_size))
            .unwrap_or(0);
        let size_select = cx.new(|cx| {
            let original = t!("controlPanel.resolutionOriginal").to_string();
            let labels = [
                original,
                "1920".to_string(),
                "1280".to_string(),
                "720".to_string(),
            ];
            let items = size_values
                .iter()
                .zip(labels)
                .map(|(value, label)| (*value, label))
                .collect();
            SelectState::new(
                choice_select(items),
                Some(IndexPath::new(size_index)),
                window,
                cx,
            )
        });

        let codec_values = ["h264", "h265", "av1"];
        let codec_labels = ["H.264", "H.265", "AV1"];
        let codec_index = codec_values
            .iter()
            .position(|value| *value == settings.mirror.video_codec)
            .unwrap_or(0);
        let codec_select = cx.new(|cx| {
            let items = codec_values
                .iter()
                .zip(codec_labels)
                .map(|(value, label)| (*value, label.to_string()))
                .collect();
            SelectState::new(
                choice_select(items),
                Some(IndexPath::new(codec_index)),
                window,
                cx,
            )
        });

        let renderer_values = ["auto", "opengl", "opengles2", "opengl_es", "software"];
        let renderer_labels = [
            t!("controlPanel.rendererAuto").to_string(),
            "OpenGL".to_string(),
            "OpenGL ES 2".to_string(),
            "OpenGL ES".to_string(),
            "Software".to_string(),
        ];
        let renderer_index = renderer_values
            .iter()
            .position(|value| *value == settings.mirror.renderer)
            .unwrap_or(0)
            .min(renderer_values.len() - 1);
        let renderer_select = cx.new(|cx| {
            let items = renderer_values
                .iter()
                .zip(renderer_labels)
                .map(|(value, label)| (*value, label))
                .collect();
            SelectState::new(
                choice_select(items),
                Some(IndexPath::new(renderer_index)),
                window,
                cx,
            )
        });

        let rotation_values = ["0", "90", "180", "270"];
        let rotation_index = rotation_values
            .iter()
            .position(|value| value.parse::<u32>().ok() == Some(settings.mirror.rotation))
            .unwrap_or(0);
        let rotation_select = cx.new(|cx| {
            let items = rotation_values
                .iter()
                .map(|value| (*value, format!("{value}°")))
                .collect();
            SelectState::new(
                choice_select(items),
                Some(IndexPath::new(rotation_index)),
                window,
                cx,
            )
        });

        let facing_values = CameraSettings::FACINGS;
        let facing_items = facing_values
            .iter()
            .map(|facing| (*facing, Self::facing_label(facing).to_string()))
            .collect();
        let facing_index = facing_values
            .iter()
            .position(|facing| *facing == settings.camera.facing)
            .unwrap_or(0);
        let camera_facing_select = cx.new(|cx| {
            SelectState::new(
                choice_select(facing_items),
                Some(IndexPath::new(facing_index)),
                window,
                cx,
            )
        });

        let aspect_values = CameraSettings::ASPECTS;
        let aspect_items = aspect_values
            .iter()
            .map(|aspect| {
                let label = if *aspect == "0" {
                    t!("controlPanel.aspectDefault").to_string()
                } else {
                    aspect.to_string()
                };
                (*aspect, label)
            })
            .collect();
        let aspect_index = aspect_values
            .iter()
            .position(|aspect| *aspect == settings.camera.aspect)
            .unwrap_or(0);
        let camera_aspect_select = cx.new(|cx| {
            SelectState::new(
                choice_select(aspect_items),
                Some(IndexPath::new(aspect_index)),
                window,
                cx,
            )
        });

        let camera_size_values = MirrorSettings::SIZES;
        let camera_size_items = camera_size_values
            .iter()
            .map(|size| {
                let label = if *size == 0 {
                    t!("controlPanel.resolutionOriginal").to_string()
                } else {
                    size.to_string()
                };
                (size.to_string(), label)
            })
            .collect::<Vec<(String, String)>>();
        let camera_size_index = camera_size_values
            .iter()
            .position(|size| *size == settings.camera.max_size)
            .unwrap_or(0);
        let camera_max_size_select = cx.new(|cx| {
            SelectState::new(
                choice_select_owned(camera_size_items),
                Some(IndexPath::new(camera_size_index)),
                window,
                cx,
            )
        });

        let mirror_selects_subscriptions = vec![
            cx.subscribe_in(
                &fps_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event
                        && let Ok(fps) = value.parse::<u32>()
                    {
                        this.settings.mirror.fps = fps;
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &size_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event
                        && let Ok(size) = value.parse::<u32>()
                    {
                        this.settings.mirror.max_size = size;
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &codec_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.settings.mirror.video_codec = value.clone();
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &renderer_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.settings.mirror.renderer = value.clone();
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &rotation_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event
                        && let Ok(rotation) = value.parse::<u32>()
                    {
                        this.settings.mirror.rotation = rotation;
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &camera_facing_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.settings.camera.facing = value.clone();
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &camera_aspect_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.settings.camera.aspect = value.clone();
                        this.persist();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &camera_max_size_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event
                        && let Ok(size) = value.parse::<u32>()
                    {
                        this.settings.camera.max_size = size;
                        this.persist();
                        cx.notify();
                    }
                },
            ),
        ];

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
            log_filter: LogFilter::All,
            theme_select,
            _theme_subscription: theme_subscription,
            fps_select,
            size_select,
            codec_select,
            renderer_select,
            rotation_select,
            camera_facing_select,
            camera_aspect_select,
            camera_max_size_select,
            _mirror_selects_subscriptions: mirror_selects_subscriptions,
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
            console_scroll: ScrollHandle::new(),
            cameras: Vec::new(),
            cameras_busy: false,
            cameras_for: None,
            cameras_error: false,
            cameras_generation: 0,
            camera_cache: HashMap::new(),
            capabilities: None,
            desktop_width,
            desktop_height,
            desktop_dpi,
            _desktop_subscriptions: desktop_subscriptions,
            update_available: None,
            pending_update: None,
            installing_update: false,
            checking_update: false,
            pairing_attempt: None,
            scrcpy_ready: None,
            android_versions: HashMap::new(),
            android_version_requests: HashSet::new(),
            downloading: false,
            download_percent: None,
            download_error: false,
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

    fn set_locale(&mut self, locale: &str, cx: &mut Context<Self>) {
        if self.settings.locale == locale {
            return;
        }
        self.settings.locale = locale.to_string();
        i18n::apply(locale);
        layout::apply(locale, cx);
        self.persist();
        cx.refresh_windows();
    }

    fn open_language_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.settings.locale.clone();
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let mut options = Vec::with_capacity(i18n::SUPPORTED_LOCALES.len());
            for locale in i18n::SUPPORTED_LOCALES {
                let weak = weak.clone();
                let selected = locale == current;
                options.push(
                    Button::new(format!("locale-{locale}"))
                        .label(i18n::label(locale))
                        .selected(selected)
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.set_locale(locale, cx));
                            window.close_dialog(cx);
                        }),
                );
            }
            dialog
                .title(t!("header.languageLabel").to_string())
                .child(div().flex().flex_col().gap_2().children(options))
        });
    }

    fn open_settings_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_sheet(cx, move |sheet, _, cx| {
            let (notifications, remember, current_locale, current_theme) = weak
                .upgrade()
                .map(|entity| {
                    let shell = entity.read(cx);
                    (
                        shell.settings.notifications_enabled,
                        shell.settings.remember_window_position,
                        shell.settings.locale.clone(),
                        shell.settings.theme,
                    )
                })
                .unwrap_or((true, true, "en".to_string(), ThemePreference::System));

            let notify_weak = weak.clone();
            let remember_weak = weak.clone();

            sheet.title(t!("appBehavior.title").to_string()).child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        Switch::new("settings-notifications")
                            .checked(notifications)
                            .label(t!("appBehavior.notifications").to_string())
                            .on_click(move |checked, _, cx| {
                                let _ = notify_weak.update(cx, |this, cx| {
                                    this.settings.notifications_enabled = *checked;
                                    this.persist();
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Switch::new("settings-remember-position")
                            .checked(remember)
                            .label(t!("sessionBehavior.rememberWindowPosition").to_string())
                            .on_click(move |checked, _, cx| {
                                let _ = remember_weak.update(cx, |this, cx| {
                                    this.settings.remember_window_position = *checked;
                                    this.persist();
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("appBehavior.keepInBackgroundTooltip").to_string()),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("settings.appearance").to_string()),
                    )
                    .child({
                        let theme_weak = weak.clone();
                        let current = current_theme;
                        div().flex_row_dir(cx).flex_wrap().gap_1().children(
                            ThemePreference::ALL.map(|preference| {
                                let theme_weak = theme_weak.clone();
                                Button::new(format!("settings-{}", preference.id()))
                                    .label(theme_label(preference))
                                    .ghost()
                                    .compact()
                                    .selected(current == preference)
                                    .on_click(move |_, window, cx| {
                                        let _ = theme_weak.update(cx, |this, cx| {
                                            this.set_theme(preference, window, cx);
                                        });
                                        window.close_sheet(cx);
                                    })
                            }),
                        )
                    })
                    .child(
                        div()
                            .mt_2()
                            .flex_row_dir(cx)
                            .items_center()
                            .gap_1()
                            .child({
                                let language_weak = weak.clone();
                                Button::new("settings-language")
                                    .label(format!(
                                        "{}: {}",
                                        t!("header.languageLabel"),
                                        i18n::label(&current_locale)
                                    ))
                                    .outline()
                                    .compact()
                                    .on_click(move |_, window, cx| {
                                        window.close_sheet(cx);
                                        let _ = language_weak.update(cx, |this, cx| {
                                            this.open_language_dialog(window, cx)
                                        });
                                    })
                            })
                            .child({
                                let about_weak = weak.clone();
                                Button::new("settings-about")
                                    .label(t!("header.aboutTitle").to_string())
                                    .ghost()
                                    .compact()
                                    .on_click(move |_, window, cx| {
                                        window.close_sheet(cx);
                                        let _ = about_weak.update(cx, |this, cx| {
                                            this.open_about_dialog(window, cx)
                                        });
                                    })
                            })
                            .child({
                                let update_weak = weak.clone();
                                Button::new("settings-check-update")
                                    .label(t!("header.checkAppUpdate").to_string())
                                    .ghost()
                                    .compact()
                                    .on_click(move |_, window, cx| {
                                        window.close_sheet(cx);
                                        let _ = update_weak
                                            .update(cx, |this, cx| this.check_update(cx));
                                    })
                            }),
                    ),
            )
        });
    }

    fn open_about_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let version = env!("CARGO_PKG_VERSION");
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title(t!("header.aboutTitle").to_string())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_sm().child(format!("v{version}")))
                        .child(
                            div()
                                .text_sm()
                                .child(t!("header.aboutDescription").to_string()),
                        )
                        .child(
                            div()
                                .flex_row_dir(cx)
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new("about-repository")
                                        .label(t!("header.aboutRepository").to_string())
                                        .on_click(|_, _, cx| {
                                            cx.open_url(
                                                "https://github.com/yanhenrique-dev/OpenScrcpy",
                                            )
                                        }),
                                )
                                .child(
                                    Button::new("about-releases")
                                        .label(t!("header.aboutReleases").to_string())
                                        .on_click(|_, _, cx| {
                                            cx.open_url(
                                                "https://github.com/yanhenrique-dev/OpenScrcpy/releases/latest",
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .child(t!("header.aboutLicense").to_string()),
                        ),
                )
        });
    }

    /// Selects a capture mode by its index in [`CaptureMode::ALL`].
    pub fn select_mode(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(mode) = CaptureMode::ALL.get(index) {
            self.switch_mode(*mode, cx);
        }
    }

    fn switch_mode(&mut self, mode: CaptureMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    fn select_device(&mut self, serial: String, cx: &mut Context<Self>) {
        if self.active_device.as_deref() != Some(serial.as_str()) {
            self.active_device = Some(serial);
            self.auto_load_cameras(cx);
            cx.notify();
        }
    }

    /// Loads the camera list for the active device from cache, or asks scrcpy
    /// once when the device changes. The manual refresh button is only a retry
    /// path for failures.
    fn auto_load_cameras(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            self.cameras.clear();
            self.cameras_for = None;
            return;
        };
        if self.cameras_for.as_deref() == Some(device.as_str()) {
            return;
        }
        if self.cameras_busy {
            // A request for the previous device may still be running; make its
            // generation stale and let the new one take over.
            self.cameras_busy = false;
            self.cameras_generation = self.cameras_generation.wrapping_add(1);
        }
        if let Some(cached) = self.camera_cache.get(&device).cloned() {
            self.cameras = cached;
            self.cameras_for = Some(device);
            self.cameras_error = false;
            cx.notify();
            return;
        }
        self.refresh_lenses(cx);
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.logs.push(line.into());
        if self.logs.len() > MAX_LOG_LINES {
            let overflow = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(..overflow);
        }
        self.console_scroll.scroll_to_bottom();
    }

    fn push_error(&mut self, error: impl Into<String>) {
        self.push_log(format!("[ERROR] {}", error.into()));
    }

    fn apply_event(&mut self, event: AppEvent, cx: &mut Context<Self>) {
        match event {
            AppEvent::Log(line) => self.push_log(line),
            AppEvent::Status(payload) => {
                match payload.get("type").and_then(Value::as_str) {
                    Some("downloading") => self.downloading = true,
                    Some("download-progress") => {
                        self.downloading = true;
                        self.download_percent = payload.get("percent").and_then(Value::as_u64);
                    }
                    Some("download-complete") => {
                        self.downloading = false;
                        self.download_percent = Some(100);
                    }
                    _ => {}
                }
                if let Some(running) = payload.get("running").and_then(Value::as_bool) {
                    self.running = running;
                    self.starting = false;
                    if !running {
                        self.mic_route = None;
                        self.mic_db = None;
                        self.notify_unexpected_end(cx, &payload);
                    }
                }
            }
            AppEvent::AudioStatus(_) => {}
            AppEvent::WindowPos(payload) => {
                if !self.settings.remember_window_position {
                    return;
                }
                if let (Some(x), Some(y)) = (
                    payload.get("x").and_then(Value::as_i64),
                    payload.get("y").and_then(Value::as_i64),
                ) {
                    let x = x as i32;
                    let y = y as i32;
                    if self.settings.mirror.window_x != Some(x)
                        || self.settings.mirror.window_y != Some(y)
                    {
                        self.settings.mirror.window_x = Some(x);
                        self.settings.mirror.window_y = Some(y);
                        self.persist();
                    }
                }
            }
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
        self.request_android_versions(cx);
        self.auto_load_cameras(cx);
        cx.notify();
    }

    fn request_android_versions(&mut self, cx: &mut Context<Self>) {
        let serials: Vec<String> = self
            .devices
            .iter()
            .filter(|device| device.state == DeviceState::Online)
            .map(|device| device.serial.clone())
            .filter(|serial| {
                !self.android_versions.contains_key(serial)
                    && !self.android_version_requests.contains(serial)
            })
            .collect();
        for serial in serials {
            self.android_version_requests.insert(serial.clone());
            let services = self.services.clone();
            cx.spawn(async move |this, cx| {
                let version = services
                    .android_version(serial.clone())
                    .await
                    .ok()
                    .flatten();
                let _ = this.update(cx, |shell, cx| {
                    shell.android_version_requests.remove(&serial);
                    if let Some(version) = version {
                        shell.android_versions.insert(serial.clone(), version);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
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

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.pending_update.clone() else {
            return;
        };
        if self.installing_update {
            return;
        }
        self.installing_update = true;
        self.push_log(format!(
            "[SYSTEM] Downloading OpenScrcpy {}...",
            update.version
        ));
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.download_update(update).await;
            let _ = this.update(cx, |shell, cx| {
                shell.installing_update = false;
                match result {
                    Ok(Ok(path)) => shell.finish_update(path, cx),
                    Ok(Err(error)) => shell.push_error(error),
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_update(&mut self, download: PathBuf, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if let Ok(target) = std::env::var("APPIMAGE") {
            let target = PathBuf::from(target);
            if target.exists() {
                self.push_log(format!(
                    "[SYSTEM] Replacing {} and restarting...",
                    target.display()
                ));
                match spawn_appimage_replacement(&target, &download) {
                    Ok(()) => {
                        cx.quit();
                        return;
                    }
                    Err(error) => self.push_error(error),
                }
            }
        }
        self.push_log(format!(
            "[SYSTEM] Update downloaded to {}; install it manually (AppImage self-replace is Linux/AppImage only).",
            download.display()
        ));
    }

    fn start_download(&mut self, cx: &mut Context<Self>) {
        if self.downloading {
            return;
        }
        self.downloading = true;
        self.download_error = false;
        self.download_percent = None;
        self.push_log("[SYSTEM] Downloading the scrcpy core binaries...".to_string());
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.download_scrcpy().await;
            let _ = this.update(cx, |shell, cx| {
                shell.downloading = false;
                match result {
                    Ok(Ok(())) => {
                        shell.download_error = false;
                        shell.scrcpy_ready = Some(true);
                        shell.push_log("[SYSTEM] Core binaries installed.".to_string());
                    }
                    Ok(Err(error)) => {
                        shell.download_error = true;
                        shell.push_error(error);
                    }
                    Err(error) => {
                        shell.download_error = true;
                        shell.push_error(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.settings.onboarding_done = true;
        self.persist();
        cx.notify();
    }

    fn onboarding_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let percent = self.download_percent.unwrap_or(0);
        let progress = t!("onboarding.downloadingProgress", progress = percent).to_string();
        let ready = self.scrcpy_ready == Some(true);

        div()
            .id("onboarding-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_4()
            .p_8()
            .child(
                div()
                    .id("onboarding-title")
                    .role(Role::Heading)
                    .aria_label(t!("onboarding.coreInitialization").to_string())
                    .text_2xl()
                    .child(t!("onboarding.coreInitialization").to_string())
                    .test_support(),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("onboarding.introQuote").to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .child(t!("onboarding.binariesDescription").to_string()),
            )
            .when(self.downloading, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .max_w_96()
                        .child(div().text_sm().child(progress.clone()))
                        .child(
                            Progress::new("core-download-progress")
                                .value(percent as f32)
                                .accessibility_label(progress),
                        ),
                )
            })
            .when(self.download_error, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(t!("onboarding.downloadFailed").to_string()),
                )
            })
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("onboarding-download")
                            .label(t!("onboarding.downloadCoreBinaries").to_string())
                            .primary()
                            .disabled(self.downloading)
                            .on_click(cx.listener(|this, _, _, cx| this.start_download(cx))),
                    )
                    .when(ready, |this| {
                        this.child(
                            Button::new("onboarding-continue")
                                .label(t!("onboarding.continueToApp").to_string())
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.finish_onboarding(cx))),
                        )
                    })
                    .child(
                        Button::new("onboarding-releases")
                            .label(t!("onboarding.githubReleases").to_string())
                            .ghost()
                            .on_click(|_, _, cx| {
                                cx.open_url(
                                    "https://github.com/yanhenrique-dev/OpenScrcpy/releases",
                                )
                            }),
                    ),
            )
    }

    fn check_update(&mut self, cx: &mut Context<Self>) {
        if self.checking_update {
            return;
        }
        self.checking_update = true;
        self.push_log("[SYSTEM] Checking for a native update...".to_string());
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.check_app_update().await;
            let _ = this.update(cx, |shell, cx| {
                shell.checking_update = false;
                match result {
                    Ok(Ok(Some(update))) => {
                        shell.push_log(format!(
                            "[SYSTEM] Version {} is available: {}",
                            update.version, update.url
                        ));
                        shell.update_available = Some(update.version.clone());
                        shell.pending_update = Some(update);
                    }
                    Ok(Ok(None)) => {
                        shell.update_available = None;
                        shell.pending_update = None;
                        shell.push_log(
                            "[SYSTEM] No update available (the release channel may not be published yet)."
                                .to_string(),
                        );
                    }
                    Ok(Err(error)) => shell.push_error(error),
                    Err(error) => shell.push_error(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
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
            self.push_log("[WARN] No device selected for push/install.");
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
        if self.wireless_busy {
            return;
        }
        let typed = self.pairing_address.read(cx).value().trim().to_string();
        let Some(address) = resolve_pairing_address(&self.discovered, &typed) else {
            self.push_log(
                "[WARN] No pairing service found; enable Wireless debugging and enter IP:Port.",
            );
            cx.notify();
            return;
        };
        let code = self.pairing_code.read(cx).value().trim().to_string();
        if code.is_empty() {
            return;
        }

        self.wireless_busy = true;
        self.pairing_attempt = Some(1);
        self.push_log(format!("[SYSTEM] Pairing with {address}..."));
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let mut last_error = String::new();
            for attempt in 1..=PAIRING_MAX_ATTEMPTS {
                if attempt > 1 {
                    let delay = pairing_retry_delay(attempt);
                    cx.background_executor().timer(delay).await;
                }
                let result = services.pair(address.clone(), code.clone()).await;
                let success = match result {
                    Ok(Ok(value)) => {
                        let paired = value.get("success").and_then(Value::as_bool) == Some(true);
                        if !paired {
                            last_error = value
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("pairing failed")
                                .to_string();
                        }
                        paired
                    }
                    Ok(Err(error)) => {
                        last_error = error;
                        false
                    }
                    Err(error) => {
                        last_error = error.to_string();
                        false
                    }
                };
                if success {
                    let _ = this.update(cx, |shell, cx| {
                        shell.wireless_busy = false;
                        shell.pairing_attempt = None;
                        shell.push_log(format!("[SYSTEM] Paired with {address}."));
                        cx.notify();
                    });
                    return;
                }
                if attempt < PAIRING_MAX_ATTEMPTS {
                    let _ = this.update(cx, |shell, cx| {
                        shell.pairing_attempt = Some(attempt + 1);
                        cx.notify();
                    });
                }
            }
            let _ = this.update(cx, |shell, cx| {
                shell.wireless_busy = false;
                shell.pairing_attempt = None;
                shell.push_error(format!("Pairing with {address} failed: {last_error}"));
                cx.notify();
            });
        })
        .detach();
    }

    fn start_label(&self) -> String {
        match self.mode {
            CaptureMode::Desktop => t!("controlPanel.ejectToDesktop").to_string(),
            CaptureMode::Microphone => t!("controlPanel.startMicBridge").to_string(),
            CaptureMode::Camera => t!("controlPanel.engageCamera").to_string(),
            _ => t!("controlPanel.startMission").to_string(),
        }
    }

    fn start_session(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            return;
        };
        if self.starting || self.running {
            return;
        }
        self.starting = true;
        let session_label = match self.mode {
            CaptureMode::Desktop => "desktop",
            CaptureMode::Camera => "camera",
            CaptureMode::Microphone => "microphone",
            CaptureMode::Screen => "mirror",
        };
        self.push_log(format!(
            "[SYSTEM] Starting {session_label} session for {device}..."
        ));
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

    fn confirm_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(device) = self.active_device.clone() else {
            return;
        };
        if !self.running {
            self.stop_session_now(cx);
            return;
        }

        let weak = cx.weak_entity();
        let title = t!("controlPanel.stopSession").to_string();
        let description = t!("controlPanel.stopConfirm", device = device.as_str()).to_string();
        let cancel = t!("common.cancel").to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .show_cancel(true)
                .ok_text(title.clone())
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(cancel.clone())
                .on_ok(move |_, _, cx| {
                    let _ = weak.update(cx, |shell, cx| shell.stop_session_now(cx));
                    true
                })
        });
    }

    fn stop_session_now(&mut self, cx: &mut Context<Self>) {
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
        let mut config = self.mode_config(device);
        if !self.settings.remember_window_position {
            config.window_x = None;
            config.window_y = None;
        }
        config
    }

    fn mode_config(&self, device: &str) -> open_scrcpy_core::config::ScrcpyConfig {
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

    fn panel_heading(&self, cx: &App) -> impl IntoElement {
        div()
            .id("panel-title")
            .role(Role::Heading)
            .aria_label(self.mode.label().to_string())
            .text_xl()
            .text_start(cx)
            .child(self.mode.label())
            .test_support()
    }

    fn session_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_start = self.active_device.is_some() && !self.starting && !self.running;
        let can_stop = self.running;
        let start_label = self.start_label();

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
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("start-mission")
                            .label(start_label)
                            .primary()
                            .disabled(!can_start)
                            .on_click(cx.listener(|this, _, _, cx| this.start_session(cx))),
                    )
                    .child(
                        Button::new("stop-session")
                            .label(t!("controlPanel.stopSession").to_string())
                            .danger()
                            .disabled(!can_stop)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_stop(window, cx)),
                            ),
                    ),
            )
    }

    fn titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let version = env!("CARGO_PKG_VERSION");
        let services = self.services.clone();
        TitleBar::new()
            .on_close_window(move |_, window, _| {
                services.teardown();
                window.remove_window();
            })
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_2()
                    .pl_2()
                    .flex_1()
                    .child(div().id("app-title").text_sm().child("OpenScrcpy"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("v{version}")),
                    )
                    .child(
                        div()
                            .flex_row_dir(cx)
                            .items_center()
                            .gap_1()
                            .child(div().w_1p5().h_1p5().rounded_full().bg(if self.running {
                                cx.theme().success
                            } else if self.starting {
                                cx.theme().warning
                            } else if self.scrcpy_ready == Some(true) {
                                cx.theme().success
                            } else {
                                cx.theme().danger
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.running {
                                        t!("status.running").to_string()
                                    } else if self.starting {
                                        t!("status.starting").to_string()
                                    } else if self.scrcpy_ready == Some(true) {
                                        t!("header.scrcpyReady").to_string()
                                    } else {
                                        t!("header.scrcpyMissing").to_string()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.active_device.clone().unwrap_or_default()),
                    ),
            )
    }

    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale_label = format!(
            "{}: {}",
            t!("header.languageLabel"),
            i18n::label(&self.settings.locale)
        );
        div()
            .flex_row_dir(cx)
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().flex_1())
            .child(
                div().w_40().child(
                    Select::new(&self.theme_select)
                        .small()
                        .accessibility_label(t!("settings.appearance").to_string()),
                ),
            )
            .when_some(self.update_available.clone(), |this, version| {
                this.child(
                    Button::new("install-update")
                        .icon(IconName::Download)
                        .label(t!("header.updateBadge", v = version).to_string())
                        .primary()
                        .disabled(self.installing_update)
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                )
            })
            .child(
                Button::new("about")
                    .icon(IconName::Info)
                    .label(t!("header.aboutTitle").to_string())
                    .ghost()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_about_dialog(window, cx)),
                    ),
            )
            .child(
                Button::new("app-settings")
                    .icon(IconName::Settings)
                    .label(t!("appBehavior.title").to_string())
                    .ghost()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_settings_sheet(window, cx)),
                    ),
            )
            .child(
                Button::new("check-update")
                    .icon(IconName::RefreshCw)
                    .label(t!("header.checkAppUpdate").to_string())
                    .ghost()
                    .disabled(self.checking_update)
                    .on_click(cx.listener(|this, _, _, cx| this.check_update(cx))),
            )
            .child(
                Button::new("locale-toggle").label(locale_label).on_click(
                    cx.listener(|this, _, window, cx| this.open_language_dialog(window, cx)),
                ),
            )
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = Vec::with_capacity(self.devices.len());
        for device in &self.devices {
            let serial = device.serial.clone();
            let name = device.display_name();
            let version = self.android_versions.get(&device.serial).cloned();
            let wireless = device.serial.contains(':') || device.serial.contains('.');
            let (state_label, state_color) = match device.state {
                DeviceState::Online => (t!("sidebar.ready").to_string(), cx.theme().success),
                DeviceState::Unauthorized => (
                    t!("sidebar.deviceUnauthorized").to_string(),
                    cx.theme().warning,
                ),
                DeviceState::Offline => (
                    t!("sidebar.deviceOffline").to_string(),
                    cx.theme().muted_foreground,
                ),
            };
            let transport = if wireless {
                t!("sidebar.wifi").to_string()
            } else {
                t!("sidebar.usb").to_string()
            };
            rows.push(
                Button::new(format!("device-{}", device.serial))
                    .selected(self.active_device.as_deref() == Some(device.serial.as_str()))
                    .accessibility_label(device.label())
                    .w_full()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_device(serial.clone(), cx)),
                    )
                    .child(
                        div()
                            .flex_row_dir(cx)
                            .items_center()
                            .gap_2()
                            .w_full()
                            .child(Avatar::new().name(name.clone()).small())
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap_0p5()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().truncate().child(name))
                                    .child(
                                        div()
                                            .flex_row_dir(cx)
                                            .items_center()
                                            .flex_wrap()
                                            .gap_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .w_1p5()
                                                    .h_1p5()
                                                    .rounded_full()
                                                    .bg(state_color),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(state_label),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!("· {transport}")),
                                            )
                                            .when_some(version, |this, version| {
                                                this.child(
                                                    Tag::new()
                                                        .small()
                                                        .child(format!("Android {version}")),
                                                )
                                            }),
                                    ),
                            ),
                    ),
            );
        }

        let busy = self.wireless_busy;

        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .gap_2()
            .w_72()
            .flex_shrink_0()
            .p_3()
            .border_e(px(1.), cx)
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
                    .outline()
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
                            .primary()
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
                            .outline()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.pair_wireless(cx))),
                    )
                    .when_some(self.pairing_attempt, |this, attempt| {
                        this.child(
                            div()
                                .id("pair-status")
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} ({attempt}/{PAIRING_MAX_ATTEMPTS})",
                                    t!("sidebar.pairModalWaiting")
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .id("shortcuts-title")
                            .aria_label(t!("shortcuts.title").to_string())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("shortcuts.title").to_string())
                            .test_support(),
                    )
                    .children(["F", "H", "B", "S", "P", "R", "V", "O"].map(|key| {
                        div()
                            .flex_row_dir(cx)
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Alt+{key}")),
                            )
                            .child(div().text_xs().child(shortcut_label(key)))
                    })),
            )
    }

    fn mode_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = CaptureMode::ALL
            .iter()
            .position(|mode| *mode == self.mode)
            .unwrap_or(0);
        div()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("mode-tabs")
                    .segmented()
                    .selected_index(active)
                    .on_click(cx.listener(|this, index, _, cx| {
                        if let Some(mode) = CaptureMode::ALL.get(*index) {
                            this.switch_mode(*mode, cx);
                        }
                    }))
                    .children(
                        CaptureMode::ALL
                            .map(|mode| Tab::new().icon(mode.icon()).label(mode.label())),
                    ),
            )
    }

    fn mirror_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.mirror.clone();
        let bitrate = self.bitrate.read(cx).value().end().round() as u32;

        let audio_sources = [
            "output",
            "playback",
            "mic",
            "voice-call",
            "voice-performance",
        ];
        let audio_source_index = audio_sources
            .iter()
            .position(|source| *source == settings.mirror_audio_source)
            .unwrap_or(0);
        let audio_source_options = audio_sources.map(|source| {
            let label = match source {
                "playback" => t!("sessionBehavior.audioSourcePlayback"),
                "mic" => t!("sessionBehavior.audioSourceMic"),
                "voice-call" => t!("sessionBehavior.audioSourceVoiceCall"),
                "voice-performance" => t!("sessionBehavior.audioSourceVoicePerformance"),
                _ => t!("sessionBehavior.audioSourceOutput"),
            };
            Radio::new(format!("mirror-audio-source-{source}")).label(label.to_string())
        });
        let audio_on = settings.audio;
        let dup_enabled = audio_on && settings.mirror_audio_source == "playback";

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.fps").to_string()),
                    )
                    .child(
                        Select::new(&self.fps_select)
                            .small()
                            .w_24()
                            .menu_width(px(150.))
                            .accessibility_label(t!("controlPanel.fps").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.resolution").to_string()),
                    )
                    .child(
                        Select::new(&self.size_select)
                            .small()
                            .w_24()
                            .menu_width(px(150.))
                            .accessibility_label(t!("controlPanel.resolution").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.codec").to_string()),
                    )
                    .child(
                        Select::new(&self.codec_select)
                            .small()
                            .w_24()
                            .menu_width(px(150.))
                            .accessibility_label(t!("controlPanel.codec").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("sessionBehavior.audioSource").to_string()),
                    )
                    .child(
                        RadioGroup::horizontal("mirror-audio-source")
                            .selected_index(Some(audio_source_index))
                            .disabled(!audio_on)
                            .on_change(cx.listener(|this, index, _, cx| {
                                let sources = [
                                    "output",
                                    "playback",
                                    "mic",
                                    "voice-call",
                                    "voice-performance",
                                ];
                                let source = sources.get(*index).copied().unwrap_or("output");
                                this.settings.mirror.mirror_audio_source = source.to_string();
                                if source != "playback" {
                                    this.settings.mirror.audio_dup = false;
                                }
                                this.persist();
                                cx.notify();
                            }))
                            .children(audio_source_options),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .flex_wrap()
                    .gap_6()
                    .child(
                        Switch::new("mirror-audio-dup")
                            .checked(settings.audio_dup)
                            .disabled(!dup_enabled)
                            .label(t!("sessionBehavior.audioDup").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.audio_dup = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("mirror-require-audio")
                            .checked(settings.require_audio)
                            .disabled(!audio_on)
                            .label(t!("sessionBehavior.requireAudio").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.mirror.require_audio = *checked;
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.graphicsRenderer").to_string()),
                    )
                    .child(
                        Select::new(&self.renderer_select)
                            .small()
                            .w_32()
                            .menu_width(px(190.))
                            .accessibility_label(t!("controlPanel.graphicsRenderer").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.rotation").to_string()),
                    )
                    .child(
                        Select::new(&self.rotation_select)
                            .small()
                            .w_24()
                            .menu_width(px(120.))
                            .accessibility_label(t!("controlPanel.rotation").to_string()),
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.width").to_string()),
                    )
                    .child(
                        div().w_64().child(
                            Slider::new(&self.desktop_width)
                                .horizontal()
                                .accessibility_label(t!("controlPanel.width").to_string()),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} px", self.settings.desktop.width)),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.height").to_string()),
                    )
                    .child(
                        div().w_64().child(
                            Slider::new(&self.desktop_height)
                                .horizontal()
                                .accessibility_label(t!("controlPanel.height").to_string()),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} px", self.settings.desktop.height)),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
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
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} DPI", self.settings.desktop.dpi)),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.uiScaling").to_string()),
                    )
                    .child(
                        div().w_64().child(
                            Slider::new(&self.desktop_dpi)
                                .horizontal()
                                .accessibility_label(t!("controlPanel.uiScaling").to_string()),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
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
            .when(
                self.capabilities
                    .as_ref()
                    .is_none_or(|caps| caps.supports("flex-display")),
                |this| {
                    this.child(
                        Switch::new("desktop-flex")
                            .checked(desktop.flex_display)
                            .label(t!("controlPanel.flexDisplay").to_string())
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.settings.desktop.flex_display = *checked;
                                this.persist();
                                cx.notify();
                            })),
                    )
                },
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
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
            .child(self.panel_heading(cx))
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
        self.cameras_error = false;
        self.cameras_generation = self.cameras_generation.wrapping_add(1);
        let generation = self.cameras_generation;
        self.push_log(format!(
            "[SYSTEM] Running scrcpy --list-cameras for {device}..."
        ));
        cx.notify();

        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let result = services.list_cameras(device.clone()).await;
            let _ = this.update(cx, |shell, cx| {
                if shell.cameras_generation != generation
                    || shell.active_device.as_deref() != Some(device.as_str())
                {
                    return;
                }
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
                        shell.cameras = cameras.clone();
                        shell.camera_cache.insert(device.clone(), cameras);
                        shell.cameras_for = Some(device);
                        if !shell
                            .cameras
                            .iter()
                            .any(|camera| camera.id == shell.settings.camera.camera_id)
                        {
                            shell.settings.camera.camera_id.clear();
                            shell.persist();
                        }
                    }
                    Err(error) => {
                        shell.cameras_error = true;
                        shell.push_error(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn camera_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let camera = self.settings.camera.clone();
        let zoom = self.camera_zoom.read(cx).value().end().clamp(1.0, 10.0);
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
                    .flex_row_dir(cx)
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
                    .when(self.cameras_busy, |this| this.child(Spinner::new()))
                    .when(
                        !self.cameras_busy
                            && self.active_device.is_some()
                            && (self.cameras_error || self.cameras.is_empty()),
                        |this| {
                            this.child(
                                Button::new("camera-lenses-retry")
                                    .label(refresh_label)
                                    .ghost()
                                    .compact()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_lenses(cx)),
                                    ),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.facing").to_string()),
                    )
                    .child(
                        Select::new(&self.camera_facing_select)
                            .small()
                            .w_24()
                            .menu_width(px(140.))
                            .accessibility_label(t!("controlPanel.facing").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.aspect").to_string()),
                    )
                    .child(
                        Select::new(&self.camera_aspect_select)
                            .small()
                            .w_24()
                            .menu_width(px(140.))
                            .accessibility_label(t!("controlPanel.aspect").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
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
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .child(t!("controlPanel.resolution").to_string()),
                    )
                    .child(
                        Select::new(&self.camera_max_size_select)
                            .small()
                            .w_24()
                            .menu_width(px(140.))
                            .accessibility_label(t!("controlPanel.resolution").to_string()),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
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
            .child(self.panel_heading(cx))
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
            .child(self.panel_heading(cx))
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
            .child(self.panel_heading(cx))
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

    fn copy_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.logs.is_empty() {
            return;
        }
        let text = self.logs.join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let message = t!("logPanel.copied").to_string();
        window.push_notification(Notification::success(message.clone()), cx);
        self.push_log(message);
        cx.notify();
    }

    fn save_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let message = t!("logPanel.reportSavedAlert", fileName = path).to_string();
                window.push_notification(Notification::success(message.clone()), cx);
                self.push_log(t!("logPanel.diagnosticReportSaved", fileName = path).to_string());
            }
            Err(error) => self.push_error(error),
        }
        cx.notify();
    }

    fn console(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut lines = Vec::with_capacity(self.logs.len());
        for line in &self.logs {
            let visible = match self.log_filter {
                LogFilter::All => true,
                LogFilter::Errors => log_level(line) == LogLevel::Error,
                LogFilter::System => line.starts_with("[SYSTEM]") || line.contains("[server]"),
            };
            if !visible {
                continue;
            }
            let color = match log_level(line) {
                LogLevel::Error => cx.theme().danger,
                LogLevel::Warning => cx.theme().warning,
                LogLevel::Info => cx.theme().muted_foreground,
            };
            lines.push(
                div()
                    .text_xs()
                    .text_color(color)
                    .when(line.starts_with("[SYSTEM] Starting "), |this| {
                        this.mt_1()
                            .pt_1()
                            .border_t_1()
                            .border_color(cx.theme().border)
                    })
                    .child(line.clone()),
            );
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_row_dir(cx)
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
                    .child(
                        div()
                            .flex_row_dir(cx)
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("log-filter-all")
                                    .label(t!("logPanel.allLogs").to_string())
                                    .ghost()
                                    .compact()
                                    .selected(self.log_filter == LogFilter::All)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.log_filter = LogFilter::All;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("log-filter-system")
                                    .label(t!("logPanel.systemLogs").to_string())
                                    .ghost()
                                    .compact()
                                    .selected(self.log_filter == LogFilter::System)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.log_filter = LogFilter::System;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("log-filter-errors")
                                    .label(t!("logPanel.errorLogs").to_string())
                                    .ghost()
                                    .compact()
                                    .selected(self.log_filter == LogFilter::Errors)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.log_filter = LogFilter::Errors;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("console-clear")
                            .label(t!("logPanel.clear").to_string())
                            .ghost()
                            .compact()
                            .disabled(self.logs.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.clear_console(cx))),
                    )
                    .child(
                        Button::new("console-copy")
                            .label(t!("logPanel.copy").to_string())
                            .ghost()
                            .compact()
                            .disabled(self.logs.is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.copy_console(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("console-report")
                            .label(t!("logPanel.report").to_string())
                            .ghost()
                            .compact()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_report(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_row_dir(cx)
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pb_2()
                    .child(Input::new(&self.terminal_input).flex_1())
                    .child(
                        Button::new("terminal-run")
                            .label(t!("logPanel.run").to_string())
                            .primary()
                            .compact()
                            .disabled(self.terminal_busy)
                            .on_click(cx.listener(|this, _, _, cx| this.run_terminal_command(cx))),
                    ),
            )
            .child(
                div()
                    .id("console-body")
                    .role(Role::Log)
                    .aria_label(t!("logPanel.systemConsole").to_string())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.console_scroll)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .flex_row_dir(cx)
            .size_full()
            .min_h_0()
            .child(self.sidebar(cx))
            .when(
                needs_onboarding(self.scrcpy_ready, self.settings.onboarding_done),
                |this| this.child(self.onboarding_panel(cx).into_any_element()),
            )
            .when(
                !needs_onboarding(self.scrcpy_ready, self.settings.onboarding_done),
                |this| {
                    this.child(
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
                    )
                },
            );

        let console_max = px((window.viewport_size().height * 0.5).as_f32().max(160.));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.handle_drop(paths, cx)))
            .child(self.titlebar(cx))
            .child(self.header(cx))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("workspace-console")
                        .child(resizable_panel().child(body))
                        .child(
                            resizable_panel()
                                .size(px(180.))
                                .size_range(px(120.)..console_max)
                                .child(self.console(cx)),
                        ),
                ),
            )
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

fn spawn_capability_load(cx: &mut Context<AppShell>, services: AppServices) {
    cx.spawn(async move |this, cx| {
        let capabilities = services.detect_capabilities().await.ok();
        let _ = this.update(cx, |shell, cx| {
            let flex_supported = capabilities
                .as_ref()
                .is_none_or(|caps| caps.supports("flex-display"));
            shell.capabilities = capabilities;
            if !flex_supported && shell.settings.desktop.flex_display {
                shell.settings.desktop.flex_display = false;
                shell.persist();
            }
            cx.notify();
        });
    })
    .detach();
}

fn spawn_core_check(cx: &mut Context<AppShell>, services: AppServices) {
    cx.spawn(async move |this, cx| {
        let value = services.check_scrcpy().await;
        let ready = value
            .ok()
            .and_then(|value| value.get("found").and_then(Value::as_bool));
        let _ = this.update(cx, |shell, cx| {
            shell.scrcpy_ready = ready;
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

const PAIRING_MAX_ATTEMPTS: u32 = 20;

/// Address to pair with: what the user typed, or the first pairing service
/// reported by mDNS. Pairing ports change, so the discovery list is the source
/// of truth when the field is empty.
fn resolve_pairing_address(discovered: &[DiscoveredService], typed: &str) -> Option<String> {
    let typed = typed.trim();
    if !typed.is_empty() {
        return Some(typed.to_string());
    }
    discovered
        .iter()
        .find(|service| service.service.contains("_adb-tls-pairing"))
        .map(|service| service.address.clone())
}

fn pairing_retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(u64::from(attempt.clamp(1, 5)))
}

/// Waits for this process to exit, then replaces the AppImage and relaunches.
/// Arguments are passed positionally so no path is interpolated into the
/// script text.
#[cfg(target_os = "linux")]
fn spawn_appimage_replacement(
    target: &std::path::Path,
    download: &std::path::Path,
) -> Result<(), String> {
    const SCRIPT: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; mv -f "$2" "$3" && chmod +x "$3" && exec "$3""#;
    std::process::Command::new("sh")
        .arg("-c")
        .arg(SCRIPT)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(download)
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn needs_onboarding(ready: Option<bool>, done: bool) -> bool {
    ready == Some(false) && !done
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogLevel {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogFilter {
    All,
    System,
    Errors,
}

/// The core logs carry a `[LEVEL]` prefix; color those, keep the rest muted.
fn log_level(line: &str) -> LogLevel {
    if line.contains("[ERROR]") {
        LogLevel::Error
    } else if line.contains("[WARN]") || line.contains("[WARNING]") {
        LogLevel::Warning
    } else {
        LogLevel::Info
    }
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

impl AppShell {
    /// Native notification for a session that ended without the user asking.
    fn notify_unexpected_end(&self, cx: &mut Context<Self>, payload: &Value) {
        if !self.settings.notifications_enabled {
            return;
        }
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
    fn pairing_uses_the_typed_address_or_the_mdns_service() {
        let discovered = vec![
            DiscoveredService {
                name: "phone".into(),
                service: "_adb-tls-connect._tcp".into(),
                address: "10.0.0.2:5555".into(),
            },
            DiscoveredService {
                name: "phone".into(),
                service: "_adb-tls-pairing._tcp".into(),
                address: "10.0.0.2:37123".into(),
            },
        ];
        assert_eq!(
            resolve_pairing_address(&discovered, "  ").as_deref(),
            Some("10.0.0.2:37123")
        );
        assert_eq!(
            resolve_pairing_address(&discovered, "192.168.0.9:40000").as_deref(),
            Some("192.168.0.9:40000")
        );
        assert!(resolve_pairing_address(&[], "").is_none());
    }

    #[test]
    fn pairing_retry_delay_grows_then_caps() {
        assert_eq!(pairing_retry_delay(1), Duration::from_secs(1));
        assert_eq!(pairing_retry_delay(4), Duration::from_secs(4));
        assert_eq!(pairing_retry_delay(20), Duration::from_secs(5));
    }

    #[test]
    fn log_levels_come_from_the_prefix() {
        assert_eq!(log_level("[ERROR] boom"), LogLevel::Error);
        assert_eq!(log_level("[WARN] careful"), LogLevel::Warning);
        assert_eq!(log_level("[SYSTEM] hello"), LogLevel::Info);
    }

    #[test]
    fn onboarding_only_when_the_core_is_missing() {
        assert!(needs_onboarding(Some(false), false));
        assert!(!needs_onboarding(Some(false), true));
        assert!(!needs_onboarding(Some(true), false));
        assert!(!needs_onboarding(None, false));
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
