//! Scrcpy configuration DTO, validation and command-line construction.
//!
//! The config arrives from the webview with camelCase field names and is
//! treated as data: every value that reaches the command line is validated
//! here (or in `validate`) before `build_scrcpy_args` runs. Public fields are
//! deliberate for this wire DTO; behavior stays behind functions.

use serde::Deserialize;

use crate::validate;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScrcpyConfig {
    pub device: String,
    pub session_mode: String,
    // ... other fields matching JS ...
    pub bitrate: Option<u32>,
    pub fps: Option<u32>,
    pub stay_awake: Option<bool>,
    pub turn_off: Option<bool>,
    pub audio_enabled: Option<bool>,
    pub audio_codec: Option<String>,
    /// Mirror mode: which device audio scrcpy captures. `output` is the
    /// default and mutes the phone; `playback` captures app media audio.
    pub mirror_audio_source: Option<String>,
    /// Mirror + playback only: keep the audio playing on the device.
    pub audio_dup: Option<bool>,
    /// Make scrcpy fail (instead of silently mirroring video only) when the
    /// device cannot capture audio. Off by default so older devices keep
    /// working; the UI surfaces the failure either way.
    pub require_audio: Option<bool>,
    pub always_on_top: Option<bool>,
    pub fullscreen: Option<bool>,
    pub borderless: Option<bool>,
    pub record: Option<bool>,
    pub record_path: Option<String>,
    pub scrcpy_path: Option<String>,
    pub otg_pure: Option<bool>,
    pub camera_facing: Option<String>,
    pub camera_id: Option<String>,
    pub codec: Option<String>,
    pub camera_ar: Option<String>,
    pub camera_high_speed: Option<bool>,
    pub vd_width: Option<u32>,
    pub vd_height: Option<u32>,
    pub vd_dpi: Option<u32>,
    pub rotation: Option<String>,
    pub res: Option<String>,
    pub hid_keyboard: Option<bool>,
    pub hid_mouse: Option<bool>,
    pub render_driver: Option<String>,
    // v4 features
    pub flex_display: Option<bool>,
    pub camera_torch: Option<bool>,
    pub camera_zoom: Option<f32>,
    /// Camera mode: route the phone microphone into the OpenScrcpy Mic virtual
    /// sink so OBS/Discord can select it as a recording device.
    pub camera_mic_sink: Option<bool>,
    pub background_color: Option<String>,
    pub keep_active: Option<bool>,
    /// Retry a session that ends unexpectedly (USB/ADB drop or crash) with
    /// exponential backoff. Deliberate stops are never retried.
    pub auto_reconnect: Option<bool>,
    /// Force SDL renderer VSync on/off (anti-tearing). scrcpy 4.0 (SDL3)
    /// disables renderer VSync by default; this re-asserts it via the SDL hint.
    pub vsync: Option<bool>,
    /// Last known screen position of the scrcpy mirror window. Captured (client
    /// area top-left) while a windowed session runs and restored via
    /// --window-x / --window-y on the next launch, so the window reopens where
    /// the user left it, including when relaunching borderless.
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    // v4.1 features
    /// Ignore video encoder size constraints entirely (scrcpy v4.1+).
    /// Useful when the device reports incorrect encoder limits that prevent
    /// scrcpy from starting.
    pub ignore_video_encoder_constraints: Option<bool>,
    // Mic bridge (audio-only capture source)
    /// Audio source: "mic" (raw) or "mic-voice-communication" (Android-side
    /// noise suppression + AGC + echo cancellation), for Android 12+.
    /// The legacy "voice-communication" value is still accepted and mapped.
    pub mic_source: Option<String>,
    /// Lossy codec for the bridge: "opus" (default) or "aac".
    pub mic_codec: Option<String>,
    /// Bitrate in kbps for the lossy mic codecs (64..256).
    pub mic_bitrate: Option<u32>,
    /// Experimental: create the virtual sink through a PipeWire RNNoise
    /// filter-chain (PC-side noise suppression). Best effort with fallback.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub mic_denoise: Option<bool>,
    /// Linux: make the virtual monitor source the system default input while
    /// the bridge runs, restoring the previous default when it stops.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub mic_set_default_source: Option<bool>,
}
fn resolve_audio_codec_flag<'a>(
    config: &'a ScrcpyConfig,
    audio_codec_override: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(c) = audio_codec_override {
        let trimmed = c.trim();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    config.audio_codec.as_deref().and_then(|c| {
        let trimmed = c.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(trimmed)
        }
    })
}

