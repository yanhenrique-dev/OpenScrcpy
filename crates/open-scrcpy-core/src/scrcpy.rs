//! `scrcpy --help` discovery: option listing and render-driver support.

use std::time::Duration;

use serde_json::{json, Value};

use crate::{paths, process, validate};

/// Runs `scrcpy -s <device> <arg>` and returns its combined output. Used by
/// the camera/encoder explorers.
pub async fn list_options(device: &str, arg: &str, custom_path: Option<&str>) -> Value {
    if let Err(error) = validate::device(device) {
        return json!({ "success": false, "message": error });
    }
    let exe_path = paths::binary_path("scrcpy", custom_path);

    let mut command = process::create_command(&exe_path);
    command.arg("-s").arg(device).arg(arg);
    let output = process::output_with_timeout(command, Duration::from_secs(15)).await;

    match output {
        Ok(output) => {
            let out_text = String::from_utf8_lossy(&output.stdout);
            let err_text = String::from_utf8_lossy(&output.stderr); // scrcpy often prints lists to stderr
            json!({
                "success": output.status.success(),
                "output": format!("{}{}", out_text, err_text)
            })
        }
        Err(error) => json!({ "success": false, "message": error.to_string() }),
    }
}

/// One lens reported by `scrcpy --list-cameras`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
}

/// Parses both camera-list formats scrcpy has used:
///
/// - `--camera-id=0 (Back camera)` (4.x);
/// - `[0] (1920x1080) Back camera` or `- [0] (...) ...` (older builds).
pub fn parse_cameras(output: &str) -> Vec<CameraInfo> {
    let mut cameras: Vec<CameraInfo> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (id, name) = if let Some(index) = trimmed.find("--camera-id=") {
            let rest = &trimmed[index + "--camera-id=".len()..];
            let id: String = rest
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
                })
                .collect();
            if id.is_empty() {
                continue;
            }
            let label = rest
                .split_once('(')
                .and_then(|(_, tail)| tail.split_once(')'))
                .map(|(label, _)| label.trim().to_string())
                .unwrap_or_else(|| id.clone());
            (id.clone(), format!("{id}: {label}"))
        } else {
            let line = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            let Some(open) = line.find('[') else {
                continue;
            };
            let Some(close_offset) = line[open..].find(']') else {
                continue;
            };
            let close = open + close_offset;
            let id = line[open + 1..close].trim();
            if id.is_empty()
                || !id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                continue;
            }
            let rest = &line[close + 1..];
            let Some(paren_open) = rest.find('(') else {
                continue;
            };
            let Some(paren_close_offset) = rest[paren_open..].find(')') else {
                continue;
            };
            let paren_close = paren_open + paren_close_offset;
            let resolution = rest[paren_open + 1..paren_close].trim();
            let metadata = rest[paren_close + 1..].trim();
            let label = if metadata.is_empty() {
                format!("{id}: Camera ({resolution})")
            } else {
                format!("{id}: {metadata} ({resolution})")
            };
            (id.to_string(), label)
        };

        if !cameras.iter().any(|camera| camera.id == id) {
            cameras.push(CameraInfo { id, name });
        }
    }

    cameras
}

pub fn detect_host_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

pub fn render_driver_label(driver: &str) -> &'static str {
    match driver {
        "direct3d" => "D3D11 (Direct3D)",
        "opengl" => "OpenGL",
        "opengles2" => "OpenGL ES 2",
        "opengles" => "OpenGL ES",
        "metal" => "Metal",
        "software" => "Software",
        "vulkan" => "Vulkan",
        _ => "Custom",
    }
}

pub fn is_driver_allowed_on_os(driver: &str, host_os: &str) -> bool {
    match host_os {
        "windows" => driver != "metal",
        "macos" => driver != "direct3d",
        "linux" => driver != "direct3d" && driver != "metal",
        _ => true,
    }
}

pub fn detect_render_drivers_from_help(help_output: &str) -> Vec<String> {
    let known = [
        "direct3d",
        "opengl",
        "opengles2",
        "opengles",
        "metal",
        "software",
        "vulkan",
    ];

    let lower = help_output.to_lowercase();
    let render_context = if let Some(start) = lower.find("--render-driver") {
        let mut end = (start + 1600).min(lower.len());
        // The 1600-byte window can land in the middle of a multibyte
        // character when a custom build prints localized help; slicing there
        // would panic.
        while end > start && !lower.is_char_boundary(end) {
            end -= 1;
        }
        lower[start..end].to_string()
    } else {
        String::new()
    };

    known
        .iter()
        .filter(|driver| {
            let quoted = render_context.contains(&format!("\"{}\"", driver))
                || render_context.contains(&format!("'{}'", driver));
            // Match whole tokens too: "opengl" must not be detected just
            // because the help mentions "opengles2".
            let token = render_context
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|part| part == **driver);
            quoted || token
        })
        .map(|driver| (*driver).to_string())
        .collect()
}

