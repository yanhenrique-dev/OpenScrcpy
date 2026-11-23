//! Persisted application settings.
//!
//! The file is user-writable, so everything read here is untrusted: unknown
//! locales, out-of-range window sizes and schema changes are normalized back to
//! safe defaults instead of being trusted by the shell. Writes are atomic
//! (temp file + rename) so a crash cannot leave a truncated JSON behind.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use gpui_kit::{App, Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

/// Current on-disk schema. Bump it when the shape changes and add the
/// migration to [`AppSettings::normalized`] before shipping the new version.
pub const SCHEMA_VERSION: u32 = 1;

pub const DEFAULT_WIDTH: f32 = 1200.0;
pub const DEFAULT_HEIGHT: f32 = 890.0;
pub const MIN_WIDTH: f32 = 960.0;
pub const MIN_HEIGHT: f32 = 620.0;
const MAX_SIZE: f32 = 16384.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemePreference {
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::System];

    /// Stable identity for the header control and its tests.
    pub fn id(self) -> &'static str {
        match self {
            Self::Light => "theme-light",
            Self::Dark => "theme-dark",
            Self::System => "theme-system",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    /// Top-left in screen coordinates, when the window was moved.
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            x: None,
            y: None,
            maximized: false,
        }
    }
}

impl WindowState {
    fn normalized(mut self) -> Self {
        if !self.width.is_finite() || !(MIN_WIDTH..=MAX_SIZE).contains(&self.width) {
            self.width = DEFAULT_WIDTH;
        }
        if !self.height.is_finite() || !(MIN_HEIGHT..=MAX_SIZE).contains(&self.height) {
            self.height = DEFAULT_HEIGHT;
        }
        let valid_x = self.x.filter(|value| value.is_finite());
        let valid_y = self.y.filter(|value| value.is_finite());
        // A half-restored position would land the window somewhere arbitrary.
        if valid_x.is_none() || valid_y.is_none() {
            self.x = None;
            self.y = None;
        } else {
            self.x = valid_x;
            self.y = valid_y;
        }
        self
    }

    fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(self.x.unwrap_or(0.0)), px(self.y.unwrap_or(0.0))),
            size: size(px(self.width), px(self.height)),
        }
    }
}

/// Mirror-session defaults the shell persists. Device serials are not stored
/// here: the active device is runtime state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MirrorSettings {
    /// Video bitrate in Mbps.
    pub bitrate: u32,
    pub fps: u32,
    /// Longest side in pixels; 0 keeps the device resolution.
    pub max_size: u32,
    /// `h264`, `h265` or `av1`.
    pub video_codec: String,
    pub audio: bool,
    pub vsync: bool,
    pub stay_awake: bool,
    pub turn_screen_off: bool,
    pub auto_reconnect: bool,
    pub hid_keyboard: bool,
    pub hid_mouse: bool,
    /// Pure HID/OTG mode: no mirror, the phone is driven as an input device.
    pub otg_pure: bool,
    pub always_on_top: bool,
    pub borderless: bool,
    pub fullscreen: bool,
    /// Record the session to the user's videos directory.
    pub record: bool,
    /// 0, 90, 180 or 270.
    pub rotation: u32,
    /// `auto` or one of the driver ids advertised by the scrcpy binary.
    pub renderer: String,
}

impl Default for MirrorSettings {
    fn default() -> Self {
        Self {
            bitrate: 8,
            fps: 60,
            max_size: 0,
            video_codec: "h264".to_string(),
            audio: true,
            vsync: true,
            stay_awake: false,
            turn_screen_off: false,
            auto_reconnect: true,
            hid_keyboard: false,
            hid_mouse: false,
            otg_pure: false,
            always_on_top: false,
            borderless: false,
            fullscreen: false,
            record: false,
            rotation: 0,
            renderer: "auto".to_string(),
        }
    }
}

