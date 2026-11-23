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
        self
    }

    /// Builds the core DTO for one device. The shell owns presentation
    /// defaults; the core still validates every field before spawning.
    pub fn to_config(&self, device: &str) -> open_scrcpy_core::config::ScrcpyConfig {
        let mut config = open_scrcpy_core::config::ScrcpyConfig::mirror(device);
        config.bitrate = Some(self.bitrate);
        config.fps = Some(self.fps);
        config.res = Some(if self.max_size == 0 {
            "0".to_string()
        } else {
            self.max_size.to_string()
        });
        config.codec = Some(self.video_codec.clone());
        config.audio_enabled = Some(self.audio);
        config.vsync = Some(self.vsync);
        config.stay_awake = Some(self.stay_awake);
        config.turn_off = Some(self.turn_screen_off);
        config.auto_reconnect = Some(self.auto_reconnect);
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            locale: String::new(),
            theme: ThemePreference::System,
            window: WindowState::default(),
            mirror: MirrorSettings::default(),
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