/// Sanitize an arbitrary identifier (e.g. a device id from user-typed IPs or
/// ADB output) for use inside a file name (used for the `--record` filename).
pub fn sanitize_filename_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "device".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn build_scrcpy_args(
    config: &ScrcpyConfig,
    video_dir_fallback: Option<String>,
    audio_codec_override: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();

    // Construct arguments based on config
    if !config.device.is_empty() {
        args.push("-s".to_string());
        args.push(config.device.clone());
    }

    let codec = config.codec.as_deref().unwrap_or("h264");
    args.push(format!("--video-codec={}", codec));

    let otg_pure = config.otg_pure.unwrap_or(false);
    let hid_keyboard = config.hid_keyboard.unwrap_or(false);
    let hid_mouse = config.hid_mouse.unwrap_or(false);

    if config.session_mode == "mirror" && (hid_keyboard || hid_mouse) && otg_pure {
        if config.device.contains('.') || config.device.contains(':') {
            args.push("--no-video".to_string());
            args.push("--no-audio".to_string());
            args.push("--keyboard=uhid".to_string());
            args.push("--mouse=uhid".to_string());
        } else {
            args.push("--otg".to_string());
        }
    } else {
        if config.session_mode == "mic" {
            // Audio-only bridge: the phone's mic reaches the PC as a
            // recording device, with no video pipeline and no window. Every
            // video-specific knob below is irrelevant here, so this branch
            // builds a minimal set and returns early.
            args.push("--no-window".to_string());
            args.push("--no-video".to_string());
            let source = config.mic_source.as_deref().unwrap_or("").trim();
            let source = if source.is_empty() { "mic" } else { source };
            // Older builds persisted "voice-communication"; scrcpy expects
            // "mic-voice-communication". Normalize here so a saved config
            // keeps working after the rename.
            let source = validate::normalize_mic_source(source);
            args.push(format!("--audio-source={}", source));
            // The bridge has its own codec choice; the automatic repair
            // chain's override (when present) still wins so recovery can try
            // alternatives after a codec failure.
            let codec = audio_codec_override
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    config
                        .mic_codec
                        .as_deref()
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "opus".to_string());
            args.push(format!("--audio-codec={}", codec));
            if codec == "opus" || codec == "aac" {
                let kbps = config.mic_bitrate.unwrap_or(128).clamp(32, 256);
                args.push(format!("--audio-bit-rate={}k", kbps));
            }
            return args;
        }
        if hid_keyboard {
            args.push("--keyboard=uhid".to_string());
        }
        if hid_mouse {
            args.push("--mouse=uhid".to_string());
        }

        if let Some(render_driver) = &config.render_driver {
            let selected_driver = render_driver.trim();
            if !selected_driver.is_empty() && selected_driver != "auto" {
                args.push("--render-driver".to_string());
                args.push(selected_driver.to_string());
            }
        }

        if let Some(bitrate) = config.bitrate {
            args.push("--video-bit-rate".to_string());
            args.push(format!("{}M", bitrate));
        }

        let audio_enabled = config.audio_enabled.unwrap_or(true);
        if !audio_enabled {
            args.push("--no-audio".to_string());
        }
        if audio_enabled {
            // A webcam needs the phone's microphone; the default `output`
            // source would capture media audio and mute the device.
            if config.session_mode == "camera" {
                args.push("--audio-source=mic".to_string());
            }
            if config.session_mode == "mirror" {
                let source = config
                    .mirror_audio_source
                    .as_deref()
                    .unwrap_or("output")
                    .trim();
                if source != "output" && validate::MIRROR_AUDIO_SOURCES.contains(&source) {
                    args.push(format!("--audio-source={}", source));
                }
                // scrcpy only accepts --audio-dup together with
                // --audio-source=playback.
                if source == "playback" && config.audio_dup.unwrap_or(false) {
                    args.push("--audio-dup".to_string());
                }
            }
            if let Some(codec) = resolve_audio_codec_flag(config, audio_codec_override) {
                args.push(format!("--audio-codec={}", codec));
            }
            // By default scrcpy degrades to video-only when audio capture
            // fails. The opt-in makes that failure visible (scrcpy exits),
            // which this module then reports in the console.
            if config.require_audio.unwrap_or(false) {
                args.push("--require-audio".to_string());
            }
        }
        if let Some(aot) = config.always_on_top {
            if aot {
                args.push("--always-on-top".to_string());
            }
        }
        if let Some(fs) = config.fullscreen {
            if fs {
                args.push("--fullscreen".to_string());
            }
        }
        if let Some(bl) = config.borderless {
            if bl {
                args.push("--window-borderless".to_string());
            }
        }

        // Restore the last saved window position (client-area origin), for
        // mirror sessions only. Skipped in fullscreen (scrcpy ignores it) and
        // for camera/desktop, whose windows must not share, or overwrite, the
        // mirror window's remembered position.
        if config.session_mode == "mirror" && !config.fullscreen.unwrap_or(false) {
            if let Some(wx) = config.window_x {
                args.push(format!("--window-x={}", wx));
            }
            if let Some(wy) = config.window_y {
                args.push(format!("--window-y={}", wy));
            }
        }

        if let Some(rot) = &config.rotation {
            if rot != "0" {
                args.push("--orientation".to_string());
                args.push(rot.clone());
            }
        }

        let can_control = config.session_mode != "camera";
        if can_control {
            if let Some(sa) = config.stay_awake {
                if sa {
                    args.push("--stay-awake".to_string());
                }
            }
            if let Some(ka) = config.keep_active {
                if ka {
                    args.push("--keep-active".to_string());
                }
            }
            if let Some(to) = config.turn_off {
                if to {
                    // Keep --no-power-on off as a fallback. The startup path
                    // also wakes the display over ADB before launching (see
                    // `wake_display_before_screen_off`) to avoid the async
                    // power-on/power-off race, but scrcpy must still be able
                    // to wake a phone whose ADB wake failed.
                    args.push("--turn-screen-off".to_string());
                }
            }
        }

        if config.session_mode == "camera" {
            args.push("--video-source=camera".to_string());
            if let Some(cid) = &config.camera_id {
                if !cid.is_empty() {
                    args.push(format!("--camera-id={}", cid));
                } else if let Some(facing) = &config.camera_facing {
                    args.push(format!("--camera-facing={}", facing));
                }
            } else if let Some(facing) = &config.camera_facing {
                args.push(format!("--camera-facing={}", facing));
            }

            // Resolution logic: Map res (e.g., "1920") to standard camera sizes (e.g., "1920x1080")
            if let Some(res) = &config.res {
                if res != "0" {
                    let camera_size = match res.as_str() {
                        "3840" => "3840x2160",
                        "2560" => "2560x1440",
                        "1920" => "1920x1080",
                        "1600" => "1600x900",
                        "1280" => "1280x720",
                        "1024" => "1024x576",
                        "800" => "800x480",
                        _ => "1920x1080",
                    };
                    args.push(format!("--camera-size={}", camera_size));
                } else {
                    // Safe fallback default to avoid camera configuration error on non-standard sensor photo sizes (e.g. 4080x3060)
                    args.push("--camera-size=1920x1080".to_string());
                }
            } else {
                args.push("--camera-size=1920x1080".to_string());
            }

            if let Some(ar) = &config.camera_ar {
                if ar != "0" {
                    args.push(format!("--camera-ar={}", ar));
                }
            }
            if let Some(chs) = config.camera_high_speed {
                if chs {
                    args.push("--camera-high-speed".to_string());
                }
            }
            // v4: camera torch and zoom
            if let Some(true) = config.camera_torch {
                args.push("--camera-torch".to_string());
            }
            if let Some(zoom) = config.camera_zoom {
                if zoom > 1.005 {
                    // avoid floating point noise at 1.0
                    args.push(format!("--camera-zoom={:.1}", zoom));
                }
            }
            // fps handled generically below
        } else if config.session_mode == "desktop" {
            let w = config.vd_width.unwrap_or(1920);
            let h = config.vd_height.unwrap_or(1080);
            let dpi = config.vd_dpi.unwrap_or(420);
            args.push(format!("--new-display={}x{}/{}", w, h, dpi));
            args.push("--video-buffer=100".to_string());
            // v4: flex display (resize virtual display with window)
            if let Some(true) = config.flex_display {
                args.push("--flex-display".to_string());
            }
        }

        if let Some(fps) = config.fps {
            if fps > 0 {
                if config.session_mode == "camera" {
                    args.push("--camera-fps".to_string());
                } else {
                    args.push("--max-fps".to_string());
                }
                args.push(fps.to_string());
            }
        } else if config.session_mode == "camera" && config.camera_high_speed.unwrap_or(false) {
            args.push("--camera-fps".to_string());
            args.push("60".to_string());
        }

        // Shared resolution logic (applies to mirror and desktop)
        if config.session_mode != "camera" {
            if let Some(res) = &config.res {
                if res != "0" {
                    args.push("--max-size".to_string());
                    args.push(res.clone());
                }
            }
        }

        if let Some(rec) = config.record {
            if rec {
                let mut path = config.record_path.clone().unwrap_or_default();

                // If path is empty, try to get Video dir fallback
                if path.trim().is_empty() {
                    path = video_dir_fallback.unwrap_or_else(|| ".".to_string());
                }

                // The device id can come from user-typed IPs or ADB output,
                // so sanitize it before using it inside a file name.
                let safe_device = sanitize_filename_component(&config.device);
                let filename = format!(
                    "scrcpy_{}_{}.mkv",
                    safe_device,
                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                );
                let full_path = std::path::Path::new(&path).join(filename);
                args.push(format!("--record={}", full_path.to_string_lossy()));
            }
        }

        // v4: background color (all modes)
        if let Some(ref color) = config.background_color {
            let trimmed = color.trim();
            if !trimmed.is_empty() {
                args.push(format!("--background-color={}", trimmed));
            }
        }

        // v4.1: ignore video encoder size constraints
        if let Some(true) = config.ignore_video_encoder_constraints {
            args.push("--ignore-video-encoder-constraints".to_string());
        }
    }

    args
}