impl MirrorSettings {
    pub(crate) const FPS: [u32; 3] = [30, 60, 90];
    pub(crate) const SIZES: [u32; 4] = [0, 1920, 1280, 720];
    pub(crate) const CODECS: [&'static str; 3] = ["h264", "h265", "av1"];

    pub fn normalized(mut self) -> Self {
        self.bitrate = self.bitrate.clamp(1, 50);
        if !Self::FPS.contains(&self.fps) {
            self.fps = 60;
        }
        if !Self::SIZES.contains(&self.max_size) {
            self.max_size = 0;
        }
        if !Self::CODECS.contains(&self.video_codec.as_str()) {
            self.video_codec = "h264".to_string();
        }
        if !DesktopSettings::ROTATIONS.contains(&self.rotation) {
            self.rotation = 0;
        }
        self.renderer = self.renderer.trim().to_string();
        if self.renderer.is_empty() {
            self.renderer = "auto".to_string();
        }
        self
    }

    /// Fields every windowed mode shares: performance, audio, VSync and
    /// session behavior.
    fn apply_shared(&self, config: &mut open_scrcpy_core::config::ScrcpyConfig) {
        config.bitrate = Some(self.bitrate);
        config.fps = Some(self.fps);
        config.codec = Some(self.video_codec.clone());
        config.audio_enabled = Some(self.audio);
        config.vsync = Some(self.vsync);
        config.stay_awake = Some(self.stay_awake);
        config.turn_off = Some(self.turn_screen_off);
        config.auto_reconnect = Some(self.auto_reconnect);
        config.hid_keyboard = Some(self.hid_keyboard);
        config.hid_mouse = Some(self.hid_mouse);
        config.otg_pure = Some(self.otg_pure);
        config.always_on_top = Some(self.always_on_top);
        config.borderless = Some(self.borderless);
        config.fullscreen = Some(self.fullscreen);
        config.record = Some(self.record);
        config.rotation = (self.rotation != 0).then(|| self.rotation.to_string());
        config.render_driver = (self.renderer != "auto").then(|| self.renderer.clone());
    }

    /// Builds the core DTO for one device. The shell owns presentation
    /// defaults; the core still validates every field before spawning.
    pub fn to_config(&self, device: &str) -> open_scrcpy_core::config::ScrcpyConfig {
        let mut config = open_scrcpy_core::config::ScrcpyConfig::mirror(device);
        self.apply_shared(&mut config);
        config.res = Some(if self.max_size == 0 {
            "0".to_string()
        } else {
            self.max_size.to_string()
        });
        config
    }
}

/// Camera-mode defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    /// `back`, `front` or `external`.
    pub facing: String,
    /// `0` (scrcpy default), `1:1`, `4:3` or `16:9`.
    pub aspect: String,
    pub torch: bool,
    /// 1.0 is no zoom; scrcpy supports 1.0..=10.0.
    pub zoom: f32,
    pub high_speed: bool,
    /// Route the phone microphone into the virtual sink.
    pub mic_sink: bool,
    /// Max video size; 0 uses scrcpy's 1920x1080 default.
    pub max_size: u32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            facing: "back".to_string(),
            aspect: "0".to_string(),
            torch: false,
            zoom: 1.0,
            high_speed: false,
            mic_sink: false,
            max_size: 0,
        }
    }
}