/// Asks the resolved scrcpy binary which render drivers it advertises, then
/// filters them by host OS. A build without `--render-driver` is a normal
/// result, not an error.
pub async fn get_render_drivers(custom_path: Option<&str>) -> Value {
    let exe_path = paths::binary_path("scrcpy", custom_path);
    let host_os = detect_host_os();

    let mut cmd = process::create_command(&exe_path);
    cmd.arg("--help");
    let output = process::output_with_timeout(cmd, Duration::from_secs(5)).await;

    match output {
        Ok(output) => {
            let out_text = String::from_utf8_lossy(&output.stdout);
            let err_text = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", out_text, err_text);
            let lower = combined.to_lowercase();

            if !lower.contains("--render-driver") {
                return json!({
                    "success": true,
                    "hostOs": host_os,
                    "supportsRenderDriver": false,
                    "detectedDrivers": [],
                    "supportedDrivers": [],
                    "diagnostics": "scrcpy does not advertise --render-driver in --help output"
                });
            }

            let detected_drivers = detect_render_drivers_from_help(&combined);
            let supported_drivers: Vec<Value> = detected_drivers
                .iter()
                .filter(|driver| is_driver_allowed_on_os(driver, host_os))
                .map(|driver| {
                    json!({
                        "id": driver,
                        "label": render_driver_label(driver)
                    })
                })
                .collect();

            json!({
                "success": output.status.success(),
                "hostOs": host_os,
                "supportsRenderDriver": true,
                "detectedDrivers": detected_drivers,
                "supportedDrivers": supported_drivers,
                "diagnostics": "render drivers parsed from scrcpy --help"
            })
        }
        Err(error) => json!({
            "success": false,
            "hostOs": host_os,
            "supportsRenderDriver": false,
            "detectedDrivers": [],
            "supportedDrivers": [],
            "message": error.to_string()
        }),
    }
}

/// Parses `scrcpy --version` from the resolved binary. Returns `None` on a
/// missing/broken binary or if the output has no recognizable version token.
pub async fn local_version(custom_path: Option<&str>) -> Option<String> {
    let exe_path = paths::binary_path("scrcpy", custom_path);
    let mut cmd = process::create_command(&exe_path);
    cmd.arg("--version");
    let output = process::output_with_timeout(cmd, Duration::from_secs(3))
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let out_str = String::from_utf8_lossy(&output.stdout);
    for line in out_str.lines() {
        if line.contains("scrcpy") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(idx) = parts.iter().position(|&part| part == "scrcpy") {
                if let Some(version) = parts.get(idx + 1) {
                    return Some(version.trim_start_matches('v').to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_new_camera_list_format() {
        let output = "Cameras:\n--camera-id=0 (Back camera)\n--camera-id=1 (Front camera)\n";
        let cameras = parse_cameras(output);
        assert_eq!(cameras.len(), 2);
        assert_eq!(cameras[0].id, "0");
        assert_eq!(cameras[0].name, "0: Back camera");
        assert_eq!(cameras[1].id, "1");
    }

    #[test]
    fn parses_the_legacy_camera_list_format() {
        let output =
            "* daemon not running\n[0] (1920x1080) Back camera\n- [2] (1280x720) Ultra-wide\n";
        let cameras = parse_cameras(output);
        assert_eq!(cameras.len(), 2);
        assert_eq!(cameras[0].id, "0");
        assert!(cameras[0].name.contains("Back camera"));
        assert!(cameras[0].name.contains("1920x1080"));
        assert_eq!(cameras[1].id, "2");
    }

    #[test]
    fn ignores_noise_and_duplicate_ids() {
        let output = "no cameras here\n--camera-id=0 (A)\n--camera-id=0 (B)\n";
        let cameras = parse_cameras(output);
        assert_eq!(cameras.len(), 1);
        assert_eq!(cameras[0].name, "0: A");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_version_times_out_on_hung_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("openscrcpy-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fake = dir.join("scrcpy");
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").expect("fake scrcpy");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let path = dir.to_string_lossy().to_string();
        let started = std::time::Instant::now();
        let version = local_version(Some(&path)).await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(version.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn drivers_handle_multibyte_help_and_whole_tokens() {
        // A multibyte character straddling the 1600-byte window used to panic.
        let help = format!("--render-driver \"opengl\" {}é", "a".repeat(1574));
        assert!(detect_render_drivers_from_help(&help).contains(&"opengl".to_string()));

        // "opengles2" must not be reported as plain "opengl".
        assert_eq!(
            detect_render_drivers_from_help("--render-driver=\"opengles2\""),
            vec!["opengles2".to_string()]
        );
    }
}