pub fn validate_scrcpy_config(config: &ScrcpyConfig) -> Result<(), String> {
    validate::device(&config.device)?;
    validate::video_codec(config.codec.as_deref())?;
    validate::audio_codec(config.audio_codec.as_deref())?;
    validate::mirror_audio_source(config.mirror_audio_source.as_deref())?;
    validate::mic_source(config.mic_source.as_deref())?;
    validate::mic_codec(config.mic_codec.as_deref())?;
    validate::rotation(config.rotation.as_deref())?;
    validate::fps(config.fps)?;
    validate::bitrate(config.bitrate)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(mode: &str) -> ScrcpyConfig {
        serde_json::from_value(json!({"device": "serial1", "sessionMode": mode}))
            .expect("minimal config")
    }

    #[test]
    fn default_mirror_args_keep_device_and_codec() {
        let args = build_scrcpy_args(&config("mirror"), None, None);
        assert!(args.contains(&"-s".to_string()));
        assert!(args.contains(&"serial1".to_string()));
        assert!(args.contains(&"--video-codec=h264".to_string()));
    }

    #[test]
    fn mic_mode_is_audio_only() {
        let args = build_scrcpy_args(&config("mic"), None, None);
        assert!(args.contains(&"--no-window".to_string()));
        assert!(args.contains(&"--no-video".to_string()));
        assert!(args.contains(&"--audio-source=mic".to_string()));
    }

    #[test]
    fn validation_rejects_unknown_codec() {
        let mut config = config("mirror");
        config.codec = Some("vp9".to_string());
        assert!(validate_scrcpy_config(&config).is_err());
    }
}