impl CameraSettings {
    pub(crate) const FACINGS: [&'static str; 3] = ["back", "front", "external"];
    pub(crate) const ASPECTS: [&'static str; 4] = ["0", "1:1", "4:3", "16:9"];

    pub fn normalized(mut self) -> Self {
        if !Self::FACINGS.contains(&self.facing.as_str()) {
            self.facing = "back".to_string();
        }
        if !Self::ASPECTS.contains(&self.aspect.as_str()) {
            self.aspect = "0".to_string();
        }
        if !self.zoom.is_finite() {
            self.zoom = 1.0;
        }
        self.zoom = self.zoom.clamp(1.0, 10.0);
        if !MirrorSettings::SIZES.contains(&self.max_size) {
            self.max_size = 0;
        }
        self
    }

    pub fn to_config(
        &self,
        device: &str,
        shared: &MirrorSettings,
    ) -> open_scrcpy_core::config::ScrcpyConfig {
        let mut config = open_scrcpy_core::config::ScrcpyConfig::camera(device);
        shared.apply_shared(&mut config);
        config.camera_facing = Some(self.facing.clone());
        config.camera_ar = (self.aspect != "0").then(|| self.aspect.clone());
        config.camera_torch = Some(self.torch);
        config.camera_zoom = Some(self.zoom);
        config.camera_high_speed = Some(self.high_speed);
        config.camera_mic_sink = Some(self.mic_sink);
        config.res = Some(if self.max_size == 0 {
            "0".to_string()
        } else {
            self.max_size.to_string()
        });
        config
    }
}

/// Microphone-bridge defaults./// Microphone-bridge defaults. The bridge is audio-only and Linux-only for the
/// virtual sink; the other fields are still validated on every platform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MicSettings {
    /// `mic` or `mic-voice-communication`.
    pub source: String,
    /// `opus` or `aac`.
    pub codec: String,
    /// kbps, 32..=256.
    pub bitrate: u32,
    /// PipeWire RNNoise sink on Linux; experimental.
    pub denoise: bool,
    /// Make the virtual monitor the system default input (Linux).
    pub set_default_source: bool,
}

impl Default for MicSettings {
    fn default() -> Self {
        Self {
            source: "mic".to_string(),
            codec: "opus".to_string(),
            bitrate: 128,
            denoise: false,
            set_default_source: false,
        }
    }
}

impl MicSettings {
    pub(crate) const SOURCES: [&'static str; 2] = ["mic", "mic-voice-communication"];
    pub(crate) const CODECS: [&'static str; 2] = ["opus", "aac"];
    pub(crate) const BITRATES: [u32; 5] = [64, 96, 128, 192, 256];

    pub fn normalized(mut self) -> Self {
        // Older builds persisted the pre-rename value.
        if self.source == "voice-communication" {
            self.source = "mic-voice-communication".to_string();
        }
        if !Self::SOURCES.contains(&self.source.as_str()) {
            self.source = "mic".to_string();
        }
        if !Self::CODECS.contains(&self.codec.as_str()) {
            self.codec = "opus".to_string();
        }
        self.bitrate = self.bitrate.clamp(32, 256);
        self
    }

    pub fn to_config(
        &self,
        device: &str,
        shared: &MirrorSettings,
    ) -> open_scrcpy_core::config::ScrcpyConfig {
        let mut config = open_scrcpy_core::config::ScrcpyConfig::mic(device);
        config.mic_source = Some(self.source.clone());
        config.mic_codec = Some(self.codec.clone());
        config.mic_bitrate = Some(self.bitrate);
        config.mic_denoise = Some(self.denoise);
        config.mic_set_default_source = Some(self.set_default_source);
        config.auto_reconnect = Some(shared.auto_reconnect);
        config
    }
}

/// Virtual-display defaults for Desktop mode./// Virtual-display defaults for Desktop mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub width: u32,
    pub height: u32,
    /// Virtual display DPI.
    pub dpi: u32,
    /// 0, 90, 180 or 270.
    pub rotation: u32,
    pub flex_display: bool,
    /// Max video size; 0 keeps the virtual display resolution.
    pub max_size: u32,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            dpi: 420,
            rotation: 0,
            flex_display: false,
            max_size: 0,
        }
    }
}

impl DesktopSettings {
    pub(crate) const SIZES: [(u32, u32); 4] =
        [(1920, 1080), (2560, 1440), (3840, 2160), (2560, 1080)];
    pub(crate) const DPI: [u32; 5] = [160, 240, 320, 420, 480];
    pub(crate) const ROTATIONS: [u32; 4] = [0, 90, 180, 270];

