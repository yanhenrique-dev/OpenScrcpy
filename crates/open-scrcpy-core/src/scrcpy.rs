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