    pub fn normalized(mut self) -> Self {
        self.width = self.width.clamp(480, 3840);
        self.height = self.height.clamp(320, 2160);
        self.dpi = self.dpi.clamp(120, 640);
        if !Self::ROTATIONS.contains(&self.rotation) {
            self.rotation = 0;
        }
        if !MirrorSettings::SIZES.contains(&self.max_size) {
            self.max_size = 0;
        }
        self
    }

    pub fn to_config(
        &self,
        device: &str,
        shared: &MirrorSettings,
    ) -> open_scrcpy_core::config::ScrcpyConfig {
        let mut config = open_scrcpy_core::config::ScrcpyConfig::desktop(device);
        shared.apply_shared(&mut config);
        config.vd_width = Some(self.width);
        config.vd_height = Some(self.height);
        config.vd_dpi = Some(self.dpi);
        config.rotation = (self.rotation != 0).then(|| self.rotation.to_string());
        config.flex_display = Some(self.flex_display);
        config.res = Some(if self.max_size == 0 {
            "0".to_string()
        } else {
            self.max_size.to_string()
        });
        config
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub version: u32,
    /// Concrete locale code. Empty means "follow the OS on next start".
    pub locale: String,
    pub theme: ThemePreference,
    pub window: WindowState,
    pub mirror: MirrorSettings,
    pub desktop: DesktopSettings,
    pub mic: MicSettings,
    pub camera: CameraSettings,
    /// Last wireless addresses that connected, newest first (max 5).
    pub recent_wireless: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            locale: String::new(),
            theme: ThemePreference::System,
            window: WindowState::default(),
            mirror: MirrorSettings::default(),
            desktop: DesktopSettings::default(),
            mic: MicSettings::default(),
            camera: CameraSettings::default(),
            recent_wireless: Vec::new(),
        }
    }
}

impl AppSettings {
    /// Rejects values a corrupted or hand-edited file could contain.
    pub fn normalized(mut self) -> Self {
        if self.version != SCHEMA_VERSION {
            // There is no older schema yet. When one exists, migrate it here
            // instead of dropping the user's settings wholesale.
            let locale = self.locale;
            self = Self::default();
            self.locale = locale;
        }
        if !crate::i18n::is_supported(&self.locale) {
            self.locale.clear();
        }
        self.window = self.window.normalized();
        self.mirror = self.mirror.normalized();
        self.desktop = self.desktop.normalized();
        self.mic = self.mic.normalized();
        self.camera = self.camera.normalized();
        self.recent_wireless = normalize_recent(self.recent_wireless);
        self
    }

    /// Restores the saved position when it exists, otherwise centers a default
    /// window on the current display.
    pub fn window_bounds(&self, cx: &App) -> WindowBounds {
        let bounds = self.window.bounds();
        if self.window.maximized {
            WindowBounds::Maximized(bounds)
        } else if self.window.x.is_some() {
            WindowBounds::Windowed(bounds)
        } else {
            WindowBounds::centered(bounds.size, cx)
        }
    }
}

/// Resolves and reads/writes the settings file. A missing or unreadable file is
/// not an error: the app starts with defaults.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: Option<PathBuf>,
}

impl SettingsStore {
    pub fn new() -> Self {
        let path = ProjectDirs::from("dev", "yanhenrique", "OpenScrcpy")
            .map(|dirs| dirs.config_dir().join("settings.json"));
        Self { path }
    }

    /// Store with no on-disk path: used by the UI tests and by embedded
    /// launches where writing the user's config dir is not allowed.
    pub fn without_persistence() -> Self {
        Self { path: None }
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn load(&self) -> AppSettings {
        let Some(path) = &self.path else {
            return AppSettings::default();
        };
        let Ok(text) = fs::read_to_string(path) else {
            return AppSettings::default();
        };
        match serde_json::from_str::<AppSettings>(&text) {
            Ok(settings) => settings.normalized(),
            Err(error) => {
                eprintln!("[settings] ignoring unreadable settings: {error}");
                AppSettings::default()
            }
        }
    }

    pub fn save(&self, settings: &AppSettings) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        write_atomically(path, &json)
    }

    /// Updates only the window geometry, keeping whatever else was saved.
    pub fn save_window(&self, bounds: Bounds<Pixels>, maximized: bool) {
        let mut settings = self.load();
        settings.window.width = f32::from(bounds.size.width);
        settings.window.height = f32::from(bounds.size.height);
        settings.window.x = Some(f32::from(bounds.origin.x));
        settings.window.y = Some(f32::from(bounds.origin.y));
        settings.window.maximized = maximized;
        if let Err(error) = self.save(&settings) {
            eprintln!("[settings] could not save the window state: {error}");
        }
    }
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_recent(addresses: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for address in addresses {
        let address = address.trim().to_string();
        if address.is_empty() || seen.contains(&address) {
            continue;
        }
        seen.push(address);
        if seen.len() == 5 {
            break;
        }
    }
    seen
}

fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(contents.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    // `rename` replaces the target on Unix but not on Windows.
    if fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(path);
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store(name: &str) -> (SettingsStore, PathBuf) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("openscrcpy-settings-{name}-{stamp}"));
        let path = dir.join("settings.json");
        (SettingsStore::at(path.clone()), dir)
    }

    #[test]
    fn missing_file_yields_defaults() {
        let (store, dir) = temp_store("missing");
        assert_eq!(store.load(), AppSettings::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn roundtrip_preserves_values() {
        let (store, dir) = temp_store("roundtrip");
        let settings = AppSettings {
            locale: "pt-BR".into(),
            theme: ThemePreference::Dark,
            window: WindowState {
                width: 1280.0,
                height: 720.0,
                x: Some(40.0),
                y: Some(30.0),
                maximized: false,
            },
            ..AppSettings::default()
        };
        store.save(&settings).expect("save");
        let loaded = store.load();
        assert_eq!(loaded, settings);
        assert!(store.path().expect("path").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let (store, dir) = temp_store("corrupt");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("settings.json"), "{ not json").expect("write");
        assert_eq!(store.load(), AppSettings::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_locale_and_bad_sizes_are_normalized() {
        let settings = AppSettings {
            locale: "xx-YY".into(),
            window: WindowState {
                width: f32::NAN,
                height: 10.0,
                x: Some(-100.0),
                y: None,
                maximized: false,
            },
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(settings.locale, "");
        assert_eq!(settings.window.width, DEFAULT_WIDTH);
        assert_eq!(settings.window.height, DEFAULT_HEIGHT);
        assert_eq!(settings.window.x, None);
        assert_eq!(settings.window.y, None);
    }

    #[test]
    fn mirror_baseline_maps_to_the_core_dto() {
        let settings = MirrorSettings {
            bitrate: 16,
            fps: 90,
            max_size: 1920,
            video_codec: "h265".into(),
            audio: false,
            vsync: false,
            stay_awake: true,
            turn_screen_off: true,
            auto_reconnect: false,
            ..MirrorSettings::default()
        };
        let config = settings.to_config("device1");
        assert_eq!(config.device, "device1");
        assert_eq!(config.bitrate, Some(16));
        assert_eq!(config.fps, Some(90));
        assert_eq!(config.res.as_deref(), Some("1920"));
        assert_eq!(config.codec.as_deref(), Some("h265"));
        assert_eq!(config.audio_enabled, Some(false));
        assert_eq!(config.vsync, Some(false));
        assert_eq!(config.stay_awake, Some(true));
        assert_eq!(config.turn_off, Some(true));
        assert_eq!(config.auto_reconnect, Some(false));
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--video-bit-rate".to_string()));
        assert!(args.contains(&"16M".to_string()));
        assert!(args.contains(&"--video-codec=h265".to_string()));
        assert!(args.contains(&"--max-size".to_string()));
        assert!(args.contains(&"1920".to_string()));
        assert!(!args.contains(&"--audio".to_string()));
    }

    #[test]
    fn mirror_defaults_keep_the_device_resolution() {
        let config = MirrorSettings::default().to_config("device1");
        assert_eq!(config.res.as_deref(), Some("0"));
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(!args.contains(&"--max-size".to_string()));
    }

    #[test]
    fn desktop_baseline_maps_to_the_virtual_display_arguments() {
        let desktop = DesktopSettings {
            width: 2560,
            height: 1440,
            dpi: 320,
            rotation: 90,
            flex_display: true,
            max_size: 1920,
        };
        let config = desktop.to_config("device1", &MirrorSettings::default());
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--new-display=2560x1440/320".to_string()));
        assert!(args.contains(&"--orientation".to_string()));
        assert!(args.contains(&"90".to_string()));
        assert!(args.contains(&"--flex-display".to_string()));
        assert!(args.contains(&"--max-size".to_string()));
        assert!(args.contains(&"1920".to_string()));
    }

    #[test]
    fn mic_baseline_maps_to_the_bridge_arguments() {
        let mic = MicSettings {
            source: "mic-voice-communication".into(),
            codec: "aac".into(),
            bitrate: 192,
            denoise: true,
            set_default_source: true,
        };
        let config = mic.to_config("device1", &MirrorSettings::default());
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--audio-source=mic-voice-communication".to_string()));
        assert!(args.contains(&"--audio-codec=aac".to_string()));
        assert!(args.contains(&"--audio-bit-rate=192k".to_string()));
        assert_eq!(config.mic_denoise, Some(true));
        assert_eq!(config.mic_set_default_source, Some(true));
    }

    #[test]
    fn camera_settings_map_to_the_camera_arguments() {
        let camera = CameraSettings {
            facing: "front".into(),
            aspect: "16:9".into(),
            torch: true,
            zoom: 2.0,
            high_speed: true,
            mic_sink: true,
            max_size: 1920,
        };
        let config = camera.to_config("device1", &MirrorSettings::default());
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--camera-facing=front".to_string()));
        assert!(args.contains(&"--camera-ar=16:9".to_string()));
        assert!(args.contains(&"--camera-torch".to_string()));
        assert!(args.contains(&"--camera-zoom=2.0".to_string()));
        assert!(args.contains(&"--camera-high-speed".to_string()));
        assert!(args.contains(&"--camera-size=1920x1080".to_string()));
        assert_eq!(config.camera_mic_sink, Some(true));
    }

    #[test]
    fn invalid_camera_settings_are_normalized() {
        let camera = CameraSettings {
            facing: "sideways".into(),
            aspect: "21:9".into(),
            torch: false,
            zoom: f32::NAN,
            high_speed: false,
            mic_sink: false,
            max_size: 1234,
        }
        .normalized();
        assert_eq!(camera.facing, "back");
        assert_eq!(camera.aspect, "0");
        assert_eq!(camera.zoom, 1.0);
        assert_eq!(camera.max_size, 0);
    }

    #[test]
    fn invalid_mic_settings_are_normalized() {
        let mic = MicSettings {
            source: "voice-communication".into(),
            codec: "flac".into(),
            bitrate: 999,
            denoise: false,
            set_default_source: false,
        }
        .normalized();
        assert_eq!(mic.source, "mic-voice-communication");
        assert_eq!(mic.codec, "opus");
        assert_eq!(mic.bitrate, 256);

        let mic = MicSettings {
            source: "speaker".into(),
            ..MicSettings::default()
        }
        .normalized();
        assert_eq!(mic.source, "mic");
    }

    #[test]
    fn invalid_desktop_settings_are_normalized() {
        let desktop = DesktopSettings {
            width: 100,
            height: 9000,
            dpi: 40,
            rotation: 45,
            flex_display: true,
            max_size: 1234,
        }
        .normalized();
        assert_eq!(desktop.width, 480);
        assert_eq!(desktop.height, 2160);
        assert_eq!(desktop.dpi, 120);
        assert_eq!(desktop.rotation, 0);
        assert_eq!(desktop.max_size, 0);
        assert!(desktop.flex_display);
    }

    #[test]
    fn pure_hid_maps_to_the_hid_flags() {
        let mirror = MirrorSettings {
            hid_keyboard: true,
            hid_mouse: true,
            otg_pure: true,
            ..MirrorSettings::default()
        };
        // Over TCP, pure HID disables video/audio and drives uhid directly.
        let config = mirror.to_config("10.0.0.2:5555");
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--no-video".to_string()));
        assert!(args.contains(&"--keyboard=uhid".to_string()));
        assert!(args.contains(&"--mouse=uhid".to_string()));
    }

    #[test]
    fn window_extras_map_to_the_core_arguments() {
        let mirror = MirrorSettings {
            always_on_top: true,
            borderless: true,
            record: true,
            rotation: 90,
            ..MirrorSettings::default()
        };
        let config = mirror.to_config("device1");
        let args = open_scrcpy_core::config::build_scrcpy_args(
            &config,
            Some("/tmp/videos".to_string()),
            None,
        );
        assert!(args.contains(&"--always-on-top".to_string()));
        assert!(args.contains(&"--window-borderless".to_string()));
        assert!(args.iter().any(|arg| arg.starts_with("--record=")));
        assert!(args.contains(&"--orientation".to_string()));
        assert!(args.contains(&"90".to_string()));
    }

    #[test]
    fn renderer_maps_to_the_driver_flag() {
        let mirror = MirrorSettings {
            renderer: "opengl".to_string(),
            ..MirrorSettings::default()
        };
        let config = mirror.to_config("device1");
        let args = open_scrcpy_core::config::build_scrcpy_args(&config, None, None);
        assert!(args.contains(&"--render-driver".to_string()));
        assert!(args.contains(&"opengl".to_string()));

        let auto = MirrorSettings {
            renderer: "  ".to_string(),
            ..MirrorSettings::default()
        }
        .normalized();
        assert_eq!(auto.renderer, "auto");
        let config = auto.to_config("device1");
        assert_eq!(config.render_driver, None);
    }

    #[test]
    fn invalid_rotation_falls_back_to_zero() {
        let mirror = MirrorSettings {
            rotation: 45,
            ..MirrorSettings::default()
        }
        .normalized();
        assert_eq!(mirror.rotation, 0);
    }

    #[test]
    fn invalid_mirror_settings_are_normalized() {
        let settings = MirrorSettings {
            bitrate: 500,
            fps: 17,
            max_size: 1234,
            video_codec: "vp9".into(),
            ..MirrorSettings::default()
        }
        .normalized();
        assert_eq!(settings.bitrate, 50);
        assert_eq!(settings.fps, 60);
        assert_eq!(settings.max_size, 0);
        assert_eq!(settings.video_codec, "h264");
    }

    #[test]
    fn recent_wireless_is_trimmed_deduped_and_capped() {
        let recent = normalize_recent(vec![
            " 10.0.0.2:5555 ".into(),
            "10.0.0.2:5555".into(),
            "".into(),
            "10.0.0.3:5555".into(),
            "10.0.0.4:5555".into(),
            "10.0.0.5:5555".into(),
            "10.0.0.6:5555".into(),
            "10.0.0.7:5555".into(),
        ]);
        assert_eq!(recent.len(), 5);
        assert_eq!(recent[0], "10.0.0.2:5555");
        assert_eq!(recent[4], "10.0.0.6:5555");
    }

    #[test]
    fn older_schema_resets_to_defaults_but_keeps_the_locale() {
        let settings = AppSettings {
            version: 0,
            locale: "fr".into(),
            theme: ThemePreference::Light,
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(settings.version, SCHEMA_VERSION);
        assert_eq!(settings.locale, "fr");
        assert_eq!(settings.theme, ThemePreference::System);
    }
}
